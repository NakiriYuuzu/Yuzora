//! Fake sidecars are shell scripts speaking the admin NDJSON protocol.
use super::*;
use crate::companion_push::tests::{self as push_tests, Wire};
use std::path::Path;

const DEVICE: &str = include_str!("../../companion/tests/fixtures/device.json");
const READY: &str = r#"{"v":1,"type":"ready","ports":{"loopback":47110,"lan":null}}"#;

struct Harness {
    _root: tempfile::TempDir,
    dir: PathBuf,
    bin: PathBuf,
    manager: Arc<CompanionManager>,
    events: Arc<Mutex<Vec<Value>>>,
    stopped: Arc<std::sync::atomic::AtomicUsize>,
    wire: Wire,
}
impl Harness {
    fn new(script: &str) -> Self {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("companion");
        let bin = root.path().join("sidecar");
        std::fs::write(&bin, format!("#!/bin/sh\n{script}\n")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let events = Arc::new(Mutex::new(Vec::new()));
        let stopped = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let sink = events.clone();
        let count = stopped.clone();
        let wire = push_tests::wire();
        let manager = Arc::new(CompanionManager {
            dir: dir.clone(),
            emit: Arc::new(move |event| sink.lock().unwrap().push(event)),
            stopped: Arc::new(move || {
                count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }),
            closing: OnceLock::new(),
            push: wire.push(&dir),
            inner: Mutex::default(),
            store: Mutex::new(None),
            commands: tokio::sync::Mutex::new(()),
            timing: Timing {
                command: Duration::from_millis(500),
                stop: Duration::from_millis(500),
                ready: Duration::from_millis(1000),
            },
        });
        Self {
            _root: root,
            dir,
            bin,
            manager,
            events,
            stopped,
            wire,
        }
    }
    fn launch(&self) -> Launch {
        Launch {
            program: self.bin.clone(),
            init: json!({"v": 1, "type": "init", "marker": "init-frame"}),
        }
    }
    async fn until(&self, what: &str, done: impl Fn(&Self) -> bool) {
        for _ in 0..500 {
            if done(self) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("timed out waiting for {what}: {:?}", self.manager.status());
    }
    fn stops(&self) -> usize {
        self.stopped.load(std::sync::atomic::Ordering::SeqCst)
    }
    fn sidecar_file(&self, suffix: &str) -> String {
        std::fs::read_to_string(format!("{}.{suffix}", self.bin.display())).unwrap_or_default()
    }
}
fn running(h: &Harness) -> bool {
    matches!(h.manager.status(), Status::Running { .. })
}

#[tokio::test]
async fn companion_service_start_relays_events_and_stops_on_request() {
    let h = Harness::new(&format!(
        r#"read init; printf '%s\n' "$init" > "$0.init"; env > "$0.env"
echo '{READY}'
echo '{{"v":1,"type":"session.opened","peerId":1,"deviceId":"AAAAAAAAAAAAAAAAAAAAAA"}}'
while read line; do
  case "$line" in *'"shutdown"'*) echo shutdown > "$0.stopped"; exit 0;; esac
done"#
    ));
    let mut stored = Devices::load(&h.dir).unwrap();
    stored
        .upsert(serde_json::from_str(DEVICE).unwrap())
        .unwrap();
    h.manager.start(h.launch()).unwrap();
    assert_eq!(h.manager.until_ready().await, Ok(47110));
    assert_eq!(h.manager.status(), Status::Running { port: 47110 });
    let init: Value = serde_json::from_str(&h.sidecar_file("init")).unwrap();
    assert_eq!(init["marker"], "init-frame");
    assert_eq!(
        init["devices"],
        json!([serde_json::from_str::<Value>(DEVICE).unwrap()])
    );
    // Only allowlisted variables reach the gateway, and the ones it is given;
    // sh adds PWD, SHLVL and _.
    let env = h.sidecar_file("env");
    for line in env.lines() {
        let key = line.split('=').next().unwrap();
        assert!(
            ENV_ALLOWLIST.contains(&key)
                || GATEWAY_GIT_ENV.iter().any(|(given, _)| *given == key)
                || ["PWD", "SHLVL", "_", "OLDPWD"].contains(&key),
            "leaked {key}"
        );
    }
    // Never inherited, always set: a phone's git read must not fetch from a remote.
    for (key, _) in GATEWAY_GIT_ENV {
        assert!(!ENV_ALLOWLIST.contains(key), "{key} would be inherited");
    }
    let given = |name: &str| {
        env.lines()
            .filter(|line| line.starts_with(&format!("{name}=")))
            .collect::<Vec<_>>()
    };
    assert_eq!(given("GIT_NO_LAZY_FETCH"), ["GIT_NO_LAZY_FETCH=1"]);
    // Empty, but set: git before 2.44 refuses every transport with it.
    assert_eq!(given("GIT_ALLOW_PROTOCOL"), ["GIT_ALLOW_PROTOCOL="]);
    h.until("session event", |h| {
        h.events
            .lock()
            .unwrap()
            .iter()
            .any(|e| e["type"] == "session.opened")
    })
    .await;
    h.manager.stop().await;
    h.until("disabled", |h| h.manager.status() == Status::Disabled)
        .await;
    assert_eq!(h.sidecar_file("stopped").trim(), "shutdown");
    assert_eq!(h.stops(), 0, "a requested stop does its own cleanup");
    assert!(h.manager.start(h.launch()).is_ok(), "can start again");
    h.manager.stop().await;
}

#[tokio::test]
async fn companion_service_pairing_reply_is_returned_not_emitted() {
    let h = Harness::new(&format!(
        r#"read init; echo '{READY}'
while read line; do
  case "$line" in
    *'"pairing.create"'*) echo '{{"v":1,"type":"pairing.created","pairingId":"P1","url":"https://desktop.tail1234.ts.net:8443/pair#p=P1.SECRET","expiresAt":"2030-01-01T00:05:00Z"}}';;
    *'"pairing.approve"'*) echo '{{"v":1,"type":"pairing.state","pairingId":"P1","state":"approved","reason":"approved"}}';;
    *'"shutdown"'*) exit 0;;
  esac
done"#
    ));
    h.manager.start(h.launch()).unwrap();
    h.until("ready", running).await;
    let created = h.manager.pairing_create(None).await.unwrap();
    assert_eq!(created["pairingId"], "P1");
    assert!(created["url"].as_str().unwrap().contains("SECRET"));
    let decided = h
        .manager
        .pairing_decide("pairing.approve", "P1")
        .await
        .unwrap();
    assert_eq!(decided["state"], "approved");
    h.until("pairing state", |h| {
        h.events
            .lock()
            .unwrap()
            .iter()
            .any(|e| e["type"] == "pairing.state")
    })
    .await;
    let emitted = serde_json::to_string(&*h.events.lock().unwrap()).unwrap();
    assert!(!emitted.contains("SECRET") && !emitted.contains("pairing.created"));
    h.manager.stop().await;
}

