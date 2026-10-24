use super::*;
use crate::companion_push::tests::{self as push_tests, Wire};
use std::collections::BTreeSet;

// The gateway's own init fixture; it parses init with deny_unknown_fields.
const GATEWAY_INIT: &str = include_str!("../../companion/tests/fixtures/init.json");

fn identity() -> NetIdentity {
    NetIdentity {
        dns_name: "desktop.tail1234.ts.net".into(),
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
    let secret = URL_SAFE_NO_PAD.encode([7u8; 32]);
    let mut init = build_init(
        &settings,
        &GatewayInit {
            identity: &identity(),
            herdr_binary: Path::new("/opt/herdr"),
            web_root: Path::new("/opt/companion-web"),
            enable_until: "2030-01-01T08:00:00Z",
            proxy_secret: &secret,
            space_roots: &[],
            push: &Value::Null,
        },
    );
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
    // The node serves port 443: no port in the host.
    assert_eq!(
        init["listeners"]["loopback"]["publicHost"],
        "desktop.tail1234.ts.net"
    );
    assert_eq!(
        init["listeners"]["loopback"]["requiredTailscaleLogin"],
        "user@example.com"
    );
    assert_eq!(init["listeners"]["loopback"]["proxySecret"], secret);
    assert_eq!(
        init["trustStore"],
        json!(yuzora_host::workspace_trust::default_store_path())
    );
    assert_eq!(init["spaceRoots"], json!([]));
    assert_eq!(init["push"], Value::Null);
    assert_eq!(init["policy"]["enableUntil"], "2030-01-01T08:00:00Z");
    assert_eq!(init["policy"]["fullSessionEnabled"], false);
}

#[test]
fn companion_phone_link_is_the_served_address_without_a_secret() {
    let link = phone_link(&identity()).unwrap();
    assert_eq!(link.url, "https://desktop.tail1234.ts.net/");
    assert!(link.qr_svg.contains("<svg"));
    let upper = NetIdentity {
        dns_name: "Desktop.Tail1234.ts.net".into(),
        ..identity()
    };
    assert_eq!(
        phone_link(&upper).unwrap().url,
        "https://desktop.tail1234.ts.net/"
    );
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
    let push = push_tests::wire().push(&dir);
    let manager = CompanionManager::new(dir.clone(), Arc::new(|_| {}), Arc::new(|| {}), push);
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
fn companion_pairing_qr_needs_the_cancel_id_and_a_near_future_deadline() {
    let now = chrono::DateTime::parse_from_rfc3339("2026-10-25T10:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let reply = |id: Value, at: Value| json!({"url": "https://desktop.tail1234.ts.net/pair#p=P1.S", "pairingId": id, "expiresAt": at});
    let good = pairing_qr(&reply(json!("P1"), json!("2026-10-25T10:05:00Z")), now).unwrap();
    assert_eq!(good.pairing_id, "P1");
    assert_eq!(good.expires_at, "2026-10-25T10:05:00Z");
    for (id, at) in [
        (Value::Null, json!("2026-10-25T10:05:00Z")),
        (json!(""), json!("2026-10-25T10:05:00Z")),
        (json!("P1"), Value::Null),
        (json!("P1"), json!("")),
        (json!("P1"), json!("soon")),
        (json!("P1"), json!("2026-10-25T10:05:00+02:00")),
        (json!("P1"), json!("2026-10-25T09:59:00Z")),
        (json!("P1"), json!("2026-10-25T10:00:00Z")),
        (json!("P1"), json!("2026-10-25T12:00:01Z")),
        (json!("P1"), json!("2030-01-01T00:05:00Z")),
    ] {
        assert_eq!(
            pairing_qr(&reply(id.clone(), at.clone()), now)
                .err()
                .as_deref(),
            Some("companion-invalid-reply"),
            "{id} {at}"
        );
    }
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
        push_tests::wire().push(&dir),
    );
    // The init frame carries every optional field, so the gateway's own parser
    // judges the shapes.
    let roots = [mapped("default", "w1", web.to_str().unwrap())];
    let push = manager.push().init();
    assert!(push.is_object());
    let settings = Settings {
        gateway_port: 0,
        session_allowlist: vec!["default".into()],
        ..Settings::default()
    };
    let init = build_init(
        &settings,
        &GatewayInit {
            identity: &identity(),
            herdr_binary: Path::new("/nonexistent/DO-NOT-EXECUTE-HERDR"),
            web_root: &web,
            enable_until: "2099-01-01T00:00:00Z",
            proxy_secret: &URL_SAFE_NO_PAD.encode([7u8; 32]),
            space_roots: &roots,
            push: &push,
        },
    );
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
    // The commands the gateway does not answer are accepted: it would say so.
    manager.notify(&roots_frame(&roots));
    manager.notify(&json!({"v": 1, "type": "push.devices", "devices": [{
        "deviceId": "AAAAAAAAAAAAAAAAAAAAAA",
        "endpoints": [crate::companion_push::endpoint_id(&push_tests::endpoint(1))],
    }]}));
    // The guard rejects requests that did not come through the node.
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    stream
        .write_all(b"GET / HTTP/1.1\r\nHost: desktop.tail1234.ts.net\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    assert!(response.starts_with("HTTP/1.1 403"), "{response}");
    let created = manager.pairing_create(None).await.unwrap();
    let url = created["url"].as_str().unwrap();
    assert!(url.starts_with("https://desktop.tail1234.ts.net/pair#p="));
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
    assert!(
        events.lock().unwrap().iter().all(|e| e["type"] != "error"),
        "the gateway rejected a command: {emitted}"
    );
}

// The settings UI is tested against this file; it must be what serde emits.
const UI_FIXTURES: &str = include_str!("../../../src/test/companionFixtures.json");

#[test]
fn companion_ui_fixtures_match_the_serialized_shapes() {
    let logged_out = NetOverview {
        state: net::State::Stopped,
        mode: None,
        auth_url: None,
        dns_name: None,
        login: None,
        https_capable: None,
        cert: None,
        serving: false,
        logged_in: false,
        error: None,
    };
    let known = |net: NetOverview| NetOverview {
        dns_name: Some("desktop.tail1234.ts.net".into()),
        login: Some("user@example.com".into()),
        logged_in: true,
        ..net
    };
    let stopped = known(logged_out.clone());
    let needs_login = NetOverview {
        state: net::State::NeedsLogin,
        mode: Some(net::Mode::Login),
        auth_url: Some("https://login.tailscale.com/a/abc123".into()),
        ..logged_out.clone()
    };
    // Signed in, and waiting for an admin to approve the machine: the name is
    // the pending machine's, not a remembered identity (`logged_in` stays off).
    let needs_approval = NetOverview {
        state: net::State::NeedsApproval,
        mode: Some(net::Mode::Login),
        dns_name: Some("yuzora-1.tail1234.ts.net".into()),
        ..logged_out.clone()
    };
    let enabling = NetOverview {
        state: net::State::Starting,
        mode: Some(net::Mode::Enable),
        ..stopped.clone()
    };
    let signing_out = NetOverview {
        state: net::State::Starting,
        mode: Some(net::Mode::Logout),
        ..stopped.clone()
    };
    let cert_pending = NetOverview {
        state: net::State::Running,
        mode: Some(net::Mode::Enable),
        https_capable: Some(true),
        cert: Some(net::Cert {
            state: net::CertState::Pending,
            not_after: None,
            error: None,
        }),
        ..stopped.clone()
    };
    let running = NetOverview {
        cert: Some(net::Cert {
            state: net::CertState::Ready,
            not_after: Some("2026-12-31T10:00:00Z".into()),
            error: None,
        }),
        serving: true,
        ..cert_pending.clone()
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
    // A device that subscribed, as the UI is told.
    let (_root, _wire, subscribed) = push_tests::ready(&[push_tests::DEVICE_ID]);
    let actual = json!({
        "pushOverview": subscribed.overview(),
        "pushEvent": subscribed.event(),
        "overviewOff": CompanionOverview {
            status: Status::Disabled,
            settings: Settings::default(),
            enable_until: None,
            owner: true,
            net: logged_out.clone(),
            push: PushOverview::default(),
        },
        "overviewRunning": CompanionOverview {
            status: Status::Running { port: 47110 },
            settings: Settings {
                experimental: true,
                session_allowlist: vec!["default".into()],
                net: Some(identity()),
                ..Settings::default()
            },
            enable_until: Some("2026-10-18T20:00:00Z".into()),
            owner: true,
            net: running.clone(),
            push: PushOverview::default(),
        },
        "netLoggedOut": logged_out,
        "netStopped": stopped,
        "netNeedsApproval": needs_approval,
        "netNeedsLogin": needs_login,
        "netEnabling": enabling,
        "netSigningOut": signing_out,
        "netCertPending": cert_pending,
        "netRunning": running,
        "statusFailed": Status::Failed { code: "companion-crashed".into() },
        "statusStarting": Status::Starting,
        "statusStopping": Status::Stopping,
        "device": device_summary(&device),
        "pairingQr": PairingQr {
            pairing_id: "pairing-1".into(),
            qr_svg: "<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>".into(),
            expires_at: "2026-10-18T12:05:00Z".into(),
        },
        "phoneLink": PhoneLink {
            url: "https://desktop.tail1234.ts.net/".into(),
            qr_svg: "<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>".into(),
        },
        "sessionsSet": SessionsSet { restart_required: true },
        "peersSnapshot": crate::companion_service::PeersSnapshot {
            peers: vec![crate::companion_service::PeerRow {
                peer_id: 3,
                device_id: "AAAAAAAAAAAAAAAAAAAAAA".into(),
            }],
            streams: vec![
                crate::companion_service::StreamRow {
                    stream: "12".into(),
                    device_id: "AAAAAAAAAAAAAAAAAAAAAA".into(),
                    peer_id: 3,
                    session: "work".into(),
                    scope: "control".into(),
                    target: Some("w:p".into()),
                },
                crate::companion_service::StreamRow {
                    stream: "13".into(),
                    device_id: "AAAAAAAAAAAAAAAAAAAAAA".into(),
                    peer_id: 3,
                    session: "work".into(),
                    scope: "full".into(),
                    target: None,
                },
            ],
        },
    });
    let expected: Value = serde_json::from_str(UI_FIXTURES).unwrap();
    assert_eq!(
        actual,
        expected,
        "regenerate src/test/companionFixtures.json:\n{}",
        serde_json::to_string_pretty(&actual).unwrap()
    );
}

// The enable flow against fake sidecars. Both log what they see to one file,
// so the order of the steps can be read back.
const LOG: &str = "@LOG@";
const NET_INIT: &str = r#"read init; printf '%s\n' "$init" > "$0.init"; echo net:init >> @LOG@"#;
const NET_RUNNING: &str = r#"echo net:running >> @LOG@
echo '{"v":1,"type":"running","dnsName":"desktop.tail1234.ts.net","login":"user@example.com","displayName":"User","httpsCapable":true,"magicDns":true}'"#;
const NET_RUNNING_NO_HTTPS: &str = r#"echo net:running >> @LOG@
echo '{"v":1,"type":"running","dnsName":"desktop.tail1234.ts.net","login":"user@example.com","displayName":"User","httpsCapable":false,"magicDns":true}'"#;
const NET_NEEDS_LOGIN: &str = r#"echo '{"v":1,"type":"state","backendState":"NeedsLogin"}'
echo '{"v":1,"type":"login-required","authUrl":"https://login.tailscale.com/a/abc123"}'"#;
const NET_CERT: &str = r#"echo '{"v":1,"type":"cert","state":"pending","notAfter":null,"error":null}'
sleep 0.1
echo net:cert-ready >> @LOG@
echo '{"v":1,"type":"cert","state":"ready","notAfter":"2027-01-01T00:00:00Z","error":null}'"#;
// The certificate takes a while; the card can be opened meanwhile.
const NET_CERT_SLOW: &str = r#"echo '{"v":1,"type":"cert","state":"pending","notAfter":null,"error":null}'
sleep 0.6
echo net:cert-ready >> @LOG@
echo '{"v":1,"type":"cert","state":"ready","notAfter":"2027-01-01T00:00:00Z","error":null}'"#;
// The node runs, and then its key expires before the certificate is ready.
const NET_CERT_THEN_LOGIN: &str = r#"echo '{"v":1,"type":"cert","state":"pending","notAfter":null,"error":null}'
sleep 0.1
echo '{"v":1,"type":"state","backendState":"NeedsLogin"}'
echo '{"v":1,"type":"login-required","authUrl":"https://login.tailscale.com/a/abc123"}'"#;
// Signed in, and waiting for an admin to approve the machine.
const NET_NEEDS_APPROVAL: &str =
    r#"echo '{"v":1,"type":"state","backendState":"NeedsMachineAuth"}'"#;
const NET_CERT_THEN_APPROVAL: &str = r#"echo '{"v":1,"type":"cert","state":"pending","notAfter":null,"error":null}'
sleep 0.1
echo '{"v":1,"type":"state","backendState":"NeedsMachineAuth"}'"#;
const NET_CERT_ERROR: &str = r#"echo '{"v":1,"type":"cert","state":"pending","notAfter":null,"error":null}'
echo net:cert-error >> @LOG@
echo '{"v":1,"type":"cert","state":"error","notAfter":null,"error":"rate limited"}'"#;
const NET_LOGIN_LATER: &str = r#"sleep 0.3"#;
const NET_COMMANDS: &str = r#"while read line; do
  case "$line" in
    *'"serve"'*) printf '%s\n' "$line" > "$0.serve"; echo net:serve >> @LOG@; echo '{"v":1,"type":"serving","url":"https://desktop.tail1234.ts.net/"}';;
    *'"unserve"'*) echo net:unserve >> @LOG@; echo '{"v":1,"type":"unserved"}';;
    *'"logout"'*) echo net:logout >> @LOG@; echo '{"v":1,"type":"logged-out"}'; exit 0;;
    *'"shutdown"'*) echo net:shutdown >> @LOG@; exit 0;;
  esac
