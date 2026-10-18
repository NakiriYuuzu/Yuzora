use super::*;
use std::collections::BTreeSet;

// The gateway's own init fixture; it parses init with deny_unknown_fields.
const GATEWAY_INIT: &str = include_str!("../../companion/tests/fixtures/init.json");

fn applied() -> AppliedServe {
    AppliedServe {
        https_port: 8443,
        gateway_port: 47110,
        host: "desktop.tail1234.ts.net".into(),
        login: "user@example.com".into(),
    }
}
fn keys(value: &Value) -> BTreeSet<String> {
    value.as_object().unwrap().keys().cloned().collect()
}

#[test]
fn companion_init_matches_the_gateway_schema() {
    let fixture: Value = serde_json::from_str(GATEWAY_INIT).unwrap();
    let settings = Settings {
        session_allowlist: vec!["default".into()],
        ..Settings::default()
    };
    let mut init = build_init(
        &settings,
        &applied(),
        Path::new("/opt/herdr"),
        Path::new("/opt/companion-web"),
        "2030-01-01T08:00:00Z",
    )
    .unwrap();
    // The supervisor adds devices from disk at every start.
    init["devices"] = json!([]);
    assert_eq!(keys(&init), keys(&fixture));
    for path in ["/listeners", "/listeners/loopback", "/policy"] {
        assert_eq!(
            keys(init.pointer(path).unwrap()),
            keys(fixture.pointer(path).unwrap()),
            "{path}"
        );
    }
    assert_eq!(init["sessionAllowlist"], json!(["default"]));
    assert_eq!(init["listeners"]["loopback"]["port"], 47110);
    assert_eq!(
        init["listeners"]["loopback"]["publicHost"],
        "desktop.tail1234.ts.net:8443"
    );
    assert_eq!(
        init["listeners"]["loopback"]["requiredTailscaleLogin"],
        "user@example.com"
    );
    assert_eq!(init["policy"]["enableUntil"], "2030-01-01T08:00:00Z");
    assert_eq!(init["policy"]["fullSessionEnabled"], false);
    let mut unapplied = applied();
    unapplied.login.clear();
    assert!(build_init(&settings, &unapplied, Path::new("h"), Path::new("w"), "t").is_err());
}

#[test]
fn companion_public_host_omits_the_default_port() {
    assert_eq!(
        public_host("Desktop.tail1234.ts.net", 8443),
        "desktop.tail1234.ts.net:8443"
    );
    assert_eq!(
        public_host("desktop.tail1234.ts.net", 443),
        "desktop.tail1234.ts.net"
    );
}

#[test]
fn companion_phone_link_is_the_served_address_without_a_secret() {
    let link = phone_link(&applied()).unwrap();
    assert_eq!(link.url, "https://desktop.tail1234.ts.net:8443/");
    assert!(link.qr_svg.contains("<svg"));
    let default_port = AppliedServe {
        https_port: 443,
        ..applied()
    };
    assert_eq!(
        phone_link(&default_port).unwrap().url,
        "https://desktop.tail1234.ts.net/"
    );
}

#[test]
fn companion_serve_record_is_forgotten_only_off_its_node() {
    let applied = applied();
    assert!(!may_forget(Some("desktop.tail1234.ts.net"), &applied));
    assert!(may_forget(Some("renamed.tail1234.ts.net"), &applied));
    assert!(may_forget(Some("desktop.other.ts.net"), &applied));
    // Logged out, no CLI, or a probe that failed.
    assert!(may_forget(None, &applied));
}