#[tokio::test]
async fn companion_service_pairing_decisions_queue_with_other_commands() {
    // The rejection's error belongs to the decision, never to the pairing
    // request queued behind it.
    let h = Harness::new(&format!(
        r#"read init; echo '{READY}'
while read line; do
  case "$line" in
    *'"pairing.reject"'*) sleep 0.2; echo '{{"v":1,"type":"error","code":"admin-command-rejected","message":"Companion request rejected"}}';;
    *'"pairing.cancel"'*) echo '{{"v":1,"type":"command.deferred","command":"pairing.cancel","target":"P2","committed":true}}';;
    *'"pairing.approve"'*) echo '{{"v":1,"type":"pairing.state","pairingId":"P4","state":"expired","reason":"ttl"}}'
      echo '{{"v":1,"type":"error","code":"admin-command-rejected","message":"Companion request rejected"}}';;
    *'"pairing.create"'*) echo '{{"v":1,"type":"pairing.created","pairingId":"P3","url":"https://h/pair#p=P3.S","expiresAt":"2030-01-01T00:05:00Z"}}';;
    *'"shutdown"'*) exit 0;;
  esac
done"#
    ));
    h.manager.start(h.launch()).unwrap();
    h.until("ready", running).await;
    let (rejected, created) =
        tokio::join!(h.manager.pairing_decide("pairing.reject", "P1"), async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            h.manager.pairing_create(None).await
        });
    assert_eq!(rejected.unwrap_err(), "admin-command-rejected");
    assert_eq!(created.unwrap()["pairingId"], "P3");
    let deferred = h
        .manager
        .pairing_decide("pairing.cancel", "P2")
        .await
        .unwrap();
    assert_eq!(deferred["type"], "command.deferred");
    // Expired while it was being decided: the approve failed.
    assert_eq!(
        h.manager
            .pairing_decide("pairing.approve", "P4")
            .await
            .unwrap_err(),
        "admin-command-rejected"
    );
    assert!(h
        .events
        .lock()
        .unwrap()
        .iter()
        .any(|e| e["type"] == "pairing.state" && e["state"] == "expired"));
    h.manager.stop().await;
}

#[tokio::test]
async fn companion_service_device_changes_persist_before_they_count() {
    let h = Harness::new(&format!(
        r#"read init; echo '{READY}'
echo '{{"v":1,"type":"device.registered","device":{device}}}'
while read line; do
  case "$line" in
    *'"device.revoke"'*) echo '{{"v":1,"type":"device.revoked","deviceId":"AAAAAAAAAAAAAAAAAAAAAA"}}';;
    *'"device.unlock"'*) echo '{{"v":1,"type":"error","code":"admin-command-rejected","message":"Companion request rejected"}}';;
    *'"shutdown"'*) exit 0;;
  esac
done"#,
        device = DEVICE.trim()
    ));
    h.manager.start(h.launch()).unwrap();
    h.until("registered", |h| h.manager.devices().unwrap().len() == 1)
        .await;
    // The UI sees no credential material.
    let summary = &h.manager.devices().unwrap()[0];
    assert_eq!(summary["label"], "Test phone");
    assert!(summary.get("credentials").is_none());
    let emitted = h.events.lock().unwrap().clone();
    let registered = emitted
        .iter()
        .find(|e| e["type"] == "device.registered")
        .unwrap();
    assert!(registered["device"].get("credentials").is_none());
    assert_eq!(
        h.manager
            .device_command(json!({"type": "device.unlock", "deviceId": "AAAAAAAAAAAAAAAAAAAAAA"}))
            .await
            .unwrap_err(),
        "admin-command-rejected"
    );
    let revoked = h
        .manager
        .device_command(json!({"type": "device.revoke", "deviceId": "AAAAAAAAAAAAAAAAAAAAAA"}))
        .await
        .unwrap();
    assert_eq!(revoked["type"], "device.revoked");
    assert!(Devices::load(&h.dir).unwrap().all().is_empty());
    h.manager.stop().await;
}

#[tokio::test]
async fn companion_service_deferred_changes_are_persisted() {
    // A deferred change is already live in the gateway; only its event is late.
    let h = Harness::new(&format!(
        r#"read init; echo '{READY}'
while read line; do
  case "$line" in
    *'"device.update"'*) echo '{{"v":1,"type":"command.deferred","command":"device.update","target":"AAAAAAAAAAAAAAAAAAAAAA","committed":true}}';;
    *'"device.revoke"'*) echo '{{"v":1,"type":"command.deferred","command":"device.revoke","target":"AAAAAAAAAAAAAAAAAAAAAA","committed":true}}';;
    *'"shutdown"'*) exit 0;;
  esac
done"#
    ));
    Devices::load(&h.dir)
        .unwrap()
        .upsert(serde_json::from_str(DEVICE).unwrap())
        .unwrap();
    h.manager.start(h.launch()).unwrap();
    h.until("ready", running).await;
    let renamed = h
        .manager
        .device_command(json!({"type": "device.update", "deviceId": "AAAAAAAAAAAAAAAAAAAAAA", "label": "Renamed", "grants": null}))
        .await
        .unwrap();
    assert_eq!(renamed["type"], "command.deferred");
    let stored = Devices::load(&h.dir).unwrap().all().to_vec();
    assert_eq!(stored[0]["label"], "Renamed");
    assert_eq!(
        stored[0]["grants"],
        serde_json::from_str::<Value>(DEVICE).unwrap()["grants"]
    );
    h.manager
        .device_command(json!({"type": "device.revoke", "deviceId": "AAAAAAAAAAAAAAAAAAAAAA"}))
        .await
        .unwrap();
    // A stop now must not bring the device back at the next init.
    assert!(Devices::load(&h.dir).unwrap().all().is_empty());
    h.manager.stop().await;
}

