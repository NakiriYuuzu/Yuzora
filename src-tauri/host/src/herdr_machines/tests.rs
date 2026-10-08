use super::*;

const ID: &str = "0123456789abcdef0123456789abcdef";

fn bytes(n: usize, ch: char) -> String {
    ch.to_string().repeat(n)
}

// ── validators ──────────────────────────────────────────────────────────────

#[test]
fn herdr_machine_validate_id_accepts_only_32_lowercase_hex() {
    assert!(validate_machine_id(ID).is_ok());
    for bad in [
        "",
        "0123456789ABCDEF0123456789ABCDEF",
        "0123456789abcdef0123456789abcde",
        "0123456789abcdef0123456789abcdef0",
        "0123456789abcdef0123456789abcdeg",
        "my-label",
    ] {
        assert_eq!(
            validate_machine_id(bad).unwrap_err(),
            "machines-invalid-id",
            "{bad}"
        );
    }
}

#[test]
fn herdr_machine_validate_target_rules() {
    for ok in [
        "host",
        "user@host",
        "ssh://user@host:2222",
        "ssh://u@[::1]",
        "alias-with-dash",
    ] {
        assert!(validate_machine_target(ok).is_ok(), "{ok}");
    }
    for bad in [
        "".to_string(),
        "   ".to_string(),
        "-oProxyCommand=x".to_string(),
        "-x".to_string(),
        " -x".to_string(),
        "user:pw@host".to_string(),
        "ssh://user:pw@host:22".to_string(),
        "host\nname".to_string(),
        "host\u{7}".to_string(),
        bytes(1025, 'a'),
    ] {
        let err = validate_machine_target(&bad).unwrap_err();
        assert!(
            err.starts_with("machines-invalid-target"),
            "{bad:?} -> {err}"
        );
    }
    assert!(validate_machine_target(&bytes(1024, 'a')).is_ok());
    // A colon after the host (port) is not an embedded password.
    assert!(validate_machine_target("user@host:22").is_ok());
}

#[test]
fn herdr_machine_validate_label_counts_bytes_not_chars() {
    assert!(validate_machine_label("  prod  ").is_ok());
    assert!(validate_machine_label(&bytes(128, 'a')).is_ok());
    assert!(validate_machine_label(&bytes(129, 'a')).is_err());
    // 64 x 2-byte chars = 128 bytes ok; 65 = 130 bytes rejected.
    assert!(validate_machine_label(&bytes(64, 'é')).is_ok());
    assert!(validate_machine_label(&bytes(65, 'é')).is_err());
    for bad in ["", "   ", "a\nb", "a\u{1b}[0m"] {
        let err = validate_machine_label(bad).unwrap_err();
        assert!(err.starts_with("machines-invalid-label"), "{bad:?}");
    }
    // A label starting with `-` is safe: it follows the fixed --label flag.
    assert!(validate_machine_label("-dash").is_ok());
}

#[test]
fn herdr_machine_validate_session_rules() {
    assert!(validate_machine_session("default").is_ok());
    assert!(validate_machine_session("a.b_c-d").is_ok());
    assert!(validate_machine_session(&bytes(64, 'a')).is_ok());
    for bad in [
        "".to_string(),
        bytes(65, 'a'),
        ".".to_string(),
        "..".to_string(),
        "a b".to_string(),
        "a/b".to_string(),
        "é".to_string(),
    ] {
        assert_eq!(
            validate_machine_session(&bad).unwrap_err(),
            "machines-invalid-session",
            "{bad}"
        );
    }
}

// ── argv ────────────────────────────────────────────────────────────────────

