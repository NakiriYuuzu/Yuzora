use super::*;
use crate::test_support::git_repo::{git, git_input, init, Repo, SENTINEL};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};

fn at(extra: Value) -> Value {
    let mut params = json!({"session":"work","workspaceId":"w"});
    for (key, value) in extra.as_object().unwrap() {
        params[key] = value.clone();
    }
    params
}

fn spaces(repo: &Repo) -> Spaces {
    let roots = [serde_json::from_value(repo.root("work", "w")).unwrap()];
    Spaces::new(&repo.trust_store, &["work".to_owned()], &roots).unwrap()
}

fn start() -> Timestamp {
    serde_json::from_str("\"2030-01-01T00:00:00Z\"").unwrap()
}

/// What `Reads::git` does before the executor: parse, find the Space, run.
fn call(spaces: &Spaces, method: &str, extra: Value) -> Result<Value, &'static str> {
    call_as(spaces, "phone", start(), method, extra)
}

fn call_as(
    spaces: &Spaces,
    device: &str,
    now: Timestamp,
    method: &str,
    extra: Value,
) -> Result<Value, &'static str> {
    let request = parse(method, at(extra))?;
    let space = spaces.get(&request.session, &request.workspace_id)?;
    request.run(&space, &spaces.tokens(device, now))
}

fn trusted() -> (Repo, Spaces) {
    let repo = Repo::new();
    repo.commit("a.txt", "one\ntwo\nthree\n", "first");
    repo.trust();
    let spaces = spaces(&repo);
    (repo, spaces)
}

fn root(session: &str, workspace_id: &str, root: &str) -> SpaceRoot {
    SpaceRoot {
        session: session.into(),
        workspace_id: workspace_id.into(),
        root: root.into(),
    }
}

fn absolute(name: &str) -> String {
    std::env::temp_dir().join(name).to_str().unwrap().to_owned()
}

#[test]
fn git_init_and_admin_roots_are_validated_and_filtered_by_the_allowlist() {
    let allow = ["work".to_owned()];
    let store = std::env::temp_dir().join("workspace-trust.json");
    let dir = absolute("space");
    let build = |roots: &[SpaceRoot]| Spaces::new(&store, &allow, roots).err();
    assert_eq!(build(&[root("work", "w", &dir)]), None);
    assert_eq!(
        Spaces::new(Path::new("workspace-trust.json"), &allow, &[]).err(),
        Some("invalid-trust-store")
    );
    // Absolute on every platform ("/a" is relative on Windows), `len` bytes long.
    let padded = |len: usize| format!("{dir}{}", "a".repeat(len - dir.len()));
    let long_root = padded(ROOT_BYTES + 1);
    let unique: Vec<_> = (0..=MAX_ROOTS)
        .map(|n| root("work", &n.to_string(), &dir))
        .collect();
    for (name, roots) in [
        ("empty session", vec![root("", "w", &dir)]),
        ("empty workspace", vec![root("work", "", &dir)]),
        (
            "long session",
            vec![root(&"s".repeat(ID_BYTES + 1), "w", &dir)],
        ),
        (
            "long workspace",
            vec![root("work", &"w".repeat(ID_BYTES + 1), &dir)],
        ),
        ("empty root", vec![root("work", "w", "")]),
        ("relative root", vec![root("work", "w", "relative/dir")]),
        ("long root", vec![root("work", "w", &long_root)]),
        (
            "duplicate",
            vec![root("work", "w", &dir), root("work", "w", &dir)],
        ),
        (
            "duplicate outside the allowlist",
            vec![root("other", "w", &dir), root("other", "w", &dir)],
        ),
        ("too many", unique.clone()),
    ] {
        assert_eq!(build(&roots), Some("invalid-space-roots"), "{name}");
    }
    // Exactly at every bound is fine.
    let longest = padded(ROOT_BYTES);
    assert_eq!(
        build(&[root(&"s".repeat(ID_BYTES), &"w".repeat(ID_BYTES), &longest)]),
        None
    );
    assert_eq!(build(&unique[..MAX_ROOTS]), None);

    // A session outside the allowlist is dropped, not an error.
    let spaces = Spaces::new(
        &store,
        &allow,
        &[root("other", "w", &dir), root("work", "w", &dir)],
    )
    .unwrap();
    assert_eq!(spaces.get("other", "w").err(), Some("session-not-allowed"));
    assert!(spaces.get("work", "w").is_ok());
    assert_eq!(spaces.get("work", "x").err(), Some("git-unavailable"));
    let mut snapshot = json!({"workspaces":[{"id":"w"},{"id":"x"}]});
    spaces.mark("work", &mut snapshot);
    assert_eq!(snapshot["workspaces"][0]["git"], true);
    assert_eq!(snapshot["workspaces"][1]["git"], false);
    spaces.mark("other", &mut snapshot);
    assert_eq!(snapshot["workspaces"][0]["git"], false);

    // `spaces.roots` replaces the mapping; an invalid one changes nothing.
    let before = spaces.get("work", "w").ok().unwrap();
    assert!(!spaces.replace(&[root("work", "w", "relative")]));
    assert!(Arc::ptr_eq(&before, &spaces.get("work", "w").ok().unwrap()));
    // Unchanged roots keep their cached Git state; changed and removed ones lose it.
    assert!(spaces.replace(&[root("work", "w", &dir), root("work", "y", &dir)]));
    assert!(Arc::ptr_eq(&before, &spaces.get("work", "w").ok().unwrap()));
    assert!(spaces.get("work", "y").is_ok());
    assert!(spaces.replace(&[root("work", "w", &absolute("moved"))]));
    assert!(!Arc::ptr_eq(
        &before,
        &spaces.get("work", "w").ok().unwrap()
    ));
    assert_eq!(spaces.get("work", "y").err(), Some("git-unavailable"));
    assert!(spaces.replace(&[]));
    assert_eq!(spaces.get("work", "w").err(), Some("git-unavailable"));
}

