#[path = "support/identity.rs"]
mod identity;
#[path = "support/init.rs"]
mod init_fixture;
use identity::PROXY_SECRET;
use yuzora_companion::admin::{self, CommandBody, Event, RESERVED_EVENTS};

#[tokio::test]
async fn admin_version_unknown_malformed_and_truncated_frames() {
    for (input, expected) in [
        ("{\"v\":2,\"type\":\"shutdown\"}\n", "unsupported-version"),
        ("{not-json}\n", "invalid-command"),
        (
            "{\"v\":2,\"type\":\"init\",\"newField\":true}\n",
            "unsupported-version",
        ),
        (
            "{\"v\":1,\"type\":\"shutdown\",\"extra\":true}\n",
            "invalid-command",
        ),
        ("{\"v\":1,\"type\":\"shutdown\"}", "invalid-frame"),
        ("{\"type\":\"shutdown\"}\n", "invalid-command"),
    ] {
        let mut reader = input.as_bytes();
        assert!(matches!(admin::read(&mut reader).await, Err(code) if code == expected));
    }
    let mut reader = b"{\"v\":1,\"type\":\"future.command\",\"keyPem\":\"sensitive\"}\n{\"v\":1,\"type\":\"shutdown\"}\n".as_slice();
    assert!(matches!(
        admin::read(&mut reader).await.unwrap().unwrap().body,
        CommandBody::Unknown
    ));
    assert!(matches!(
        admin::read(&mut reader).await.unwrap().unwrap().body,
        CommandBody::Shutdown
    ));
    assert!(admin::read(&mut reader).await.unwrap().is_none());
}

#[tokio::test]
async fn admin_host_wire_frame_limit() {
    let bytes = vec![b'x'; yuzora_host::protocol::MAX_FRAME_BYTES + 1];
    let mut reader = bytes.as_slice();
    assert!(matches!(
        admin::read(&mut reader).await,
        Err("invalid-frame")
    ));
}

#[tokio::test]
async fn admin_events_are_versioned_ndjson() {
    use tokio::io::AsyncReadExt;
    let (output, mut input) = tokio::io::duplex(1024);
    let (events, writer) = admin::spawn_writer(output);
    assert!(events.emit(Event::ready(47110)).await);
    assert!(events.emit(Event::error("unknown-type")).await);
    drop(events);
    writer.await.unwrap().unwrap();
    let mut text = String::new();
    input.read_to_string(&mut text).await.unwrap();
    let events: Vec<serde_json::Value> = text
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    assert_eq!(events.len(), 2);
    assert_eq!(
        events[0],
        serde_json::json!({"v":1,"type":"ready","ports":{"loopback":47110,"lan":null}})
    );
    assert_eq!(events[1]["v"], 1);
    assert_eq!(events[1]["type"], "error");
    assert_eq!(events[1]["code"], "unknown-type");
    // `push.request` is a typed event now, not a reserved name.
    assert!(RESERVED_EVENTS.is_empty());
}

#[test]
fn admin_loopback_default_and_invalid_configuration() {
    use yuzora_companion::{admin::Loopback, guard::Guard};
    let config: Loopback = serde_json::from_value(serde_json::json!({
        "publicHost":"desktop.tailnet.ts.net:8443", "requiredTailscaleLogin":"user@example.com",
        "proxySecret":PROXY_SECRET
    }))
    .unwrap();
    assert_eq!(config.port, 47110);
    assert!(Guard::new(&config).is_ok());
    for host in [
        "127.0.0.1:8443",
        "[::1]:8443",
        "user@host:8443",
        "https://host:8443",
        "host:invalid",
        "Desktop.tailnet.ts.net:8443",
        "desktop.tailnet.ts.net:443",
        "desktop.tailnet.ts.net:08443",
        "desktop.tailnet.ts.net.:8443",
        "host:0",
        "host:",
        "",
    ] {
        let mut invalid = config.clone();
        invalid.public_host = host.into();
        assert!(Guard::new(&invalid).is_err(), "{host}");
    }
    for login in ["", "user\n@example.com"] {
        let mut invalid = config.clone();
        invalid.required_tailscale_login = login.into();
        assert!(Guard::new(&invalid).is_err());
    }
}

