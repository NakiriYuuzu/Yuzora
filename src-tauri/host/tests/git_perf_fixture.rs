//! Large-repository fixture and status timings. The default test is a tiny smoke
//! run; the heavy measurement is `#[ignore]`:
//! `cargo test --locked --manifest-path host/Cargo.toml --test git_perf_fixture -- --ignored --nocapture`
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use yuzora_host::git_service::{run_git, set_logger, status_of, DEFAULT_TIMEOUT};

static COMMANDS: AtomicUsize = AtomicUsize::new(0);

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn write_tree(root: &Path, prefix: &str, count: usize, fan: usize) {
    for index in 0..count {
        let dir = root.join(format!(
            "{prefix}/d{}/d{}/d{}",
            index % fan,
            (index / fan) % fan,
            (index / (fan * fan)) % fan
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(format!("f{index}.txt")),
            format!("{prefix} {index}\n"),
        )
        .unwrap();
    }
}

/// `tracked` committed files, `untracked` loose files, `ignored` files under an
/// ignored `target/`, plus staged and unstaged edits.
fn build_fixture(root: &Path, tracked: usize, untracked: usize, ignored: usize) {
    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["config", "user.name", "Git performance fixture"]);
    git(root, &["config", "user.email", "fixture@example.invalid"]);
    std::fs::write(root.join(".gitignore"), "target/\n").unwrap();
    write_tree(root, "src", tracked, 12);
    git(root, &["add", "-A"]);
    git(
        root,
        &[
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "-m",
            "fixture",
        ],
    );
    write_tree(root, "target", ignored, 10);
    write_tree(root, "scratch", untracked, 8);
    for index in 0..40.min(tracked) {
        let file = format!(
            "src/d{}/d{}/d{}/f{index}.txt",
            index % 12,
            (index / 12) % 12,
            (index / 144) % 12
        );
        std::fs::write(root.join(&file), "changed\n").unwrap();
        if index % 2 == 0 {
            git(root, &["add", &file]);
            std::fs::write(root.join(&file), "changed again\n").unwrap();
        }
    }
}

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    sorted[(((sorted.len() - 1) as f64) * p).round() as usize]
}

fn summarize(label: &str, mut samples: Vec<Duration>) -> Duration {
    samples.sort();
    let p50 = percentile(&samples, 0.5);
    println!(
        "{label}: p50={:.1}ms p95={:.1}ms (n={})",
        p50.as_secs_f64() * 1e3,
        percentile(&samples, 0.95).as_secs_f64() * 1e3,
        samples.len()
    );
    p50
}

fn time(run: &mut impl FnMut()) -> Duration {
    let start = Instant::now();
    run();
    start.elapsed()
}

/// Alternates A and B so machine noise hits both equally.
fn compare(label: &str, runs: usize, mut a: impl FnMut(), mut b: impl FnMut()) {
    a();
    b();
    let (mut sa, mut sb) = (Vec::new(), Vec::new());
    for _ in 0..runs {
        sa.push(time(&mut a));
        sb.push(time(&mut b));
    }
    let base = summarize(&format!("{label} [default]"), sa);
    let alt = summarize(&format!("{label} [candidate]"), sb);
    println!(
        "  candidate vs default p50: {:+.1}%",
        (alt.as_secs_f64() / base.as_secs_f64() - 1.0) * 100.0
    );
}

#[test]
fn small_fixture_status_reflects_staged_unstaged_and_untracked_changes() {
    let dir = tempfile::tempdir().unwrap();
    build_fixture(dir.path(), 60, 10, 20);
    let status = status_of(dir.path(), None).unwrap().parsed;
    assert_eq!(status.untracked.len(), 10);
    assert!(!status.staged.is_empty() && !status.unstaged.is_empty());
}

