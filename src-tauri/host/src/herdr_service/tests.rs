use super::*;
use std::fs;
#[cfg(unix)]
use std::io::BufRead;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::sync::mpsc;
use std::time::Duration;
#[cfg(unix)]
use std::time::Instant;

#[cfg(unix)]
#[path = "event_idle_tests.rs"]
mod event_idle_tests;

fn frame(seq: u64, full: bool) -> HerdrWireFrame {
    HerdrWireFrame {
        kind: "terminal.frame".into(),
        seq: Some(seq),
        full: Some(full),
        encoding: Some("ansi".into()),
        width: Some(40),
        height: Some(10),
        bytes: Some("AAA=".into()),
        reason: None,
    }
}

#[test]
fn frame_geometry_is_checked_before_sequence_advances() {
    let mut tracker = FrameTracker::new();
    for (width, height) in [
        (Some(0), Some(24)),
        (Some(u32::MAX), Some(24)),
        (Some(80), Some(1001)),
        (None, None),
        (Some(80), None),
    ] {
        let mut hostile = frame(1, true);
        hostile.width = width;
        hostile.height = height;
        assert_eq!(
            tracker.ingest_frame(&hostile),
            FrameDecision::InvalidGeometry
        );
    }
    assert!(matches!(
        tracker.ingest_frame(&frame(1, true)),
        FrameDecision::Accept(_)
    ));
    let mut delta = frame(2, false);
    delta.width = None;
    delta.height = None;
    assert!(matches!(
        tracker.ingest_frame(&delta),
        FrameDecision::Accept(_)
    ));
}

#[test]
fn herdr_workspace_order_preserves_authoritative_ids_and_rejects_malformed_results() {
    let result = serde_json::json!({"result":{"type":"workspace_list","workspaces":[{"workspace_id":"b"},{"workspace_id":"a"}]}});
    assert_eq!(
        parse_workspace_order(result).unwrap().workspace_ids,
        ["b", "a"]
    );
    for result in [
        serde_json::json!({"result":{"type":"ok"}}),
        serde_json::json!({"result":{"type":"workspace_list","workspaces":[{"workspace_id":"a"},{"workspace_id":"a"}]}}),
        serde_json::json!({"result":{"type":"workspace_list","workspaces":[{}]}}),
    ] {
        assert!(parse_workspace_order(result).is_err());
    }
}

#[test]
fn frame_tracker_requires_first_full_frame() {
    let mut tracker = FrameTracker::new();
    match tracker.ingest_frame(&frame(1, false)) {
        FrameDecision::Resync { message, .. } => {
            assert!(message.contains("first terminal.frame must be full"));
        }
        other => panic!("expected resync, got {other:?}"),
    }
}

#[test]
fn frame_tracker_accepts_contiguous_and_ignores_duplicates() {
    let mut tracker = FrameTracker::new();
    assert!(matches!(
        tracker.ingest_frame(&frame(1, true)),
        FrameDecision::Accept(_)
    ));
    assert!(matches!(
        tracker.ingest_frame(&frame(1, true)),
        FrameDecision::IgnoreDuplicate { seq: 1 }
    ));
    assert!(matches!(
        tracker.ingest_frame(&frame(2, false)),
        FrameDecision::Accept(ParsedTerminalFrame {
            seq: 2,
            full: false,
            ..
        })
    ));
}

#[test]
fn frame_tracker_gap_becomes_resync() {
    let mut tracker = FrameTracker::new();
    assert!(matches!(
        tracker.ingest_frame(&frame(1, true)),
        FrameDecision::Accept(_)
    ));
    match tracker.ingest_frame(&frame(4, false)) {
        FrameDecision::Resync {
            expected_seq: Some(2),
            received_seq: Some(4),
            ..
        } => {}
        other => panic!("expected gap resync, got {other:?}"),
    }
}

#[test]
fn control_command_json_matches_herdr_wire() {
    let input = TerminalControlCommand::input(Some("hi".into()), None).unwrap();
    assert_eq!(
        serde_json::to_string(&input).unwrap(),
        r#"{"type":"terminal.input","text":"hi"}"#
    );
    let bytes = TerminalControlCommand::input(None, Some("eA==".into())).unwrap();
    assert_eq!(
        serde_json::to_string(&bytes).unwrap(),
        r#"{"type":"terminal.input","bytes":"eA=="}"#
    );
    let resize = TerminalControlCommand::resize(80, 24).unwrap();
    assert_eq!(
        serde_json::to_string(&resize).unwrap(),
        r#"{"type":"terminal.resize","cols":80,"rows":24}"#
    );
    let scroll = TerminalControlCommand::scroll(HerdrScrollDirection::Up, 3, None, None).unwrap();
    assert_eq!(
        serde_json::to_string(&scroll).unwrap(),
        r#"{"type":"terminal.scroll","direction":"up","lines":3}"#
    );
    // HERDR encodes a mouse-reporting wheel at this zero-based cell.
    let at_cell =
        TerminalControlCommand::scroll(HerdrScrollDirection::Down, 1, Some(10), Some(5)).unwrap();
    assert_eq!(
        serde_json::to_string(&at_cell).unwrap(),
        r#"{"type":"terminal.scroll","direction":"down","lines":1,"column":10,"row":5}"#
    );
    assert_eq!(
        serde_json::to_string(&TerminalControlCommand::Release).unwrap(),
        r#"{"type":"terminal.release"}"#
    );
}

#[test]
fn control_command_rejects_invalid_combos() {
    assert!(TerminalControlCommand::input(Some("a".into()), Some("eA==".into())).is_err());
    assert!(TerminalControlCommand::input(None, None).is_err());
    assert!(TerminalControlCommand::resize(0, 24).is_err());
    assert!(TerminalControlCommand::scroll(HerdrScrollDirection::Down, 0, None, None).is_err());
}

#[test]
fn parse_snapshot_response_reads_protocol_from_payload() {
    let response = serde_json::json!({
        "id": "1",
        "result": {
            "type": "session_snapshot",
            "snapshot": {
                "version": "0.8.0",
                "protocol": 19,
                "workspaces": [],
                "tabs": [],
                "panes": [],
                "layouts": [],
                "agents": []
            }
        }
    });
    let parsed = parse_snapshot_response(response).unwrap();
    assert_eq!(parsed.protocol, 19);
    assert_eq!(parsed.version, "0.8.0");
    assert_eq!(parsed.snapshot["protocol"], 19);
}

#[test]
fn parse_tab_created_response_reads_root_pane_identity() {
    let response = serde_json::json!({
        "id": "1",
        "result": {
            "type": "tab_created",
            "tab": {
                "tab_id": "tab_9",
                "workspace_id": "ws_1",
                "number": 2,
                "label": "Shell",
                "focused": true,
                "pane_count": 1,
                "agent_status": "idle"
            },
            "root_pane": {
                "pane_id": "pane_9",
                "terminal_id": "term_9",
                "workspace_id": "ws_1",
                "tab_id": "tab_9",
                "focused": true,
                "agent_status": "idle",
                "revision": 1,
                "title": "Shell"
            }
        }
    });
    let parsed = parse_tab_created_response(response).unwrap();
    assert_eq!(parsed.terminal_id, "term_9");
    assert_eq!(parsed.pane_id, "pane_9");
    assert_eq!(parsed.tab_id, "tab_9");
    assert_eq!(parsed.workspace_id, "ws_1");
    assert_eq!(parsed.title.as_deref(), Some("Shell"));
}

#[test]
fn parse_tab_created_response_rejects_cross_tab_or_workspace_identity() {
    let response = |root_tab_id: &str, root_workspace_id: &str| {
        serde_json::json!({
            "id": "1",
            "result": {
                "type": "tab_created",
                "tab": {
                    "tab_id": "tab_9",
                    "workspace_id": "ws_1",
                    "label": "Shell"
                },
                "root_pane": {
                    "pane_id": "pane_9",
                    "terminal_id": "term_9",
                    "workspace_id": root_workspace_id,
                    "tab_id": root_tab_id
                }
            }
        })
    };
    assert_eq!(
        parse_tab_created_response(response("tab_existing", "ws_1")).unwrap_err(),
        "tab_created root_pane tab_id does not match tab"
    );
    assert_eq!(
        parse_tab_created_response(response("tab_9", "ws_other")).unwrap_err(),
        "tab_created root_pane workspace_id does not match tab"
    );
}

#[test]
fn apply_status_json_gates_server_fields() {
    let mut caps = HerdrCapabilities {
        binary_path: Some("/bin/herdr".into()),
        binary_version: None,
        binary_protocol: None,
        channel: None,
        binary_source: HerdrBinarySourceInfo {
            custom_path: None,
            configured: HerdrBinarySource::Global,
            active: HerdrBinarySource::Global,
            resolved: Some(HerdrBinarySource::Global),
            available: true,
            path: Some("/bin/herdr".into()),
            reason: None,
            version: None,
            protocol: None,
            configured_available: true,
            configured_path: Some("/bin/herdr".into()),
            configured_reason: None,
            configured_version: None,
            configured_protocol: None,
            configuration_error: None,
            restart_required: false,
        },
        server: HerdrServerCapability {
            running: false,
            version: None,
            protocol: None,
            compatible: None,
            socket_path: None,
            capabilities: None,
        },
        api: HerdrApiCapability {
            snapshot: false,
            ping: false,
            tab_create: false,
            workspace_focus: false,
            workspace_create: false,
            workspace_move: false,
            workspace_move_block: false,
            workspace_rename: false,
            workspace_close: false,
            tab_rename: false,
            tab_close: false,
            tab_focus: false,
            tab_move: false,
            pane_focus: false,
            pane_rename: false,
            pane_split: false,
            pane_zoom: false,
            pane_swap: false,
            pane_close: false,
            layout_export: false,
            layout_set_split_ratio: false,
            events_subscribe: false,
            worktree_list: false,
            methods: Vec::new(),
            schema_protocol: None,
            schema_version: None,
            reason: None,
        },
        terminal: HerdrTerminalCapability {
            observe: false,
            control: false,
            takeover: false,
            input: false,
            resize: false,
            scroll: false,
            release: false,
            create: false,
            reason: None,
        },
        events: HerdrEventsCapability {
            status: HerdrEventsStatus::Unavailable,
            reason: None,
        },
    };
    let status = serde_json::json!({
        "client": {
            "version": "0.8.0",
            "channel": "stable",
            "protocol": 19,
            "binary": "/Users/me/.local/bin/herdr"
        },
        "server": {
            "status": "running",
            "running": true,
            "version": "0.8.0",
            "protocol": 19,
            "compatible": true,
            "socket": "/tmp/herdr.sock",
            "capabilities": { "live_handoff": true }
        }
    });
    apply_status_json(&mut caps, &status);
    assert_eq!(caps.binary_protocol, Some(19));
    assert_eq!(caps.binary_version.as_deref(), Some("0.8.0"));
    assert!(caps.server.running);
    assert_eq!(caps.server.socket_path.as_deref(), Some("/tmp/herdr.sock"));
    assert_eq!(caps.server.protocol, Some(19));
}

#[test]
fn events_capability_is_unavailable_when_binary_missing() {
    let mgr = HerdrManager::new();
    // Force missing binary so we don't depend on the host install for this assertion.
    *mgr.binary_override.lock().unwrap() = Some(PathBuf::from("/nonexistent/herdr-binary"));
    let caps = mgr.capabilities();
    assert_eq!(caps.events.status, HerdrEventsStatus::Unavailable);
    assert!(caps.events.reason.is_some());
    assert!(!caps.binary_source.available);
    assert!(!caps
        .events
        .reason
        .as_deref()
        .unwrap_or("")
        .contains("only supported on unix hosts"));
}

#[test]
fn events_capability_is_available_for_compatible_running_schema() {
    let mut events = HerdrEventsCapability {
        status: HerdrEventsStatus::Unavailable,
        reason: Some("unset".into()),
    };
    apply_events_capability(&mut events, true, true, false);
    assert_eq!(events.status, HerdrEventsStatus::Available);
    assert!(events.reason.is_none());

    apply_events_capability(&mut events, true, false, false);
    assert_eq!(events.status, HerdrEventsStatus::Unavailable);
    assert_eq!(
        events.reason.as_deref(),
        Some("selected herdr schema lacks events.subscribe")
    );

    apply_events_capability(&mut events, false, true, true);
    assert_eq!(events.status, HerdrEventsStatus::Unavailable);
    assert_eq!(
        events.reason.as_deref(),
        Some("herdr session is not running")
    );
}

#[test]
fn api_request_roundtrip_uses_local_stream_transport() {
    use crate::herdr_transport::{bind_local_listener, unique_local_socket_path};
    use interprocess::local_socket::traits::Listener as _;

    let path = unique_local_socket_path("api-roundtrip");
    let listener = bind_local_listener(&path).unwrap();
    let advertised = path.to_string_lossy().into_owned();
    let server = std::thread::spawn(move || {
        let mut stream = listener.accept().unwrap();
        let mut pending = Vec::new();
        let _ = read_local_ndjson_line(
            &mut stream,
            &mut pending,
            Some(Instant::now() + Duration::from_secs(2)),
            MAX_NDJSON_LINE_BYTES,
        );
        write_local_all_until(
            &mut stream,
            b"{\"result\":{\"type\":\"pong\",\"version\":\"0.8.0\",\"protocol\":19}}\n",
            Instant::now() + Duration::from_secs(2),
        )
        .unwrap();
    });
    let value = api_request(&advertised, "ping", serde_json::json!({})).unwrap();
    assert_eq!(value["result"]["protocol"], 19);
    server.join().unwrap();
    let _ = fs::remove_file(path);
}

#[cfg(unix)]
fn write_source_fixture(dir: &Path, compatible: bool) -> PathBuf {
    let binary = dir.join("herdr-fixture");
    let status = serde_json::json!({"client":{"version":"0.9.0","protocol":22},"server":{"running":true,"version":"0.9.0","protocol":22,"compatible":compatible,"socket":"/preserved.sock"}});
    let schema = serde_json::json!({"protocol":22,"methods":["session.snapshot","events.subscribe","tab.create"]});
    fs::write(&binary, format!("#!/bin/sh\ncase \"$1\" in\nsession) printf '%s\\n' '{{\"sessions\":[{{\"name\":\"default\",\"running\":true}}]}}';;\nstatus) printf '%s\\n' '{status}';;\napi) printf '%s\\n' '{schema}';;\n*) exit 9;;\nesac\n")).unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
    binary
}

#[cfg(unix)]
#[test]
fn verified_custom_source_persists_without_hot_swapping_the_active_client() {
    let dir = tempfile::tempdir().unwrap();
    let binary = write_source_fixture(dir.path(), true);
    let mgr = HerdrManager::new();
    mgr.set_config_dir_for_test(dir.path().join("config"));
    let result = mgr
        .set_binary_source_with_path(
            HerdrBinarySource::Custom,
            Some(binary.to_string_lossy().into_owned()),
        )
        .unwrap();
    assert!(result.restart_required);
    assert_eq!(
        *mgr.active_source.lock().unwrap(),
        HerdrBinarySource::Default
    );
    assert_eq!(
        mgr.binary_source_info().configured_path.as_deref(),
        binary.to_str()
    );
    let reloaded = HerdrManager::new();
    reloaded.set_config_dir_for_test(dir.path().join("config"));
    assert_eq!(reloaded.resolve_binary().as_deref(), Some(binary.as_path()));
    assert!(!reloaded.binary_source_info().restart_required);
}