#[test]
fn git_params_are_closed_and_bounded() {
    let hash = "0123456789abcdef0123456789abcdef01234567";
    let sha256 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    let ok = |method: &str, extra: Value| parse(method, at(extra)).is_ok();
    let bad = |method: &str, params: Value| parse(method, params).err() == Some("invalid-params");
    assert!(ok("git.status", json!({})));
    assert!(ok("git.branches", json!({})));
    assert!(ok("git.log", json!({})));
    assert!(ok("git.log", json!({"cursor":null,"limit":1})));
    assert!(ok("git.log", json!({"cursor":"opaque","limit":100})));
    assert!(ok("git.commit", json!({"hash":hash})));
    assert!(ok("git.commit", json!({"hash":sha256})));
    assert!(ok("git.commit", json!({"hash":hash.to_ascii_uppercase()})));
    assert!(ok("git.diff", json!({"kind":"worktree","path":"a"})));
    assert!(ok(
        "git.diff",
        json!({"kind":"worktree","path":"a","untracked":true,"origPath":null,"commit":null})
    ));
    assert!(ok(
        "git.diff",
        json!({"kind":"staged","path":"a","origPath":"b"})
    ));
    assert!(ok(
        "git.diff",
        json!({"kind":"commit","path":"a","commit":hash})
    ));
    assert!(ok(
        "git.diff",
        json!({"kind":"commit","path":"a","origPath":"b","commit":hash})
    ));
    for method in ["git.status", "git.branches", "git.log"] {
        assert!(bad(method, Value::Null), "{method}");
        assert!(bad(method, json!({"session":"work"})), "{method}");
        assert!(bad(method, json!({"workspaceId":"w"})), "{method}");
        assert!(bad(method, at(json!({"extra":1}))), "{method}");
        assert!(bad(method, at(json!({"workspaceId":""}))), "{method}");
        assert!(bad(method, at(json!({"session":""}))), "{method}");
        assert!(bad(method, at(json!({"workspaceId":7}))), "{method}");
        let long = "w".repeat(ID_BYTES + 1);
        assert!(bad(method, at(json!({ "workspaceId": long }))), "{method}");
    }
    assert!(bad("git.status", at(json!({"limit":5}))));
    for limit in [json!(0), json!(101), json!(-1), json!(1.5), json!("5")] {
        assert!(bad("git.log", at(json!({ "limit": limit }))), "{limit}");
    }
    assert!(bad("git.log", at(json!({"cursor":5}))));
    for hash in [
        "",
        "abc1234",
        "HEAD",
        "main",
        "--output=x",
        "0123456789abcdef0123456789abcdef0123456",
        "0123456789abcdef0123456789abcdef012345678",
        "0123456789abcdef0123456789abcdef0123456g",
    ] {
        assert!(bad("git.commit", at(json!({ "hash": hash }))), "{hash}");
        assert!(
            bad(
                "git.diff",
                at(json!({"kind":"commit","path":"a","commit":hash}))
            ),
            "{hash}"
        );
    }
    assert!(bad("git.commit", at(json!({}))));
    for extra in [
        json!({}),
        json!({"kind":"worktree"}),
        json!({"path":"a"}),
        json!({"kind":"other","path":"a"}),
        json!({"kind":"worktree","path":""}),
        json!({"kind":"worktree","path":"a\u{0}b"}),
        json!({"kind":"worktree","path":"p".repeat(PATH_BYTES + 1)}),
        json!({"kind":"staged","path":"a","origPath":"o".repeat(PATH_BYTES + 1)}),
        json!({"kind":"commit","path":"a","commit":"0123456789abcdef0123456789abcdef01234567","origPath":"o".repeat(PATH_BYTES + 1)}),
        json!({"kind":"worktree","path":"a","untracked":"yes"}),
        json!({"kind":"commit","path":"a"}),
        json!({"kind":"commit","path":"a","commit":null}),
        json!({"kind":"worktree","path":"a","other":true}),
    ] {
        assert!(bad("git.diff", at(extra.clone())), "{extra}");
    }
    assert!(bad("git.show", at(json!({}))));
}

#[test]
fn git_unmapped_space_and_unlisted_session_never_reach_git() {
    let (_repo, spaces) = trusted();
    assert_eq!(
        call(&spaces, "git.status", json!({"workspaceId":"missing"})),
        Err("git-unavailable")
    );
    assert_eq!(
        call(&spaces, "git.status", json!({"session":"other"})),
        Err("session-not-allowed")
    );
    // Not a valid session name, even if somebody put it on the allowlist.
    assert_eq!(
        call(&spaces, "git.status", json!({"session":"-work"})),
        Err("session-not-allowed")
    );
    // Parameters are checked first.
    assert_eq!(
        parse("git.log", at(json!({"session":"other","limit":0}))).err(),
        Some("invalid-params")
    );
    assert!(call(&spaces, "git.status", json!({})).is_ok());
}

#[test]
fn git_untrusted_repository_then_trust_granted_revoked_and_corrupted_on_disk() {
    let repo = Repo::new();
    repo.commit("a.txt", "one\n", "first");
    let spaces = spaces(&repo);
    // No trust file at all.
    for method in ["git.status", "git.branches", "git.log"] {
        assert_eq!(call(&spaces, method, json!({})), Err("git-untrusted"));
    }
    repo.trust();
    assert_eq!(
        call(&spaces, "git.status", json!({})).unwrap()["status"]["branch"],
        "main"
    );
    // The desktop revokes while the gateway holds a detected repository.
    repo.revoke();
    for method in ["git.status", "git.branches", "git.log"] {
        assert_eq!(call(&spaces, method, json!({})), Err("git-untrusted"));
    }
    assert_eq!(
        call(
            &spaces,
            "git.diff",
            json!({"kind":"worktree","path":"a.txt"})
        ),
        Err("git-untrusted")
    );
    repo.trust();
    assert!(call(&spaces, "git.status", json!({})).is_ok());
    // A corrupt or unknown-version store is not trust either.
    for content in ["not json", r#"{"version":9,"workspaces":[]}"#] {
        std::fs::write(&repo.trust_store, content).unwrap();
        assert_eq!(
            call(&spaces, "git.status", json!({})),
            Err("git-untrusted"),
            "{content}"
        );
    }
}

#[test]
fn git_subdirectory_space_is_read_while_its_own_trust_stands() {
    let repo = Repo::new();
    repo.commit("sub/a.txt", "one\n", "first");
    let sub = repo.dir.join("sub");
    let desktop = repo.desktop();
    desktop.0.grant_for_tests(sub.to_str().unwrap());
    let roots = [root("work", "w", sub.to_str().unwrap())];
    let spaces = Spaces::new(&repo.trust_store, &["work".to_owned()], &roots).unwrap();
    // The repository root is the parent; the trusted Space authorizes it.
    let status = call(&spaces, "git.status", json!({})).unwrap();
    assert_eq!(status["repo"]["name"], "repo");
    desktop.0.revoke(sub.to_str().unwrap()).unwrap();
    assert_eq!(call(&spaces, "git.status", json!({})), Err("git-untrusted"));
}

/// A repository with two Spaces in subfolders of it, each trusted on its own.
fn monorepo() -> (Repo, Spaces, [String; 2]) {
    let repo = Repo::new();
    repo.commit("web/a.txt", "one\n", "web");
    repo.commit("api/b.txt", "one\n", "api");
    let paths = ["web", "api"].map(|name| repo.dir.join(name).to_str().unwrap().to_owned());
    for path in &paths {
        repo.desktop().0.grant_for_tests(path);
    }
    let roots = [
        root("work", "web", &paths[0]),
        root("work", "api", &paths[1]),
    ];
    let spaces = Spaces::new(&repo.trust_store, &["work".to_owned()], &roots).unwrap();
    (repo, spaces, paths)
}
fn status_in(spaces: &Spaces, workspace: &str) -> Result<Value, &'static str> {
    call(spaces, "git.status", json!({"workspaceId":workspace}))
}