fn strs(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[test]
fn herdr_machine_argv_matches_the_official_cli_shapes() {
    assert_eq!(
        build_machine_argv(&MachineOp::List),
        strs(&["machine", "list", "--json"])
    );
    assert_eq!(
        build_machine_argv(&MachineOp::Status(ID)),
        strs(&["machine", "status", ID, "--json"])
    );
    assert_eq!(
        build_machine_argv(&MachineOp::Agents(ID)),
        strs(&["--machine", ID, "api", "snapshot"])
    );
    assert_eq!(
        build_machine_argv(&MachineOp::Rename(ID, " -new ")),
        strs(&["machine", "rename", ID, "--label", "-new"])
    );
    assert_eq!(
        build_machine_argv(&MachineOp::Enable(ID)),
        strs(&["machine", "enable", ID])
    );
    assert_eq!(
        build_machine_argv(&MachineOp::Disable(ID)),
        strs(&["machine", "disable", ID])
    );
    assert_eq!(
        build_machine_argv(&MachineOp::Remove(ID)),
        strs(&["machine", "remove", ID])
    );
    assert_eq!(
        build_machine_argv(&MachineOp::Capability),
        strs(&["machine", "status", "--help"])
    );
}

#[test]
fn herdr_machine_interactive_args_put_target_before_flags() {
    let add = HerdrMachineInteractiveSpec::Add {
        target: "user@host".into(),
        remote_session: Some("work".into()),
        label: Some(" prod ".into()),
    };
    assert_eq!(
        build_interactive_args(&add, false).unwrap(),
        strs(&[
            "machine",
            "add",
            "user@host",
            "--remote-session",
            "work",
            "--label",
            "prod"
        ])
    );
    let bare = HerdrMachineInteractiveSpec::Add {
        target: "host".into(),
        remote_session: None,
        label: None,
    };
    assert_eq!(
        build_interactive_args(&bare, false).unwrap(),
        strs(&["machine", "add", "host"])
    );
    assert_eq!(
        build_interactive_args(&HerdrMachineInteractiveSpec::Client, false).unwrap(),
        strs(&["client"])
    );
    let reconnect = HerdrMachineInteractiveSpec::Reconnect {
        machine_id: ID.into(),
    };
    assert_eq!(
        build_interactive_args(&reconnect, false).unwrap(),
        strs(&["machine", "reconnect", ID])
    );
}

#[test]
fn herdr_machine_interactive_args_reject_invalid_input_and_windows_reconnect() {
    let dash = HerdrMachineInteractiveSpec::Add {
        target: "-oProxyCommand=evil".into(),
        remote_session: None,
        label: None,
    };
    assert!(build_interactive_args(&dash, false)
        .unwrap_err()
        .starts_with("machines-invalid-target"));
    let bad_session = HerdrMachineInteractiveSpec::Add {
        target: "host".into(),
        remote_session: Some("..".into()),
        label: None,
    };
    assert_eq!(
        build_interactive_args(&bad_session, false).unwrap_err(),
        "machines-invalid-session"
    );
    let bad_label = HerdrMachineInteractiveSpec::Add {
        target: "host".into(),
        remote_session: None,
        label: Some("  ".into()),
    };
    assert!(build_interactive_args(&bad_label, false)
        .unwrap_err()
        .starts_with("machines-invalid-label"));
    let reconnect = HerdrMachineInteractiveSpec::Reconnect {
        machine_id: ID.into(),
    };
    assert_eq!(
        build_interactive_args(&reconnect, true).unwrap_err(),
        "machines-reconnect-unsupported-windows"
    );
    assert_eq!(
        build_interactive_args(
            &HerdrMachineInteractiveSpec::Reconnect {
                machine_id: "X".into()
            },
            false
        )
        .unwrap_err(),
        "machines-invalid-id"
    );
    // The client PTY is unaffected by the Windows reconnect restriction.
    assert!(build_interactive_args(&HerdrMachineInteractiveSpec::Client, true).is_ok());
}

#[test]
fn herdr_machine_interactive_spec_deserializes_tagged_camel_case_and_rejects_unknown() {
    let add: HerdrMachineInteractiveSpec =
        serde_json::from_str(r#"{"kind":"add","target":"h","remoteSession":"s","label":"l"}"#)
            .unwrap();
    assert_eq!(
        add,
        HerdrMachineInteractiveSpec::Add {
            target: "h".into(),
            remote_session: Some("s".into()),
            label: Some("l".into())
        }
    );
    let reconnect: HerdrMachineInteractiveSpec =
        serde_json::from_str(&format!(r#"{{"kind":"reconnect","machineId":"{ID}"}}"#)).unwrap();
    assert!(matches!(
        reconnect,
        HerdrMachineInteractiveSpec::Reconnect { .. }
    ));
    assert_eq!(
        serde_json::from_str::<HerdrMachineInteractiveSpec>(r#"{"kind":"client"}"#).unwrap(),
        HerdrMachineInteractiveSpec::Client
    );
    assert!(serde_json::from_str::<HerdrMachineInteractiveSpec>(
        r#"{"kind":"add","target":"h","extra":1}"#
    )
    .is_err());
}

// ── version ─────────────────────────────────────────────────────────────────

#[test]
fn herdr_machine_version_parse_and_compare() {
    assert_eq!(parse_version("herdr 0.9.3"), Some((0, 9, 3)));
    assert_eq!(parse_version("herdr 0.9.3\n"), Some((0, 9, 3)));
    assert_eq!(parse_version("herdr v0.10.0-beta.1"), Some((0, 10, 0)));
    assert_eq!(parse_version("0.9"), Some((0, 9, 0)));
    assert_eq!(parse_version("herdr"), None);
    assert_eq!(parse_version(""), None);
    assert!(version_at_least((0, 9, 2), MIN_MACHINES_VERSION));
    assert!(version_at_least((0, 10, 0), MIN_MACHINES_VERSION));
    assert!(version_at_least((1, 0, 0), MIN_MACHINES_VERSION));
    assert!(!version_at_least((0, 9, 1), MIN_MACHINES_VERSION));
    assert!(!version_at_least((0, 8, 9), MIN_MACHINES_VERSION));
}

// ── list / status parsing ───────────────────────────────────────────────────

#[test]
fn herdr_machine_parse_list_official_shape_and_tolerates_unknown_fields() {
    let json = format!(
        r#"[{{"id":"{ID}","label":"prod","target":"u@h","session":"default","enabled":true,"selected":true,"future":{{"x":1}}}},
            {{"id":"{ID}","label":"b","target":"t","enabled":false}}]"#
    );
    let parsed = parse_machine_list(json.as_bytes()).unwrap();
    assert_eq!(parsed.dropped, 0);
    assert_eq!(parsed.machines.len(), 2);
    assert_eq!(
        parsed.machines[0],
        HerdrMachine {
            id: ID.into(),
            label: "prod".into(),
            target: "u@h".into(),
            session: "default".into(),
            enabled: true,
            selected: true
        }
    );
    assert!(!parsed.machines[1].enabled);
    assert!(!parsed.machines[1].selected);
}

#[test]
fn herdr_machine_parse_list_drops_incomplete_rows_and_bad_ids() {
    let json = format!(
        r#"[{{"id":"{ID}","label":"ok","target":"t"}},
            {{"id":"{ID}","target":"no-label"}},
            {{"id":"NOT-HEX","label":"x","target":"t"}},
            "garbage", 7]"#
    );
    let parsed = parse_machine_list(json.as_bytes()).unwrap();
    assert_eq!(parsed.machines.len(), 1);
    assert_eq!(parsed.dropped, 4);
}

#[test]
fn herdr_machine_parse_list_handles_empty_ansi_and_corruption() {
    assert!(parse_machine_list(b"[]").unwrap().machines.is_empty());
    assert!(parse_machine_list(b"\n\n  [ ]  \n\n")
        .unwrap()
        .machines
        .is_empty());
    let ansi =
        format!("\u{1b}[0m\n[{{\"id\":\"{ID}\",\"label\":\"a\",\"target\":\"t\"}}]\u{1b}[0m\n");
    assert_eq!(
        parse_machine_list(ansi.as_bytes()).unwrap().machines.len(),
        1
    );
    for bad in [
        &b"{not json"[..],
        b"",
        b"   ",
        b"\xff\xfe",
        b"\"str\"",
        b"{\"a\":1}",
    ] {
        let err = parse_machine_list(bad).unwrap_err();
        assert!(err.starts_with("machines-parse-failed"), "{err}");
    }
    let huge = vec![b' '; MAX_NDJSON_LINE_BYTES + 1];
    assert_eq!(
        parse_machine_list(&huge).unwrap_err(),
        "machines-output-too-large"
    );
}

#[test]
fn herdr_machine_parse_status_normalizes_the_four_strings() {
    let status = |raw: &str| {
        let json = format!(r#"{{"id":"{ID}","label":"p","status":"{raw}","error":null}}"#);
        parse_machine_status(json.as_bytes(), ID, "fallback").unwrap()
    };
    assert_eq!(status("reachable").status, "reachable");
    assert_eq!(status("auth required").status, "auth-required");
    assert_eq!(status("auth-required").status, "auth-required");
    assert_eq!(status("disabled").status, "disabled");
    let unknown = status("exploded");
    assert_eq!(unknown.status, "error");
    assert_eq!(unknown.error.as_deref(), Some("exploded"));
    let with_error = parse_machine_status(
        format!(r#"{{"id":"{ID}","status":"error","error":"boom"}}"#).as_bytes(),
        ID,
        "fallback",
    )
    .unwrap();
    assert_eq!(
        (with_error.status.as_str(), with_error.error.as_deref()),
        ("error", Some("boom"))
    );
    assert_eq!(with_error.label, "fallback");
}

#[test]
fn herdr_machine_parse_status_selects_the_requested_entry_from_a_list() {
    let other = "ffffffffffffffffffffffffffffffff";
    let json = format!(
        r#"[{{"id":"{other}","label":"o","status":"reachable"}},{{"id":"{ID}","label":"p","status":"auth required"}}]"#
    );
    let got = parse_machine_status(json.as_bytes(), ID, "x").unwrap();
    assert_eq!(
        (got.label.as_str(), got.status.as_str()),
        ("p", "auth-required")
    );
    let two_without_match = format!(
        r#"[{{"id":"{other}","status":"reachable"}},{{"id":"{other}","status":"reachable"}}]"#
    );
    assert!(parse_machine_status(two_without_match.as_bytes(), ID, "x").is_err());
    assert!(parse_machine_status(b"{\"id\":\"x\"}", ID, "x").is_err());
}

// ── snapshot ────────────────────────────────────────────────────────────────

#[test]
fn herdr_machine_folder_name_handles_both_separators() {
    assert_eq!(folder_name("/home/u/proj").as_deref(), Some("proj"));
    assert_eq!(folder_name("/home/u/proj/").as_deref(), Some("proj"));
    assert_eq!(folder_name(r"C:\Users\u\proj").as_deref(), Some("proj"));
    assert_eq!(folder_name(r"C:\Users\u\proj\\").as_deref(), Some("proj"));
    assert_eq!(folder_name("proj").as_deref(), Some("proj"));
    assert_eq!(folder_name("/"), None);
    assert_eq!(folder_name(""), None);
}

fn snapshot_json(extra_agent: &str) -> String {
    format!(
        r#"{{"id":"1","result":{{"type":"session_snapshot","snapshot":{{
          "version":"0.9.3","protocol":22,
          "workspaces":[
            {{"workspace_id":"w1","label":"Repo","agent_status":"working",
              "worktree":{{"repo_name":"yuzora","checkout_path":"/srv/yuzora"}}}},
            {{"workspace_id":"w2","label":null,"agent_status":"idle"}}
          ],
          "panes":[{{"pane_id":"p9","foreground_cwd":"C:\\work\\winproj"}}],
          "agents":[
            {{"terminal_id":"t1","pane_id":"p1","tab_id":"tb1","workspace_id":"w1","agent":"claude",
              "display_agent":"Claude","name":"n","title":"T","cwd":"/srv/yuzora/src","agent_status":"working","focused":true}},
            {{"terminal_id":"t2","pane_id":"p2","tab_id":"tb2","workspace_id":"w1","agent_status":"weird"}},
            {{"pane_id":"p9","tab_id":"tb3","workspace_id":"w2","agent_status":"blocked"}}
            {extra_agent}
          ]}}}}}}"#
    )
}