#[cfg(unix)]
#[test]
fn incompatible_source_does_not_overwrite_a_valid_saved_choice() {
    let dir = tempfile::tempdir().unwrap();
    let good = dir.path().join("good");
    let bad = dir.path().join("bad");
    fs::create_dir_all(&good).unwrap();
    fs::create_dir_all(&bad).unwrap();
    let good_binary = write_source_fixture(&good, true);
    let bad_binary = write_source_fixture(&bad, false);
    let config = dir.path().join("config");
    let mgr = HerdrManager::new();
    mgr.set_config_dir_for_test(config.clone());
    mgr.set_binary_source_with_path(
        HerdrBinarySource::Custom,
        Some(good_binary.to_string_lossy().into_owned()),
    )
    .unwrap();
    let before = fs::read(binary_source_config_path(&config)).unwrap();
    assert!(mgr
        .set_binary_source_with_path(
            HerdrBinarySource::Custom,
            Some(bad_binary.to_string_lossy().into_owned())
        )
        .unwrap_err()
        .contains("runtime-incompatible"));
    assert_eq!(
        fs::read(binary_source_config_path(&config)).unwrap(),
        before
    );
    assert_eq!(
        mgr.binary_source_info().custom_path.as_deref(),
        good_binary.to_str()
    );
}

#[test]
fn corrupt_binary_source_preference_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(binary_source_config_path(dir.path()), "{not-json").unwrap();
    let mgr = HerdrManager::new();
    mgr.set_config_dir_for_test(dir.path().to_path_buf());
    assert!(mgr
        .binary_source_info()
        .configuration_error
        .unwrap()
        .contains("invalid Herdr binary-source preference"));
}

#[test]
fn unavailable_managed_source_cannot_be_saved() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = HerdrManager::new();
    mgr.set_config_dir_for_test(dir.path().to_path_buf());
    assert!(mgr.set_binary_source(HerdrBinarySource::Default).is_err());
    assert!(!binary_source_config_path(dir.path()).exists());
    assert!(!mgr.binary_source_info().configured_available);
}

#[test]
fn managed_binary_layout_prefers_herdr_then_host_and_reports_missing() {
    let dir = tempfile::tempdir().unwrap();
    let fallback = managed_binary_path(dir.path());
    let relative = fallback.strip_prefix(dir.path().join("host")).unwrap();
    let primary = dir.path().join("herdr").join(relative);
    let manager = HerdrManager::new();
    *manager.resource_dir.lock().unwrap() = Some(dir.path().to_path_buf());
    let (path, reason) = manager.resolve_binary_for_source(HerdrBinarySource::Default);
    assert!(path.is_none());
    assert!(reason.unwrap().contains(&fallback.display().to_string()));

    fs::create_dir_all(fallback.parent().unwrap()).unwrap();
    fs::write(&fallback, "host binary").unwrap();
    #[cfg(unix)]
    fs::set_permissions(&fallback, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(managed_binary_path(dir.path()), fallback);
    assert_eq!(manager.resolve_binary(), Some(fallback.clone()));
    let fingerprint = manager.active_binary_fingerprint();
    assert!(fingerprint.is_some());

    fs::create_dir_all(primary.parent().unwrap()).unwrap();
    fs::write(&primary, "primary binary").unwrap();
    #[cfg(unix)]
    fs::set_permissions(&primary, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(managed_binary_path(dir.path()), primary);
    assert_eq!(manager.resolve_binary(), Some(primary.clone()));
    assert_ne!(manager.active_binary_fingerprint(), fingerprint);
    fs::remove_file(primary).unwrap();
    assert_eq!(manager.resolve_binary(), Some(fallback));
}

#[test]
fn custom_source_requires_an_absolute_executable_path() {
    assert!(checked_custom_binary(Some(Path::new("relative/herdr"))).is_err());
    assert!(checked_custom_binary(None).is_err());
}

#[cfg(unix)]
#[test]
fn managed_binary_override_resolves_only_the_default_source() {
    let dir = tempfile::tempdir().unwrap();
    let binary = dir.path().join("herdr-managed");
    fs::write(&binary, "managed Herdr fixture").unwrap();
    #[cfg(unix)]
    {
        let mut permissions = fs::metadata(&binary).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&binary, permissions).unwrap();
    }
    let mgr = HerdrManager::new();
    mgr.set_managed_binary_override(Some(binary.clone()));
    let (resolved, reason) = mgr.resolve_binary_for_source(HerdrBinarySource::Default);
    assert_eq!(resolved.as_deref(), Some(binary.as_path()));
    assert!(reason.is_none());
}

#[test]
fn windows_pathext_parser_keeps_safe_unique_extensions() {
    assert_eq!(
        windows_executable_extensions(Some(".EXE;.CMD;.exe;BAT;.;.PS1-evil")),
        vec![".EXE", ".CMD", ".BAT"]
    );
    assert_eq!(
        windows_executable_extensions(None),
        vec![".EXE", ".CMD", ".BAT", ".COM"]
    );
}

#[test]
fn shared_ndjson_helper_rejects_hostile_frames_before_parse() {
    use crate::herdr_limits::{
        read_bounded_ndjson_line, BoundedNdjsonReadError, HerdrProtocolError, MAX_NDJSON_LINE_BYTES,
    };
    use std::io::Cursor;

    let mut output = String::new();
    let oversized = vec![b'x'; MAX_NDJSON_LINE_BYTES + 1];
    match read_bounded_ndjson_line(&mut Cursor::new(oversized), &mut output) {
        Err(BoundedNdjsonReadError::Protocol(HerdrProtocolError::UnterminatedOverLimit)) => {}
        other => panic!("expected unterminated over-limit, got {other:?}"),
    }

    let mut terminated = vec![b'x'; MAX_NDJSON_LINE_BYTES + 1];
    terminated.push(b'\n');
    match read_bounded_ndjson_line(&mut Cursor::new(terminated), &mut output) {
        Err(BoundedNdjsonReadError::Protocol(HerdrProtocolError::LineTooLarge)) => {}
        other => panic!("expected line too large, got {other:?}"),
    }

    match read_bounded_ndjson_line(&mut Cursor::new([0xff, 0xfe, b'\n']), &mut output) {
        Err(BoundedNdjsonReadError::Protocol(HerdrProtocolError::InvalidUtf8)) => {}
        other => panic!("expected invalid UTF-8, got {other:?}"),
    }
}

#[test]
fn parse_snapshot_rejects_excessive_pane_array() {
    use crate::herdr_limits::MAX_PANE_COUNT;
    let mut panes = Vec::with_capacity(MAX_PANE_COUNT + 1);
    for index in 0..=MAX_PANE_COUNT {
        panes.push(serde_json::json!({ "pane_id": format!("p{index}") }));
    }
    let error = parse_snapshot_response(serde_json::json!({
        "result": {
            "type": "session_snapshot",
            "snapshot": {
                "protocol": 19,
                "version": "0.8.0",
                "panes": panes
            }
        }
    }))
    .unwrap_err();
    assert!(error.contains("tooComplex"), "{error}");
    assert!(error.contains("pane"), "{error}");
}

#[test]
fn parse_snapshot_accepts_pane_count_at_limit() {
    use crate::herdr_limits::MAX_PANE_COUNT;
    let panes = vec![serde_json::json!({ "pane_id": "p" }); MAX_PANE_COUNT];
    let parsed = parse_snapshot_response(serde_json::json!({
        "result": {
            "type": "session_snapshot",
            "snapshot": {
                "protocol": 19,
                "version": "0.8.0",
                "panes": panes
            }
        }
    }))
    .unwrap();
    assert_eq!(parsed.protocol, 19);
    assert_eq!(
        parsed.snapshot["panes"].as_array().unwrap().len(),
        MAX_PANE_COUNT
    );
}

#[test]
fn scroll_capability_reaches_frontend_when_official_schema_supports_it() {
    let manager = HerdrManager::new();
    *manager.binary_override.lock().unwrap() =
        Some(PathBuf::from("/nonexistent/herdr-scroll-fixture"));
    let mut api = manager.capabilities().api;
    let schema: serde_json::Value = serde_json::from_str(include_str!(
        "../../tests/fixtures/herdr-0.9.0-methods.json"
    ))
    .unwrap();
    let methods = collect_schema_methods(&schema);
    assert!(methods.contains("pane.get") && methods.contains("pane.scroll"));
    apply_schema_method_flags(&mut api, &methods, true);
    assert!(api.methods.iter().any(|method| method == "pane.get"));
    assert!(
        api.methods.iter().any(|method| method == "pane.scroll"),
        "the frontend disables every scrollbar without this capability"
    );
}

#[test]
fn implemented_methods_exist_in_official_protocol22_schema() {
    // Keep the old server contract alongside the bundled client inventory.
    for fixture in [
        include_str!("../../tests/fixtures/herdr-0.9.0-methods.json"),
        include_str!("../../tests/fixtures/herdr-0.9.1-methods.json"),
        include_str!("../../tests/fixtures/herdr-0.9.3-methods.json"),
    ] {
        let schema: serde_json::Value = serde_json::from_str(fixture).unwrap();
        assert_eq!(schema["protocol"], 22);
        let methods = collect_schema_methods(&schema);
        for method in IMPLEMENTED_API_METHODS.iter().chain(FEATURE_API_METHODS) {
            assert!(methods.contains(*method), "official schema lacks {method}");
        }
    }
}

#[test]
fn subscriptions_match_official_protocol22_required_fields() {
    // Official 0.9.0 baseline, retained for old server compatibility.
    let schema: serde_json::Value = serde_json::from_str(include_str!(
        "../../tests/fixtures/herdr-0.9.0-subscriptions.json"
    ))
    .unwrap();
    let request =
        subscription_request("test", &["w1:p1".into(), "w2:p1".into(), "w1:p1".into()]).unwrap();
    let selectors = request["params"]["subscriptions"].as_array().unwrap();
    for selector in selectors {
        let variant = schema["oneOf"]
            .as_array()
            .unwrap()
            .iter()
            .find(|variant| variant["properties"]["type"]["const"] == selector["type"])
            .expect("selector must exist in the official schema");
        for required in variant["required"].as_array().unwrap() {
            assert!(
                selector.get(required.as_str().unwrap()).is_some(),
                "{selector} requires {required}"
            );
        }
    }
    assert_eq!(
        selectors
            .iter()
            .filter(|s| s["type"] == "pane.agent_status_changed")
            .count(),
        2
    );
    assert!(subscription_request("test", &[String::new()]).is_err());
    assert!(subscription_request("test", &vec!["w1:p1".into(); MAX_PANE_COUNT + 1]).is_err());
}

#[test]
fn pane_membership_events_trigger_snapshot_refresh() {
    for kind in [
        "pane.created",
        "pane.closed",
        "pane.moved",
        "pane.updated",
        "pane.agent_detected",
        "workspace.renamed",
        "tab.renamed",
    ] {
        let line =
            serde_json::json!({"event":kind,"data":{"pane":{"pane_id":"w1:p1"}}}).to_string();
        assert!(matches!(
            parse_subscription_event_line("sub", &line).unwrap(),
            Some(HerdrSubscriptionEvent::TopologyChanged { .. })
        ));
    }
}

#[test]
fn parse_subscription_event_line_reads_pane_exited() {
    let event = parse_subscription_event_line(
        "sub-exit",
        r#"{"event":"pane.exited","data":{"pane_id":"w1:p2","workspace_id":"w1"}}"#,
    )
    .unwrap()
    .unwrap();
    match event {
        HerdrSubscriptionEvent::PaneExited {
            subscription_id,
            pane_id,
            workspace_id,
        } => {
            assert_eq!(subscription_id, "sub-exit");
            assert_eq!(pane_id, "w1:p2");
            assert_eq!(workspace_id, "w1");
        }
        other => panic!("unexpected event: {other:?}"),
    }
}

#[test]
fn parse_subscription_event_line_reads_tab_topology() {
    let created = parse_subscription_event_line(
        "sub-tab",
        r#"{"event":"tab.created","data":{"tab":{"tab_id":"t2","workspace_id":"w1"}}}"#,
    )
    .unwrap()
    .unwrap();
    match created {
        HerdrSubscriptionEvent::TopologyChanged {
            kind,
            workspace_id,
            tab_id,
            ..
        } => {
            assert_eq!(kind, "tab.created");
            assert_eq!(workspace_id.as_deref(), Some("w1"));
            assert_eq!(tab_id.as_deref(), Some("t2"));
        }
        other => panic!("unexpected event: {other:?}"),
    }

    let moved = parse_subscription_event_line(
        "sub-tab",
        r#"{"event":"tab.moved","data":{"tab_id":"t2","workspace_id":"w1","insert_index":1}}"#,
    )
    .unwrap()
    .unwrap();
    match moved {
        HerdrSubscriptionEvent::TopologyChanged { kind, .. } => {
            assert_eq!(kind, "tab.moved");
        }
        other => panic!("unexpected event: {other:?}"),
    }

    let closed = parse_subscription_event_line(
        "sub-tab",
        r#"{"event":"tab.closed","data":{"tab_id":"t2","workspace_id":"w1"}}"#,
    )
    .unwrap()
    .unwrap();
    match closed {
        HerdrSubscriptionEvent::TopologyChanged {
            kind,
            workspace_id,
            tab_id,
            ..
        } => {
            assert_eq!(kind, "tab.closed");
            assert_eq!(workspace_id.as_deref(), Some("w1"));
            assert_eq!(tab_id.as_deref(), Some("t2"));
        }
        other => panic!("unexpected event: {other:?}"),
    }
}

#[test]
fn parse_subscription_event_line_reads_workspace_topology() {
    for (line, expected) in [
        (
            r#"{"event":"workspace.created","data":{"workspace":{"workspace_id":"w2"}}}"#,
            "workspace.created",
        ),
        (
            r#"{"event":"workspace.closed","data":{"workspace_id":"w2"}}"#,
            "workspace.closed",
        ),
        (
            r#"{"event":"workspace.moved","data":{"workspace_id":"w2","insert_index":1}}"#,
            "workspace.moved",
        ),
        (
            r#"{"event":"workspace.reordered","data":{"workspace_ids":["w1","w2"]}}"#,
            "workspace.reordered",
        ),
    ] {
        let event = parse_subscription_event_line("sub-ws", line)
            .unwrap()
            .unwrap();
        match event {
            HerdrSubscriptionEvent::TopologyChanged { kind, .. } => {
                assert_eq!(kind, expected);
            }
            other => panic!("unexpected event for {expected}: {other:?}"),
        }
    }
}

#[test]
fn parse_subscription_event_line_reads_agent_status_changed() {
    let event = parse_subscription_event_line(
            "sub-1",
            r#"{"event":"pane.agent_status_changed","data":{"pane_id":"w1:p1","workspace_id":"w1","agent_status":"done","title":"Review"}}"#,
        )
        .unwrap()
        .unwrap();
    match event {
        HerdrSubscriptionEvent::AgentStatusChanged {
            pane_id,
            agent_status,
            title,
            ..
        } => {
            assert_eq!(pane_id, "w1:p1");
            assert_eq!(agent_status, "done");
            assert_eq!(title.as_deref(), Some("Review"));
        }
        other => panic!("unexpected event: {other:?}"),
    }
}

#[test]
fn subscription_event_serializes_agent_status_changed_with_camel_case_fields() {
    let event = HerdrSubscriptionEvent::AgentStatusChanged {
        subscription_id: "sub-1".to_owned(),
        pane_id: "w1:p1".to_owned(),
        workspace_id: "w1".to_owned(),
        agent_status: "working".to_owned(),
        agent: Some("pi".to_owned()),
        display_agent: Some("Pi".to_owned()),
        title: Some("Review".to_owned()),
        state_labels: HashMap::from([("working".to_owned(), "Working".to_owned())]),
    };

    assert_eq!(
        serde_json::to_value(event).unwrap(),
        serde_json::json!({
            "type": "agent_status_changed",
            "subscriptionId": "sub-1",
            "paneId": "w1:p1",
            "workspaceId": "w1",
            "agentStatus": "working",
            "agent": "pi",
            "displayAgent": "Pi",
            "title": "Review",
            "stateLabels": { "working": "Working" }
        })
    );
}

#[test]
fn collect_schema_methods_reads_methods_array_and_request_union() {
    let schema = serde_json::json!({
        "protocol": 19,
        "schema_version": 1,
        "methods": ["session.snapshot", "tab.create", "session.ping", "plugin.pane.open"],
        "schemas": {
            "request": {
                "oneOf": [
                    { "properties": { "method": { "const": "session.snapshot" } } },
                    { "properties": { "method": { "const": "tab.create" } } },
                    { "properties": { "method": { "const": "plugin.action.invoke" } } }
                ]
            }
        }
    });
    let methods = collect_schema_methods(&schema);
    assert!(methods.contains("session.snapshot"));
    assert!(methods.contains("tab.create"));
    assert!(methods.contains("session.ping"));
    assert!(methods.contains("plugin.pane.open"));
    assert!(methods.contains("plugin.action.invoke"));
}

#[test]
fn nested_plugin_methods_reach_feature_capabilities() {
    let manager = HerdrManager::new();
    *manager.binary_override.lock().unwrap() =
        Some(PathBuf::from("/nonexistent/herdr-plugin-fixture"));
    let mut api = manager.capabilities().api;
    let schema: serde_json::Value = serde_json::from_str(include_str!(
        "../../tests/fixtures/herdr-0.9.3-methods.json"
    ))
    .unwrap();
    apply_schema_method_flags(&mut api, &collect_schema_methods(&schema), true);
    for method in [
        "plugin.pane.open",
        "plugin.action.invoke",
        "plugin.log.list",
    ] {
        assert!(api.methods.iter().any(|available| available == method));
    }
    for invalid in [
        "plugin",
        "plugin..open",
        ".pane.open",
        "pane.open.",
        "pane.open now",
    ] {
        assert!(
            !looks_like_api_method(invalid),
            "accepted invalid method {invalid}"
        );
    }
}

#[test]
fn collect_schema_methods_empty_schemas_reports_no_methods() {
    let schema = serde_json::json!({
        "protocol": 19,
        "schema_version": 1,
        "schemas": {}
    });
    assert!(collect_schema_methods(&schema).is_empty());
}

#[cfg(unix)]
fn write_executable_fixture(path: &Path, script: &str) {
    // Another parallel test can fork while fs::write holds a writable FD,
    // inheriting it until exec and causing Linux ETXTBSY. Open the file only
    // in a dedicated child and wait for that writer to exit before execution.
    let mut writer = Command::new("/bin/sh")
        .args(["-c", "cat > \"$1\" && chmod 755 \"$1\"", "fixture-writer"])
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

#[cfg(unix)]
fn write_fake_herdr_with(dir: &Path, status_json: &str, schema_json: &str) -> PathBuf {
    write_fake_herdr_with_sessions(
        dir,
        status_json,
        schema_json,
        r#"{"sessions":[{"name":"default","default":true,"running":false,"session_dir":"/tmp/herdr-default","socket_path":"/tmp/herdr.sock"}]}"#,
    )
}

#[cfg(unix)]
fn write_fake_herdr_with_sessions(
    dir: &Path,
    status_json: &str,
    schema_json: &str,
    sessions_json: &str,
) -> PathBuf {
    let path = dir.join("herdr");
    let script = format!(
        r#"#!/bin/sh
set -e
if [ "$1" = "session" ] && [ "$2" = "list" ] && [ "$3" = "--json" ]; then
  cat <<'JSON'
{sessions}
JSON
  exit 0
fi
if [ "$1" = "status" ] && [ "$2" = "--json" ]; then
  cat <<'JSON'
{status}
JSON
  exit 0
fi
if [ "$1" = "api" ] && [ "$2" = "schema" ] && [ "$3" = "--json" ]; then
  cat <<'JSON'
{schema}
JSON
  exit 0
fi
if [ "$1" = "terminal" ] && [ "$2" = "session" ]; then
  mode="$3"
  # Echo HERDR_SESSION to a side channel file when present (tests inspect env).
  if [ -n "${{HERDR_SESSION:-}}" ] && [ -n "${{HERDR_TEST_ENV_FILE:-}}" ]; then
    # Expose the empty-file interval before the shell writes its result.
    : > "$HERDR_TEST_ENV_FILE"
    sleep 0.05
    printf '%s\n' "$HERDR_SESSION" > "$HERDR_TEST_ENV_FILE"
  fi
  printf '%s\n' '{{"type":"terminal.frame","seq":1,"full":true,"encoding":"ansi","width":40,"height":10,"bytes":"AAA="}}'
  if [ "$mode" = "control" ]; then
    while IFS= read -r line; do
      case "$line" in
        *terminal.release*)
          printf '%s\n' '{{"type":"terminal.closed","reason":"detached"}}'
          exit 0
          ;;
        *terminal.resize*)
          printf '%s\n' '{{"type":"terminal.frame","seq":2,"full":false,"encoding":"ansi","width":40,"height":10,"bytes":"AQE="}}'
          ;;
      esac
    done
  else
    sleep 2
  fi
  exit 0
fi
echo "unexpected args: $*" >&2
exit 2
"#,
        sessions = sessions_json,
        status = status_json,
        schema = schema_json,
    );
    write_executable_fixture(&path, &script);
    path
}

