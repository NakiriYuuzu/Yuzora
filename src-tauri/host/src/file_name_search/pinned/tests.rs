use super::*;
use crate::files::WorkspaceFiles;

fn pin(root: &Path) -> PinnedSearchRoot {
    let mut files = WorkspaceFiles::default();
    let opened = files.open(root.to_str().unwrap()).unwrap();
    files
        .file_name_search_root(opened["capabilityId"].as_str().unwrap())
        .unwrap()
}

fn write(root: &Path, path: &str, content: &str) {
    let path = root.join(path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

fn link_directory(target: &Path, link: &Path) {
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).unwrap();
    #[cfg(windows)]
    {
        // Junctions need no symlink privilege on the Windows Host.
        let output = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
    }
}

#[test]
fn file_name_search_root_swap_after_capability_check_barrier() {
    use std::sync::{Arc, Barrier};
    for nonempty in [false, true] {
        let tmp = tempfile::tempdir().unwrap();
        let root_path = tmp.path().join("workspace");
        let outside = tmp.path().join("outside");
        std::fs::create_dir(&root_path).unwrap();
        if nonempty {
            write(&root_path, "child/inside-target.ts", "");
            write(&root_path, ".ignore", "blocked-target.ts\n");
            write(&root_path, "blocked-target.ts", "");
        }
        write(&outside, "outside-secret-target.ts", "");
        let root = pin(&root_path);
        let canonical = root.canonical.clone();
        let barrier = Arc::new(Barrier::new(2));
        let worker_barrier = barrier.clone();
        let worker = std::thread::spawn(move || {
            // WorkspaceFiles::get has verified identity; traversal waits for the swap.
            worker_barrier.wait();
            run_pinned_file_name_search(
                &root,
                "target",
                1,
                &AtomicU64::new(1),
                Duration::from_secs(5),
            )
            .unwrap()
        });
        std::fs::rename(&root_path, tmp.path().join("original")).unwrap();
        link_directory(&outside, &root_path);
        barrier.wait();
        let result = worker.join().unwrap();
        assert!(
            !result
                .files
                .iter()
                .any(|file| file.name.contains("outside")),
            "outside names leaked: {:?}",
            result.files
        );
        assert!(!result.incomplete);
        if nonempty {
            assert_eq!(result.files.len(), 1);
            assert_eq!(result.files[0].name, "inside-target.ts");
            assert_eq!(
                result.files[0].path,
                canonical.join("child/inside-target.ts").to_str().unwrap()
            );
        } else {
            assert!(result.files.is_empty());
        }
    }
}

#[test]
fn file_name_search_child_swapped_after_listing_is_not_followed() {
    let tmp = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    write(tmp.path(), "child/inside-target.ts", "");
    write(outside.path(), "outside-secret-target.ts", "");
    let root = pin(tmp.path());
    let mut swapped = false;
    let result = search(
        &root,
        "target",
        1,
        &AtomicU64::new(1),
        Duration::from_secs(5),
        &mut |relative| {
            assert_eq!(relative, Path::new("child"));
            std::fs::rename(tmp.path().join("child"), tmp.path().join("original-child")).unwrap();
            link_directory(outside.path(), &tmp.path().join("child"));
            swapped = true;
        },
    )
    .unwrap();
    assert!(swapped);
    assert!(result.files.is_empty());
    assert!(result.incomplete);
}

#[test]
fn file_name_search_open_child_stays_pinned_when_ancestor_is_swapped() {
    let tmp = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    write(tmp.path(), "child/deep/inside-target.ts", "");
    write(outside.path(), "deep/outside-secret-target.ts", "");
    let root = pin(tmp.path());
    let mut swapped = false;
    let result = search(
        &root,
        "target",
        1,
        &AtomicU64::new(1),
        Duration::from_secs(5),
        &mut |relative| {
            if relative == Path::new("child/deep") {
                std::fs::rename(tmp.path().join("child"), tmp.path().join("original-child"))
                    .unwrap();
                link_directory(outside.path(), &tmp.path().join("child"));
                swapped = true;
            }
        },
    )
    .unwrap();
    assert!(swapped);
    assert!(!result.incomplete);
    assert_eq!(result.files.len(), 1);
    assert_eq!(result.files[0].name, "inside-target.ts");
    assert_eq!(
        result.files[0].path,
        root.canonical
            .join("child/deep/inside-target.ts")
            .to_str()
            .unwrap()
    );
}

#[test]
fn file_name_search_nested_ignore_precedence_matches_local_walker() {
    let tmp = tempfile::tempdir().unwrap();
    write(
        tmp.path(),
        ".git/info/exclude",
        "\u{feff}*exclude-target*\r\n",
    );
    write(tmp.path(), ".gitignore", "\u{feff}*git-target*\r\n");
    write(
        tmp.path(),
        ".ignore",
        "*ignore-target*\n!child/keep-git-target.ts\n",
    );
    write(
        tmp.path(),
        "child/.gitignore",
        "!keep-git-target.ts\n!blocked-ignore-target.ts\n!keep-exclude-target.ts\nnested-target/\n",
    );
    write(
        tmp.path(),
        "child/.ignore",
        "\u{feff}!keep-ignore-target.ts\r\n/anchored-target.ts\r\n",
    );
    for path in [
        "root-git-target.ts",
        "root-ignore-target.ts",
        "root-exclude-target.ts",
        "child/keep-git-target.ts",
        "child/keep-ignore-target.ts",
        "child/keep-exclude-target.ts",
        "child/blocked-ignore-target.ts",
        "child/nested-target/file.ts",
        "child/anchored-target.ts",
        "child/deep/anchored-target.ts",
        ".hidden-target.ts",
        "child/ordinary-target.ts",
    ] {
        write(tmp.path(), path, "");
    }
    let root = pin(tmp.path());
    let source = AtomicU64::new(1);
    let remote =
        run_pinned_file_name_search(&root, "target", 1, &source, Duration::from_secs(5)).unwrap();
    let local = super::super::run_file_name_search(
        &root.canonical,
        "target",
        1,
        &source,
        Duration::from_secs(5),
    )
    .unwrap();
    assert!(!remote.incomplete);
    assert_eq!(remote.files.len(), 6);
    assert_eq!(
        serde_json::to_value(remote).unwrap(),
        serde_json::to_value(local).unwrap()
    );
}

#[cfg(unix)]
#[test]
fn file_name_search_ignore_symlink_is_not_read() {
    let tmp = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    write(tmp.path(), "target.ts", "");
    write(outside.path(), "ignore", "!target.ts\n");
    std::os::unix::fs::symlink(outside.path().join("ignore"), tmp.path().join(".ignore")).unwrap();
    let result = run_pinned_file_name_search(
        &pin(tmp.path()),
        "target",
        1,
        &AtomicU64::new(1),
        Duration::from_secs(5),
    )
    .unwrap();
    assert!(result.incomplete);
    assert!(result.files.is_empty());
}

#[test]
fn file_name_search_pinned_time_budget_and_cancellation() {
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "child/target.ts", "");
    let root = pin(tmp.path());
    let source = AtomicU64::new(1);
    let result = run_pinned_file_name_search(&root, "target", 1, &source, Duration::ZERO).unwrap();
    assert!(result.incomplete);
    assert!(result.files.is_empty());
    assert_eq!(
        search(
            &root,
            "target",
            1,
            &source,
            Duration::from_secs(5),
            &mut |_| {
                source.store(2, Ordering::Relaxed);
            }
        )
        .unwrap_err(),
        "file-name-search-cancelled"
    );
    assert_eq!(
        run_pinned_file_name_search(&root, "target", 1, &source, Duration::from_secs(5))
            .unwrap_err(),
        "file-name-search-cancelled"
    );
}

