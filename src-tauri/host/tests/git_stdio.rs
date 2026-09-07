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
                call: TrustCommand::Challenge {
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
async fn dev_server_proof_is_bound_to_exact_command_workspace_and_single_use() {
    let home = tempfile::tempdir().unwrap();
    let repo = home.path().join("中文 workspace");
    let other = home.path().join("other");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&other).unwrap();
    let mut control = Helper::start(home.path(), "dev-host").await;
    let workspace = control.open(&repo).await;
    let other = control.open(&other).await;
    control.trust(&workspace).await;
    control.trust(&other).await;
    let command = "echo authorized > must-not-exist";
    let proof = control
        .ok(Operation::Trust {
            call: TrustCommand::ExecutionChallenge {
                workspace: workspace.clone(),
                command: command.into(),
            },
        })
        .await;
    let challenge = proof["challengeId"].as_str().unwrap();
    let authorize = |workspace: &str, command: &str| Operation::DevServerAuthorize {
        workspace: workspace.into(),
        command: command.into(),
        challenge_id: challenge.into(),
    };
    assert!(matches!(
        control.call(authorize(&workspace, "echo changed")).await,
        Outcome::Error { .. }
    ));
    // Failed proofs are consumed too; request a new proof for subsequent cases.
    let proof = control
        .ok(Operation::Trust {
            call: TrustCommand::ExecutionChallenge {
                workspace: workspace.clone(),
                command: command.into(),
            },
        })
        .await;
    assert!(matches!(
        control
            .call(Operation::DevServerAuthorize {
                workspace: other,
                command: command.into(),
                challenge_id: proof["challengeId"].as_str().unwrap().into(),
            })
            .await,
        Outcome::Error { .. }
    ));
    let proof = control
        .ok(Operation::Trust {
            call: TrustCommand::ExecutionChallenge {
                workspace: workspace.clone(),
                command: command.into(),
            },
        })
        .await;
    let authorize = || Operation::DevServerAuthorize {
        workspace: workspace.clone(),
        command: command.into(),
        challenge_id: proof["challengeId"].as_str().unwrap().into(),
    };
    let accepted = control.ok(authorize()).await;
    assert_eq!(
        accepted["canonicalPath"],
        repo.canonicalize().unwrap().to_str().unwrap()
    );
    assert_eq!(accepted["command"], command);
    assert!(matches!(
        control.call(authorize()).await,
        Outcome::Error { .. }
    ));
    assert!(
        !repo.join("must-not-exist").exists(),
        "authorization must never spawn"
    );
    control.close().await;
}

#[tokio::test]
async fn dev_server_stream_rechecks_trust_and_reaps_process_on_eof_or_backpressure() {
    use yuzora_host::stream_protocol::*;
    for mode in ["untrusted", "eof", "backpressure"] {
        let home = tempfile::tempdir().unwrap();
        let repo = home.path().join("中文 workspace");
        std::fs::create_dir_all(&repo).unwrap();
        let mut control = Helper::start(home.path(), "dev-host").await;
        let workspace = control.open(&repo).await;
        if mode != "untrusted" {
            control.trust(&workspace).await;
        }
        let command = if mode == "backpressure" {
            "echo $$ > dev.pid; exec /usr/bin/yes x"
        } else {
            "echo $$ > dev.pid; echo 'Local: http://localhost:5173'; exec /bin/sleep 30"
        };
        let mut child = tokio::process::Command::new(helper_binary())
            .arg("--stream")
            .env("HOME", home.path())
            .env("SHELL", "/bin/sh")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        let mut output = BufReader::new(child.stdout.take().unwrap());
        let request = StreamRequest {
            version: PROTOCOL_VERSION,
            id: "dev".into(),
            owner: control.owner.clone(),
            operation: StreamCommand::Open {
                config: StreamConfig::DevServer {
                    workspace,
                    path: repo.canonicalize().unwrap().to_str().unwrap().into(),
                    command: command.into(),
                    port: None,
                    challenge_id: String::new(),
                },
            },
        };
        input
            .write_all(format!("{}\n", serde_json::to_string(&request).unwrap()).as_bytes())
            .await
            .unwrap();
        if mode == "eof" {
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    let bytes = yuzora_host::wire::read_frame(&mut output)
                        .await
                        .unwrap()
                        .unwrap();
                    let frame: StreamFrame = serde_json::from_slice(&bytes).unwrap();
                    assert_eq!(frame.owner, control.owner);
                    if let StreamPayload::DevServerStatus { info } = frame.payload {
                        if info.port == Some(5173) {
                            break;
                        }
                    }
                }
            })
            .await
            .unwrap();
            drop(input);
        }
        // In backpressure mode stdout intentionally remains unread. The helper
        // must terminate with bounded queues, without waiting for our reader.
        let status = tokio::time::timeout(Duration::from_secs(5), child.wait())
            .await
            .unwrap_or_else(|_| panic!("{mode}: helper did not exit"))
            .unwrap();
        assert_eq!(status.success(), mode == "eof");
        if mode == "untrusted" {
            assert!(!repo.join("dev.pid").exists());
        } else {
            let pid: i32 = std::fs::read_to_string(repo.join("dev.pid"))
                .unwrap()
                .trim()
                .parse()
                .unwrap();
            assert_eq!(
                unsafe { libc::kill(pid, 0) },
                -1,
                "{mode} left process {pid} running"
            );
            assert_eq!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::ESRCH)
            );
        }
        control.close().await;
    }
}

