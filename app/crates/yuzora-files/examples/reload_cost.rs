//! What one reload of the Files sidebar costs, with no window: the same calls
//! the sidebar's mod makes (`gitStatus`, `listDir`), each on a thread of its own
//! as the host runs them, in the order the mod puts them in.
//!
//! ```sh
//! cargo run --release -p yuzora-files --example reload_cost -- \
//!   --root /path/to/repo [--on machine] [--open src,src/app] [-n 20] \
//!   [--only serial|parallel] [--hc /path/to/hc-wrapper]
//! ```
//!
//! `serial` is what the mod did before the tree stopped waiting for git: git
//! status, then the root, then every open folder one after the other.
//! `parallel` starts git and every listing at once, at most `CAP` listings in
//! flight, as the mod does now. The two run alternately (A, B, A, B), so a busy
//! machine costs both the same; `--only` runs one, for counting processes.
//!
//! Two times are printed for each run, in milliseconds from the start of the reload:
//! `t_tree`, when the root's rows could be drawn, and `t_full`, when the last open
//! folder is listed and git's marks are in. The output is one JSON object with the
//! raw samples of each side (the first pair of runs is dropped as a warm-up).

use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Instant;

use yuzora_files::{fsops, git};

/// How many listings the mod keeps in flight (`LISTINGS` in `tree-state.js`).
const CAP: usize = 6;
const PER_FOLDER: usize = 2000;
const MARKS: usize = 20_000;

struct Args {
    root: String,
    on: Option<String>,
    open: Vec<String>,
    runs: usize,
    only: Option<bool>,
    sub: Option<String>,
}

fn parse() -> Args {
    let mut args = Args {
        root: String::new(),
        on: None,
        open: Vec::new(),
        runs: 20,
        only: None,
        sub: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut value = || it.next().unwrap_or_else(|| panic!("{flag} needs a value"));
        match flag.as_str() {
            "--root" => args.root = value(),
            "--on" => args.on = Some(value()),
            "--open" => {
                args.open = value()
                    .split(',')
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect()
            }
            "-n" => args.runs = value().parse().expect("-n takes a number"),
            "--only" => args.only = Some(value() == "parallel"),
            "--sub" => args.sub = Some(value()),
            "--hc" => {
                let hc = PathBuf::from(value());
                yuzora_core::remote::set_hc_program(move || hc.clone());
            }
            other => panic!("unknown flag {other}"),
        }
    }
    assert!(!args.root.is_empty(), "--root is required");
    args
}

fn list(root: &str, folder: &str, on: Option<&str>) {
    let path = if folder.is_empty() {
        root.to_owned()
    } else {
        format!("{root}/{folder}")
    };
    fsops::list_dir(&path, on, PER_FOLDER).unwrap_or_else(|e| panic!("list {path}: {e}"));
}

fn serial(a: &Args) -> (f64, f64) {
    let start = Instant::now();
    let on = a.on.as_deref();
    let _ = git::status(&a.root, on, MARKS);
    list(&a.root, "", on);
    let tree = start.elapsed().as_secs_f64() * 1000.0;
    for folder in &a.open {
        list(&a.root, folder, on);
    }
    (tree, start.elapsed().as_secs_f64() * 1000.0)
}

/// Runs `work` on every folder of `folders`, on `workers` threads that each take the next one when
/// they are free: the listings the mod keeps in flight (`listAll` in `tree-state.js`).
fn spread<F>(folders: Vec<String>, workers: usize, work: F) -> Vec<thread::JoinHandle<()>>
where
    F: Fn(String) + Send + Sync + 'static,
{
    let queue = Arc::new(Mutex::new(folders.into_iter().rev().collect::<Vec<_>>()));
    let work = Arc::new(work);
    (0..workers)
        .map(|_| {
            let (queue, work) = (queue.clone(), work.clone());
            thread::spawn(move || {
                loop {
                    // The guard is a temporary: it has to be gone before `work`, or the workers take turns.
                    let next = queue.lock().unwrap().pop();
                    let Some(folder) = next else { break };
                    work(folder);
                }
            })
        })
        .collect()
}

fn parallel(a: &Args) -> (f64, f64) {
    let start = Instant::now();
    let git_done = {
        let (root, on) = (a.root.clone(), a.on.clone());
        thread::spawn(move || {
            let _ = git::status(&root, on.as_deref(), MARKS);
        })
    };
    let folders: Vec<String> = std::iter::once(String::new())
        .chain(a.open.iter().cloned())
        .collect();
    let (tx, rx) = mpsc::channel::<(String, f64)>();
    let (root, on) = (a.root.clone(), a.on.clone());
    let workers = spread(folders, CAP.min(a.open.len() + 1), move |folder| {
        list(&root, &folder, on.as_deref());
        let _ = tx.send((folder, start.elapsed().as_secs_f64() * 1000.0));
    });
    let mut tree = f64::NAN;
    for (folder, at) in rx {
        if folder.is_empty() {
            tree = at;
        }
    }
    for w in workers {
        w.join().unwrap();
    }
    git_done.join().unwrap();
    (tree, start.elapsed().as_secs_f64() * 1000.0)
}