#[test]
#[ignore = "heavy: builds ~53k files; run with -- --ignored --nocapture"]
fn measure_status_on_large_fixture() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let built = Instant::now();
    build_fixture(root, 30_000, 3_000, 20_000);
    println!("fixture built in {:.1}s", built.elapsed().as_secs_f64());
    set_logger(|_, _, _| {
        COMMANDS.fetch_add(1, Ordering::Relaxed);
    });
    let runs = 25;
    COMMANDS.store(0, Ordering::Relaxed);
    status_of(root, None).unwrap();
    println!(
        "status_of processes per call: {}",
        COMMANDS.load(Ordering::Relaxed)
    );
    let mut samples = Vec::new();
    for _ in 0..runs {
        samples.push(time(&mut || {
            status_of(root, None).unwrap();
        }));
    }
    summarize("status_of", samples);
    let mut samples = Vec::new();
    for _ in 0..runs {
        samples.push(time(&mut || {
            run_git(root, &["rev-parse", "--git-dir"], DEFAULT_TIMEOUT, &[]).unwrap();
        }));
    }
    summarize("rev-parse --git-dir (the removed process)", samples);

    let args = [
        "status",
        "--porcelain=v2",
        "--branch",
        "--untracked-files=all",
        "-z",
    ];
    let run = |before: &[&str], after: &[&str]| {
        let mut full: Vec<&str> = before.to_vec();
        full.extend(args);
        full.extend(after);
        let out = run_git(root, &full, DEFAULT_TIMEOUT, &[]).unwrap();
        assert_eq!(out.code, 0);
        out
    };
    let reference = run(&[], &[]).stdout;
    let candidates: [(&str, &[&str], &[&str]); 2] = [
        (
            "core.untrackedCache=true",
            &["-c", "core.untrackedCache=true"],
            &[],
        ),
        ("--no-renames", &[], &["--no-renames"]),
    ];
    for (label, before, after) in candidates {
        println!(
            "{label}: identical output {}",
            run(before, after).stdout == reference
        );
        compare(
            &format!("git status {label}"),
            runs,
            || {
                run(&[], &[]);
            },
            || {
                run(before, after);
            },
        );
    }

    watcher_storm(root, 5_000);
}

/// Real watcher: an ignored-directory build storm, then one tracked edit.
fn watcher_storm(root: &Path, files: usize) {
    let batches = std::sync::Arc::new(std::sync::Mutex::new(Vec::<(Vec<String>, bool)>::new()));
    let seen = batches.clone();
    let _watcher = yuzora_host::watcher::build_classified_watcher(root, move |paths, relevant| {
        seen.lock().unwrap().push((paths, relevant));
    })
    .unwrap();
    std::thread::sleep(Duration::from_millis(500));
    for index in 0..files {
        let dir = root.join(format!("target/storm/d{}", index % 50));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("o{index}.o")), "x").unwrap();
    }
    std::thread::sleep(Duration::from_secs(2));
    let storm = batches.lock().unwrap().clone();
    println!(
        "ignored storm ({files} files in target/): {} callbacks, {} would refresh git status",
        storm.len(),
        storm.iter().filter(|(_, relevant)| *relevant).count()
    );
    for (paths, relevant) in storm.iter().filter(|(_, relevant)| *relevant) {
        println!("  relevant batch: {paths:?} {relevant}");
    }
    batches.lock().unwrap().clear();
    std::fs::write(
        root.join("src/d0/d0/d0/f0.txt"),
        "edited during storm test\n",
    )
    .unwrap();
    std::thread::sleep(Duration::from_secs(2));
    let edit = batches.lock().unwrap().clone();
    println!("tracked edit: {edit:?}");
    assert!(edit.iter().any(|(_, relevant)| *relevant));
}

#[test]
#[ignore = "timing-based (FS events); run with -- --ignored --nocapture"]
fn ignored_storm_on_small_fixture() {
    let dir = tempfile::tempdir().unwrap();
    build_fixture(dir.path(), 60, 10, 20);
    watcher_storm(dir.path(), 500);
}