done"#;
// Never answers serve or unserve.
const NET_COMMANDS_SILENT: &str = r#"while read line; do
  case "$line" in
    *'"serve"'*) echo net:serve >> @LOG@;;
    *'"unserve"'*) echo net:unserve >> @LOG@;;
    *'"shutdown"'*) echo net:shutdown >> @LOG@; exit 0;;
  esac
done"#;
const NET_COMMANDS_REFUSE: &str = r#"while read line; do
  case "$line" in
    *'"serve"'*) echo net:serve >> @LOG@; echo '{"v":1,"type":"error","code":"net-listen-failed","message":"in use"}';;
    *'"unserve"'*) echo net:unserve >> @LOG@; echo '{"v":1,"type":"unserved"}';;
    *'"shutdown"'*) echo net:shutdown >> @LOG@; exit 0;;
  esac
done"#;
// Serves, then dies without being asked to.
const NET_COMMANDS_CRASH: &str = r#"while read line; do
  case "$line" in
    *'"serve"'*) echo net:serve >> @LOG@; echo '{"v":1,"type":"serving","url":"https://desktop.tail1234.ts.net/"}'; sleep 0.3; exit 1;;
    *'"shutdown"'*) echo net:shutdown >> @LOG@; exit 0;;
  esac
done"#;
// Serves, and a moment later says its key has expired.
const NET_COMMANDS_KEY_EXPIRES: &str = r#"while read line; do
  case "$line" in
    *'"serve"'*) echo net:serve >> @LOG@; echo '{"v":1,"type":"serving","url":"https://desktop.tail1234.ts.net/"}'; sleep 0.3
      echo '{"v":1,"type":"state","backendState":"NeedsLogin"}'
      echo '{"v":1,"type":"login-required","authUrl":"https://login.tailscale.com/a/abc123"}';;
    *'"unserve"'*) echo net:unserve >> @LOG@; echo '{"v":1,"type":"unserved"}';;
    *'"shutdown"'*) echo net:shutdown >> @LOG@; exit 0;;
  esac
done"#;
const NET_COMMANDS_LOGOUT_FAILS: &str = r#"while read line; do
  case "$line" in
    *'"logout"'*) echo net:logout >> @LOG@; echo '{"v":1,"type":"error","code":"net-logout-failed","message":"no"}';;
    *'"shutdown"'*) echo net:shutdown >> @LOG@; exit 0;;
  esac
done"#;
const GATEWAY_INIT_LOG: &str =
    r#"read init; printf '%s\n' "$init" > "$0.init"; echo gw:init >> @LOG@"#;
const GATEWAY_READY: &str = r#"echo gw:ready >> @LOG@
echo '{"v":1,"type":"ready","ports":{"loopback":47110,"lan":null}}'
while read line; do
  case "$line" in *'"shutdown"'*) echo gw:shutdown >> @LOG@; exit 0;; esac
done"#;
// Ready only after a while, then logs every command it is sent.
const GATEWAY_READY_LATE: &str = r#"sleep 0.4
echo gw:ready >> @LOG@
echo '{"v":1,"type":"ready","ports":{"loopback":47110,"lan":null}}'
while read line; do
  printf '%s\n' "$line" >> "$0.in"
  case "$line" in *'"shutdown"'*) echo gw:shutdown >> @LOG@; exit 0;; esac
done"#;
const GATEWAY_EXITS: &str = "exit 1";
// After the expiry the gateway keeps its port bound until it is told to go
// (`shutdown`) or its stdin closes, as the real one does.
const GATEWAY_EXPIRES: &str = r#"echo gw:ready >> @LOG@
echo '{"v":1,"type":"ready","ports":{"loopback":47110,"lan":null}}'
sleep 0.3
echo '{"v":1,"type":"error","code":"policy-expired","message":"Companion request rejected"}'
while read line; do case "$line" in *'"shutdown"'*) break;; esac; done
exit 1"#;
// The same, saying when the port is let go.
const GATEWAY_EXPIRES_LOGGED: &str = r#"echo gw:ready >> @LOG@
echo '{"v":1,"type":"ready","ports":{"loopback":47110,"lan":null}}'
sleep 0.3
echo '{"v":1,"type":"error","code":"policy-expired","message":"Companion request rejected"}'
while read line; do case "$line" in *'"shutdown"'*) break;; esac; done
echo gw:port-released >> @LOG@
exit 1"#;
// A gateway holds its port as long as its process lives.
const GATEWAY_PID: &str = r#"echo $$ > "$(dirname "$0")/gateway.pid""#;
// Tells whether the gateway still holds its port.
const NET_GATEWAY_STATE: &str = r#"gw() { if kill -0 "$(cat "$(dirname "$0")/gateway.pid")" 2>/dev/null; then echo alive; else echo dead; fi; }"#;
// Closes its listener slowly, and says what the gateway was doing meanwhile.
const NET_COMMANDS_GUARDED: &str = r#"while read line; do
  case "$line" in
    *'"serve"'*) echo net:serve >> @LOG@; echo '{"v":1,"type":"serving","url":"https://desktop.tail1234.ts.net/"}';;
    *'"unserve"'*) echo net:unserve >> @LOG@; echo "gw:$(gw)-at-unserve" >> @LOG@; sleep 0.5; echo net:unserved >> @LOG@; echo '{"v":1,"type":"unserved"}';;
    *'"shutdown"'*) echo net:shutdown >> @LOG@; echo "gw:$(gw)-at-node-shutdown" >> @LOG@; exit 0;;
  esac
done"#;
// The same, but never says the listener is closed.
const NET_COMMANDS_GUARDED_SILENT: &str = r#"while read line; do
  case "$line" in
    *'"serve"'*) echo net:serve >> @LOG@; echo '{"v":1,"type":"serving","url":"https://desktop.tail1234.ts.net/"}';;
    *'"unserve"'*) echo net:unserve >> @LOG@; echo "gw:$(gw)-at-unserve" >> @LOG@;;
    *'"shutdown"'*) echo net:shutdown >> @LOG@; echo "gw:$(gw)-at-node-shutdown" >> @LOG@; exit 0;;
  esac
done"#;
// Opens its listener slowly, and closes it at once.
const NET_COMMANDS_SLOW_TO_SERVE: &str = r#"while read line; do
  case "$line" in
    *'"serve"'*) echo net:serve >> @LOG@; sleep 0.4; echo '{"v":1,"type":"serving","url":"https://desktop.tail1234.ts.net/"}';;
    *'"unserve"'*) echo net:unserve >> @LOG@; echo "gw:$(gw)-at-unserve" >> @LOG@; echo net:unserved >> @LOG@; echo '{"v":1,"type":"unserved"}';;
    *'"shutdown"'*) echo net:shutdown >> @LOG@; echo "gw:$(gw)-at-node-shutdown" >> @LOG@; exit 0;;
  esac
done"#;
// Ready, and then gone without being asked.
const GATEWAY_READY_THEN_CRASHES: &str = r#"echo gw:ready >> @LOG@
echo '{"v":1,"type":"ready","ports":{"loopback":47110,"lan":null}}'
sleep 0.4
exit 1"#;
// The policy expires as soon as the gateway is ready, and the process then
// lingers after it let go of its port.
const GATEWAY_EXPIRES_AT_ONCE: &str = r#"echo gw:ready >> @LOG@
echo '{"v":1,"type":"ready","ports":{"loopback":47110,"lan":null}}'
echo '{"v":1,"type":"error","code":"policy-expired","message":"Companion request rejected"}'
while read line; do case "$line" in *'"shutdown"'*) break;; esac; done
echo gw:port-released >> @LOG@
sleep 0.6
exit 1"#;