#[tokio::test]
async fn companion_service_device_reply_waits_for_its_own_event() {
    // A late deferral of an earlier, timed-out update and a lastSeen touch for
    // the same device both arrive before the revoke lands.
    let h = Harness::new(&format!(
        r#"read init; echo '{READY}'
while read line; do
  case "$line" in
    *'"device.revoke"'*) echo '{{"v":1,"type":"command.deferred","command":"device.update","target":"AAAAAAAAAAAAAAAAAAAAAA","committed":true}}'
      echo '{{"v":1,"type":"device.updated","device":{touched}}}'; sleep 0.2
      echo '{{"v":1,"type":"device.revoked","deviceId":"AAAAAAAAAAAAAAAAAAAAAA"}}';;
    *'"shutdown"'*) exit 0;;
  esac
done"#,
        touched = DEVICE.trim()
    ));
    Devices::load(&h.dir)
        .unwrap()
        .upsert(serde_json::from_str(DEVICE).unwrap())
        .unwrap();
    h.manager.start(h.launch()).unwrap();
    h.until("ready", running).await;
    let revoked = h
        .manager
        .device_command(json!({"type": "device.revoke", "deviceId": "AAAAAAAAAAAAAAAAAAAAAA"}))
        .await
        .unwrap();
    assert_eq!(revoked["type"], "device.revoked");
    assert!(Devices::load(&h.dir).unwrap().all().is_empty());
    h.manager.stop().await;
}

#[tokio::test]
async fn companion_service_update_reply_carries_the_requested_change() {
    // A lastSeen touch queued before the rename must not complete it.
    let mut renamed: Value = serde_json::from_str(DEVICE).unwrap();
    renamed["label"] = json!("Renamed");
    let h = Harness::new(&format!(
        r#"read init; echo '{READY}'
while read line; do
  case "$line" in
    *'"device.update"'*) echo '{{"v":1,"type":"device.updated","device":{touched}}}'; sleep 0.2
      echo '{{"v":1,"type":"device.updated","device":{renamed}}}';;
    *'"shutdown"'*) exit 0;;
  esac
done"#,
        touched = DEVICE.trim()
    ));
    Devices::load(&h.dir)
        .unwrap()
        .upsert(serde_json::from_str(DEVICE).unwrap())
        .unwrap();
    h.manager.start(h.launch()).unwrap();
    h.until("ready", running).await;
    let reply = h
        .manager
        .device_command(json!({"type": "device.update", "deviceId": "AAAAAAAAAAAAAAAAAAAAAA", "label": "Renamed", "grants": null}))
        .await
        .unwrap();
    assert_eq!(reply["device"]["label"], "Renamed");
    assert_eq!(Devices::load(&h.dir).unwrap().all()[0]["label"], "Renamed");
    h.manager.stop().await;
}

#[test]
fn companion_service_snapshots_match_requests_by_value() {
    let device: Value = serde_json::from_str(DEVICE).unwrap();
    let update = |grants: Value| json!({"type": "device.update", "deviceId": "x", "label": null, "grants": grants});
    // The gateway re-serializes timestamps; the instant is what counts.
    assert!(reflects(
        &update(json!({"control": {"until": "2030-01-02T00:00:00+00:00"}, "full": true})),
        &device
    ));
    assert!(!reflects(
        &update(json!({"control": {"until": "2030-01-02T00:00:01Z"}, "full": true})),
        &device
    ));
    assert!(!reflects(
        &update(json!({"control": null, "full": true})),
        &device
    ));
    assert!(!reflects(
        &update(json!({"control": {"until": "2030-01-02T00:00:00Z"}, "full": false})),
        &device
    ));
    assert!(reflects(
        &json!({"type": "device.unlock", "deviceId": "x"}),
        &device
    ));
    let mut locked = device.clone();
    locked["locked"] = json!(true);
    assert!(!reflects(
        &json!({"type": "device.unlock", "deviceId": "x"}),
        &locked
    ));
}

#[tokio::test]
async fn companion_service_killed_gateway_output_cannot_undo_offline_edits() {
    // A child outlives the kill and still holds stdout; what it writes
    // afterwards must not bring back a device revoked while stopped.
    let h = Harness::new(&format!(
        r#"trap '' TERM; read init; echo '{READY}'
( sleep 1.5; echo '{{"v":1,"type":"device.updated","device":{device}}}' ) &
while :; do sleep 1; done"#,
        device = DEVICE.trim()
    ));
    Devices::load(&h.dir)
        .unwrap()
        .upsert(serde_json::from_str(DEVICE).unwrap())
        .unwrap();
    h.manager.start(h.launch()).unwrap();
    h.until("ready", running).await;
    h.manager.stop().await;
    assert!(h.manager.idle());
    h.manager
        .edit_stopped(&json!({"type": "device.revoke", "deviceId": "AAAAAAAAAAAAAAAAAAAAAA"}))
        .unwrap();
    tokio::time::sleep(Duration::from_millis(1800)).await;
    assert!(Devices::load(&h.dir).unwrap().all().is_empty());
}

#[tokio::test]
async fn companion_service_unpersisted_device_change_halts_the_gateway() {
    let h = Harness::new(&format!(
        r#"read init; echo '{READY}'
while read line; do
  case "$line" in
    *'"device.revoke"'*) echo '{{"v":1,"type":"device.revoked","deviceId":"AAAAAAAAAAAAAAAAAAAAAA"}}';;
  esac
done"#
    ));
    let mut stored = Devices::load(&h.dir).unwrap();
    stored
        .upsert(serde_json::from_str(DEVICE).unwrap())
        .unwrap();
    h.manager.start(h.launch()).unwrap();
    h.until("ready", running).await;
    std::fs::create_dir(h.dir.join("devices.json.tmp")).unwrap();
    assert!(h
        .manager
        .device_command(json!({"type": "device.revoke", "deviceId": "AAAAAAAAAAAAAAAAAAAAAA"}))
        .await
        .is_err());
    h.until("halted", |h| {
        matches!(h.manager.status(), Status::Failed { ref code } if code.starts_with("companion-store-write-failed"))
    })
    .await;
    h.until("stop hook", |h| h.stops() == 1).await;
}

