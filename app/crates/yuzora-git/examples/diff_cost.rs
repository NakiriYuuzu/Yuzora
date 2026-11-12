//! What opening a file's unstaged diff costs before the pane has its rows:
//! reading both sides (`DiffSession::load`), the first view (the rows, and
//! with them the parse of both sides for their syntax colours), the view
//! again (the parse is kept) and handing the rows to the script engine.
//!
//!   YUZORA_APP_HOME=<a temporary folder> cargo run -p yuzora-git --example diff_cost \
//!     --config 'profile.dev.package.yuzora-git.opt-level=2' \
//!     --config 'profile.dev.package.yuzora-files.opt-level=2' -- <repo> <path> [runs] [context]
//!
//! (Both crates at opt-level 2, as the dev profile builds every dependency; the
//! parse runs in `yuzora-files`.)
//!
//! One line a run, in milliseconds, then the median and the range of each.
//! `context` is the lines kept around a change (3, as a diff opens; -1 for the
//! whole file). A session is loaded anew for every run, so nothing is kept
//! from one to the next but what the system caches of the files.

use std::time::Instant;

use yuzora_git::diff::Whitespace;
use yuzora_git::diffview::{DiffSession, Mode};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [repo, path, rest @ ..] = args.as_slice() else {
        eprintln!("usage: diff_cost <repo> <path> [runs] [context]");
        std::process::exit(2);
    };
    let runs: usize = rest.first().and_then(|n| n.parse().ok()).unwrap_or(7);
    let context: i64 = rest.get(1).and_then(|n| n.parse().ok()).unwrap_or(3);
    let repo = yuzora_git::repo::open(repo, None).expect("a repository");
    let ms = |from: Instant| from.elapsed().as_secs_f64() * 1000.;
    let mut columns: [Vec<f64>; 4] = Default::default();
    println!("{path}, context {context}");
    println!("{:>8} {:>8} {:>8} {:>8} {:>8} {:>8} {:>10}", "load", "view", "again", "to host", "rows", "marks", "json bytes");
    for _ in 0..runs {
        let at = Instant::now();
        let session = DiffSession::load(repo.clone(), path.clone(), None, Mode::Unstaged).expect("a diff");
        let load = ms(at);
        let at = Instant::now();
        let view = session.view("side", context, Whitespace::Exact);
        let first = ms(at);
        let at = Instant::now();
        let again = session.view("side", context, Whitespace::Exact);
        let second = ms(at);
        assert_eq!(view, again, "a view is the same each time");
        let at = Instant::now();
        let handed = yuzora_core::host::to_host(&view);
        let host = ms(at);
        drop(handed);
        let rows = view["rows"].as_array().expect("rows");
        assert!(!rows.is_empty(), "no rows: is the file changed?");
        let marks: usize = rows.iter().map(|row| ["m", "lm", "rm"].iter().filter_map(|key| row.get(*key)?.as_array()).map(Vec::len).sum::<usize>()).sum();
        println!("{load:8.1} {first:8.1} {second:8.1} {host:8.1} {:8} {marks:8} {:10}", rows.len(), view.to_string().len());
        for (column, value) in columns.iter_mut().zip([load, first, second, host]) {
            column.push(value);
        }
    }
    for (name, mut column) in ["load", "view", "again", "to host"].into_iter().zip(columns) {
        column.sort_by(f64::total_cmp);
        println!("{name:>8}: median {:.1} ms, {:.1} to {:.1} ({} runs)", column[column.len() / 2], column[0], column[column.len() - 1], column.len());
    }
}
