use super::*;
use crate::test_support::Fixture;

#[test]
fn pairing_sas_fixed_vector_and_constant_time_primitive() {
    assert_eq!(derive_sas(&[1; 32], &[2; 32], &[3; 32]), "918675");
    for byte in 0..=255u8 {
        let mut wrong = [8; 32];
        wrong[0] = byte;
        assert_eq!(
            secrets::matches(&secrets::hash(&[8; 32]), &wrong),
            byte == 8
        );
    }
    // A structural regression tripwire, not a claim of timing proof on a CPU.
    assert!(include_str!("secrets.rs").contains("expected.ct_eq(&hash(supplied))"));
}

#[tokio::test(start_paused = true)]
async fn pairing_ttl_single_claim_and_five_failure_void() {
    for scenario in ["ttl", "second", "failures", "malformed"] {
        let mut f = Fixture::new();
        let (id, secret) = f.create().await;
        match scenario {
            "ttl" => {
                tokio::time::advance(TTL).await;
                f.gateway.pairings.maintenance();
                assert_eq!(f.event().await["state"], "expired");
                assert!(f
                    .gateway
                    .pairings
                    .claim(Fixture::claim_request(&id, &secret), "")
                    .is_err());
            }
            "second" => {
                let first = f
                    .gateway
                    .pairings
                    .claim(Fixture::claim_request(&id, &secret), "")
                    .unwrap();
                assert_eq!(f.event().await["type"], "pairing.claimed");
                assert!(f
                    .gateway
                    .pairings
                    .claim(Fixture::claim_request(&id, &secret), "")
                    .is_err());
                assert_eq!(f.event().await["reason"], "second-claim");
                let status = f
                    .gateway
                    .pairings
                    .status(StatusRequest {
                        pairing_id: id,
                        claim_token: first.claim_token,
                    })
                    .await
                    .unwrap();
                assert_eq!(status.state, State::Rejected);
            }
            _ => {
                let wrong = if scenario == "malformed" {
                    "bad".into()
                } else {
                    secrets::encode(&[0; 32])
                };
                for i in 0..5 {
                    assert!(f
                        .gateway
                        .pairings
                        .claim(Fixture::claim_request(&id, &wrong), "")
                        .is_err());
                    assert_eq!(
                        f.gateway.pairings.entries.lock().unwrap()[&id].state,
                        if i == 4 {
                            State::Rejected
                        } else {
                            State::Pending
                        }
                    );
                }
                assert_eq!(f.event().await["reason"], "attempt-limit");
                assert!(f
                    .gateway
                    .pairings
                    .claim(Fixture::claim_request(&id, &secret), "")
                    .is_err());
            }
        }
    }
}

#[tokio::test]
async fn pairing_claim_validation_nonce_label_cap_lan_and_admin_decisions() {
    let mut f = Fixture::new();
    let error = f
        .gateway
        .pairings
        .command(PairingCommand::Create {
            v: 1,
            path: "lan".into(),
            add_to_device: None,
        })
        .unwrap_err();
    assert_eq!(error.1, "LAN pairing not supported");
    let (id, secret) = f.create().await;
    for (label, nonce) in [
        ("", secrets::encode(&[7; 32])),
        (" phone", secrets::encode(&[7; 32])),
        ("bad\n", secrets::encode(&[7; 32])),
        ("phone", "!".repeat(43)),
        ("phone", "a".repeat(500)),
    ] {
        let mut request = Fixture::claim_request(&id, &secret);
        request.device_label = label.into();
        request.device_nonce = nonce;
        assert!(f.gateway.pairings.claim(request, "").is_err());
    }
    for user_agent in ["x".repeat(257), "手機".into(), "bad\n".into()] {
        assert!(f
            .gateway
            .pairings
            .claim(Fixture::claim_request(&id, &secret), &user_agent)
            .is_err());
    }
    assert!(f
        .gateway
        .pairings
        .command(PairingCommand::Approve {
            v: 1,
            pairing_id: id.clone()
        })
        .is_err());
    for _ in 1..MAX_LIVE {
        f.create().await;
    }
    assert_eq!(
        f.gateway
            .pairings
            .command(PairingCommand::Create {
                v: 1,
                path: "tailscale".into(),
                add_to_device: None,
            })
            .unwrap_err(),
        BUSY
    );
    f.gateway
        .pairings
        .command(PairingCommand::Cancel {
            v: 1,
            pairing_id: id,
        })
        .unwrap();
    assert_eq!(f.event().await["reason"], "cancelled");
    f.create().await;
    for _ in 0..30 {
        let id = f
            .gateway
            .pairings
            .entries
            .lock()
            .unwrap()
            .iter()
            .find(|(_, e)| e.live())
            .unwrap()
            .0
            .clone();
        f.gateway
            .pairings
            .command(PairingCommand::Reject {
                v: 1,
                pairing_id: id,
            })
            .unwrap();
        f.event().await;
        f.create().await;
    }
    assert!(f.gateway.pairings.entries.lock().unwrap().len() <= MAX_RECORDS);
}

