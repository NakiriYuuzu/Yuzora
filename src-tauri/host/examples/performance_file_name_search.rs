//! Isolated filesystem search benchmark. No host service, Git writes or user files.
//! cargo run --locked --release --manifest-path src-tauri/host/Cargo.toml --example performance_file_name_search
use std::path::Path;
use std::sync::atomic::AtomicU64;
use std::time::{Duration, Instant};
use yuzora_host::file_name_search::run_file_name_search;

#[cfg(unix)]
fn process_usage() -> (Option<f64>, Option<i64>) {
    // getrusage initializes the plain C record on success.
    let usage = unsafe {
        let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
        assert_eq!(libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()), 0);
        usage.assume_init()
    };
    let cpu_ms = (usage.ru_utime.tv_sec + usage.ru_stime.tv_sec) as f64 * 1000.0
        + (usage.ru_utime.tv_usec + usage.ru_stime.tv_usec) as f64 / 1000.0;
    (Some(cpu_ms), Some(usage.ru_maxrss))
}

#[cfg(not(unix))]
fn process_usage() -> (Option<f64>, Option<i64>) {
    (None, None)
}

fn descriptor_count() -> Option<usize> {
    let path = if cfg!(target_os = "linux") {
        "/proc/self/fd"
    } else {
        "/dev/fd"
    };
    std::fs::read_dir(path).ok().map(|entries| entries.count())
}

fn populate(root: &Path) {
    for directory in 0..120 {
        let path = root.join(format!("module-{directory:03}"));
        std::fs::create_dir(&path).unwrap();
        for file in 0..50 {
            std::fs::write(path.join(format!("file-{file:03}.ts")), b"fixture\n").unwrap();
        }
        std::fs::write(path.join("資料檔.ts"), b"fixture\n").unwrap();
    }
}

fn main() {
    assert!(!cfg!(debug_assertions), "Run this benchmark with --release");
    let fixture = tempfile::tempdir().unwrap();
    populate(fixture.path());
    let generation = AtomicU64::new(1);
    let queries = [
        ("absent", "no-such-fixture", 0, false),
        ("unique", "module-099/file-049.ts", 1, false),
        ("unicode", "資料", 120, false),
        ("case-and-separators", "MODULE-099\\FILE-049.TS", 1, false),
        ("early-cap", "file-", 200, true),
    ];
    println!(
        "{}",
        serde_json::json!({
            "kind": "environment", "profile": "release", "platform": std::env::consts::OS,
            "directories": 120, "files": 6120, "warmRepetitions": 7,
            "peakRssUnits": if cfg!(target_os = "macos") { "bytes" } else if cfg!(target_os = "linux") { "KiB" } else { "platform dependent or unavailable" },
            "note": "Fixture creation precedes measurement. First pass is not an OS cold-cache claim. Peak RSS is process-wide high water; no forced GC or restart between samples."
        })
    );
    for (name, query, matches, incomplete) in queries {
        for pass in 0..8 {
            let descriptors_before = descriptor_count();
            let (cpu_before, _) = process_usage();
            let started = Instant::now();
            let result = run_file_name_search(
                fixture.path(),
                query,
                1,
                &generation,
                Duration::from_secs(10),
            )
            .unwrap();
            let wall_ms = started.elapsed().as_secs_f64() * 1000.0;
            let (cpu_after, peak_rss) = process_usage();
            let descriptors_after = descriptor_count();
            assert_eq!(result.files.len(), matches);
            assert_eq!(result.incomplete, incomplete);
            assert_eq!(descriptors_after, descriptors_before);
            println!(
                "{}",
                serde_json::json!({
                    "kind": "sample", "scenario": name, "pass": pass,
                    "firstPass": pass == 0, "wallMs": wall_ms,
                    "cpuMs": cpu_after.zip(cpu_before).map(|(after, before)| after - before),
                    "peakRss": peak_rss, "descriptorsBefore": descriptors_before,
                    "descriptorsAfter": descriptors_after,
                    "matches": result.files.len(), "incomplete": result.incomplete
                })
            );
        }
    }
}