#[cfg(unix)]
fn write_fake_herdr_startup(dir: &Path) -> PathBuf {
    let path = dir.join("herdr");
    write_executable_fixture(
        &path,
        r#"#!/bin/sh
set -e
base=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
ready="$base/server.ready"
invoked="$base/server.invoked"
if [ "$1" = "--session" ]; then shift 2; fi
if [ "$1" = "status" ] && [ "$2" = "--json" ]; then
  if [ -f "$ready" ] && [ -f "$base/server.incompatible" ]; then
    printf '%s\n' '{"server":{"status":"running","running":true,"version":"0.8.0","protocol":19,"compatible":false,"socket":"/tmp/herdr-fake.sock"}}'
  elif [ -f "$ready" ]; then
    printf '%s\n' '{"server":{"status":"running","running":true,"version":"0.0.0-fake","protocol":20,"compatible":true,"socket":"/tmp/herdr-fake.sock"}}'
  else
    printf '%s\n' '{"server":{"status":"not_running","running":false,"version":null,"protocol":null,"compatible":null,"socket":null}}'
  fi
  exit 0
fi
if [ "$1" = "server" ] && [ -z "${2:-}" ]; then
  printf '%s\n' x >> "$invoked"
  sleep 0.05
  : > "$ready"
  exit 0
fi
echo "unexpected args: $*" >&2
exit 2
"#,
    );
    path
}

#[cfg(unix)]
fn write_fake_herdr_startup_exit(dir: &Path) -> PathBuf {
    let path = dir.join("herdr");
    write_executable_fixture(
        &path,
        r#"#!/bin/sh
set -e
if [ "$1" = "status" ] && [ "$2" = "--json" ]; then
  printf '%s\n' '{"server":{"status":"not_running","running":false,"version":null,"protocol":null,"compatible":null,"socket":null}}'
  exit 0
fi
if [ "$1" = "server" ] && [ -z "${2:-}" ]; then
  exit 23
fi
if [ "$1" = "session" ] && [ "$2" = "list" ] && [ "$3" = "--json" ]; then
  printf '%s\n' '{"sessions":[{"name":"default","default":true,"running":false,"session_dir":"/tmp/herdr-default","socket_path":"/tmp/herdr.sock"}]}'
  exit 0
fi
if [ "$1" = "api" ] && [ "$2" = "schema" ] && [ "$3" = "--json" ]; then
  printf '%s\n' '{"protocol":19,"schema_version":1,"methods":[]}'
  exit 0
fi
echo "unexpected args: $*" >&2
exit 2
"#,
    );
    path
}

#[cfg(unix)]
fn write_fake_herdr_startup_hang(dir: &Path) -> PathBuf {
    let path = dir.join("herdr");
    write_executable_fixture(
        &path,
        r#"#!/bin/sh
set -e
base=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
pid_file="$base/server.pid"
if [ "$1" = "status" ] && [ "$2" = "--json" ]; then
  if [ -f "$pid_file" ]; then
    sleep 5
    : > "$base/probe.completed"
  fi
  printf '%s\n' '{"server":{"status":"not_running","running":false,"version":null,"protocol":null,"compatible":null,"socket":null}}'
  exit 0
fi
if [ "$1" = "server" ] && [ -z "${2:-}" ]; then
  printf '%s\n' "$$" > "$pid_file"
  exec sleep 30
fi
echo "unexpected args: $*" >&2
exit 2
"#,
    );
    path
}

#[test]
#[cfg(unix)]
fn startup_launches_resolved_headless_server_and_waits_until_ready() {
    let dir = tempfile::tempdir().unwrap();
    let binary = write_fake_herdr_startup(dir.path());
    let manager = HerdrManager::with_binary(binary);

    let started = manager.ensure_server_running_on_startup().unwrap();

    assert!(started, "a stopped resolved Herdr server must be launched");
    assert!(
        dir.path().join("server.invoked").is_file(),
        "startup must invoke the resolved binary as `herdr server`"
    );
    assert!(
        dir.path().join("server.ready").is_file(),
        "startup must not return before Herdr reports running"
    );
}

#[test]
#[cfg(unix)]
fn startup_and_named_session_start_share_one_launch_lock() {
    let dir = tempfile::tempdir().unwrap();
    let manager = Arc::new(HerdrManager::with_binary(write_fake_herdr_startup(
        dir.path(),
    )));
    let guard = manager.startup_lock.lock().unwrap();
    let (send, receive) = mpsc::channel();
    let workers: Vec<_> = [false, true]
        .into_iter()
        .map(|named| {
            let manager = manager.clone();
            let send = send.clone();
            std::thread::spawn(move || {
                send.send(()).unwrap();
                if named {
                    manager
                        .feature("default", HerdrFeatureRequest::SessionStart {})
                        .unwrap()["started"]
                        .as_bool()
                        .unwrap()
                } else {
                    manager.ensure_server_running_on_startup().unwrap()
                }
            })
        })
        .collect();
    receive.recv().unwrap();
    receive.recv().unwrap();
    std::thread::sleep(Duration::from_millis(100));
    assert!(!dir.path().join("server.invoked").exists());
    drop(guard);
    let starts = workers
        .into_iter()
        .filter_map(|worker| worker.join().unwrap().then_some(()))
        .count();
    assert_eq!(starts, 1);
    assert_eq!(spawn_count(&dir.path().join("server.invoked")), 1);
}

#[test]
#[cfg(unix)]
fn startup_keeps_an_existing_herdr_server() {
    let dir = tempfile::tempdir().unwrap();
    let binary = write_fake_herdr_startup(dir.path());
    fs::write(dir.path().join("server.ready"), "").unwrap();
    let manager = HerdrManager::with_binary(binary);

    let started = manager.ensure_server_running_on_startup().unwrap();

    assert!(!started, "an already-running Herdr server must be reused");
    assert!(!dir.path().join("server.invoked").exists());
}

#[test]
#[cfg(unix)]
fn startup_rejects_an_existing_incompatible_server_without_stopping_it() {
    let dir = tempfile::tempdir().unwrap();
    let binary = write_fake_herdr_startup(dir.path());
    fs::write(dir.path().join("server.ready"), "").unwrap();
    fs::write(dir.path().join("server.incompatible"), "").unwrap();
    let manager = HerdrManager::with_binary(binary);

    let error = manager.ensure_server_running_on_startup().unwrap_err();

    assert!(error.contains("protocol incompatible"));
    assert!(error.contains("preserving running sessions"));
    assert!(dir.path().join("server.ready").exists());
    assert!(!dir.path().join("server.invoked").exists());
    assert_eq!(
        manager.startup_error.lock().unwrap().as_deref(),
        Some(error.as_str())
    );
}

#[test]
#[cfg(unix)]
fn startup_failure_is_retained_in_capability_diagnostics() {
    let dir = tempfile::tempdir().unwrap();
    let binary = write_fake_herdr_startup_exit(dir.path());
    let manager = HerdrManager::with_binary(binary);

    let error = manager.ensure_server_running_on_startup().unwrap_err();
    assert!(error.contains("exited before becoming ready"));

    let caps = manager.capabilities();
    for reason in [
        caps.api.reason.as_deref(),
        caps.terminal.reason.as_deref(),
        caps.events.reason.as_deref(),
    ] {
        let reason = reason.expect("failed startup must remain user-visible");
        assert!(reason.contains("herdr automatic startup failed"));
        assert!(reason.contains(&error), "diagnostic must retain: {error}");
    }
}

#[test]
#[cfg(unix)]
fn startup_timeout_bounds_probe_and_reaps_spawned_server() {
    let dir = tempfile::tempdir().unwrap();
    let binary = write_fake_herdr_startup_hang(dir.path());
    let manager = HerdrManager::with_binary(binary);

    let error = manager
        .ensure_server_running_with_timeouts(
            Duration::from_millis(250),
            Duration::from_secs(30),
            Duration::from_millis(10),
        )
        .unwrap_err();

    assert!(error.contains("did not become ready"));
    assert!(
        !dir.path().join("probe.completed").exists(),
        "a readiness probe must be stopped at the startup deadline"
    );
    let pid = fs::read_to_string(dir.path().join("server.pid"))
        .unwrap()
        .trim()
        .to_string();
    let alive = Command::new("kill")
        .args(["-0", &pid])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if alive {
        let _ = Command::new("kill")
            .args(["-9", &pid])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    assert!(!alive, "timed-out startup child {pid} must be reaped");
}

#[cfg(unix)]
fn write_fake_herdr(dir: &Path) -> PathBuf {
    write_fake_herdr_with(
        dir,
        r#"{"client":{"version":"0.0.0-fake","channel":"test","protocol":19,"binary":"FAKE"},"server":{"status":"not_running","running":false,"version":null,"protocol":null,"compatible":null,"socket":null},"update":{"restart_needed":false}}"#,
        r#"{"protocol":19,"schema_version":1,"schemas":{}}"#,
    )
}

#[cfg(unix)]
fn write_fake_herdr_running_session(dir: &Path) -> PathBuf {
    write_fake_herdr_with_sessions(
        dir,
        r#"{"client":{"version":"0.0.0-fake","channel":"test","protocol":19,"binary":"FAKE"},"server":{"status":"running","running":true,"version":"0.0.0-fake","protocol":19,"compatible":true,"socket":"/tmp/herdr.sock"},"update":{"restart_needed":false}}"#,
        r#"{"protocol":19,"schema_version":1,"methods":["session.snapshot","tab.create","workspace.focus","workspace.create","session.ping"],"schemas":{"session.snapshot":{},"tab.create":{},"workspace.focus":{},"workspace.create":{}}}"#,
        r#"{"sessions":[{"name":"default","default":true,"running":true,"session_dir":"/tmp/herdr-default","socket_path":"/tmp/herdr.sock"}]}"#,
    )
}

#[cfg(unix)]
fn write_fake_herdr_event_session(dir: &Path, socket: &Path) -> PathBuf {
    let socket = socket.to_string_lossy();
    let status = serde_json::json!({
        "client": {
            "version": "0.0.0-fake",
            "channel": "test",
            "protocol": 19,
            "binary": "FAKE"
        },
        "server": {
            "status": "running",
            "running": true,
            "version": "0.0.0-fake",
            "protocol": 19,
            "compatible": true,
            "socket": socket
        }
    })
    .to_string();
    let schema = serde_json::json!({
        "protocol": 19,
        "schema_version": 1,
        "methods": ["session.snapshot", "tab.create", "events.subscribe"]
    })
    .to_string();
    let sessions = serde_json::json!({
        "sessions": [{
            "name": "default",
            "default": true,
            "running": true,
            "session_dir": "/tmp/herdr-default",
            "socket_path": socket
        }]
    })
    .to_string();
    write_fake_herdr_with_sessions(dir, &status, &schema, &sessions)
}

#[cfg(unix)]
#[test]
fn event_subscription_release_interrupts_idle_socket_reader() {
    use std::io::Read;
    use std::os::unix::net::UnixListener;

    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("events.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = String::new();
        BufReader::new(stream.try_clone().unwrap())
            .read_line(&mut request)
            .unwrap();
        stream
            .write_all(b"{\"result\":{\"type\":\"subscription_started\"}}\n")
            .unwrap();
        let mut remaining = String::new();
        let _ = BufReader::new(stream).read_to_string(&mut remaining);
        request
    });
    let binary = write_fake_herdr_event_session(dir.path(), &socket);
    let manager = Arc::new(HerdrManager::with_binary(binary));
    let (tx, rx) = mpsc::channel();
    let callback: OnSubscriptionEvent =
        Arc::new(move |event| tx.send(event).map_err(|error| error.to_string()));
    let id = manager
        .events_subscribe(Some("default".into()), vec![], callback)
        .unwrap();
    assert!(matches!(
        rx.recv_timeout(TEST_EVENT_RECV_TIMEOUT).unwrap(),
        HerdrSubscriptionEvent::Subscribed { .. }
    ));
    let started = Instant::now();
    manager.events_release(&id).unwrap();
    assert!(started.elapsed() < Duration::from_secs(1));
    let request: serde_json::Value = serde_json::from_str(server.join().unwrap().trim()).unwrap();
    assert_eq!(request["method"], "events.subscribe");
    let selectors = request["params"]["subscriptions"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|item| item["type"].as_str())
        .collect::<Vec<_>>();
    assert!(selectors.contains(&"tab.closed"));
    assert!(selectors.contains(&"workspace.moved"));
    assert!(selectors.contains(&"workspace.reordered"));
}

