use super::*;
use std::path::PathBuf;

fn repository() -> (tempfile::TempDir, PathBuf, String, String) {
    let temp = tempfile::tempdir().unwrap();
    let remote = temp.path().join("origin.git");
    let work = temp.path().join("work");
    std::fs::create_dir_all(&remote).unwrap();
    std::fs::create_dir_all(&work).unwrap();
    test_repo::git(&remote, &["init", "--bare", "-b", "main"]);
    test_repo::init(&work);
    test_repo::write_and_commit(&work, "file.txt", "remote\n", "remote");
    let remote_oid = crate::git_oid::resolve_commit_oid(&work, "HEAD")
        .unwrap()
        .as_str()
        .to_owned();
    test_repo::git(
        &work,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    test_repo::git(&work, &["push", "origin", "HEAD:refs/heads/topic"]);
    test_repo::write_and_commit(&work, "file.txt", "different\n", "different");
    let other_oid = crate::git_oid::resolve_commit_oid(&work, "HEAD")
        .unwrap()
        .as_str()
        .to_owned();
    assert_ne!(remote_oid, other_oid);
    (temp, work, remote_oid, other_oid)
}

fn assert_remote_tracking(work: &Path, remote_oid: &str, spec: &str) {
    create_branch(work, "from-remote", Some(spec)).unwrap();
    assert_eq!(
        crate::git_oid::resolve_commit_oid(work, "HEAD")
            .unwrap()
            .as_str(),
        remote_oid
    );
    let env = test_repo::isolated_env();
    assert_eq!(
        git_config_value(work, "branch.from-remote.remote", &env)
            .unwrap()
            .as_deref(),
        Some("origin")
    );
    assert_eq!(
        git_config_value(work, "branch.from-remote.merge", &env)
            .unwrap()
            .as_deref(),
        Some("refs/heads/topic")
    );
    let listed = branches(work).unwrap();
    let branch = listed
        .local
        .iter()
        .find(|branch| branch.name == "from-remote")
        .unwrap();
    assert!(branch.is_current);
    assert_eq!(branch.upstream.as_deref(), Some("origin/topic"));
}

#[test]
fn full_remote_start_preserves_tracking_with_same_named_local_branch() {
    let (_temp, work, remote_oid, other_oid) = repository();
    test_repo::git(&work, &["branch", "origin/topic", &other_oid]);
    assert_remote_tracking(&work, &remote_oid, "refs/remotes/origin/topic");
}

#[test]
fn full_remote_start_preserves_tracking_with_same_named_lightweight_tag() {
    let (_temp, work, remote_oid, other_oid) = repository();
    test_repo::git(&work, &["tag", "origin/topic", &other_oid]);
    assert_remote_tracking(&work, &remote_oid, "refs/remotes/origin/topic");
}

#[test]
fn full_remote_start_preserves_tracking_with_same_named_annotated_tag() {
    let (_temp, work, remote_oid, other_oid) = repository();
    test_repo::git(
        &work,
        &[
            "tag",
            "-a",
            "origin/topic",
            "-m",
            "different tag",
            &other_oid,
        ],
    );
    assert_remote_tracking(&work, &remote_oid, "refs/remotes/origin/topic");
}

#[test]
fn full_remote_start_preserves_tracking_when_all_namespaces_collide() {
    let (_temp, work, remote_oid, other_oid) = repository();
    test_repo::git(&work, &["branch", "origin/topic", &other_oid]);
    test_repo::git(&work, &["tag", "origin/topic", &other_oid]);
    assert_remote_tracking(&work, &remote_oid, "refs/remotes/origin/topic");
}

#[test]
fn unambiguous_short_remote_start_keeps_existing_tracking_behavior() {
    let (_temp, work, remote_oid, _) = repository();
    assert_remote_tracking(&work, &remote_oid, "origin/topic");
}

#[test]
fn unconfigured_remote_ref_still_allows_branch_creation_without_tracking() {
    let temp = tempfile::tempdir().unwrap();
    let work = temp.path();
    test_repo::init(work);
    test_repo::write_and_commit(work, "file.txt", "base\n", "base");
    let oid = crate::git_oid::resolve_commit_oid(work, "HEAD")
        .unwrap()
        .as_str()
        .to_owned();
    test_repo::git(work, &["update-ref", "refs/remotes/missing/topic", &oid]);
    create_branch(work, "from-missing", Some("refs/remotes/missing/topic")).unwrap();
    assert_eq!(
        crate::git_oid::resolve_commit_oid(work, "HEAD")
            .unwrap()
            .as_str(),
        oid
    );
    let branch = branches(work)
        .unwrap()
        .local
        .into_iter()
        .find(|branch| branch.name == "from-missing")
        .unwrap();
    assert!(branch.is_current);
    assert!(branch.upstream.is_none());
}

#[test]
fn branch_list_preserves_upstream_namespaces_in_serialized_response() {
    let (_temp, work, remote_oid, other_oid) = repository();
    test_repo::git(&work, &["branch", "origin/topic", &other_oid]);
    test_repo::git(
        &work,
        &[
            "branch",
            "--track",
            "local-consumer",
            "refs/heads/origin/topic",
        ],
    );
    test_repo::git(
        &work,
        &[
            "branch",
            "--track",
            "remote-consumer",
            "refs/remotes/origin/topic",
        ],
    );
    test_repo::git(&work, &["tag", "v1", &remote_oid]);
    let listed = serde_json::to_value(branches(&work).unwrap()).unwrap();
    let locals = listed["local"].as_array().unwrap();
    let local = locals
        .iter()
        .find(|b| b["name"] == "local-consumer")
        .unwrap();
    let remote = locals
        .iter()
        .find(|b| b["name"] == "remote-consumer")
        .unwrap();
    assert_eq!(local["upstream"], "origin/topic");
    assert_eq!(remote["upstream"], "origin/topic");
    assert_ne!(
        crate::git_oid::resolve_commit_oid(&work, "local-consumer").unwrap(),
        crate::git_oid::resolve_commit_oid(&work, "remote-consumer").unwrap()
    );
    assert_eq!(local["upstreamRef"], "refs/heads/origin/topic");
    assert_eq!(remote["upstreamRef"], "refs/remotes/origin/topic");
    let main = locals.iter().find(|b| b["name"] == "main").unwrap();
    assert!(main.as_object().unwrap().contains_key("upstreamRef"));
    assert!(main["upstreamRef"].is_null());
    assert_eq!(listed["tags"][0]["name"], "v1");
    assert!(!listed["tags"][0]["date"].as_str().unwrap().is_empty());
}

#[test]
fn branch_list_preserves_gone_remote_upstream_namespace() {
    let (_temp, work, _, _) = repository();
    test_repo::git(
        &work,
        &[
            "branch",
            "--track",
            "remote-consumer",
            "refs/remotes/origin/topic",
        ],
    );
    test_repo::git(&work, &["update-ref", "-d", "refs/remotes/origin/topic"]);
    let listed = serde_json::to_value(branches(&work).unwrap()).unwrap();
    let branch = listed["local"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["name"] == "remote-consumer")
        .unwrap();
    assert_eq!(branch["upstream"], "origin/topic");
    assert_eq!(branch["gone"], true);
    assert_eq!(branch["upstreamRef"], "refs/remotes/origin/topic");
}

#[test]
fn branch_list_keeps_direct_remote_branches_ending_in_head() {
    let (_temp, work, _, other_oid) = repository();
    for name in ["origin/feature/HEAD", "upstream/HEAD"] {
        test_repo::git(
            &work,
            &["update-ref", &format!("refs/remotes/{name}"), &other_oid],
        );
    }
    test_repo::git(&work, &["branch", "feature/HEAD", &other_oid]);
    test_repo::git(&work, &["tag", "feature/HEAD", &other_oid]);
    let listed = branches(&work).unwrap();
    assert!(
        listed.remote.contains(&"origin/feature/HEAD".to_string()),
        "{:?}",
        listed.remote
    );
    assert!(listed.remote.contains(&"upstream/HEAD".to_string()));
    assert!(listed.local.iter().any(|b| b.name == "feature/HEAD"));
    assert!(listed.tags.iter().any(|t| t.name == "feature/HEAD"));
}

#[test]
fn branch_list_hides_symbolic_head_aliases_but_keeps_other_alias_names() {
    let (_temp, work, _, _) = repository();
    for name in ["origin/HEAD", "origin/feature/HEAD", "origin/alias"] {
        test_repo::git(
            &work,
            &[
                "symbolic-ref",
                &format!("refs/remotes/{name}"),
                "refs/remotes/origin/topic",
            ],
        );
    }
    let listed = branches(&work).unwrap();
    assert!(!listed.remote.contains(&"origin/HEAD".to_string()));
    assert!(!listed.remote.contains(&"origin/feature/HEAD".to_string()));
    assert!(listed.remote.contains(&"origin/alias".to_string()));
    assert!(listed.remote.contains(&"origin/topic".to_string()));
}

#[test]
fn branch_list_shows_head_after_symbolic_ref_is_replaced_with_direct_ref() {
    let (_temp, work, _, other_oid) = repository();
    let full_ref = "refs/remotes/origin/HEAD";
    test_repo::git(
        &work,
        &["symbolic-ref", full_ref, "refs/remotes/origin/topic"],
    );
    assert!(!branches(&work)
        .unwrap()
        .remote
        .contains(&"origin/HEAD".to_string()));
    test_repo::git(&work, &["symbolic-ref", "--delete", full_ref]);
    test_repo::git(&work, &["update-ref", full_ref, &other_oid]);
    assert_eq!(
        crate::git_oid::resolve_commit_oid(&work, full_ref)
            .unwrap()
            .as_str(),
        other_oid
    );
    assert!(branches(&work)
        .unwrap()
        .remote
        .contains(&"origin/HEAD".to_string()));
}

#[test]
fn direct_remote_head_requires_a_usable_local_name_and_keeps_exact_tracking() {
    let (_temp, work, remote_oid, _) = repository();
    let full_ref = "refs/remotes/origin/HEAD";
    test_repo::git(&work, &["update-ref", full_ref, &remote_oid]);
    assert!(create_branch(&work, "HEAD", Some(full_ref)).is_err());
    assert!(branches(&work)
        .unwrap()
        .local
        .iter()
        .any(|b| b.name == "main" && b.is_current));
    create_branch(&work, "from-origin", Some(full_ref)).unwrap();
    assert_eq!(
        crate::git_oid::resolve_commit_oid(&work, "HEAD")
            .unwrap()
            .as_str(),
        remote_oid
    );
    let branch = branches(&work)
        .unwrap()
        .local
        .into_iter()
        .find(|b| b.name == "from-origin")
        .unwrap();
    assert!(branch.is_current);
    assert_eq!(branch.upstream_ref.as_deref(), Some(full_ref));
}
