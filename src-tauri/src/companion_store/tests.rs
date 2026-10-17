use super::*;
use serde_json::json;

// The gateway's own fixture: what it emits is what init must accept back.
const DEVICE: &str = include_str!("../../companion/tests/fixtures/device.json");

fn device() -> Value {
    serde_json::from_str(DEVICE).unwrap()
}

#[test]
fn companion_store_devices_round_trip_byte_for_byte_shape() {
    let dir = tempfile::tempdir().unwrap();
    let mut devices = Devices::load(dir.path()).unwrap();
    assert!(devices.all().is_empty());
    devices.upsert(device()).unwrap();
    let reloaded = Devices::load(dir.path()).unwrap();
    assert_eq!(reloaded.all(), [device()]);
    let mut renamed = device();
    renamed["label"] = json!("Renamed phone");
    devices.upsert(renamed.clone()).unwrap();
    assert_eq!(Devices::load(dir.path()).unwrap().all(), [renamed]);
    devices.remove("AAAAAAAAAAAAAAAAAAAAAA").unwrap();
    assert!(Devices::load(dir.path()).unwrap().all().is_empty());
}

#[test]
fn companion_store_applies_admin_device_commands() {
    let dir = tempfile::tempdir().unwrap();
    let mut devices = Devices::load(dir.path()).unwrap();
    devices.upsert(device()).unwrap();
    let id = "AAAAAAAAAAAAAAAAAAAAAA";
    devices
        .apply(
            &json!({"type": "device.update", "deviceId": id, "label": "Renamed", "grants": null}),
        )
        .unwrap();
    let stored = Devices::load(dir.path()).unwrap().all()[0].clone();
    assert_eq!(stored["label"], "Renamed");
    assert_eq!(stored["grants"], device()["grants"]);
    let grants = json!({"control": null, "full": true});
    devices
        .apply(&json!({"type": "device.update", "deviceId": id, "grants": grants}))
        .unwrap();
    assert_eq!(
        Devices::load(dir.path()).unwrap().all()[0]["grants"],
        grants
    );
    let mut locked = Devices::load(dir.path()).unwrap().all()[0].clone();
    locked["locked"] = json!(true);
    devices.upsert(locked).unwrap();
    devices
        .apply(&json!({"type": "device.unlock", "deviceId": id}))
        .unwrap();
    assert_eq!(Devices::load(dir.path()).unwrap().all()[0]["locked"], false);
    // Mirrors the gateway: an empty update or a mixed grant delta is rejected.
    let wide = json!({"control": {"until": "2030-01-01T00:15:00Z"}, "full": false});
    devices
        .apply(&json!({"type": "device.update", "deviceId": id, "grants": {"control": null, "full": false}}))
        .unwrap();
    devices
        .apply(&json!({"type": "device.update", "deviceId": id, "grants": wide}))
        .unwrap();
    for grants in [
        json!({"control": null, "full": true}),
        json!({"control": {"until": "2030-01-01T00:10:00Z"}, "full": true}),
    ] {
        assert_eq!(
            devices
                .apply(&json!({"type": "device.update", "deviceId": id, "grants": grants}))
                .unwrap_err(),
            "companion-grants-mixed",
            "{grants}"
        );
    }
    assert_eq!(Devices::load(dir.path()).unwrap().all()[0]["grants"], wide);
    for (frame, code) in [
        (
            json!({"type": "device.update", "deviceId": id, "label": null, "grants": null}),
            "companion-device-invalid",
        ),
        (
            json!({"type": "device.unlock", "deviceId": "BBBBBBBBBBBBBBBBBBBBBB"}),
            "companion-device-unknown",
        ),
        (
            json!({"type": "device.rename", "deviceId": id}),
            "companion-device-invalid",
        ),
        (json!({"type": "device.unlock"}), "companion-device-invalid"),
    ] {
        assert_eq!(devices.apply(&frame).unwrap_err(), code, "{frame}");
    }
    devices
        .apply(&json!({"type": "device.revoke", "deviceId": id}))
        .unwrap();
    assert!(Devices::load(dir.path()).unwrap().all().is_empty());
}

#[test]
fn companion_store_rejects_devices_without_ids() {
    let dir = tempfile::tempdir().unwrap();
    let mut devices = Devices::load(dir.path()).unwrap();
    assert_eq!(
        devices.upsert(json!({"label": "no id"})).unwrap_err(),
        "companion-device-invalid"
    );
    assert!(!dir.path().join("devices.json").exists());
}

#[test]
fn companion_store_failed_write_leaves_memory_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let mut devices = Devices::load(dir.path()).unwrap();
    devices.upsert(device()).unwrap();
    // A directory in the temp file's place makes the next write fail.
    std::fs::create_dir(dir.path().join("devices.json.tmp")).unwrap();
    assert!(devices
        .remove("AAAAAAAAAAAAAAAAAAAAAA")
        .unwrap_err()
        .starts_with("companion-store-write-failed"));
    assert_eq!(devices.all(), [device()]);
    assert_eq!(Devices::load(dir.path()).unwrap().all(), [device()]);
}

#[test]
fn companion_store_corrupt_files_are_kept_and_backed_up() {
    for (file, contents) in [
        ("devices.json", "{not json"),
        ("devices.json", r#"{"version":2,"devices":[]}"#),
        (
            "devices.json",
            r#"{"version":1,"devices":[{"label":"no id"}]}"#,
        ),
        ("settings.json", r#"{"version":1}"#),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(file);
        std::fs::write(&path, contents).unwrap();
        let error = if file == "devices.json" {
            Devices::load(dir.path()).err().unwrap()
        } else {
            Settings::load(dir.path()).unwrap_err()
        };
        assert_eq!(error, "companion-store-corrupt", "{contents}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), contents);
        let backups = std::fs::read_dir(dir.path())
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(&format!("{file}.corrupt-"))
            })
            .count();
        assert_eq!(backups, 1, "{contents}");
    }
}

#[test]
fn companion_store_settings_defaults_and_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let defaults = Settings::load(dir.path()).unwrap();
    assert_eq!(defaults, Settings::default());
    assert_eq!((defaults.https_port, defaults.gateway_port), (8443, 47110));
    assert_eq!(defaults.duration, EnableFor::EightHours);
    let mut settings = defaults;
    settings.duration = EnableFor::UntilExit;
    settings.session_allowlist = vec!["default".into()];
    settings.applied = Some(AppliedServe {
        https_port: 8443,
        gateway_port: 47110,
        host: "desktop.tail1234.ts.net".into(),
        login: "user@example.com".into(),
    });
    settings.save(dir.path()).unwrap();
    assert_eq!(Settings::load(dir.path()).unwrap(), settings);
    let raw: Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("settings.json")).unwrap()).unwrap();
    assert_eq!(raw["duration"], "untilExit");
    assert_eq!(raw["applied"]["httpsPort"], 8443);
}

#[cfg(unix)]
#[test]
fn companion_store_files_are_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("companion");
    Devices::load(&dir).unwrap().upsert(device()).unwrap();
    Settings::default().save(&dir).unwrap();
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&dir), 0o700);
    assert_eq!(mode(&dir.join("devices.json")), 0o600);
    assert_eq!(mode(&dir.join("settings.json")), 0o600);
}