#[test]
fn herdr_machine_parse_snapshot_extracts_agents_workspaces_and_folders() {
    let snap = parse_snapshot(snapshot_json("").as_bytes(), ID, 42).unwrap();
    assert_eq!((snap.machine_id.as_str(), snap.fetched_at), (ID, 42));
    assert_eq!(snap.server_version.as_deref(), Some("0.9.3"));
    assert_eq!(snap.workspaces.len(), 2);
    assert_eq!(snap.workspaces[0].repo_name.as_deref(), Some("yuzora"));
    assert_eq!(
        snap.workspaces[0].checkout_path.as_deref(),
        Some("/srv/yuzora")
    );
    assert_eq!(snap.workspaces[0].agent_status, "working");
    assert_eq!(snap.workspaces[1].repo_name, None);
    assert_eq!(snap.agents.len(), 3);
    let a = &snap.agents[0];
    assert_eq!(a.folder.as_deref(), Some("src"));
    assert_eq!(a.agent.as_deref(), Some("Claude"));
    assert_eq!(a.workspace_label.as_deref(), Some("Repo"));
    assert!(a.focused);
    assert_eq!(a.status, "working");
    // No cwd on the agent: falls back to the workspace checkout path.
    assert_eq!(snap.agents[1].folder.as_deref(), Some("yuzora"));
    assert_eq!(snap.agents[1].status, "unknown");
    // Windows foreground_cwd is found through the pane; terminal id falls back to pane id.
    assert_eq!(snap.agents[2].folder.as_deref(), Some("winproj"));
    assert_eq!(snap.agents[2].terminal_id, "p9");
    assert_eq!(snap.agents[2].status, "blocked");
}