#[tokio::test]
async fn companion_service_fail_halts_a_running_gateway_from_outside() {
    // The node in front of the gateway is gone.
    let h = Harness::new(&format!(
        r#"read init; echo '{READY}'; while read line; do :; done"#
    ));
    h.manager.fail("companion-net-crashed");
    assert_eq!(
        h.manager.status(),
        Status::Disabled,
        "nothing runs: nothing to halt"
    );
    h.manager.start(h.launch()).unwrap();
    h.until("ready", running).await;
    h.manager.fail("companion-net-crashed");
    let failed = Status::Failed {
        code: "companion-net-crashed".into(),
    };
    assert_eq!(h.manager.status(), failed);
    h.until("gateway gone", |h| h.manager.idle()).await;
    assert_eq!(h.manager.status(), failed);
    h.until("stop hook", |h| h.stops() == 1).await;
}

/// What the node does before the gateway may go: takes a while to stop
/// forwarding. `closed` is set when it is done, and is the only thing that
/// lets the gateway go.
fn closing_in(
    h: &Harness,
    took: Duration,
) -> (
    Arc<std::sync::atomic::AtomicBool>,
    Arc<std::sync::atomic::AtomicUsize>,
) {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    let (closed, asked) = (
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicUsize::new(0)),
    );
    let (done, count) = (closed.clone(), asked.clone());
    h.manager.on_closing(Arc::new(move || {
        let (done, count) = (done.clone(), count.clone());
        Box::pin(async move {
            count.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(took).await;
            done.store(true, Ordering::SeqCst);
        })
    }));
    (closed, asked)
}

/// After a halt the failure is published, and the gateway lives until the node
/// has stopped forwarding to it; only then is it killed.
async fn halted_gateway_outlives_the_closing(
    h: &Harness,
    closed: &std::sync::atomic::AtomicBool,
    code: &str,
) {
    use std::sync::atomic::Ordering;
    let failed = Status::Failed { code: code.into() };
    assert_eq!(h.manager.status(), failed);
    assert!(
        h.events
            .lock()
            .unwrap()
            .iter()
            .any(|e| e["type"] == "status" && e["status"]["code"] == code),
        "the failure was not published"
    );
    assert!(!h.manager.idle(), "{code}: killed before the node closed");
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while !closed.load(Ordering::SeqCst) {
        assert!(
            std::time::Instant::now() < deadline,
            "{code}: the node was never asked to close"
        );
        // `closed` is set before the kill: an idle gateway means it was.
        let idle = h.manager.idle();
        assert!(
            !idle || closed.load(Ordering::SeqCst),
            "{code}: killed before the node closed"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    h.until("gateway gone", |h| h.manager.idle()).await;
    assert_eq!(h.manager.status(), failed);
}

#[tokio::test]
async fn companion_service_a_failed_gateway_is_killed_only_after_the_node_stopped_forwarding() {
    let ready_loop = format!(r#"read init; echo '{READY}'; while read line; do :; done"#);

    // Failed from outside (the node's key expired, a crash).
    let h = Harness::new(&ready_loop);
    let (closed, asked) = closing_in(&h, Duration::from_millis(500));
    h.manager.start(h.launch()).unwrap();
    h.until("ready", running).await;
    h.manager.fail("companion-net-login-required");
    halted_gateway_outlives_the_closing(&h, &closed, "companion-net-login-required").await;
    assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 1);
    h.until("stop hook", |h| h.stops() == 1).await;

    // A command the gateway never answers.
    let h = Harness::new(&ready_loop);
    let (closed, _) = closing_in(&h, Duration::from_millis(500));
    h.manager.start(h.launch()).unwrap();
    h.until("ready", running).await;
    assert_eq!(
        h.manager.pairing_create(None).await.unwrap_err(),
        "companion-timeout"
    );
    halted_gateway_outlives_the_closing(&h, &closed, "companion-timeout").await;

    // A device change that could not be written down.
    let h = Harness::new(&format!(
        r#"read init; echo '{READY}'
while read line; do
  case "$line" in *'"device.revoke"'*) echo '{{"v":1,"type":"device.revoked","deviceId":"AAAAAAAAAAAAAAAAAAAAAA"}}';; esac
done"#
    ));
    let (closed, _) = closing_in(&h, Duration::from_millis(500));
    Devices::load(&h.dir)
        .unwrap()
        .upsert(serde_json::from_str(DEVICE).unwrap())
        .unwrap();
    h.manager.start(h.launch()).unwrap();
    h.until("ready", running).await;
    std::fs::create_dir(h.dir.join("devices.json.tmp")).unwrap();
    assert!(h
        .manager
        .device_command(json!({"type": "device.revoke", "deviceId": "AAAAAAAAAAAAAAAAAAAAAA"}))
        .await
        .is_err());
    let code = match h.manager.status() {
        Status::Failed { code } => code,
        other => panic!("{other:?}"),
    };
    assert!(code.starts_with("companion-store-write-failed"), "{code}");
    halted_gateway_outlives_the_closing(&h, &closed, &code).await;

    // Ready on another port than the node would forward to.
    let h = Harness::new(&ready_loop);
    let (closed, _) = closing_in(&h, Duration::from_millis(500));
    let mut launch = h.launch();
    launch.init["listeners"] = json!({"loopback": {"port": 47111}});
    h.manager.start(launch).unwrap();
    h.until("port mismatch", |h| {
        matches!(h.manager.status(), Status::Failed { .. })
    })
    .await;
    halted_gateway_outlives_the_closing(&h, &closed, "companion-port-mismatch").await;

    // Never ready.
    let h = Harness::new("read init; while read line; do :; done");
    let (closed, _) = closing_in(&h, Duration::from_millis(500));
    h.manager.start(h.launch()).unwrap();
    h.until("start timeout", |h| {
        matches!(h.manager.status(), Status::Failed { .. })
    })
    .await;
    halted_gateway_outlives_the_closing(&h, &closed, "companion-start-timeout").await;

    // A requested stop is the caller's own cleanup, not a halt: nothing waits.
    let h = Harness::new(&ready_loop);
    let (_, asked) = closing_in(&h, Duration::from_millis(500));
    h.manager.start(h.launch()).unwrap();
    h.until("ready", running).await;
    h.manager.stop().await;
    assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[tokio::test]
async fn companion_service_an_expired_gateway_is_told_to_go_only_after_the_node_stopped_forwarding()
{
    use std::sync::atomic::Ordering;
    // As the real one: it keeps its port until told to go, or its stdin closes.
    let h = Harness::new(&format!(
        r#"read init; echo '{READY}'; sleep 0.1
echo '{{"v":1,"type":"error","code":"policy-expired","message":"Companion request rejected"}}'
while read line; do
  printf '%s
' "$line" >> "$0.in"
  case "$line" in *'"shutdown"'*) echo released > "$0.released"; exit 1;; esac
done"#
    ));
    let (closed, asked) = closing_in(&h, Duration::from_millis(800));
    h.manager.start(h.launch()).unwrap();
    h.until("expiry noticed", |h| {
        h.events
            .lock()
            .unwrap()
            .iter()
            .any(|e| e["type"] == "error" && e["code"] == "policy-expired")
    })
    .await;
    // While the node closes its listener the gateway holds the port.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !closed.load(Ordering::SeqCst),
        "the closing ended too early for this test"
    );
    assert!(
        gateway_frames(&h, "shutdown").is_empty(),
        "told to go before the node closed"
    );
    assert_eq!(h.sidecar_file("released"), "");
    assert!(!h.manager.idle());

    // Then it is told, and leaves; the expiry is reported as before.
    h.until("released", |h| !h.sidecar_file("released").is_empty())
        .await;
    assert!(
        closed.load(Ordering::SeqCst),
        "told to go before the node closed"
    );
    h.until("disabled", |h| h.manager.status() == Status::Disabled)
        .await;
    assert_eq!(asked.load(Ordering::SeqCst), 1);
    h.until("stop hook", |h| h.stops() == 1).await;
    assert!(h
        .events
        .lock()
        .unwrap()
        .iter()
        .all(|e| e["type"] != "status" || e["status"]["state"] != "failed"));
}