#[test]
fn companion_serve_record_is_kept_while_ours_may_still_serve() {
    let check = |file: &str| {
        let json = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("src/companion_tailscale/fixtures")
                .join(file),
        )
        .unwrap();
        // The fixtures proxy to gateway port 43123.
        tailscale::check_serve(&json, "desktop.tail1234.ts.net", 8443, 43123).unwrap()
    };
    let host = Some("desktop.tail1234.ts.net");
    let ours = check("serve-ours.json");
    assert_eq!(ours.port, tailscale::PortState::Ours);
    // Ours, alone or next to a mount the user added: only our root goes.
    let shared = check("serve-extra-mount.json");
    assert_eq!(shared.port, tailscale::PortState::Shared);
    for serve in [&ours, &shared] {
        assert_eq!(
            removal(Some(serve), host, &applied()).unwrap(),
            Some(vec![
                "serve".into(),
                "--https=8443".into(),
                "--set-path=/".into(),
                "off".into()
            ])
        );
    }
    // Nothing of ours left on that node: the record can go.
    let free = tailscale::check_serve("null", "desktop.tail1234.ts.net", 8443, 47110).unwrap();
    let taken = check("serve-taken.json");
    assert_eq!(taken.port, tailscale::PortState::Taken);
    for serve in [&free, &taken] {
        assert_eq!(removal(Some(serve), host, &applied()).unwrap(), None);
    }
    // Logged out, another profile or a renamed node: ours may still be there.
    assert_eq!(
        removal(None, host, &applied()).unwrap_err(),
        "companion-tailscale-unavailable"
    );
    for serve in [&ours, &free, &taken] {
        assert_eq!(
            removal(Some(serve), Some("other.tail9999.ts.net"), &applied()).unwrap_err(),
            "companion-tailscale-changed"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn companion_offline_device_edits_wait_for_a_stopped_gateway() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("companion");
    let device: Value =
        serde_json::from_str(include_str!("../../companion/tests/fixtures/device.json")).unwrap();
    Devices::load(&dir).unwrap().upsert(device).unwrap();
    // Never reports ready: a gateway that has read the file but not started.
    let bin = root.path().join("sidecar");
    std::fs::write(
        &bin,
        "#!/bin/sh\nwhile read line; do case \"$line\" in *shutdown*) exit 0;; esac; done\n",
    )
    .unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let manager = CompanionManager::new(dir.clone(), Arc::new(|_| {}), Arc::new(|| {}));
    let rename = json!({"type": "device.update", "deviceId": "AAAAAAAAAAAAAAAAAAAAAA", "label": "Renamed", "grants": null});
    manager
        .start(Launch {
            program: bin,
            init: json!({}),
        })
        .unwrap();
    assert_eq!(manager.status(), Status::Starting);
    assert_eq!(manager.edit_stopped(&rename).unwrap_err(), "companion-busy");
    assert_eq!(
        stored_device(&dir, "AAAAAAAAAAAAAAAAAAAAAA").unwrap()["label"],
        "Test phone"
    );
    manager.stop().await;
    assert_eq!(manager.status(), Status::Disabled);
    manager.edit_stopped(&rename).unwrap();
    assert_eq!(
        stored_device(&dir, "AAAAAAAAAAAAAAAAAAAAAA").unwrap()["label"],
        "Renamed"
    );
    // A failed start leaves nothing that could write the file either.
    let missing = Launch {
        program: "/nonexistent/yuzora-companion".into(),
        init: json!({}),
    };
    assert!(manager.start(missing).is_err());
    assert!(matches!(manager.status(), Status::Failed { .. }));
    manager
        .edit_stopped(&json!({"type": "device.revoke", "deviceId": "AAAAAAAAAAAAAAAAAAAAAA"}))
        .unwrap();
    assert!(Devices::load(&dir).unwrap().all().is_empty());
}

#[test]
fn companion_one_instance_owns_the_companion() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("companion");
    let first = instance_lock(&dir).expect("first instance");
    assert!(instance_lock(&dir).is_none(), "second instance");
    drop(first);
    // A child another test forks meanwhile shares the open lock file until
    // its exec closes it, so the lock may free a moment after the drop.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut again = instance_lock(&dir);
    while again.is_none() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
        again = instance_lock(&dir);
    }
    assert!(again.is_some(), "after the first quits");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }
}

#[test]
fn companion_pairing_qr_is_an_svg() {
    let svg = qr_svg("https://desktop.tail1234.ts.net:8443/pair#p=ID.SECRET").unwrap();
    assert!(svg.starts_with("<?xml") || svg.starts_with("<svg"));
    assert!(svg.contains("<svg"));
    // The secret is only in the modules, never as text.
    assert!(!svg.contains("SECRET"));
}

#[test]
fn companion_refusal_codes_match_the_serialized_names() {
    assert_eq!(refusal_code(tailscale::Refusal::PortTaken), "portTaken");
    assert_eq!(
        refusal_code(tailscale::Refusal::HttpsNotReady),
        "httpsNotReady"
    );
}

