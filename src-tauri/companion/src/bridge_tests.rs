use super::*;
use crate::test_support::Fixture;

fn authenticated_fixture() -> (Fixture, AuthContext) {
    authenticated_fixture_with(|_| {})
}

fn authenticated_fixture_with(
    configure: impl FnOnce(&mut crate::admin::Init),
) -> (Fixture, AuthContext) {
    let mut init = Fixture::init();
    configure(&mut init);
    init.devices =
        vec![serde_json::from_str(include_str!("../tests/fixtures/device.json")).unwrap()];
    let f = Fixture::with_init(init);
    let (id, binding) = f
        .gateway
        .identity
        .lock()
        .unwrap()
        .create_for_test("AAAAAAAAAAAAAAAAAAAAAA", "desktop.tailnet.ts.net")
        .unwrap();
    let mut headers = HeaderMap::new();
    headers.insert(
        "cookie",
        crate::session::session_cookie(&id).parse().unwrap(),
    );
    headers.insert("x-yzc-binding", binding.parse().unwrap());
    let ctx = f.gateway.authenticate(&headers, Scope::View).unwrap();
    (f, ctx)
}

#[tokio::test]
async fn bridge_allowlist_schema_request_methods_and_explicit_deny_list() {
    let schema: Value =
        serde_json::from_str(include_str!("../tests/fixtures/herdr-0.9.5-methods.json")).unwrap();
    assert_eq!(schema["herdrVersion"], "0.9.5");
    assert_eq!(schema["protocol"], 22);
    let methods = schema["methods"].as_array().unwrap();
    // 129 was a text regex count mixing methods with 26 event definitions.
    assert_eq!(methods.len(), 102);
    assert_eq!(schema["events"].as_array().unwrap().len(), 26);
    assert_eq!(schema["subscriptionEvents"].as_array().unwrap().len(), 3);
    let denied = [
        "server.*",
        "plugin.*",
        "integration.*",
        "command.invoke",
        "worktree.remove",
        "workspace.close",
        "tab.close",
        "pane.close",
        "pane.clear",
        "layout.apply",
        "client_shell.*",
        "client.window_title.*",
        "notification.show",
        "popup.*",
        "pane.report_*",
        "pane.clear_agent_authority",
        "pane.release_agent",
    ];
    let (f, ctx) = authenticated_fixture();
    for pattern in denied {
        let matches = |name: &str| {
            pattern
                .strip_suffix('*')
                .map_or(name == pattern, |prefix| name.starts_with(prefix))
        };
        assert!(
            methods.iter().any(|name| matches(name.as_str().unwrap())),
            "stale deny rule {pattern}"
        );
        assert!(
            !METHODS.iter().any(|method| matches(method.name)),
            "forbidden gateway method {pattern}"
        );
        for name in methods
            .iter()
            .filter(|name| matches(name.as_str().unwrap()))
        {
            let response = rpc(
                &f.gateway,
                &ctx,
                &json!({"id":1,"method":name,"params":{}}).to_string(),
            )
            .unwrap_or_else(|_| panic!("valid view context"));
            assert_eq!(response["error"]["code"], "unknown-method", "{name}");
        }
    }
    // This is NOT a Herdr mapping table: 3B must implement explicit mappings.
    // Every schema name outside the closed gateway inventory is already refused;
    // coincident names are scope checked placeholders, never raw API forwarding.
    for name in methods {
        let response = rpc(
            &f.gateway,
            &ctx,
            &json!({"id":1,"method":name,"params":{}}).to_string(),
        )
        .unwrap_or_else(|_| panic!("valid view context"));
        assert_eq!(
            response["error"]["code"],
            match name.as_str().unwrap() {
                "events.subscribe" => "not-implemented",
                "agent.prompt" => "elevation-required",
                _ => "unknown-method",
            }
        );
    }
    for name in schema["events"]
        .as_array()
        .unwrap()
        .iter()
        .chain(schema["subscriptionEvents"].as_array().unwrap())
    {
        assert!(!METHODS.iter().any(|method| name == method.name));
    }
    let expected = [
        ("hello", Scope::View),
        ("session.ping", Scope::View),
        ("sessions.list", Scope::View),
        ("snapshot.get", Scope::View),
        ("events.subscribe", Scope::View),
        ("terminal.observe", Scope::View),
        ("stream.close", Scope::View),
        ("push.subscribe", Scope::View),
        ("push.unsubscribe", Scope::View),
        ("push.status", Scope::View),
        ("git.status", Scope::View),
        ("git.branches", Scope::View),
        ("git.log", Scope::View),
        ("git.commit", Scope::View),
        ("git.diff", Scope::View),
        ("terminal.control", Scope::Control),
        ("terminal.input", Scope::Control),
        ("terminal.scroll", Scope::Control),
        ("terminal.release", Scope::Control),
        ("agent.prompt", Scope::Control),
        ("pane.sendKeys", Scope::Control),
        ("full.open", Scope::Full),
        ("full.input", Scope::Full),
        ("full.resize", Scope::Full),
        ("full.close", Scope::Full),
    ];
    assert_eq!(
        METHODS
            .iter()
            .map(|m| (m.name, m.scope))
            .collect::<Vec<_>>(),
        expected
    );
}