fn flow_timing() -> net::Timing {
    net::Timing {
        running: Duration::from_secs(3),
        cert: Duration::from_secs(3),
        serve: Duration::from_secs(2),
        unserve: Duration::from_secs(2),
        logout: Duration::from_secs(3),
        stop: Duration::from_secs(1),
    }
}

struct Flow {
    _root: tempfile::TempDir,
    dir: PathBuf,
    log: PathBuf,
    net_bin: PathBuf,
    gateway_bin: PathBuf,
    state: CompanionState,
    events: Arc<std::sync::Mutex<Vec<Value>>>,
    wire: Wire,
}
impl Flow {
    fn new(net: &[&str], gateway: &[&str]) -> Self {
        Self::with_timing(net, gateway, flow_timing())
    }
    fn with_timing(net: &[&str], gateway: &[&str], timing: net::Timing) -> Self {
        Self::build(net, gateway, timing, false)
    }
    /// A desktop that has seen its node run: settings remember it, and tsnet
    /// has its state.
    fn registered(net: &[&str], gateway: &[&str]) -> Self {
        let flow = Self::build(net, gateway, flow_timing(), true);
        std::fs::write(flow.dir.join("net/tailscaled.state"), "x").unwrap();
        flow
    }
    fn build(net: &[&str], gateway: &[&str], timing: net::Timing, remembered: bool) -> Self {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("companion");
        if remembered {
            crate::companion_store::create_private_dir(&dir.join("net")).unwrap();
            let settings = Settings {
                net: Some(identity()),
                ..Settings::default()
            };
            settings.save(&dir).unwrap();
        }
        let log = root.path().join("order.log");
        let sidecar = |name: &str, parts: &[&str]| {
            let bin = root.path().join(name);
            let script = parts.join("\n").replace(LOG, log.to_str().unwrap());
            std::fs::write(&bin, format!("#!/bin/sh\n{script}\n")).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            bin
        };
        let (net_bin, gateway_bin) = (sidecar("net", net), sidecar("gateway", gateway));
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = events.clone();
        let wire = push_tests::wire();
        let state = CompanionState::with_emit(
            Arc::new(move |event| sink.lock().unwrap().push(event)),
            "9.9.9".into(),
            timing,
            dir.clone(),
            None,
            wire.push(&dir),
        );
        assert!(state.owner.is_some());
        Self {
            _root: root,
            dir,
            log,
            net_bin,
            gateway_bin,
            state,
            events,
            wire,
        }
    }
    fn inputs(&self) -> Inputs {
        Inputs {
            net_program: self.net_bin.clone(),
            gateway_program: self.gateway_bin.clone(),
            herdr_binary: "/nonexistent/DO-NOT-EXECUTE-HERDR".into(),
            web_root: self.dir.clone(),
        }
    }
    /// What the command does: under `ops`, with the experimental opt-in on.
    async fn enable(&self) -> Result<Status, String> {
        let _turn = self.state.ops.lock().await;
        let settings = Settings {
            experimental: true,
            ..Settings::default()
        };
        run_enable(&self.state, &self.inputs(), settings, EnableFor::OneHour).await
    }
    /// Where the sidecar is found, as `net_logout` asks for it.
    fn net_program(&self) -> impl FnOnce() -> Result<PathBuf, String> + Send {
        let bin = self.net_bin.clone();
        move || Ok(bin)
    }
    fn order(&self) -> Vec<String> {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }
    /// The commands the running gateway was sent, after its init frame.
    fn gateway_input(&self) -> Vec<Value> {
        let file = format!("{}.in", self.gateway_bin.display());
        std::fs::read_to_string(file)
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
    fn json(&self, bin: &Path, suffix: &str) -> Value {
        let file = format!("{}.{suffix}", bin.display());
        serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap()
    }
    async fn until(&self, what: &str, done: impl Fn(&Self) -> bool) {
        for _ in 0..500 {
            if done(self) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!(
            "timed out waiting for {what}: {:?} {:?} {:?}",
            self.state.manager.status(),
            self.state.net.overview(),
            self.order()
        );
    }
}
#[tokio::test]
async fn companion_enable_starts_the_node_then_the_gateway_then_serves() {
    let flow = Flow::new(
        &[NET_INIT, NET_RUNNING, NET_CERT, NET_COMMANDS],
        &[GATEWAY_INIT_LOG, GATEWAY_READY],
    );
    let status = flow.enable().await.unwrap();
    assert_eq!(status, Status::Running { port: 47110 });
    // The certificate is ready before the gateway starts, and the node only
    // serves once the gateway listens.
    let started = [
        "net:init",
        "net:running",
        "net:cert-ready",
        "gw:init",
        "gw:ready",
        "net:serve",
    ];
    assert_eq!(flow.order(), started);

    // One secret ties the node to this gateway; the node is told whom to let in.
    let init = flow.json(&flow.gateway_bin, "init");
    let serve = flow.json(&flow.net_bin, "serve");
    let secret = init["listeners"]["loopback"]["proxySecret"]
        .as_str()
        .unwrap();
    assert_eq!(serve["proxySecret"], secret);
    assert_eq!(URL_SAFE_NO_PAD.decode(secret).unwrap().len(), 32);
    assert_eq!(secret.len(), 43);
    assert_eq!(serve["target"], "http://127.0.0.1:47110");
    assert_eq!(serve["requiredLogin"], "user@example.com");
    assert_eq!(
        init["listeners"]["loopback"]["publicHost"],
        "desktop.tail1234.ts.net"
    );
    assert_eq!(
        init["listeners"]["loopback"]["requiredTailscaleLogin"],
        "user@example.com"
    );
    assert_eq!(
        flow.json(&flow.net_bin, "init"),
        json!({"v": 1, "type": "init", "stateDir": flow.dir.join("net").to_str().unwrap(), "hostname": "yuzora", "version": "9.9.9"})
    );
    let saved = Settings::load(&flow.dir).unwrap();
    assert_eq!(saved.net, Some(identity()));
    assert_eq!(saved.duration, EnableFor::OneHour);
    let overview = overview(&flow.state).unwrap();
    assert!(overview.net.serving);
    assert_eq!(overview.net.state, net::State::Running);
    assert_eq!(overview.net.cert.unwrap().state, net::CertState::Ready);
    let events = serde_json::to_string(&*flow.events.lock().unwrap()).unwrap();
    assert!(
        !events.contains(secret),
        "the proxy secret reached an event"
    );

    // Disable: the listener closes before the gateway goes, the node last.
    disable(&flow.state).await;
    assert_eq!(
        flow.order()[started.len()..],
        ["net:unserve", "gw:shutdown", "net:shutdown"]
    );
    assert_eq!(flow.state.manager.status(), Status::Disabled);
    let stopped = flow.state.net.overview();
    assert_eq!(
        (stopped.state, stopped.serving),
        (net::State::Stopped, false)
    );

    // Every run has its own secret.
    flow.enable().await.unwrap();
    let again = flow.json(&flow.gateway_bin, "init");
    assert_ne!(again["listeners"]["loopback"]["proxySecret"], secret);
    disable(&flow.state).await;
}

/// One way enable can fail: what the sidecars do, and what must have happened
/// by the time it returns.
struct Failure {
    name: &'static str,
    net: &'static [&'static str],
    gateway: &'static [&'static str],
    timing: net::Timing,
    error: &'static str,
    order: &'static [&'static str],
}

#[tokio::test]
async fn companion_enable_failures_stop_everything_they_started() {
    let base = flow_timing();
    let ms = Duration::from_millis;
    let failures = [
        Failure {
            name: "the node needs a login",
            net: &[NET_INIT, NET_NEEDS_LOGIN, NET_COMMANDS],
            gateway: &[GATEWAY_INIT_LOG, GATEWAY_READY],
            timing: base,
            error: "companion-net-login-required",
            order: &["net:init", "net:shutdown"],
        },
        Failure {
            name: "the node waits for an admin to approve it",
            net: &[NET_INIT, NET_NEEDS_APPROVAL, NET_COMMANDS],
            gateway: &[GATEWAY_INIT_LOG, GATEWAY_READY],
            timing: base,
            error: "companion-net-approval-required",
            order: &["net:init", "net:shutdown"],
        },
        Failure {
            name: "the node needs a login while the certificate is pending",
            net: &[NET_INIT, NET_RUNNING, NET_CERT_THEN_LOGIN, NET_COMMANDS],
            gateway: &[GATEWAY_INIT_LOG, GATEWAY_READY],
            timing: base,
            error: "companion-net-login-required",
            order: &["net:init", "net:running", "net:shutdown"],
        },
        Failure {
            name: "the node needs an approval while the certificate is pending",
            net: &[NET_INIT, NET_RUNNING, NET_CERT_THEN_APPROVAL, NET_COMMANDS],
            gateway: &[GATEWAY_INIT_LOG, GATEWAY_READY],
            timing: base,
            error: "companion-net-approval-required",
            order: &["net:init", "net:running", "net:shutdown"],
        },
        Failure {
            name: "https certificates are off for the tailnet",
            net: &[NET_INIT, NET_RUNNING_NO_HTTPS, NET_COMMANDS],
            gateway: &[GATEWAY_INIT_LOG, GATEWAY_READY],
            timing: base,
            error: "companion-net-https-disabled",
            order: &["net:init", "net:running", "net:shutdown"],
        },
        Failure {
            name: "the certificate is never issued",
            net: &[NET_INIT, NET_RUNNING, NET_CERT_ERROR, NET_COMMANDS],
            gateway: &[GATEWAY_INIT_LOG, GATEWAY_READY],
            timing: net::Timing {
                cert: ms(400),
                ..base
            },
            error: "companion-net-cert-failed",
            order: &["net:init", "net:running", "net:cert-error", "net:shutdown"],
        },
        Failure {
            name: "the node never runs",
            net: &[NET_INIT, NET_COMMANDS],
            gateway: &[GATEWAY_INIT_LOG, GATEWAY_READY],
            timing: net::Timing {
                running: ms(400),
                ..base
            },
            error: "companion-net-start-failed",
            order: &["net:init", "net:shutdown"],
        },
        Failure {
            name: "the gateway fails to start",
            net: &[NET_INIT, NET_RUNNING, NET_CERT, NET_COMMANDS],
            gateway: &[GATEWAY_INIT_LOG, GATEWAY_EXITS],
            timing: base,
            error: "companion-exited",
            order: &[
                "net:init",
                "net:running",
                "net:cert-ready",
                "gw:init",
                "net:shutdown",
            ],
        },
        Failure {
            name: "the node never serves",
            net: &[NET_INIT, NET_RUNNING, NET_CERT, NET_COMMANDS_SILENT],
            gateway: &[GATEWAY_INIT_LOG, GATEWAY_READY],
            timing: net::Timing {
                serve: ms(400),
                unserve: ms(300),
                ..base
            },
            error: "companion-net-serve-failed",
            // It never said its listener is closed: it goes before the gateway
            // lets go of its port.
            order: &[
                "net:init",
                "net:running",
                "net:cert-ready",
                "gw:init",
                "gw:ready",
                "net:serve",
                "net:unserve",
                "net:shutdown",
                "gw:shutdown",
            ],
        },
        Failure {
            name: "the node cannot listen",
            net: &[NET_INIT, NET_RUNNING, NET_CERT, NET_COMMANDS_REFUSE],
            gateway: &[GATEWAY_INIT_LOG, GATEWAY_READY],
            timing: base,
            error: "companion-net-serve-failed",
            order: &[
                "net:init",
                "net:running",
                "net:cert-ready",
                "gw:init",
                "gw:ready",
                "net:serve",
                "net:unserve",
                "gw:shutdown",
                "net:shutdown",
            ],
        },
    ];
    for failure in failures {
        let name = failure.name;
        let flow = Flow::with_timing(failure.net, failure.gateway, failure.timing);
        assert_eq!(flow.enable().await.unwrap_err(), failure.error, "{name}");
        assert_eq!(flow.order(), failure.order, "{name}");
        assert!(flow.state.manager.idle(), "{name}: gateway left running");
        assert_eq!(flow.state.net.mode(), None, "{name}: node left running");
        assert_eq!(
            flow.state.net.overview().state,
            net::State::Stopped,
            "{name}"
        );
    }
}