#[test]
fn herdr_machine_parse_snapshot_accepts_empty_agents_and_rejects_garbage() {
    let empty =
        r#"{"result":{"type":"session_snapshot","snapshot":{"workspaces":[],"agents":[]}}}"#;
    let snap = parse_snapshot(empty.as_bytes(), ID, 1).unwrap();
    assert!(snap.agents.is_empty() && snap.server_version.is_none());
    for bad in [&b"{\"result\":{}}"[..], b"[]", b"nope"] {
        assert!(parse_snapshot(bad, ID, 1)
            .unwrap_err()
            .starts_with("machines-parse-failed"));
    }
}

// ── classification ──────────────────────────────────────────────────────────

fn code(exit: i32, stderr: &str) -> String {
    classify_machine_error(Some(exit), "", stderr).0
}

#[test]
fn herdr_machine_classify_uses_stdout_when_stderr_is_empty() {
    let (code, _) = classify_machine_error(
        Some(1),
        "Error: Custom { kind: Other, error: \"Permission denied (publickey).\" }",
        "",
    );
    assert_eq!(code, "machines-auth-required");
    let (code, _) = classify_machine_error(Some(1), "Permission denied (publickey).", "  \n");
    assert_eq!(code, "machines-auth-required");
    let (code, _) = classify_machine_error(
        Some(1),
        "{\"error\":{\"code\":\"protocol_mismatch\",\"message\":\"x\"}}",
        "",
    );
    assert_eq!(code, "machines-remote-incompatible");
}

#[test]
fn herdr_machine_classify_exit_two_variants() {
    assert_eq!(
        code(2, "error: unknown machine 'x'"),
        "machines-unknown-machine"
    );
    assert_eq!(code(2, "machine 'x' is disabled"), "machines-disabled");
    assert_eq!(
        code(2, "this is not an API-backed machine command"),
        "machines-unsupported-subcommand"
    );
    assert_eq!(code(2, "something else"), "machines-invalid-target");
    let dup = classify_machine_error(Some(2), "", "label 'a' already exists");
    assert_eq!(dup, ("machines-invalid-label".into(), "duplicate".into()));
}

#[test]
fn herdr_machine_classify_profile_not_found_any_exit_code() {
    let msg = "Error: machine profile 0123456789abcdef0123456789abcdef was not found";
    assert_eq!(code(1, msg), "machines-unknown-machine");
    assert_eq!(code(2, msg), "machines-unknown-machine");
}

#[test]
fn herdr_machine_classify_json_errors() {
    let mismatch = r#"{"error":{"code":"protocol_mismatch","message":"bad"}}"#;
    assert_eq!(code(1, mismatch), "machines-remote-incompatible");
    let other = classify_machine_error(
        Some(1),
        "",
        r#"{"error":{"code":"x","message":"it broke"}}"#,
    );
    assert_eq!(other, ("herdr-operation-error".into(), "it broke".into()));
}

#[test]
fn herdr_machine_classify_custom_errors() {
    let refused = r#"Error: Custom { kind: Other, error: "machine 'x': ssh: connect to host h port 22: Connection refused" }"#;
    assert_eq!(code(1, refused), "machines-unreachable");
    let escaped = r#"Error: Custom { kind: Other, error: "machine \"x\": Permission denied (publickey,password)." }"#;
    assert_eq!(code(1, escaped), "machines-auth-required");
    assert_eq!(
        code(
            1,
            r#"Error: Custom { kind: Other, error: "Permission denied (keyboard-interactive)" }"#
        ),
        "machines-auth-required"
    );
    assert_eq!(
        code(
            1,
            r#"Error: Custom { kind: Other, error: "sign_and_send_pubkey: signing failed for ED25519" }"#
        ),
        "machines-auth-required"
    );
    assert_eq!(
        code(
            1,
            r#"Error: Custom { kind: Other, error: "remote does not support machine API forwarding" }"#
        ),
        "machines-remote-incompatible"
    );
    assert_eq!(
        code(
            1,
            r#"Error: Custom { kind: Other, error: "failed to connect to remote Herdr API socket /x" }"#
        ),
        "machines-remote-server-stopped"
    );
    assert_eq!(
        code(
            1,
            r#"Error: Custom { kind: Other, error: "ssh: Could not resolve hostname nope" }"#
        ),
        "machines-unreachable"
    );
    assert_eq!(
        code(
            1,
            r#"Error: Custom { kind: Other, error: "mystery failure" }"#
        ),
        "machines-unreachable"
    );
}