#[tokio::test(start_paused = true)]
async fn pairing_status_claim_token_required_bound_and_five_failures() {
    let mut f = Fixture::new();
    let (id, secret) = f.create().await;
    let a = f
        .gateway
        .pairings
        .claim(Fixture::claim_request(&id, &secret), "")
        .unwrap();
    f.event().await;
    let (other, secret) = f.create().await;
    let b = f
        .gateway
        .pairings
        .claim(Fixture::claim_request(&other, &secret), "")
        .unwrap();
    f.event().await;
    for token in [
        "".into(),
        a.claim_token,
        secrets::encode(&[0; 32]),
        "bad".into(),
        "wrong".into(),
    ] {
        assert!(f
            .gateway
            .pairings
            .status(StatusRequest {
                pairing_id: other.clone(),
                claim_token: token
            })
            .await
            .is_err());
    }
    assert_eq!(f.event().await["reason"], "attempt-limit");
    assert_eq!(
        f.gateway
            .pairings
            .status(StatusRequest {
                pairing_id: other,
                claim_token: b.claim_token
            })
            .await
            .unwrap()
            .state,
        State::Rejected
    );
}

#[tokio::test(start_paused = true)]
async fn pairing_status_wakes_approve_reject_expiry_timeout_and_poll_bounds() {
    for outcome in ["approve", "reject", "expiry", "timeout"] {
        let mut f = Fixture::new();
        let (id, secret) = f.create().await;
        let claim = f
            .gateway
            .pairings
            .claim(Fixture::claim_request(&id, &secret), "")
            .unwrap();
        f.event().await;
        if outcome == "expiry" {
            tokio::time::advance(Duration::from_secs(290)).await;
        }
        let gateway = f.gateway.clone();
        let request = StatusRequest {
            pairing_id: id.clone(),
            claim_token: claim.claim_token.clone(),
        };
        let poll = tokio::spawn(async move { gateway.pairings.status(request).await });
        tokio::task::yield_now().await;
        assert_eq!(
            f.gateway
                .pairings
                .status(StatusRequest {
                    pairing_id: id.clone(),
                    claim_token: claim.claim_token
                })
                .await
                .err(),
            Some(BUSY)
        );
        assert_eq!(f.gateway.pairings.polls.available_permits(), 7);
        match outcome {
            "approve" => f
                .gateway
                .pairings
                .command(PairingCommand::Approve {
                    v: 1,
                    pairing_id: id,
                })
                .map(|_| ())
                .unwrap(),
            "reject" => f
                .gateway
                .pairings
                .command(PairingCommand::Reject {
                    v: 1,
                    pairing_id: id,
                })
                .map(|_| ())
                .unwrap(),
            "expiry" => tokio::time::advance(Duration::from_secs(10)).await,
            _ => {
                tokio::time::advance(Duration::from_secs(24)).await;
                assert!(!poll.is_finished());
                tokio::time::advance(Duration::from_secs(1)).await;
            }
        }
        let response = poll.await.unwrap().unwrap();
        assert_eq!(
            response.state,
            match outcome {
                "approve" => State::Approved,
                "reject" => State::Rejected,
                "expiry" => State::Expired,
                _ => State::Pending,
            }
        );
        assert_eq!(response.registration_ticket.is_some(), outcome == "approve");
        assert_eq!(f.gateway.pairings.polls.available_permits(), 8);
    }
}