#[tokio::test]
async fn bridge_hello_advertises_push_and_push_methods_answer_and_notify_the_desktop() {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    let key = URL_SAFE_NO_PAD.encode([&[4u8][..], &[7u8; 64]].concat());
    let (mut f, ctx) = authenticated_fixture_with(|init| {
        init.push = Some(crate::admin::PushInit {
            vapid_public_key: key.clone(),
            devices: Vec::new(),
        });
    });
    let gateway = f.gateway.clone();
    let call = |method: &str, params: Value| {
        rpc(
            &gateway,
            &ctx,
            &json!({"id":5,"method":method,"params":params}).to_string(),
        )
        .unwrap_or_else(|_| panic!("valid view context"))
    };
    assert_eq!(
        call("hello", json!({}))["result"]["push"],
        json!({"vapidPublicKey":key})
    );
    let endpoint = "https://updates.push.services.mozilla.com/wpush/v2/abc";
    let status = json!({"endpoint":endpoint});
    assert_eq!(
        call("push.status", status.clone()),
        json!({"id":5,"result":{"available":true,"subscribed":false}})
    );
    // `push.subscribe` waits for the desktop's ack, so the read loop answers it
    // (see the push tests in read_bridge_tests). Here: the desktop stored it
    // and says so: on for this browser, off for another.
    let id = crate::secrets::encode(&crate::secrets::hash(endpoint.as_bytes()));
    let stored = serde_json::from_value(json!({"v":1,"type":"push.devices","devices":[
        {"deviceId":"AAAAAAAAAAAAAAAAAAAAAA","endpoints":[id]}]}))
    .unwrap();
    gateway
        .command(crate::admin::CommandBody::Push(stored))
        .unwrap();
    assert_eq!(
        call("push.status", status.clone()),
        json!({"id":5,"result":{"available":true,"subscribed":true}})
    );
    assert_eq!(
        call(
            "push.status",
            json!({"endpoint":"https://fcm.googleapis.com/x"})
        ),
        json!({"id":5,"result":{"available":true,"subscribed":false}})
    );
    assert_eq!(
        call("push.status", json!({})),
        json!({"id":5,"result":{"available":true,"subscribed":false}})
    );
    assert_eq!(
        call("push.unsubscribe", status),
        json!({"id":5,"result":{"subscribed":false}})
    );
    assert_eq!(
        f.event().await,
        json!({"v":1,"type":"push.subscription","deviceId":"AAAAAAAAAAAAAAAAAAAAAA",
            "subscription":null,"endpointId":id})
    );
    for (method, params) in [
        ("push.unsubscribe", Value::Null),
        ("push.unsubscribe", json!({})),
        ("push.unsubscribe", json!({"endpoint":null})),
        ("push.status", json!({"x":1})),
        ("push.status", json!({"endpoint":5})),
    ] {
        assert_eq!(
            call(method, params)["error"]["code"],
            "invalid-params",
            "{method}"
        );
    }
}

