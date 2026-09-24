use std::{path::Path, time::Duration};
use yuzora_lib::git_service::{
    branches, checkout, commit, create_branch, remote_probe, run_git, stage, status_of, unstage,
};

fn git(root: &Path, args: &[&str]) -> String {
    let null = if cfg!(windows) { "NUL" } else { "/dev/null" };
    let output = run_git(
        root,
        args,
        Duration::from_secs(30),
        &[
            ("GIT_CONFIG_GLOBAL".into(), null.into()),
            ("GIT_CONFIG_SYSTEM".into(), null.into()),
        ],
    )
    .unwrap();
    assert_eq!(output.code, 0, "git {args:?}: {}", output.stderr);
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn identity(root: &Path) {
    git(root, &["config", "user.name", "Workbench test"]);
    git(root, &["config", "user.email", "test@example.invalid"]);
    git(root, &["config", "commit.gpgsign", "false"]);
    git(
        root,
        &[
            "config",
            "core.hooksPath",
            root.join("empty-hooks").to_str().unwrap(),
        ],
    );
}

#[test]
fn local_remote_roundtrip_and_rejected_push_preserve_work() {
    // All writes, commits and pushes stay inside this disposable directory.
    let temp = tempfile::tempdir().unwrap();
    let local = temp.path().join("local");
    let remote = temp.path().join("remote.git");
    let peer = temp.path().join("peer");
    git(
        temp.path(),
        &[
            "init",
            "--bare",
            "--initial-branch=main",
            remote.to_str().unwrap(),
        ],
    );
    git(
        temp.path(),
        &["init", "--initial-branch=main", local.to_str().unwrap()],
    );
    identity(&local);
    std::fs::write(local.join("base.txt"), "base\n").unwrap();
    stage(&local, &["base.txt".into()]).unwrap();
    std::fs::write(local.join("base.txt"), "edited after staging\n").unwrap();
    unstage(&local, &["base.txt".into()]).unwrap();
    assert!(status_of(&local, None).unwrap().parsed.staged.is_empty());
    assert_eq!(
        std::fs::read_to_string(local.join("base.txt")).unwrap(),
        "edited after staging\n"
    );
    stage(&local, &["base.txt".into()]).unwrap();
    commit(&local, "initial").unwrap();
    git(
        &local,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git(&local, &["push", "-u", "origin", "main"]);
    assert_eq!(remote_probe(&local, &[]).unwrap(), "no");

    create_branch(&local, "feature/workbench", None).unwrap();
    assert!(branches(&local)
        .unwrap()
        .local
        .iter()
        .any(|branch| branch.name == "feature/workbench" && branch.is_current));
    std::fs::write(local.join("feature.txt"), "feature\n").unwrap();
    stage(&local, &["feature.txt".into()]).unwrap();
    commit(&local, "feature").unwrap();
    git(&local, &["push", "-u", "origin", "feature/workbench"]);
    checkout(&local, "main").unwrap();
    git(&local, &["merge", "--ff-only", "feature/workbench"]);
    git(&local, &["push"]);

    git(
        temp.path(),
        &["clone", remote.to_str().unwrap(), peer.to_str().unwrap()],
    );
    identity(&peer);
    std::fs::write(peer.join("peer.txt"), "peer\n").unwrap();
    stage(&peer, &["peer.txt".into()]).unwrap();
    commit(&peer, "peer change").unwrap();
    git(&peer, &["push"]);
    assert_eq!(remote_probe(&local, &[]).unwrap(), "yes");
    git(&local, &["fetch"]);
    git(&local, &["pull", "--ff-only"]);
    assert_eq!(
        git(&local, &["rev-parse", "HEAD"]),
        git(&peer, &["rev-parse", "HEAD"])
    );
    assert!(local.join("peer.txt").exists());

    std::fs::write(local.join("local-only.txt"), "keep local work\n").unwrap();
    stage(&local, &["local-only.txt".into()]).unwrap();
    commit(&local, "local divergence").unwrap();
    let local_head = git(&local, &["rev-parse", "HEAD"]);
    std::fs::write(peer.join("peer-only.txt"), "new peer work\n").unwrap();
    stage(&peer, &["peer-only.txt".into()]).unwrap();
    commit(&peer, "peer divergence").unwrap();
    git(&peer, &["push"]);
    let rejected = run_git(&local, &["push"], Duration::from_secs(30), &[]).unwrap();
    assert_ne!(rejected.code, 0);
    assert!(!rejected.stderr.is_empty());
    assert_eq!(git(&local, &["rev-parse", "HEAD"]), local_head);
    assert_eq!(
        std::fs::read_to_string(local.join("local-only.txt")).unwrap(),
        "keep local work\n"
    );
}
