use super::*;

fn staged_rename(old: &str, new: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    test_repo::init(dir.path());
    std::fs::write(dir.path().join(old), "original content\n").unwrap();
    test_repo::git(dir.path(), &["add", "--", old]);
    test_repo::git(dir.path(), &["commit", "-m", "fixture baseline"]);
    std::fs::rename(dir.path().join(old), dir.path().join(new)).unwrap();
    test_repo::git(dir.path(), &["add", "--", old, new]);
    let status = status_of(dir.path(), None).unwrap();
    assert_eq!(status.parsed.staged.len(), 1);
    assert_eq!(status.parsed.staged[0].status, "R");
    assert_eq!(status.parsed.staged[0].orig_path.as_deref(), Some(old));
    dir
}

fn assert_rename_unstaged(old: &str, new: &str) {
    let dir = staged_rename(old, new);
    let result = unstage(dir.path(), &[new.to_string()]);
    let after = status_of(dir.path(), None).unwrap();
    assert!(
        after.parsed.staged.is_empty(),
        "{result:?}; staged={:?}",
        after.parsed.staged
    );
    assert!(result.is_ok(), "{result:?}");
    assert!(after
        .parsed
        .unstaged
        .iter()
        .any(|entry| entry.path == old && entry.status == "D"));
    assert_eq!(after.parsed.untracked, [new]);
    assert_eq!(
        std::fs::read(dir.path().join(new)).unwrap(),
        b"original content\n"
    );
    assert!(!dir.path().join(old).exists());
}

#[test]
fn unstage_selected_rename_restores_both_index_paths() {
    for (old, new) in [
        ("old.txt", "new.txt"),
        ("old space.txt", "new space.txt"),
        ("舊檔.txt", "新檔.txt"),
        ("-old.txt", "-new.txt"),
    ] {
        assert_rename_unstaged(old, new);
    }
}

#[cfg(unix)]
#[test]
fn unstage_selected_rename_preserves_newline_paths() {
    assert_rename_unstaged("old\nname.txt", "new\nname.txt");
}

#[test]
fn unstage_selected_rename_preserves_working_edits_and_unselected_index() {
    let dir = staged_rename("old.txt", "new.txt");
    let root = dir.path();
    std::fs::write(root.join("canary.txt"), "staged canary\n").unwrap();
    test_repo::git(root, &["add", "--", "canary.txt"]);
    std::fs::write(root.join("new.txt"), "later unstaged edits\n").unwrap();
    std::fs::write(root.join("old.txt"), "reoccupied source\n").unwrap();
    let before = status_of(root, None).unwrap();
    let canary = before
        .parsed
        .staged
        .iter()
        .find(|entry| entry.path == "canary.txt")
        .unwrap()
        .clone();
    let result = unstage(root, &["new.txt".into()]);
    let after = status_of(root, None).unwrap();
    assert_eq!(after.parsed.staged, [canary], "{result:?}");
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(
        std::fs::read(root.join("new.txt")).unwrap(),
        b"later unstaged edits\n"
    );
    assert_eq!(
        std::fs::read(root.join("old.txt")).unwrap(),
        b"reoccupied source\n"
    );
    assert_eq!(
        std::fs::read(root.join("canary.txt")).unwrap(),
        b"staged canary\n"
    );
}

#[test]
fn unstage_copy_does_not_unstage_its_modified_source() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    test_repo::init(root);
    test_repo::write_and_commit(root, "source.txt", "source\n", "baseline");
    std::fs::write(root.join("source.txt"), "source changed\n").unwrap();
    std::fs::copy(root.join("source.txt"), root.join("copy.txt")).unwrap();
    test_repo::git(root, &["add", "--", "source.txt", "copy.txt"]);
    let before = status_of(root, None).unwrap();
    let source = before
        .parsed
        .staged
        .iter()
        .find(|entry| entry.path == "source.txt")
        .unwrap()
        .clone();
    unstage(root, &["copy.txt".into()]).unwrap();
    assert_eq!(status_of(root, None).unwrap().parsed.staged, [source]);
    assert_eq!(
        std::fs::read(root.join("copy.txt")).unwrap(),
        b"source changed\n"
    );
}

