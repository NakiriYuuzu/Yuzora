//! Fake sidecars are shell scripts speaking the admin NDJSON protocol.
use super::*;
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
        let manager = Arc::new(CompanionManager {
            dir: dir.clone(),
            emit: Arc::new(move |event| sink.lock().unwrap().push(event)),
            stopped: Arc::new(move || {
                count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }),
            inner: Mutex::default(),
            store: Mutex::new(None),
            commands: tokio::sync::Mutex::new(()),
            timing: Timing {
                command: Duration::from_millis(500),
                stop: Duration::from_millis(500),
            },
        });
        Self {
            _root: root,
            dir,
            bin,
            manager,
            events,
            stopped,
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
    h.until("ready", running).await;
    assert_eq!(h.manager.status(), Status::Running { port: 47110 });
    let init: Value = serde_json::from_str(&h.sidecar_file("init")).unwrap();
    assert_eq!(init["marker"], "init-frame");
    assert_eq!(
        init["devices"],
        json!([serde_json::from_str::<Value>(DEVICE).unwrap()])
    );
    // Only allowlisted variables reach the gateway; sh adds PWD, SHLVL and _.
    for line in h.sidecar_file("env").lines() {
        let key = line.split('=').next().unwrap();
        assert!(
            ENV_ALLOWLIST.contains(&key) || ["PWD", "SHLVL", "_", "OLDPWD"].contains(&key),
            "leaked {key}"
        );
    }
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
    h.manager.stop().await;
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