#[tokio::test]
async fn bridge_push_methods_are_unavailable_when_init_has_no_push() {
    let (f, ctx) = authenticated_fixture();
    let call = |method: &str, params: Value| {
        rpc(
            &f.gateway,
            &ctx,
            &json!({"id":6,"method":method,"params":params}).to_string(),
        )
        .unwrap_or_else(|_| panic!("valid view context"))
    };
    assert_eq!(call("hello", json!({}))["result"]["push"], Value::Null);
    assert_eq!(
        call("push.status", json!({})),
        json!({"id":6,"result":{"available":false,"subscribed":false}})
    );
    assert_eq!(
        call("push.unsubscribe", json!({}))["error"]["code"],
        "push-unavailable"
    );
}

#[tokio::test]
async fn ws_slow_consumer_byte_budget_reclaimed_and_loopback_close_1013() {
    let (f, ctx) = authenticated_fixture();
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        let (outbound, mut pending) = Outbound::new();
        let payload = Message::binary(vec![0; MESSAGE_BYTES]);
        let mut count = 0;
        while outbound.send(payload.clone()).is_ok() {
            count += 1;
        }
        assert_eq!(
            count,
            (OUTBOUND_BYTES - WRITE_BUFFER_BYTES - REPLY_HEADROOM) / (MESSAGE_BYTES + 16)
                + REPLY_HEADROOM / (MESSAGE_BYTES + 16)
        );
        assert_eq!(
            outbound.bytes.available_permits() + outbound.reply_bytes.available_permits(),
            OUTBOUND_BYTES - WRITE_BUFFER_BYTES - count * (MESSAGE_BYTES + 16)
        );
        // Dequeue alone does NOT return the in-flight budget; completing/dropping
        // the writer's owned permit does. A stalled writer is simulated by holding it.
        let in_flight = pending.recv().await.unwrap();
        assert!(outbound.send(payload.clone()).is_err());
        drop(in_flight);
        assert!(outbound.send(payload).is_ok());
        // Also hit the bounded frame queue (small frames use little byte budget).
        // No writer drains it before drive; same code path as a blocked TCP sink.
        while outbound.send(Message::text("small")).is_ok() {}
        let (_stop, mut stop) = watch::channel(0u16);
        let (_invalid, mut invalid) = watch::channel(false);
        let (close, _) = drive(
            ws,
            &f.gateway,
            &ctx,
            0,
            &mut stop,
            &mut invalid,
            outbound,
            pending,
        )
        .await;
        assert_eq!(close.code, 1013);
        assert_eq!(close.reason, "slow-consumer");
    });
    let stream = tokio::net::TcpStream::connect(address).await.unwrap();
    let (mut client, _) = tokio_tungstenite::client_async(format!("ws://{address}/"), stream)
        .await
        .unwrap();
    let received = tokio::time::timeout(Duration::from_secs(2), client.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(matches!(received, Message::Close(Some(frame)) if u16::from(frame.code) == 1013));
    server.await.unwrap();
}

#[tokio::test]
async fn ws_registry_subscription_race_queue_pressure_and_unregister_are_bounded() {
    let (mut f, ctx) = authenticated_fixture();
    let (id, mut invalidated) = f
        .gateway
        .with_authorized(&ctx, Scope::View, |identity| identity.register_peer(&ctx))
        .unwrap()
        .unwrap();
    for _ in 0..64 {
        assert!(f.events.try_emit(Event::error("filler")));
    }
    let result = f
        .gateway
        .command(crate::admin::CommandBody::Device(
            crate::admin::DeviceCommand::Revoke {
                v: 1,
                device_id: ctx.device_id().into(),
            },
        ))
        .unwrap();
    assert!(result.deferred_event().is_some());
    // Notification does not depend on stdout queue space OR on polling a new RPC.
    assert!(*invalidated.borrow());
    invalidated.changed().await.unwrap();
    assert!(f
        .gateway
        .with_authorized(&ctx, Scope::View, |identity| identity.register_peer(&ctx))
        .is_err());
    f.gateway.identity.lock().unwrap().unregister_peer(id);
    assert!(invalidated.changed().await.is_err());
    for _ in 0..64 {
        f.event().await;
    }
}