#[cfg(unix)]
#[test]
fn event_subscription_ack_is_bounded() {
    use std::os::unix::net::UnixListener;

    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("events-ack.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut request = String::new();
        BufReader::new(stream.try_clone().unwrap())
            .read_line(&mut request)
            .unwrap();
        std::thread::sleep(Duration::from_millis(1_200));
    });
    let binary = write_fake_herdr_event_session(dir.path(), &socket);
    let manager = Arc::new(HerdrManager::with_binary(binary));
    let callback: OnSubscriptionEvent = Arc::new(|_| Ok(()));
    let started = Instant::now();
    let error = manager
        .events_subscribe(Some("default".into()), vec![], callback)
        .unwrap_err();
    assert!(error.contains("ack read failed"));
    assert!(started.elapsed() < Duration::from_secs(5));
    server.join().unwrap();
}

#[cfg(unix)]
#[test]
fn api_request_rejects_oversized_unterminated_and_invalid_utf8() {
    use std::os::unix::net::UnixListener;

    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("api-hostile.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let server = std::thread::spawn(move || {
        // Over-limit unterminated payload, no newline.
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = String::new();
        BufReader::new(stream.try_clone().unwrap())
            .read_line(&mut request)
            .unwrap();
        let _ = stream.write_all(&vec![b'x'; crate::herdr_limits::MAX_NDJSON_LINE_BYTES + 2]);

        let (mut stream, _) = listener.accept().unwrap();
        let mut request = String::new();
        BufReader::new(stream.try_clone().unwrap())
            .read_line(&mut request)
            .unwrap();
        let _ = stream.write_all(&[0xff, 0xfe, b'\n']);
    });

    let error = api_request(socket.to_str().unwrap(), "ping", serde_json::json!({})).unwrap_err();
    assert!(error.contains("tooLarge"), "{error}");
    assert!(
        error.contains("unterminated") || error.contains("1 MiB"),
        "{error}"
    );

    let error = api_request(socket.to_str().unwrap(), "ping", serde_json::json!({})).unwrap_err();
    assert!(error.contains("invalidUtf8"), "{error}");
    server.join().unwrap();
}

#[cfg(unix)]
#[test]
fn api_request_accepts_line_at_byte_cap_and_rejects_one_byte_over() {
    use crate::herdr_limits::MAX_NDJSON_LINE_BYTES;
    use std::os::unix::net::UnixListener;

    let prefix = b"{\"result\":{\"type\":\"pong\",\"version\":\"0.8.0\",\"protocol\":19,\"pad\":\"";
    let suffix = b"\"}}";
    let pad = MAX_NDJSON_LINE_BYTES - prefix.len() - suffix.len();
    let mut at_cap = Vec::with_capacity(MAX_NDJSON_LINE_BYTES + 1);
    at_cap.extend_from_slice(prefix);
    at_cap.extend(std::iter::repeat_n(b'a', pad));
    at_cap.extend_from_slice(suffix);
    assert_eq!(at_cap.len(), MAX_NDJSON_LINE_BYTES);
    at_cap.push(b'\n');

    let mut over = at_cap.clone();
    over.insert(over.len() - 1, b'b');

    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("api-cap.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = String::new();
        BufReader::new(stream.try_clone().unwrap())
            .read_line(&mut request)
            .unwrap();
        stream.write_all(&at_cap).unwrap();

        let (mut stream, _) = listener.accept().unwrap();
        let mut request = String::new();
        BufReader::new(stream.try_clone().unwrap())
            .read_line(&mut request)
            .unwrap();
        stream.write_all(&over).unwrap();
    });

    let ok = api_request(socket.to_str().unwrap(), "ping", serde_json::json!({})).unwrap();
    assert_eq!(ok["result"]["protocol"], 19);

    let error = api_request(socket.to_str().unwrap(), "ping", serde_json::json!({})).unwrap_err();
    assert!(error.contains("tooLarge"), "{error}");
    server.join().unwrap();
}

#[cfg(unix)]
#[test]
fn api_request_rejects_empty_response_and_excessive_depth() {
    use crate::herdr_limits::MAX_JSON_DEPTH;
    use std::os::unix::net::UnixListener;

    let mut deep = serde_json::json!(1);
    for _ in 0..=MAX_JSON_DEPTH {
        deep = serde_json::json!([deep]);
    }
    let hostile = serde_json::json!({ "result": deep });
    let mut deep_line = serde_json::to_vec(&hostile).unwrap();
    deep_line.push(b'\n');

    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("api-empty.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = String::new();
        BufReader::new(stream.try_clone().unwrap())
            .read_line(&mut request)
            .unwrap();
        stream.write_all(b"\n").unwrap();

        let (mut stream, _) = listener.accept().unwrap();
        let mut request = String::new();
        BufReader::new(stream.try_clone().unwrap())
            .read_line(&mut request)
            .unwrap();
        stream.write_all(&deep_line).unwrap();
    });

    let empty = api_request(socket.to_str().unwrap(), "ping", serde_json::json!({})).unwrap_err();
    assert!(empty.contains("emptyResponse"), "{empty}");

    let deep_err =
        api_request(socket.to_str().unwrap(), "ping", serde_json::json!({})).unwrap_err();
    assert!(deep_err.contains("tooComplex"), "{deep_err}");
    server.join().unwrap();
}

#[cfg(unix)]
#[test]
fn event_subscription_oversized_line_is_terminal() {
    use std::os::unix::net::UnixListener;

    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("events-oversize.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = String::new();
        BufReader::new(stream.try_clone().unwrap())
            .read_line(&mut request)
            .unwrap();
        stream
            .write_all(b"{\"result\":{\"type\":\"subscription_started\"}}\n")
            .unwrap();
        let _ = stream.write_all(&vec![b'x'; crate::herdr_limits::MAX_NDJSON_LINE_BYTES + 2]);
        std::thread::sleep(Duration::from_millis(200));
    });
    let binary = write_fake_herdr_event_session(dir.path(), &socket);
    let manager = Arc::new(HerdrManager::with_binary(binary));
    let (tx, rx) = mpsc::channel();
    let callback: OnSubscriptionEvent =
        Arc::new(move |event| tx.send(event).map_err(|error| error.to_string()));
    let id = manager
        .events_subscribe(Some("default".into()), vec![], callback)
        .unwrap();
    assert!(matches!(
        rx.recv_timeout(TEST_EVENT_RECV_TIMEOUT).unwrap(),
        HerdrSubscriptionEvent::Subscribed { .. }
    ));
    match rx.recv_timeout(TEST_EVENT_RECV_TIMEOUT).unwrap() {
        HerdrSubscriptionEvent::Error { message, .. } => {
            assert!(message.contains("tooLarge"), "{message}");
        }
        other => panic!("expected oversized error, got {other:?}"),
    }
    manager.events_release(&id).unwrap();
    server.join().unwrap();
}

#[cfg(unix)]
#[test]
fn malformed_event_is_terminal_and_releasable() {
    use std::os::unix::net::UnixListener;

    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("events-malformed.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = String::new();
        BufReader::new(stream.try_clone().unwrap())
            .read_line(&mut request)
            .unwrap();
        stream
            .write_all(b"{\"result\":{\"type\":\"subscription_started\"}}\n{not-json}\n")
            .unwrap();
        std::thread::sleep(Duration::from_millis(500));
    });
    let binary = write_fake_herdr_event_session(dir.path(), &socket);
    let manager = Arc::new(HerdrManager::with_binary(binary));
    let (tx, rx) = mpsc::channel();
    let callback: OnSubscriptionEvent =
        Arc::new(move |event| tx.send(event).map_err(|error| error.to_string()));
    let id = manager
        .events_subscribe(Some("default".into()), vec![], callback)
        .unwrap();
    assert!(matches!(
        rx.recv_timeout(TEST_EVENT_RECV_TIMEOUT).unwrap(),
        HerdrSubscriptionEvent::Subscribed { .. }
    ));
    assert!(matches!(
        rx.recv_timeout(TEST_EVENT_RECV_TIMEOUT).unwrap(),
        HerdrSubscriptionEvent::Error { .. }
    ));
    manager.events_release(&id).unwrap();
    server.join().unwrap();
}

#[cfg(unix)]
#[test]
fn event_subscription_delivers_duplicate_events_then_disconnects() {
    use std::os::unix::net::UnixListener;

    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("events-stream.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = String::new();
        BufReader::new(stream.try_clone().unwrap())
            .read_line(&mut request)
            .unwrap();
        let event = b"{\"event\":\"pane.agent_status_changed\",\"data\":{\"pane_id\":\"w1:p1\",\"workspace_id\":\"w1\",\"agent_status\":\"done\"}}\n";
        stream
            .write_all(b"{\"result\":{\"type\":\"subscription_started\"}}\n")
            .unwrap();
        stream.write_all(event).unwrap();
        stream.write_all(event).unwrap();
    });
    let binary = write_fake_herdr_event_session(dir.path(), &socket);
    let manager = Arc::new(HerdrManager::with_binary(binary));
    let (tx, rx) = mpsc::channel();
    let callback: OnSubscriptionEvent =
        Arc::new(move |event| tx.send(event).map_err(|error| error.to_string()));
    let id = manager
        .events_subscribe(Some("default".into()), vec![], callback)
        .unwrap();
    assert!(matches!(
        rx.recv_timeout(TEST_EVENT_RECV_TIMEOUT).unwrap(),
        HerdrSubscriptionEvent::Subscribed { .. }
    ));
    for _ in 0..2 {
        assert!(matches!(
            rx.recv_timeout(TEST_EVENT_RECV_TIMEOUT).unwrap(),
            HerdrSubscriptionEvent::AgentStatusChanged { .. }
        ));
    }
    assert!(matches!(
        rx.recv_timeout(TEST_EVENT_RECV_TIMEOUT).unwrap(),
        HerdrSubscriptionEvent::Disconnected { .. }
    ));
    manager.events_release(&id).unwrap();
    server.join().unwrap();
}

#[cfg(unix)]
#[test]
fn fake_binary_capabilities_use_discovered_protocol() {
    let dir = tempfile::tempdir().unwrap();
    let binary = write_fake_herdr(dir.path());
    let mgr = HerdrManager::with_binary(binary.clone());
    let caps = mgr.capabilities();
    assert_eq!(caps.binary_protocol, Some(19));
    assert_eq!(caps.api.schema_protocol, Some(19));
    assert_eq!(caps.binary_version.as_deref(), Some("0.0.0-fake"));
    assert!(!caps.terminal.observe); // named session stopped in fixture
    assert!(!caps.terminal.control);
    assert!(!caps.api.snapshot); // server not running in fixture
    assert_eq!(caps.events.status, HerdrEventsStatus::Unavailable);
    // status overwrites binary path from fixture client.binary
    assert_eq!(caps.binary_path.as_deref(), Some("FAKE"));
    let _ = binary;
}

#[cfg(unix)]
#[test]
fn mutation_capability_cache_avoids_reprobing_status_and_schema() {
    use std::os::unix::net::UnixListener;

    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("cache-ping.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let server = std::thread::spawn(move || {
        // The discovery probe pings once; the cached call reuses that
        // identity inside the validation window instead of pinging again.
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = String::new();
        BufReader::new(stream.try_clone().unwrap())
            .read_line(&mut request)
            .unwrap();
        stream
                .write_all(b"{\"id\":\"cache\",\"result\":{\"type\":\"pong\",\"version\":\"0.8.0\",\"protocol\":19}}\n")
                .unwrap();
    });
    let count_file = dir.path().join("probe-count.txt");
    let binary = dir.path().join("herdr");
    let script = format!(
        r#"#!/bin/sh
set -e
if [ "$1" = "session" ]; then
  printf '%s\n' '{{"sessions":[{{"name":"default","default":true,"running":true,"session_dir":"/tmp/default","socket_path":"{}"}}]}}'
  exit 0
fi
printf '%s\n' x >> '{}'
if [ "$1" = "status" ]; then
  printf '%s\n' '{{"client":{{"version":"0.8.0","protocol":19,"binary":"FAKE"}},"server":{{"status":"running","running":true,"version":"0.8.0","protocol":19,"compatible":true,"socket":"/tmp/default.sock"}}}}'
  exit 0
fi
printf '%s\n' '{{"protocol":19,"schema_version":1,"methods":["session.snapshot","tab.create","workspace.focus"]}}'
"#,
        socket.display(),
        count_file.display()
    );
    write_executable_fixture(&binary, &script);
    let mgr = HerdrManager::with_binary(binary);

    assert!(
        mgr.capabilities_for_session(Some("default"))
            .api
            .workspace_focus
    );
    let after_first = fs::read_to_string(&count_file).unwrap().lines().count();
    assert!(
        mgr.cached_capabilities_for_session(Some("default"))
            .api
            .workspace_focus
    );
    let after_second = fs::read_to_string(&count_file).unwrap().lines().count();

    assert_eq!(after_first, after_second);
    server.join().unwrap();
}