#[test]
fn git_revoking_the_space_detected_last_leaves_its_sibling_readable() {
    let (repo, spaces, paths) = monorepo();
    assert!(status_in(&spaces, "web").is_ok());
    assert!(status_in(&spaces, "api").is_ok());
    repo.desktop().0.revoke(&paths[1]).unwrap();
    assert_eq!(status_in(&spaces, "api"), Err("git-untrusted"));
    // Trusted on its own path, whatever its sibling does to the repository root.
    assert!(status_in(&spaces, "web").is_ok());
    repo.desktop().0.grant_for_tests(&paths[1]);
    assert!(status_in(&spaces, "api").is_ok());
    assert!(status_in(&spaces, "web").is_ok());
}

#[test]
fn git_revoking_a_space_detected_earlier_is_honoured() {
    let (repo, spaces, paths) = monorepo();
    assert!(status_in(&spaces, "web").is_ok());
    assert!(status_in(&spaces, "api").is_ok());
    repo.desktop().0.revoke(&paths[0]).unwrap();
    // The sibling that detected the repository last still stands; web does not.
    assert_eq!(status_in(&spaces, "web"), Err("git-untrusted"));
    assert!(status_in(&spaces, "api").is_ok());
    repo.desktop().0.grant_for_tests(&paths[0]);
    assert!(status_in(&spaces, "web").is_ok());
    repo.desktop().0.revoke(&paths[0]).unwrap();
    repo.desktop().0.revoke(&paths[1]).unwrap();
    for workspace in ["web", "api"] {
        assert_eq!(status_in(&spaces, workspace), Err("git-untrusted"));
    }
}

#[test]
fn git_revoking_a_subfolder_space_is_honoured_while_its_repository_root_is_trusted() {
    let (repo, spaces, paths) = monorepo();
    // The monorepo root itself is trusted too (it was once opened as a workspace).
    repo.trust();
    let head = repo.head();
    let reads = |workspace: &str| {
        [
            ("git.status", json!({})),
            ("git.branches", json!({})),
            ("git.log", json!({})),
            ("git.commit", json!({"hash":head})),
            (
                "git.diff",
                json!({"kind":"commit","commit":head,"path":"api/b.txt"}),
            ),
        ]
        .map(|(method, mut extra)| {
            extra["workspaceId"] = json!(workspace);
            (method, call(&spaces, method, extra))
        })
    };
    for (method, result) in reads("web") {
        assert!(result.is_ok(), "{method}: {result:?}");
    }
    // The desktop revokes web only; the gateway still holds the repository root
    // it detected, and that root stands on its own trust record.
    repo.desktop().0.revoke(&paths[0]).unwrap();
    for _ in 0..2 {
        for (method, result) in reads("web") {
            assert_eq!(result, Err("git-untrusted"), "{method}");
        }
    }
    // Its sibling Space is not touched by that.
    assert!(status_in(&spaces, "api").is_ok());
    // Trusting the folder again applies to the next read, without a restart.
    repo.desktop().0.grant_for_tests(&paths[0]);
    for (method, result) in reads("web") {
        assert!(result.is_ok(), "{method}: {result:?}");
    }
    // A revoked Space stays refused once the repository root is revoked as well.
    repo.desktop().0.revoke(&paths[0]).unwrap();
    repo.revoke();
    assert_eq!(status_in(&spaces, "web"), Err("git-untrusted"));
}

#[test]
fn git_a_refused_read_forgets_the_repository_it_had_detected() {
    let (repo, spaces, paths) = monorepo();
    // The monorepo root is trusted too, so it would still authorize the cached root.
    repo.trust();
    assert_eq!(status_in(&spaces, "web").unwrap()["repo"]["name"], "repo");
    repo.desktop().0.revoke(&paths[0]).unwrap();
    assert_eq!(status_in(&spaces, "web"), Err("git-untrusted"));
    // While web is refused it becomes a repository of its own, and is trusted again.
    let web = repo.dir.join("web");
    init(&web);
    std::fs::write(web.join("own.txt"), "own\n").unwrap();
    git(&web, &["add", "--", "own.txt"]);
    git(&web, &["commit", "-q", "-m", "own"]);
    repo.desktop().0.grant_for_tests(&paths[0]);
    // The running gateway answers as a freshly started one does: from web's own
    // repository, not from the root it had detected before the refusal.
    let running = status_in(&spaces, "web").unwrap()["repo"]["name"].clone();
    let roots = [root("work", "web", &paths[0])];
    let fresh = Spaces::new(&repo.trust_store, &["work".to_owned()], &roots).unwrap();
    let fresh = status_in(&fresh, "web").unwrap()["repo"]["name"].clone();
    assert_eq!(fresh, "web");
    assert_eq!(running, fresh);
}

#[test]
fn git_retrusting_a_replaced_space_folder_is_detected_again_without_a_restart() {
    let (repo, spaces, paths) = monorepo();
    assert!(status_in(&spaces, "web").is_ok());
    // The desktop revokes, the folder is replaced by another one, and the new
    // one is trusted: the identity the cached repository was authorized by is gone.
    repo.desktop().0.revoke(&paths[0]).unwrap();
    let web = repo.dir.join("web");
    std::fs::rename(&web, repo.dir.join("web-old")).unwrap();
    std::fs::create_dir(&web).unwrap();
    assert_eq!(status_in(&spaces, "web"), Err("git-untrusted"));
    repo.desktop().0.grant_for_tests(&paths[0]);
    assert!(status_in(&spaces, "web").is_ok());
    // Trust lost again for good: still refused, no matter how often it is asked.
    repo.desktop().0.revoke(&paths[0]).unwrap();
    for _ in 0..2 {
        assert_eq!(status_in(&spaces, "web"), Err("git-untrusted"));
    }
}

#[test]
fn git_replaced_directory_is_detected_again_once_and_trust_follows_the_new_identity() {
    let (repo, spaces) = trusted();
    assert!(call(&spaces, "git.status", json!({})).is_ok());
    let swap = |replacement: &dyn Fn(&Path)| {
        let old = repo.dir.with_file_name("repo-old");
        let new = repo.dir.with_file_name("repo-new");
        let _ = std::fs::remove_dir_all(&old);
        replacement(&new);
        std::fs::rename(&repo.dir, &old).unwrap();
        std::fs::rename(&new, &repo.dir).unwrap();
    };
    let fresh_repo = |dir: &Path| {
        init(dir);
        std::fs::write(dir.join("b.txt"), "fresh\n").unwrap();
        git(dir, &["add", "--", "b.txt"]);
        git(dir, &["commit", "-q", "-m", "fresh"]);
    };

    // Another directory at the trusted path is a different identity: untrusted.
    swap(&fresh_repo);
    assert_eq!(call(&spaces, "git.status", json!({})), Err("git-untrusted"));
    // Trusted again, the cached root is stale: one re-detect inside the request.
    repo.trust();
    let log = call(&spaces, "git.log", json!({})).unwrap();
    assert_eq!(log["commits"][0]["subject"], "fresh");
    assert_eq!(log["commits"].as_array().unwrap().len(), 1);

    // Replaced by something that is no repository: said so, not a Git error.
    swap(&|dir| std::fs::create_dir(dir).unwrap());
    repo.trust();
    assert_eq!(
        call(&spaces, "git.status", json!({})),
        Err("git-not-repository")
    );
}