#[tokio::test]
async fn static_preview_enforces_source_host_allowlist_and_workspace_revocation() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("workspace");
    std::fs::create_dir_all(root.join("sub")).unwrap();
    std::fs::write(
        root.join("sub/中文 #%.html"),
        "<script src='app.js'></script>",
    )
    .unwrap();
    std::fs::write(root.join("sub/app.js"), "window.sourceHost = true").unwrap();
    std::fs::write(root.join("sub/secret.txt"), "must not serve").unwrap();
    let mut helper = Helper::start(home.path(), "preview-host").await;
    let outer = helper.open(&root).await;
    let inner = helper.open(&root.join("sub")).await;
    let first = helper
        .ok(Operation::PreviewCreate {
            workspace: outer.clone(),
            path: "sub/中文 #%.html".into(),
        })
        .await;
    let second = helper
        .ok(Operation::PreviewCreate {
            workspace: inner.clone(),
            path: "中文 #%.html".into(),
        })
        .await;
    let _ = rustls::crypto::ring::default_provider().install_default();
    let client = reqwest::Client::builder().build().unwrap();
    let first_url = first["url"].as_str().unwrap();
    let second_url = second["url"].as_str().unwrap();
    assert!(first_url.contains("%23%25.html"));
    assert_eq!(client.get(first_url).send().await.unwrap().status(), 200);
    let asset = first_url.rsplit_once('/').unwrap().0;
    assert_eq!(
        client
            .get(format!("{asset}/app.js"))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
        "window.sourceHost = true"
    );
    let denied = client
        .get(format!("{asset}/secret.txt"))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 403);
    assert_eq!(denied.headers()["x-content-type-options"], "nosniff");
    assert!(
        matches!(helper.call(Operation::PreviewRevoke { workspace: inner.clone(), token: first["token"].as_str().unwrap().into() }).await, Outcome::Error { message, .. } if message == "preview-owner-mismatch")
    );
    helper
        .ok(Operation::WorkspaceClose { workspace: outer })
        .await;
    assert_eq!(client.get(first_url).send().await.unwrap().status(), 404);
    assert_eq!(client.get(second_url).send().await.unwrap().status(), 200);
    helper.close().await;
    assert!(
        client.get(second_url).send().await.is_err(),
        "helper EOF kept static preview serving"
    );
}