#[tokio::test(start_paused = true)]
async fn pairing_registration_ticket_rotation_single_use_ttl_window_and_binding() {
    for scenario in ["rotation", "ttl", "window", "consume"] {
        let mut f = Fixture::new();
        let (id, secret) = f.create().await;
        let claim = f
            .gateway
            .pairings
            .claim(Fixture::claim_request(&id, &secret), "")
            .unwrap();
        f.event().await;
        f.gateway
            .pairings
            .command(PairingCommand::Approve {
                v: 1,
                pairing_id: id.clone(),
            })
            .unwrap();
        let status = || StatusRequest {
            pairing_id: id.clone(),
            claim_token: claim.claim_token.clone(),
        };
        let first = f
            .gateway
            .pairings
            .status(status())
            .await
            .unwrap()
            .registration_ticket
            .unwrap();
        assert!(f
            .gateway
            .pairings
            .consume_ticket(&secrets::encode(&[8; 16]), &first)
            .is_err());
        match scenario {
            "rotation" => {
                let second = f
                    .gateway
                    .pairings
                    .status(status())
                    .await
                    .unwrap()
                    .registration_ticket
                    .unwrap();
                assert_ne!(first, second);
                assert!(f.gateway.pairings.consume_ticket(&id, &first).is_err());
                let registration = f.gateway.pairings.consume_ticket(&id, &second).unwrap();
                assert_eq!(registration.pairing_id, id);
                assert_eq!(registration.device_label, "Test phone");
                assert_eq!(
                    registration.deadline,
                    registration.started_at.after(TICKET_TTL)
                );
            }
            "ttl" => {
                tokio::time::advance(TICKET_TTL).await;
                assert!(f.gateway.pairings.consume_ticket(&id, &first).is_err());
                assert!(f
                    .gateway
                    .pairings
                    .status(status())
                    .await
                    .unwrap()
                    .registration_ticket
                    .is_some());
            }
            "window" => {
                tokio::time::advance(Duration::from_secs(599)).await;
                let last = f
                    .gateway
                    .pairings
                    .status(status())
                    .await
                    .unwrap()
                    .registration_ticket
                    .unwrap();
                tokio::time::advance(Duration::from_secs(1)).await;
                assert!(f.gateway.pairings.consume_ticket(&id, &last).is_err());
                let response = f.gateway.pairings.status(status()).await.unwrap();
                assert_eq!(response.state, State::Expired);
                assert!(response.registration_ticket.is_none());
            }
            _ => {
                f.gateway.pairings.consume_ticket(&id, &first).unwrap();
                assert!(f.gateway.pairings.consume_ticket(&id, &first).is_err());
                let response = f.gateway.pairings.status(status()).await.unwrap();
                assert_eq!(response.state, State::Registering);
                assert!(response.registration_ticket.is_none());
            }
        }
    }
}

#[tokio::test]
async fn pairing_queue_full_does_not_commit_claim_and_secrets_not_in_events() {
    let mut f = Fixture::new();
    let (id, secret) = f.create().await;
    for _ in 0..64 {
        assert!(f.events.try_emit(Event::error("test")));
    }
    let failure = f
        .gateway
        .pairings
        .claim(Fixture::claim_request(&id, &secret), "Test UA")
        .err()
        .unwrap();
    assert_eq!(failure.0, StatusCode::SERVICE_UNAVAILABLE);
    assert!(f.gateway.pairings.entries.lock().unwrap()[&id]
        .claim_hash
        .is_none());
    for _ in 0..64 {
        f.event().await;
    }
    let claim = f
        .gateway
        .pairings
        .claim(Fixture::claim_request(&id, &secret), "Test UA")
        .unwrap();
    let event = f.event().await;
    assert_eq!(event["sas"], claim.sas);
    assert_eq!(event["tailscaleLogin"], "user@example.com");
    assert_eq!(event["via"], "tailscale");
    let text = event.to_string();
    for forbidden in [&secret, &claim.claim_token, &claim.gateway_nonce] {
        assert!(!text.contains(forbidden));
    }
    assert!(f.gateway.pairings.entries.lock().unwrap()[&id]
        .secret
        .is_none());
}