#[test]
fn git_not_a_repository_needs_no_trust() {
    let plain = tempfile::tempdir().unwrap();
    let store = plain.path().join("workspace-trust.json");
    let roots = [root("work", "w", plain.path().to_str().unwrap())];
    let spaces = Spaces::new(&store, &["work".to_owned()], &roots).unwrap();
    for method in ["git.status", "git.branches", "git.log"] {
        assert_eq!(
            call(&spaces, method, json!({})),
            Err("git-not-repository"),
            "{method}"
        );
    }
}

#[test]
fn git_status_lists_changes_and_caps_each_list() {
    let (repo, spaces) = trusted();
    let head = repo.commit("b.txt", "bee\n", "second");
    repo.write("a.txt", "one\nTWO\nthree\n");
    repo.write("c.txt", "new\n");
    git(&repo.dir, &["add", "--", "c.txt"]);
    git(&repo.dir, &["mv", "b.txt", "d.txt"]);
    repo.write("u.txt", "untracked\n");
    let reply = call(&spaces, "git.status", json!({})).unwrap();
    assert_eq!(reply["repo"], json!({"name":"repo"}));
    assert_eq!(reply["truncated"], false);
    let status = &reply["status"];
    assert_eq!(status["branch"], "main");
    assert_eq!(status["headOid"], head);
    assert_eq!(status["detached"], false);
    assert_eq!(status["upstream"], Value::Null);
    assert_eq!(status["ahead"], 0);
    assert_eq!(status["behind"], 0);
    assert_eq!(status["inProgress"], Value::Null);
    assert_eq!(
        status["unstaged"],
        json!([{"path":"a.txt","origPath":null,"status":"M"}])
    );
    assert_eq!(status["untracked"], json!(["u.txt"]));
    assert_eq!(status["conflicted"], json!([]));
    let staged = status["staged"].as_array().unwrap();
    assert!(staged.contains(&json!({"path":"c.txt","origPath":null,"status":"A"})));
    assert!(staged.contains(&json!({"path":"d.txt","origPath":"b.txt","status":"R"})));
    assert!(!reply.to_string().contains(SENTINEL));

    // Exactly the cap is complete; one more cuts that list (and says so).
    let (repo, spaces) = trusted();
    for n in 0..STATUS_ENTRIES {
        repo.write(&format!("bulk/{n:04}.txt"), "x");
    }
    let reply = call(&spaces, "git.status", json!({})).unwrap();
    assert_eq!(reply["status"]["untracked"].as_array().unwrap().len(), 1000);
    assert_eq!(reply["truncated"], false);
    repo.write("bulk/extra.txt", "x");
    let reply = call(&spaces, "git.status", json!({})).unwrap();
    assert_eq!(reply["status"]["untracked"].as_array().unwrap().len(), 1000);
    assert_eq!(reply["truncated"], true);
}

#[test]
fn git_repository_without_commits_reads_without_failing() {
    let repo = Repo::new();
    repo.trust();
    let spaces = spaces(&repo);
    repo.write("new.txt", "hello\n");
    let status = call(&spaces, "git.status", json!({})).unwrap();
    assert_eq!(status["status"]["branch"], "main");
    assert_eq!(status["status"]["untracked"], json!(["new.txt"]));
    let branches = call(&spaces, "git.branches", json!({})).unwrap();
    assert_eq!(branches["local"], json!([]));
    let log = call(&spaces, "git.log", json!({})).unwrap();
    assert_eq!(log["commits"], json!([]));
    assert_eq!(log["hasMore"], false);
    let diff = call(
        &spaces,
        "git.diff",
        json!({"kind":"worktree","path":"new.txt","untracked":true}),
    )
    .unwrap();
    assert!(diff["text"].as_str().unwrap().contains("+hello"));
}

#[test]
fn git_branches_list_refs_and_cap_each_list() {
    let (repo, spaces) = trusted();
    repo.commit("a.txt", "two\n", "second");
    git(&repo.dir, &["branch", "feature", "HEAD~1"]);
    git(&repo.dir, &["tag", "v1"]);
    git(&repo.dir, &["remote", "add", "origin", "../nowhere.git"]);
    git(
        &repo.dir,
        &["update-ref", "refs/remotes/origin/main", "HEAD~1"],
    );
    git(&repo.dir, &["config", "branch.main.remote", "origin"]);
    git(
        &repo.dir,
        &["config", "branch.main.merge", "refs/heads/main"],
    );
    let reply = call(&spaces, "git.branches", json!({})).unwrap();
    assert_eq!(reply["truncated"], false);
    let local = reply["local"].as_array().unwrap();
    let main = local.iter().find(|b| b["name"] == "main").unwrap();
    let pushed = git(&repo.dir, &["rev-parse", "HEAD~1"]).trim().to_owned();
    assert_eq!(
        *main,
        json!({"name":"main","upstream":"origin/main","pushOid":pushed,"upstreamRef":"refs/remotes/origin/main","ahead":1,"behind":0,"isCurrent":true,"gone":false})
    );
    let feature = local.iter().find(|b| b["name"] == "feature").unwrap();
    assert_eq!(feature["isCurrent"], false);
    assert_eq!(feature["upstream"], Value::Null);
    assert_eq!(reply["remote"], json!(["origin/main"]));
    assert_eq!(reply["tags"][0]["name"], "v1");
    assert!(!reply["tags"][0]["date"].as_str().unwrap().is_empty());
    assert!(!reply.to_string().contains(SENTINEL));

    let refs = |prefix: &str, count: usize| {
        (0..count)
            .map(|n| format!("create {prefix}{n:03} HEAD\n"))
            .collect::<String>()
    };
    let at_cap = refs("refs/heads/l", BRANCHES - 2) // plus main and feature
        + &refs("refs/remotes/origin/r", BRANCHES - 1) // plus origin/main
        + &refs("refs/tags/t", TAGS - 1); // plus v1
    git_input(&repo.dir, &["update-ref", "--stdin"], &at_cap);
    let reply = call(&spaces, "git.branches", json!({})).unwrap();
    assert_eq!(reply["local"].as_array().unwrap().len(), BRANCHES);
    assert_eq!(reply["remote"].as_array().unwrap().len(), BRANCHES);
    assert_eq!(reply["tags"].as_array().unwrap().len(), TAGS);
    assert_eq!(reply["truncated"], false);
    // One more of any kind cuts that list and sets the flag.
    for (prefix, list, cap) in [
        ("refs/heads/l", "local", BRANCHES),
        ("refs/remotes/origin/r", "remote", BRANCHES),
        ("refs/tags/t", "tags", TAGS),
    ] {
        let extra = format!("{prefix}999");
        git_input(
            &repo.dir,
            &["update-ref", "--stdin"],
            &format!("create {extra} HEAD\n"),
        );
        let reply = call(&spaces, "git.branches", json!({})).unwrap();
        assert_eq!(reply["truncated"], true, "{prefix}");
        assert_eq!(reply[list].as_array().unwrap().len(), cap, "{prefix}");
        git_input(
            &repo.dir,
            &["update-ref", "--stdin"],
            &format!("delete {extra}\n"),
        );
    }
}