#[test]
fn file_name_search_pinned_entry_budget() {
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "target.ts", "");
    let source = AtomicU64::new(1);
    let mut budget = Budget {
        started: Instant::now(),
        time: Duration::from_secs(5),
        remaining: 0,
        generation: 1,
        source: &source,
    };
    assert!(budget.entries(&pin(tmp.path()).dir).is_err());
}

#[test]
fn file_name_search_nested_repository_excludes_match_local_walker() {
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), ".gitignore", "*parent-target*\n");
    write(tmp.path(), ".ignore", "*ignore-target*\n");
    write(tmp.path(), ".git/info/exclude", "*outer-target*\n");
    write(tmp.path(), "child/.git/info/exclude", "*inner-target*\n");
    for path in [
        "parent-target.ts",
        "outer-target.ts",
        "child/parent-target.ts",
        "child/outer-target.ts",
        "child/inner-target.ts",
        "child/ignore-target.ts",
        "child/ordinary-target.ts",
    ] {
        write(tmp.path(), path, "");
    }
    let root = pin(tmp.path());
    let source = AtomicU64::new(1);
    let remote =
        run_pinned_file_name_search(&root, "target", 1, &source, Duration::from_secs(5)).unwrap();
    let local = super::super::run_file_name_search(
        &root.canonical,
        "target",
        1,
        &source,
        Duration::from_secs(5),
    )
    .unwrap();
    assert!(!remote.incomplete);
    assert_eq!(remote.files.len(), 1);
    assert_eq!(
        serde_json::to_value(remote).unwrap(),
        serde_json::to_value(local).unwrap()
    );
}