/// The first view of `sub` inside the repo at `a.root`, in the three orders the sidebar has used.
fn first_view(a: &Args, order: &str) -> (f64, f64) {
    let start = Instant::now();
    let on = a.on.as_deref();
    let sub = a.sub.as_deref().unwrap();
    let cwd = format!("{}/{sub}", a.root);
    let ms = || start.elapsed().as_secs_f64() * 1000.0;
    match order {
        "serial" => {
            let _ = git::status(&cwd, on, MARKS);
            list(&a.root, "", on);
            let t = ms();
            (t, t)
        }
        "previous" => {
            let git_done = {
                let (cwd, on) = (cwd.clone(), a.on.clone());
                thread::spawn(move || {
                    let _ = git::status(&cwd, on.as_deref(), MARKS);
                })
            };
            list(&cwd, "", on);
            let tree = ms();
            git_done.join().unwrap();
            list(&a.root, "", on);
            (tree, ms())
        }
        _ => {
            let top = {
                let (root, cwd, on) = (a.root.clone(), cwd.clone(), a.on.clone());
                thread::spawn(move || {
                    let _ = git::status(&cwd, on.as_deref(), MARKS);
                    list(&root, "", on.as_deref());
                    start.elapsed().as_secs_f64() * 1000.0
                })
            };
            list(&cwd, "", on);
            let tree = ms();
            (tree, top.join().unwrap())
        }
    }
}

fn main() {
    let a = parse();
    if a.sub.is_some() {
        let orders = ["serial", "previous", "now"];
        for o in orders {
            first_view(&a, o);
        }
        let mut samples: Vec<(Vec<f64>, Vec<f64>)> =
            orders.iter().map(|_| (Vec::new(), Vec::new())).collect();
        for _ in 0..a.runs {
            for (i, o) in orders.iter().enumerate() {
                let (t, f) = first_view(&a, o);
                samples[i].0.push(t);
                samples[i].1.push(f);
            }
        }
        let fmt = |v: &[f64]| {
            v.iter()
                .map(|x| format!("{x:.1}"))
                .collect::<Vec<_>>()
                .join(",")
        };
        let parts: Vec<String> = orders
            .iter()
            .enumerate()
            .map(|(i, o)| {
                format!(
                    "\"{o}\":{{\"t_tree\":[{}],\"t_full\":[{}]}}",
                    fmt(&samples[i].0),
                    fmt(&samples[i].1)
                )
            })
            .collect();
        println!(
            "{{\"on\":{},{}}}",
            a.on.as_ref().map_or("null".into(), |m| format!("\"{m}\"")),
            parts.join(",")
        );
        return;
    }
    let sides: Vec<bool> = a.only.map_or(vec![false, true], |p| vec![p]);
    let run = |parallel_side: bool| {
        if parallel_side {
            parallel(&a)
        } else {
            serial(&a)
        }
    };
    for side in &sides {
        run(*side);
    }
    let mut samples: Vec<(Vec<f64>, Vec<f64>)> =
        sides.iter().map(|_| (Vec::new(), Vec::new())).collect();
    for _ in 0..a.runs {
        for (i, side) in sides.iter().enumerate() {
            let (t, f) = run(*side);
            samples[i].0.push(t);
            samples[i].1.push(f);
        }
    }
    let fmt = |v: &[f64]| {
        v.iter()
            .map(|x| format!("{x:.1}"))
            .collect::<Vec<_>>()
            .join(",")
    };
    let side = |i: usize| {
        format!(
            "{{\"t_tree\":[{}],\"t_full\":[{}]}}",
            fmt(&samples[i].0),
            fmt(&samples[i].1)
        )
    };
    let mut parts = Vec::new();
    for (i, parallel_side) in sides.iter().enumerate() {
        parts.push(format!(
            "\"{}\":{}",
            if *parallel_side { "parallel" } else { "serial" },
            side(i)
        ));
    }
    println!(
        "{{\"on\":{},\"open\":{},{}}}",
        a.on.as_ref().map_or("null".into(), |m| format!("\"{m}\"")),
        a.open.len(),
        parts.join(","),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Barrier;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[test]
    fn the_workers_list_at_once() {
        // Each of CAP listings waits for the other CAP - 1: they end only if all are under way together.
        let barrier = Arc::new(Barrier::new(CAP));
        let folders = (0..CAP).map(|i| i.to_string()).collect();
        let workers = spread(folders, CAP, move |_| {
            barrier.wait();
        });
        let (done, ended) = mpsc::channel();
        thread::spawn(move || {
            workers.into_iter().for_each(|w| w.join().unwrap());
            let _ = done.send(());
        });
        ended
            .recv_timeout(Duration::from_secs(5))
            .expect("the workers list one after the other");
    }

    #[test]
    fn every_folder_is_listed_once() {
        let listed = Arc::new(AtomicUsize::new(0));
        let count = listed.clone();
        let folders = (0..25).map(|i| i.to_string()).collect();
        for w in spread(folders, 3, move |_| {
            count.fetch_add(1, Ordering::SeqCst);
        }) {
            w.join().unwrap();
        }
        assert_eq!(listed.load(Ordering::SeqCst), 25);
    }
}