#[test]
fn git_log_pages_by_opaque_cursor_and_rejects_foreign_ones() {
    let (repo, spaces) = trusted();
    let mut hashes = vec![repo.head()];
    for n in 2..=5 {
        hashes.push(repo.commit("a.txt", &format!("{n}\n"), &format!("commit {n}")));
    }
    hashes.reverse(); // newest first
    git(&repo.dir, &["tag", "v1"]);

    let first = call(&spaces, "git.log", json!({"limit":2})).unwrap();
    assert_eq!(first["hasMore"], true);
    let commits = first["commits"].as_array().unwrap();
    assert_eq!(commits.len(), 2);
    assert_eq!(commits[0]["hash"], hashes[0]);
    assert_eq!(commits[0]["shortHash"], hashes[0][..7]);
    assert_eq!(commits[0]["subject"], "commit 5");
    assert_eq!(commits[0]["authorName"], "Tester");
    assert_eq!(commits[0]["authorEmail"], "tester@example.com");
    assert!(commits[0]["timestamp"].as_i64().unwrap() > 0);
    assert_eq!(commits[0]["parents"], json!([hashes[1]]));
    let refs = commits[0]["refs"].as_array().unwrap();
    assert!(refs.iter().any(|r| r["kind"] == "tag" && r["name"] == "v1"));
    let mut seen: Vec<String> = commits
        .iter()
        .map(|c| c["hash"].as_str().unwrap().to_owned())
        .collect();

    let mut cursor = first["nextCursor"].as_str().unwrap().to_owned();
    loop {
        let page = call(&spaces, "git.log", json!({"limit":2,"cursor":cursor})).unwrap();
        seen.extend(
            page["commits"]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| c["hash"].as_str().unwrap().to_owned()),
        );
        match page["nextCursor"].as_str() {
            Some(next) => {
                assert_eq!(page["hasMore"], true);
                cursor = next.to_owned();
            }
            None => {
                assert_eq!(page["hasMore"], false);
                break;
            }
        }
    }
    assert_eq!(seen, hashes);

    // The default page is 50, so one page holds all five.
    let all = call(&spaces, "git.log", json!({"cursor":null})).unwrap();
    assert_eq!(all["commits"].as_array().unwrap().len(), 5);
    assert_eq!(all["hasMore"], false);
    assert_eq!(all["nextCursor"], Value::Null);

    // Unknown, tampered and foreign cursors are all "invalid". So is a raw Host
    // cursor: only tokens this gateway issued count.
    let mut tampered = first["nextCursor"].as_str().unwrap().to_owned();
    tampered.push('0');
    let other = Repo::new();
    other.commit("a.txt", "x\n", "other");
    other.commit("a.txt", "y\n", "other 2");
    other.trust();
    let foreign = call(&self::spaces(&other), "git.log", json!({"limit":1})).unwrap()["nextCursor"]
        .as_str()
        .unwrap()
        .to_owned();
    let host = git_log::log_page(&repo.dir, None, 2, None, None, None, None)
        .unwrap()
        .next_cursor
        .unwrap();
    for cursor in [
        "garbage",
        "",
        tampered.as_str(),
        foreign.as_str(),
        host.as_str(),
    ] {
        assert_eq!(
            call(&spaces, "git.log", json!({ "cursor": cursor })),
            Err("git-cursor-invalid"),
            "{cursor}"
        );
    }
}

/// One commit per ref with its own committer time and zone, through fast-import.
fn commits_on(repo: &Repo, refs: &[(String, i64, &str)]) {
    let stream: String = refs
        .iter()
        .map(|(name, time, zone)| {
            format!(
                "commit {name}\ncommitter T <t@example.com> {time} {zone}\ndata 4\nc000\nM 100644 inline f.txt\ndata 2\nx\n\n"
            )
        })
        .collect();
    git_input(&repo.dir, &["fast-import", "--quiet"], &stream);
}

#[test]
fn git_log_cursors_are_short_tokens_bound_to_their_device_space_and_time() {
    let repo = Repo::new();
    repo.commit("a.txt", "1\n", "commit 1");
    for n in 2..=5 {
        repo.commit("a.txt", &format!("{n}\n"), &format!("commit {n}"));
    }
    repo.trust();
    let (here, same) = (repo.root("work", "w"), repo.root("work", "x"));
    let play = repo.root("play", "w");
    let roots: Vec<SpaceRoot> = [here, same, play]
        .into_iter()
        .map(|entry| serde_json::from_value(entry).unwrap())
        .collect();
    let allowlist = ["work".to_owned(), "play".to_owned()];
    let spaces = Spaces::new(&repo.trust_store, &allowlist, &roots).unwrap();
    let page = |device: &str, session: &str, workspace: &str, now: Timestamp, cursor: &str| {
        let extra = json!({"session":session,"workspaceId":workspace,"limit":2,"cursor":cursor});
        call_as(&spaces, device, now, "git.log", extra)
    };

    let first = call_as(&spaces, "phone", start(), "git.log", json!({"limit":2})).unwrap();
    let token = first["nextCursor"].as_str().unwrap().to_owned();
    // Short, URL-safe, at least 16 random bytes; the Host cursor stays inside.
    assert!(token.len() <= 64, "{token}");
    assert!(URL_SAFE_NO_PAD.decode(&token).unwrap().len() >= 16);
    let host = git_log::log_page(&repo.dir, None, 2, None, None, None, None)
        .unwrap()
        .next_cursor
        .unwrap();
    assert_ne!(token, host);
    assert!(!first.to_string().contains(&host));
    let again = call_as(&spaces, "phone", start(), "git.log", json!({"limit":2})).unwrap();
    assert_ne!(again["nextCursor"], first["nextCursor"]);

    // Its own device, Space and time: the page after the first, and again.
    for _ in 0..2 {
        let second = page("phone", "work", "w", start(), &token).unwrap();
        assert_eq!(second["commits"][0]["subject"], "commit 3");
    }
    // Never another device, another workspace id, or another session, even
    // when they map the very same repository.
    for (device, session, workspace) in [
        ("tablet", "work", "w"),
        ("phone", "work", "x"),
        ("phone", "play", "w"),
    ] {
        assert_eq!(
            page(device, session, workspace, start(), &token),
            Err("git-cursor-invalid"),
            "{device} {session} {workspace}"
        );
    }
    // 30 minutes on the gateway's clock; the last second still counts.
    let ttl = |seconds| start().after(Duration::from_secs(seconds));
    assert!(page("phone", "work", "w", ttl(30 * 60 - 1), &token).is_ok());
    for now in [ttl(30 * 60), ttl(24 * 3600)] {
        assert_eq!(
            page("phone", "work", "w", now, &token),
            Err("git-cursor-invalid")
        );
    }
    // The device starts over from the first page, as the PWA does.
    assert!(call_as(&spaces, "phone", ttl(30 * 60), "git.log", json!({})).is_ok());
}

