use super::*;
use std::path::PathBuf;

fn repository() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let remote = temp.path().join("origin.git");
    let work = temp.path().join("work");
    std::fs::create_dir_all(&remote).unwrap();
    std::fs::create_dir_all(&work).unwrap();
    test_repo::git(&remote, &["init", "--bare", "-b", "main"]);
    test_repo::init(&work);
    test_repo::git(&work, &["config", "push.default", "simple"]);
    test_repo::write_and_commit(&work, "file.txt", "base\n", "base");
    test_repo::git(
        &work,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    test_repo::git(&work, &["push", "--set-upstream", "origin", "main"]);
    (temp, work, remote)
}

fn gone_upstream(policy: &str) -> (tempfile::TempDir, PathBuf, PathBuf) {
    let fixture = repository();
    let work = &fixture.1;
    test_repo::git(work, &["switch", "-c", "work", "--track", "origin/main"]);
    test_repo::git(work, &["config", "push.default", policy]);
    test_repo::write_and_commit(work, "file.txt", "changed\n", "changed");
    test_repo::git(work, &["update-ref", "-d", "refs/remotes/origin/main"]);
    fixture
}

fn reference(root: &Path, name: &str) -> Option<String> {
    let out = run_git(
        root,
        &["rev-parse", "--verify", name],
        DEFAULT_TIMEOUT,
        &test_repo::isolated_env(),
    )
    .unwrap();
    (out.code == 0).then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn push(work: &Path, force: bool, tags: bool) -> (Vec<String>, GitOutput) {
    let env = test_repo::isolated_env();
    let args = push_args_with_options_env(work, force, tags, &env).unwrap();
    let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = run_git(work, &borrowed, DEFAULT_TIMEOUT, &env).unwrap();
    (args, result)
}

fn tracking(work: &Path, key: &str) -> Option<String> {
    git_config_value(work, key, &test_repo::isolated_env()).unwrap()
}

#[test]
fn gone_upstream_simple_policy_does_not_publish_wrong_branch() {
    let (_temp, work, remote) = gone_upstream("simple");
    let original = reference(&remote, "refs/heads/main");
    let (args, result) = push(&work, false, false);
    assert!(
        reference(&remote, "refs/heads/work").is_none(),
        "{args:?} unexpectedly published work: {}",
        result.stderr
    );
    assert_ne!(result.code, 0, "Git simple must reject the name mismatch");
    assert_eq!(reference(&remote, "refs/heads/main"), original);
    assert_eq!(
        tracking(&work, "branch.work.remote").as_deref(),
        Some("origin")
    );
    assert_eq!(
        tracking(&work, "branch.work.merge").as_deref(),
        Some("refs/heads/main")
    );
}

#[test]
fn gone_upstream_upstream_policy_uses_configured_destination() {
    let (_temp, work, remote) = gone_upstream("upstream");
    let (args, result) = push(&work, false, false);
    assert_eq!(result.code, 0, "{args:?}: {}", result.stderr);
    assert_eq!(
        reference(&remote, "refs/heads/main"),
        reference(&work, "HEAD")
    );
    assert!(reference(&remote, "refs/heads/work").is_none());
    assert_eq!(
        tracking(&work, "branch.work.merge").as_deref(),
        Some("refs/heads/main")
    );
}

#[test]
fn gone_upstream_current_policy_preserves_pull_tracking() {
    let (_temp, work, remote) = gone_upstream("current");
    let original = reference(&remote, "refs/heads/main");
    let (args, result) = push(&work, false, false);
    assert_eq!(result.code, 0, "{args:?}: {}", result.stderr);
    assert_eq!(
        reference(&remote, "refs/heads/work"),
        reference(&work, "HEAD")
    );
    assert_eq!(reference(&remote, "refs/heads/main"), original);
    assert_eq!(
        tracking(&work, "branch.work.merge").as_deref(),
        Some("refs/heads/main")
    );
}

#[test]
fn gone_upstream_nothing_policy_is_not_bypassed() {
    let (_temp, work, remote) = gone_upstream("nothing");
    let (args, result) = push(&work, false, false);
    assert_ne!(result.code, 0, "{args:?} bypassed push.default=nothing");
    assert!(reference(&remote, "refs/heads/work").is_none());
    assert_eq!(
        tracking(&work, "branch.work.merge").as_deref(),
        Some("refs/heads/main")
    );
}

#[test]
fn gone_upstream_push_remote_does_not_replace_pull_remote() {
    for key in ["branch.work.pushRemote", "remote.pushDefault"] {
        let (temp, work, origin) = gone_upstream("simple");
        let fork = temp.path().join("fork.git");
        std::fs::create_dir(&fork).unwrap();
        test_repo::git(&fork, &["init", "--bare", "-b", "main"]);
        test_repo::git(&work, &["remote", "add", "fork", fork.to_str().unwrap()]);
        test_repo::git(&work, &["config", key, "fork"]);
        let original = reference(&origin, "refs/heads/main");
        let (args, result) = push(&work, false, false);
        assert_eq!(result.code, 0, "{key} {args:?}: {}", result.stderr);
        assert_eq!(
            reference(&fork, "refs/heads/work"),
            reference(&work, "HEAD")
        );
        assert_eq!(reference(&origin, "refs/heads/main"), original);
        assert!(reference(&origin, "refs/heads/work").is_none());
        assert_eq!(
            tracking(&work, "branch.work.remote").as_deref(),
            Some("origin")
        );
        assert_eq!(
            tracking(&work, "branch.work.merge").as_deref(),
            Some("refs/heads/main")
        );
    }
}

#[test]
fn gone_upstream_force_lease_does_not_publish_an_unrelated_branch() {
    let (_temp, work, remote) = gone_upstream("upstream");
    test_repo::git(&work, &["tag", "-a", "v-next", "-m", "next"]);
    let original = reference(&remote, "refs/heads/main");
    let (args, result) = push(&work, true, true);
    assert!(args.iter().any(|arg| arg == "--force-with-lease"));
    assert!(args.iter().any(|arg| arg == "--follow-tags"));
    assert_ne!(
        result.code, 0,
        "{args:?} bypassed the missing tracking lease"
    );
    assert_eq!(reference(&remote, "refs/heads/main"), original);
    assert!(reference(&remote, "refs/heads/work").is_none());
    assert_eq!(
        tracking(&work, "branch.work.merge").as_deref(),
        Some("refs/heads/main")
    );
}

#[test]
fn configured_empty_upstream_is_not_replaced_by_automatic_publish() {
    let (_temp, work, remote) = gone_upstream("simple");
    test_repo::git(&work, &["config", "branch.work.merge", ""]);
    let (args, result) = push(&work, false, false);
    assert_ne!(
        result.code, 0,
        "{args:?} replaced an invalid explicit configuration"
    );
    assert!(reference(&remote, "refs/heads/work").is_none());
    let merge = run_git(
        &work,
        &["config", "--get", "branch.work.merge"],
        DEFAULT_TIMEOUT,
        &test_repo::isolated_env(),
    )
    .unwrap();
    assert_eq!(merge.code, 0);
    assert!(String::from_utf8_lossy(&merge.stdout).trim().is_empty());
}

#[test]
fn same_named_gone_upstream_still_pushes_without_changing_tracking() {
    let (_temp, work, remote) = repository();
    test_repo::write_and_commit(&work, "file.txt", "next\n", "next");
    test_repo::git(&work, &["update-ref", "-d", "refs/remotes/origin/main"]);
    let (args, result) = push(&work, false, false);
    assert_eq!(result.code, 0, "{args:?}: {}", result.stderr);
    assert_eq!(
        reference(&remote, "refs/heads/main"),
        reference(&work, "HEAD")
    );
    assert_eq!(
        tracking(&work, "branch.main.merge").as_deref(),
        Some("refs/heads/main")
    );
}

#[test]
fn detached_head_preserves_git_push_error() {
    let (_temp, work, remote) = repository();
    let original = reference(&remote, "refs/heads/main");
    test_repo::git(&work, &["switch", "--detach", "HEAD"]);
    let (_args, result) = push(&work, false, false);
    assert_ne!(result.code, 0);
    assert_eq!(reference(&remote, "refs/heads/main"), original);
}

#[test]
fn configured_unborn_branch_keeps_the_error_and_tracking_configuration() {
    let temp = tempfile::tempdir().unwrap();
    let remote = temp.path().join("origin.git");
    let work = temp.path().join("work");
    std::fs::create_dir_all(&remote).unwrap();
    std::fs::create_dir_all(&work).unwrap();
    test_repo::git(&remote, &["init", "--bare", "-b", "main"]);
    test_repo::init(&work);
    test_repo::git(
        &work,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    test_repo::git(&work, &["config", "branch.main.remote", "origin"]);
    test_repo::git(&work, &["config", "branch.main.merge", "refs/heads/main"]);
    let (_args, result) = push(&work, false, false);
    assert_ne!(result.code, 0);
    assert!(reference(&remote, "refs/heads/main").is_none());
    assert_eq!(
        tracking(&work, "branch.main.remote").as_deref(),
        Some("origin")
    );
    assert_eq!(
        tracking(&work, "branch.main.merge").as_deref(),
        Some("refs/heads/main")
    );
}