#[test]
fn admin_loopback_requires_a_canonical_32_byte_proxy_secret() {
    use yuzora_companion::{admin::Loopback, guard::Guard};
    let loopback = |secret: &serde_json::Value| {
        let mut config = serde_json::json!({
            "publicHost":"desktop.tailnet.ts.net:8443", "requiredTailscaleLogin":"user@example.com"
        });
        config["proxySecret"] = secret.clone();
        serde_json::from_value::<Loopback>(config)
    };
    // Required, and a string: serde rejects both before any guard exists.
    assert!(serde_json::from_value::<Loopback>(serde_json::json!({
        "publicHost":"desktop.tailnet.ts.net:8443", "requiredTailscaleLogin":"user@example.com"
    }))
    .is_err());
    for secret in [
        serde_json::json!(null),
        serde_json::json!(7),
        serde_json::json!([PROXY_SECRET]),
    ] {
        assert!(loopback(&secret).is_err());
    }
    assert!(Guard::new(&loopback(&serde_json::json!(PROXY_SECRET)).unwrap()).is_ok());
    let mut non_canonical = PROXY_SECRET.to_owned();
    non_canonical.replace_range(42.., "B");
    for secret in [
        String::new(),
        PROXY_SECRET[..42].to_owned(),
        format!("{PROXY_SECRET}A"),
        format!("{PROXY_SECRET}="),
        PROXY_SECRET.replace('Q', "+"),
        PROXY_SECRET.replace('Q', "/"),
        PROXY_SECRET.replace('Q', " "),
        format!("{}\u{e9}", &PROXY_SECRET[..41]),
        non_canonical,
    ] {
        let config = loopback(&serde_json::json!(secret)).unwrap();
        assert!(
            matches!(Guard::new(&config), Err("invalid-proxy-secret")),
            "{secret:?}"
        );
    }
}

#[tokio::test]
async fn admin_init_frame_without_proxy_secret_is_invalid() {
    let mut init: serde_json::Value = serde_json::from_str(&init_fixture::json()).unwrap();
    let mut frame = init.to_string() + "\n";
    assert!(matches!(
        admin::read(&mut frame.as_bytes())
            .await
            .unwrap()
            .unwrap()
            .body,
        CommandBody::Init(_)
    ));
    init["listeners"]["loopback"]
        .as_object_mut()
        .unwrap()
        .remove("proxySecret");
    frame = init.to_string() + "\n";
    assert!(matches!(
        admin::read(&mut frame.as_bytes()).await,
        Err("invalid-command")
    ));
}

#[tokio::test]
async fn admin_init_frame_requires_trust_store_and_space_roots() {
    let entry = serde_json::json!({"session":"work","workspaceId":"w","root":"/space"});
    let parse = |init: &serde_json::Value| {
        let frame = init.to_string() + "\n";
        let frame = frame.into_bytes();
        async move { admin::read(&mut frame.as_slice()).await }
    };
    let mut init: serde_json::Value = serde_json::from_str(&init_fixture::json()).unwrap();
    init["spaceRoots"] = serde_json::json!([entry]);
    let Ok(Some(command)) = parse(&init).await else {
        panic!("valid init")
    };
    let CommandBody::Init(parsed) = command.body else {
        panic!("init")
    };
    assert!(parsed.trust_store.is_absolute());
    assert_eq!(parsed.space_roots.len(), 1);
    assert_eq!(
        (
            parsed.space_roots[0].session.as_str(),
            parsed.space_roots[0].workspace_id.as_str(),
            parsed.space_roots[0].root.as_str()
        ),
        ("work", "w", "/space")
    );
    for field in ["trustStore", "spaceRoots"] {
        let mut missing = init.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert!(matches!(parse(&missing).await, Err("invalid-command")));
    }
    for (field, value) in [
        ("trustStore", serde_json::json!(7)),
        ("spaceRoots", serde_json::json!(null)),
        ("spaceRoots", serde_json::json!({})),
        ("spaceRoots", serde_json::json!([{"session":"work"}])),
        (
            "spaceRoots",
            serde_json::json!([{"session":"work","workspaceId":"w","root":"/s","extra":1}]),
        ),
        (
            "spaceRoots",
            serde_json::json!([{"session":"work","workspaceId":1,"root":"/s"}]),
        ),
    ] {
        let mut invalid = init.clone();
        invalid[field] = value;
        assert!(matches!(parse(&invalid).await, Err("invalid-command")));
    }
}