#[tokio::test(start_paused = true)]
async fn ws_write_stall_twenty_seconds_closes_1013_and_emits_slow_consumer() {
    let (mut f, ctx) = authenticated_fixture();
    let (id, mut invalidated) = f
        .gateway
        .with_authorized(&ctx, Scope::View, |identity| identity.register_peer(&ctx))
        .unwrap()
        .unwrap();
    let mut peer = Peer {
        gateway: f.gateway.clone(),
        id,
        device: ctx.device_id().into(),
        reason: "aborted",
    };
    // A real bounded transport, not a paused writer or a pre-filled full queue.
    // 1 KiB cannot absorb even the first 64 KiB frame while the client won't read.
    let (server_io, client_io) = tokio::io::duplex(1024);
    let server = WebSocketStream::from_raw_socket(
        server_io,
        Role::Server,
        Some(
            WebSocketConfig::default()
                .write_buffer_size(0)
                .max_write_buffer_size(WRITE_BUFFER_BYTES),
        ),
    )
    .await;
    let mut client = WebSocketStream::from_raw_socket(client_io, Role::Client, None).await;
    let (outbound, pending) = Outbound::new();
    let budget = outbound.bytes.clone();
    for _ in 0..3 {
        assert!(outbound
            .send(Message::binary(vec![0; MESSAGE_BYTES]))
            .is_ok());
    }
    let (_stop, mut stop) = watch::channel(0u16);
    let started = tokio::time::Instant::now();
    let gateway = f.gateway.clone();
    let driving = drive(
        server,
        &gateway,
        &ctx,
        0,
        &mut stop,
        &mut invalidated,
        outbound,
        pending,
    );
    tokio::pin!(driving);
    assert!(futures_util::poll!(&mut driving).is_pending());
    tokio::time::advance(Duration::from_secs(19)).await;
    assert!(futures_util::poll!(&mut driving).is_pending());
    assert!(budget.available_permits() < OUTBOUND_BYTES - WRITE_BUFFER_BYTES - REPLY_HEADROOM);
    tokio::time::advance(Duration::from_secs(1)).await;
    // Poll timeout before releasing backpressure; close now has its own 250ms
    // budget. A still-unread peer cannot be guaranteed the wire Close.
    assert!(futures_util::poll!(&mut driving).is_pending());
    let draining = async {
        let mut binary = 0;
        loop {
            match client.next().await.unwrap().unwrap() {
                Message::Binary(bytes) => {
                    assert_eq!(bytes.len(), MESSAGE_BYTES);
                    binary += 1;
                }
                Message::Close(Some(frame)) => {
                    assert_eq!(u16::from(frame.code), 1013);
                    break;
                }
                other => panic!("unexpected stalled-writer output: {other:?}"),
            }
        }
        assert_eq!(
            binary, 1,
            "only the in-flight frame, never queued frames, may drain"
        );
    };
    let ((close, _), ()) = tokio::join!(driving, draining);
    assert_eq!(close.code, 1013);
    assert_eq!(close.reason, "slow-consumer");
    assert_eq!(started.elapsed(), Duration::from_secs(20));
    assert_eq!(
        budget.available_permits(),
        OUTBOUND_BYTES - WRITE_BUFFER_BYTES - REPLY_HEADROOM
    );
    peer.reason = close.reason;
    drop(peer);
    let closed = f.event().await;
    assert_eq!(closed["type"], "session.closed");
    assert_eq!(closed["reason"], "slow-consumer");
    assert_eq!(closed["peerId"], id);
}

