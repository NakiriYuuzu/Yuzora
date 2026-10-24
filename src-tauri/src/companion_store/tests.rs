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
        (
            "settings.json",
            r#"{"version":1,"gatewayPort":0,"duration":"8h","sessionAllowlist":[],"fullSessionEnabled":false}"#,
        ),
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
    assert_eq!(defaults.gateway_port, 47110);
    assert_eq!(defaults.duration, EnableFor::EightHours);
    assert!(!defaults.experimental, "the Experimental opt-in starts off");
    assert_eq!(defaults.net, None);
    // Files written before the opt-in or the node existed still load, with
    // them off; so do the fields of the Serve handler this replaced.
    std::fs::write(
        dir.path().join("settings.json"),
        r#"{"version":1,"httpsPort":8443,"gatewayPort":47110,"duration":"8h","sessionAllowlist":[],"fullSessionEnabled":false,"applied":{"httpsPort":0,"gatewayPort":47110,"host":"h","login":"l"}}"#,
    )
    .unwrap();
    let old = Settings::load(dir.path()).unwrap();
    assert!(!old.experimental);
    assert_eq!(old.net, None);
    std::fs::remove_file(dir.path().join("settings.json")).unwrap();
    let mut settings = defaults;
    settings.duration = EnableFor::UntilExit;
    settings.experimental = true;
    settings.session_allowlist = vec!["default".into()];
    settings.net = Some(NetIdentity {
        dns_name: "desktop.tail1234.ts.net".into(),
        login: "user@example.com".into(),
    });
    settings.save(dir.path()).unwrap();
    assert_eq!(Settings::load(dir.path()).unwrap(), settings);
    let raw: Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("settings.json")).unwrap()).unwrap();
    assert_eq!(raw["duration"], "untilExit");
    assert_eq!(raw["net"]["dnsName"], "desktop.tail1234.ts.net");
    assert!(raw.get("applied").is_none() && raw.get("httpsPort").is_none());
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

#[test]
fn companion_store_validation_matches_the_gateway() {
    assert!(valid_grants(&json!({"control": null, "full": false})).is_ok());
    assert!(
        valid_grants(&json!({"control": {"until": "2030-01-01T00:15:00Z"}, "full": true})).is_ok()
    );
    for bad in [
        json!(null),
        json!({"control": null}),
        json!({"control": null, "full": "yes"}),
        json!({"control": {"until": "soon"}, "full": false}),
        json!({"control": {"until": "2030-01-01T08:15:00+08:00"}, "full": false}),
        json!({"control": {"until": "2030-01-01T00:15:00Z", "extra": 1}, "full": false}),
        json!({"control": null, "full": false, "admin": true}),
    ] {
        assert_eq!(
            valid_grants(&bad).unwrap_err(),
            "companion-grants-invalid",
            "{bad}"
        );
    }
    for good in ["iPhone", "📱 Yuuzu の iPhone", &"x".repeat(80)] {
        assert!(valid_label(good).is_ok(), "{good}");
    }
    // The gateway's rules: 80 bytes, trimmed, no controls or format characters.
    for bad in [
        "",
        "line\nbreak",
        " leading",
        "trailing ",
        "zero\u{200b}width",
        "bidi\u{202e}override",
        "soft\u{ad}hyphen",
        &"x".repeat(81),
        &"é".repeat(41),
    ] {
        assert_eq!(
            valid_label(bad).unwrap_err(),
            "companion-label-invalid",
            "{bad:?}"
        );
    }
    for good in [
        "2030-01-01T00:15:00Z",
        "2030-01-01T00:15:00+00:00",
        "2030-01-01T00:15:00.123456789Z",
    ] {
        assert!(valid_until(good), "{good}");
    }
    for bad in [
        "2030-01-01t00:15:00Z",
        "2030-01-01 00:15:00Z",
        "2030-01-01T00:15:00.Z",
        "2030-01-01T00:15:00.1234567890Z",
        "2030-01-01T00:15:60Z",
        "2030-01-01T00:15:00-00:00",
        "2030-02-30T00:15:00Z",
    ] {
        assert!(!valid_until(bad), "{bad}");
    }
}