#[tokio::test]
async fn companion_net_crash_halts_the_gateway() {
    let flow = Flow::new(
        &[NET_INIT, NET_RUNNING, NET_CERT, NET_COMMANDS_CRASH],
        &[GATEWAY_INIT_LOG, GATEWAY_READY],
    );
    flow.enable().await.unwrap();
    let halted = Status::Failed {
        code: "companion-net-crashed".into(),
    };
    flow.until("gateway halted", |f| f.state.manager.status() == halted)
        .await;
    flow.until("gateway gone", |f| f.state.manager.idle()).await;
    let node = flow.state.net.overview();
    assert_eq!(node.state, net::State::Failed);
    assert_eq!(node.error.as_deref(), Some("companion-net-crashed"));
    // The gateway was halted, not asked: and the failure stays visible.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(flow.state.manager.status(), halted);
    assert!(!flow.order().contains(&"gw:shutdown".to_string()));
}

#[tokio::test]
async fn companion_gateway_ending_by_itself_stops_the_node() {
    // Expiry: the gateway leaves on its own, and nothing may keep serving it.
    let flow = Flow::new(
        &[NET_INIT, NET_RUNNING, NET_CERT, NET_COMMANDS],
        &[GATEWAY_INIT_LOG, GATEWAY_EXPIRES],
    );
    flow.enable().await.unwrap();
    flow.until("node stopped", |f| {
        f.order().contains(&"net:shutdown".to_string())
    })
    .await;
    assert_eq!(
        flow.order()[5..],
        ["net:serve", "net:unserve", "net:shutdown"]
    );
    assert_eq!(flow.state.manager.status(), Status::Disabled);
    flow.until("node gone", |f| f.state.net.mode().is_none())
        .await;
}

/// What the settings card reads is the last status and the last net overview
/// published. Once a teardown has begun, it must never see a node that runs for
/// an enable beside a status that has already left it: it takes that for an
/// enable in progress and hides every action. Returns the last status.
fn teardown_never_looks_like_an_enable(events: &[Value]) -> Value {
    let (mut status, mut mode) = (Value::Null, Value::Null);
    for (at, event) in events.iter().enumerate() {
        match event["type"].as_str() {
            Some("status") => status = event["status"].clone(),
            Some("net") => mode = event["net"]["mode"].clone(),
            _ => continue,
        }
        let leaving = ["stopping", "disabled", "failed"]
            .iter()
            .any(|state| status["state"] == *state);
        assert!(
            !(leaving && mode == "enable"),
            "event {at}: {status} beside a node that runs for an enable: {events:#?}"
        );
    }
    status
}

#[tokio::test]
async fn companion_disable_never_shows_the_teardown_as_an_enable() {
    let flow = Flow::new(
        &[NET_INIT, NET_RUNNING, NET_CERT, NET_COMMANDS],
        &[GATEWAY_INIT_LOG, GATEWAY_READY],
    );
    flow.enable().await.unwrap();
    assert_eq!(flow.state.net.overview().mode, Some(net::Mode::Enable));
    disable(&flow.state).await;
    let events = flow.events.lock().unwrap().clone();
    let last = teardown_never_looks_like_an_enable(&events);
    assert_eq!(last["state"], "disabled");
    // The logic still knows what the sidecar was for until it is gone.
    assert_eq!(flow.state.net.overview().mode, None);

    // The next enable is an enable again, and the card may say so.
    flow.enable().await.unwrap();
    assert_eq!(flow.state.net.overview().mode, Some(net::Mode::Enable));
    disable(&flow.state).await;
}

#[tokio::test]
async fn companion_expiry_never_shows_the_teardown_as_an_enable() {
    let flow = Flow::new(
        &[NET_INIT, NET_RUNNING, NET_CERT, NET_COMMANDS],
        &[GATEWAY_INIT_LOG, GATEWAY_EXPIRES],
    );
    flow.enable().await.unwrap();
    flow.until("node gone", |f| {
        f.order().contains(&"net:shutdown".to_string()) && f.state.net.mode().is_none()
    })
    .await;
    let events = flow.events.lock().unwrap().clone();
    let last = teardown_never_looks_like_an_enable(&events);
    assert_eq!(last["state"], "disabled");
}

#[tokio::test]
async fn companion_a_halted_gateway_never_shows_the_teardown_as_an_enable() {
    let flow = Flow::new(
        &[NET_INIT, NET_RUNNING, NET_CERT, NET_COMMANDS],
        &[GATEWAY_INIT_LOG, GATEWAY_READY],
    );
    flow.enable().await.unwrap();
    flow.state.manager.fail("companion-timeout");
    flow.until("node gone", |f| {
        f.order().contains(&"net:shutdown".to_string()) && f.state.net.mode().is_none()
    })
    .await;
    let events = flow.events.lock().unwrap().clone();
    let last = teardown_never_looks_like_an_enable(&events);
    assert_eq!(
        last,
        json!({"state": "failed", "code": "companion-timeout"})
    );
}

#[tokio::test]
async fn companion_key_expiry_while_serving_fails_closed() {
    let flow = Flow::new(
        &[NET_INIT, NET_RUNNING, NET_CERT, NET_COMMANDS_KEY_EXPIRES],
        &[GATEWAY_INIT_LOG, GATEWAY_READY],
    );
    assert_eq!(
        flow.enable().await.unwrap(),
        Status::Running { port: 47110 }
    );
    assert_eq!(Settings::load(&flow.dir).unwrap().net, Some(identity()));
    let halted = Status::Failed {
        code: "companion-net-login-required".into(),
    };
    flow.until("gateway halted", |f| f.state.manager.status() == halted)
        .await;
    flow.until("node stopped", |f| {
        f.state.net.overview().state == net::State::Stopped
    })
    .await;
    flow.until("settings forget the node", |f| {
        Settings::load(&f.dir).unwrap().net.is_none()
    })
    .await;
    // The listener closed first, then the node; the gateway was halted, not asked.
    assert_eq!(
        flow.order()[5..],
        ["net:serve", "net:unserve", "net:shutdown"]
    );
    assert!(flow.state.manager.idle());
    assert_eq!(flow.state.manager.status(), halted, "the failure stays");
    let shown = overview(&flow.state).unwrap();
    assert_eq!(shown.status, halted);
    assert_eq!(
        (
            shown.net.state,
            shown.net.mode,
            shown.net.logged_in,
            shown.net.serving,
            shown.net.dns_name,
        ),
        (net::State::Stopped, None, false, false, None)
    );
    let events = flow.events.lock().unwrap().clone();
    let last = teardown_never_looks_like_an_enable(&events);
    assert_eq!(
        last,
        json!({"state": "failed", "code": "companion-net-login-required"})
    );
}

#[tokio::test]
async fn companion_key_expiry_before_the_gateway_starts_keeps_its_error() {
    let flow = Flow::registered(
        &[NET_INIT, NET_NEEDS_LOGIN, NET_COMMANDS],
        &[GATEWAY_INIT_LOG, GATEWAY_READY],
    );
    assert_eq!(
        flow.enable().await.unwrap_err(),
        "companion-net-login-required"
    );
    flow.until("settings forget the node", |f| {
        Settings::load(&f.dir).unwrap().net.is_none()
    })
    .await;
    // No gateway ran, so none was failed: the enable's own error is the only report.
    assert_eq!(flow.order(), ["net:init", "net:shutdown"]);
    assert_eq!(flow.state.manager.status(), Status::Disabled);
    let events = flow.events.lock().unwrap().clone();
    assert!(
        events
            .iter()
            .all(|e| e["type"] != "status" || e["status"]["state"] == "disabled"),
        "{events:#?}"
    );
}

/// Waits for a line of the log, and returns the log.
async fn settled(flow: &Flow, last: &str) -> Vec<String> {
    flow.until(last, |f| f.order().iter().any(|line| line == last))
        .await;
    flow.order()
}

#[tokio::test]
async fn companion_expiry_keeps_the_port_until_the_node_stopped_forwarding() {
    // The gateway's port is held until the desktop has the node's word that
    // its listener is closed; only then is the gateway told to go.
    let flow = Flow::new(
        &[
            NET_GATEWAY_STATE,
            NET_INIT,
            NET_RUNNING,
            NET_CERT,
            NET_COMMANDS_GUARDED,
        ],
        &[GATEWAY_INIT_LOG, GATEWAY_PID, GATEWAY_EXPIRES_LOGGED],
    );
    flow.enable().await.unwrap();
    let order = settled(&flow, "gw:dead-at-node-shutdown").await;
    assert_eq!(
        order[6..],
        [
            "net:unserve",
            "gw:alive-at-unserve",
            "net:unserved",
            "gw:port-released",
            "net:shutdown",
            "gw:dead-at-node-shutdown",
        ],
        "{order:?}"
    );
    // Reported as an expiry always was: disabled, not a failure.
    flow.until("disabled", |f| f.state.manager.status() == Status::Disabled)
        .await;
    flow.until("node gone", |f| f.state.net.mode().is_none())
        .await;
    let last = teardown_never_looks_like_an_enable(&flow.events.lock().unwrap());
    assert_eq!(last["state"], "disabled");
}