fn assert_deep_search_is_bounded(low_fd_limit: bool) {
    let tmp = tempfile::tempdir().unwrap();
    let mut path = tmp.path().to_path_buf();
    for _ in 0..100 {
        path.push("d");
        std::fs::create_dir(&path).unwrap();
    }
    std::fs::write(path.join("target.ts"), "").unwrap();
    // A skipped deep branch must not prevent searching shallower siblings.
    write(tmp.path(), "z-target.ts", "");
    let root = pin(tmp.path());
    #[cfg(unix)]
    let original_limit = if low_fd_limit {
        let mut original = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        assert_eq!(
            unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut original) },
            0
        );
        let low = libc::rlimit {
            rlim_cur: 64,
            rlim_max: original.rlim_max,
        };
        assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &low) }, 0);
        Some(original)
    } else {
        None
    };
    #[cfg(not(unix))]
    let _ = low_fd_limit;
    let mut peak_frames = 1;
    let result = search(
        &root,
        "target",
        1,
        &AtomicU64::new(1),
        Duration::from_secs(10),
        &mut |relative| {
            // This hook runs immediately before each successful descent. In a
            // single-path tree every ancestor is live; include the new frame.
            peak_frames = peak_frames.max(relative.components().count() + 1);
            assert!(
                peak_frames <= MAX_OPEN_FRAMES,
                "unbounded frames: {peak_frames}"
            );
            // Retained frames plus all transient handles have a 32-handle
            // envelope. Leave room for unrelated concurrent Host work.
            let spare = (0..16)
                .map(|_| root.dir.open_subdir("").unwrap())
                .collect::<Vec<_>>();
            drop(spare);
        },
    )
    .unwrap();
    #[cfg(unix)]
    if let Some(original) = original_limit {
        assert_eq!(
            unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &original) },
            0
        );
    }
    assert_eq!(peak_frames, MAX_OPEN_FRAMES);
    assert!(result.incomplete);
    assert_eq!(result.files.len(), 1);
    assert_eq!(result.files[0].name, "z-target.ts");
}

#[test]
fn file_name_search_caps_live_directory_frames() {
    assert_deep_search_is_bounded(false);
}

#[cfg(unix)]
#[test]
fn file_name_search_low_fd_limit_subprocess() {
    const CHILD: &str = "YUZORA_SEARCH_LOW_FD_CHILD";
    if std::env::var_os(CHILD).is_some() {
        assert_deep_search_is_bounded(true);
        return;
    }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "file_name_search::pinned::tests::file_name_search_low_fd_limit_subprocess",
            "--nocapture",
        ])
        .env(CHILD, "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn file_name_search_listing_observes_mid_entry_cancellation_and_deadline() {
    let tmp = tempfile::tempdir().unwrap();
    for n in 0..100 {
        write(tmp.path(), &format!("target-{n}.ts"), "");
    }
    let root = pin(tmp.path());
    for cancel_generation in [true, false] {
        let source = AtomicU64::new(1);
        let mut budget = Budget {
            started: Instant::now(),
            time: Duration::from_secs(10),
            remaining: 100,
            generation: 1,
            source: &source,
        };
        let mut checkpoints = 0;
        let result = root.dir.list_entries_until(100, || {
            checkpoints += 1;
            if checkpoints == 8 {
                if cancel_generation {
                    source.store(2, Ordering::Relaxed);
                } else {
                    budget.time = Duration::ZERO;
                }
            }
            budget.check().is_err()
        });
        assert_eq!(result.unwrap_err(), "directory-listing-stopped");
        assert_eq!(checkpoints, 8, "must not read the remaining entries");
        // The unchanged non-cancellable API still returns the complete set.
        assert_eq!(root.dir.list_entries(100).unwrap().len(), 100);
    }
}