#[test]
fn companion_store_load_rejects_what_gateway_init_would() {
    let load = |devices: Vec<Value>| {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("devices.json"),
            json!({"version": 1, "devices": devices}).to_string(),
        )
        .unwrap();
        Devices::load(dir.path()).map(|d| d.all().len())
    };
    // Distinct ids and credentials, as the gateway would emit them.
    let numbered = |n: u8| {
        let mut device = device();
        device["deviceId"] = json!(URL_SAFE_NO_PAD.encode([n; 16]));
        device["credentials"][0]["id"] = json!(URL_SAFE_NO_PAD.encode([n]));
        device
    };
    // Good files load, up to the gateway's limit.
    assert_eq!(load(vec![device()]), Ok(1));
    let mut offset = device();
    offset["createdAt"] = json!("2030-01-01T00:00:00+00:00");
    offset["lastSeen"] = json!("2030-01-01T00:00:00.5Z");
    assert_eq!(load(vec![offset]), Ok(1));
    assert_eq!(load((0..64).map(numbered).collect()), Ok(64));
    let mut cases: Vec<(&str, Vec<Value>)> = vec![
        ("65 devices", (0..65).map(numbered).collect()),
        ("duplicate deviceId", vec![device(), device()]),
    ];
    let mut shared = numbered(1);
    shared["credentials"] = device()["credentials"].clone();
    cases.push(("credential on two devices", vec![device(), shared]));
    let mut broken = |name: &'static str, change: fn(&mut Value)| {
        let mut device = device();
        change(&mut device);
        cases.push((name, vec![device]));
    };
    broken("unknown field", |d| d["extra"] = json!(1));
    broken("missing field", |d| {
        d.as_object_mut().unwrap().remove("locked");
    });
    broken("non-canonical id", |d| {
        d["deviceId"] = json!("AAAAAAAAAAAAAAAAAAAAAB")
    });
    broken("short id", |d| d["deviceId"] = json!("AAAA"));
    broken("label", |d| d["label"] = json!(" padded"));
    broken("grants", |d| d["grants"] = json!({"control": null}));
    broken("locked", |d| d["locked"] = json!("no"));
    broken("timestamp shape", |d| {
        d["createdAt"] = json!("2030-01-01 00:00:00Z")
    });
    broken("seen before created", |d| {
        d["lastSeen"] = json!("2029-12-31T23:59:59Z")
    });
    broken("no credentials", |d| d["credentials"] = json!([]));
    broken("credential without rpId", |d| {
        d["credentials"][0].as_object_mut().unwrap().remove("rpId");
    });
    for (name, devices) in cases {
        assert_eq!(
            load(devices),
            Err("companion-store-corrupt".into()),
            "{name}"
        );
    }
}

/// Each rejected row is a credential the gateway's `CredentialRecord` refuses
/// at init (`companion/src/devices.rs`), so the file must take the
/// corrupt-backup path instead of failing every enable. The accepted rows are
/// edges the gateway allows: the check must not be stricter.
#[test]
fn companion_store_credentials_follow_the_gateways_rules() {
    fn with(change: impl FnOnce(&mut Value)) -> Value {
        let mut value = device();
        change(&mut value["credentials"][0]);
        value
    }
    let rejected = [
        (
            "missing publicKeyState",
            with(|c| {
                c.as_object_mut().unwrap().remove("publicKeyState");
            }),
        ),
        ("unknown field", with(|c| c["extra"] = json!(1))),
        (
            "publicKeyState not a string",
            with(|c| c["publicKeyState"] = json!(1)),
        ),
        ("rpId uppercase", with(|c| c["rpId"] = json!("Bad_Host"))),
        ("rpId ip address", with(|c| c["rpId"] = json!("127.0.0.1"))),
        ("rpId empty label", with(|c| c["rpId"] = json!("a..b"))),
        ("id not base64url", with(|c| c["id"] = json!("***"))),
        ("id empty", with(|c| c["id"] = json!(""))),
        (
            "id too long",
            with(|c| c["id"] = json!(URL_SAFE_NO_PAD.encode([1u8; 1025]))),
        ),
        (
            "wrong stateFormat",
            with(|c| c["stateFormat"] = json!("webauthn_rp-0.2.0-static")),
        ),
        (
            "signCount over u32",
            with(|c| c["signCount"] = json!(4_294_967_296u64)),
        ),
        ("signCount negative", with(|c| c["signCount"] = json!(-1))),
        ("signCount float", with(|c| c["signCount"] = json!(1.5))),
        (
            "backupState without eligible",
            with(|c| c["backupState"] = json!(true)),
        ),
        (
            "backupEligible not a bool",
            with(|c| c["backupEligible"] = json!("no")),
        ),
        (
            "duplicate transport",
            with(|c| c["transports"] = json!(["usb", "usb"])),
        ),
        (
            "unknown transport",
            with(|c| c["transports"] = json!(["carrier-pigeon"])),
        ),
        (
            "transports not an array",
            with(|c| c["transports"] = json!("usb")),
        ),
        ("credential not an object", with(|c| *c = Value::Null)),
    ];
    for (name, bad) in rejected {
        let dir = tempfile::tempdir().unwrap();
        let contents = json!({"version": 1, "devices": [bad]}).to_string();
        std::fs::write(dir.path().join("devices.json"), &contents).unwrap();
        assert_eq!(
            Devices::load(dir.path()).err().as_deref(),
            Some("companion-store-corrupt"),
            "{name}"
        );
        let backed_up = std::fs::read_dir(dir.path()).unwrap().any(|e| {
            e.unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("devices.json.corrupt-")
        });
        assert!(backed_up, "{name}");
    }
    let accepted = [
        (
            "every transport",
            with(|c| {
                c["transports"] = json!(["usb", "nfc", "ble", "internal", "hybrid", "smart-card"])
            }),
        ),
        ("no transports", with(|c| c["transports"] = json!([]))),
        (
            "largest signCount",
            with(|c| c["signCount"] = json!(u32::MAX)),
        ),
        (
            "synced passkey",
            with(|c| {
                c["backupEligible"] = json!(true);
                c["backupState"] = json!(true);
            }),
        ),
        (
            "longest id",
            with(|c| c["id"] = json!(URL_SAFE_NO_PAD.encode([1u8; 1024]))),
        ),
    ];
    for (name, good) in accepted {
        let dir = tempfile::tempdir().unwrap();
        let contents = json!({"version": 1, "devices": [good.clone()]}).to_string();
        std::fs::write(dir.path().join("devices.json"), contents).unwrap();
        assert_eq!(Devices::load(dir.path()).unwrap().all(), [good], "{name}");
    }
}