/// Real sidecar smoke: `YUZORA_COMPANION_SMOKE_BIN=<yuzora-companion> cargo test
/// --lib companion_real_gateway_smoke -- --ignored`. Never executes HERDR.
#[tokio::test]
#[ignore]
async fn companion_real_gateway_smoke() {
    let program = PathBuf::from(std::env::var_os("YUZORA_COMPANION_SMOKE_BIN").unwrap());
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("companion");
    let web = root.path().join("web");
    std::fs::create_dir_all(&web).unwrap();
    std::fs::write(web.join("index.html"), "<!doctype html>").unwrap();
    let device: Value =
        serde_json::from_str(include_str!("../../companion/tests/fixtures/device.json")).unwrap();
    Devices::load(&dir).unwrap().upsert(device.clone()).unwrap();
    let events = Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
    let sink = events.clone();
    let manager = CompanionManager::new(
        dir.clone(),
        Arc::new(move |event| sink.lock().unwrap().push(event)),
        Arc::new(|| {}),
    );
    let mut serve = applied();
    serve.gateway_port = 0;
    let init = build_init(
        &Settings::default(),
        &serve,
        Path::new("/nonexistent/DO-NOT-EXECUTE-HERDR"),
        &web,
        "2099-01-01T00:00:00Z",
    )
    .unwrap();
    manager.start(Launch { program, init }).unwrap();
    for _ in 0..500 {
        if matches!(manager.status(), Status::Running { .. }) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let Status::Running { port } = manager.status() else {
        panic!(
            "not running: {:?} {:?}",
            manager.status(),
            events.lock().unwrap()
        );
    };
    assert_ne!(port, 0);
    // The guard rejects requests without the serve identity header.
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    stream
        .write_all(
            b"GET / HTTP/1.1\r\nHost: desktop.tail1234.ts.net:8443\r\nConnection: close\r\n\r\n",
        )
        .await
        .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    assert!(response.starts_with("HTTP/1.1 403"), "{response}");
    let created = manager.pairing_create(None).await.unwrap();
    let url = created["url"].as_str().unwrap();
    assert!(url.starts_with("https://desktop.tail1234.ts.net:8443/pair#p="));
    assert!(qr_svg(url).unwrap().contains("<svg"));
    let revoked = manager
        .device_command(json!({"type": "device.revoke", "deviceId": "AAAAAAAAAAAAAAAAAAAAAA"}))
        .await
        .unwrap();
    assert_eq!(revoked["type"], "device.revoked");
    assert!(Devices::load(&dir).unwrap().all().is_empty());
    let started = std::time::Instant::now();
    manager.stop().await;
    assert_eq!(manager.status(), Status::Disabled);
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
    let emitted = serde_json::to_string(&*events.lock().unwrap()).unwrap();
    assert!(
        !emitted.contains("/pair#p="),
        "pairing URL leaked to events"
    );
}

// The settings UI is tested against this file; it must be what serde emits.
const UI_FIXTURES: &str = include_str!("../../../src/test/companionFixtures.json");

#[test]
fn companion_ui_fixtures_match_the_serialized_shapes() {
    let cli = || {
        Some(tailscale::Cli {
            path: "/usr/local/bin/tailscale".into(),
            variant: tailscale::Variant::Cli,
        })
    };
    let node = |login: Option<&str>, tagged: bool| tailscale::Node {
        version: "1.104.1".into(),
        backend_state: "Running".into(),
        dns_name: login.map(|_| "desktop.tail1234.ts.net".into()),
        login: login.map(str::to_owned),
        tagged,
        magic_dns: true,
        https_ready: true,
    };
    let serve = |port| tailscale::Serve {
        port,
        foreground: false,
        funnel: false,
        other_ports: vec![],
    };
    let probe = |cli, node, serve, refusal| TailscaleProbe {
        cli,
        node,
        serve,
        refusal,
        https_port: 8443,
        gateway_port: 47110,
        command: "tailscale serve --bg --https=8443 http://127.0.0.1:47110".into(),
    };
    let device = json!({
        "deviceId": "AAAAAAAAAAAAAAAAAAAAAA",
        "label": "iPhone",
        "credentials": [{"rpId": "desktop.tail1234.ts.net"}],
        "grants": {"control": {"until": "2026-10-18T13:00:00Z"}, "full": false},
        "locked": false,
        "createdAt": "2026-10-18T10:00:00Z",
        "lastSeen": "2026-10-18T12:00:00Z",
    });
    let actual = json!({
        "overviewOff": CompanionOverview {
            status: Status::Disabled,
            settings: Settings::default(),
            enable_until: None,
            owner: true,
        },
        "overviewRunning": CompanionOverview {
            status: Status::Running { port: 47110 },
            settings: Settings {
                experimental: true,
                session_allowlist: vec!["default".into()],
                applied: Some(applied()),
                ..Settings::default()
            },
            enable_until: Some("2026-10-18T20:00:00Z".into()),
            owner: true,
        },
        "statusFailed": Status::Failed { code: "companion-crashed".into() },
        "statusStarting": Status::Starting,
        "statusStopping": Status::Stopping,
        "probeReady": probe(cli(), Some(node(Some("user@example.com"), false)), Some(serve(tailscale::PortState::Free)), None),
        "probeNoCli": probe(None, None, None, None),
        "probeLoggedOut": probe(cli(), Some(node(None, false)), None, Some(tailscale::Refusal::NoLogin)),
        "probeTagged": probe(cli(), Some(node(Some("tagged-devices"), true)), Some(serve(tailscale::PortState::Free)), Some(tailscale::Refusal::Tagged)),
        "probePortTaken": probe(cli(), Some(node(Some("user@example.com"), false)), Some(serve(tailscale::PortState::Taken)), Some(tailscale::Refusal::PortTaken)),
        "device": device_summary(&device),
        "pairingQr": PairingQr {
            pairing_id: "pairing-1".into(),
            qr_svg: "<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>".into(),
            expires_at: "2026-10-18T12:05:00Z".into(),
        },
        "phoneLink": PhoneLink {
            url: "https://desktop.tail1234.ts.net:8443/".into(),
            qr_svg: "<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>".into(),
        },
        "sessionsSet": SessionsSet { restart_required: true },
    });
    let expected: Value = serde_json::from_str(UI_FIXTURES).unwrap();
    assert_eq!(
        actual,
        expected,
        "regenerate src/test/companionFixtures.json:\n{}",
        serde_json::to_string_pretty(&actual).unwrap()
    );
}