#[tokio::test]
async fn companion_expiry_stops_the_node_first_when_it_does_not_confirm_the_listener_closed() {
    let ms = Duration::from_millis;
    let flow = Flow::with_timing(
        &[
            NET_GATEWAY_STATE,
            NET_INIT,
            NET_RUNNING,
            NET_CERT,
            NET_COMMANDS_GUARDED_SILENT,
        ],
        &[GATEWAY_INIT_LOG, GATEWAY_PID, GATEWAY_EXPIRES_LOGGED],
        net::Timing {
            unserve: ms(300),
            ..flow_timing()
        },
    );
    flow.enable().await.unwrap();
    let order = settled(&flow, "gw:port-released").await;
    // The node was stopped, with the gateway still holding its port, before
    // the gateway was let go.
    assert_eq!(
        order[6..],
        [
            "net:unserve",
            "gw:alive-at-unserve",
            "net:shutdown",
            "gw:alive-at-node-shutdown",
            "gw:port-released",
        ],
        "{order:?}"
    );
    flow.until("disabled", |f| f.state.manager.status() == Status::Disabled)
        .await;
    flow.until("node gone", |f| f.state.net.mode().is_none())
        .await;
    tokio::time::sleep(ms(200)).await;
    assert_eq!(flow.order(), order, "nothing more was asked of the node");
}

#[tokio::test]
async fn companion_a_halt_keeps_the_gateway_until_the_node_stopped_forwarding() {
    let flow = Flow::new(
        &[
            NET_GATEWAY_STATE,
            NET_INIT,
            NET_RUNNING,
            NET_CERT,
            NET_COMMANDS_GUARDED,
        ],
        &[GATEWAY_INIT_LOG, GATEWAY_PID, GATEWAY_READY],
    );
    flow.enable().await.unwrap();
    flow.state.manager.fail("companion-timeout");
    let failed = Status::Failed {
        code: "companion-timeout".into(),
    };
    // The failure is out at once; the node is asked to close, and the gateway
    // still holds its port while it does.
    flow.until("node asked", |f| {
        f.order().contains(&"net:unserve".to_string())
    })
    .await;
    assert_eq!(flow.state.manager.status(), failed);
    assert!(flow
        .events
        .lock()
        .unwrap()
        .iter()
        .any(|e| e["type"] == "status"
            && e["status"] == json!({"state": "failed", "code": "companion-timeout"})));
    assert!(!flow.state.manager.idle(), "killed before the node closed");
    let order = settled(&flow, "gw:dead-at-node-shutdown").await;
    assert_eq!(
        order[6..],
        [
            "net:unserve",
            "gw:alive-at-unserve",
            "net:unserved",
            "net:shutdown",
            "gw:dead-at-node-shutdown",
        ],
        "{order:?}"
    );
    flow.until("gateway gone", |f| f.state.manager.idle()).await;
    assert_eq!(flow.state.manager.status(), failed, "the failure stays");
    let last = teardown_never_looks_like_an_enable(&flow.events.lock().unwrap());
    assert_eq!(
        last,
        json!({"state": "failed", "code": "companion-timeout"})
    );
}

#[tokio::test]
async fn companion_a_halt_stops_the_node_first_when_it_does_not_confirm_the_listener_closed() {
    let ms = Duration::from_millis;
    let flow = Flow::with_timing(
        &[
            NET_GATEWAY_STATE,
            NET_INIT,
            NET_RUNNING,
            NET_CERT,
            NET_COMMANDS_GUARDED_SILENT,
        ],
        &[GATEWAY_INIT_LOG, GATEWAY_PID, GATEWAY_READY],
        net::Timing {
            unserve: ms(300),
            ..flow_timing()
        },
    );
    flow.enable().await.unwrap();
    flow.state.manager.fail("companion-timeout");
    let order = settled(&flow, "gw:alive-at-node-shutdown").await;
    assert_eq!(
        order[6..],
        [
            "net:unserve",
            "gw:alive-at-unserve",
            "net:shutdown",
            "gw:alive-at-node-shutdown",
        ],
        "{order:?}"
    );
    // And now the gateway goes.
    flow.until("gateway gone", |f| f.state.manager.idle()).await;
    flow.until("node gone", |f| f.state.net.mode().is_none())
        .await;
    tokio::time::sleep(ms(200)).await;
    assert_eq!(flow.order(), order, "nothing more was asked of the node");
    assert_eq!(
        flow.state.manager.status(),
        Status::Failed {
            code: "companion-timeout".into()
        }
    );
}

// Slow to leave once it is asked to: it is still there, and still has not
// said its listener is closed, while a gateway that is told to go would let go
// of its port.
const NET_COMMANDS_GUARDED_SILENT_SLOW_EXIT: &str = r#"while read line; do
  case "$line" in
    *'"serve"'*) echo net:serve >> @LOG@; echo '{"v":1,"type":"serving","url":"https://desktop.tail1234.ts.net/"}';;
    *'"unserve"'*) echo net:unserve >> @LOG@; echo "gw:$(gw)-at-unserve" >> @LOG@;;
    *'"shutdown"'*) echo net:shutdown >> @LOG@; echo "gw:$(gw)-at-node-shutdown" >> @LOG@; sleep 0.7; echo net:exit >> @LOG@; exit 0;;
  esac
done"#;

/// A flow whose node, running `commands`, never says its listener is closed.
fn silent_node_flow(commands: &str, gateway: &[&str]) -> Flow {
    let ms = Duration::from_millis;
    Flow::with_timing(
        &[NET_GATEWAY_STATE, NET_INIT, NET_RUNNING, NET_CERT, commands],
        gateway,
        net::Timing {
            unserve: ms(300),
            ..flow_timing()
        },
    )
}

/// What a requested stop of a Companion with a node that does not confirm
/// `unserve` must look like: the node is stopped while the gateway still holds
/// its port, and only then is the gateway told to go.
fn node_first_then_gateway(order: &[String]) {
    assert_eq!(
        order[6..],
        [
            "net:unserve",
            "gw:alive-at-unserve",
            "net:shutdown",
            "gw:alive-at-node-shutdown",
            "gw:shutdown",
        ],
        "{order:?}"
    );
}