#[test]
fn herdr_machine_custom_error_extraction_undoes_debug_escapes() {
    let raw = r#"Error: Custom { kind: Other, error: "machine \"x\": path C:\\a\nnext" }"#;
    assert_eq!(
        extract_custom_error(raw).as_deref(),
        Some("machine \"x\": path C:\\a\nnext")
    );
    // Unterminated quote keeps what was read; a missing marker yields nothing.
    assert_eq!(
        extract_custom_error(r#"error: "cut"#).as_deref(),
        Some("cut")
    );
    assert_eq!(extract_custom_error("no marker"), None);
}

#[test]
fn herdr_machine_classify_host_key_is_not_auth() {
    let host_key = r#"Error: Custom { kind: Other, error: "Host key verification failed." }"#;
    assert_eq!(code(1, host_key), "machines-host-key");
    let changed = r#"Error: Custom { kind: Other, error: "REMOTE HOST IDENTIFICATION HAS CHANGED! Permission denied (publickey)." }"#;
    assert_eq!(code(1, changed), "machines-host-key");
    let both = r#"Error: Custom { kind: Other, error: "sign_and_send_pubkey: signing failed\r\nHost key verification failed." }"#;
    assert_eq!(code(1, both), "machines-host-key");
}

#[test]
fn herdr_machine_classify_detail_is_sanitized_and_truncated() {
    let stderr = format!(
        "Error: Custom {{ error: \"x\u{1b}[31m\u{7}\" }}\n{}",
        "é".repeat(5000)
    );
    let (_, detail) = classify_machine_error(Some(1), "", &stderr);
    assert!(detail.len() <= 4096);
    assert!(!detail.chars().any(|c| c.is_control()));
    // Plain failures that match nothing do not masquerade as network errors.
    assert_eq!(code(1, "disk quota exceeded"), "herdr-operation-error");
    assert_eq!(code(1, "ssh: Connection timed out"), "machines-unreachable");
    // stdout is the fallback when stderr is empty.
    assert_eq!(
        classify_machine_error(Some(2), "unknown machine q", "").0,
        "machines-unknown-machine"
    );
}

// ── gate / in-flight ────────────────────────────────────────────────────────

#[test]
fn herdr_machine_gate_limits_concurrent_permits() {
    let gate = Gate::new(3);
    let a = gate.try_acquire().unwrap();
    let _b = gate.try_acquire().unwrap();
    let _c = gate.try_acquire().unwrap();
    assert!(gate.try_acquire().is_none());
    drop(a);
    assert!(gate.try_acquire().is_some());
}

#[test]
fn herdr_machine_inflight_claim_rejects_duplicates_until_released() {
    let id = "11111111111111111111111111111111";
    let first = InFlight::claim(id).unwrap();
    assert_eq!(InFlight::claim(id).err().as_deref(), Some("machines-busy"));
    assert!(InFlight::claim("22222222222222222222222222222222").is_ok());
    drop(first);
    assert!(InFlight::claim(id).is_ok());
}

// ── fake binary (unix) ──────────────────────────────────────────────────────

#[cfg(unix)]
mod fake {
    use super::*;
    use crate::herdr_service::{HerdrClientSize, HerdrTerminalEvent};
    use std::io::Write;
    use std::sync::Arc;

    fn write_executable(path: &Path, script: &str) {
        // A forked sibling inheriting a writable fd causes ETXTBSY; write in a
        // short-lived child that exits before the script is executed.
        let mut writer = Command::new("/bin/sh")
            .args(["-c", "cat > \"$1\" && chmod 755 \"$1\"", "writer"])
            .arg(path)
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        writer
            .stdin
            .take()
            .unwrap()
            .write_all(script.as_bytes())
            .unwrap();
        assert!(writer.wait().unwrap().success());
    }

    pub fn fake_herdr(dir: &Path, version: &str, body: &str) -> PathBuf {
        let path = dir.join("herdr");
        let script = format!(
            "#!/bin/sh\n\
             if [ \"$1\" = \"--version\" ]; then echo 'herdr {version}'; exit 0; fi\n\
             if [ \"$1\" = machine ] && [ \"$2\" = status ] && [ \"$3\" = \"--help\" ]; then exit 0; fi\n\
             echo \"$@\" >> \"$(dirname \"$0\")/calls.log\"\n\
             {body}\n"
        );
        write_executable(&path, &script);
        path
    }

    #[test]
    fn herdr_machine_detection_failure_is_not_cached() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("herdr");
        let script = "#!/bin/sh\n\
             d=\"$(dirname \"$0\")\"\n\
             if [ \"$1\" = \"--version\" ]; then\n\
               if [ ! -f \"$d/ok\" ]; then exit 1; fi\n\
               echo 'herdr 0.9.3'; exit 0\n\
             fi\n\
             exit 0\n";
        write_executable(&path, script);
        let manager = HerdrManager::with_binary(path);
        let caps = machines_capabilities(&manager);
        assert_eq!(caps.reason.as_deref(), Some("machines-runtime-too-old"));
        std::fs::write(dir.path().join("ok"), "").unwrap();
        let caps = machines_capabilities(&manager);
        assert!(caps.supported, "{:?}", caps.reason);
        assert_eq!(caps.version.as_deref(), Some("0.9.3"));
    }

    #[test]
    fn herdr_machine_detection_is_not_cached_when_status_probe_fails_to_complete() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("herdr");
        // First probe floods stdout past the output cap (no normal exit);
        // after the flag file exists it behaves.
        let script = "#!/bin/sh\n\
             d=\"$(dirname \"$0\")\"\n\
             if [ \"$1\" = \"--version\" ]; then echo 'herdr 0.9.3'; exit 0; fi\n\
             if [ \"$3\" = \"--help\" ]; then\n\
               if [ ! -f \"$d/ok\" ]; then head -c 3000000 /dev/zero | tr '\\0' x; fi\n\
               exit 0\n\
             fi\n\
             exit 0\n";
        write_executable(&path, script);
        let manager = HerdrManager::with_binary(path);
        let caps = machines_capabilities(&manager);
        assert!(!caps.has_status, "{caps:?}");
        std::fs::write(dir.path().join("ok"), "").unwrap();
        let caps = machines_capabilities(&manager);
        assert!(caps.supported && caps.has_status, "{caps:?}");
    }

    fn calls(dir: &Path) -> String {
        std::fs::read_to_string(dir.join("calls.log")).unwrap_or_default()
    }

    const LIST: &str = r#"