#[tokio::test]
async fn companion_service_a_failure_is_forgotten_only_when_nothing_runs() {
    let h = Harness::new(&format!(
        r#"read init; echo '{READY}'; while read line; do :; done"#
    ));
    let statuses = |h: &Harness| -> Vec<Value> {
        h.events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| e["type"] == "status")
            .map(|e| e["status"]["state"].clone())
            .collect()
    };
    // Nothing failed: nothing to forget, nothing to say.
    h.manager.clear_failure();
    assert!(statuses(&h).is_empty());

    h.manager.start(h.launch()).unwrap();
    h.until("ready", running).await;
    h.manager.clear_failure();
    assert!(running(&h), "a gateway that runs has no failure to forget");

    // A halted gateway that is still on its way out keeps its failure.
    let (closed, _) = closing_in(&h, Duration::from_millis(500));
    h.manager.fail("companion-timeout");
    h.manager.clear_failure();
    assert!(matches!(h.manager.status(), Status::Failed { .. }));
    for _ in 0..300 {
        if closed.load(std::sync::atomic::Ordering::SeqCst) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(closed.load(std::sync::atomic::Ordering::SeqCst));
    h.until("gone", |h| h.manager.idle()).await;
    let before = statuses(&h).len();
    assert!(matches!(h.manager.status(), Status::Failed { .. }));

    // Once it is gone the failure is forgotten, once.
    h.manager.clear_failure();
    assert_eq!(h.manager.status(), Status::Disabled);
    assert_eq!(statuses(&h)[before..], ["disabled"]);
    h.manager.clear_failure();
    assert_eq!(statuses(&h).len(), before + 1);
    assert!(h.manager.idle());
}

#[tokio::test]
async fn companion_service_crash_fails_without_restarting() {
    // A restart would leave Serve forwarding to an unbound port meanwhile.
    let h = Harness::new(&format!(
        r#"read init; echo start >> "$0.starts"; echo '{READY}'; exit 1"#
    ));
    h.manager.start(h.launch()).unwrap();
    h.until("crashed", |h| {
        h.manager.status()
            == Status::Failed {
                code: "companion-crashed".into(),
            }
    })
    .await;
    h.until("stop hook", |h| h.stops() == 1).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(h.sidecar_file("starts").lines().count(), 1);
    assert!(h.manager.idle());
}

#[tokio::test]
async fn companion_service_startup_errors_and_expiry_never_restart() {
    for (script, status) in [
        (
            r#"read init; echo start >> "$0.starts"; echo '{"v":1,"type":"error","code":"port-in-use","message":"Companion request rejected"}'; exit 1"#
                .to_string(),
            Status::Failed {
                code: "port-in-use".into(),
            },
        ),
        (
            format!(
                r#"read init; echo start >> "$0.starts"; echo '{READY}'; echo '{{"v":1,"type":"error","code":"policy-expired","message":"Companion request rejected"}}'; exit 1"#
            ),
            Status::Disabled,
        ),
    ] {
        let h = Harness::new(&script);
        h.manager.start(h.launch()).unwrap();
        h.until("terminal status", |h| h.manager.status() == status).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(h.sidecar_file("starts").lines().count(), 1, "{status:?}");
        assert_eq!(h.manager.status(), status);
        assert_eq!(h.stops(), 1, "{status:?}");
    }
}

#[tokio::test]
async fn companion_service_ready_must_come_in_time_on_the_configured_port() {
    // Hangs after init: stopped as failed, and the stop hook closes the node's listener.
    let h = Harness::new("read init; while read line; do :; done");
    h.manager.start(h.launch()).unwrap();
    assert_eq!(
        h.manager.until_ready().await.unwrap_err(),
        "companion-start-timeout"
    );
    h.until("stop hook", |h| h.stops() == 1).await;
    // Ready on another port than Serve proxies to.
    let h = Harness::new(&format!(
        r#"read init; echo '{READY}'; while read line; do :; done"#
    ));
    let mut launch = h.launch();
    launch.init["listeners"] = json!({"loopback": {"port": 47111}});
    h.manager.start(launch).unwrap();
    h.until("port mismatch", |h| {
        h.manager.status()
            == Status::Failed {
                code: "companion-port-mismatch".into(),
            }
    })
    .await;
    h.until("stop hook", |h| h.stops() == 1).await;
    // Port 0 asks for any port.
    let h = Harness::new(&format!(
        r#"read init; echo '{READY}'; while read line; do case "$line" in *shutdown*) exit 0;; esac; done"#
    ));
    let mut launch = h.launch();
    launch.init["listeners"] = json!({"loopback": {"port": 0}});
    h.manager.start(launch).unwrap();
    h.until("ready", running).await;
    h.manager.stop().await;
}