/// Fake runtime for validation-cache tests: counts `session list` spawns,
/// serves ping/pane.get on a Unix socket and can flip the Session to stopped.
#[cfg(unix)]
struct ValidationFixture {
    _dir: tempfile::TempDir,
    mgr: HerdrManager,
    list_count: PathBuf,
    stopped_flag: PathBuf,
    pings: Arc<std::sync::atomic::AtomicUsize>,
    fail_next: Arc<AtomicBool>,
}

#[cfg(unix)]
fn validation_fixture() -> ValidationFixture {
    use std::os::unix::net::UnixListener;
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("validation.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let pings = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let fail_next = Arc::new(AtomicBool::new(false));
    let (server_pings, server_fail) = (pings.clone(), fail_next.clone());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let mut request = String::new();
            if BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut request)
                .is_err()
            {
                continue;
            }
            let reply = if request.contains("\"ping\"") {
                server_pings.fetch_add(1, Ordering::SeqCst);
                "{\"id\":\"v\",\"result\":{\"type\":\"pong\",\"version\":\"0.9.1\",\"protocol\":22}}\n".to_string()
            } else if server_fail.swap(false, Ordering::SeqCst) {
                "{\"id\":\"v\",\"error\":{\"code\":\"pane_not_found\",\"message\":\"gone\"}}\n"
                    .to_string()
            } else {
                "{\"id\":\"v\",\"result\":{\"type\":\"pane_info\",\"pane\":{\"pane_id\":\"w1:p1\",\"scroll\":{\"offset_from_bottom\":0,\"max_offset_from_bottom\":10,\"viewport_rows\":5}}}}\n".to_string()
            };
            let _ = stream.write_all(reply.as_bytes());
        }
    });
    let list_count = dir.path().join("list-count.txt");
    let stopped_flag = dir.path().join("stopped");
    let binary = dir.path().join("herdr");
    let script = format!(
        r#"#!/bin/sh
set -e
if [ "$1" = "session" ]; then
  printf '%s\n' x >> '{count}'
  if [ -f '{dir}/pause-list' ]; then
    touch '{dir}/list-entered'
    while [ -f '{dir}/pause-list' ]; do sleep 0.01; done
  fi
  if [ -f '{dir}/fail-list' ]; then exit 1; fi
  running=true
  if [ -f '{stopped}' ]; then running=false; fi
  printf '%s\n' "{{\"sessions\":[{{\"name\":\"default\",\"default\":true,\"running\":$running,\"session_dir\":\"/tmp/default\",\"socket_path\":\"{socket}\"}}]}}"
  exit 0
fi
if [ "$1" = "status" ]; then
  printf '%s\n' '{{"client":{{"version":"0.9.1","protocol":22,"binary":"FAKE"}},"server":{{"status":"running","running":true,"version":"0.9.1","protocol":22,"compatible":true,"socket":"{socket}"}}}}'
  exit 0
fi
printf '%s\n' '{{"protocol":22,"schema_version":1,"methods":["session.snapshot","tab.create","workspace.focus","pane.get","pane.scroll","ping"]}}'
"#,
        count = list_count.display(),
        dir = dir.path().display(),
        stopped = stopped_flag.display(),
        socket = socket.display(),
    );
    write_executable_fixture(&binary, &script);
    let mgr = HerdrManager::with_binary(binary);
    assert!(mgr.capabilities_for_session(Some("default")).api.snapshot);
    fs::write(&list_count, "").unwrap();
    pings.store(0, Ordering::SeqCst);
    ValidationFixture {
        _dir: dir,
        mgr,
        list_count,
        stopped_flag,
        pings,
        fail_next,
    }
}

#[cfg(unix)]
fn spawn_count(path: &Path) -> usize {
    fs::read_to_string(path).unwrap().lines().count()
}

#[cfg(unix)]
#[test]
fn hot_pane_requests_reuse_session_list_and_ping_within_the_validation_window() {
    let fixture = validation_fixture();
    for _ in 0..5 {
        let scroll = fixture
            .mgr
            .pane_scroll_state(Some("default"), "w1:p1".into())
            .unwrap();
        assert_eq!(scroll.unwrap().max_offset_from_bottom, 10);
    }
    // Warm-up already listed and pinged; five requests add no process spawn
    // and no extra ping (previously: two spawns and one ping per request).
    assert_eq!(spawn_count(&fixture.list_count), 0);
    assert_eq!(fixture.pings.load(Ordering::SeqCst), 0);
    // An explicit refresh always reads the authoritative list.
    fixture.mgr.list_sessions().unwrap();
    assert_eq!(spawn_count(&fixture.list_count), 1);
}

#[cfg(unix)]
fn wait_for_paused_session_list(fixture: &ValidationFixture) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while !fixture._dir.path().join("list-entered").exists() {
        assert!(
            Instant::now() < deadline,
            "session list did not reach pause"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(unix)]
#[test]
fn warm_pane_request_completes_while_explicit_session_list_is_paused() {
    let fixture = validation_fixture();
    let pause = fixture._dir.path().join("pause-list");
    fs::write(&pause, "").unwrap();
    std::thread::scope(|scope| {
        let refresh = scope.spawn(|| fixture.mgr.list_sessions());
        wait_for_paused_session_list(&fixture);
        let (tx, rx) = mpsc::channel();
        let manager = &fixture.mgr;
        let hot_request = scope.spawn(move || {
            tx.send(manager.pane_scroll_state(Some("default"), "w1:p1".into()))
                .unwrap();
        });
        let hot_result = rx.recv_timeout(Duration::from_secs(1));
        // Release the CLI before asserting, including on the failing old code.
        fs::remove_file(&pause).unwrap();
        refresh.join().unwrap().unwrap();
        hot_request.join().unwrap();
        assert_eq!(
            hot_result
                .expect("warm pane request blocked behind explicit session list")
                .unwrap()
                .unwrap()
                .max_offset_from_bottom,
            10
        );
    });
    assert_eq!(spawn_count(&fixture.list_count), 1);
}

#[cfg(unix)]
#[test]
fn invalidation_during_session_list_prevents_stale_cache_publication() {
    let fixture = validation_fixture();
    let pause = fixture._dir.path().join("pause-list");
    fs::write(&pause, "").unwrap();
    std::thread::scope(|scope| {
        let refresh = scope.spawn(|| fixture.mgr.list_sessions());
        wait_for_paused_session_list(&fixture);
        let (tx, rx) = mpsc::channel();
        let manager = &fixture.mgr;
        let invalidation = scope.spawn(move || {
            manager.invalidate_runtime_caches();
            tx.send(()).unwrap();
        });
        let invalidated = rx.recv_timeout(Duration::from_secs(1));
        fs::remove_file(&pause).unwrap();
        refresh.join().unwrap().unwrap();
        invalidation.join().unwrap();
        invalidated.expect("invalidation must not wait for CLI I/O");
    });
    assert!(fixture.mgr.session_inventory.lock().unwrap().is_none());
    fixture
        .mgr
        .pane_scroll_state(Some("default"), "w1:p1".into())
        .unwrap();
    assert_eq!(spawn_count(&fixture.list_count), 2);
}

#[cfg(unix)]
#[test]
fn failed_explicit_session_list_invalidates_warm_inventory() {
    let fixture = validation_fixture();
    let fail = fixture._dir.path().join("fail-list");
    fs::write(&fail, "").unwrap();
    assert!(fixture.mgr.list_sessions().is_err());
    assert!(fixture.mgr.session_inventory.lock().unwrap().is_none());
    fs::remove_file(fail).unwrap();
    fixture
        .mgr
        .pane_scroll_state(Some("default"), "w1:p1".into())
        .unwrap();
    assert_eq!(spawn_count(&fixture.list_count), 2);
}

#[cfg(unix)]
#[test]
fn hot_requests_reduce_cli_spawns_over_nine_seconds_without_slowing_ping() {
    assert_eq!(RUNTIME_VALIDATION_TTL, Duration::from_secs(10));
    for (ttl, expected_spawns) in [(Duration::from_secs(1), 9), (RUNTIME_VALIDATION_TTL, 0)] {
        let fixture = validation_fixture();
        fixture.mgr.set_validation_ttl_for_test(ttl);
        for second in 1..=9 {
            // Advance cache ages deterministically rather than sleeping 18 seconds.
            fixture
                .mgr
                .session_inventory
                .lock()
                .unwrap()
                .as_mut()
                .unwrap()
                .0 =
                Instant::now() - Duration::from_secs(if expected_spawns == 0 { second } else { 1 });
            for (at, _) in fixture.mgr.socket_identity.lock().unwrap().values_mut() {
                *at = Instant::now() - SOCKET_IDENTITY_TTL;
            }
            for _ in 0..2 {
                fixture
                    .mgr
                    .pane_scroll_state(Some("default"), "w1:p1".into())
                    .unwrap();
            }
        }
        assert_eq!(spawn_count(&fixture.list_count), expected_spawns);
        assert_eq!(fixture.pings.load(Ordering::SeqCst), 9);
        fixture
            .mgr
            .session_inventory
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .0 = Instant::now() - ttl;
        fixture
            .mgr
            .pane_scroll_state(Some("default"), "w1:p1".into())
            .unwrap();
        assert_eq!(spawn_count(&fixture.list_count), expected_spawns + 1);
    }
}

#[test]
fn binary_fingerprint_uses_the_long_validation_window() {
    let manager = HerdrManager::new();
    *manager.binary_fingerprint_cache.lock().unwrap() = Some((
        Instant::now() - Duration::from_secs(2),
        Some("cached fingerprint".into()),
    ));
    assert_eq!(
        manager.recent_binary_fingerprint().as_deref(),
        Some("cached fingerprint")
    );
    manager
        .binary_fingerprint_cache
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .0 = Instant::now() - RUNTIME_VALIDATION_TTL;
    assert_eq!(manager.recent_binary_fingerprint(), None);
}

#[cfg(unix)]
#[test]
fn simultaneous_hot_requests_share_one_expired_inventory_refresh() {
    let fixture = validation_fixture();
    fixture
        .mgr
        .session_inventory
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .0 = Instant::now() - RUNTIME_VALIDATION_TTL;
    let manager = Arc::new(fixture.mgr);
    let barrier = Arc::new(std::sync::Barrier::new(6));
    let workers: Vec<_> = (0..6)
        .map(|_| {
            let manager = manager.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                manager
                    .pane_scroll_state(Some("default"), "w1:p1".into())
                    .unwrap();
            })
        })
        .collect();
    for worker in workers {
        worker.join().unwrap();
    }
    assert_eq!(spawn_count(&fixture.list_count), 1);
}

#[cfg(unix)]
#[test]
fn missing_or_stopped_cached_sessions_refresh_exactly_once() {
    for stopped in [false, true] {
        let fixture = validation_fixture();
        {
            let mut inventory = fixture.mgr.session_inventory.lock().unwrap();
            let sessions = &mut inventory.as_mut().unwrap().1;
            if stopped {
                sessions[0].running = false;
            } else {
                sessions.clear();
            }
        }
        // The authoritative list is running: recover from stale negative cache.
        fixture
            .mgr
            .pane_scroll_state(Some("default"), "w1:p1".into())
            .unwrap();
        assert_eq!(spawn_count(&fixture.list_count), 1);
        fs::write(&fixture.list_count, "").unwrap();
        if stopped {
            fs::write(&fixture.stopped_flag, "").unwrap();
            fixture
                .mgr
                .session_inventory
                .lock()
                .unwrap()
                .as_mut()
                .unwrap()
                .1[0]
                .running = false;
        }
        let target = if stopped { "default" } else { "missing" };
        assert!(fixture
            .mgr
            .pane_scroll_state(Some(target), "w1:p1".into())
            .is_err());
        assert_eq!(spawn_count(&fixture.list_count), 1);
        assert!(fixture.mgr.session_inventory.lock().unwrap().is_none());
    }
}

#[cfg(unix)]
#[test]
fn explicit_list_refreshes_cached_running_state_and_socket_path() {
    let fixture = validation_fixture();
    fixture
        .mgr
        .session_inventory
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .1[0]
        .socket_path = "stale".into();
    let fresh = fixture.mgr.list_sessions().unwrap();
    assert_eq!(spawn_count(&fixture.list_count), 1);
    assert_ne!(fresh[0].socket_path, "stale");
    assert_eq!(
        fixture
            .mgr
            .require_running_session_socket(Some("default"))
            .unwrap()
            .1,
        fresh[0].socket_path
    );
    assert_eq!(spawn_count(&fixture.list_count), 1);
}

#[cfg(unix)]
#[test]
fn a_failed_request_drops_validation_state_and_the_next_request_rediscovers() {
    let fixture = validation_fixture();
    fixture.fail_next.store(true, Ordering::SeqCst);
    assert!(fixture
        .mgr
        .pane_scroll_state(Some("default"), "w1:p1".into())
        .is_err());
    assert!(fixture
        .mgr
        .pane_scroll_state(Some("default"), "w1:p1".into())
        .is_ok());
    assert_eq!(spawn_count(&fixture.list_count), 1);
    assert_eq!(fixture.pings.load(Ordering::SeqCst), 1);
}

#[cfg(unix)]
#[test]
fn a_stopped_session_is_refused_once_the_validation_window_expires() {
    let fixture = validation_fixture();
    fixture
        .mgr
        .set_validation_ttl_for_test(Duration::from_millis(40));
    assert!(fixture
        .mgr
        .pane_scroll_state(Some("default"), "w1:p1".into())
        .is_ok());
    fs::write(&fixture.stopped_flag, "").unwrap();
    std::thread::sleep(Duration::from_millis(60));
    let error = fixture
        .mgr
        .pane_scroll_state(Some("default"), "w1:p1".into())
        .unwrap_err();
    assert!(
        error.contains("not running") || error.contains("unavailable"),
        "unexpected error: {error}"
    );
}

#[cfg(unix)]
#[test]
fn capabilities_do_not_claim_api_when_server_incompatible() {
    let dir = tempfile::tempdir().unwrap();
    let binary = write_fake_herdr_with_sessions(
        dir.path(),
        r#"{"client":{"version":"0.0.0-fake","channel":"test","protocol":19,"binary":"FAKE"},"server":{"status":"running","running":true,"version":"0.0.0-fake","protocol":18,"compatible":false,"socket":"/tmp/herdr-fake.sock"},"update":{"restart_needed":false}}"#,
        r#"{"protocol":19,"schema_version":1,"methods":["session.snapshot","tab.create","session.ping"],"schemas":{"session.snapshot":{},"tab.create":{}}}"#,
        r#"{"sessions":[{"name":"default","default":true,"running":true,"session_dir":"/tmp/herdr-default","socket_path":"/tmp/herdr-fake.sock"}]}"#,
    );
    let mgr = HerdrManager::with_binary(binary);
    let caps = mgr.capabilities();
    assert_eq!(caps.server.compatible, Some(false));
    assert!(!caps.api.snapshot);
    assert!(!caps.api.tab_create);
    assert!(!caps.api.ping);
    assert!(!caps.terminal.control);
    assert!(!caps.terminal.create);
    assert!(!caps.terminal.observe);
    assert!(caps
        .api
        .reason
        .as_deref()
        .unwrap_or("")
        .contains("incompatible"));
}

