#![cfg(unix)]
mod support;
use serde_json::{json, Value};
use std::process::Stdio;
use std::time::Duration;
use support::helper_binary;
use tokio::io::{AsyncWriteExt, BufReader};
use yuzora_host::protocol::{
    ConnectionOwner, Operation, Outcome, Request, Response, PROTOCOL_VERSION,
};

#[tokio::test]
async fn account_login_path_reaches_lsp_without_rc_output_entering_rpc() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    let home = directory.path().canonicalize().unwrap();
    let tools = home.join("tools");
    std::fs::create_dir(&tools).unwrap();
    let server = tools.join("vtsls");
    std::fs::write(&server, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&server, std::fs::Permissions::from_mode(0o700)).unwrap();
    let shell = home.join("account-shell");
    std::fs::write(&shell, "#!/bin/sh\nprintf 'account rc banner\\n'\nexport PATH=\"$HOME/tools:/usr/bin:/bin\"\nexport YUZORA_DB_WORKER=ignored\nexec /bin/sh -c \"$2\"\n").unwrap();
    std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut child = tokio::process::Command::new(helper_binary())
        .arg("--stdio")
        .env("HOME", &home)
        .env("SHELL", shell)
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    let owner = json!({"hostId":"login-fixture", "generation":1});
    for (id, operation) in [
        ("hello", json!({"method":"hello"})),
        (
            "open",
            json!({"method":"workspaceOpen", "params":{"path":home}}),
        ),
    ] {
        input
            .write_all(
                format!(
                    "{}\n",
                    json!({"version":1,"id":id,"owner":owner,"operation":operation})
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let bytes = tokio::time::timeout(
            Duration::from_secs(5),
            yuzora_host::wire::read_frame(&mut output),
        )
        .await
        .unwrap()
        .unwrap()
        .unwrap();
        let response: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(response["status"], "ok", "{response}");
        if id == "hello" {
            assert_eq!(response["value"]["home"], json!(home));
        } else {
            let workspace = &response["value"]["capabilityId"];
            input.write_all(format!("{}\n", json!({"version":1,"id":"detect","owner":owner,"operation":{"method":"lspDetect","params":{"workspace":workspace,"language":"typescript"}}})).as_bytes()).await.unwrap();
            let bytes = tokio::time::timeout(
                Duration::from_secs(5),
                yuzora_host::wire::read_frame(&mut output),
            )
            .await
            .unwrap()
            .unwrap()
            .unwrap();
            let response: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(response["status"], "ok", "{response}");
            assert_eq!(response["value"]["path"], json!(server));
        }
    }
    drop(input);
    assert!(tokio::time::timeout(Duration::from_secs(3), child.wait())
        .await
        .unwrap()
        .unwrap()
        .success());
}

#[tokio::test]
async fn built_helper_roundtrip_conflict_and_connection_identity() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("中文 file.txt");
    std::fs::write(&path, "initial").unwrap();
    let mut child = tokio::process::Command::new(helper_binary())
        .arg("--stdio")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    let owner = ConnectionOwner {
        host_id: "host-a".into(),
        generation: 42,
    };
    async fn call(
        input: &mut tokio::process::ChildStdin,
        output: &mut BufReader<tokio::process::ChildStdout>,
        owner: &ConnectionOwner,
        id: &str,
        operation: Operation,
    ) -> Outcome {
        let mut bytes = serde_json::to_vec(&Request {
            version: PROTOCOL_VERSION,
            id: id.into(),
            owner: owner.clone(),
            operation,
        })
        .unwrap();
        bytes.push(b'\n');
        input.write_all(&bytes).await.unwrap();
        let frame = tokio::time::timeout(
            // Account startup can consume its bounded five-second probe budget.
            Duration::from_secs(10),
            yuzora_host::wire::read_frame(output),
        )
        .await
        .unwrap()
        .unwrap()
        .unwrap();
        let response: Response = serde_json::from_slice(&frame).unwrap();
        assert_eq!(response.id, id);
        assert_eq!(response.owner, *owner);
        response.outcome
    }
    fn value(outcome: Outcome) -> Value {
        match outcome {
            Outcome::Ok { value } => value,
            other => panic!("{other:?}"),
        }
    }
    let hello = value(call(&mut input, &mut output, &owner, "hello", Operation::Hello).await);
    assert_eq!(hello["protocol"], json!(PROTOCOL_VERSION));
    let root = value(
        call(
            &mut input,
            &mut output,
            &owner,
            "open",
            Operation::WorkspaceOpen {
                path: directory.path().to_string_lossy().into_owned(),
            },
        )
        .await,
    );
    let workspace = root["capabilityId"].as_str().unwrap().to_owned();
    for _ in 0..2 {
        let listing = value(
            call(
                &mut input,
                &mut output,
                &owner,
                "list",
                Operation::FilesList {
                    workspace: workspace.clone(),
                    path: "".into(),
                },
            )
            .await,
        );
        assert_eq!(listing.as_array().unwrap().len(), 1);
        assert_eq!(listing[0]["name"], "中文 file.txt");
    }
    let file = value(
        call(
            &mut input,
            &mut output,
            &owner,
            "read",
            Operation::FilesRead {
                workspace: workspace.clone(),
                path: "中文 file.txt".into(),
            },
        )
        .await,
    );
    assert_eq!(file["file"]["content"], "initial");
    std::fs::write(&path, "external").unwrap();
    let write = call(
        &mut input,
        &mut output,
        &owner,
        "save",
        Operation::FilesWrite {
            workspace,
            path: "中文 file.txt".into(),
            content: "mine".into(),
            revision: file["revision"].as_str().unwrap().into(),
        },
    )
    .await;
    assert!(matches!(write,Outcome::Error {code,..} if code=="file-conflict"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "external");
    let foreign = ConnectionOwner {
        host_id: "host-b".into(),
        generation: 42,
    };
    assert!(
        matches!(call(&mut input,&mut output,&foreign,"foreign",Operation::Hello).await,Outcome::Error {code,..} if code=="connection-owner-mismatch")
    );
    drop(input);
    let status = tokio::time::timeout(Duration::from_secs(5), child.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(status.success());
}

#[tokio::test]
async fn built_files_stream_delivers_owned_events_and_exits_on_eof() {
    use yuzora_host::stream_protocol::{
        StreamCommand, StreamConfig, StreamFrame, StreamPayload, StreamRequest,
    };
    let root = tempfile::tempdir().unwrap();
    let existing = root.path().join("read-only observation.ts");
    std::fs::write(&existing, "export const value = 1;").unwrap();
    let mut child = tokio::process::Command::new(helper_binary())
        .arg("--stream")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    let owner = ConnectionOwner {
        host_id: "watch-host".into(),
        generation: 7,
    };
    let request = StreamRequest {
        version: PROTOCOL_VERSION,
        owner: owner.clone(),
        id: "open".into(),
        operation: StreamCommand::Open {
            config: StreamConfig::Files {
                path: root.path().to_string_lossy().into_owned(),
            },
        },
    };
    input
        .write_all(format!("{}\n", serde_json::to_string(&request).unwrap()).as_bytes())
        .await
        .unwrap();
    let read = async |output: &mut BufReader<tokio::process::ChildStdout>| {
        let bytes = tokio::time::timeout(
            Duration::from_secs(10),
            yuzora_host::wire::read_frame(output),
        )
        .await
        .unwrap()
        .unwrap()
        .unwrap();
        let frame: StreamFrame = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(frame.owner, owner);
        frame.payload
    };
    assert!(matches!(
        read(&mut output).await,
        StreamPayload::Reply {
            outcome: Outcome::Ok { .. },
            ..
        }
    ));
    // Linux reports IN_OPEN/IN_ACCESS/IN_CLOSE_NOWRITE for editor and LSP
    // reads. These must never invalidate documents or restart the language server.
    #[cfg(target_os = "linux")]
    {
        assert_eq!(
            std::fs::read_to_string(&existing).unwrap(),
            "export const value = 1;"
        );
        assert!(
            tokio::time::timeout(
                Duration::from_millis(900),
                yuzora_host::wire::read_frame(&mut output)
            )
            .await
            .is_err(),
            "reading a file emitted an external-change event"
        );
    }
    std::fs::write(root.path().join("外部 change.txt"), "changed").unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let event = tokio::time::timeout_at(deadline, read(&mut output))
            .await
            .unwrap();
        if let StreamPayload::Files {
            workspace_root,
            paths,
        } = event
        {
            assert_eq!(
                workspace_root,
                root.path().canonicalize().unwrap().to_str().unwrap()
            );
            if paths.iter().any(|p| p.ends_with("外部 change.txt")) {
                break;
            }
        }
    }
    drop(input);
    assert!(tokio::time::timeout(Duration::from_secs(3), child.wait())
        .await
        .unwrap()
        .unwrap()
        .success());
}