#[tokio::test(start_paused = true)]
async fn pairing_http_long_poll_decisions_expiry_timeout_and_cancellation() {
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpStream,
        sync::oneshot,
    };
    for outcome in ["approve", "reject", "expiry", "timeout", "cancel-client"] {
        let mut f = Fixture::new();
        let (id, secret) = f.create().await;
        let claim = f
            .gateway
            .pairings
            .claim(Fixture::claim_request(&id, &secret), "")
            .unwrap();
        f.event().await;
        if outcome == "expiry" {
            tokio::time::advance(Duration::from_secs(290)).await;
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
        let (stop, stopped) = oneshot::channel();
        let server_task = tokio::spawn(server.run(stopped));
        let request =
            serde_json::json!({"pairingId":id,"claimToken":claim.claim_token}).to_string();
        let poll = tokio::spawn(async move {
            let mut client = TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
                .await
                .unwrap();
            client.write_all(format!("POST /api/pair/status HTTP/1.1\r\nHost: desktop.tailnet.ts.net:8443\r\nTailscale-User-Login: user@example.com\r\nOrigin: https://desktop.tailnet.ts.net:8443\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{request}", request.len()).as_bytes()).await.unwrap();
            let mut response = String::new();
            client.read_to_string(&mut response).await.unwrap();
            response
        });
        // Keep the paused runtime runnable while actual loopback IO settles.
        for _ in 0..10000 {
            if f.gateway.pairings.polls.available_permits() == 7 {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(f.gateway.pairings.polls.available_permits(), 7);
        match outcome {
            "approve" => f
                .gateway
                .pairings
                .command(PairingCommand::Approve {
                    v: 1,
                    pairing_id: id.clone(),
                })
                .map(|_| ())
                .unwrap(),
            "reject" => f
                .gateway
                .pairings
                .command(PairingCommand::Reject {
                    v: 1,
                    pairing_id: id.clone(),
                })
                .map(|_| ())
                .unwrap(),
            "expiry" => tokio::time::advance(Duration::from_secs(10)).await,
            "cancel-client" => poll.abort(),
            _ => {
                tokio::time::advance(Duration::from_secs(24)).await;
                assert!(!poll.is_finished());
                tokio::time::advance(Duration::from_secs(1)).await;
            }
        }
        if outcome != "cancel-client" {
            for _ in 0..10000 {
                if poll.is_finished() {
                    break;
                }
                tokio::task::yield_now().await;
            }
            assert!(poll.is_finished());
            let response = poll.await.unwrap();
            assert!(response.starts_with("HTTP/1.1 200"), "{response}");
            assert!(response.contains("cache-control: no-store"));
            let body: serde_json::Value =
                serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap();
            assert_eq!(
                body["state"],
                match outcome {
                    "approve" => "approved",
                    "reject" => "rejected",
                    "expiry" => "expired",
                    _ => "pending",
                }
            );
        } else {
            let _ = poll.await;
        }
        stop.send(()).unwrap();
        server_task.await.unwrap();
        assert_eq!(f.gateway.pairings.polls.available_permits(), 8);
    }
}

#[tokio::test(start_paused = true)]
async fn pairing_global_poll_cap_and_permits_released_after_cancellation() {
    let mut f = Fixture::new();
    let (id, secret) = f.create().await;
    let claim = f
        .gateway
        .pairings
        .claim(Fixture::claim_request(&id, &secret), "")
        .unwrap();
    f.event().await;
    let all = f
        .gateway
        .pairings
        .polls
        .clone()
        .acquire_many_owned(8)
        .await
        .unwrap();
    assert_eq!(
        f.gateway
            .pairings
            .status(StatusRequest {
                pairing_id: id.clone(),
                claim_token: claim.claim_token.clone()
            })
            .await
            .err(),
        Some(BUSY)
    );
    assert_eq!(
        f.gateway.pairings.entries.lock().unwrap()[&id]
            .polls
            .available_permits(),
        1
    );
    drop(all);
    let gateway = f.gateway.clone();
    let poll = tokio::spawn(async move {
        gateway
            .pairings
            .status(StatusRequest {
                pairing_id: id,
                claim_token: claim.claim_token,
            })
            .await
    });
    tokio::task::yield_now().await;
    assert_eq!(f.gateway.pairings.polls.available_permits(), 7);
    poll.abort();
    let _ = poll.await;
    assert_eq!(f.gateway.pairings.polls.available_permits(), 8);
}

#[tokio::test(start_paused = true)]
async fn pairing_approved_registering_cancellable_and_finish_atomic_stub() {
    for stage in ["approved", "registering"] {
        for outcome in ["cancel", "reject", "expiry", "finish"] {
            let mut f = Fixture::new();
            let (id, secret) = f.create().await;
            let claim = f
                .gateway
                .pairings
                .claim(Fixture::claim_request(&id, &secret), "")
                .unwrap();
            f.event().await;
            f.gateway
                .pairings
                .command(PairingCommand::Approve {
                    v: 1,
                    pairing_id: id.clone(),
                })
                .unwrap();
            f.event().await;
            let request = || StatusRequest {
                pairing_id: id.clone(),
                claim_token: claim.claim_token.clone(),
            };
            let ticket = f
                .gateway
                .pairings
                .status(request())
                .await
                .unwrap()
                .registration_ticket
                .unwrap();
            let registration = if stage == "registering" {
                // Consume near the end of the window to exercise the min cap.
                tokio::time::advance(Duration::from_secs(550)).await;
                let ticket = f
                    .gateway
                    .pairings
                    .status(request())
                    .await
                    .unwrap()
                    .registration_ticket
                    .unwrap();
                let ctx = f.gateway.pairings.consume_ticket(&id, &ticket).unwrap();
                assert_eq!(ctx.deadline, ctx.started_at.after(Duration::from_secs(50)));
                let status = f.gateway.pairings.status(request()).await.unwrap();
                assert_eq!(status.state, State::Registering);
                assert!(status.registration_ticket.is_none());
                Some(ctx)
            } else {
                None
            };
            let expected = match outcome {
                "cancel" | "reject" => {
                    let cmd = if outcome == "cancel" {
                        PairingCommand::Cancel {
                            v: 1,
                            pairing_id: id.clone(),
                        }
                    } else {
                        PairingCommand::Reject {
                            v: 1,
                            pairing_id: id.clone(),
                        }
                    };
                    f.gateway.pairings.command(cmd).unwrap();
                    State::Rejected
                }
                "expiry" => {
                    tokio::time::advance(if stage == "registering" {
                        Duration::from_secs(50)
                    } else {
                        REGISTRATION_WINDOW
                    })
                    .await;
                    f.gateway.maintenance();
                    State::Expired
                }
                _ => {
                    let ctx = registration.as_ref().cloned().unwrap_or_else(|| {
                        f.gateway.pairings.consume_ticket(&id, &ticket).unwrap()
                    });
                    f.gateway.pairings.complete_registration(&ctx).unwrap();
                    assert!(f.gateway.pairings.complete_registration(&ctx).is_err());
                    State::Registered
                }
            };
            if outcome != "finish" {
                if let Some(ctx) = &registration {
                    assert!(f.gateway.pairings.complete_registration(ctx).is_err());
                }
            }
            assert!(f.gateway.pairings.consume_ticket(&id, &ticket).is_err());
            let status = f.gateway.pairings.status(request()).await.unwrap();
            assert_eq!(status.state, expected);
            assert!(status.registration_ticket.is_none());
            let entries = f.gateway.pairings.entries.lock().unwrap();
            assert!(entries[&id].ticket.is_none());
            assert!(entries[&id].registration.is_none());
        }
    }
}
#[tokio::test]
async fn pairing_queue_full_cancel_reject_and_security_voids_retry_notifications() {
    for outcome in [
        "cancel",
        "reject",
        "second-wrong-secret",
        "attempt-limit",
        "expiry",
    ] {
        let mut f = Fixture::new();
        let (id, secret) = f.create().await;
        let claim = f
            .gateway
            .pairings
            .claim(Fixture::claim_request(&id, &secret), "")
            .unwrap();
        f.event().await;
        f.gateway
            .pairings
            .command(PairingCommand::Approve {
                v: 1,
                pairing_id: id.clone(),
            })
            .unwrap();
        f.event().await;
        let request = || StatusRequest {
            pairing_id: id.clone(),
            claim_token: claim.claim_token.clone(),
        };
        let ticket = f
            .gateway
            .pairings
            .status(request())
            .await
            .unwrap()
            .registration_ticket
            .unwrap();
        for _ in 0..64 {
            assert!(f.events.try_emit(Event::error("filler")));
        }
        match outcome {
            "cancel" | "reject" => {
                let cmd = if outcome == "cancel" {
                    PairingCommand::Cancel {
                        v: 1,
                        pairing_id: id.clone(),
                    }
                } else {
                    PairingCommand::Reject {
                        v: 1,
                        pairing_id: id.clone(),
                    }
                };
                let result = f.gateway.pairings.command(cmd).unwrap();
                let event = serde_json::to_value(result.deferred_event().unwrap()).unwrap();
                assert_eq!(event["type"], "command.deferred");
                assert_eq!(event["committed"], true);
                assert_eq!(event["target"], id);
            }
            "second-wrong-secret" => {
                assert!(f
                    .gateway
                    .pairings
                    .claim(Fixture::claim_request(&id, &secrets::encode(&[0; 32])), "")
                    .is_err());
            }
            "attempt-limit" => {
                for _ in 0..5 {
                    assert!(f
                        .gateway
                        .pairings
                        .status(StatusRequest {
                            pairing_id: id.clone(),
                            claim_token: "wrong".into()
                        })
                        .await
                        .is_err());
                }
            }
            _ => {
                f.gateway
                    .pairings
                    .entries
                    .lock()
                    .unwrap()
                    .get_mut(&id)
                    .unwrap()
                    .until = f.gateway.clock.now();
                f.gateway.maintenance();
            }
        }
        {
            let entries = f.gateway.pairings.entries.lock().unwrap();
            let entry = &entries[&id];
            assert!(!entry.live());
            assert!(entry.ticket.is_none());
            assert!(entry.notification.is_some());
        }
        assert!(f.gateway.pairings.consume_ticket(&id, &ticket).is_err());
        for _ in 0..64 {
            assert_eq!(f.event().await["code"], "filler");
        }
        f.gateway.maintenance();
        assert_eq!(
            f.event().await["reason"],
            match outcome {
                "cancel" => "cancelled",
                "second-wrong-secret" => "second-claim",
                "expiry" => "ttl",
                "reject" => "rejected",
                _ => "attempt-limit",
            }
        );
        assert!(f.gateway.pairings.entries.lock().unwrap()[&id]
            .notification
            .is_none());
        assert_ne!(
            f.gateway.pairings.status(request()).await.unwrap().state,
            State::Approved
        );
    }
}
#[tokio::test]
async fn pairing_approve_rechecks_entry_expiry_at_decision_lock() {
    let mut f = Fixture::new();
    let (id, secret) = f.create().await;
    f.gateway
        .pairings
        .claim(Fixture::claim_request(&id, &secret), "")
        .unwrap();
    f.event().await;
    // Deterministic seam: maintenance observed a live entry, then time reaches
    // its deadline before the under-lock decision (no scheduling race needed).
    f.gateway.pairings.maintenance();
    let mut entries = f.gateway.pairings.entries.lock().unwrap();
    let entry = entries.get_mut(&id).unwrap();
    assert_eq!(
        Pairings::decide(
            entry,
            &id,
            State::Approved,
            "approved",
            entry.until,
            &f.events
        ),
        Err(INVALID)
    );
    assert_eq!(entry.state, State::Expired);
    assert!(entry.ticket.is_none());
}

#[tokio::test(start_paused = true)]
async fn pairing_registration_retry_never_resurrects_cancelled_or_expired() {
    for expired in [false, true] {
        let mut f = Fixture::new();
        let (id, secret) = f.create().await;
        let claim = f
            .gateway
            .pairings
            .claim(Fixture::claim_request(&id, &secret), "")
            .unwrap();
        f.gateway
            .pairings
            .command(PairingCommand::Approve {
                v: 1,
                pairing_id: id.clone(),
            })
            .unwrap();
        let status = || StatusRequest {
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
        let registration = f.gateway.pairings.consume_ticket(&id, &ticket).unwrap();
        if expired {
            tokio::time::advance(TICKET_TTL).await;
        } else {
            f.gateway
                .pairings
                .command(PairingCommand::Cancel {
                    v: 1,
                    pairing_id: id.clone(),
                })
                .unwrap();
        }
        f.gateway
            .pairings
            .retry_registration(&registration)
            .unwrap();
        let status = f.gateway.pairings.status(status()).await.unwrap();
        assert_eq!(
            status.state,
            if expired {
                State::Expired
            } else {
                State::Rejected
            }
        );
        assert!(status.registration_ticket.is_none());
    }
}