#[cfg(unix)]
#[test]
fn capabilities_do_not_claim_methods_missing_from_schema() {
    let dir = tempfile::tempdir().unwrap();
    let binary = write_fake_herdr_with_sessions(
        dir.path(),
        r#"{"client":{"version":"0.0.0-fake","channel":"test","protocol":19,"binary":"FAKE"},"server":{"status":"running","running":true,"version":"0.0.0-fake","protocol":19,"compatible":true,"socket":"/tmp/herdr-fake.sock"},"update":{"restart_needed":false}}"#,
        // Running + compatible, but schema advertises neither required method.
        r#"{"protocol":19,"schema_version":1,"methods":["session.ping"],"schemas":{}}"#,
        r#"{"sessions":[{"name":"default","default":true,"running":true,"session_dir":"/tmp/herdr-default","socket_path":"/tmp/herdr-fake.sock"}]}"#,
    );
    let mgr = HerdrManager::with_binary(binary);
    let caps = mgr.discover_capabilities_for_session(None);
    assert!(caps.server.running);
    assert_eq!(caps.server.compatible, Some(true));
    assert!(!caps.api.snapshot);
    assert!(!caps.api.tab_create);
    assert!(!caps.terminal.create);
    assert!(caps.terminal.control); // running + compatible
    let reason = caps.api.reason.as_deref().unwrap_or("");
    assert!(
        reason.contains("session.snapshot") || reason.contains("tab.create"),
        "unexpected reason: {reason}"
    );
}

#[cfg(unix)]
#[test]
fn capabilities_claim_methods_present_in_schema_when_compatible() {
    let dir = tempfile::tempdir().unwrap();
    let binary = write_fake_herdr_with_sessions(
        dir.path(),
        r#"{"client":{"version":"0.0.0-fake","channel":"test","protocol":19,"binary":"FAKE"},"server":{"status":"running","running":true,"version":"0.0.0-fake","protocol":19,"compatible":true,"socket":"/tmp/herdr-fake.sock"},"update":{"restart_needed":false}}"#,
        r#"{"protocol":19,"schema_version":1,"methods":["session.snapshot","tab.create","tab.move","workspace.focus","workspace.create","workspace.move","session.ping","events.subscribe"],"schemas":{"session.snapshot":{},"tab.create":{},"tab.move":{},"workspace.focus":{},"workspace.create":{},"workspace.move":{},"events.subscribe":{}}}"#,
        r#"{"sessions":[{"name":"default","default":true,"running":true,"session_dir":"/tmp/herdr-default","socket_path":"/tmp/herdr-fake.sock"}]}"#,
    );
    let mgr = HerdrManager::with_binary(binary);
    let caps = mgr.discover_capabilities_for_session(None);
    assert!(caps.api.snapshot);
    assert!(caps.api.tab_create);
    assert!(caps.api.tab_move);
    assert!(caps.api.methods.iter().any(|method| method == "tab.move"));
    assert!(caps.api.workspace_focus);
    assert!(caps.api.workspace_create);
    assert!(caps.api.workspace_move);
    assert!(caps.api.ping);
    assert!(caps.terminal.create);
    assert!(caps.terminal.control);
    assert!(caps.api.events_subscribe);
    assert_eq!(caps.events.status, HerdrEventsStatus::Available);
    assert!(caps.events.reason.is_none());
    assert!(caps.api.reason.is_none());
}

#[cfg(unix)]
#[test]
fn capabilities_downgrade_when_live_socket_probe_fails() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("missing-herdr.sock");
    let status = format!(
        r#"{{"client":{{"version":"0.8.0","channel":"test","protocol":19,"binary":"FAKE"}},"server":{{"status":"running","running":true,"version":"0.8.0","protocol":19,"compatible":true,"socket":"{}"}},"update":{{"restart_needed":false}}}}"#,
        socket.display()
    );
    let sessions = format!(
        r#"{{"sessions":[{{"name":"default","default":true,"running":true,"session_dir":"/tmp/herdr-default","socket_path":"{}"}}]}}"#,
        socket.display()
    );
    let binary = write_fake_herdr_with_sessions(
        dir.path(),
        &status,
        r#"{"protocol":19,"schema_version":1,"methods":["session.snapshot","tab.create","session.ping","events.subscribe"],"schemas":{"session.snapshot":{},"tab.create":{},"events.subscribe":{}}}"#,
        &sessions,
    );
    let mgr = HerdrManager::with_binary(binary);
    let caps = mgr.capabilities();

    assert!(caps.server.running);
    assert!(!caps.api.snapshot);
    assert!(!caps.api.tab_create);
    assert!(!caps.api.events_subscribe);
    assert!(!caps.terminal.observe);
    assert!(!caps.terminal.control);
    assert!(!caps.terminal.create);
    assert_eq!(caps.events.status, HerdrEventsStatus::Unavailable);
    assert!(caps
        .api
        .reason
        .as_deref()
        .unwrap_or("")
        .contains("local socket probe failed"));
}

#[test]
fn connector_reader_rejects_oversized_line_and_does_not_emit_frame() {
    use crate::herdr_limits::MAX_NDJSON_LINE_BYTES;
    let session = Arc::new(ConnectorSession {
        id: "herdr-term-oversize".into(),
        mode: HerdrTerminalMode::Observe,
        cols: Mutex::new(40),
        rows: Mutex::new(10),
        child: Mutex::new(None),
        process_tree: Mutex::new(None),
        stdin: Mutex::new(None),
        reader: Mutex::new(None),
        closed: Mutex::new(false),
    });
    let (tx, rx) = mpsc::channel();
    let on_event: OnTerminalEvent =
        Arc::new(move |event| tx.send(event).map_err(|error| error.to_string()));
    let mut stdout = Vec::new();
    stdout.extend(std::iter::repeat_n(b'x', MAX_NDJSON_LINE_BYTES + 1));
    connector_reader_loop(
        session,
        std::io::Cursor::new(stdout),
        None::<std::io::Cursor<Vec<u8>>>,
        on_event,
    );
    let event = rx.recv_timeout(Duration::from_secs(1)).unwrap();
    match event {
        HerdrTerminalEvent::Error { code, message, .. } => {
            assert_eq!(code, "tooLarge");
            assert!(message.contains("tooLarge"), "{message}");
        }
        other => panic!("expected tooLarge error, got {other:?}"),
    }
    assert!(rx
        .try_iter()
        .all(|event| !matches!(event, HerdrTerminalEvent::Frame { .. })));
}

#[cfg(unix)]
#[test]
fn fake_connector_observe_emits_first_full_frame_and_release_kills_only_child() {
    let dir = tempfile::tempdir().unwrap();
    let binary = write_fake_herdr_running_session(dir.path());
    let mgr = Arc::new(HerdrManager::with_binary(binary));
    let (tx, rx) = mpsc::channel();
    let on_event: OnTerminalEvent = Arc::new(move |event| {
        let _ = tx.send(event);
        Ok(())
    });

    let opened = mgr
        .open_terminal(
            "term_test".into(),
            HerdrTerminalMode::Observe,
            false,
            40,
            10,
            None,
            on_event,
        )
        .unwrap();
    assert_eq!(opened.role, HerdrTerminalRole::Observer);
    assert_eq!(opened.mode, HerdrTerminalMode::Observe);

    let event = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("expected first frame");
    match event {
        HerdrTerminalEvent::Frame {
            session_id,
            seq,
            full,
            ..
        } => {
            assert_eq!(session_id, opened.session_id);
            assert_eq!(seq, 1);
            assert!(full);
        }
        other => panic!("unexpected event: {other:?}"),
    }

    let closed = rx
        .recv_timeout(Duration::from_secs(3))
        .expect("expected clean EOF to emit closed");
    assert!(matches!(
        closed,
        HerdrTerminalEvent::Closed {
            reason: Some(ref reason),
            ..
        } if reason == "connector_eof"
    ));

    mgr.terminal_release(&opened.session_id).unwrap();
    assert!(mgr.sessions.lock().unwrap().is_empty());
}

#[cfg(unix)]
#[test]
fn fake_connector_control_resize_and_release() {
    let dir = tempfile::tempdir().unwrap();
    let binary = write_fake_herdr_running_session(dir.path());
    let mgr = Arc::new(HerdrManager::with_binary(binary));
    let (tx, rx) = mpsc::channel::<HerdrTerminalEvent>();
    let on_event: OnTerminalEvent = Arc::new(move |event| {
        let _ = tx.send(event);
        Ok(())
    });

    let opened = mgr
        .open_terminal(
            "term_test".into(),
            HerdrTerminalMode::Control,
            false,
            40,
            10,
            None,
            on_event,
        )
        .unwrap();
    assert_eq!(opened.role, HerdrTerminalRole::Controller);
    {
        let sessions = mgr.sessions.lock().unwrap();
        let session = sessions.get(&opened.session_id).unwrap();
        assert!(session.child.lock().unwrap().is_some());
        assert!(session.process_tree.lock().unwrap().is_some());
    }

    let first = rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(matches!(
        first,
        HerdrTerminalEvent::Frame {
            seq: 1,
            full: true,
            ..
        }
    ));

    mgr.terminal_resize(&opened.session_id, 40, 12).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut saw_seq2 = false;
    while Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(HerdrTerminalEvent::Frame {
                seq: 2,
                full: false,
                ..
            }) => {
                saw_seq2 = true;
                break;
            }
            Ok(_) => continue,
            Err(_) => continue,
        }
    }
    assert!(saw_seq2, "expected seq=2 frame after resize");

    mgr.terminal_release(&opened.session_id).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut saw_closed = false;
    while Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(HerdrTerminalEvent::Closed { .. }) => {
                saw_closed = true;
                break;
            }
            Ok(_) => continue,
            Err(_) => break,
        }
    }
    assert!(saw_closed || mgr.sessions.lock().unwrap().is_empty());
}

#[cfg(unix)]
#[test]
fn release_all_connectors_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let binary = write_fake_herdr_running_session(dir.path());
    let mgr = Arc::new(HerdrManager::with_binary(binary));
    let on_event: OnTerminalEvent = Arc::new(|_| Ok(()));
    let _ = mgr
        .open_terminal(
            "term_a".into(),
            HerdrTerminalMode::Observe,
            false,
            20,
            10,
            None,
            on_event.clone(),
        )
        .unwrap();
    let _ = mgr
        .open_terminal(
            "term_b".into(),
            HerdrTerminalMode::Control,
            false,
            20,
            10,
            None,
            on_event,
        )
        .unwrap();
    assert_eq!(mgr.sessions.lock().unwrap().len(), 2);
    mgr.release_all_connectors();
    assert!(mgr.sessions.lock().unwrap().is_empty());
    mgr.release_all_connectors();
}
#[test]
fn parse_session_list_json_rejects_excessive_sessions() {
    let mut sessions = Vec::with_capacity(MAX_SESSION_COUNT + 1);
    for index in 0..=MAX_SESSION_COUNT {
        sessions.push(serde_json::json!({
            "name": format!("s{index}"),
            "default": false,
            "running": false,
            "session_dir": "/tmp",
            "socket_path": "/tmp/s.sock"
        }));
    }
    let error = parse_session_list_json(&serde_json::json!({ "sessions": sessions })).unwrap_err();
    assert!(error.contains("tooComplex"), "{error}");
}

#[test]
fn parse_session_list_json_reads_exact_dto_fields() {
    let value = serde_json::json!({
        "sessions": [
            {
                "name": "default",
                "default": true,
                "running": true,
                "session_dir": "/tmp/herdr-default",
                "socket_path": "/tmp/herdr-default.sock"
            },
            {
                "name": "work",
                "default": false,
                "running": false,
                "session_dir": "/tmp/herdr-work",
                "socket_path": "/tmp/herdr-work.sock"
            }
        ]
    });
    let sessions = parse_session_list_json(&value).unwrap();
    assert_eq!(sessions.len(), 2);
    assert_eq!(sessions[0].name, "default");
    assert!(sessions[0].default);
    assert!(sessions[0].running);
    assert_eq!(sessions[0].session_dir, "/tmp/herdr-default");
    assert_eq!(sessions[0].socket_path, "/tmp/herdr-default.sock");
    assert_eq!(sessions[1].name, "work");
    assert!(!sessions[1].running);
}

#[cfg(unix)]
#[test]
fn list_sessions_uses_session_list_json_only() {
    let dir = tempfile::tempdir().unwrap();
    let binary = write_fake_herdr_with_sessions(
        dir.path(),
        r#"{"client":{"version":"0.0.0-fake","channel":"test","protocol":19,"binary":"FAKE"},"server":{"status":"running","running":true,"version":"0.0.0-fake","protocol":19,"compatible":true,"socket":"/tmp/ignored.sock"},"update":{"restart_needed":false}}"#,
        r#"{"protocol":19,"schema_version":1,"methods":["session.snapshot"],"schemas":{"session.snapshot":{}}}"#,
        r#"{"sessions":[{"name":"work","default":false,"running":true,"session_dir":"/tmp/work","socket_path":"/tmp/work.sock"},{"name":"default","default":true,"running":false,"session_dir":"/tmp/default","socket_path":"/tmp/default.sock"}]}"#,
    );
    let mgr = HerdrManager::with_binary(binary);
    let sessions = mgr.list_sessions().unwrap();
    assert_eq!(sessions.len(), 2);
    assert_eq!(sessions[0].socket_path, "/tmp/work.sock");
    let default = mgr.resolve_named_session(None).unwrap();
    assert_eq!(default.name, "default");
    assert!(!default.running);
    let work = mgr.resolve_named_session(Some("work")).unwrap();
    assert_eq!(work.socket_path, "/tmp/work.sock");
}

#[cfg(unix)]
#[test]
fn snapshot_and_mutations_reject_stopped_session_without_starting_server() {
    let dir = tempfile::tempdir().unwrap();
    let binary = write_fake_herdr_with_sessions(
        dir.path(),
        r#"{"client":{"version":"0.0.0-fake","channel":"test","protocol":19,"binary":"FAKE"},"server":{"status":"not_running","running":false,"version":null,"protocol":null,"compatible":null,"socket":null},"update":{"restart_needed":false}}"#,
        r#"{"protocol":19,"schema_version":1,"methods":["session.snapshot","tab.create","workspace.focus","workspace.create"],"schemas":{"session.snapshot":{},"tab.create":{},"workspace.focus":{},"workspace.create":{}}}"#,
        r#"{"sessions":[{"name":"work","default":true,"running":false,"session_dir":"/tmp/work","socket_path":"/tmp/work.sock"}]}"#,
    );
    let mgr = Arc::new(HerdrManager::with_binary(binary));
    let caps = mgr.capabilities_for_session(Some("work"));
    assert!(!caps.api.snapshot);
    assert!(!caps.api.tab_create);
    assert!(!caps.api.workspace_focus);
    assert!(!caps.api.workspace_create);
    assert!(!caps.terminal.create);
    assert!(!caps.terminal.observe);
    assert!(caps
        .api
        .reason
        .as_deref()
        .unwrap_or("")
        .contains("not running"));

    let err = mgr.snapshot(Some("work")).unwrap_err();
    assert!(err.contains("not running"), "{err}");
    let err = mgr
        .workspace_focus(Some("work"), "ws-1".into())
        .unwrap_err();
    assert!(err.contains("not running"), "{err}");
    let err = mgr
        .workspace_create(Some("work"), Some("/tmp/x".into()), Some("X".into()), true)
        .unwrap_err();
    assert!(err.contains("not running"), "{err}");
    let err = mgr
        .create_terminal(Some("work"), Some("ws-1".into()), None)
        .unwrap_err();
    assert!(err.contains("not running"), "{err}");
    let on_event: OnTerminalEvent = Arc::new(|_| Ok(()));
    let err = mgr
        .open_terminal(
            "term".into(),
            HerdrTerminalMode::Observe,
            false,
            40,
            10,
            Some("work".into()),
            on_event,
        )
        .unwrap_err();
    assert!(err.contains("not running"), "{err}");
    // Fake binary has no session attach / server start subcommands — ensuring we never call them.
}

