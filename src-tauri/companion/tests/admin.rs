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
    assert_eq!(RESERVED_EVENTS, ["push.request"]);
}

#[test]
fn admin_loopback_default_and_invalid_configuration() {
    use yuzora_companion::{admin::Loopback, guard::Guard};
    let config: Loopback = serde_json::from_value(serde_json::json!({
        "publicHost":"desktop.tailnet.ts.net:8443", "requiredTailscaleLogin":"user@example.com"
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