#[test]
fn unstage_bulk_renames_and_ordinary_file_preserves_working_tree() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    test_repo::init(root);
    for name in ["old-a", "old-b", "ordinary"] {
        std::fs::write(root.join(name), format!("content {name}\n")).unwrap();
    }
    test_repo::git(root, &["add", "--all"]);
    test_repo::git(root, &["commit", "-m", "baseline"]);
    std::fs::rename(root.join("old-a"), root.join("new-a")).unwrap();
    std::fs::rename(root.join("old-b"), root.join("new-b")).unwrap();
    std::fs::write(root.join("ordinary"), "edited\n").unwrap();
    test_repo::git(root, &["add", "--all"]);
    unstage(root, &["new-a".into(), "new-b".into(), "ordinary".into()]).unwrap();
    assert!(status_of(root, None).unwrap().parsed.staged.is_empty());
    assert_eq!(std::fs::read(root.join("ordinary")).unwrap(), b"edited\n");
    for (old, new) in [("old-a", "new-a"), ("old-b", "new-b")] {
        assert!(!root.join(old).exists());
        assert_eq!(
            std::fs::read_to_string(root.join(new)).unwrap(),
            format!("content {old}\n")
        );
    }
}

#[test]
fn unstage_unborn_keeps_working_file() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    test_repo::init(root);
    std::fs::write(root.join("new.txt"), "new\n").unwrap();
    test_repo::git(root, &["add", "--", "new.txt"]);
    unstage(root, &["new.txt".into()]).unwrap();
    assert!(status_of(root, None).unwrap().parsed.staged.is_empty());
    assert_eq!(std::fs::read(root.join("new.txt")).unwrap(), b"new\n");
}

#[test]
fn unstage_rename_rejects_ambiguous_replacement_character_before_mutating_index() {
    let dir = staged_rename("old\u{fffd}", "new.txt");
    let root = dir.path();
    let index = || {
        run_ok(root, &["ls-files", "--stage", "-z"], DEFAULT_TIMEOUT, &[])
            .unwrap()
            .stdout
    };
    let before = index();
    let error = unstage(root, &["new.txt".into()]).unwrap_err();
    assert!(error.contains("not valid UTF-8"), "{error}");
    assert_eq!(index(), before);
    assert_eq!(
        std::fs::read(root.join("new.txt")).unwrap(),
        b"original content\n"
    );
}

// APFS rejects non-UTF-8 names before Git can observe them. The shared guard
// above runs on every platform; this byte-alias fixture requires a Linux FS.
#[cfg(target_os = "linux")]
#[test]
fn unstage_rename_rejects_lossy_source_before_mutating_index() {
    use std::os::unix::ffi::OsStringExt;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let invalid = std::ffi::OsString::from_vec(b"old\xff".to_vec());
    let canary = "old\u{fffd}";
    test_repo::init(root);
    std::fs::write(root.join(&invalid), "rename source\n").unwrap();
    std::fs::write(root.join(canary), "canary baseline\n").unwrap();
    test_repo::git(root, &["add", "--all"]);
    test_repo::git(root, &["commit", "-m", "baseline"]);
    std::fs::rename(root.join(&invalid), root.join("new.txt")).unwrap();
    std::fs::write(root.join(canary), "canary staged\n").unwrap();
    test_repo::git(root, &["add", "--all"]);
    let index = || {
        run_ok(root, &["ls-files", "--stage", "-z"], DEFAULT_TIMEOUT, &[])
            .unwrap()
            .stdout
    };
    let before = index();
    let error = unstage(root, &["new.txt".into()]).unwrap_err();
    assert!(error.contains("not valid UTF-8"), "{error}");
    assert_eq!(index(), before);
    assert_eq!(
        std::fs::read(root.join(canary)).unwrap(),
        b"canary staged\n"
    );
    assert_eq!(
        std::fs::read(root.join("new.txt")).unwrap(),
        b"rename source\n"
    );
}