#[test]
fn git_log_cursor_stays_short_in_a_repository_with_many_refs() {
    let (repo, spaces) = trusted();
    let tips: Vec<_> = (0..300)
        .map(|n| (format!("refs/heads/t{n:03}"), 1_700_000_000 + n, "+0000"))
        .collect();
    commits_on(&repo, &tips);
    // The Host cursor of this repository is far past what a request may carry.
    let host = git_log::log_page(&repo.dir, None, 2, None, None, None, None)
        .unwrap()
        .next_cursor
        .unwrap();
    assert!(host.len() > 16 * 1024, "{}", host.len());

    let mut hashes = HashSet::new();
    let mut cursor = Value::Null;
    loop {
        let page = call(&spaces, "git.log", json!({"limit":100,"cursor":cursor})).unwrap();
        for commit in page["commits"].as_array().unwrap() {
            hashes.insert(commit["hash"].as_str().unwrap().to_owned());
        }
        cursor = page["nextCursor"].clone();
        if cursor.is_null() {
            break;
        }
        assert!(cursor.as_str().unwrap().len() <= 64);
    }
    assert_eq!(hashes.len(), 301);
}

#[test]
fn git_cursor_tokens_keep_the_newest_512_and_forget_expired_ones() {
    let allowlist = ["work".to_owned()];
    let store = std::env::temp_dir().join("workspace-trust.json");
    let spaces = Spaces::new(&store, &allowlist, &[]).unwrap();
    let tokens = spaces.tokens("phone", start());
    let open = |tokens: &Tokens, token: &str| tokens.open("work", "w", token);
    let issued: Vec<String> = (0..CURSORS)
        .map(|n| tokens.issue("work", "w", format!("host-{n}")).unwrap())
        .collect();
    for (n, token) in issued.iter().enumerate() {
        assert_eq!(open(&tokens, token), Some(format!("host-{n}")));
    }
    // One more drops exactly the oldest, whichever device holds the others.
    let other = spaces.tokens("tablet", start());
    let newest = other.issue("work", "w", "host-new".into()).unwrap();
    assert_eq!(open(&tokens, &issued[0]), None);
    assert!(open(&tokens, &issued[1]).is_some());
    assert!(open(&other, &newest).is_some());
    assert_eq!(open(&tokens, &newest), None);
    let held = spaces.cursors.lock().unwrap();
    assert_eq!((held.by_token.len(), held.order.len()), (CURSORS, CURSORS));
    drop(held);

    // Expired ones go as soon as the next token is issued.
    let later = spaces.tokens("phone", start().after(CURSOR_TTL));
    assert_eq!(open(&later, &issued[1]), None);
    let fresh = later.issue("work", "w", "host-fresh".into()).unwrap();
    assert!(open(&later, &fresh).is_some());
    let held = spaces.cursors.lock().unwrap();
    assert_eq!((held.by_token.len(), held.order.len()), (1, 1));
}

#[test]
fn git_branches_sort_tags_newest_first_undated_last_then_by_name() {
    let (repo, spaces) = trusted();
    // The newer instant has the lexically smaller local time: dates are compared
    // as instants, not as text.
    commits_on(
        &repo,
        &[
            ("refs/tags/east".into(), 1_700_000_000, "+1400"),
            ("refs/tags/west".into(), 1_700_003_600, "-1200"),
            ("refs/tags/old".into(), 1_600_000_000, "+0000"),
        ],
    );
    git(&repo.dir, &["tag", "same-b", "HEAD"]);
    git(&repo.dir, &["tag", "same-a", "HEAD"]);
    // A tag on a blob has no date at all.
    let blob = git_input(&repo.dir, &["hash-object", "-w", "--stdin"], "x\n");
    git(&repo.dir, &["tag", "aaa-undated", blob.trim()]);
    let reply = call(&spaces, "git.branches", json!({})).unwrap();
    let names: Vec<&str> = reply["tags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tag| tag["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        ["same-a", "same-b", "west", "east", "old", "aaa-undated"]
    );
    assert_eq!(reply["tags"][5]["date"], "");
}