#[cfg(unix)]
#[test]
fn session_specific_socket_routes_api_and_sets_herdr_session_env() {
    let dir = tempfile::tempdir().unwrap();
    let env_file = dir.path().join("herdr-session-env");
    let sessions = r#"{"sessions":[{"name":"work","default":false,"running":true,"session_dir":"/tmp/work","socket_path":"/tmp/work.sock"},{"name":"default","default":true,"running":true,"session_dir":"/tmp/default","socket_path":"/tmp/default.sock"}]}"#;
    let binary = write_fake_herdr_with_sessions(
        dir.path(),
        r#"{"client":{"version":"0.0.0-fake","channel":"test","protocol":19,"binary":"FAKE"},"server":{"status":"running","running":true,"version":"0.0.0-fake","protocol":19,"compatible":true,"socket":"/tmp/default.sock"},"update":{"restart_needed":false}}"#,
        r#"{"protocol":19,"schema_version":1,"methods":["session.snapshot","tab.create","workspace.focus","workspace.create","session.ping"],"schemas":{"session.snapshot":{},"tab.create":{},"workspace.focus":{},"workspace.create":{}}}"#,
        sessions,
    );
    let mgr = Arc::new(HerdrManager::with_binary(binary));
    let caps = mgr.discover_capabilities_for_session(Some("work"));
    assert_eq!(caps.server.socket_path.as_deref(), Some("/tmp/work.sock"));
    assert!(caps.api.snapshot);
    assert!(caps.api.workspace_focus);
    assert!(caps.api.workspace_create);

    // Socket override only for API path verification without a real unix socket.
    // workspace_focus payload uses require_running_session_socket → override.
    // We validate payload builders via parse helpers + env for connectors.

    let workspace_created = parse_workspace_created_response(serde_json::json!({
        "id": "1",
        "result": {
            "type": "workspace_created",
            "workspace": {
                "workspace_id": "ws-9",
                "number": 1,
                "label": "feature-x",
                "focused": true,
                "pane_count": 1,
                "tab_count": 1,
                "active_tab_id": "tab-9",
                "agent_status": "idle",
                "worktree": {
                    "checkout_path": "/tmp/feature-x",
                    "is_linked_worktree": true,
                    "repo_key": "k",
                    "repo_name": "r",
                    "repo_root": "/tmp/r"
                }
            },
            "tab": { "tab_id": "tab-9", "workspace_id": "ws-9" },
            "root_pane": {
                "pane_id": "pane-9",
                "terminal_id": "term-9",
                "workspace_id": "ws-9"
            }
        }
    }))
    .unwrap();
    assert_eq!(workspace_created.workspace_id, "ws-9");
    assert_eq!(workspace_created.label, "feature-x");
    assert_eq!(workspace_created.path.as_deref(), Some("/tmp/feature-x"));
    assert_eq!(workspace_created.terminal_id.as_deref(), Some("term-9"));

    // Connector must export HERDR_SESSION=<name>.
    std::env::set_var("HERDR_TEST_ENV_FILE", &env_file);
    let on_event: OnTerminalEvent = Arc::new(|_| Ok(()));
    let opened = mgr
        .open_terminal(
            "term_work".into(),
            HerdrTerminalMode::Observe,
            false,
            40,
            10,
            Some("work".into()),
            on_event,
        )
        .unwrap();
    // Redirection creates the file before printf writes the environment.
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut saw = None;
    while Instant::now() < deadline {
        if let Some(value) = fs::read_to_string(&env_file)
            .ok()
            .filter(|value| !value.is_empty())
        {
            saw = Some(value);
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    mgr.terminal_release(&opened.session_id).unwrap();
    std::env::remove_var("HERDR_TEST_ENV_FILE");
    assert_eq!(saw.as_deref().map(str::trim), Some("work"));
}

#[cfg(unix)]
#[test]
fn unknown_named_session_never_inherits_default_capabilities() {
    let dir = tempfile::tempdir().unwrap();
    let binary = write_fake_herdr_with_sessions(
        dir.path(),
        r#"{"client":{"version":"0.0.0-fake","channel":"test","protocol":19,"binary":"FAKE"},"server":{"status":"running","running":true,"version":"0.0.0-fake","protocol":19,"compatible":true,"socket":"/tmp/default.sock"},"update":{"restart_needed":false}}"#,
        r#"{"protocol":19,"schema_version":1,"methods":["session.snapshot","tab.create","workspace.focus","workspace.create"],"schemas":{"session.snapshot":{},"tab.create":{},"workspace.focus":{},"workspace.create":{}}}"#,
        r#"{"sessions":[{"name":"default","default":true,"running":true,"session_dir":"/tmp/default","socket_path":"/tmp/default.sock"}]}"#,
    );
    let mgr = HerdrManager::with_binary(binary);
    let caps = mgr.capabilities_for_session(Some("missing"));
    assert!(!caps.server.running);
    assert!(!caps.api.snapshot);
    assert!(!caps.api.tab_create);
    assert!(!caps.api.workspace_focus);
    assert!(!caps.api.workspace_create);
    assert!(!caps.terminal.observe);
    assert!(caps
        .api
        .reason
        .as_deref()
        .unwrap_or("")
        .contains("session not found"));
}

#[cfg(unix)]
#[test]
fn live_alias_resolves_to_default_named_session() {
    let dir = tempfile::tempdir().unwrap();
    let binary = write_fake_herdr_with_sessions(
        dir.path(),
        r#"{"client":{"version":"0.0.0-fake","channel":"test","protocol":19,"binary":"FAKE"},"server":{"status":"running","running":true,"version":"0.0.0-fake","protocol":19,"compatible":true,"socket":"/tmp/default.sock"},"update":{"restart_needed":false}}"#,
        r#"{"protocol":19,"schema_version":1,"methods":["session.snapshot"],"schemas":{"session.snapshot":{}}}"#,
        r#"{"sessions":[{"name":"default","default":true,"running":true,"session_dir":"/tmp/default","socket_path":"/tmp/default.sock"}]}"#,
    );
    let mgr = HerdrManager::with_binary(binary);
    let live = mgr.resolve_named_session(Some("live")).unwrap();
    assert_eq!(live.name, "default");
    assert_eq!(live.socket_path, "/tmp/default.sock");
}

#[test]
fn protocol19_request_payloads_match_installed_schema() {
    let tab = build_tab_create_params(
        Some("ws_1".into()),
        Some("Shell".into()),
        Some("/tmp/proj".into()),
        true,
    );
    assert_eq!(
        tab,
        serde_json::json!({
            "workspace_id": "ws_1",
            "label": "Shell",
            "cwd": "/tmp/proj",
            "focus": true
        })
    );

    let moved = build_tab_move_params("tab_1".into(), 2);
    assert_eq!(
        moved,
        serde_json::json!({ "tab_id": "tab_1", "insert_index": 2 })
    );

    let workspace_moved = build_workspace_move_params("ws_1".into(), 3);
    assert_eq!(
        workspace_moved,
        serde_json::json!({ "workspace_id": "ws_1", "insert_index": 3 })
    );

    let split = build_pane_split_params(
        HerdrSplitDirection::Right,
        Some("pane_1".into()),
        Some("ws_1".into()),
        None,
        Some(0.4),
        true,
    );
    assert_eq!(split["direction"], "right");
    assert_eq!(split["target_pane_id"], "pane_1");
    assert_eq!(split["ratio"], 0.4);
    assert_eq!(split["focus"], true);

    let export = build_layout_export_params(Some("tab_1".into()), None);
    assert_eq!(export, serde_json::json!({ "tab_id": "tab_1" }));

    // false = first, true = second along the BSP path.
    let ratio =
        build_layout_set_split_ratio_params(Some("tab_1".into()), None, &[false, true], 0.6);
    assert_eq!(
        ratio,
        serde_json::json!({
            "tab_id": "tab_1",
            "path": [false, true],
            "ratio": 0.6
        })
    );
}

#[test]
fn parse_layout_export_reads_recursive_root_and_boolean_path_semantics() {
    let response = serde_json::json!({
        "id": "1",
        "result": {
            "type": "layout_export",
            "layout": {
                "workspace_id": "ws_1",
                "tab_id": "tab_1",
                "zoomed": false,
                "focused_pane_id": "pane_2",
                "root": {
                    "type": "split",
                    "direction": "right",
                    "ratio": 0.6,
                    "first": {
                        "type": "pane",
                        "pane_id": "pane_1",
                        "label": "A",
                        "cwd": "/tmp/a"
                    },
                    "second": {
                        "type": "split",
                        "direction": "down",
                        "ratio": 0.5,
                        "first": {
                            "type": "pane",
                            "pane_id": "pane_2"
                        },
                        "second": {
                            "type": "pane",
                            "pane_id": "pane_3"
                        }
                    }
                }
            }
        }
    });
    let layout = parse_layout_export_response(response).unwrap();
    assert_eq!(layout.workspace_id, "ws_1");
    assert_eq!(layout.tab_id, "tab_1");
    assert!(!layout.zoomed);
    assert_eq!(layout.focused_pane_id, "pane_2");
    let ipc_layout = serde_json::to_value(&layout).unwrap();
    assert_eq!(
        ipc_layout
            .pointer("/root/first/paneId")
            .and_then(|v| v.as_str()),
        Some("pane_1")
    );
    assert!(ipc_layout.pointer("/root/first/pane_id").is_none());
    match layout.root {
        HerdrLayoutNode::Split {
            direction,
            ratio,
            first,
            second,
        } => {
            assert_eq!(direction, HerdrSplitDirection::Right);
            assert!((ratio - 0.6).abs() < f64::EPSILON);
            match *first {
                HerdrLayoutNode::Pane {
                    pane_id,
                    label,
                    cwd,
                } => {
                    assert_eq!(pane_id.as_deref(), Some("pane_1"));
                    assert_eq!(label.as_deref(), Some("A"));
                    assert_eq!(cwd.as_deref(), Some("/tmp/a"));
                }
                other => panic!("expected first pane, got {other:?}"),
            }
            // path [true, false] => second then first => pane_2
            match *second {
                HerdrLayoutNode::Split {
                    direction,
                    first,
                    second,
                    ..
                } => {
                    assert_eq!(direction, HerdrSplitDirection::Down);
                    match (*first, *second) {
                        (
                            HerdrLayoutNode::Pane {
                                pane_id: Some(a), ..
                            },
                            HerdrLayoutNode::Pane {
                                pane_id: Some(b), ..
                            },
                        ) => {
                            assert_eq!(a, "pane_2");
                            assert_eq!(b, "pane_3");
                        }
                        other => panic!("expected nested panes, got {other:?}"),
                    }
                }
                other => panic!("expected nested split, got {other:?}"),
            }
        }
        other => panic!("expected root split, got {other:?}"),
    }

    let ratio_set = parse_layout_set_split_ratio_response(serde_json::json!({
        "id": "2",
        "result": {
            "type": "layout_split_ratio_set",
            "layout": {
                "workspace_id": "ws_1",
                "tab_id": "tab_1",
                "zoomed": false,
                "focused_pane_id": "pane_1",
                "root": {
                    "type": "pane",
                    "pane_id": "pane_1"
                }
            }
        }
    }))
    .unwrap();
    assert_eq!(ratio_set.focused_pane_id, "pane_1");
}

#[test]
fn parse_layout_export_rejects_excessive_recursion() {
    let mut node = serde_json::json!({
        "type": "pane",
        "pane_id": "pane_leaf"
    });
    for _ in 0..=MAX_LAYOUT_DEPTH {
        node = serde_json::json!({
            "type": "split",
            "direction": "right",
            "ratio": 0.5,
            "first": node,
            "second": { "type": "pane", "pane_id": "pane_r" }
        });
    }
    let error = parse_layout_export_response(serde_json::json!({
        "result": {
            "type": "layout_export",
            "layout": {
                "workspace_id": "ws_1",
                "tab_id": "tab_1",
                "zoomed": false,
                "focused_pane_id": "pane_leaf",
                "root": node
            }
        }
    }))
    .unwrap_err();
    assert!(error.contains("tooComplex"), "{error}");
}

#[test]
fn parse_pane_info_response_reads_split_identity() {
    let parsed = parse_pane_info_response(serde_json::json!({
        "id": "1",
        "result": {
            "type": "pane_info",
            "pane": {
                "pane_id": "pane_new",
                "terminal_id": "term_new",
                "workspace_id": "ws_1",
                "tab_id": "tab_1",
                "focused": true,
                "agent_status": "idle",
                "revision": 2,
                "title": "Split"
            }
        }
    }))
    .unwrap();
    assert_eq!(parsed.pane_id, "pane_new");
    assert_eq!(parsed.terminal_id, "term_new");
    assert_eq!(parsed.tab_id, "tab_1");
    assert_eq!(parsed.workspace_id, "ws_1");
    assert_eq!(parsed.title.as_deref(), Some("Split"));
}

#[cfg(unix)]
#[test]
fn native_interaction_methods_gate_on_schema_and_stopped_session() {
    let dir = tempfile::tempdir().unwrap();
    // Running + compatible, but schema omits interaction methods.
    let binary = write_fake_herdr_with_sessions(
        dir.path(),
        r#"{"client":{"version":"0.0.0-fake","channel":"test","protocol":19,"binary":"FAKE"},"server":{"status":"running","running":true,"version":"0.0.0-fake","protocol":19,"compatible":true,"socket":"/tmp/herdr-fake.sock"},"update":{"restart_needed":false}}"#,
        r#"{"protocol":19,"schema_version":1,"methods":["session.snapshot","tab.create","session.ping"],"schemas":{"session.snapshot":{},"tab.create":{}}}"#,
        r#"{"sessions":[{"name":"default","default":true,"running":true,"session_dir":"/tmp/herdr-default","socket_path":"/tmp/herdr-fake.sock"}]}"#,
    );
    let mgr = HerdrManager::with_binary(binary);
    let caps = mgr.discover_capabilities_for_session(None);
    assert!(caps.api.snapshot);
    assert!(caps.api.tab_create);
    assert!(!caps.api.workspace_rename);
    assert!(!caps.api.workspace_close);
    assert!(!caps.api.tab_focus);
    assert!(!caps.api.tab_rename);
    assert!(!caps.api.tab_close);
    assert!(!caps.api.pane_focus);
    assert!(!caps.api.pane_rename);
    assert!(!caps.api.pane_split);
    assert!(!caps.api.pane_zoom);
    assert!(!caps.api.pane_swap);
    assert!(!caps.api.pane_close);
    assert!(!caps.api.layout_export);
    assert!(!caps.api.layout_set_split_ratio);
    assert!(!caps.api.methods.iter().any(|m| m == "pane.split"));

    let err = mgr
        .layout_export(None, Some("tab_1".into()), None)
        .unwrap_err();
    assert!(
        err.contains("unavailable")
            || err.contains("lacks")
            || err.contains("not running")
            || err.contains("socket probe failed"),
        "{err}"
    );

    let dir2 = tempfile::tempdir().unwrap();
    let stopped = write_fake_herdr_with_sessions(
        dir2.path(),
        r#"{"client":{"version":"0.0.0-fake","channel":"test","protocol":19,"binary":"FAKE"},"server":{"status":"not_running","running":false,"version":null,"protocol":null,"compatible":null,"socket":null},"update":{"restart_needed":false}}"#,
        r#"{"protocol":19,"schema_version":1,"methods":["session.snapshot","tab.create","workspace.rename","workspace.close","tab.focus","tab.rename","tab.close","pane.focus","pane.rename","pane.split","pane.zoom","pane.swap","pane.close","layout.export","layout.set_split_ratio"],"schemas":{}}"#,
        r#"{"sessions":[{"name":"work","default":true,"running":false,"session_dir":"/tmp/work","socket_path":"/tmp/work.sock"}]}"#,
    );
    let mgr2 = HerdrManager::with_binary(stopped);
    let caps2 = mgr2.capabilities_for_session(Some("work"));
    assert!(!caps2.api.layout_export);
    assert!(!caps2.api.pane_split);
    assert!(!caps2.api.workspace_rename);
    for err in [
        mgr2.workspace_rename(Some("work"), "ws".into(), "X".into())
            .unwrap_err(),
        mgr2.workspace_close(Some("work"), "ws".into()).unwrap_err(),
        mgr2.tab_focus(Some("work"), "tab".into()).unwrap_err(),
        mgr2.tab_rename(Some("work"), "tab".into(), "T".into())
            .unwrap_err(),
        mgr2.tab_close(Some("work"), "tab".into()).unwrap_err(),
        mgr2.pane_focus(Some("work"), "pane".into()).unwrap_err(),
        mgr2.pane_rename(Some("work"), "pane".into(), Some("P".into()))
            .unwrap_err(),
        mgr2.pane_split(
            Some("work"),
            HerdrSplitDirection::Down,
            Some("pane".into()),
            None,
            None,
            None,
            true,
        )
        .unwrap_err(),
        mgr2.pane_zoom(Some("work"), Some("pane".into()), None)
            .unwrap_err(),
        mgr2.pane_swap(Some("work"), Some("a".into()), Some("b".into()), None, None)
            .unwrap_err(),
        mgr2.pane_close(Some("work"), "pane".into()).unwrap_err(),
        mgr2.layout_export(Some("work"), Some("tab".into()), None)
            .unwrap_err(),
        mgr2.layout_set_split_ratio(Some("work"), Some("tab".into()), None, vec![false], 0.5)
            .unwrap_err(),
    ] {
        assert!(err.contains("not running"), "{err}");
    }
}

#[test]
fn parse_worktree_list_response_accepts_protocol19_fields() {
    let parsed = parse_worktree_list_response(serde_json::json!({
        "id": "1",
        "result": {
            "type": "worktree_list",
            "source": {
                "repo_key": "/repo/.git",
                "repo_name": "repo",
                "repo_root": "/repo",
                "source_checkout_path": "/repo",
                "source_workspace_id": "w1"
            },
            "worktrees": [
                {
                    "path": "/repo",
                    "branch": "main",
                    "is_bare": false,
                    "is_detached": false,
                    "is_prunable": false,
                    "is_linked_worktree": false,
                    "label": "repo",
                    "open_workspace_id": "w1"
                },
                {
                    "path": "/repo-feature",
                    "branch": null,
                    "is_bare": false,
                    "is_detached": true,
                    "is_prunable": true,
                    "is_linked_worktree": true,
                    "label": "feature",
                    "open_workspace_id": "w2"
                },
                {
                    "path": "\\\\?\\C:\\src\\yuzora",
                    "branch": "main",
                    "is_bare": false,
                    "is_detached": false,
                    "is_prunable": false,
                    "is_linked_worktree": false,
                    "label": "win",
                    "open_workspace_id": "w3"
                }
            ]
        }
    }))
    .unwrap();
    assert_eq!(parsed.source.repo_name, "repo");
    assert_eq!(parsed.source.source_workspace_id.as_deref(), Some("w1"));
    assert_eq!(parsed.worktrees.len(), 3);
    assert_eq!(parsed.worktrees[0].branch.as_deref(), Some("main"));
    assert!(!parsed.worktrees[0].is_linked_worktree);
    assert!(parsed.worktrees[1].is_detached);
    assert!(parsed.worktrees[1].is_linked_worktree);
    assert!(parsed.worktrees[1].branch.is_none());
    assert_eq!(parsed.worktrees[2].path, "\\\\?\\C:\\src\\yuzora");
}

#[test]
fn parse_worktree_list_rejects_wrong_type() {
    let err = parse_worktree_list_response(serde_json::json!({
        "id": "1",
        "result": { "type": "workspace_list", "workspaces": [] }
    }))
    .unwrap_err();
    assert!(
        err.contains("unexpected worktree.list result type"),
        "{err}"
    );
}

#[test]
fn parse_subscription_event_worktree_dirty_signals() {
    let cases = [
        (
            r#"{"event":"worktree_created","data":{"type":"worktree_created","workspace":{"workspace_id":"w9"},"worktree":{"path":"/tmp/x"}}}"#,
            "created",
        ),
        (
            r#"{"event":"worktree_opened","data":{"type":"worktree_opened","workspace":{"workspace_id":"w9"},"worktree":{"path":"/tmp/x"},"already_open":false}}"#,
            "opened",
        ),
        (
            r#"{"event":"worktree_removed","data":{"type":"worktree_removed","workspace_id":"w9","worktree":{"path":"/tmp/x"},"forced":false}}"#,
            "removed",
        ),
    ];
    for (line, expected_kind) in cases {
        let event = parse_subscription_event_line("sub-1", line)
            .unwrap()
            .unwrap();
        assert_eq!(
            event,
            HerdrSubscriptionEvent::WorktreeChanged {
                subscription_id: "sub-1".into(),
                kind: expected_kind.into(),
                workspace_id: Some("w9".into()),
            }
        );
    }
}

#[test]
fn parse_subscription_event_accepts_dotted_worktree_selector_envelopes() {
    let event = parse_subscription_event_line(
        "sub-1",
        r#"{"event":"worktree.created","data":{"workspace":{"workspace_id":"w9"}}}"#,
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        event,
        HerdrSubscriptionEvent::WorktreeChanged {
            subscription_id: "sub-1".into(),
            kind: "created".into(),
            workspace_id: Some("w9".into()),
        }
    );
}

#[cfg(unix)]
#[test]
fn worktree_list_refuses_stopped_named_session() {
    let dir = tempfile::tempdir().unwrap();
    let binary = write_fake_herdr_with_sessions(
        dir.path(),
        r#"{"client":{"version":"0.0.0-fake","channel":"test","protocol":19,"binary":"FAKE"},"server":{"status":"stopped","running":false,"version":"0.0.0-fake","protocol":19,"compatible":true,"socket":"/tmp/work.sock"},"update":{"restart_needed":false}}"#,
        r#"{"protocol":19,"schema_version":1,"methods":["session.snapshot","worktree.list"],"schemas":{"session.snapshot":{},"worktree.list":{}}}"#,
        r#"{"sessions":[{"name":"work","default":false,"running":false,"session_dir":"/tmp/work","socket_path":"/tmp/work.sock"},{"name":"default","default":true,"running":true,"session_dir":"/tmp/default","socket_path":"/tmp/default.sock"}]}"#,
    );
    let mgr = HerdrManager::with_binary(binary);
    let err = mgr
        .worktree_list(Some("work"), None, Some("w1".into()))
        .unwrap_err();
    assert!(err.contains("not running"), "{err}");
}

#[cfg(unix)]
#[test]
fn worktree_list_capability_is_schema_gated() {
    let dir = tempfile::tempdir().unwrap();
    let binary = write_fake_herdr_with_sessions(
        dir.path(),
        r#"{"client":{"version":"0.0.0-fake","channel":"test","protocol":19,"binary":"FAKE"},"server":{"status":"running","running":true,"version":"0.0.0-fake","protocol":19,"compatible":true,"socket":"/tmp/default.sock"},"update":{"restart_needed":false}}"#,
        r#"{"protocol":19,"schema_version":1,"methods":["session.snapshot","workspace.focus"],"schemas":{"session.snapshot":{},"workspace.focus":{}}}"#,
        r#"{"sessions":[{"name":"default","default":true,"running":true,"session_dir":"/tmp/default","socket_path":"/tmp/default.sock"}]}"#,
    );
    let mgr = HerdrManager::with_binary(binary);
    let caps = mgr.discover_capabilities_for_session(Some("default"));
    assert!(!caps.api.worktree_list);
    assert!(!caps.api.methods.iter().any(|m| m == "worktree.list"));
    let err = mgr.worktree_list(Some("default"), None, None).unwrap_err();
    // Schema-gated false method surfaces the capability reason (never invents success).
    assert!(
        err.contains("unavailable")
            || err.contains("not")
            || err.contains("lacks")
            || err.contains("worktree.list")
            || err.contains("socket probe failed"),
        "{err}"
    );
}

#[cfg(unix)]
#[test]
fn tab_move_capability_is_schema_gated() {
    let dir = tempfile::tempdir().unwrap();
    let binary = write_fake_herdr_with_sessions(
        dir.path(),
        r#"{"client":{"version":"0.0.0-fake","channel":"test","protocol":19,"binary":"FAKE"},"server":{"status":"running","running":true,"version":"0.0.0-fake","protocol":19,"compatible":true,"socket":"/tmp/default.sock"},"update":{"restart_needed":false}}"#,
        r#"{"protocol":19,"schema_version":1,"methods":["session.snapshot","tab.create"],"schemas":{"session.snapshot":{},"tab.create":{}}}"#,
        r#"{"sessions":[{"name":"default","default":true,"running":true,"session_dir":"/tmp/default","socket_path":"/tmp/default.sock"}]}"#,
    );
    let mgr = HerdrManager::with_binary(binary);
    let caps = mgr.discover_capabilities_for_session(Some("default"));
    assert!(!caps.api.tab_move);
    assert!(!caps.api.methods.iter().any(|m| m == "tab.move"));
    let err = mgr
        .tab_move(Some("default"), "tab_1".into(), 1)
        .unwrap_err();
    assert!(
        err.contains("unavailable")
            || err.contains("not")
            || err.contains("lacks")
            || err.contains("tab.move")
            || err.contains("socket probe failed"),
        "{err}"
    );
}

#[cfg(unix)]
#[test]
fn workspace_move_capability_is_schema_gated() {
    let dir = tempfile::tempdir().unwrap();
    let binary = write_fake_herdr_with_sessions(
        dir.path(),
        r#"{"client":{"version":"0.0.0-fake","channel":"test","protocol":19,"binary":"FAKE"},"server":{"status":"running","running":true,"version":"0.0.0-fake","protocol":19,"compatible":true,"socket":"/tmp/default.sock"},"update":{"restart_needed":false}}"#,
        r#"{"protocol":19,"schema_version":1,"methods":["session.snapshot","workspace.focus"],"schemas":{"session.snapshot":{},"workspace.focus":{}}}"#,
        r#"{"sessions":[{"name":"default","default":true,"running":true,"session_dir":"/tmp/default","socket_path":"/tmp/default.sock"}]}"#,
    );
    let mgr = HerdrManager::with_binary(binary);
    let caps = mgr.discover_capabilities_for_session(Some("default"));
    assert!(!caps.api.workspace_move);
    assert!(!caps.api.methods.iter().any(|m| m == "workspace.move"));
    let err = mgr
        .workspace_move(Some("default"), "workspace_1".into(), 1)
        .unwrap_err();
    assert!(
        err.contains("unavailable")
            || err.contains("not")
            || err.contains("lacks")
            || err.contains("workspace.move")
            || err.contains("socket probe failed"),
        "{err}"
    );
}

#[cfg(unix)]
#[test]
fn workspace_move_block_capability_is_schema_gated() {
    let dir = tempfile::tempdir().unwrap();
    let binary = write_fake_herdr_with_sessions(
        dir.path(),
        r#"{"client":{"version":"0.0.0-fake","channel":"test","protocol":22,"binary":"FAKE"},"server":{"status":"running","running":true,"version":"0.0.0-fake","protocol":22,"compatible":true,"socket":"/tmp/default.sock"},"update":{"restart_needed":false}}"#,
        r#"{"protocol":22,"schema_version":2,"methods":["session.snapshot","workspace.focus","workspace.move_block"],"schemas":{"session.snapshot":{},"workspace.focus":{},"workspace.move_block":{}}}"#,
        r#"{"sessions":[{"name":"default","default":true,"running":true,"session_dir":"/tmp/default","socket_path":"/tmp/default.sock"}]}"#,
    );
    let mgr = HerdrManager::with_binary(binary);
    let caps = mgr.discover_capabilities_for_session(Some("default"));
    assert!(caps.api.workspace_move_block);
    assert!(caps.api.methods.iter().any(|m| m == "workspace.move_block"));
    assert!(mgr
        .workspace_move_block(Some("default"), Vec::new(), None)
        .is_err());
}

#[cfg(unix)]
#[test]
fn herdr_cli_oversized_stdout_is_killed_and_too_large() {
    let script = format!(
        "import sys; sys.stdout.buffer.write(b'x' * {})",
        MAX_NDJSON_LINE_BYTES + 8
    );
    let mut child = Command::new("python3")
        .args(["-c", &script])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("python3");
    let mut process_tree = process_kill::attach_process_tree(&mut child).unwrap();
    let err = wait_bounded_child(
        &mut child,
        &mut process_tree,
        Duration::from_secs(5),
        MAX_NDJSON_LINE_BYTES,
    )
    .expect_err("oversized stdout");
    assert!(err.contains("tooLarge"), "{err}");
}

#[cfg(unix)]
#[test]
fn herdr_cli_invalid_utf8_is_protocol_error() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), [0xff, 0xfe]).unwrap();
    let err = run_herdr_json_with_session_timeout(
        Path::new("/bin/cat"),
        &[tmp.path().to_str().unwrap()],
        None,
        Duration::from_secs(2),
    )
    .unwrap_err();
    assert!(err.contains("invalidUtf8"), "{err}");
}