#[tokio::test]
async fn admin_spaces_roots_frame_is_versioned_closed_and_unacknowledged() {
    use yuzora_companion::{clock::Clock, gateway::Gateway};
    // Absolute on every platform ("/space" is relative on Windows).
    let root = std::env::temp_dir().to_string_lossy().into_owned();
    let entry = serde_json::json!({"session":"work","workspaceId":"w","root":root});
    let frame = |value: serde_json::Value| (value.to_string() + "\n").into_bytes();
    let read = |bytes: Vec<u8>| async move { admin::read(&mut bytes.as_slice()).await };
    let Ok(Some(command)) = read(frame(
        serde_json::json!({"v":1,"type":"spaces.roots","roots":[entry]}),
    ))
    .await
    else {
        panic!("valid command")
    };
    let CommandBody::Spaces(roots) = command.body else {
        panic!("spaces.roots")
    };
    assert_eq!(roots.roots.len(), 1);
    assert!(matches!(
        read(frame(
            serde_json::json!({"v":1,"type":"spaces.roots","roots":[]})
        ))
        .await
        .unwrap()
        .unwrap()
        .body,
        CommandBody::Spaces(_)
    ));
    assert!(matches!(
        read(frame(
            serde_json::json!({"v":2,"type":"spaces.roots","roots":[]})
        ))
        .await,
        Err("unsupported-version")
    ));
    for invalid in [
        serde_json::json!({"v":1,"type":"spaces.roots"}),
        serde_json::json!({"v":1,"type":"spaces.roots","roots":null}),
        serde_json::json!({"v":1,"type":"spaces.roots","roots":[],"extra":true}),
        serde_json::json!({"v":1,"type":"spaces.roots","roots":[{"session":"work"}]}),
        serde_json::json!({"v":1,"type":"spaces.roots","roots":[{"session":"work","workspaceId":"w","root":"/s","x":1}]}),
    ] {
        assert!(matches!(read(frame(invalid)).await, Err("invalid-command")));
    }

    // The gateway applies a valid mapping without any reply, and rejects an
    // invalid one as a refused command (the desktop sees `admin-command-rejected`).
    let init: yuzora_companion::admin::Init = serde_json::from_str(&init_fixture::json()).unwrap();
    let (events, _writer) = admin::spawn_writer(tokio::io::sink());
    let gateway = Gateway::new(&init, events, Clock::default()).unwrap();
    let spaces = |roots: serde_json::Value| {
        CommandBody::Spaces(
            serde_json::from_value(serde_json::json!({"v":1,"type":"spaces.roots","roots":roots}))
                .unwrap(),
        )
    };
    assert_eq!(
        gateway.command(spaces(serde_json::json!([entry]))),
        Ok(admin::AdminResult::Applied)
    );
    for invalid in [
        serde_json::json!([{"session":"work","workspaceId":"w","root":"relative"}]),
        serde_json::json!([entry, entry]),
        serde_json::json!([{"session":"work","workspaceId":"","root":"/s"}]),
    ] {
        let rejected = gateway.command(spaces(invalid)).unwrap_err();
        assert_eq!(rejected.0, axum::http::StatusCode::BAD_REQUEST);
    }
}

#[tokio::test]
async fn admin_init_frame_push_is_null_absent_or_a_closed_object() {
    let parse = |init: &serde_json::Value| {
        let frame = (init.to_string() + "\n").into_bytes();
        async move { admin::read(&mut frame.as_slice()).await }
    };
    let mut init: serde_json::Value = serde_json::from_str(&init_fixture::json()).unwrap();
    assert_eq!(init["push"], serde_json::Value::Null);
    let Ok(Some(command)) = parse(&init).await else {
        panic!("push: null")
    };
    let CommandBody::Init(parsed) = command.body else {
        panic!("init")
    };
    assert!(parsed.push.is_none());
    init.as_object_mut().unwrap().remove("push");
    let Ok(Some(command)) = parse(&init).await else {
        panic!("push absent")
    };
    assert!(matches!(command.body, CommandBody::Init(parsed) if parsed.push.is_none()));
    let endpoint = "TpvauLvnGJwArrsJIvxJ_kDgBZTEAPPhtdz5ejXHn4U";
    init["push"] = serde_json::json!({"vapidPublicKey":"key","devices":[
        {"deviceId":"AAAAAAAAAAAAAAAAAAAAAA","endpoints":[endpoint]}]});
    let Ok(Some(command)) = parse(&init).await else {
        panic!("push object")
    };
    let CommandBody::Init(parsed) = command.body else {
        panic!("init")
    };
    let push = parsed.push.unwrap();
    assert_eq!(push.vapid_public_key, "key");
    assert_eq!(push.devices.len(), 1);
    assert_eq!(push.devices[0].device_id, "AAAAAAAAAAAAAAAAAAAAAA");
    assert_eq!(push.devices[0].endpoints, [endpoint]);
    for invalid in [
        serde_json::json!(7),
        serde_json::json!("key"),
        serde_json::json!([]),
        serde_json::json!({}),
        serde_json::json!({"vapidPublicKey":"key"}),
        serde_json::json!({"devices":[]}),
        serde_json::json!({"vapidPublicKey":null,"devices":[]}),
        serde_json::json!({"vapidPublicKey":"key","devices":null}),
        serde_json::json!({"vapidPublicKey":"key","devices":[1]}),
        // The old shape, a bare device id, is gone.
        serde_json::json!({"vapidPublicKey":"key","devices":["AAAAAAAAAAAAAAAAAAAAAA"]}),
        serde_json::json!({"vapidPublicKey":"key","devices":[{"deviceId":"d"}]}),
        serde_json::json!({"vapidPublicKey":"key","devices":[{"endpoints":[]}]}),
        serde_json::json!({"vapidPublicKey":"key","devices":[{"deviceId":"d","endpoints":[1]}]}),
        serde_json::json!({"vapidPublicKey":"key","devices":[{"deviceId":"d","endpoints":[],"x":1}]}),
        serde_json::json!({"vapidPublicKey":"key","devices":[],"privateKey":"x"}),
    ] {
        init["push"] = invalid;
        assert!(matches!(parse(&init).await, Err("invalid-command")));
    }
}

