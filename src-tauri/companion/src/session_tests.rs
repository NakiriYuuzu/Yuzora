use super::*;
use crate::test_support::Fixture;

const DEVICE: &str = "AAAAAAAAAAAAAAAAAAAAAA";
const RP: &str = "desktop.tailnet.ts.net";
fn fixture() -> Fixture {
    let mut init = Fixture::init();
    init.devices =
        vec![serde_json::from_str(include_str!("../tests/fixtures/device.json")).unwrap()];
    init.policy.full_session_enabled = true;
    Fixture::with_init(init)
}
#[test]
fn session_cookie_attributes_and_ambiguous_cookie_rejection() {
    let id = [7; 32];
    let cookie = session_cookie(&id);
    assert!(cookie.starts_with("__Host-yzc="));
    assert!(cookie.ends_with("; Secure; HttpOnly; SameSite=Strict; Path=/"));
    assert!(!cookie.contains("Domain"));
    assert!(clear_cookie().ends_with("Max-Age=0"));
    let value = crate::secrets::encode(&id);
    for (input, expected) in [
        (format!("__Host-yzc={value}"), Some(id)),
        (format!("__Host-yzc={value}; __Host-yzc={value}"), None),
        ("__Host-yzc=bad".into(), None),
        (format!("other={value}"), None),
    ] {
        let mut headers = HeaderMap::new();
        headers.insert("cookie", input.parse().unwrap());
        assert_eq!(cookie_id(&headers), expected);
    }
}
#[tokio::test(start_paused = true)]
async fn session_idle_absolute_enable_until_and_rp_boundaries() {
    for scenario in ["idle", "absolute", "enable"] {
        let mut init = Fixture::init();
        init.devices =
            vec![serde_json::from_str(include_str!("../tests/fixtures/device.json")).unwrap()];
        if scenario == "enable" {
            init.policy.enable_until = init.devices[0].created_at.after(Duration::from_secs(60));
        }
        let f = Fixture::with_init(init);
        let id = f
            .gateway
            .identity
            .lock()
            .unwrap()
            .create_for_test(DEVICE, RP)
            .unwrap()
            .0;
        assert!(!f
            .gateway
            .identity
            .lock()
            .unwrap()
            .validate(&id, "other.ts.net", Scope::View));
        assert!(f
            .gateway
            .identity
            .lock()
            .unwrap()
            .validate(&id, RP, Scope::View));
        match scenario {
            "idle" => tokio::time::advance(Duration::from_secs(1800)).await,
            "enable" => tokio::time::advance(Duration::from_secs(60)).await,
            _ => {
                for _ in 0..47 {
                    tokio::time::advance(Duration::from_secs(900)).await;
                    assert!(f
                        .gateway
                        .identity
                        .lock()
                        .unwrap()
                        .validate(&id, RP, Scope::View));
                }
                tokio::time::advance(Duration::from_secs(900)).await;
            }
        }
        assert!(!f
            .gateway
            .identity
            .lock()
            .unwrap()
            .validate(&id, RP, Scope::View));
    }
}
#[tokio::test(start_paused = true)]
async fn session_elevations_distinct_scopes_grant_cap_full_freshness() {
    let f = fixture();
    let id = f
        .gateway
        .identity
        .lock()
        .unwrap()
        .create_for_test(DEVICE, RP)
        .unwrap()
        .0;
    {
        let mut identity = f.gateway.identity.lock().unwrap();
        assert!(!identity.validate(&id, RP, Scope::Control));
        identity
            .devices
            .get_mut(DEVICE)
            .unwrap()
            .grants
            .control
            .as_mut()
            .unwrap()
            .until = f.gateway.clock.now().after(Duration::from_secs(60));
        assert_eq!(
            identity.elevate_for_test(&id, RP, Scope::Control).unwrap(),
            f.gateway.clock.now().after(Duration::from_secs(60))
        );
        assert!(identity.validate(&id, RP, Scope::Control));
        assert!(!identity.validate(&id, RP, Scope::Full));
    }
    tokio::time::advance(Duration::from_secs(60)).await;
    {
        let mut identity = f.gateway.identity.lock().unwrap();
        assert!(!identity.validate(&id, RP, Scope::Control));
        assert!(identity.elevate_for_test(&id, RP, Scope::Control).is_none());
        assert!(identity.elevate_for_test(&id, RP, Scope::Full).is_some());
        assert!(identity.validate(&id, RP, Scope::Full));
    }
    tokio::time::advance(Duration::from_secs(121)).await;
    {
        let mut identity = f.gateway.identity.lock().unwrap();
        assert!(identity.validate(&id, RP, Scope::Full));
        let ctx = AuthContext {
            session_id: id,
            device_id: DEVICE.into(),
            rp_id: RP.into(),
        };
        assert!(identity.write_deadline(&ctx, Scope::Full, true).is_none());
        assert!(identity.write_deadline(&ctx, Scope::Full, false).is_some());
    }
    tokio::time::advance(Duration::from_secs(779)).await;
    assert!(!f
        .gateway
        .identity
        .lock()
        .unwrap()
        .validate(&id, RP, Scope::Full));
}
#[tokio::test]
async fn devices_update_revoke_unlock_invalidate_sessions_and_elevations() {
    for action in ["update", "revoke", "unlock"] {
        let mut f = fixture();
        let id = f
            .gateway
            .identity
            .lock()
            .unwrap()
            .create_for_test(DEVICE, RP)
            .unwrap()
            .0;
        let other = f
            .gateway
            .identity
            .lock()
            .unwrap()
            .create_for_test(DEVICE, RP)
            .unwrap()
            .0;
        f.gateway
            .identity
            .lock()
            .unwrap()
            .elevate_for_test(&id, RP, Scope::Control)
            .unwrap();
        let command = match action {
            "update" => DeviceCommand::Update {
                v: 1,
                device_id: DEVICE.into(),
                label: Some("New label".into()),
                grants: None,
            },
            "revoke" => DeviceCommand::Revoke {
                v: 1,
                device_id: DEVICE.into(),
            },
            _ => DeviceCommand::Unlock {
                v: 1,
                device_id: DEVICE.into(),
            },
        };
        f.gateway
            .command(crate::admin::CommandBody::Device(command))
            .unwrap();
        let event = f.event().await;
        assert_eq!(
            event["type"],
            if action == "revoke" {
                "device.revoked"
            } else {
                "device.updated"
            }
        );
        for id in [id, other] {
            assert!(!f
                .gateway
                .identity
                .lock()
                .unwrap()
                .validate(&id, RP, Scope::View));
        }
        assert_eq!(
            f.gateway
                .identity
                .lock()
                .unwrap()
                .devices
                .contains_key(DEVICE),
            action != "revoke"
        );
    }
}
#[tokio::test]
async fn session_logout_real_loopback_validates_and_clears_cookie() {
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpStream,
        sync::oneshot,
    };
    let f = fixture();
    let (id, binding) = f
        .gateway
        .identity
        .lock()
        .unwrap()
        .create_for_test(DEVICE, RP)
        .unwrap();
    let root = tempfile::tempdir().unwrap();
    let server = crate::http::Server::bind(
        &Fixture::init().listeners.loopback,
        root.path(),
        f.gateway.clone(),
    )
    .await
    .unwrap();
    let port = server.port().unwrap();
    let (stop, stopped) = oneshot::channel();
    let task = tokio::spawn(server.run(stopped));
    for (extra, expected) in [
        (String::new(), 401),
        ("X-Yzc-Binding: bad\r\n".into(), 401),
        (
            format!("X-Yzc-Binding: {}\r\n", crate::secrets::encode(&[0; 32])),
            401,
        ),
        (
            format!("X-Yzc-Binding: {binding}\r\nX-Yzc-Binding: {binding}\r\n"),
            401,
        ),
        (format!("X-Yzc-Binding: {binding}\r\n"), 204),
        (format!("X-Yzc-Binding: {binding}\r\n"), 401),
    ] {
        let mut client = TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
            .await
            .unwrap();
        client.write_all(format!("POST /api/auth/logout HTTP/1.1\r\nHost: desktop.tailnet.ts.net:8443\r\nTailscale-User-Login: user@example.com\r\nOrigin: https://desktop.tailnet.ts.net:8443\r\nCookie: __Host-yzc={}\r\n{extra}Content-Length: 0\r\nConnection: close\r\n\r\n", crate::secrets::encode(&id)).as_bytes()).await.unwrap();
        let mut response = String::new();
        client.read_to_string(&mut response).await.unwrap();
        assert!(response.starts_with(&format!("HTTP/1.1 {expected}")));
        assert!(response.contains("cache-control: no-store"));
        if expected == 204 {
            assert!(response.contains(&format!("set-cookie: {}", clear_cookie())));
        } else {
            assert!(response.ends_with("Authentication required"));
        }
    }
    stop.send(()).unwrap();
    task.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn policy_injected_clock_closes_listener_and_all_connections() {
    use tokio::{io::AsyncReadExt, net::TcpStream, sync::oneshot};
    let mut init = Fixture::init();
    init.policy.enable_until = serde_json::from_str("\"2030-01-01T00:00:10Z\"").unwrap();
    let f = Fixture::with_init(init);
    let root = tempfile::tempdir().unwrap();
    let server = crate::http::Server::bind(
        &Fixture::init().listeners.loopback,
        root.path(),
        f.gateway.clone(),
    )
    .await
    .unwrap();
    let port = server.port().unwrap();
    let (_stop, stopped) = oneshot::channel();
    let task = tokio::spawn(server.run(stopped));
    let connecting = tokio::spawn(TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)));
    for _ in 0..10000 {
        if connecting.is_finished() {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(connecting.is_finished());
    let mut client = connecting.await.unwrap().unwrap();
    let remaining = (f.gateway.policy.enable_until.0 - f.gateway.clock.now().0)
        .to_std()
        .unwrap();
    tokio::time::advance(remaining).await;
    for _ in 0..10000 {
        if task.is_finished() {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(
        task.is_finished(),
        "policy expiry must not await another tick"
    );
    assert!(task.await.unwrap());
    assert_eq!(f.gateway.clock.now(), f.gateway.policy.enable_until);
    assert!(TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
        .await
        .is_err());
    let mut byte = [0];
    let read = client.read(&mut byte).await;
    assert!(
        matches!(read, Ok(0))
            || read.is_err_and(|e| matches!(
                e.kind(),
                std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted
            ))
    );
}

#[test]
fn session_cookie_skips_unrelated_bytes_and_concatenates_headers() {
    use axum::http::HeaderValue;
    let value = crate::secrets::encode(&[7; 32]);
    for unrelated in [
        b"nameless; =value".as_slice(),
        b"other=\xff",
        b"broken; another=ok",
    ] {
        let mut headers = HeaderMap::new();
        headers.append("cookie", HeaderValue::from_bytes(unrelated).unwrap());
        headers.append("cookie", format!("__Host-yzc={value}").parse().unwrap());
        assert_eq!(cookie_id(&headers), Some([7; 32]));
        headers.append("cookie", format!("__Host-yzc={value}").parse().unwrap());
        assert_eq!(cookie_id(&headers), None);
    }
    for target in [
        "__Host-yzc",
        "__Host-yzc=",
        "__Host-yzc=bad",
        "__Host-yzc=\"bad\"",
    ] {
        let mut headers = HeaderMap::new();
        headers.append("cookie", target.parse().unwrap());
        headers.append("cookie", format!("__Host-yzc={value}").parse().unwrap());
        assert_eq!(cookie_id(&headers), None);
    }
}

fn auth_headers(id: &[u8; 32], binding: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        "cookie",
        format!("__Host-yzc={}", crate::secrets::encode(id))
            .parse()
            .unwrap(),
    );
    headers.insert("x-yzc-binding", binding.parse().unwrap());
    headers
}
#[tokio::test]
async fn session_binding_exactly_one_header_hash_and_context_revalidation() {
    let f = fixture();
    let (id, binding) = f
        .gateway
        .identity
        .lock()
        .unwrap()
        .create_for_test(DEVICE, RP)
        .unwrap();
    let headers = auth_headers(&id, &binding);
    let ctx = f.gateway.authenticate(&headers, Scope::View).unwrap();
    assert_eq!(ctx.session_id(), &id);
    assert_eq!(ctx.device_id(), DEVICE);
    assert_eq!(ctx.rp_id(), RP);
    assert_eq!(
        f.gateway.identity.lock().unwrap().sessions[&id].binding_hash,
        crate::secrets::hash(&decode::<32>(&binding).unwrap())
    );
    for invalid in [
        None,
        Some("bad".into()),
        Some(crate::secrets::encode(&[0; 32])),
    ] {
        let mut h = headers.clone();
        h.remove("x-yzc-binding");
        if let Some(value) = invalid {
            h.insert("x-yzc-binding", value.parse().unwrap());
        }
        assert!(f.gateway.authenticate(&h, Scope::View).is_none());
    }
    let mut duplicate = headers.clone();
    duplicate.append("x-yzc-binding", binding.parse().unwrap());
    assert!(f.gateway.authenticate(&duplicate, Scope::View).is_none());
    let (other, _) = f
        .gateway
        .identity
        .lock()
        .unwrap()
        .create_for_test(DEVICE, RP)
        .unwrap();
    assert!(f
        .gateway
        .authenticate(&auth_headers(&other, &binding), Scope::View)
        .is_none());
    assert!(f
        .gateway
        .with_authorized(&ctx, Scope::Control, |_| ())
        .is_err());
    assert_eq!(f.gateway.with_authorized(&ctx, Scope::View, |_| 7), Ok(7));
    f.gateway
        .command(crate::admin::CommandBody::Device(DeviceCommand::Revoke {
            v: 1,
            device_id: DEVICE.into(),
        }))
        .unwrap();
    let mut wrote = false;
    assert_eq!(
        f.gateway.with_authorized(&ctx, Scope::View, |_| {
            wrote = true;
        }),
        Err((StatusCode::UNAUTHORIZED, "Authentication required"))
    );
    assert!(!wrote);
    assert!(f.gateway.logout(&ctx).is_err());
}
#[tokio::test(start_paused = true)]
async fn session_bound_maintenance_purges_expired_and_elevation_checks_rp() {
    let f = fixture();
    {
        let mut identity = f.gateway.identity.lock().unwrap();
        let (id, _) = identity.create_for_test(DEVICE, RP).unwrap();
        assert!(identity
            .elevate_for_test(&id, "other.ts.net", Scope::Control)
            .is_none());
        assert!(identity.elevate_for_test(&id, RP, Scope::Control).is_some());
        for _ in 1..identity.session_limit() {
            identity.create_for_test(DEVICE, RP).unwrap();
        }
        assert_eq!(identity.session_count(), 20);
        assert!(identity.create_for_test(DEVICE, RP).is_none());
    }
    tokio::time::advance(Duration::from_secs(1800)).await;
    f.gateway.maintenance();
    assert_eq!(f.gateway.identity.lock().unwrap().session_count(), 0);
    assert!(f
        .gateway
        .identity
        .lock()
        .unwrap()
        .create_for_test(DEVICE, RP)
        .is_some());
}
#[tokio::test]
async fn devices_tightening_and_neutral_commit_before_full_queue_then_notify() {
    for action in ["revoke", "label", "control", "short-control", "full"] {
        let mut f = fixture();
        let (id, _) = f
            .gateway
            .identity
            .lock()
            .unwrap()
            .create_for_test(DEVICE, RP)
            .unwrap();
        f.gateway
            .identity
            .lock()
            .unwrap()
            .elevate_for_test(&id, RP, Scope::Control)
            .unwrap();
        for _ in 0..64 {
            assert!(f.events.try_emit(Event::error("filler")));
        }
        let mut grants = f
            .gateway
            .identity
            .lock()
            .unwrap()
            .device(DEVICE)
            .unwrap()
            .grants
            .clone();
        if action == "control" {
            grants.control = None;
        }
        if action == "full" {
            grants.full = false;
        }
        let short_until = f.gateway.clock.now().after(Duration::from_secs(60));
        if action == "short-control" {
            grants.control.as_mut().unwrap().until = short_until;
        }
        let command = if action == "revoke" {
            DeviceCommand::Revoke {
                v: 1,
                device_id: DEVICE.into(),
            }
        } else {
            DeviceCommand::Update {
                v: 1,
                device_id: DEVICE.into(),
                label: (action == "label").then(|| "New label".into()),
                grants: (action != "label").then_some(grants),
            }
        };
        let result = f
            .gateway
            .command(crate::admin::CommandBody::Device(command))
            .unwrap();
        let event = result.deferred_event().expect("committed, NOT error");
        let value = serde_json::to_value(&event).unwrap();
        assert_eq!(
            value,
            serde_json::json!({"v":1,"type":"command.deferred","command":if action == "revoke" {"device.revoke"} else {"device.update"},"target":DEVICE,"committed":true})
        );
        {
            let mut identity = f.gateway.identity.lock().unwrap();
            assert!(!identity.validate(&id, RP, Scope::View));
            assert!(identity.sessions.is_empty());
            match action {
                "revoke" => assert!(identity.device(DEVICE).is_none()),
                "label" => assert_eq!(identity.device(DEVICE).unwrap().label, "New label"),
                "control" => assert!(identity.device(DEVICE).unwrap().grants.control.is_none()),
                "short-control" => assert_eq!(
                    identity
                        .device(DEVICE)
                        .unwrap()
                        .grants
                        .control
                        .as_ref()
                        .unwrap()
                        .until,
                    short_until
                ),
                _ => assert!(!identity.device(DEVICE).unwrap().grants.full),
            }
        }
        // Main uses lifecycle emit for the distinct deferred acknowledgement.
        let events = f.events.clone();
        let ack = tokio::spawn(async move { events.emit(event).await });
        for _ in 0..64 {
            assert_eq!(f.event().await["code"], "filler");
        }
        assert_eq!(f.event().await["type"], "command.deferred");
        assert!(ack.await.unwrap());
        f.gateway.maintenance();
        let delivered = f.event().await;
        assert_eq!(
            delivered["type"],
            if action == "revoke" {
                "device.revoked"
            } else {
                "device.updated"
            }
        );
        assert!(f.gateway.identity.lock().unwrap().notifications.is_empty());
    }
}
#[tokio::test]
async fn devices_mixed_deltas_rejected_widening_enqueue_first_and_deferred_coalescing() {
    let mut f = fixture();
    let original = f
        .gateway
        .identity
        .lock()
        .unwrap()
        .device(DEVICE)
        .unwrap()
        .grants
        .clone();
    let update = |grants| DeviceCommand::Update {
        v: 1,
        device_id: DEVICE.into(),
        label: None,
        grants: Some(grants),
    };
    let command = |cmd| f.gateway.command(crate::admin::CommandBody::Device(cmd));
    let mut mixed = original.clone();
    mixed.full = false;
    mixed.control.as_mut().unwrap().until = mixed
        .control
        .as_ref()
        .unwrap()
        .until
        .after(Duration::from_secs(1));
    assert_eq!(
        command(update(mixed)).unwrap_err().0,
        StatusCode::BAD_REQUEST
    );
    for _ in 0..64 {
        assert!(f.events.try_emit(Event::error("filler")));
    }
    let mut wider = original.clone();
    wider.control.as_mut().unwrap().until = wider
        .control
        .as_ref()
        .unwrap()
        .until
        .after(Duration::from_secs(1));
    assert_eq!(
        command(update(wider)).unwrap_err().0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        command(DeviceCommand::Unlock {
            v: 1,
            device_id: DEVICE.into()
        })
        .unwrap_err()
        .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    let reduced = crate::devices::Grants {
        control: None,
        full: false,
    };
    assert!(command(update(reduced)).unwrap().deferred_event().is_some());
    // Enqueue space later does not allow widening past a pending snapshot.
    for _ in 0..64 {
        f.event().await;
    }
    assert_eq!(
        f.gateway
            .command(crate::admin::CommandBody::Device(update(original)))
            .unwrap_err()
            .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    for _ in 0..64 {
        assert!(f.events.try_emit(Event::error("filler")));
    }
    let result = f
        .gateway
        .command(crate::admin::CommandBody::Device(DeviceCommand::Revoke {
            v: 1,
            device_id: DEVICE.into(),
        }))
        .unwrap();
    assert!(result.deferred_event().is_some());
    assert_eq!(f.gateway.identity.lock().unwrap().notifications.len(), 1);
    for _ in 0..64 {
        f.event().await;
    }
    f.gateway.maintenance();
    assert_eq!(f.event().await["type"], "device.revoked");
    assert!(f.gateway.identity.lock().unwrap().notifications.is_empty());
    // A sentinel proves no stale device.updated was queued after revoke.
    assert!(f.events.try_emit(Event::error("sentinel")));
    assert_eq!(f.event().await["code"], "sentinel");
}

use crate::test_support::authenticator;

fn verified_registration(f: &Fixture, byte: u8) -> crate::webauthn::VerifiedRegistration {
    let auth = authenticator::Authenticator::new("ES256", byte, false);
    let id = crate::secrets::encode(&[byte; 16]);
    let (state, options) = f.gateway.rp.registration(&id, "Phone", &[], true).unwrap();
    f.gateway
        .rp
        .verify_registration(
            state,
            auth.response(&options, true, 10, "valid"),
            f.gateway.policy.passkey_binding,
        )
        .unwrap()
}
fn registration_context(f: &Fixture) -> crate::pairing::Registration {
    crate::pairing::Registration {
        pairing_id: crate::secrets::encode(&[7; 16]),
        device_label: "Phone".into(),
        add_to_device: None,
        started_at: f.gateway.clock.now(),
        deadline: f.gateway.clock.now().after(Duration::from_secs(120)),
    }
}
#[tokio::test]
async fn webauthn_registration_enqueue_first_pending_tombstones_bounded() {
    let mut f = fixture();
    for _ in 0..64 {
        assert!(f.events.try_emit(Event::error("filler")));
    }
    let verified = verified_registration(&f, 7);
    assert_eq!(
        f.gateway
            .identity
            .lock()
            .unwrap()
            .register_verified(&registration_context(&f), verified, &f.events)
            .unwrap_err()
            .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(f.gateway.identity.lock().unwrap().devices().count(), 1);
    f.gateway
        .command(crate::admin::CommandBody::Device(DeviceCommand::Revoke {
            v: 1,
            device_id: DEVICE.into(),
        }))
        .unwrap();
    assert_eq!(f.gateway.identity.lock().unwrap().notifications.len(), 1);
    for _ in 0..64 {
        f.event().await;
    }
    // Even with queue room, do not put a registration ahead of a deferred revoke.
    assert_eq!(
        f.gateway
            .identity
            .lock()
            .unwrap()
            .register_verified(
                &registration_context(&f),
                verified_registration(&f, 7),
                &f.events
            )
            .unwrap_err()
            .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    f.gateway.maintenance();
    assert_eq!(f.event().await["type"], "device.revoked");
    assert!(f
        .gateway
        .identity
        .lock()
        .unwrap()
        .register_verified(
            &registration_context(&f),
            verified_registration(&f, 7),
            &f.events
        )
        .is_ok());
    assert_eq!(f.event().await["type"], "device.registered");
}
#[tokio::test]
async fn webauthn_lockout_and_counter_snapshots_commit_first_coalesce_full_queue() {
    let mut f = Fixture::new();
    let auth = authenticator::Authenticator::new("ES256", 7, false);
    let device = crate::secrets::encode(&[7; 16]);
    f.gateway
        .identity
        .lock()
        .unwrap()
        .register_verified(
            &registration_context(&f),
            verified_registration(&f, 7),
            &f.events,
        )
        .unwrap();
    f.event().await;
    let session = f
        .gateway
        .identity
        .lock()
        .unwrap()
        .create_for_test(&device, RP)
        .unwrap()
        .0;
    for _ in 0..64 {
        assert!(f.events.try_emit(Event::error("filler")));
    }
    for count in [11, 12] {
        let (state, mut options) = f.gateway.rp.login().unwrap();
        options["user"] = serde_json::json!({"id":device});
        let response = auth.response(&options, false, count, "valid");
        f.gateway
            .identity
            .lock()
            .unwrap()
            .finish_assertion(
                &f.gateway.rp,
                crate::webauthn::AssertionState::Login(state),
                response,
                None,
                &f.events,
            )
            .unwrap();
    }
    {
        let mut identity = f.gateway.identity.lock().unwrap();
        assert_eq!(
            identity.device(&device).unwrap().credentials[0].sign_count,
            12
        );
        assert_eq!(identity.notifications.len(), 1);
        for _ in 0..5 {
            identity.assertion_failed(&device, false, &f.events);
        }
        assert!(identity.device(&device).unwrap().locked);
        assert!(!identity.validate(&session, RP, Scope::View));
        assert_eq!(identity.session_count(), 0);
    }
    for _ in 0..64 {
        f.event().await;
    }
    f.gateway.maintenance();
    let event = f.event().await;
    assert_eq!(event["type"], "device.updated");
    assert_eq!(event["device"]["locked"], true);
    assert_eq!(event["device"]["credentials"][0]["signCount"], 12);
}
#[tokio::test(start_paused = true)]
async fn webauthn_global_session_cap_fixed_503_and_expired_purge() {
    let f = fixture();
    let auth = authenticator::Authenticator::new("ES256", 7, false);
    let device = crate::secrets::encode(&[7; 16]);
    f.gateway
        .identity
        .lock()
        .unwrap()
        .register_verified(
            &registration_context(&f),
            verified_registration(&f, 7),
            &f.events,
        )
        .unwrap();
    // cfg(test) constructor deliberately exercises the otherwise unreachable
    // global guard after production's per-device-four eviction invariant.
    {
        let mut identity = f.gateway.identity.lock().unwrap();
        for _ in 0..identity.session_limit() {
            identity.create_for_test(DEVICE, RP).unwrap();
        }
    }
    async fn post(port: u16, path: &str, body: serde_json::Value) -> (u16, String) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let request = format!("POST {path} HTTP/1.1\r\nHost: desktop.tailnet.ts.net:8443\r\nTailscale-User-Login: user@example.com\r\nOrigin: https://desktop.tailnet.ts.net:8443\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.to_string().len());
        let task = tokio::spawn(async move {
            let mut socket = tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
                .await
                .unwrap();
            socket.write_all(request.as_bytes()).await.unwrap();
            let mut response = String::new();
            socket.read_to_string(&mut response).await.unwrap();
            response
        });
        for _ in 0..100000 {
            if task.is_finished() {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(task.is_finished());
        let response = task.await.unwrap();
        assert!(response.contains("cache-control: no-store"));
        (
            response.split_whitespace().nth(1).unwrap().parse().unwrap(),
            response.split_once("\r\n\r\n").unwrap().1.into(),
        )
    }
    let root = tempfile::tempdir().unwrap();
    let server = crate::http::Server::bind(
        &Fixture::init().listeners.loopback,
        root.path(),
        f.gateway.clone(),
    )
    .await
    .unwrap();
    let port = server.port().unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(server.run(stopped));
    for (count, expected) in [(11, 503), (12, 200)] {
        if count == 12 {
            tokio::time::advance(Duration::from_secs(1800)).await;
        }
        let (status, options) = post(port, "/api/auth/options", serde_json::json!({})).await;
        assert_eq!(status, 200);
        let mut options: serde_json::Value = serde_json::from_str(&options).unwrap();
        options["user"] = serde_json::json!({"id":device});
        let response = auth.response(&options, false, count, "valid");
        let (status, body) = post(
            port,
            "/api/auth/finish",
            serde_json::json!({"credential":response}),
        )
        .await;
        assert_eq!(status, expected);
        if status == 503 {
            assert_eq!(body, "Session limit reached");
        }
    }
    assert_eq!(f.gateway.identity.lock().unwrap().session_count(), 1);
    stop.send(()).unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn webauthn_locked_device_internal_reentry_does_not_count_or_notify() {
    let mut f = fixture();
    {
        let mut identity = f.gateway.identity.lock().unwrap();
        for _ in 0..5 {
            identity.assertion_failed(DEVICE, false, &f.events);
        }
        assert!(identity.device(DEVICE).unwrap().locked);
        assert_eq!(identity.failures[DEVICE], 5);
    }
    assert_eq!(f.event().await["device"]["locked"], true);
    for rollback in [false, true] {
        let mut identity = f.gateway.identity.lock().unwrap();
        identity.assertion_failed(DEVICE, rollback, &f.events);
        assert_eq!(identity.failures[DEVICE], 5);
        assert!(identity.notifications.is_empty());
    }
    assert!(f.events.try_emit(Event::error("sentinel")));
    assert_eq!(f.event().await["code"], "sentinel");
}

#[tokio::test]
async fn webauthn_stale_elevation_preserves_nonzero_failure_count_and_replacement() {
    let f = Fixture::new();
    let auth = authenticator::Authenticator::new("ES256", 7, false);
    let device = crate::secrets::encode(&[7; 16]);
    f.gateway
        .identity
        .lock()
        .unwrap()
        .register_verified(
            &registration_context(&f),
            verified_registration(&f, 7),
            &f.events,
        )
        .unwrap();
    f.gateway
        .command(crate::admin::CommandBody::Device(DeviceCommand::Update {
            v: 1,
            device_id: device.clone(),
            label: None,
            grants: Some(crate::devices::Grants {
                control: Some(crate::devices::ControlGrant {
                    until: f.gateway.clock.now().after(Duration::from_secs(600)),
                }),
                full: false,
            }),
        }))
        .unwrap();
    let (id, binding) = f
        .gateway
        .identity
        .lock()
        .unwrap()
        .create_for_test(&device, RP)
        .unwrap();
    let ctx = f
        .gateway
        .authenticate(&auth_headers(&id, &binding), Scope::View)
        .unwrap();
    // Existing verified failures must neither increase nor reset on stale input.
    for _ in 0..4 {
        f.gateway
            .identity
            .lock()
            .unwrap()
            .assertion_failed(&device, false, &f.events);
    }
    let mut old = f.gateway.elevation_options(&ctx, Scope::Control).unwrap();
    old["user"] = serde_json::json!({"id":device});
    let mut new = f.gateway.elevation_options(&ctx, Scope::Control).unwrap();
    new["user"] = serde_json::json!({"id":device});
    for credential in [
        auth.response(&old, false, 11, "valid"),
        auth.response(&new, false, 11, "challenge"),
    ] {
        assert!(f
            .gateway
            .elevation_finish(
                &ctx,
                crate::webauthn::ElevationFinish {
                    scope: Scope::Control,
                    credential
                }
            )
            .is_err());
        assert_eq!(f.gateway.identity.lock().unwrap().failures[&device], 4);
    }
    assert!(f
        .gateway
        .elevation_finish(
            &ctx,
            crate::webauthn::ElevationFinish {
                scope: Scope::Control,
                credential: auth.response(&new, false, 11, "valid")
            }
        )
        .is_ok());
    assert!(!f
        .gateway
        .identity
        .lock()
        .unwrap()
        .failures
        .contains_key(&device));
}