#[cfg(unix)]
#[test]
fn herdr_cli_normal_json_is_parsed() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), br#"{"ok":true}"#).unwrap();
    let value = run_herdr_json_with_session_timeout(
        Path::new("/bin/cat"),
        &[tmp.path().to_str().unwrap()],
        None,
        Duration::from_secs(2),
    )
    .unwrap();
    assert_eq!(value["ok"], true);
}

#[cfg(unix)]
#[test]
fn herdr_cli_timeout_kills_and_reaps_child() {
    let started = Instant::now();
    let mut child = Command::new("sleep")
        .arg("30")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let pid = child.id();
    let mut process_tree = process_kill::attach_process_tree(&mut child).unwrap();
    let err = wait_bounded_child(
        &mut child,
        &mut process_tree,
        Duration::from_millis(80),
        MAX_NDJSON_LINE_BYTES,
    )
    .expect_err("timeout");
    assert!(err.contains("timeout"), "{err}");
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "30s child was not reaped within the configured timeout"
    );
    assert!(!unix_pid_exists(pid), "child {pid} should be reaped");
}

#[cfg(unix)]
#[test]
fn herdr_cli_success_with_invalid_utf8_stderr_is_protocol_error() {
    let script = r#"
import sys
sys.stdout.buffer.write(b'{"ok":true}')
sys.stderr.buffer.write(b"\xff\xfe")
"#;
    let err = run_herdr_json_with_session_timeout(
        Path::new("python3"),
        &["-c", script],
        None,
        Duration::from_secs(2),
    )
    .unwrap_err();
    assert!(err.contains("invalidUtf8"), "{err}");
}

#[cfg(unix)]
#[test]
fn herdr_cli_long_child_is_reaped_within_timeout() {
    let started = Instant::now();
    let err = run_herdr_json_with_session_timeout(
        Path::new("sleep"),
        &["30"],
        None,
        Duration::from_millis(120),
    )
    .unwrap_err();
    assert!(err.contains("timeout"), "{err}");
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "long child was not reaped within timeout"
    );
}

#[cfg(unix)]
fn unix_pid_exists(pid: u32) -> bool {
    #[cfg(unix)]
    {
        let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
        if rc == 0 {
            return true;
        }
        std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}
