use super::*;
use crate::test_support::authenticator;
use crate::{
    admin::{CommandBody, DeviceCommand},
    test_support::{post, Fixture},
};
use authenticator::Authenticator;
use serde_json::json;
use std::time::Duration;

const DEVICE: &str = "AAAAAAAAAAAAAAAAAAAAAA";
const RP: &str = "desktop.tailnet.ts.net";

fn saved_fixture() -> Fixture {
    let mut init = Fixture::init();
    init.devices =
        vec![serde_json::from_str(include_str!("../tests/fixtures/device.json")).unwrap()];
    let rp = RelyingParty::new("desktop.tailnet.ts.net:8443").unwrap();
    let auth = Authenticator::new("ES256", 7, false);
    let (state, options) = rp.registration(DEVICE, "Phone", &[], true).unwrap();
    let (_, record) = rp
        .verify_registration(
            state,
            auth.response(&options, true, 10, "valid"),
            PasskeyBinding::O2,
        )
        .unwrap()
        .into_parts();
    init.devices[0].credentials = vec![record];
    init.policy.max_devices = 1;
    init.policy.full_session_enabled = true;
    Fixture::with_init(init)
}
fn session(f: &Fixture) -> (crate::session::AuthContext, String) {
    let (id, binding) = f
        .gateway
        .identity
        .lock()
        .unwrap()
        .create_for_test(DEVICE, RP)
        .unwrap();
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        "cookie",
        crate::session::session_cookie(&id).parse().unwrap(),
    );
    headers.insert("x-yzc-binding", binding.parse().unwrap());
    let ctx = f.gateway.authenticate(&headers, Scope::View).unwrap();
    (
        ctx,
        format!(
            "Cookie: {}\r\nX-Yzc-Binding: {binding}\r\n",
            crate::session::session_cookie(&id)
        ),
    )
}
async fn server(
    f: &Fixture,
) -> (
    tempfile::TempDir,
    u16,
    tokio::task::JoinHandle<bool>,
    tokio::sync::oneshot::Sender<()>,
) {
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
    (root, port, tokio::spawn(server.run(stopped)), stop)
}

// Only bypasses option filtering: verification still uses the O2 gateway policy.
// This entire file is loaded solely by webauthn.rs's #[cfg(test)] module.
fn unfiltered_elevation_for_test(
    f: &Fixture,
    ctx: &crate::session::AuthContext,
    scope: Scope,
) -> Value {
    let device = f
        .gateway
        .identity
        .lock()
        .unwrap()
        .device(DEVICE)
        .unwrap()
        .clone();
    let (state, options) = f
        .gateway
        .rp
        .elevation(&device.credentials, PasskeyBinding::O3, scope)
        .unwrap();
    let now = f.gateway.clock.now();
    f.gateway
        .ceremonies
        .lock()
        .unwrap()
        .insert(
            Ceremony {
                until: now.after(Duration::from_secs(120)),
                kind: CeremonyKind::Elevation {
                    session: *ctx.session_id(),
                    scope,
                    state,
                },
            },
            now,
        )
        .unwrap();
    options
}
#[tokio::test(start_paused = true)]
async fn webauthn_o2_full_independent_verifier_layers_loopback() {
    // A credential that cannot sync passes the same path: the 403 is the BE check.
    for (backup_eligible, expected) in [(true, 403), (false, 200)] {
        let auth = Authenticator::new("ES256", 7, backup_eligible);
        let rp = RelyingParty::new("desktop.tailnet.ts.net:8443").unwrap();
        let (state, options) = rp.registration(DEVICE, "Phone", &[], true).unwrap();
        let (_, record) = rp
            .verify_registration(
                state,
                auth.response(&options, true, 10, "eligible"),
                PasskeyBinding::O2,
            )
            .unwrap()
            .into_parts();
        let mut init = Fixture::init();
        let mut device: crate::devices::Device =
            serde_json::from_str(include_str!("../tests/fixtures/device.json")).unwrap();
        device.credentials = vec![record];
        init.devices = vec![device];
        init.policy.full_session_enabled = true;
        let f = Fixture::with_init(init);
        let (ctx, headers) = session(&f);
        let (_root, port, task, stop) = server(&f).await;
        let mut options = unfiltered_elevation_for_test(&f, &ctx, Scope::Full);
        assert_eq!(options["allowCredentials"][0]["id"], auth.id);
        options["user"] = json!({"id":DEVICE});
        let credential = auth.response(&options, false, 11, "eligible");
        let reply = post(
            port,
            "/api/auth/elevate/finish",
            json!({"scope":"full","credential":credential}),
            &headers,
        )
        .await;
        assert_eq!(
            reply.0, expected,
            "each O2 verifier layer must independently reject BE=1 for full: {}",
            reply.2
        );
        assert_eq!(
            f.gateway.with_authorized(&ctx, Scope::Full, |_| ()).is_ok(),
            !backup_eligible
        );
        stop.send(()).unwrap();
        task.await.unwrap();
    }
}