#[tokio::test]
async fn admin_push_devices_frame_is_versioned_closed_and_unacknowledged() {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    use yuzora_companion::{admin::PushInit, clock::Clock, gateway::Gateway};
    let frame = |value: serde_json::Value| (value.to_string() + "\n").into_bytes();
    let read = |bytes: Vec<u8>| async move { admin::read(&mut bytes.as_slice()).await };
    let endpoint = "TpvauLvnGJwArrsJIvxJ_kDgBZTEAPPhtdz5ejXHn4U";
    let Ok(Some(command)) = read(frame(
        serde_json::json!({"v":1,"type":"push.devices","devices":[
            {"deviceId":"AAAAAAAAAAAAAAAAAAAAAA","endpoints":[endpoint]}]}),
    ))
    .await
    else {
        panic!("valid command")
    };
    let CommandBody::Push(devices) = command.body else {
        panic!("push.devices")
    };
    assert_eq!(devices.devices.len(), 1);
    assert_eq!(devices.devices[0].device_id, "AAAAAAAAAAAAAAAAAAAAAA");
    assert_eq!(devices.devices[0].endpoints, [endpoint]);
    assert!(matches!(
        read(frame(
            serde_json::json!({"v":1,"type":"push.devices","devices":[]})
        ))
        .await
        .unwrap()
        .unwrap()
        .body,
        CommandBody::Push(_)
    ));
    assert!(matches!(
        read(frame(
            serde_json::json!({"v":2,"type":"push.devices","devices":[]})
        ))
        .await,
        Err("unsupported-version")
    ));
    for invalid in [
        serde_json::json!({"v":1,"type":"push.devices"}),
        serde_json::json!({"v":1,"type":"push.devices","devices":null}),
        serde_json::json!({"v":1,"type":"push.devices","devices":[7]}),
        serde_json::json!({"v":1,"type":"push.devices","devices":[],"extra":true}),
        // The old shape is gone.
        serde_json::json!({"v":1,"type":"push.devices","deviceIds":[]}),
        serde_json::json!({"v":1,"type":"push.devices","devices":["AAAAAAAAAAAAAAAAAAAAAA"]}),
        serde_json::json!({"v":1,"type":"push.devices","devices":[{"deviceId":"d"}]}),
        serde_json::json!({"v":1,"type":"push.devices","devices":[{"deviceId":"d","endpoints":[3]}]}),
        serde_json::json!({"v":1,"type":"push.devices","devices":[{"deviceId":"d","endpoints":[],"x":1}]}),
    ] {
        assert!(matches!(read(frame(invalid)).await, Err("invalid-command")));
    }

    // Applied without a reply when push exists; refused (the desktop sees
    // `admin-command-rejected`) when the init had no VAPID key or the list
    // breaks a limit.
    let push_devices = |devices: serde_json::Value| {
        CommandBody::Push(
            serde_json::from_value(
                serde_json::json!({"v":1,"type":"push.devices","devices":devices}),
            )
            .unwrap(),
        )
    };
    let gateway = |push: Option<PushInit>| {
        let mut init: yuzora_companion::admin::Init =
            serde_json::from_str(&init_fixture::json()).unwrap();
        init.push = push;
        let (events, _writer) = admin::spawn_writer(tokio::io::sink());
        Gateway::new(&init, events, Clock::default()).unwrap()
    };
    let with_push = gateway(Some(PushInit {
        vapid_public_key: URL_SAFE_NO_PAD.encode([&[4u8][..], &[1u8; 64]].concat()),
        devices: Vec::new(),
    }));
    let device = |id: &str, endpoints: usize| serde_json::json!({"deviceId":id,"endpoints":(0..endpoints).map(|n| URL_SAFE_NO_PAD.encode([n as u8; 32])).collect::<Vec<_>>()});
    assert_eq!(
        with_push.command(push_devices(serde_json::json!([device(
            "unknown-device",
            4
        )]))),
        Ok(admin::AdminResult::Applied)
    );
    let too_many: Vec<_> = (0..65).map(|n| device(&format!("device-{n}"), 1)).collect();
    for refused in [
        serde_json::json!(too_many),
        serde_json::json!([device("device", 0)]),
        serde_json::json!([device("device", 5)]),
    ] {
        assert_eq!(
            with_push.command(push_devices(refused)).unwrap_err().0,
            axum::http::StatusCode::BAD_REQUEST
        );
    }
    let without_push = gateway(None);
    assert_eq!(
        without_push
            .command(push_devices(serde_json::json!([])))
            .unwrap_err()
            .0,
        axum::http::StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn admin_push_ack_and_peers_list_frames_are_versioned_and_closed() {
    let frame = |value: serde_json::Value| (value.to_string() + "\n").into_bytes();
    let read = |bytes: Vec<u8>| async move { admin::read(&mut bytes.as_slice()).await };
    let Ok(Some(command)) = read(frame(
        serde_json::json!({"v":1,"type":"push.ack","requestId":7,"stored":false}),
    ))
    .await
    else {
        panic!("valid command")
    };
    let CommandBody::PushAck(ack) = command.body else {
        panic!("push.ack")
    };
    assert_eq!((ack.request_id, ack.stored), (7, false));
    let Ok(Some(command)) = read(frame(
        serde_json::json!({"v":1,"type":"peers.list","requestId":3}),
    ))
    .await
    else {
        panic!("valid command")
    };
    let CommandBody::Peers(list) = command.body else {
        panic!("peers.list")
    };
    assert_eq!(list.request_id, 3);
    for (kind, body) in [
        ("push.ack", serde_json::json!({"requestId":1,"stored":true})),
        ("peers.list", serde_json::json!({"requestId":1})),
    ] {
        let mut versioned = body.clone();
        versioned["v"] = serde_json::json!(2);
        versioned["type"] = serde_json::json!(kind);
        assert!(matches!(
            read(frame(versioned)).await,
            Err("unsupported-version")
        ));
    }
    for invalid in [
        serde_json::json!({"v":1,"type":"push.ack"}),
        serde_json::json!({"v":1,"type":"push.ack","requestId":1}),
        serde_json::json!({"v":1,"type":"push.ack","stored":true}),
        serde_json::json!({"v":1,"type":"push.ack","requestId":"1","stored":true}),
        serde_json::json!({"v":1,"type":"push.ack","requestId":-1,"stored":true}),
        serde_json::json!({"v":1,"type":"push.ack","requestId":1,"stored":"yes"}),
        serde_json::json!({"v":1,"type":"push.ack","requestId":1,"stored":true,"x":1}),
        serde_json::json!({"v":1,"type":"peers.list"}),
        serde_json::json!({"v":1,"type":"peers.list","requestId":null}),
        serde_json::json!({"v":1,"type":"peers.list","requestId":1,"x":1}),
    ] {
        assert!(matches!(read(frame(invalid)).await, Err("invalid-command")));
    }
}

#[tokio::test]
async fn admin_peers_snapshot_event_is_a_closed_reply_with_its_request_id() {
    use yuzora_companion::{admin::AdminResult, clock::Clock, gateway::Gateway};
    let init: yuzora_companion::admin::Init = serde_json::from_str(&init_fixture::json()).unwrap();
    let (events, _writer) = admin::spawn_writer(tokio::io::sink());
    let gateway = Gateway::new(&init, events, Clock::default()).unwrap();
    let list = CommandBody::Peers(
        serde_json::from_value(serde_json::json!({"v":1,"type":"peers.list","requestId":9}))
            .unwrap(),
    );
    let result = gateway.command(list).unwrap();
    assert!(
        matches!(&result, AdminResult::Peers { request_id: 9, peers, streams } if peers.is_empty() && streams.is_empty())
    );
    let event = serde_json::to_value(result.deferred_event().unwrap()).unwrap();
    assert_eq!(
        event,
        serde_json::json!({"v":1,"type":"peers.snapshot","requestId":9,"peers":[],"streams":[]})
    );
    let filled = AdminResult::Peers {
        request_id: 1,
        peers: vec![admin::PeerInfo {
            peer_id: 4,
            device_id: "AAAAAAAAAAAAAAAAAAAAAA".into(),
        }],
        streams: vec![admin::StreamInfo {
            stream: "12".into(),
            device_id: "AAAAAAAAAAAAAAAAAAAAAA".into(),
            peer_id: 4,
            session: "work".into(),
            scope: admin::Scope::Control,
            target: Some("w:p".into()),
        }],
    };
    assert_eq!(
        serde_json::to_value(filled.deferred_event().unwrap()).unwrap(),
        serde_json::json!({"v":1,"type":"peers.snapshot","requestId":1,
            "peers":[{"peerId":4,"deviceId":"AAAAAAAAAAAAAAAAAAAAAA"}],
            "streams":[{"stream":"12","deviceId":"AAAAAAAAAAAAAAAAAAAAAA","peerId":4,
                "session":"work","scope":"control","target":"w:p"}]})
    );
}

#[tokio::test]
async fn admin_concurrent_emits_are_ordered_complete_lines() {
    use tokio::io::AsyncReadExt;
    // Tiny transport forces each line to span multiple writes.
    let (output, mut input) = tokio::io::duplex(7);
    let (events, writer) = admin::spawn_writer(output);
    let reader = tokio::spawn(async move {
        let mut text = String::new();
        input.read_to_string(&mut text).await.unwrap();
        text
    });
    let mut producers = tokio::task::JoinSet::new();
    for producer in 0..8u16 {
        let events = events.clone();
        producers.spawn(async move {
            for sequence in 0..20u16 {
                assert!(events.emit(Event::ready(producer * 100 + sequence)).await);
            }
        });
    }
    while let Some(result) = producers.join_next().await {
        result.unwrap();
    }
    drop(events);
    writer.await.unwrap().unwrap();
    let text = reader.await.unwrap();
    assert!(text.ends_with('\n'));
    let mut next = [0; 8];
    for line in text.lines() {
        let event: serde_json::Value = serde_json::from_str(line).unwrap();
        let port = event["ports"]["loopback"].as_u64().unwrap() as usize;
        let producer = port / 100;
        assert_eq!(port % 100, next[producer]);
        next[producer] += 1;
    }
    assert_eq!(next, [20; 8]);
}

#[tokio::test]
async fn admin_zeroizing_frame_growth_boundaries_and_errors() {
    use tokio::io::BufReader;
    use yuzora_host::protocol::MAX_FRAME_BYTES;
    let mut input = vec![b'x'; MAX_FRAME_BYTES];
    input[MAX_FRAME_BYTES - 1] = b'\n';
    let mut reader = BufReader::with_capacity(31, input.as_slice());
    let frame = admin::read_frame_zeroizing(&mut reader)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&*frame, &input);
    assert!(admin::read_frame_zeroizing(&mut reader)
        .await
        .unwrap()
        .is_none());
    input.push(b'\n');
    input[MAX_FRAME_BYTES - 1] = b'x';
    let mut reader = BufReader::with_capacity(31, input.as_slice());
    assert!(matches!(
        admin::read_frame_zeroizing(&mut reader).await,
        Err("invalid-frame")
    ));
    let mut truncated = BufReader::with_capacity(2, b"secret".as_slice());
    assert!(matches!(
        admin::read_frame_zeroizing(&mut truncated).await,
        Err("invalid-frame")
    ));
    // Inspect the owner pattern too: allocator-freed memory cannot be read safely.
    let source = include_str!("../src/admin.rs").replace("\r\n", "\n");
    assert!(!source.contains("yuzora_host::wire::read_frame("));
    assert!(source.contains("let mut replacement = Zeroizing::new(Vec::with_capacity(capacity))"));
    assert!(!source
        .split("pub struct Init")
        .next()
        .unwrap()
        .contains("serde(flatten)"));
}

#[tokio::test]
async fn admin_try_emit_full_queue_is_nonblocking_and_does_not_enqueue() {
    use tokio::io::AsyncReadExt;
    let (output, mut input) = tokio::io::duplex(1);
    let (events, writer) = admin::spawn_writer(output);
    // No await: the writer cannot drain the queue during this fill.
    for port in 0..64 {
        assert!(events.try_emit(Event::ready(port)));
    }
    let started = std::time::Instant::now();
    assert!(!events.try_emit(Event::ready(999)));
    assert!(started.elapsed() < std::time::Duration::from_millis(100));
    assert!(!writer.is_finished());
    drop(events);
    let mut text = String::new();
    input.read_to_string(&mut text).await.unwrap();
    writer.await.unwrap().unwrap();
    assert_eq!(text.lines().count(), 64);
    assert!(!text.contains("999"));
}

#[tokio::test]
async fn admin_slow_live_stdout_preserves_queued_lifecycle_ack() {
    use tokio::{
        io::{AsyncBufReadExt, BufReader},
        time::{Duration, Instant},
    };
    let (output, input) = tokio::io::duplex(1);
    let (events, writer) = admin::spawn_writer(output);
    for port in 0..4 {
        assert!(events.try_emit(Event::ready(port)));
    }
    let lifecycle = events.clone();
    let started = Instant::now();
    let ack = tokio::spawn(async move { lifecycle.emit(Event::ready(4)).await });
    let reader = tokio::spawn(async move {
        let mut input = BufReader::new(input);
        for port in 0..5 {
            // Each write progresses within 1s, but total queue time exceeds 1s.
            tokio::time::sleep(Duration::from_millis(300)).await;
            let mut line = String::new();
            input.read_line(&mut line).await.unwrap();
            let event: serde_json::Value = serde_json::from_str(&line).unwrap();
            assert_eq!(event["ports"]["loopback"], port);
        }
    });
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert!(
        !writer.is_finished(),
        "slow but alive stdout must remain open"
    );
    assert!(
        !ack.is_finished(),
        "enqueue delay must not fail lifecycle ACK"
    );
    assert!(tokio::time::timeout(Duration::from_secs(2), ack)
        .await
        .unwrap()
        .unwrap());
    assert!(started.elapsed() >= Duration::from_millis(1500));
    drop(events);
    writer.await.unwrap().unwrap();
    reader.await.unwrap();
}

#[tokio::test]
async fn admin_broken_stdout_ends_writer_and_closes_senders() {
    use tokio::time::{timeout, Duration};
    let (output, input) = tokio::io::duplex(1);
    let (events, writer) = admin::spawn_writer(output);
    drop(input);
    assert!(events.try_emit(Event::ready(1)));
    timeout(Duration::from_secs(2), events.closed())
        .await
        .unwrap();
    assert!(writer.await.unwrap().is_err());
    assert!(!events.try_emit(Event::ready(2)));
    assert!(!events.emit(Event::ready(3)).await);
}

#[tokio::test(start_paused = true)]
async fn admin_stalled_stdout_writer_timeout_closes_pending_ack() {
    use tokio::time::{Duration, Instant};
    let (output, _input) = tokio::io::duplex(1);
    let (events, writer) = admin::spawn_writer(output);
    let started = Instant::now();
    assert!(!events.emit(Event::ready(1)).await);
    assert_eq!(started.elapsed(), Duration::from_secs(1));
    assert_eq!(
        writer.await.unwrap().unwrap_err().kind(),
        std::io::ErrorKind::TimedOut
    );
    events.closed().await;
}

fn nested(open: &str, close: &str, depth: usize) -> String {
    format!("{}0{}", open.repeat(depth), close.repeat(depth))
}

#[tokio::test]
async fn admin_malformed_input_table_wrong_types_duplicates_nesting_bytes_and_blank_lines() {
    let deep_array = nested("[", "]", 100_000);
    let deep_object = format!("{}0{}", "{\"a\":".repeat(100_000), "}".repeat(100_000));
    let mut cases: Vec<(String, &str)> = Vec::new();
    // Wrong primitive types in the envelope are invalid-command, never a version error.
    for v in [
        "\"1\"",
        "1.0",
        "1.5",
        "-1",
        "true",
        "null",
        "[1]",
        "{}",
        "4294967297",
    ] {
        cases.push((
            format!("{{\"v\":{v},\"type\":\"shutdown\"}}\n"),
            "invalid-command",
        ));
    }
    for kind in ["1", "null", "true", "[\"shutdown\"]", "{}"] {
        cases.push((format!("{{\"v\":1,\"type\":{kind}}}\n"), "invalid-command"));
    }
    for top in ["[]", "\"shutdown\"", "1", "null", "true"] {
        cases.push((format!("{top}\n"), "invalid-command"));
    }
    // Duplicate envelope keys are rejected (serde derive), so neither first-wins
    // nor last-wins can smuggle a different version/type past a logger or proxy.
    cases.push((
        "{\"v\":1,\"v\":2,\"type\":\"shutdown\"}\n".into(),
        "invalid-command",
    ));
    cases.push((
        "{\"v\":2,\"v\":1,\"type\":\"shutdown\"}\n".into(),
        "invalid-command",
    ));
    cases.push((
        "{\"v\":1,\"type\":\"shutdown\",\"type\":\"init\"}\n".into(),
        "invalid-command",
    ));
    cases.push((
        "{\"v\":1,\"type\":\"pairing.approve\",\"pairingId\":\"a\",\"pairingId\":\"b\"}\n".into(),
        "invalid-command",
    ));
    // Nesting beyond serde_json's recursion limit fails cleanly: no stack overflow.
    cases.push((
        format!("{{\"v\":1,\"type\":\"shutdown\",\"x\":{deep_array}}}\n"),
        "invalid-command",
    ));
    cases.push((
        format!("{{\"v\":1,\"type\":\"init\",\"x\":{deep_object}}}\n"),
        "invalid-command",
    ));
    cases.push((
        format!("{{\"v\":1,\"type\":\"init\",\"herdrBinary\":{deep_array}}}\n"),
        "invalid-command",
    ));
    // Positional (array) envelopes are not NDJSON command objects.
    cases.push(("[1,\"shutdown\"]\n".into(), "invalid-command"));
    cases.push(("[2,\"shutdown\"]\n".into(), "invalid-command"));
    cases.push((format!("{deep_array}\n"), "invalid-command"));
    // Invalid UTF-8 and raw NUL bytes.
    let mut frames: Vec<(Vec<u8>, &str)> = vec![(
        b"{\"v\":1,\"type\":\"sh\xffutdown\"}\n".to_vec(),
        "invalid-command",
    )];
    frames.push((
        b"{\"v\":1,\"type\":\"init\",\"x\":\"\xc3\x28\"}\n".to_vec(),
        "invalid-command",
    ));
    frames.push((
        b"{\"v\":1,\"type\":\"shut\0down\"}\n".to_vec(),
        "invalid-command",
    ));
    frames.push((
        b"{\"v\":1,\"type\":\"shutdown\"}\0\n".to_vec(),
        "invalid-command",
    ));
    frames.push((b"\0\n".to_vec(), "invalid-command"));
    // Empty, whitespace-only and CRLF-only lines are frames, and invalid ones.
    for blank in ["\n", "\r\n", " \n", "\t\r\n"] {
        frames.push((blank.as_bytes().to_vec(), "invalid-command"));
    }
    // A lone CR is not a terminator: the frame is truncated at EOF.
    frames.push((
        b"{\"v\":1,\"type\":\"shutdown\"}\r".to_vec(),
        "invalid-frame",
    ));
    frames.extend(
        cases
            .into_iter()
            .map(|(text, code)| (text.into_bytes(), code)),
    );
    for (bytes, expected) in frames {
        let mut reader = bytes.as_slice();
        let result = admin::read(&mut reader).await;
        assert!(
            matches!(&result, Err(code) if *code == expected),
            "{:?} -> {:?}",
            String::from_utf8_lossy(&bytes[..bytes.len().min(80)]),
            result.as_ref().map(|_| "accepted").map_err(|e| *e)
        );
    }
    // Accepted shapes that must stay accepted: CRLF-terminated frame, nesting
    // inside the recursion limit, and an escaped NUL in an ignored future field.
    let mut reader = b"{\"v\":1,\"type\":\"shutdown\"}\r\n".as_slice();
    assert!(matches!(
        admin::read(&mut reader).await.unwrap().unwrap().body,
        CommandBody::Shutdown
    ));
    let shallow = format!(
        "{{\"v\":1,\"type\":\"future\",\"x\":{},\"y\":\"\\u0000\"}}\n",
        nested("[", "]", 64)
    );
    let mut reader = shallow.as_bytes();
    assert!(matches!(
        admin::read(&mut reader).await.unwrap().unwrap().body,
        CommandBody::Unknown
    ));
    // An IGNORED field of a future command type is skipped iteratively by serde_json:
    // even 100000-deep nesting neither overflows the stack nor needs rejecting.
    let ignored =
        format!("{{\"v\":1,\"type\":\"future\",\"x\":{deep_array},\"y\":{deep_object}}}\n");
    let mut reader = ignored.as_bytes();
    assert!(matches!(
        admin::read(&mut reader).await.unwrap().unwrap().body,
        CommandBody::Unknown
    ));
    // serde_json skips (does not UTF-8 validate) strings of an IGNORED field; that
    // only happens for a future command type whose body is discarded as Unknown.
    // Typed bodies (init above, shutdown, pairing...) still validate every byte.
    let mut reader = b"{\"v\":1,\"type\":\"future\",\"x\":\"\xc3\x28\"}\n".as_slice();
    assert!(matches!(
        admin::read(&mut reader).await.unwrap().unwrap().body,
        CommandBody::Unknown
    ));
    // A bad frame consumes exactly its line: the next command still parses.
    let mut reader = b"\r\n{\"v\":1,\"type\":\"shutdown\"}\n".as_slice();
    assert!(admin::read(&mut reader).await.is_err());
    assert!(matches!(
        admin::read(&mut reader).await.unwrap().unwrap().body,
        CommandBody::Shutdown
    ));
}