if [ "$1" = machine ] && [ "$2" = list ]; then
  printf '%s' '[{"id":"0123456789abcdef0123456789abcdef","label":"prod","target":"u@h","session":"default","enabled":true,"selected":false},{"id":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","label":"off","target":"u@o","session":"default","enabled":false,"selected":false}]'
  exit 0
fi
"#;

    #[test]
    fn herdr_machine_capabilities_report_version_and_flags() {
        let dir = tempfile::tempdir().unwrap();
        let manager = HerdrManager::with_binary(fake_herdr(dir.path(), "0.9.3", "exit 0"));
        let caps = machines_capabilities(&manager);
        assert!(caps.supported && caps.has_status && caps.has_reconnect);
        assert_eq!(caps.version.as_deref(), Some("0.9.3"));
        assert_eq!(caps.reason, None);

        let old = tempfile::tempdir().unwrap();
        let manager = HerdrManager::with_binary(fake_herdr(old.path(), "0.9.1", "exit 0"));
        let caps = machines_capabilities(&manager);
        assert!(!caps.supported && !caps.has_status);
        assert_eq!(caps.reason.as_deref(), Some("machines-runtime-too-old"));
        assert_eq!(
            machines_list(&manager).unwrap_err(),
            "machines-runtime-too-old"
        );
    }

    #[test]
    fn herdr_machine_list_runs_through_the_cli() {
        let dir = tempfile::tempdir().unwrap();
        let manager = HerdrManager::with_binary(fake_herdr(dir.path(), "0.9.3", LIST));
        let machines = machines_list(&manager).unwrap();
        assert_eq!(machines.len(), 2);
        assert_eq!(machines[0].label, "prod");
        assert!(calls(dir.path()).contains("machine list --json"));
    }

    #[test]
    fn herdr_machine_status_exit_one_with_json_is_a_verdict_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let body = format!(
            "{LIST}
if [ \"$1\" = machine ] && [ \"$2\" = status ]; then
  printf '%s' '{{\"id\":\"{ID}\",\"label\":\"prod\",\"status\":\"auth required\",\"error\":null}}'
  exit 1
fi
exit 9"
        );
        let manager = HerdrManager::with_binary(fake_herdr(dir.path(), "0.9.3", &body));
        let status = machines_status(&manager, ID).unwrap();
        assert_eq!(status.status, "auth-required");
    }

    #[test]
    fn herdr_machine_status_exit_one_without_json_classifies_stderr() {
        let dir = tempfile::tempdir().unwrap();
        let body = format!(
            "{LIST}
if [ \"$1\" = machine ] && [ \"$2\" = status ]; then
  echo 'Error: Custom {{ kind: Other, error: \"Host key verification failed.\" }}' >&2
  exit 1
fi
exit 9"
        );
        let manager = HerdrManager::with_binary(fake_herdr(dir.path(), "0.9.3", &body));
        assert!(machines_status(&manager, ID)
            .unwrap_err()
            .starts_with("machines-host-key"));
    }

    #[test]
    fn herdr_machine_status_disabled_machine_is_not_probed() {
        let dir = tempfile::tempdir().unwrap();
        let body = format!("{LIST}\nexit 2");
        let manager = HerdrManager::with_binary(fake_herdr(dir.path(), "0.9.3", &body));
        let off = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let status = machines_status(&manager, off).unwrap();
        assert_eq!(
            (status.status.as_str(), status.label.as_str()),
            ("disabled", "off")
        );
        assert!(!calls(dir.path()).contains("machine status aaaa"));
        assert_eq!(
            machines_status(&manager, "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb").unwrap_err(),
            "machines-unknown-machine"
        );
        assert_eq!(
            machines_status(&manager, "nope").unwrap_err(),
            "machines-invalid-id"
        );
    }

    #[test]
    fn herdr_machine_agents_fetches_snapshot_and_dedupes_in_flight() {
        let dir = tempfile::tempdir().unwrap();
        let snapshot = snapshot_json("").replace('\n', " ");
        let body = format!(
            "if [ \"$1\" = --machine ]; then cat <<'JSON'\n{snapshot}\nJSON\nexit 0; fi\nexit 9"
        );
        let manager = HerdrManager::with_binary(fake_herdr(dir.path(), "0.9.3", &body));
        let snap = machines_agents(&manager, ID).unwrap();
        assert_eq!(snap.agents.len(), 3);
        assert!(calls(dir.path()).contains(&format!("--machine {ID} api snapshot")));
        let claim = InFlight::claim(ID).unwrap();
        assert_eq!(machines_agents(&manager, ID).unwrap_err(), "machines-busy");
        drop(claim);
        assert!(machines_agents(&manager, ID).is_ok());
    }

    #[test]
    fn herdr_machine_agents_nonzero_exit_is_classified() {
        let dir = tempfile::tempdir().unwrap();
        let body =
            "echo '{\"error\":{\"code\":\"protocol_mismatch\",\"message\":\"m\"}}' >&2\nexit 1";
        let manager = HerdrManager::with_binary(fake_herdr(dir.path(), "0.9.3", body));
        // A distinct id: the dedupe test holds an in-flight claim on ID.
        assert!(
            machines_agents(&manager, "cccccccccccccccccccccccccccccccc")
                .unwrap_err()
                .starts_with("machines-remote-incompatible")
        );
    }

    #[test]
    fn herdr_machine_committed_mutation_reports_a_failed_relist_distinctly() {
        let dir = tempfile::tempdir().unwrap();
        let body =
            "if [ \"$1\" = machine ] && [ \"$2\" = list ]; then echo boom >&2; exit 1; fi\nexit 0";
        let manager = HerdrManager::with_binary(fake_herdr(dir.path(), "0.9.3", body));
        let error = machines_rename(&manager, ID, "new").unwrap_err();
        assert!(error.starts_with("machines-relist-failed"), "{error}");
    }

    #[test]
    fn herdr_machine_mutations_run_then_relist() {
        let dir = tempfile::tempdir().unwrap();
        let body = format!("{LIST}\nexit 0");
        let manager = HerdrManager::with_binary(fake_herdr(dir.path(), "0.9.3", &body));
        assert_eq!(machines_rename(&manager, ID, " new ").unwrap().len(), 2);
        assert_eq!(machines_set_enabled(&manager, ID, false).unwrap().len(), 2);
        assert_eq!(machines_set_enabled(&manager, ID, true).unwrap().len(), 2);
        assert_eq!(machines_remove(&manager, ID).unwrap().len(), 2);
        let log = calls(dir.path());
        for expected in [
            format!("machine rename {ID} --label new"),
            format!("machine disable {ID}"),
            format!("machine enable {ID}"),
            format!("machine remove {ID}"),
        ] {
            assert!(log.contains(&expected), "{expected} in {log}");
        }
        assert!(machines_rename(&manager, ID, "")
            .unwrap_err()
            .starts_with("machines-invalid-label"));
    }

    #[test]
    fn herdr_machine_cli_times_out_and_bounds_output() {
        let dir = tempfile::tempdir().unwrap();
        let slow = fake_herdr(dir.path(), "0.9.3", "sleep 30");
        let started = std::time::Instant::now();
        let err = run_machine_cli(
            &slow,
            &strs(&["machine", "list"]),
            Duration::from_millis(200),
        )
        .unwrap_err();
        assert_eq!(err, "machines-timeout");
        assert!(started.elapsed() < Duration::from_secs(10));

        let big = tempfile::tempdir().unwrap();
        let loud = fake_herdr(
            big.path(),
            "0.9.3",
            "yes 0123456789 | head -c 3000000; exit 0",
        );
        let err = run_machine_cli(&loud, &strs(&["machine", "list"]), Duration::from_secs(20))
            .unwrap_err();
        assert_eq!(err, "machines-output-too-large");
    }

    #[test]
    fn herdr_machine_binary_missing_is_reported() {
        let manager = HerdrManager::with_binary(PathBuf::from("/nonexistent/herdr"));
        assert_eq!(
            machines_list(&manager).unwrap_err(),
            "machines-binary-unavailable"
        );
        let caps = machines_capabilities(&manager);
        assert!(!caps.supported);
        assert_eq!(caps.reason.as_deref(), Some("machines-binary-unavailable"));
    }

    #[test]
    fn herdr_machine_remote_manager_refuses_every_operation() {
        struct Never;
        impl crate::herdr_backend::HerdrRemoteBackend for Never {
            fn metadata(
                &self,
                _: crate::herdr_backend::HerdrMetadata,
                _: Option<&str>,
            ) -> Result<serde_json::Value, String> {
                Err("unused".into())
            }
            fn request(
                &self,
                _: &str,
                _: &str,
                _: serde_json::Value,
            ) -> Result<serde_json::Value, String> {
                Err("unused".into())
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let binary = fake_herdr(dir.path(), "0.9.3", "exit 0");
        let manager = Arc::new(HerdrManager::with_remote(binary, Arc::new(Never)));
        assert_eq!(machines_list(&manager).unwrap_err(), "machines-local-only");
        assert_eq!(
            machines_status(&manager, ID).unwrap_err(),
            "machines-local-only"
        );
        let size = HerdrClientSize {
            cols: 80,
            rows: 24,
            cell_width: 8,
            cell_height: 16,
        };
        let err = machines_interactive_open(
            &manager,
            &HerdrMachineInteractiveSpec::Client,
            size,
            Arc::new(|_| Ok(())),
        )
        .unwrap_err();
        assert_eq!(err, "machines-local-only");
        assert_eq!(
            machines_capabilities(&manager).reason.as_deref(),
            Some("machines-local-only")
        );
    }

    fn collect_output(
        events: &Arc<std::sync::Mutex<String>>,
        needle: &str,
        closed: &Arc<std::sync::atomic::AtomicBool>,
    ) -> String {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let text = events.lock().unwrap().clone();
            if text.contains(needle) || std::time::Instant::now() > deadline {
                return text;
            }
            if closed.load(std::sync::atomic::Ordering::SeqCst) && text.contains(needle) {
                return text;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn collecting_handler() -> (
        OnTerminalEvent,
        Arc<std::sync::Mutex<String>>,
        Arc<std::sync::atomic::AtomicBool>,
    ) {
        use base64::Engine;
        let text = Arc::new(std::sync::Mutex::new(String::new()));
        let closed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (t, c) = (text.clone(), closed.clone());
        let handler: OnTerminalEvent = Arc::new(move |event| {
            match event {
                HerdrTerminalEvent::Frame { bytes_base64, .. } => {
                    let raw = base64::engine::general_purpose::STANDARD
                        .decode(bytes_base64)
                        .unwrap();
                    t.lock().unwrap().push_str(&String::from_utf8_lossy(&raw));
                }
                HerdrTerminalEvent::Closed { .. } => {
                    c.store(true, std::sync::atomic::Ordering::SeqCst)
                }
                #[allow(unreachable_patterns)]
                _ => {}
            }
            Ok(())
        });
        (handler, text, closed)
    }

    fn size() -> HerdrClientSize {
        HerdrClientSize {
            cols: 100,
            rows: 30,
            cell_width: 8,
            cell_height: 16,
        }
    }

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn herdr_machine_pty_client_runs_official_client_without_session_pin() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let body = "printf 'ARGS=[%s] SESSION=[%s] HENV=[%s] TERM=[%s]\\n' \"$*\" \"$HERDR_SESSION\" \"$HERDR_ENV\" \"$TERM\"; exit 0";
        let manager = Arc::new(HerdrManager::with_binary(fake_herdr(
            dir.path(),
            "0.9.3",
            body,
        )));
        std::env::set_var("HERDR_SESSION", "leaked-session");
        std::env::set_var("HERDR_ENV", "1");
        let (handler, text, closed) = collecting_handler();
        let opened = machines_interactive_open(
            &manager,
            &HerdrMachineInteractiveSpec::Client,
            size(),
            handler,
        );
        std::env::remove_var("HERDR_SESSION");
        std::env::remove_var("HERDR_ENV");
        let opened = opened.unwrap();
        assert!(opened.session_id.starts_with("herdr-client-"));
        let out = collect_output(&text, "TERM=[", &closed);
        assert!(out.contains("ARGS=[client]"), "{out}");
        assert!(out.contains("SESSION=[]"), "{out}");
        assert!(out.contains("HENV=[]"), "{out}");
        assert!(out.contains("TERM=[xterm-256color]"), "{out}");
        manager.release_all_connectors();
    }

    #[test]
    fn herdr_machine_pty_add_passes_target_before_flags() {
        let dir = tempfile::tempdir().unwrap();
        let body = "printf 'ARGS=[%s]\\n' \"$*\"; exit 0";
        let manager = Arc::new(HerdrManager::with_binary(fake_herdr(
            dir.path(),
            "0.9.3",
            body,
        )));
        let (handler, text, closed) = collecting_handler();
        machines_interactive_open(
            &manager,
            &HerdrMachineInteractiveSpec::Add {
                target: "u@h".into(),
                remote_session: Some("s1".into()),
                label: Some("L".into()),
            },
            size(),
            handler,
        )
        .unwrap();
        let out = collect_output(
            &text,
            "ARGS=[machine add u@h --remote-session s1 --label L]",
            &closed,
        );
        assert!(
            out.contains("ARGS=[machine add u@h --remote-session s1 --label L]"),
            "{out}"
        );
        manager.release_all_connectors();
    }

    #[test]
    fn herdr_machine_pty_shares_the_sixteen_client_limit() {
        let dir = tempfile::tempdir().unwrap();
        let manager = Arc::new(HerdrManager::with_binary(fake_herdr(
            dir.path(),
            "0.9.3",
            "sleep 60",
        )));
        for _ in 0..16 {
            let (handler, _, _) = collecting_handler();
            machines_interactive_open(
                &manager,
                &HerdrMachineInteractiveSpec::Client,
                size(),
                handler,
            )
            .unwrap();
        }
        let (handler, _, _) = collecting_handler();
        let err = machines_interactive_open(
            &manager,
            &HerdrMachineInteractiveSpec::Client,
            size(),
            handler,
        )
        .unwrap_err();
        assert_eq!(err, "native-client-limit");
        manager.release_all_connectors();
    }

    #[test]
    fn herdr_machine_pty_rejects_invalid_spec_before_spawning() {
        let dir = tempfile::tempdir().unwrap();
        let manager = Arc::new(HerdrManager::with_binary(fake_herdr(
            dir.path(),
            "0.9.3",
            "exit 0",
        )));
        let (handler, _, _) = collecting_handler();
        let err = machines_interactive_open(
            &manager,
            &HerdrMachineInteractiveSpec::Add {
                target: "--evil".into(),
                remote_session: None,
                label: None,
            },
            size(),
            handler,
        )
        .unwrap_err();
        assert!(err.starts_with("machines-invalid-target"));
        assert!(!calls(dir.path()).contains("evil"));
    }
}