#[tokio::test(start_paused = true)]
async fn webauthn_elevation_prunes_inactive_sessions_and_bounds_pools() {
    let f = saved_fixture();
    let (_root, port, task, stop) = server(&f).await;
    let mut sessions = Vec::new();
    for _ in 0..4 {
        let (ctx, headers) = session(&f);
        assert_eq!(
            post(
                port,
                "/api/auth/elevate/options",
                json!({"scope":"control"}),
                &headers
            )
            .await
            .0,
            200
        );
        sessions.push((ctx, headers));
        tokio::time::advance(Duration::from_secs(1)).await;
    }
    assert_eq!(f.gateway.ceremonies.lock().unwrap().entries.len(), 4);
    // A real verified fifth login evicts the oldest session.
    let mut options = post(port, "/api/auth/options", json!({}), "").await.1;
    options["user"] = json!({"id":DEVICE});
    let auth = Authenticator::new("ES256", 7, false);
    let credential = auth.response(&options, false, 11, "valid");
    let login = post(
        port,
        "/api/auth/finish",
        json!({"credential":credential}),
        "",
    )
    .await;
    assert_eq!(login.0, 200);
    let cookie = login
        .2
        .lines()
        .find_map(|l| l.strip_prefix("set-cookie: "))
        .unwrap();
    let mut http_headers = axum::http::HeaderMap::new();
    http_headers.insert("cookie", cookie.parse().unwrap());
    http_headers.insert(
        "x-yzc-binding",
        login.1["binding"].as_str().unwrap().parse().unwrap(),
    );
    let replacement = f.gateway.authenticate(&http_headers, Scope::View).unwrap();
    let headers = format!(
        "Cookie: {cookie}\r\nX-Yzc-Binding: {}\r\n",
        login.1["binding"].as_str().unwrap()
    );
    assert_eq!(
        post(
            port,
            "/api/auth/elevate/options",
            json!({"scope":"full"}),
            &headers
        )
        .await
        .0,
        200
    );
    {
        let ceremonies = f.gateway.ceremonies.lock().unwrap();
        assert_eq!(ceremonies.entries.len(), 4);
        assert!(!ceremonies.entries.values().any(|c| matches!(&c.kind, CeremonyKind::Elevation { session, .. } if session == sessions[0].0.session_id())));
    }
    assert_eq!(
        post(
            port,
            "/api/auth/elevate/finish",
            json!({"scope":"control","credential":{}}),
            &sessions[0].1
        )
        .await
        .0,
        401
    );
    for _ in 0..60 {
        tokio::time::advance(Duration::from_millis(100)).await;
        assert_eq!(post(port, "/api/auth/options", json!({}), "").await.0, 200);
    }
    assert_eq!(f.gateway.ceremonies.lock().unwrap().entries.len(), 48 + 4);
    f.gateway
        .command(CommandBody::Device(DeviceCommand::Revoke {
            v: 1,
            device_id: DEVICE.into(),
        }))
        .unwrap();
    let (state, options) = f
        .gateway
        .rp
        .registration(DEVICE, "Phone", &[], true)
        .unwrap();
    let verified = f
        .gateway
        .rp
        .verify_registration(
            state,
            auth.response(&options, true, 10, "valid"),
            PasskeyBinding::O2,
        )
        .unwrap();
    let now = f.gateway.clock.now();
    let pairing = crate::pairing::Registration {
        pairing_id: DEVICE.into(),
        device_label: "Phone".into(),
        add_to_device: None,
        started_at: now,
        deadline: now.after(Duration::from_secs(120)),
    };
    f.gateway
        .identity
        .lock()
        .unwrap()
        .register_verified(&pairing, verified, &f.events)
        .unwrap();
    f.gateway
        .command(CommandBody::Device(DeviceCommand::Update {
            v: 1,
            device_id: DEVICE.into(),
            label: None,
            grants: Some(crate::devices::Grants {
                control: Some(crate::devices::ControlGrant {
                    until: now.after(Duration::from_secs(600)),
                }),
                full: false,
            }),
        }))
        .unwrap();
    let (_, headers) = session(&f);
    assert_eq!(
        post(
            port,
            "/api/auth/elevate/options",
            json!({"scope":"control"}),
            &headers
        )
        .await
        .0,
        200
    );
    {
        let ceremonies = f.gateway.ceremonies.lock().unwrap();
        assert_eq!(ceremonies.entries.len(), 48 + 1);
        assert!(!ceremonies.entries.values().any(|c| matches!(&c.kind, CeremonyKind::Elevation { session, .. } if session == replacement.session_id())));
    }
    stop.send(()).unwrap();
    task.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn webauthn_registration_capacity_failure_rotates_ticket_without_shortening_window() {
    let mut f = Fixture::new();
    let (id, secret) = f.create().await;
    let claim = f
        .gateway
        .pairings
        .claim(Fixture::claim_request(&id, &secret), "")
        .unwrap();
    f.gateway
        .pairings
        .command(crate::admin::PairingCommand::Approve {
            v: 1,
            pairing_id: id.clone(),
        })
        .unwrap();
    let status = || crate::pairing::StatusRequest {
        pairing_id: id.clone(),
        claim_token: claim.claim_token.clone(),
    };
    let ticket = f
        .gateway
        .pairings
        .status(status())
        .await
        .unwrap()
        .registration_ticket
        .unwrap();
    let now = f.gateway.clock.now();
    for byte in 0..16u8 {
        let (state, _) = f
            .gateway
            .rp
            .registration(DEVICE, "Phone", &[], true)
            .unwrap();
        f.gateway
            .ceremonies
            .lock()
            .unwrap()
            .insert(
                Ceremony {
                    until: now.after(Duration::from_secs(120)),
                    kind: CeremonyKind::Registration {
                        pairing: crate::pairing::Registration {
                            pairing_id: secrets::encode(&[byte; 16]),
                            device_label: "Phone".into(),
                            add_to_device: None,
                            started_at: now,
                            deadline: now.after(Duration::from_secs(120)),
                        },
                        device_id: DEVICE.into(),
                        state,
                    },
                },
                now,
            )
            .unwrap();
    }
    let (_root, port, task, stop) = server(&f).await;
    assert_eq!(
        post(
            port,
            "/api/pair/webauthn/options",
            json!({"pairingId":id,"registrationTicket":ticket}),
            ""
        )
        .await
        .0,
        429
    );
    let retry = f.gateway.pairings.status(status()).await.unwrap();
    assert_eq!(retry.state, crate::pairing::State::Approved);
    assert_ne!(retry.registration_ticket.as_ref().unwrap(), &ticket);
    tokio::time::advance(Duration::from_secs(121)).await;
    let retry = f.gateway.pairings.status(status()).await.unwrap();
    assert_eq!(
        retry.state,
        crate::pairing::State::Approved,
        "original approval has 600s, not failed ceremony's120s"
    );
    assert_eq!(
        post(
            port,
            "/api/pair/webauthn/options",
            json!({"pairingId":id,"registrationTicket":retry.registration_ticket}),
            ""
        )
        .await
        .0,
        200
    );
    stop.send(()).unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn webauthn_user_entity_preserves_label_or_omits_normalized_display_name() {
    let rp = RelyingParty::new("desktop.tailnet.ts.net:8443").unwrap();
    let auth = Authenticator::new("ES256", 7, false);
    for label in ["Phone", "手機", "Ｐｈｏｎｅ１２", "e\u{301}"] {
        assert!(crate::devices::valid_label(label));
        let (state, options) = rp.registration(DEVICE, label, &[], true).unwrap();
        assert_eq!(options["user"]["name"], "yuzora-companion");
        assert_eq!(
            options["user"]["displayName"],
            if matches!(label, "Phone" | "手機") {
                label
            } else {
                ""
            }
        );
        assert!(rp
            .verify_registration(
                state,
                auth.response(&options, true, 10, "valid"),
                PasskeyBinding::O2
            )
            .is_ok());
    }
}

#[test]
fn webauthn_challenge_mismatch_defensively_does_not_count_as_policy_failure() {
    assert!(matches!(
        AssertionFailure::from(AuthCeremonyErr::ChallengeMismatch),
        AssertionFailure::Invalid
    ));
}