#[test]
fn companion_service_halted_gateway_writes_nothing_more() {
    let h = Harness::new("exit 0");
    *h.manager.store.lock().unwrap() = Some(Devices::load(&h.dir).unwrap());
    let generation = {
        let mut inner = h.manager.inner.lock().unwrap();
        // Nothing runs: no handle to kill.
        assert!(halt(&mut inner, "companion-store-write-failed").is_none());
        inner.generation
    };
    h.manager.on_event(
        generation,
        json!({"type": "device.registered", "device": serde_json::from_str::<Value>(DEVICE).unwrap()}),
    );
    assert!(Devices::load(&h.dir).unwrap().all().is_empty());
}

#[tokio::test]
async fn companion_service_kills_a_sidecar_that_ignores_shutdown() {
    let h = Harness::new(&format!(
        r#"trap '' TERM; read init; echo '{READY}'; while :; do sleep 1; done"#
    ));
    h.manager.start(h.launch()).unwrap();
    h.until("ready", running).await;
    let started = std::time::Instant::now();
    h.manager.stop().await;
    assert_eq!(h.manager.status(), Status::Disabled);
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[tokio::test]
async fn companion_service_commands_need_a_running_gateway_and_time_out() {
    let h = Harness::new(&format!(
        r#"read init; echo '{READY}'; while read line; do :; done"#
    ));
    assert_eq!(
        h.manager.pairing_create(None).await.unwrap_err(),
        "companion-not-running"
    );
    h.manager.start(h.launch()).unwrap();
    assert_eq!(
        h.manager.start(h.launch()).unwrap_err(),
        "companion-already-running"
    );
    h.until("ready", running).await;
    assert_eq!(
        h.manager.pairing_create(None).await.unwrap_err(),
        "companion-timeout"
    );
    // A late reply could answer the next command: the gateway is stopped as
    // failed, nothing more is sent to it, and the stop hook closes the node's listener.
    assert_eq!(
        h.manager.status(),
        Status::Failed {
            code: "companion-timeout".into()
        }
    );
    assert_eq!(
        h.manager.pairing_create(None).await.unwrap_err(),
        "companion-not-running"
    );
    h.until("stop hook", |h| h.stops() == 1).await;
}

#[tokio::test]
async fn companion_service_missing_binary_and_corrupt_store_fail_to_start() {
    let h = Harness::new("exit 0");
    let missing = Launch {
        program: Path::new("/nonexistent/yuzora-companion").into(),
        init: json!({}),
    };
    assert!(h
        .manager
        .start(missing)
        .unwrap_err()
        .starts_with("companion-unavailable"));
    std::fs::create_dir_all(&h.dir).unwrap();
    std::fs::write(h.dir.join("devices.json"), "{broken").unwrap();
    assert_eq!(
        h.manager.start(h.launch()).unwrap_err(),
        "companion-store-corrupt"
    );
    assert_eq!(
        h.manager.status(),
        Status::Failed {
            code: "companion-store-corrupt".into()
        }
    );
}

fn push_events(h: &Harness) -> Vec<Value> {
    h.events
        .lock()
        .unwrap()
        .iter()
        .filter(|e| e["type"] == "push")
        .cloned()
        .collect()
}

fn gateway_frames(h: &Harness, kind: &str) -> Vec<Value> {
    h.sidecar_file("in")
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|frame| frame["type"] == kind)
        .collect()
}

#[tokio::test]
async fn companion_service_push_events_stay_here_and_a_gone_subscription_leaves_the_gateways_set() {
    let subscription = push_tests::subscription(1);
    let request = json!({"v": 1, "type": "push.request", "deviceIds": [push_tests::DEVICE_ID], "payload": push_tests::request_payload("p1")});
    let h = Harness::new(&format!(
        r#"read init; echo '{READY}'
echo '{{"v":1,"type":"push.subscription","deviceId":"{id}","subscription":{subscription}}}'
while read line; do
  printf '%s\n' "$line" >> "$0.in"
  case "$line" in
    *"{id}"*'"push.devices"'*) echo '{request}';;
    *'"shutdown"'*) exit 0;;
  esac
done"#,
        id = push_tests::DEVICE_ID
    ));
    push_tests::seed(&h.dir, &[push_tests::DEVICE_ID]);
    h.wire.answer("https://fcm.googleapis.com/fcm/send/1", 410);
    assert!(h.manager.push().init().is_object());
    h.manager.start(h.launch()).unwrap();
    h.until("subscription gone again", |h| {
        gateway_frames(h, "push.devices").len() == 2
    })
    .await;

    // The gateway is told the set, then the set without the dead endpoint.
    let sets: Vec<Value> = gateway_frames(&h, "push.devices")
        .iter()
        .map(|frame| frame["devices"].clone())
        .collect();
    let endpoint_ids = json!([crate::companion_push::endpoint_id(&push_tests::endpoint(1))]);
    assert_eq!(
        sets,
        [
            json!([{"deviceId": push_tests::DEVICE_ID, "endpoints": endpoint_ids}]),
            json!([])
        ]
    );
    assert_eq!(
        gateway_frames(&h, "push.devices")[0],
        json!({"v": 1, "type": "push.devices", "devices": [{"deviceId": push_tests::DEVICE_ID, "endpoints": endpoint_ids}]})
    );
    assert_eq!(h.wire.urls(), ["https://fcm.googleapis.com/fcm/send/1"]);
    assert!(h.manager.push().devices().is_empty());
    let file = std::fs::read_to_string(h.dir.join("push.json")).unwrap();
    assert!(!file.contains("fcm.googleapis.com"));

    // The UI follows through its own event, which says who, never how.
    let overview = |subscribed: Value| json!({"type": "push", "push": {"available": true, "subscribed": subscribed}});
    h.until("both push events", |h| push_events(h).len() == 2)
        .await;
    assert_eq!(
        push_events(&h),
        [
            overview(json!([push_tests::DEVICE_ID])),
            overview(json!([]))
        ]
    );
    let emitted = serde_json::to_string(&*h.events.lock().unwrap()).unwrap();
    for secret in [
        "fcm.googleapis.com",
        "p256dh",
        "BTBZMq",
        "push.subscription",
        "push.request",
        "claude",
    ] {
        assert!(!emitted.contains(secret), "{secret} reached the UI");
    }
    h.manager.stop().await;
}

