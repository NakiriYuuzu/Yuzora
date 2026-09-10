use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use yuzora_host::git_service::{branches, diff_content, set_logger, status_of, GradedText};

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

#[test]
fn git_read_paths_avoid_redundant_processes_without_changing_content() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git(root, &["init", "-b", "main"]);
    git(root, &["config", "user.name", "Git performance fixture"]);
    git(root, &["config", "user.email", "fixture@example.invalid"]);
    std::fs::write(root.join("a.txt"), "original\n").unwrap();
    git(root, &["add", "a.txt"]);
    git(
        root,
        &["-c", "commit.gpgsign=false", "commit", "-m", "fixture"],
    );
    git(root, &["tag", "lightweight"]);
    git(
        root,
        &[
            "-c",
            "tag.gpgsign=false",
            "tag",
            "-a",
            "annotated",
            "-m",
            "fixture",
        ],
    );
    git(root, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    git(
        root,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ],
    );
    std::fs::write(root.join("a.txt"), "staged\n").unwrap();
    git(root, &["add", "a.txt"]);
    std::fs::write(root.join("a.txt"), "working\n").unwrap();
    set_logger(|_, _, _| {
        COMMANDS.fetch_add(1, Ordering::Relaxed);
    });

    let status = status_of(root, None).unwrap();
    assert!(status.in_progress.is_none());
    let status_commands = COMMANDS.swap(0, Ordering::Relaxed);
    let refs = branches(root).unwrap();
    assert!(refs
        .local
        .iter()
        .any(|branch| branch.name == "main" && branch.is_current));
    assert_eq!(refs.remote, vec!["origin/main"]);
    assert_eq!(refs.tags.len(), 2);
    assert!(refs.tags.iter().all(|tag| !tag.date.is_empty()));
    let branch_commands = COMMANDS.swap(0, Ordering::Relaxed);
    let working = diff_content(root, "a.txt", false, None).unwrap();
    assert!(matches!(working.original, GradedText::Full { content } if content == "staged\n"));
    assert!(matches!(working.modified, GradedText::Full { content } if content == "working\n"));
    let working_commands = COMMANDS.swap(0, Ordering::Relaxed);
    let staged = diff_content(root, "a.txt", true, None).unwrap();
    assert!(matches!(staged.original, GradedText::Full { content } if content == "original\n"));
    assert!(matches!(staged.modified, GradedText::Full { content } if content == "staged\n"));
    let staged_commands = COMMANDS.swap(0, Ordering::Relaxed);
    let historical = yuzora_host::git_log::file_at_rev(root, "HEAD", "a.txt").unwrap();
    assert!(
        matches!(historical, yuzora_host::git_log::FileAtRevResult::Full { content } if content == "original\n")
    );
    let historical_commands = COMMANDS.swap(0, Ordering::Relaxed);
    assert_eq!(historical_commands, 2);
    assert_eq!(
        (
            status_commands,
            branch_commands,
            working_commands,
            staged_commands
        ),
        (2, 1, 1, 2)
    );
}