#[test]
fn file_name_search_listing_interleaved_with_files_list_has_independent_cursor() {
    use std::sync::mpsc;
    let tmp = tempfile::tempdir().unwrap();
    // Each native Windows record is >300 bytes: well over one 64-KiB batch.
    let names: Vec<_> = (0..800)
        .map(|n| format!("{n:04}-{}.ts", "target".repeat(20)))
        .collect();
    for name in &names {
        write(tmp.path(), name, "");
        write(tmp.path(), &format!("child/{name}"), "");
    }
    let mut files = WorkspaceFiles::default();
    let opened = files.open(tmp.path().to_str().unwrap()).unwrap();
    let id = opened["capabilityId"].as_str().unwrap();
    for relative in ["", "child"] {
        let root = files.file_name_search_root(id).unwrap();
        let dir = root.dir.open_subdir(relative).unwrap();
        let (paused_tx, paused_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let mut checkpoints = 0;
            dir.list_entries_until(1000, || {
                checkpoints += 1;
                if checkpoints == 10 {
                    paused_tx.send(()).unwrap();
                    resume_rx.recv_timeout(Duration::from_secs(10)).unwrap();
                }
                false
            })
            .unwrap()
        });
        paused_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        let listing = files.list(id, relative).unwrap();
        resume_tx.send(()).unwrap();
        let search = worker.join().unwrap();
        let mut expected = names.clone();
        if relative.is_empty() {
            expected.push("child".into());
        }
        expected.sort();
        let search_names: Vec<_> = search.into_iter().map(|(name, _)| name).collect();
        let mut file_names: Vec<_> = listing
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["name"].as_str().unwrap().to_owned())
            .collect();
        file_names.sort();
        assert_eq!(search_names, expected, "search cursor: {relative}");
        assert_eq!(file_names, expected, "FilesList cursor: {relative}");
    }
}

#[test]
fn file_name_search_case_insensitive_ignore_discovery_matches_local() {
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "a", "");
    if !tmp.path().join("A").exists() {
        eprintln!("SKIP: ignore-name case parity requires a case-insensitive volume");
        return;
    }
    for ignore_path in [".GITIGNORE", ".IGNORE", ".GIT/INFO/EXCLUDE"] {
        let fixture = tempfile::tempdir_in(tmp.path()).unwrap();
        write(fixture.path(), ignore_path, "target.ts\n");
        write(fixture.path(), "target.ts", "");
        write(fixture.path(), "keep-target.ts", "");
        let root = pin(fixture.path());
        let source = AtomicU64::new(1);
        let remote =
            run_pinned_file_name_search(&root, "target", 1, &source, Duration::from_secs(5))
                .unwrap();
        let local = super::super::run_file_name_search(
            &root.canonical,
            "target",
            1,
            &source,
            Duration::from_secs(5),
        )
        .unwrap();
        assert!(!remote.incomplete);
        assert_eq!(remote.files.len(), 1, "{ignore_path}");
        assert_eq!(
            serde_json::to_value(remote).unwrap(),
            serde_json::to_value(local).unwrap(),
            "{ignore_path}"
        );
    }
}

#[test]
fn file_name_search_case_sensitive_ignore_discovery_preserves_spelling() {
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "a", "");
    if tmp.path().join("A").exists() {
        eprintln!("SKIP: exact ignore-name spelling requires a case-sensitive volume");
        return;
    }
    write(tmp.path(), ".GITIGNORE", "target.ts\n");
    write(tmp.path(), "target.ts", "");
    let root = pin(tmp.path());
    let source = AtomicU64::new(1);
    let remote =
        run_pinned_file_name_search(&root, "target", 1, &source, Duration::from_secs(5)).unwrap();
    let local = super::super::run_file_name_search(
        &root.canonical,
        "target",
        1,
        &source,
        Duration::from_secs(5),
    )
    .unwrap();
    assert!(!remote.incomplete);
    assert_eq!(remote.files.len(), 1);
    assert_eq!(
        serde_json::to_value(remote).unwrap(),
        serde_json::to_value(local).unwrap()
    );
}