#[tokio::test]
async fn companion_service_a_browser_leaving_removes_only_its_own_subscription() {
    let (first, second) = (push_tests::subscription(1), push_tests::subscription(2));
    let id = |n: u32| crate::companion_push::endpoint_id(&push_tests::endpoint(n));
    let gone = id(2);
    let h = Harness::new(&format!(
        r#"read init; echo '{READY}'
echo '{{"v":1,"type":"push.subscription","deviceId":"{device}","subscription":{first}}}'
echo '{{"v":1,"type":"push.subscription","deviceId":"{device}","subscription":{second}}}'
echo '{{"v":1,"type":"push.subscription","deviceId":"{device}","subscription":null}}'
echo '{{"v":1,"type":"push.subscription","deviceId":"{device}","subscription":null,"endpointId":"{gone}"}}'
echo '{{"v":1,"type":"push.subscription","deviceId":"{device}","subscription":null,"endpointId":"{gone}"}}'
echo '{{"v":1,"type":"session.opened","peerId":1,"deviceId":"{device}"}}'
while read line; do printf '%s
' "$line" >> "$0.in"; case "$line" in *'"shutdown"'*) exit 0;; esac; done"#,
        device = push_tests::DEVICE_ID
    ));
    push_tests::seed(&h.dir, &[push_tests::DEVICE_ID]);
    assert!(h.manager.push().init().is_object());
    h.manager.start(h.launch()).unwrap();
    h.until("later event", |h| {
        h.events
            .lock()
            .unwrap()
            .iter()
            .any(|e| e["type"] == "session.opened")
    })
    .await;
    // The device keeps the first browser's endpoint; the gateway heard each
    // change as the list of ids, and the UI that the device is still subscribed.
    h.until("three sets", |h| {
        gateway_frames(h, "push.devices").len() == 3
    })
    .await;
    let sets: Vec<Value> = gateway_frames(&h, "push.devices")
        .iter()
        .map(|frame| frame["devices"].clone())
        .collect();
    let listing = |ns: &[u32]| json!([{"deviceId": push_tests::DEVICE_ID, "endpoints": ns.iter().map(|n| id(*n)).collect::<Vec<_>>()}]);
    assert_eq!(sets, [listing(&[1]), listing(&[1, 2]), listing(&[1])]);
    assert_eq!(h.manager.push().devices(), [push_tests::DEVICE_ID]);
    // `null` without an id names nothing, and the repeat found nothing.
    assert_eq!(push_events(&h).len(), 3);
    h.manager.stop().await;
}

#[tokio::test]
async fn companion_service_push_subscriptions_of_unknown_devices_are_ignored() {
    let subscription = push_tests::subscription(1);
    let h = Harness::new(&format!(
        r#"read init; echo '{READY}'
echo '{{"v":1,"type":"push.subscription","deviceId":"{other}","subscription":{subscription}}}'
echo '{{"v":1,"type":"push.subscription","deviceId":"{known}","subscription":{{"endpoint":"https://evil.example/x","p256dh":"x","auth":"y"}}}}'
echo '{{"v":1,"type":"session.opened","peerId":1,"deviceId":"{known}"}}'
while read line; do printf '%s\n' "$line" >> "$0.in"; case "$line" in *'"shutdown"'*) exit 0;; esac; done"#,
        other = push_tests::OTHER_ID,
        known = push_tests::DEVICE_ID
    ));
    push_tests::seed(&h.dir, &[push_tests::DEVICE_ID]);
    h.manager.start(h.launch()).unwrap();
    // Events are handled in order: the one after both has been.
    h.until("later event", |h| {
        h.events
            .lock()
            .unwrap()
            .iter()
            .any(|e| e["type"] == "session.opened")
    })
    .await;
    assert!(h.manager.push().devices().is_empty());
    assert!(gateway_frames(&h, "push.devices").is_empty());
    assert!(push_events(&h).is_empty());
    h.manager.stop().await;
}

#[tokio::test]
async fn companion_service_a_pane_that_just_pushed_does_not_push_again() {
    let request = json!({"v": 1, "type": "push.request", "deviceIds": [push_tests::DEVICE_ID], "payload": push_tests::request_payload("p1")});
    let h = Harness::new(&format!(
        r#"read init; echo '{READY}'
echo '{request}'
echo '{request}'
echo '{{"v":1,"type":"session.opened","peerId":1,"deviceId":"{id}"}}'
while read line; do case "$line" in *'"shutdown"'*) exit 0;; esac; done"#,
        id = push_tests::DEVICE_ID
    ));
    push_tests::seed(&h.dir, &[push_tests::DEVICE_ID]);
    let push = h.manager.push();
    assert!(push.init().is_object());
    assert!(push.subscription(push_tests::DEVICE_ID, &push_tests::subscription(1)));
    h.manager.start(h.launch()).unwrap();
    h.until("sent", |h| !h.wire.urls().is_empty()).await;
    h.until("later event", |h| {
        h.events
            .lock()
            .unwrap()
            .iter()
            .any(|e| e["type"] == "session.opened")
    })
    .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(h.wire.urls().len(), 1);
    h.manager.stop().await;
}

#[tokio::test]
async fn companion_service_a_revoked_device_loses_its_subscription() {
    let h = Harness::new(&format!(
        r#"read init; echo '{READY}'
while read line; do
  case "$line" in
    *'"device.revoke"'*) echo '{{"v":1,"type":"device.revoked","deviceId":"{id}"}}';;
    *'"shutdown"'*) exit 0;;
  esac
done"#,
        id = push_tests::DEVICE_ID
    ));
    let (kept, other) = (push_tests::OTHER_ID, push_tests::DEVICE_ID);
    push_tests::seed(&h.dir, &[other, kept]);
    let push = h.manager.push();
    assert!(push.init().is_object());
    for (n, id) in [other, kept].into_iter().enumerate() {
        assert!(push.subscription(id, &push_tests::subscription(n as u32)));
    }
    h.manager.start(h.launch()).unwrap();
    h.until("ready", running).await;
    h.manager
        .device_command(json!({"type": "device.revoke", "deviceId": other}))
        .await
        .unwrap();
    assert_eq!(push.devices(), [kept]);
    let file = std::fs::read_to_string(h.dir.join("push.json")).unwrap();
    assert!(!file.contains("/send/0") && file.contains("/send/1"));
    assert_eq!(
        push_events(&h),
        [json!({"type": "push", "push": {"available": true, "subscribed": [kept]}})]
    );
    h.manager.stop().await;

    // Revoked while no gateway runs: the desktop edits devices.json itself.
    h.manager
        .edit_stopped(&json!({"type": "device.revoke", "deviceId": kept}))
        .unwrap();
    assert!(push.devices().is_empty());
    assert!(!std::fs::read_to_string(h.dir.join("push.json"))
        .unwrap()
        .contains("/send/1"));
    assert_eq!(push_events(&h).len(), 2);
}