#[tokio::test]
async fn ws_preauth_cookie_filter_never_takes_permit_or_queues_upgrade() {
    use tungstenite::client::IntoClientRequest;
    let (f, ctx) = authenticated_fixture();
    let (upgrades, mut pending) = Upgrades::new();
    let valid = crate::session::session_cookie(ctx.session_id());
    for cookie in [
        None,
        Some("__Host-yzc=bad".into()),
        Some(crate::session::session_cookie(&[0; 32])),
        Some(format!("{valid}; {valid}")),
    ] {
        let request = "ws://desktop.tailnet.ts.net:8443/ws"
            .into_client_request()
            .unwrap();
        let mut request = request.map(|_| Body::empty());
        if let Some(cookie) = cookie {
            request
                .headers_mut()
                .insert("cookie", cookie.parse().unwrap());
        }
        assert_eq!(
            upgrades.respond(request, &f.gateway).status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(upgrades.permits.available_permits(), MAX_PEERS);
        assert!(pending.try_recv().is_err());
    }
}

#[tokio::test]
async fn bridge_snapshot_batch_reserves_all_bytes_and_one_chunked_reply() {
    let (outbound, mut pending) = Outbound::new();
    // Eight active streams can retain their complete384KiB budgets. The RPC
    // does not get to partially enqueue2MiB and turn a healthy peer into1013.
    let streams = outbound
        .bytes
        .clone()
        .try_acquire_many_owned(8 * 384 * 1024)
        .unwrap();
    let batch = || {
        let mut v = vec![json!({"id":1,"result":{"chunked":true}})];
        for n in 0..56 {
            v.push(json!({"event":"snapshot.chunk","data":{"index":n,"data":"x".repeat(48*1024)}}));
        }
        v
    };
    let available = outbound.bytes.available_permits();
    assert_eq!(outbound.batch(batch()), Err("busy"));
    assert!(pending.try_recv().is_err());
    assert_eq!(outbound.bytes.available_permits(), available);
    assert!(outbound.replies(batch()).is_ok());
    let busy = pending.try_recv().unwrap();
    let Message::Text(text) = &busy.message else {
        panic!("text")
    };
    assert_eq!(
        serde_json::from_str::<Value>(text).unwrap()["error"]["code"],
        "busy"
    );
    drop(busy);
    drop(streams);
    assert!(outbound.batch(batch()).is_ok());
    assert_eq!(outbound.batch(batch()), Err("busy"));
    let mut held = Vec::new();
    while let Ok(v) = pending.try_recv() {
        held.push(v);
    }
    assert_eq!(outbound.chunked.available_permits(), 0);
    drop(held);
    assert_eq!(outbound.chunked.available_permits(), 1);
    assert!(outbound.batch(batch()).is_ok());
}
#[tokio::test]
async fn bridge_absolute_herdr_binary_required_before_any_io() {
    let mut init = Fixture::init();
    let (sender, _writer) = crate::admin::spawn_writer(tokio::io::sink());
    init.herdr_binary = "herdr".into();
    assert!(matches!(
        Gateway::new(&init, sender.clone(), crate::clock::Clock::default()),
        Err("invalid-herdr-binary")
    ));
    init.herdr_binary = std::env::temp_dir()
        .join("yzc-nonexistent-herdr")
        .join("DO-NOT-EXECUTE-HERDR");
    assert!(Gateway::new(&init, sender, crate::clock::Clock::default()).is_ok());
}

#[test]
fn bridge_allowlist_all_102_methods_closed_host_mapping() {
    let fixture: Value =
        serde_json::from_str(include_str!("../tests/fixtures/herdr-0.9.5-methods.json")).unwrap();
    let mapping = crate::herdr::METHOD_MAPPING;
    assert_eq!(mapping.len(), 17);
    let reachable: std::collections::HashSet<_> = mapping
        .iter()
        .flat_map(|(_, api, _)| api.iter().copied())
        .collect();
    for method in fixture["methods"].as_array().unwrap() {
        let method = method.as_str().unwrap();
        assert_eq!(
            reachable.contains(method),
            matches!(
                method,
                "ping"
                    | "session.snapshot"
                    | "events.subscribe"
                    | "agent.prompt"
                    | "agent.send_keys"
            ),
            "{method}"
        );
    }
    assert_eq!(fixture["methods"].as_array().unwrap().len(), 102);
    let keys = mapping
        .iter()
        .find(|(m, _, _)| *m == "pane.sendKeys")
        .unwrap();
    assert!(keys.1.contains(&"agent.send_keys"));
    for denied in [
        "pane.send_keys",
        "pane.send_input",
        "pane.send_text",
        "server.stop",
        "plugin.link",
        "layout.apply",
        "command.invoke",
    ] {
        assert!(!reachable.contains(denied));
    }
    for (gateway, _, _) in mapping {
        assert!(METHODS.iter().any(|m| m.name == *gateway));
    }
    // Closed enums are constructed, never decoded from a client method string.
    let source = include_str!("herdr.rs");
    assert_eq!(source.matches("HerdrFeatureRequest::").count(), 2);
    assert!(source.contains("HerdrFeatureRequest::AgentPrompt("));
    assert!(source.contains("HerdrFeatureRequest::AgentSendKeys("));
    assert!(!source.contains("api_request("));
}

#[tokio::test]
async fn review_snapshot_first_then_streams_preserves_reply_and_ping_headroom() {
    let (outbound, mut pending) = Outbound::new();
    let mut snapshot = vec![json!({"id":1,"result":{"chunked":true}})];
    for index in 0..57 {
        snapshot.push(
            json!({"event":"snapshot.chunk","data":{"index":index,"data":"x".repeat(48*1024)}}),
        );
    }
    assert!(outbound.batch(snapshot).is_ok());
    let frames = crate::read_bridge::fill_streams_after_snapshot(outbound.bytes.clone());
    assert!(
        outbound
            .replies(vec![json!({"id":2,"result":{"alive":true}})])
            .is_ok(),
        "reply must not close 1008"
    );
    assert!(
        outbound
            .send(Message::Ping(b"yzc".as_slice().into()))
            .is_ok(),
        "ping must not close 1013"
    );
    let mut queued = Vec::new();
    while let Ok(v) = pending.try_recv() {
        queued.push(v);
    }
    assert!(matches!(queued.last().unwrap().message, Message::Ping(_)));
    drop(frames);
    drop(queued);
    assert_eq!(outbound.reply_bytes.available_permits(), REPLY_HEADROOM);
    assert_eq!(
        outbound.bytes.available_permits(),
        OUTBOUND_BYTES - WRITE_BUFFER_BYTES - REPLY_HEADROOM
    );
}

#[tokio::test]
async fn bridge_invalid_request_table_and_still_requires_view_scope() {
    let (f, ctx) = authenticated_fixture();
    let long_id = "a".repeat(129);
    let deep = |depth: usize| format!("{}0{}", "[".repeat(depth), "]".repeat(depth));
    let deep_params = format!(
        "{{\"id\":1,\"method\":\"hello\",\"params\":{{\"a\":{}}}}}",
        deep(1000)
    );
    let huge_deep = format!(
        "{{\"id\":1,\"method\":\"hello\",\"params\":{}}}",
        deep(100_000)
    );
    let mut invalid: Vec<String> = vec![
        // empty, truncated and non-JSON text frames
        "".into(),
        " ".into(),
        "\r\n".into(),
        "{".into(),
        "{\"id\":1,\"method\":\"hello\"".into(),
        "not json".into(),
        "null".into(),
        "42".into(),
        "\"hello\"".into(),
        "[]".into(),
        // serde derive would accept a positional array; a Call is an object
        "[1,\"hello\",{}]".into(),
        "[1,\"hello\"]".into(),
        // unknown top-level field, missing required fields
        "{\"id\":1,\"method\":\"hello\",\"extra\":1}".into(),
        "{\"id\":1,\"method\":\"hello\",\"Params\":{}}".into(),
        "{\"method\":\"hello\"}".into(),
        "{\"id\":1}".into(),
        "{}".into(),
        // wrong-type or out-of-range id
        "{\"id\":true,\"method\":\"hello\"}".into(),
        "{\"id\":false,\"method\":\"hello\"}".into(),
        "{\"id\":null,\"method\":\"hello\"}".into(),
        "{\"id\":1.5,\"method\":\"hello\"}".into(),
        "{\"id\":1.0,\"method\":\"hello\"}".into(),
        "{\"id\":-1,\"method\":\"hello\"}".into(),
        "{\"id\":18446744073709551616,\"method\":\"hello\"}".into(),
        "{\"id\":[1],\"method\":\"hello\"}".into(),
        "{\"id\":{\"a\":1},\"method\":\"hello\"}".into(),
        format!("{{\"id\":\"{long_id}\",\"method\":\"hello\"}}"),
        // params must be an object or absent/null
        "{\"id\":1,\"method\":\"hello\",\"params\":[]}".into(),
        "{\"id\":1,\"method\":\"hello\",\"params\":[{}]}".into(),
        "{\"id\":1,\"method\":\"hello\",\"params\":\"x\"}".into(),
        "{\"id\":1,\"method\":\"hello\",\"params\":1}".into(),
        "{\"id\":1,\"method\":\"hello\",\"params\":true}".into(),
        // method must be a string
        "{\"id\":1,\"method\":1}".into(),
        "{\"id\":1,\"method\":null}".into(),
        "{\"id\":1,\"method\":[\"hello\"]}".into(),
        "{\"id\":1,\"method\":{\"a\":1}}".into(),
        // duplicate keys are rejected, never first- or last-wins
        "{\"id\":1,\"id\":2,\"method\":\"hello\"}".into(),
        "{\"id\":1,\"method\":\"hello\",\"method\":\"full.open\"}".into(),
        "{\"id\":1,\"method\":\"full.open\",\"method\":\"hello\"}".into(),
        "{\"id\":1,\"method\":\"hello\",\"params\":{},\"params\":{}}".into(),
        // nesting past the parser's recursion limit fails cleanly
        deep_params,
        huge_deep,
        deep(100_000),
    ];
    // invalid UTF-8 cannot arrive as a Message::Text, but NUL can.
    invalid.push("{\"id\":1,\"method\":\"hello\"}\0".into());
    invalid.push("\0".into());
    for text in &invalid {
        let reply = rpc(&f.gateway, &ctx, text)
            .unwrap_or_else(|_| panic!("view context must get a reply: {:.60}", text));
        assert_eq!(
            reply,
            json!({"id":null,"error":{"code":"invalid-request","message":"Invalid request"}}),
            "{:.60}",
            text
        );
    }
    // Boundaries that must remain valid: 128-byte string id, u64::MAX id, null params.
    let ok_id = "a".repeat(128);
    for text in [
        format!("{{\"id\":\"{ok_id}\",\"method\":\"session.ping\"}}"),
        "{\"id\":18446744073709551615,\"method\":\"session.ping\"}".into(),
        "{\"id\":0,\"method\":\"session.ping\",\"params\":null}".into(),
        "{\"id\":\"\",\"method\":\"session.ping\",\"params\":{}}\r\n".into(),
    ] {
        let reply = rpc(&f.gateway, &ctx, &text).unwrap_or_else(|_| panic!("{text:.60}"));
        assert!(
            reply["result"]["serverTime"].is_string() || reply["result"]["serverTime"].is_number(),
            "{reply}"
        );
    }
    // invalid-request is answered only to an authorized View session: once the
    // session is gone, the same malformed text closes (4401) instead of replying.
    f.gateway.logout(&ctx).unwrap();
    for text in ["{", "{\"id\":true,\"method\":\"hello\"}", ""] {
        assert!(
            matches!(rpc(&f.gateway, &ctx, text), Err(close) if close.code == 4401),
            "{text:.60}"
        );
    }
    assert!(matches!(
        rpc(&f.gateway, &ctx, "{\"id\":1,\"method\":\"hello\"}"),
        Err(close) if close.code == 4401
    ));
}
