#[path = "support/init.rs"]
mod init_fixture;
use serde_json::{json, Value};
use yuzora_companion::{
    admin::{Init, PasskeyBinding, Policy},
    clock::Timestamp,
    devices::{CredentialRecord, Device},
};

#[test]
fn devices_strict_fields_lengths_ids_credentials_and_duplicates() {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/device.json")).unwrap();
    assert!(serde_json::from_value::<Device>(fixture.clone()).is_ok());
    for (pointer, invalid) in [
        ("/deviceId", json!("not-a-device-id")),
        ("/label", json!("")),
        ("/label", json!("x".repeat(81))),
        ("/label", json!("phone\n")),
        ("/label", json!("\u{202e}phone")),
        ("/credentials", json!([])),
        ("/credentials/0/id", json!("bad+base64")),
        ("/credentials/0/rpId", json!("Desktop.example")),
        ("/credentials/0/rpId", json!("127.0.0.1")),
        ("/credentials/0/publicKeyState", json!("a".repeat(6000))),
        ("/credentials/0/publicKeyState", json!("oQ")),
        ("/credentials/0/stateFormat", json!("webauthn_rp-next")),
        ("/credentials/0/backupState", json!(true)),
        ("/credentials/0/transports", json!(["unknown"])),
        ("/credentials/0/transports", json!(["usb", "usb"])),
        ("/createdAt", json!("not-time")),
        ("/lastSeen", json!("2029-01-01T00:00:00Z")),
    ] {
        let mut device = fixture.clone();
        *device.pointer_mut(pointer).unwrap() = invalid;
        assert!(
            serde_json::from_value::<Device>(device).is_err(),
            "{pointer}"
        );
    }
    for pointer in ["", "/grants", "/grants/control", "/credentials/0"] {
        let mut device = fixture.clone();
        device
            .pointer_mut(pointer)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("unknown".into(), json!(true));
        assert!(
            serde_json::from_value::<Device>(device).is_err(),
            "{pointer}"
        );
    }
    let mut credential = fixture["credentials"][0].clone();
    credential["id"] = json!("!");
    assert!(serde_json::from_value::<CredentialRecord>(credential).is_err());
    let mut init: Value = serde_json::from_str(&init_fixture::json()).unwrap();
    init["devices"] = json!([fixture.clone(), fixture.clone()]);
    assert!(serde_json::from_value::<Init>(init.clone()).is_err());
    init["devices"][1]["deviceId"] = json!("AQEBAQEBAQEBAQEBAQEBAQ");
    // A credential cannot be attributed to two different devices.
    assert!(serde_json::from_value::<Init>(init).is_err());
}

#[test]
fn policy_rfc3339_utc_ttl_limits_and_provisional_binding() {
    let fixture: Value = serde_json::from_str(&init_fixture::json()).unwrap();
    let policy = fixture["policy"].clone();
    assert_eq!(
        serde_json::from_value::<Policy>(policy.clone())
            .unwrap()
            .passkey_binding,
        PasskeyBinding::O2
    );
    for binding in ["o1", "o2", "o3"] {
        let mut p = policy.clone();
        p["passkeyBinding"] = json!(binding);
        assert_eq!(
            serde_json::to_value(serde_json::from_value::<Policy>(p).unwrap()).unwrap()
                ["passkeyBinding"],
            binding
        );
    }
    let mut p = policy.clone();
    p["passkeyBinding"] = json!("o4");
    assert!(serde_json::from_value::<Policy>(p).is_err());
    for (field, value) in [
        ("viewIdleMin", 0),
        ("viewIdleMin", 31),
        ("viewAbsoluteHours", 0),
        ("viewAbsoluteHours", 13),
        ("elevationMaxMin", 0),
        ("elevationMaxMin", 16),
        ("maxDevices", 0),
        ("maxDevices", 65),
    ] {
        let mut p = policy.clone();
        p[field] = json!(value);
        assert!(!serde_json::from_value::<Policy>(p).unwrap().valid());
    }
    for (time, valid) in [
        ("2030-01-01T00:00:00Z", true),
        ("2030-01-01t00:00:00Z", false),
        ("2030-01-01T00:00:00z", false),
        ("2030-01-01 00:00:00Z", false),
        ("2030-01-01T00:00:00.1234567890Z", false),
        ("2030-01-01T00:00:00.Z", false),
        ("2030-01-01T00:00:00.123456789+00:00", true),
        ("2030-01-01T00:00:00+00:00", true),
        ("2030-01-01T00:00:00.125Z", true),
        ("2030-01-01T00:00:00+01:00", false),
        ("2030-01-01T00:00:00-00:00", false),
        ("2030-02-30T00:00:00Z", false),
        ("2030-01-01", false),
        ("2030-01-01T00:00:60Z", false),
    ] {
        assert_eq!(
            serde_json::from_value::<Timestamp>(json!(time)).is_ok(),
            valid,
            "{time}"
        );
    }
}

