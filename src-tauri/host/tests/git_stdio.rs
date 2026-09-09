#![cfg(unix)]
use serde_json::{json, Value};
use std::process::{Command, Stdio};
use std::time::Duration;
use tokio::io::{AsyncWriteExt, BufReader};
use yuzora_host::protocol::{
    ConnectionOwner, Operation, Outcome, Request, Response, PROTOCOL_VERSION,
};
use yuzora_host::{git_command::GitCommand, trust_command::TrustCommand};

mod support;
use support::helper_binary;

struct Helper {
    child: tokio::process::Child,
    input: Option<tokio::process::ChildStdin>,
    output: BufReader<tokio::process::ChildStdout>,
    owner: ConnectionOwner,
}
impl Helper {
    async fn start(home: &std::path::Path, host: &str) -> Self {
        let mut child = tokio::process::Command::new(helper_binary())
            .arg("--stdio")
            .env("HOME", home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut helper = Self {
            input: child.stdin.take(),
            output: BufReader::new(child.stdout.take().unwrap()),
            child,
            owner: ConnectionOwner {
                host_id: host.into(),
                generation: 1,
            },
        };
        helper.ok(Operation::Hello).await;
        helper
    }
    async fn send(&mut self, operation: Operation) {
        let request = Request {
            version: PROTOCOL_VERSION,
            id: "request".into(),
            owner: self.owner.clone(),
            operation,
        };
        self.input
            .as_mut()
            .unwrap()
            .write_all(format!("{}\n", serde_json::to_string(&request).unwrap()).as_bytes())
            .await
            .unwrap();
    }
    async fn call(&mut self, operation: Operation) -> Outcome {
        self.send(operation).await;
        let bytes = tokio::time::timeout(
            Duration::from_secs(10),
            yuzora_host::wire::read_frame(&mut self.output),
        )
        .await
        .unwrap()
        .unwrap()
        .unwrap();
        let response: Response = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(response.owner, self.owner);
        response.outcome
    }
    async fn ok(&mut self, operation: Operation) -> Value {
        match self.call(operation).await {
            Outcome::Ok { value } => value,
            other => panic!("{other:?}"),
        }
    }
    async fn open(&mut self, path: &std::path::Path) -> String {
        self.ok(Operation::WorkspaceOpen {
            path: path.to_str().unwrap().into(),
        })
        .await["capabilityId"]
            .as_str()
            .unwrap()
            .into()
    }
    async fn trust(&mut self, workspace: &str) {
        let challenge = self
            .ok(Operation::Trust {
                call: TrustCommand::Status {
                    workspace: workspace.into(),
                },
            })
            .await;
        self.ok(Operation::Trust {
            call: TrustCommand::Grant {
                challenge: challenge["challengeId"].as_str().unwrap().into(),
            },
        })
        .await;
    }
    async fn close(mut self) {
        self.input.take();
        assert!(
            tokio::time::timeout(Duration::from_secs(3), self.child.wait())
                .await
                .unwrap()
                .unwrap()
                .success()
        );
    }
}

fn git(path: &std::path::Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
fn init(path: &std::path::Path) {
    std::fs::create_dir_all(path).unwrap();
    git(path, &["init", "-q"]);
    git(path, &["config", "user.name", "Fixture"]);
    git(path, &["config", "user.email", "fixture@example.invalid"]);
    std::fs::write(path.join("中文 file.txt"), "original").unwrap();
    git(path, &["add", "."]);
    git(path, &["commit", "-qm", "fixture"]);
}
fn call(workspace: &str, root: Option<&str>, call: GitCommand) -> Operation {
    Operation::Git {
        workspace: workspace.into(),
        repository_root: root.map(str::to_owned),
        call,
    }
}

#[tokio::test]
async fn git_authority_is_bound_to_workspace_host_and_repository_identity() {
    let home = tempfile::tempdir().unwrap();
    let a = home.path().join("a");
    let b = home.path().join("b");
    init(&a);
    init(&b);
    let root_a = a.canonicalize().unwrap().to_str().unwrap().to_owned();
    let root_b = b.canonicalize().unwrap().to_str().unwrap().to_owned();
    let mut helper = Helper::start(home.path(), "host-a").await;
    let wa = helper.open(&a).await;
    let wb = helper.open(&b).await;
    let untrusted = helper.call(call(&wa, None, GitCommand::Detect)).await;
    assert!(
        matches!(untrusted, Outcome::Error { message, .. } if serde_json::from_str::<Value>(&message).unwrap()["error"] == "untrustedWorkspace")
    );
    helper.trust(&wa).await;
    helper.trust(&wb).await;
    assert_eq!(
        helper.ok(call(&wa, None, GitCommand::Detect)).await["root"],
        root_a
    );
    assert_eq!(
        helper.ok(call(&wb, None, GitCommand::Detect)).await["root"],
        root_b
    );
    std::fs::write(a.join("中文 file.txt"), "modified").unwrap();
    assert_eq!(
        helper
            .ok(call(
                &wa,
                Some(&root_a),
                GitCommand::Status { pathspec: None }
            ))
            .await["unstaged"][0]["path"],
        "中文 file.txt"
    );
    assert_eq!(
        helper
            .ok(call(
                &wb,
                Some(&root_b),
                GitCommand::Status { pathspec: None }
            ))
            .await["unstaged"],
        json!([])
    );
    assert!(
        matches!(helper.call(call(&wa, Some(&root_b), GitCommand::Stage { paths: vec!["中文 file.txt".into()] })).await, Outcome::Error { message, .. } if message == "git-repository-identity-mismatch")
    );
    helper
        .ok(call(
            &wa,
            Some(&root_a),
            GitCommand::Stage {
                paths: vec!["中文 file.txt".into()],
            },
        ))
        .await;
    assert_eq!(
        helper
            .ok(call(
                &wa,
                Some(&root_a),
                GitCommand::Status { pathspec: None }
            ))
            .await["staged"][0]["path"],
        "中文 file.txt"
    );
    let mut other = Helper::start(home.path(), "host-b").await;
    let other_workspace = other.open(&a).await;
    assert!(matches!(
        other
            .call(call(&other_workspace, None, GitCommand::Detect))
            .await,
        Outcome::Error { .. }
    ));
    helper
        .ok(Operation::WorkspaceClose {
            workspace: wa.clone(),
        })
        .await;
    assert!(matches!(
        helper
            .call(call(
                &wa,
                Some(&root_a),
                GitCommand::Status { pathspec: None }
            ))
            .await,
        Outcome::Error { .. }
    ));
    other.close().await;
    helper.close().await;
}

#[tokio::test]
async fn eof_cancels_an_inflight_git_hook_and_does_not_replay_the_commit() {
    use std::os::unix::fs::PermissionsExt;
    let home = tempfile::tempdir().unwrap();
    let repo = home.path().join("repo");
    init(&repo);
    let root = repo.canonicalize().unwrap().to_str().unwrap().to_owned();
    let hook = repo.join(".git/hooks/pre-commit");
    std::fs::write(&hook, "#!/bin/sh\necho $$ > hook.pid\nsleep 30\n").unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(repo.join("中文 file.txt"), "pending").unwrap();
    git(&repo, &["add", "."]);
    let mut helper = Helper::start(home.path(), "eof-host").await;
    let workspace = helper.open(&repo).await;
    helper.trust(&workspace).await;
    helper.ok(call(&workspace, None, GitCommand::Detect)).await;
    helper
        .send(call(
            &workspace,
            Some(&root),
            GitCommand::Commit {
                message: "must not complete".into(),
            },
        ))
        .await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while !repo.join("hook.pid").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let pid: i32 = std::fs::read_to_string(repo.join("hook.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    helper.close().await;
    tokio::time::timeout(Duration::from_secs(3), async {
        while unsafe { libc::kill(pid, 0) } == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let output = Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["rev-list", "--count", "HEAD"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "1");
}

#[tokio::test]
async fn search_stream_delivers_owned_results_then_releases_the_helper() {
    use yuzora_host::stream_protocol::*;
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("中文 file.txt"), "needle\n").unwrap();
    let mut process = tokio::process::Command::new(helper_binary())
        .arg("--stream")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut input = process.stdin.take().unwrap();
    let mut output = BufReader::new(process.stdout.take().unwrap());
    let owner = ConnectionOwner {
        host_id: "search-host".into(),
        generation: 7,
    };
    let request = StreamRequest {
        version: PROTOCOL_VERSION,
        id: "open".into(),
        owner: owner.clone(),
        operation: StreamCommand::Open {
            config: StreamConfig::Search {
                path: root.path().to_str().unwrap().into(),
                query: "needle".into(),
                case_sensitive: true,
            },
        },
    };
    input
        .write_all(format!("{}\n", serde_json::to_string(&request).unwrap()).as_bytes())
        .await
        .unwrap();
    let mut found = false;
    loop {
        let bytes = tokio::time::timeout(
            Duration::from_secs(5),
            yuzora_host::wire::read_frame(&mut output),
        )
        .await
        .unwrap()
        .unwrap()
        .unwrap();
        let frame: StreamFrame = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(frame.owner, owner);
        match frame.payload {
            StreamPayload::Reply {
                outcome: Outcome::Ok { .. },
                ..
            } => {}
            StreamPayload::Search {
                event: yuzora_host::search::SearchEvent::Match { path, matches },
            } => {
                assert!(path.ends_with("中文 file.txt"));
                assert_eq!(matches.len(), 1);
                found = true;
            }
            StreamPayload::Search {
                event:
                    yuzora_host::search::SearchEvent::Done {
                        truncated,
                        file_count,
                    },
            } => {
                assert!(!truncated);
                assert_eq!(file_count, 1);
                break;
            }
            other => panic!("{other:?}"),
        }
    }
    assert!(found);
    assert!(tokio::time::timeout(Duration::from_secs(3), process.wait())
        .await
        .unwrap()
        .unwrap()
        .success());
}