#[tokio::test]
async fn companion_disable_stops_a_node_that_does_not_confirm_the_listener_closed_before_the_gateway(
) {
    let flow = silent_node_flow(
        NET_COMMANDS_GUARDED_SILENT,
        &[GATEWAY_INIT_LOG, GATEWAY_PID, GATEWAY_READY],
    );
    flow.enable().await.unwrap();
    disable(&flow.state).await;
    node_first_then_gateway(&flow.order());
    assert_eq!(flow.state.manager.status(), Status::Disabled);
    assert_eq!(flow.state.net.mode(), None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn companion_app_exit_stops_a_node_that_does_not_confirm_the_listener_closed_before_the_gateway(
) {
    let flow = Arc::new(silent_node_flow(
        NET_COMMANDS_GUARDED_SILENT,
        &[GATEWAY_INIT_LOG, GATEWAY_PID, GATEWAY_READY],
    ));
    flow.enable().await.unwrap();
    let exiting = flow.clone();
    tokio::task::spawn_blocking(move || exiting.state.shutdown())
        .await
        .unwrap();
    node_first_then_gateway(&flow.order());
    assert_eq!(flow.state.manager.status(), Status::Disabled);
    assert_eq!(flow.state.net.mode(), None);
}

#[tokio::test]
async fn companion_disable_during_an_expiry_hold_does_not_free_the_port_before_the_node_is_gone() {
    // The expiry hook is waiting for a node that says nothing, and the node is
    // slow to leave once stopped. Disable pressed meanwhile must not let the
    // gateway go while the node is there.
    let flow = silent_node_flow(
        NET_COMMANDS_GUARDED_SILENT_SLOW_EXIT,
        &[GATEWAY_INIT_LOG, GATEWAY_PID, GATEWAY_EXPIRES_LOGGED],
    );
    flow.enable().await.unwrap();
    flow.until("the hook asked the node", |f| {
        f.order().contains(&"net:unserve".to_string())
    })
    .await;
    {
        let _turn = flow.state.ops.lock().await;
        disable(&flow.state).await;
    }
    let order = settled(&flow, "gw:port-released").await;
    let at = |line: &str| {
        order
            .iter()
            .position(|l| l == line)
            .unwrap_or_else(|| panic!("{line} missing: {order:?}"))
    };
    assert!(
        at("net:exit") < at("gw:port-released"),
        "the gateway let go of its port while the node was alive: {order:?}"
    );
    assert_eq!(flow.state.net.mode(), None);
}

/// An enable that has seen the gateway ready and is about to tell the node: it
/// waits at the gate, as if it were slow to get there, until `happened` says
/// the gateway has left (`mishap` makes it, or it does by itself).
async fn enable_at_the_gate(
    flow: &Flow,
    mishap: impl FnOnce(&Flow),
    happened: impl Fn(&Flow) -> bool,
) -> Result<Status, String> {
    let held = flow.state.forwarding.serving.lock().await;
    let gate = async {
        flow.until("gateway ready", |f| {
            matches!(f.state.manager.status(), Status::Running { .. })
        })
        .await;
        // The enable polls for the gateway every 50 ms.
        tokio::time::sleep(Duration::from_millis(300)).await;
        mishap(flow);
        flow.until("the gateway left", happened).await;
        drop(held);
    };
    tokio::join!(flow.enable(), gate).0
}

#[tokio::test]
async fn companion_an_enable_does_not_serve_a_gateway_halted_since_it_was_ready() {
    let flow = Flow::new(
        &[
            NET_GATEWAY_STATE,
            NET_INIT,
            NET_RUNNING,
            NET_CERT,
            NET_COMMANDS_GUARDED,
        ],
        &[GATEWAY_INIT_LOG, GATEWAY_PID, GATEWAY_READY],
    );
    // Its port is as good as free: the node must never be told to forward to it.
    let enabled = enable_at_the_gate(
        &flow,
        |f| f.state.manager.fail("companion-timeout"),
        |f| f.state.manager.status() != Status::Running { port: 47110 },
    )
    .await;
    assert_eq!(enabled.unwrap_err(), "companion-timeout");
    let order = settled(&flow, "gw:dead-at-node-shutdown").await;
    assert!(
        order
            .iter()
            .all(|line| line != "net:serve" && line != "net:unserve"),
        "{order:?}"
    );
    flow.until("gateway gone", |f| f.state.manager.idle()).await;
    flow.until("node gone", |f| f.state.net.mode().is_none())
        .await;
}

#[tokio::test]
async fn companion_an_enable_does_not_serve_a_gateway_that_crashed_since_it_was_ready() {
    // Nothing orders a gateway that dies by itself: no halt came first, so
    // only the status says it is gone.
    let flow = Flow::new(
        &[NET_INIT, NET_RUNNING, NET_CERT, NET_COMMANDS],
        &[GATEWAY_INIT_LOG, GATEWAY_READY_THEN_CRASHES],
    );
    let enabled = enable_at_the_gate(
        &flow,
        |_| {},
        |f| {
            f.state.manager.status()
                == Status::Failed {
                    code: "companion-crashed".into(),
                }
        },
    )
    .await;
    assert_eq!(enabled.unwrap_err(), "companion-crashed");
    let order = settled(&flow, "net:shutdown").await;
    assert!(
        order
            .iter()
            .all(|line| line != "net:serve" && line != "net:unserve"),
        "{order:?}"
    );
}

// Ready, and gone while the node is still answering the serve command.
const GATEWAY_CRASHES_DURING_SERVE: &str = r#"echo gw:ready >> @LOG@
echo '{"v":1,"type":"ready","ports":{"loopback":47110,"lan":null}}'
sleep 0.25
exit 1"#;

#[tokio::test]
async fn companion_an_enable_whose_gateway_crashes_while_serving_fails_and_tears_down_at_once() {
    let flow = Flow::new(
        &[
            NET_GATEWAY_STATE,
            NET_INIT,
            NET_RUNNING,
            NET_CERT,
            NET_COMMANDS_SLOW_TO_SERVE,
        ],
        &[GATEWAY_INIT_LOG, GATEWAY_PID, GATEWAY_CRASHES_DURING_SERVE],
    );
    let enabled = flow.enable().await;
    // Not `Ok(Failed)`: by the time the enable returned, the listener had
    // already been closed.
    assert_eq!(enabled.unwrap_err(), "companion-crashed");
    let order = flow.order();
    assert!(order.contains(&"net:serve".to_string()), "{order:?}");
    assert!(order.contains(&"net:unserve".to_string()), "{order:?}");
    flow.until("node gone", |f| f.state.net.mode().is_none())
        .await;
}

/// Whatever the order of events: when the node was told to serve, the
/// gateway's port was not let go before the node said its listener is closed.
fn never_forwards_to_a_free_port(order: &[String]) {
    let at = |line: &str| order.iter().position(|l| l == line);
    let Some(served) = at("net:serve") else {
        return;
    };
    let released = at("gw:port-released");
    let closed = at("net:unserved");
    assert!(
        released.is_none()
            || closed.is_some_and(|closed| served < closed && closed < released.unwrap()),
        "{order:?}"
    );
}

#[tokio::test]
async fn companion_an_enable_does_not_serve_a_gateway_whose_policy_expired_since_it_was_ready() {
    let flow = Flow::new(
        &[
            NET_GATEWAY_STATE,
            NET_INIT,
            NET_RUNNING,
            NET_CERT,
            NET_COMMANDS_GUARDED,
        ],
        &[GATEWAY_INIT_LOG, GATEWAY_PID, GATEWAY_EXPIRES_AT_ONCE],
    );
    // The gateway expired, was told to go once the node was asked (nothing was
    // served yet), and let go of its port. It lingers, so the status still
    // says it runs when the enable looks.
    let enabled = flow.enable().await;
    let order = settled(&flow, "net:shutdown").await;
    never_forwards_to_a_free_port(&order);
    assert_eq!(enabled.unwrap_err(), "policy-expired", "{order:?}");
    assert!(order.iter().all(|line| line != "net:serve"), "{order:?}");
    flow.until("gateway gone", |f| f.state.manager.idle()).await;
}

#[tokio::test]
async fn companion_a_halt_while_the_node_is_told_to_serve_waits_for_the_answer() {
    let flow = Flow::new(
        &[
            NET_GATEWAY_STATE,
            NET_INIT,
            NET_RUNNING,
            NET_CERT,
            NET_COMMANDS_SLOW_TO_SERVE,
        ],
        &[GATEWAY_INIT_LOG, GATEWAY_PID, GATEWAY_READY],
    );
    // Halted at the moment the node has the serve command and is still opening
    // its listener. The node's `serving` and `unserved` come one after the
    // other, so the enable only sees the first when the unserve waited for it.
    let halting = async {
        flow.until("node asked to serve", |f| {
            f.order().contains(&"net:serve".to_string())
        })
        .await;
        flow.state.manager.fail("companion-timeout");
    };
    let (enabled, ()) = tokio::join!(flow.enable(), halting);
    // The halt ended the gateway while the node was serving: not an enabled
    // Companion, and the caller's own teardown follows the halt's.
    assert_eq!(enabled.unwrap_err(), "companion-timeout");
    let mut order = settled(&flow, "gw:dead-at-node-shutdown").await;
    order[10] = "<gateway>".into();
    assert_eq!(
        order[5..],
        [
            "net:serve",
            "net:unserve",
            "gw:alive-at-unserve",
            "net:unserved",
            // The enable's own teardown asks again; the halt may or may not
            // have stopped the gateway by then.
            "net:unserve",
            "<gateway>",
            "net:unserved",
            "net:shutdown",
            "gw:dead-at-node-shutdown",
        ],
        "{order:?}"
    );
    never_forwards_to_a_free_port(&order);
}

#[tokio::test]
async fn companion_signing_in_again_after_a_key_expiry_leaves_no_failure_behind() {
    let flow = Flow::new(
        &[
            NET_INIT,
            NET_WRITES_STATE,
            NET_RUNNING,
            NET_CERT,
            NET_COMMANDS_KEY_EXPIRES,
        ],
        &[GATEWAY_INIT_LOG, GATEWAY_READY],
    );
    flow.enable().await.unwrap();
    let halted = Status::Failed {
        code: "companion-net-login-required".into(),
    };
    flow.until("gateway halted", |f| f.state.manager.status() == halted)
        .await;
    flow.until("node stopped and forgotten", |f| {
        f.state.net.overview().state == net::State::Stopped
            && Settings::load(&f.dir).unwrap().net.is_none()
    })
    .await;
    assert!(flow.state.manager.idle());

    // The user signs in again.
    let seen = flow.events.lock().unwrap().len();
    net_login(&flow.state, &flow.net_bin).unwrap();
    flow.until("signed in", |f| {
        let node = f.state.net.overview();
        node.logged_in && node.state == net::State::Stopped && node.mode.is_none()
    })
    .await;
    // The card reads a node that is signed in and stopped: no sign-in request,
    // no failure beside it.
    let shown = overview(&flow.state).unwrap();
    assert_eq!(shown.status, Status::Disabled);
    assert_eq!(shown.net.error, None);
    assert_eq!(Settings::load(&flow.dir).unwrap().net, Some(identity()));
    let events = flow.events.lock().unwrap()[seen..].to_vec();
    let states: Vec<_> = events
        .iter()
        .filter(|e| e["type"] == "status")
        .map(|e| e["status"]["state"].clone())
        .collect();
    assert_eq!(states, ["disabled"], "{events:#?}");
    // Nothing about the sign-in looks like a teardown either.
    assert_eq!(
        teardown_never_looks_like_an_enable(&events)["state"],
        "disabled"
    );
}

#[tokio::test]
async fn companion_enabling_again_after_a_runtime_failure_shows_progress_and_not_the_failure() {
    let flow = Flow::new(
        &[NET_INIT, NET_RUNNING, NET_CERT_SLOW, NET_COMMANDS],
        &[GATEWAY_INIT_LOG, GATEWAY_READY],
    );
    flow.enable().await.unwrap();
    flow.state.manager.fail("companion-timeout");
    let failed = Status::Failed {
        code: "companion-timeout".into(),
    };
    flow.until("everything stopped", |f| {
        f.state.manager.idle()
            && f.state.net.mode().is_none()
            && f.order().contains(&"net:shutdown".to_string())
    })
    .await;
    assert_eq!(flow.state.manager.status(), failed);

    // Enable again; the card is opened (reads the overview) all through it.
    let seen = flow.events.lock().unwrap().len();
    let mut samples = Vec::new();
    {
        let enable = flow.enable();
        tokio::pin!(enable);
        loop {
            tokio::select! {
                done = &mut enable => {
                    assert_eq!(done.unwrap(), Status::Running { port: 47110 });
                    break;
                }
                _ = tokio::time::sleep(Duration::from_millis(20)) => {
                    let shown = overview(&flow.state).unwrap();
                    samples.push((shown.status, shown.net.mode, shown.net.state));
                }
            }
        }
    }
    // While the node runs for an enable, the card reads an enable that has not
    // got far yet, never the failure it follows.
    let progress = samples
        .iter()
        .filter(|(status, mode, _)| {
            *mode == Some(net::Mode::Enable)
                && matches!(status, Status::Disabled | Status::Starting)
        })
        .count();
    assert!(progress >= 5, "{samples:?}");
    assert!(
        samples
            .iter()
            .all(|(status, _, _)| !matches!(status, Status::Failed { .. })),
        "{samples:?}"
    );
    // The same in what the card was sent.
    let events = flow.events.lock().unwrap()[seen..].to_vec();
    let (mut status, mut mode) = (Value::Null, Value::Null);
    for event in &events {
        match event["type"].as_str() {
            Some("status") => status = event["status"]["state"].clone(),
            Some("net") => mode = event["net"]["mode"].clone(),
            _ => continue,
        }
        assert!(
            !(status == "failed" && mode == "enable"),
            "a failure beside an enable: {events:#?}"
        );
    }
    assert!(events
        .iter()
        .all(|e| e["type"] != "status" || e["status"]["state"] != "failed"));
    disable(&flow.state).await;
}

#[tokio::test]
async fn companion_net_login_remembers_the_node_and_lets_the_sidecar_go() {
    let flow = Flow::new(
        &[
            NET_INIT,
            NET_NEEDS_LOGIN,
            NET_LOGIN_LATER,
            NET_RUNNING,
            NET_COMMANDS,
        ],
        &[GATEWAY_INIT_LOG, GATEWAY_READY],
    );
    // Not while a gateway runs or starts.
    flow.state
        .manager
        .start(Launch {
            program: flow.gateway_bin.clone(),
            init: json!({}),
        })
        .unwrap();
    assert_eq!(
        net_login(&flow.state, &flow.net_bin).unwrap_err(),
        "companion-busy"
    );
    assert_eq!(
        net_logout(&flow.state, flow.net_program())
            .await
            .unwrap_err(),
        "companion-busy"
    );
    flow.state.manager.stop().await;
    let _ = std::fs::remove_file(&flow.log);

    let started = net_login(&flow.state, &flow.net_bin).unwrap();
    assert_eq!(started.state, net::State::Starting);
    flow.until("login url", |f| {
        f.events.lock().unwrap().iter().any(|e| {
            e["type"] == "net"
                && e["net"]["state"] == "needsLogin"
                && e["net"]["authUrl"] == "https://login.tailscale.com/a/abc123"
        })
    })
    .await;
    flow.until("node remembered", |f| {
        Settings::load(&f.dir).unwrap().net == Some(identity())
    })
    .await;
    flow.until("sidecar stopped", |f| {
        f.state.net.overview().state == net::State::Stopped
    })
    .await;
    assert_eq!(flow.order(), ["net:init", "net:running", "net:shutdown"]);
    // It stays known while offline.
    let offline = flow.state.net.overview();
    assert_eq!(offline.dns_name.as_deref(), Some("desktop.tail1234.ts.net"));
}

#[tokio::test]
async fn companion_net_cancel_stops_a_login_but_not_an_enable() {
    let flow = Flow::new(
        &[NET_INIT, NET_NEEDS_LOGIN, NET_COMMANDS],
        &[GATEWAY_INIT_LOG, GATEWAY_READY],
    );
    net_login(&flow.state, &flow.net_bin).unwrap();
    flow.until("login url", |f| {
        f.state.net.overview().state == net::State::NeedsLogin
    })
    .await;
    let cancelled = net_cancel(&flow.state).await.unwrap();
    assert_eq!(cancelled.state, net::State::Stopped);
    assert_eq!(flow.order(), ["net:init", "net:shutdown"]);

    // An enable that took the node over is not the login's to cancel.
    flow.state
        .net
        .start(&flow.net_bin, net::Mode::Enable)
        .unwrap();
    assert_eq!(
        net_cancel(&flow.state).await.unwrap().state,
        net::State::Starting
    );
    assert_eq!(flow.state.net.mode(), Some(net::Mode::Enable));
    flow.state.net.stop().await;
}

#[tokio::test]
async fn companion_net_logout_forgets_the_node() {
    let flow = Flow::registered(
        &[NET_INIT, NET_RUNNING, NET_COMMANDS],
        &[GATEWAY_INIT_LOG, GATEWAY_READY],
    );
    let state_dir = flow.dir.join("net");
    let after = net_logout(&flow.state, flow.net_program()).await.unwrap();
    assert_eq!(flow.order(), ["net:init", "net:running", "net:logout"]);
    assert!(!state_dir.exists());
    assert_eq!(Settings::load(&flow.dir).unwrap().net, None);
    assert_eq!(
        (after.logged_in, after.dns_name, after.login),
        (false, None, None)
    );

    // A failed logout keeps the node and what remembers it.
    let flow = Flow::registered(
        &[NET_INIT, NET_RUNNING, NET_COMMANDS_LOGOUT_FAILS],
        &[GATEWAY_INIT_LOG, GATEWAY_READY],
    );
    let state_dir = flow.dir.join("net");
    assert_eq!(
        net_logout(&flow.state, flow.net_program())
            .await
            .unwrap_err(),
        "companion-net-logout-failed"
    );
    assert!(state_dir.join("tailscaled.state").exists());
    assert_eq!(Settings::load(&flow.dir).unwrap().net, Some(identity()));
    assert_eq!(flow.state.net.mode(), None);
}

// tsnet writes its state file as soon as it starts, before any login.
const NET_WRITES_STATE: &str = r#"echo '{}' > "$(dirname "$0")/companion/net/tailscaled.state""#;

#[tokio::test]
async fn companion_net_cancelled_first_login_is_signed_out_and_can_sign_in_again() {
    let flow = Flow::new(
        &[NET_INIT, NET_WRITES_STATE, NET_NEEDS_LOGIN, NET_COMMANDS],
        &[GATEWAY_INIT_LOG, GATEWAY_READY],
    );
    let state_file = flow.dir.join("net/tailscaled.state");
    net_login(&flow.state, &flow.net_bin).unwrap();
    flow.until("login url", |f| {
        f.state.net.overview().state == net::State::NeedsLogin
    })
    .await;
    assert!(state_file.exists());
    assert_eq!(flow.state.net.overview().mode, Some(net::Mode::Login));

    // Cancelled in the browser: the card offers to sign in again.
    let cancelled = net_cancel(&flow.state).await.unwrap();
    assert_eq!(
        (cancelled.state, cancelled.mode, cancelled.logged_in),
        (net::State::Stopped, None, false)
    );
    assert!(state_file.exists(), "tsnet's leftover");
    assert_eq!(Settings::load(&flow.dir).unwrap().net, None);
    let shown = overview(&flow.state).unwrap();
    assert!(!shown.net.logged_in);

    // Sign out clears the leftover; there is no node to start for it.
    let after = net_logout(&flow.state, flow.net_program()).await.unwrap();
    assert!(!flow.dir.join("net").exists());
    assert_eq!(flow.order(), ["net:init", "net:shutdown"]);
    assert!(!after.logged_in);
}

#[tokio::test]
async fn companion_net_expired_key_forgets_the_node() {
    let flow = Flow::registered(
        &[NET_INIT, NET_NEEDS_LOGIN, NET_COMMANDS],
        &[GATEWAY_INIT_LOG, GATEWAY_READY],
    );
    assert!(flow.state.net.overview().logged_in);
    assert_eq!(
        flow.enable().await.unwrap_err(),
        "companion-net-login-required"
    );
    // The node says it has to log in: it is not signed in, and settings agree.
    flow.until("settings forget the node", |f| {
        Settings::load(&f.dir).unwrap().net.is_none()
    })
    .await;
    let shown = overview(&flow.state).unwrap();
    assert!(!shown.net.logged_in);
    assert_eq!(shown.net.dns_name, None);

    // Sign out has nothing to end on the tailnet: only the state goes.
    let order = flow.order();
    net_logout(&flow.state, flow.net_program()).await.unwrap();
    assert!(!flow.dir.join("net").exists());
    assert_eq!(flow.order(), order, "no sidecar was started");
}

#[tokio::test]
async fn companion_net_sign_out_without_the_sidecar_deletes_what_is_local() {
    let missing = || -> Result<PathBuf, String> { Err("companion-net-unavailable".into()) };
    // A node that never logged in: only tsnet's leftover, nothing to ask.
    let never = Flow::new(
        &[NET_INIT, NET_COMMANDS],
        &[GATEWAY_INIT_LOG, GATEWAY_READY],
    );
    std::fs::create_dir_all(never.dir.join("net")).unwrap();
    std::fs::write(never.dir.join("net/tailscaled.state"), "x").unwrap();
    net_logout(&never.state, missing).await.unwrap();
    assert!(!never.dir.join("net").exists());

    // A node whose key expired: settings forgot it, the state is still there.
    let expired = Flow::registered(
        &[NET_INIT, NET_NEEDS_LOGIN, NET_COMMANDS],
        &[GATEWAY_INIT_LOG, GATEWAY_READY],
    );
    assert_eq!(
        expired.enable().await.unwrap_err(),
        "companion-net-login-required"
    );
    expired
        .until("settings forget the node", |f| {
            Settings::load(&f.dir).unwrap().net.is_none()
        })
        .await;
    let after = net_logout(&expired.state, missing).await.unwrap();
    assert!(!expired.dir.join("net").exists());
    assert!(!after.logged_in);

    // A node that is registered has to be asked: without the sidecar, that is
    // the error, and nothing is deleted.
    let registered = Flow::registered(
        &[NET_INIT, NET_RUNNING, NET_COMMANDS],
        &[GATEWAY_INIT_LOG, GATEWAY_READY],
    );
    assert_eq!(
        net_logout(&registered.state, missing).await.unwrap_err(),
        "companion-net-unavailable"
    );
    assert!(registered.dir.join("net/tailscaled.state").exists());
    assert_eq!(
        Settings::load(&registered.dir).unwrap().net,
        Some(identity())
    );
}

// First run: tsnet registers and the machine waits for approval. With its
// state kept, the node starts straight in that wait, without a sign-in URL
// (what the control server does for a key it already knows).
const NET_REGISTERS_THEN_WAITS: &str = r#"state="$(dirname "$0")/companion/net/tailscaled.state"
if [ -f "$state" ]; then
  echo net:registered >> @LOG@
else
  echo net:fresh >> @LOG@
  echo '{}' > "$state"
  echo '{"v":1,"type":"state","backendState":"NeedsLogin"}'
  echo '{"v":1,"type":"login-required","authUrl":"https://login.tailscale.com/a/abc123"}'
  sleep 0.1
fi
echo '{"v":1,"type":"state","backendState":"NeedsMachineAuth","dnsName":"yuzora-1.tail1234.ts.net"}'"#;

#[tokio::test]
async fn companion_net_a_pending_approval_can_be_left_by_signing_out() {
    let nothing_to_ask = || -> Result<PathBuf, String> { Err("companion-net-unavailable".into()) };
    let flow = Flow::new(
        &[NET_INIT, NET_REGISTERS_THEN_WAITS, NET_COMMANDS],
        &[GATEWAY_INIT_LOG, GATEWAY_READY],
    );
    let state_file = flow.dir.join("net/tailscaled.state");

    // The sign-in is waiting for an admin: the card names that machine.
    net_login(&flow.state, &flow.net_bin).unwrap();
    flow.until("waiting for approval", |f| {
        f.state.net.overview().state == net::State::NeedsApproval
    })
    .await;
    let waiting = overview(&flow.state).unwrap();
    assert_eq!(
        (waiting.net.mode, waiting.net.logged_in),
        (Some(net::Mode::Login), false)
    );
    assert_eq!(
        waiting.net.dns_name.as_deref(),
        Some("yuzora-1.tail1234.ts.net")
    );
    // Not an identity: settings do not remember the pending machine.
    assert_eq!(Settings::load(&flow.dir).unwrap().net, None);
    assert!(state_file.exists());

    // Signing out while it waits stops that node first; there is nobody to ask.
    let after = net_logout(&flow.state, nothing_to_ask).await.unwrap();
    assert_eq!(flow.order(), ["net:init", "net:fresh", "net:shutdown"]);
    assert!(!flow.dir.join("net").exists());
    assert_eq!(Settings::load(&flow.dir).unwrap().net, None);
    assert_eq!(
        (after.state, after.mode, after.logged_in, after.dns_name),
        (net::State::Stopped, None, false, None)
    );

    // Signing in again registers afresh: a new sign-in URL, not the wait again.
    net_login(&flow.state, &flow.net_bin).unwrap();
    flow.until("a new sign-in URL", |f| {
        f.state.net.overview().auth_url.as_deref() == Some("https://login.tailscale.com/a/abc123")
    })
    .await;
    assert_eq!(flow.order().last().map(String::as_str), Some("net:fresh"));
    net_cancel(&flow.state).await.unwrap();
    // Cancelled: the registration is still there, and signing in would only
    // return to the wait (the state the card could not leave).
    net_login(&flow.state, &flow.net_bin).unwrap();
    flow.until("the wait again", |f| {
        f.state.net.overview().state == net::State::NeedsApproval
    })
    .await;
    assert_eq!(flow.state.net.overview().auth_url, None);
    assert_eq!(
        flow.order().last().map(String::as_str),
        Some("net:registered")
    );

    // Stopped, with the registration still on disk: signing out clears it too.
    net_cancel(&flow.state).await.unwrap();
    assert!(state_file.exists());
    assert_eq!(flow.state.net.overview().state, net::State::Stopped);
    let after = net_logout(&flow.state, nothing_to_ask).await.unwrap();
    assert!(!flow.dir.join("net").exists());
    assert_eq!(Settings::load(&flow.dir).unwrap().net, None);
    assert_eq!(
        (after.state, after.mode, after.logged_in),
        (net::State::Stopped, None, false)
    );
}

#[tokio::test]
async fn companion_net_a_node_whose_approval_was_taken_back_can_sign_out() {
    // Remembered, registered, and the admin has since removed its approval: it
    // starts into the wait. There is no login to end, and nothing to wait for.
    let flow = Flow::registered(
        &[NET_INIT, NET_NEEDS_APPROVAL, NET_COMMANDS],
        &[GATEWAY_INIT_LOG, GATEWAY_READY],
    );
    assert!(flow.state.net.overview().logged_in);
    let started = std::time::Instant::now();
    let after = net_logout(&flow.state, flow.net_program()).await.unwrap();
    assert!(
        started.elapsed() < flow_timing().running / 2,
        "waited for a node that never runs: {:?}",
        started.elapsed()
    );
    assert_eq!(flow.order(), ["net:init", "net:shutdown"], "no logout sent");
    assert!(!flow.dir.join("net").exists());
    assert_eq!(Settings::load(&flow.dir).unwrap().net, None);
    assert_eq!(
        (after.state, after.mode, after.logged_in, after.dns_name),
        (net::State::Stopped, None, false, None)
    );
}

#[tokio::test]
async fn companion_net_a_node_seen_running_is_remembered_whatever_the_mode() {
    // It logged in during an enable that then failed: it is registered all the same.
    let flow = Flow::with_timing(
        &[NET_INIT, NET_RUNNING, NET_CERT_ERROR, NET_COMMANDS],
        &[GATEWAY_INIT_LOG, GATEWAY_READY],
        net::Timing {
            cert: Duration::from_millis(400),
            ..flow_timing()
        },
    );
    assert_eq!(
        flow.enable().await.unwrap_err(),
        "companion-net-cert-failed"
    );
    flow.until("settings remember the node", |f| {
        Settings::load(&f.dir).unwrap().net == Some(identity())
    })
    .await;
    assert_eq!(flow.state.net.identity(), Some(identity()));
}

fn mapped(session: &str, workspace: &str, path: &str) -> SpaceRoot {
    SpaceRoot {
        session: session.into(),
        workspace_id: workspace.into(),
        root: path.into(),
    }
}

#[test]
fn companion_space_roots_are_checked_like_the_gateway_checks_them() {
    let ok = |roots: &[SpaceRoot]| valid_space_roots(roots).is_ok();
    assert!(ok(&[]));
    assert!(ok(&[
        mapped("default", "w1", "/repo"),
        mapped("default", "w2", "/repo")
    ]));
    assert!(ok(&[
        mapped("default", "w1", "/repo"),
        mapped("work", "w1", "/repo")
    ]));
    let many = |n: usize| -> Vec<SpaceRoot> {
        (0..n)
            .map(|i| mapped("default", &format!("w{i}"), "/repo"))
            .collect()
    };
    assert!(ok(&many(512)));
    assert!(!ok(&many(513)));
    for bad in [
        mapped("", "w1", "/repo"),
        mapped("default", "", "/repo"),
        mapped("default", "w1", ""),
        mapped("default", "w1", "repo"),
        mapped("default", "w1", "./repo"),
        mapped(&"s".repeat(257), "w1", "/repo"),
        mapped("default", &"w".repeat(257), "/repo"),
        mapped("default", "w1", &format!("/{}", "r".repeat(4096))),
    ] {
        assert_eq!(
            valid_space_roots(&[bad]).unwrap_err(),
            "companion-space-roots-invalid"
        );
    }
    // Init would refuse the same Space twice.
    assert!(!ok(&[
        mapped("default", "w1", "/a"),
        mapped("default", "w1", "/b")
    ]));
    assert!(ok(&[mapped(
        &"s".repeat(256),
        &"w".repeat(256),
        &format!("/{}", "r".repeat(4095))
    )]));
}

#[test]
fn companion_space_roots_wire_names_are_camel_case_and_closed() {
    let roots: Vec<SpaceRoot> = serde_json::from_value(
        json!([{"session": "default", "workspaceId": "w1", "root": "/repo"}]),
    )
    .unwrap();
    assert_eq!(roots, [mapped("default", "w1", "/repo")]);
    assert_eq!(
        roots_frame(&roots),
        json!({"v": 1, "type": "spaces.roots", "roots": [{"session": "default", "workspaceId": "w1", "root": "/repo"}]})
    );
    assert!(serde_json::from_value::<SpaceRoot>(
        json!({"session": "d", "workspaceId": "w", "root": "/r", "extra": 1})
    )
    .is_err());
}

#[tokio::test]
async fn companion_space_roots_reach_init_and_the_running_gateway() {
    let flow = Flow::new(
        &[NET_INIT, NET_RUNNING, NET_CERT, NET_COMMANDS],
        &[GATEWAY_INIT_LOG, GATEWAY_READY_LATE],
    );
    let first = vec![mapped("default", "w1", "/repo/one")];
    space_roots_set(&flow.state, first.clone()).unwrap();
    // A rejected set changes nothing.
    assert!(space_roots_set(&flow.state, vec![mapped("default", "w1", "relative")]).is_err());
    assert_eq!(*flow.state.roots.lock().unwrap(), first);

    // A set made while the gateway starts is not lost: it was not in the init
    // frame, and nothing runs yet to be told.
    let second = vec![
        mapped("default", "w1", "/repo/one"),
        mapped("default", "w2", "/repo/two"),
    ];
    let (status, ()) = tokio::join!(flow.enable(), async {
        flow.until("gateway starting", |f| {
            f.state.manager.status() == Status::Starting
        })
        .await;
        assert!(flow.gateway_input().is_empty());
        space_roots_set(&flow.state, second.clone()).unwrap();
    });
    status.unwrap();
    let init = flow.json(&flow.gateway_bin, "init");
    assert_eq!(
        init["spaceRoots"],
        json!([{"session": "default", "workspaceId": "w1", "root": "/repo/one"}])
    );
    flow.until("roots reach the gateway", |f| !f.gateway_input().is_empty())
        .await;
    assert_eq!(flow.gateway_input(), [roots_frame(&second)]);

    // And one made while it runs is sent at once.
    let third = vec![mapped("default", "w3", "/repo/three")];
    space_roots_set(&flow.state, third.clone()).unwrap();
    flow.until("roots change", |f| f.gateway_input().len() == 2)
        .await;
    assert_eq!(flow.gateway_input()[1], roots_frame(&third));

    // After a disable the next init frame has the last ones.
    disable(&flow.state).await;
    flow.enable().await.unwrap();
    let init = flow.json(&flow.gateway_bin, "init");
    assert_eq!(
        init["spaceRoots"],
        json!([{"session": "default", "workspaceId": "w3", "root": "/repo/three"}])
    );
    assert_eq!(
        init["trustStore"],
        json!(yuzora_host::workspace_trust::default_store_path())
    );
    disable(&flow.state).await;
}

#[tokio::test]
async fn companion_space_roots_of_another_instance_are_not_ours_to_set() {
    let flow = Flow::new(
        &[NET_INIT, NET_RUNNING, NET_CERT, NET_COMMANDS],
        &[GATEWAY_INIT_LOG, GATEWAY_READY],
    );
    let other = CompanionState::with_emit(
        Arc::new(|_| {}),
        "9.9.9".into(),
        flow_timing(),
        flow.dir.clone(),
        None,
        flow.wire.push(&flow.dir),
    );
    assert!(other.owner.is_none());
    // No error, even for what would be one, and nothing kept.
    space_roots_set(&other, vec![mapped("default", "w1", "relative")]).unwrap();
    space_roots_set(&other, vec![mapped("default", "w1", "/repo")]).unwrap();
    assert!(other.roots.lock().unwrap().is_empty());
}

#[tokio::test]
async fn companion_enable_gives_the_gateway_the_push_key_and_the_subscribed_devices() {
    let flow = Flow::new(
        &[NET_INIT, NET_RUNNING, NET_CERT, NET_COMMANDS],
        &[GATEWAY_INIT_LOG, GATEWAY_READY],
    );
    push_tests::seed(&flow.dir, &[push_tests::DEVICE_ID]);
    let push = flow.state.manager.push();
    assert!(push.init().is_object());
    assert!(push.subscription(push_tests::DEVICE_ID, &push_tests::subscription(1)));
    flow.enable().await.unwrap();
    let init = flow.json(&flow.gateway_bin, "init");
    let key = init["push"]["vapidPublicKey"].as_str().unwrap();
    assert_eq!(key.len(), 87);
    assert_eq!(
        init["push"]["devices"],
        json!([{"deviceId": push_tests::DEVICE_ID, "endpoints": [crate::companion_push::endpoint_id(&push_tests::endpoint(1))]}])
    );
    assert_eq!(
        init["push"].as_object().unwrap().len(),
        2,
        "only the key and the devices"
    );
    // Ids only: the endpoints are capabilities.
    assert!(!init.to_string().contains("fcm.googleapis.com"));
    let shown = overview(&flow.state).unwrap();
    assert!(shown.push.available);
    assert_eq!(shown.push.subscribed, [push_tests::DEVICE_ID]);
    // The private half stays in the credential store.
    assert!(!serde_json::to_string(&init)
        .unwrap()
        .contains(flow.wire.vault.secret.lock().unwrap().as_deref().unwrap()));
    disable(&flow.state).await;
}

#[tokio::test]
async fn companion_enable_with_a_new_push_key_starts_without_the_old_subscriptions() {
    let flow = Flow::new(
        &[NET_INIT, NET_RUNNING, NET_CERT, NET_COMMANDS],
        &[GATEWAY_INIT_LOG, GATEWAY_READY],
    );
    push_tests::seed(&flow.dir, &[push_tests::DEVICE_ID]);
    // Made for a key the credential store no longer has: it is empty.
    let push = flow.state.manager.push();
    assert!(push.subscription(push_tests::DEVICE_ID, &push_tests::subscription(1)));
    flow.enable().await.unwrap();
    let init = flow.json(&flow.gateway_bin, "init");
    assert_eq!(init["push"]["devices"], json!([]));
    assert!(push.devices().is_empty());
    // The UI is told too, not only the gateway.
    assert!(flow
        .events
        .lock()
        .unwrap()
        .iter()
        .any(|event| event
            == &json!({"type": "push", "push": {"available": true, "subscribed": []}})));
    disable(&flow.state).await;
}

#[tokio::test]
async fn companion_enable_without_a_key_vault_runs_without_push() {
    let flow = Flow::new(
        &[NET_INIT, NET_RUNNING, NET_CERT, NET_COMMANDS],
        &[GATEWAY_INIT_LOG, GATEWAY_READY],
    );
    *flow.wire.vault.fail.lock().unwrap() = true;
    assert_eq!(
        flow.enable().await.unwrap(),
        Status::Running { port: 47110 }
    );
    assert_eq!(flow.json(&flow.gateway_bin, "init")["push"], Value::Null);
    let shown = overview(&flow.state).unwrap();
    assert_eq!(
        (shown.push.available, shown.push.subscribed.len()),
        (false, 0)
    );
    disable(&flow.state).await;
}