#[tokio::test]
async fn devices_admin_commands_strict_unknown_fields_and_shapes() {
    use yuzora_companion::admin;
    for value in [
        json!({"v":1,"type":"pairing.create","path":"tailscale"}),
        json!({"v":1,"type":"pairing.approve","pairingId":"AAAAAAAAAAAAAAAAAAAAAA"}),
        json!({"v":1,"type":"pairing.reject","pairingId":"AAAAAAAAAAAAAAAAAAAAAA"}),
        json!({"v":1,"type":"pairing.cancel","pairingId":"AAAAAAAAAAAAAAAAAAAAAA"}),
        json!({"v":1,"type":"device.update","deviceId":"AAAAAAAAAAAAAAAAAAAAAA","grants":{"control":null,"full":false}}),
        json!({"v":1,"type":"device.revoke","deviceId":"AAAAAAAAAAAAAAAAAAAAAA"}),
        json!({"v":1,"type":"device.unlock","deviceId":"AAAAAAAAAAAAAAAAAAAAAA"}),
    ] {
        let frame = format!("{value}\n");
        assert!(admin::read(&mut frame.as_bytes()).await.is_ok());
        let mut invalid = value;
        invalid["unknown"] = json!(true);
        let frame = format!("{invalid}\n");
        assert!(matches!(
            admin::read(&mut frame.as_bytes()).await,
            Err("invalid-command")
        ));
    }
}

#[test]
fn devices_labels_reject_format_and_bidi_controls() {
    use yuzora_companion::devices::valid_label;
    for code in [
        0x61c, 0x200e, 0x200f, 0x2028, 0x2029, 0x200b, 0x200c, 0x200d, 0xad, 0x600, 0x605, 0x6dd,
        0x70f, 0x890, 0x891, 0x8e2, 0x180e, 0x202a, 0x202e, 0x2060, 0x2064, 0x2066, 0x206f, 0xfeff,
        0xfff9, 0xfffb, 0x110bd, 0x110cd, 0x13430, 0x1343f, 0x1bca0, 0x1bca3, 0x1d173, 0x1d17a,
        0xe0001, 0xe0020, 0xe007f,
    ] {
        assert!(
            !valid_label(&format!("phone{}label", char::from_u32(code).unwrap())),
            "U+{code:04X}"
        );
    }
    assert!(valid_label("手機 📱"));
    assert!(valid_label("日本語 e\u{301}"));
}

/// The PWA pre-checks labels and recomputes the SAS from this same fixture
/// (src/companion/lib/contract.test.ts); both sides must agree with the gateway.
#[test]
fn pwa_contract_fixture_matches_label_rules_and_sas() {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    use yuzora_companion::{devices::valid_label, pairing::derive_sas};
    let fixture: Value = serde_json::from_str(include_str!("fixtures/pwa-contract.json")).unwrap();
    for (key, expected) in [("valid", true), ("invalid", false)] {
        let labels = fixture["labels"][key].as_array().unwrap();
        assert!(labels.len() > 5);
        for label in labels {
            assert_eq!(valid_label(label.as_str().unwrap()), expected, "{label}");
        }
    }
    for case in fixture["sas"].as_array().unwrap() {
        let bytes = |key: &str| URL_SAFE_NO_PAD.decode(case[key].as_str().unwrap()).unwrap();
        assert_eq!(
            derive_sas(
                &bytes("secret"),
                &bytes("deviceNonce"),
                &bytes("gatewayNonce")
            ),
            case["sas"].as_str().unwrap()
        );
    }
}