#[tokio::test]
async fn companion_service_a_new_push_key_drops_the_subscriptions_and_tells_everyone() {
    let h = Harness::new(&format!(
        r#"read init; echo '{READY}'
while read line; do printf '%s\n' "$line" >> "$0.in"; case "$line" in *'"shutdown"'*) exit 0;; esac; done"#
    ));
    push_tests::seed(&h.dir, &[push_tests::DEVICE_ID]);
    let push = h.manager.push();
    // A phone subscribed under a key that is gone: the vault is empty.
    assert!(push.subscription(push_tests::DEVICE_ID, &push_tests::subscription(1)));
    h.manager.start(h.launch()).unwrap();
    h.until("ready", running).await;

    let init = h.manager.push_init();
    assert_eq!(init["devices"], json!([]));
    assert!(init["vapidPublicKey"].is_string());
    assert!(push.devices().is_empty());
    // The UI and the running gateway hear it, as for any other change.
    assert_eq!(
        push_events(&h),
        [json!({"type": "push", "push": {"available": true, "subscribed": []}})]
    );
    h.until("push.devices", |h| {
        !gateway_frames(h, "push.devices").is_empty()
    })
    .await;
    assert_eq!(
        gateway_frames(&h, "push.devices"),
        [json!({"v": 1, "type": "push.devices", "devices": []})]
    );

    // Nothing more to say when the key is there.
    h.manager.push_init();
    assert_eq!(push_events(&h).len(), 1);
    h.manager.stop().await;
}

#[tokio::test]
async fn companion_service_notify_reaches_only_a_running_gateway() {
    let h = Harness::new(&format!(
        r#"read init; echo '{READY}'
while read line; do printf '%s\n' "$line" >> "$0.in"; case "$line" in *'"shutdown"'*) exit 0;; esac; done"#
    ));
    let frame = json!({"v": 1, "type": "spaces.roots", "roots": []});
    // Nothing runs: dropped, and the next start gets the state through init.
    h.manager.notify(&frame);
    h.manager.start(h.launch()).unwrap();
    // Starting is not running: the gateway has not said it is ready.
    h.manager.notify(&frame);
    h.until("ready", running).await;
    h.manager.notify(&frame);
    h.until("frame", |h| !gateway_frames(h, "spaces.roots").is_empty())
        .await;
    h.manager.stop().await;
    h.manager.notify(&frame);
    assert_eq!(gateway_frames(&h, "spaces.roots"), [frame]);
}

/// Git the way the gateway runs it: its allowlisted environment and a quiet
/// config, with `extra` on top.
fn gateway_git(
    dir: &Path,
    config: &Path,
    extra: &[(&str, &str)],
    args: &[&str],
) -> std::process::Output {
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env_clear()
        .envs(
            ENV_ALLOWLIST
                .iter()
                .filter_map(|key| std::env::var_os(key).map(|value| (*key, value))),
        )
        .env("GIT_CONFIG_GLOBAL", config)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .envs(extra.iter().copied())
        .output()
        .unwrap()
}

#[test]
fn companion_gateway_git_env_refuses_lazy_fetches_even_for_a_git_before_2_44() {
    let root = tempfile::tempdir().unwrap();
    let config = root.path().join("gitconfig");
    std::fs::write(&config, "").unwrap();
    let (src, clone) = (root.path().join("src"), root.path().join("clone"));
    let ok = |output: std::process::Output| {
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };
    let git =
        |dir: &Path, extra: &[(&str, &str)], args: &[&str]| gateway_git(dir, &config, extra, args);
    std::fs::create_dir(&src).unwrap();
    ok(git(&src, &[], &["init", "-q"]));
    ok(git(
        &src,
        &[],
        &["config", "uploadpack.allowFilter", "true"],
    ));
    std::fs::write(src.join("a.txt"), "one\n").unwrap();
    std::fs::write(src.join("b.txt"), "two\n").unwrap();
    ok(git(&src, &[], &["add", "."]));
    ok(git(
        &src,
        &[],
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.com",
            "commit",
            "-qm",
            "first",
        ],
    ));
    let blob = ok(git(&src, &[], &["rev-parse", "HEAD:a.txt"]));
    let blob = blob.trim();
    // A partial clone: its blobs stay on the remote until something asks.
    ok(git(
        root.path(),
        &[],
        &[
            "clone",
            "-q",
            "--no-checkout",
            "--filter=blob:none",
            &format!("file://{}", src.display()),
            clone.to_str().unwrap(),
        ],
    ));
    let missing = || {
        ok(git(
            &clone,
            &[],
            &["rev-list", "--objects", "--all", "--missing=print"],
        ))
        .lines()
        .filter(|line| line.starts_with('?'))
        .count()
    };
    assert_eq!(missing(), 2);

    // What a git that does not know GIT_NO_LAZY_FETCH is left with.
    let older: Vec<_> = GATEWAY_GIT_ENV
        .iter()
        .copied()
        .filter(|(key, _)| *key != "GIT_NO_LAZY_FETCH")
        .collect();
    let read = git(&clone, &older, &["cat-file", "-p", blob]);
    assert!(!read.status.success(), "a missing blob was fetched");
    assert_eq!(missing(), 2, "the remote was asked for objects");
    // Nothing else forbids it, so the refusal above is the variable's.
    assert_eq!(ok(git(&clone, &[], &["cat-file", "-p", blob])), "one\n");
    assert_eq!(missing(), 1);

    // What a phone reads still works with the whole environment.
    std::fs::write(src.join("a.txt"), "changed\n").unwrap();
    assert!(ok(git(&src, GATEWAY_GIT_ENV, &["status", "--porcelain"])).contains(" M a.txt"));
    assert!(ok(git(&src, GATEWAY_GIT_ENV, &["log", "--oneline"])).contains("first"));
    assert!(ok(git(&src, GATEWAY_GIT_ENV, &["diff"])).contains("+changed"));
    assert!(ok(git(&clone, GATEWAY_GIT_ENV, &["log", "--oneline"])).contains("first"));
}