#[tokio::test]
async fn install_stream_requires_host_trust_and_eof_cancels_without_replacing_previous_server() {
    use std::os::unix::fs::PermissionsExt;
    use yuzora_host::stream_protocol::*;
    let home = tempfile::tempdir().unwrap();
    let repo = home.path().join("workspace");
    let bin = home.path().join("bin");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&bin).unwrap();
    let npm = bin.join("npm");
    std::fs::write(
        &npm,
        "#!/bin/sh\necho $$ > \"${0%/*}/install.pid\"\nexec /bin/sleep 30\n",
    )
    .unwrap();
    std::fs::set_permissions(&npm, std::fs::Permissions::from_mode(0o700)).unwrap();
    let previous = home.path().join(".yuzora/servers/npm/pyright/previous");
    std::fs::create_dir_all(previous.parent().unwrap()).unwrap();
    std::fs::write(&previous, "keep previous installation").unwrap();
    let mut control = Helper::start(home.path(), "install-host").await;
    let workspace = control.open(&repo).await;
    // The workspace override must be used by the install lane, independently
    // of the account default, which deliberately selects another installer.
    control
        .ok(Operation::LspConfig {
            workspace: workspace.clone(),
            call: yuzora_host::lsp_command::LspConfigCommand::Set {
                language: "python".into(),
                server_id: "pylsp".into(),
                global: true,
            },
        })
        .await;
    control
        .ok(Operation::LspConfig {
            workspace: workspace.clone(),
            call: yuzora_host::lsp_command::LspConfigCommand::Set {
                language: "python".into(),
                server_id: "pyright".into(),
                global: false,
            },
        })
        .await;
    for trusted in [false, true] {
        if trusted {
            control.trust(&workspace).await;
        }
        let mut child = tokio::process::Command::new(helper_binary())
            .arg("--stream")
            .env("HOME", home.path())
            .env("PATH", &bin)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        let mut output = BufReader::new(child.stdout.take().unwrap());
        let request = StreamRequest {
            version: PROTOCOL_VERSION,
            id: "install".into(),
            owner: control.owner.clone(),
            operation: StreamCommand::Open {
                config: StreamConfig::LspInstall {
                    path: repo.canonicalize().unwrap().to_str().unwrap().into(),
                    language: "python".into(),
                    global: false,
                },
            },
        };
        input
            .write_all(format!("{}\n", serde_json::to_string(&request).unwrap()).as_bytes())
            .await
            .unwrap();
        let bytes = tokio::time::timeout(
            Duration::from_secs(3),
            yuzora_host::wire::read_frame(&mut output),
        )
        .await
        .unwrap()
        .unwrap()
        .unwrap();
        let frame: StreamFrame = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(frame.owner, control.owner);
        assert!(matches!(
            frame.payload,
            StreamPayload::Reply {
                outcome: Outcome::Ok { .. },
                ..
            }
        ));
        if trusted {
            tokio::time::timeout(Duration::from_secs(5), async {
                while !bin.join("install.pid").exists() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
        } else {
            let bytes = tokio::time::timeout(
                Duration::from_secs(3),
                yuzora_host::wire::read_frame(&mut output),
            )
            .await
            .unwrap()
            .unwrap()
            .unwrap();
            let frame: StreamFrame = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(frame.owner, control.owner);
            assert!(
                matches!(frame.payload, StreamPayload::LspInstalled { outcome: Outcome::Error { message, .. } } if message.contains("untrustedWorkspace"))
            );
            assert!(!bin.join("install.pid").exists());
        }
        drop(input);
        assert!(tokio::time::timeout(Duration::from_secs(3), child.wait())
            .await
            .unwrap()
            .unwrap()
            .success());
        assert_eq!(
            std::fs::read_to_string(&previous).unwrap(),
            "keep previous installation"
        );
    }
    let pid: i32 = std::fs::read_to_string(bin.join("install.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_eq!(
        unsafe { libc::kill(pid, 0) },
        -1,
        "package manager survived helper EOF"
    );
    // A new helper can acquire the installer lock after the cancelled one exits.
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(home.path().join(".yuzora/servers/.install.lock"))
        .unwrap();
    lock.try_lock().unwrap();
    control.close().await;
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
async fn lsp_stream_frames_messages_on_the_host_and_reaps_on_eof() {
    use std::os::unix::fs::PermissionsExt;
    use yuzora_host::stream_protocol::*;
    let home = tempfile::tempdir().unwrap();
    let repo = home.path().join("workspace");
    std::fs::create_dir(&repo).unwrap();
    let bin = home.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let executable = bin.join("rust-analyzer");
    std::fs::write(
        &executable,
        "#!/bin/sh\necho $$ > \"$YUZORA_LSP_PID\"\nexec /bin/cat\n",
    )
    .unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    let pid_path = home.path().join("lsp.pid");
    let mut control = Helper::start(home.path(), "lsp-host").await;
    let workspace = control.open(&repo).await;
    control.trust(&workspace).await;
    let mut process = tokio::process::Command::new(helper_binary())
        .arg("--stream")
        .env("HOME", home.path())
        .env("PATH", &bin)
        .env("YUZORA_LSP_PID", &pid_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut input = process.stdin.take().unwrap();
    let mut output = BufReader::new(process.stdout.take().unwrap());
    let owner = control.owner.clone();
    async fn send(
        input: &mut tokio::process::ChildStdin,
        owner: &ConnectionOwner,
        id: &str,
        operation: StreamCommand,
    ) {
        let request = StreamRequest {
            version: PROTOCOL_VERSION,
            owner: owner.clone(),
            id: id.into(),
            operation,
        };
        input
            .write_all(format!("{}\n", serde_json::to_string(&request).unwrap()).as_bytes())
            .await
            .unwrap();
    }
    async fn read(output: &mut BufReader<tokio::process::ChildStdout>) -> StreamFrame {
        let bytes = tokio::time::timeout(
            Duration::from_secs(5),
            yuzora_host::wire::read_frame(output),
        )
        .await
        .unwrap()
        .unwrap()
        .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }
    send(
        &mut input,
        &owner,
        "open",
        StreamCommand::Open {
            config: StreamConfig::Lsp {
                path: repo.canonicalize().unwrap().to_str().unwrap().into(),
                language: "rust".into(),
            },
        },
    )
    .await;
    let opened = read(&mut output).await;
    assert_eq!(opened.owner, owner);
    assert!(
        matches!(opened.payload, StreamPayload::Reply { outcome: Outcome::Ok { value }, .. } if value["status"]["status"] == "starting"),
        "LSP did not start"
    );
    let message = serde_json::to_string(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"rootUri":"file:///workspace/中文"}})).unwrap();
    send(
        &mut input,
        &owner,
        "send",
        StreamCommand::LspMessage {
            message: message.clone(),
        },
    )
    .await;
    let mut reply = false;
    let mut echo = false;
    while !reply || !echo {
        let frame = read(&mut output).await;
        assert_eq!(frame.owner, owner);
        match frame.payload {
            StreamPayload::Reply {
                id,
                outcome: Outcome::Ok { .. },
            } => {
                assert_eq!(id, "send");
                reply = true;
            }
            StreamPayload::Lsp { message: received } => {
                assert_eq!(received, message);
                echo = true;
            }
            other => panic!("{other:?}"),
        }
    }
    let pid: i32 = std::fs::read_to_string(pid_path)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    drop(input);
    assert!(tokio::time::timeout(Duration::from_secs(3), process.wait())
        .await
        .unwrap()
        .unwrap()
        .success());
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
    control.close().await;
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