#[test]
fn git_branches_cap_drops_the_oldest_tags() {
    let (repo, spaces) = trusted();
    let tags: Vec<_> = (0..TAGS + 5)
        .map(|n| {
            let time = 1_500_000_000 + 86_400 * n as i64;
            (format!("refs/tags/t{n:03}"), time, "+0000")
        })
        .collect();
    commits_on(&repo, &tags);
    let reply = call(&spaces, "git.branches", json!({})).unwrap();
    assert_eq!(reply["truncated"], true);
    let names: Vec<&str> = reply["tags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tag| tag["name"].as_str().unwrap())
        .collect();
    assert_eq!(names.len(), TAGS);
    // The 205 tags are older than the repository's own commit: the cut keeps
    // the newest 200 whatever their names.
    assert_eq!(names[0], "t204");
    assert_eq!(names[TAGS - 1], "t005");
}

#[test]
fn git_diff_untracked_refuses_files_git_does_not_list_as_untracked() {
    let (repo, spaces) = trusted();
    repo.write(".gitignore", ".env\n");
    git(&repo.dir, &["add", "--", ".gitignore"]);
    git(&repo.dir, &["commit", "-q", "-m", "ignore"]);
    repo.write(".env", "API_KEY=sk-live-123\n");
    git(
        &repo.dir,
        &[
            "remote",
            "add",
            "origin",
            "https://user:tok3n@example.com/x.git",
        ],
    );
    repo.write("new.txt", "hello\n");
    let diff = |extra: Value| call(&spaces, "git.diff", extra);

    // Ignored, repository-internal (any case), tracked and missing files.
    for path in [".env", ".git/config", ".GIT/config", "a.txt", "missing.txt"] {
        assert_eq!(
            diff(json!({"kind":"worktree","path":path,"untracked":true})),
            Err("invalid-params"),
            "{path}"
        );
    }
    // `.git` is no path for any kind of diff.
    let head = repo.head();
    for extra in [
        json!({"kind":"worktree","path":".git/config"}),
        json!({"kind":"staged","path":".GIT/config"}),
        json!({"kind":"staged","path":"a.txt","origPath":".git/config"}),
        json!({"kind":"commit","path":".Git/config","commit":head}),
        json!({"kind":"commit","path":"a.txt","origPath":".git/config","commit":head}),
    ] {
        assert_eq!(diff(extra.clone()), Err("invalid-params"), "{extra}");
    }
    // What git lists as untracked is still shown.
    let shown = diff(json!({"kind":"worktree","path":"new.txt","untracked":true})).unwrap();
    assert!(shown["text"].as_str().unwrap().contains("+hello\n"));
}

#[test]
fn git_diff_untracked_refuses_another_spelling_of_an_ignored_or_git_path() {
    let (repo, spaces) = trusted();
    repo.write(".gitignore", "secrets/\n");
    git(&repo.dir, &["add", "--", ".gitignore"]);
    git(&repo.dir, &["commit", "-q", "-m", "ignore"]);
    repo.write("secrets/id_rsa", "ID-RSA-PRIVATE\n");
    // A repository copied from a case-sensitive filesystem to a folding one.
    git(&repo.dir, &["config", "core.ignorecase", "false"]);
    let diff = |path: &str| {
        call(
            &spaces,
            "git.diff",
            json!({"kind":"worktree","path":path,"untracked":true}),
        )
    };
    for path in [
        "SECRETS/id_rsa",
        "secrets\u{200c}/id_rsa",
        ".git\u{200c}/config",
        ".git./config",
        "GIT~1/config",
    ] {
        assert_eq!(diff(path), Err("invalid-params"), "{path:?}");
    }
}

#[test]
fn git_diff_of_a_commit_ignores_replace_refs() {
    let (repo, spaces) = trusted();
    let base = repo.head();
    let a = repo.commit("a.txt", "from-A\n", "A");
    git(&repo.dir, &["checkout", "-q", "-b", "other", &base]);
    let b = repo.commit("a.txt", "from-B\n", "B");
    // Honoured, `git show A` would print B.
    git(&repo.dir, &["replace", &a, &b]);
    let shown = call(
        &spaces,
        "git.diff",
        json!({"kind":"commit","path":"a.txt","commit":a}),
    )
    .unwrap();
    let text = shown["text"].as_str().unwrap();
    assert!(text.contains("+from-A\n"), "{text}");
    assert!(!text.contains("from-B"), "{text}");
}

#[test]
fn git_diff_commit_pairs_a_rename_through_orig_path() {
    let (repo, spaces) = trusted();
    let body: String = (0..20).map(|n| format!("line {n}\n")).collect();
    repo.commit("old.txt", &body, "add old");
    git(&repo.dir, &["mv", "old.txt", "new.txt"]);
    repo.write("new.txt", body.replace("line 3\n", "line three\n"));
    git(&repo.dir, &["add", "--", "new.txt"]);
    git(&repo.dir, &["commit", "-q", "-m", "rename"]);
    let hash = repo.head();
    let diff = |extra: Value| {
        let params = json!({"kind":"commit","path":"new.txt","commit":hash});
        let mut params = params;
        for (key, value) in extra.as_object().unwrap() {
            params[key] = value.clone();
        }
        call(&spaces, "git.diff", params).unwrap()["text"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    assert!(diff(json!({})).contains("new file mode"));
    let renamed = diff(json!({"origPath":"old.txt"}));
    assert!(renamed.contains("rename from old.txt\nrename to new.txt\n"));
    assert!(renamed.contains("-line 3\n+line three\n"));
    assert!(!renamed.contains("new file mode"));
}

#[test]
fn git_commit_detail_lists_files_and_caps_them() {
    let (repo, spaces) = trusted();
    let first = repo.head();
    repo.write("b.txt", "bee\n");
    git(&repo.dir, &["add", "--", "b.txt"]);
    let second = repo.commit(
        "a.txt",
        "one\ntwo\nthree\nfour\n",
        "second\n\nthe body\nof it",
    );
    let reply = call(&spaces, "git.commit", json!({ "hash": second })).unwrap();
    assert_eq!(reply["hash"], second);
    assert_eq!(reply["subject"], "second");
    assert!(reply["body"].as_str().unwrap().contains("the body\nof it"));
    assert_eq!(reply["authorName"], "Tester");
    assert_eq!(reply["authorEmail"], "tester@example.com");
    assert!(reply["timestamp"].as_i64().unwrap() > 0);
    assert_eq!(reply["parents"], json!([first]));
    assert_eq!(reply["totalAdditions"], 2);
    assert_eq!(reply["totalDeletions"], 0);
    assert_eq!(reply["truncated"], false);
    let files = reply["files"].as_array().unwrap();
    assert!(files.contains(&json!({"status":"M","path":"a.txt","oldPath":null,"additions":1,"deletions":0,"binary":false})));
    assert!(files.contains(&json!({"status":"A","path":"b.txt","oldPath":null,"additions":1,"deletions":0,"binary":false})));
    assert!(!reply.to_string().contains(SENTINEL));

    // The reply names the commit the way Git does, in lower case.
    let upper = call(
        &spaces,
        "git.commit",
        json!({ "hash": second.to_ascii_uppercase() }),
    )
    .unwrap();
    assert_eq!(upper["hash"], second);
    // A valid hash nobody has: a Git failure, with nothing of it forwarded.
    assert_eq!(
        call(
            &spaces,
            "git.commit",
            json!({"hash":"0123456789abcdef0123456789abcdef01234567"})
        ),
        Err("git-failed")
    );

    for n in 0..=COMMIT_FILES {
        repo.write(&format!("bulk/{n:04}.txt"), "x\n");
    }
    git(&repo.dir, &["add", "--", "bulk"]);
    git(&repo.dir, &["commit", "-q", "-m", "bulk"]);
    let bulk = call(&spaces, "git.commit", json!({ "hash": repo.head() })).unwrap();
    assert_eq!(bulk["files"].as_array().unwrap().len(), COMMIT_FILES);
    assert_eq!(bulk["truncated"], true);
    assert_eq!(bulk["totalAdditions"], COMMIT_FILES + 1);
}

#[test]
fn git_diff_worktree_staged_untracked_commit_rename_binary_and_truncation() {
    let (repo, spaces) = trusted();
    let first = repo.head();
    let diff = |extra: Value| {
        let reply = call(&spaces, "git.diff", extra).unwrap();
        assert!(!reply.to_string().contains(SENTINEL));
        reply
    };
    let text = |reply: &Value| reply["text"].as_str().unwrap().to_owned();

    repo.write("a.txt", "one\nTWO\nthree\n");
    let worktree = diff(json!({"kind":"worktree","path":"a.txt"}));
    assert!(text(&worktree).contains("-two\n+TWO\n"), "{worktree}");
    assert_eq!(worktree["truncated"], false);
    assert_eq!(worktree["binary"], false);
    assert_eq!(
        text(&diff(json!({"kind":"staged","path":"a.txt"}))),
        "",
        "nothing staged yet"
    );

    git(&repo.dir, &["add", "--", "a.txt"]);
    assert!(text(&diff(json!({"kind":"staged","path":"a.txt"}))).contains("+TWO\n"));
    assert_eq!(text(&diff(json!({"kind":"worktree","path":"a.txt"}))), "");

    repo.write("new.txt", "hello\n");
    let untracked = diff(json!({"kind":"worktree","path":"new.txt","untracked":true}));
    assert!(text(&untracked).contains("+hello\n"), "{untracked}");

    git(&repo.dir, &["mv", "a.txt", "renamed.txt"]);
    let renamed = diff(json!({"kind":"staged","path":"renamed.txt","origPath":"a.txt"}));
    assert!(
        text(&renamed).contains("rename to renamed.txt"),
        "{renamed}"
    );

    git(&repo.dir, &["commit", "-q", "-m", "second"]);
    let commit = diff(json!({"kind":"commit","path":"renamed.txt","commit":repo.head()}));
    assert!(text(&commit).contains("+TWO\n"), "{commit}");
    // A root commit diffs against the empty tree.
    let root = diff(json!({"kind":"commit","path":"a.txt","commit":first}));
    assert!(text(&root).contains("+one\n"), "{root}");

    repo.commit("bin.dat", "x", "binary");
    repo.write("bin.dat", [0u8, 1, 2, 0, 255, 0]);
    let binary = diff(json!({"kind":"worktree","path":"bin.dat"}));
    assert_eq!(binary["binary"], true, "{binary}");

    let big = "x".repeat(99) + "\n";
    repo.write("big.txt", big.repeat(3000));
    let cut = diff(json!({"kind":"worktree","path":"big.txt","untracked":true}));
    assert_eq!(cut["truncated"], true);
    assert!(text(&cut).len() <= DIFF_BYTES);
    assert!(text(&cut).ends_with('\n'));
    assert!(text(&cut).len() > DIFF_BYTES - 200);

    // Paths leave the repository: refused.
    for path in ["../outside", "/etc/passwd"] {
        assert_eq!(
            call(&spaces, "git.diff", json!({"kind":"worktree","path":path})),
            Err("git-failed"),
            "{path}"
        );
    }
}

#[test]
fn git_errors_are_fixed_codes_and_nothing_of_the_host_reaches_a_peer() {
    let (repo, spaces) = trusted();
    let head = repo.head();
    let closed = [
        "invalid-params",
        "session-not-allowed",
        "git-unavailable",
        "git-untrusted",
        "git-not-repository",
        "git-cursor-invalid",
        "git-failed",
    ];
    for (method, extra) in [
        ("git.status", json!({})),
        ("git.branches", json!({})),
        ("git.log", json!({})),
        ("git.commit", json!({ "hash": head })),
        ("git.diff", json!({"kind":"worktree","path":"a.txt"})),
    ] {
        let reply = call(&spaces, method, extra).unwrap();
        assert!(!reply.to_string().contains(SENTINEL), "{method}");
    }

    // Trust failures: the Host's own text names the folder; the reply must not.
    let elsewhere = Repo::new();
    elsewhere.commit("a.txt", "x\n", "x");
    let raw = elsewhere
        .desktop()
        .require_trusted(elsewhere.dir.to_str().unwrap())
        .unwrap_err();
    assert!(raw.contains(SENTINEL) && raw.contains("untrustedWorkspace"));
    assert_eq!(
        call(&self::spaces(&elsewhere), "git.status", json!({})),
        Err("git-untrusted")
    );

    // Git failures: its stderr names objects and files; the reply must not.
    let object = repo
        .dir
        .join(".git/objects")
        .join(&head[..2])
        .join(&head[2..]);
    std::fs::remove_file(&object).unwrap();
    std::fs::write(&object, b"").unwrap();
    let raw = yuzora_host::git_service::status_of(&repo.dir, None)
        .err()
        .unwrap();
    assert!(raw.contains(".git/objects"), "{raw}");
    for (method, extra) in [
        ("git.status", json!({})),
        ("git.log", json!({})),
        ("git.commit", json!({ "hash": head })),
    ] {
        assert_eq!(call(&spaces, method, extra), Err("git-failed"), "{method}");
    }
    for code in [
        call(&spaces, "git.status", json!({"workspaceId":"x"})).unwrap_err(),
        call(&spaces, "git.status", json!({"session":"x"})).unwrap_err(),
        parse("git.status", json!({}))
            .err()
            .unwrap_or("invalid-params"),
        parse("git.status", Value::Null).err().unwrap(),
    ] {
        assert!(closed.contains(&code), "{code}");
    }
}

#[test]
fn git_host_errors_map_to_fixed_codes() {
    let trust = |code: &str| {
        format!(r#"{{"error":"{code}","canonicalPath":"/private","challengeId":"c"}}"#)
    };
    for code in [
        "untrustedWorkspace",
        "identityMismatch",
        "storeCorrupt",
        "storeLocked",
    ] {
        assert_eq!(classify(&trust(code)), "git-untrusted", "{code}");
    }
    for error in [
        trust("unsupportedPath"),
        trust("persistFailed"),
        "git status failed: fatal: /private/place".to_owned(),
        r#"{"detail":"untrustedWorkspace"}"#.to_owned(),
        r#"{"error":7}"#.to_owned(),
        "git-repository-replaced".to_owned(),
        "".to_owned(),
    ] {
        assert_eq!(classify(&error), "git-failed", "{error}");
    }
    // Only the log read itself says "cursor", and only for a cursor it was given.
    assert_eq!(classify(CURSOR_INVALID), CURSOR_INVALID);
}

#[test]
fn git_read_audit_is_due_once_a_minute_per_space() {
    let start: Timestamp = serde_json::from_str("\"2030-01-01T00:00:00Z\"").unwrap();
    let after = |seconds| start.after(Duration::from_secs(seconds));
    let mut window = AuditWindow::default();
    assert!(window.due("work", "w", start));
    assert!(!window.due("work", "w", start));
    assert!(!window.due("work", "w", after(59)));
    // Another Space or session has its own minute.
    assert!(window.due("work", "x", after(1)));
    assert!(window.due("other", "w", after(1)));
    assert!(window.due("work", "w", after(60)));
    assert!(!window.due("work", "x", after(60)));
    assert!(window.due("work", "x", after(61)));
    assert!(!window.due("work", "w", after(61)));
    // Expired entries do not pile up.
    assert!(window.due("work", "y", after(10_000)));
    assert_eq!(window.0.len(), 1);
}
