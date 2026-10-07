//! App-owned, bounded channels for remote official HERDR connectors.
use crate::host_service::{open_stream, HostState, HostStream};
use serde::Serialize;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot, watch};
use yuzora_host::protocol::{ConnectionOwner, Outcome, MAX_FRAME_BYTES, PROTOCOL_VERSION};
use yuzora_host::stream_protocol::*;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostStreamOpened {
    pub stream_id: String,
    pub value: Value,
}

#[derive(Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum HostStreamEvent {
    Frame {
        stream_id: String,
        frame: StreamFrame,
    },
    Closed {
        stream_id: String,
        owner: ConnectionOwner,
        reason: String,
    },
}

type Reply = oneshot::Sender<Result<Value, String>>;
struct Queued {
    operation: StreamCommand,
    reply: Reply,
}
pub(crate) struct HostStreamSession {
    sender: mpsc::Sender<Queued>,
    cancelled: watch::Sender<bool>,
}
impl Drop for HostStreamSession {
    fn drop(&mut self) {
        self.cancelled.send_replace(true);
    }
}
impl HostStreamSession {
    async fn request(&self, operation: StreamCommand) -> Result<Value, String> {
        let (reply, response) = oneshot::channel();
        self.sender
            .try_send(Queued { operation, reply })
            .map_err(|_| "stream-closed-or-busy")?;
        match tokio::time::timeout(Duration::from_secs(30), response).await {
            Ok(Ok(result)) => result,
            _ => {
                // Outcome may be unknown: discard the connection, never replay input.
                self.cancelled.send_replace(true);
                Err("stream-response-unavailable".into())
            }
        }
    }
}

async fn send<W: AsyncWrite + Unpin>(
    output: &mut W,
    request: &StreamRequest,
) -> Result<(), String> {
    let mut bytes = serde_json::to_vec(request).map_err(|e| e.to_string())?;
    if bytes.len() >= MAX_FRAME_BYTES {
        return Err("frame-too-large".into());
    }
    bytes.push(b'\n');
    tokio::time::timeout(Duration::from_secs(10), async {
        output.write_all(&bytes).await?;
        output.flush().await
    })
    .await
    .map_err(|_| "stream-write-timeout")?
    .map_err(|e| e.to_string())
}

fn parse(bytes: &[u8], owner: &ConnectionOwner) -> Result<StreamFrame, String> {
    let frame: StreamFrame = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    if frame.version != PROTOCOL_VERSION || &frame.owner != owner {
        return Err("stream-identity-mismatch".into());
    }
    Ok(frame)
}
fn result(outcome: Outcome) -> Result<Value, String> {
    match outcome {
        Outcome::Ok { value } => Ok(value),
        Outcome::Error { message, .. } => Err(message),
    }
}

async fn run(
    io: HostStream,
    owner: ConnectionOwner,
    mut requests: mpsc::Receiver<Queued>,
    mut cancelled: watch::Receiver<bool>,
    mut host_cancelled: watch::Receiver<bool>,
    on_frame: impl Fn(StreamFrame) -> Result<(), String>,
) -> Result<(), String> {
    let (read, mut write) = tokio::io::split(io);
    let mut read = BufReader::new(read);
    let mut pending: Option<(String, Reply, tokio::time::Instant)> = None;
    let mut next_id = 0_u64;
    let stream_state = cancelled.clone();
    let host_state = host_cancelled.clone();
    let operation = async {
        loop {
            if *stream_state.borrow() || *host_state.borrow() {
                return Err("host-stream-closed".into());
            }
            let next = yuzora_host::wire::read_frame(&mut read);
            tokio::pin!(next);
            let bytes = loop {
                let deadline = pending
                    .as_ref()
                    .map(|p| p.2)
                    .unwrap_or_else(|| tokio::time::Instant::now() + Duration::from_secs(30));
                tokio::select! {
                    biased;
                    _ = tokio::time::sleep_until(deadline), if pending.is_some() => return Err("stream-request-timeout".into()),
                    frame = &mut next => break frame?.ok_or("host-stream-ended")?,
                    request = requests.recv(), if pending.is_none() => {
                        let Some(request) = request else {return Ok(());};
                        if request.reply.is_closed() { continue; }
                        next_id += 1;
                        let id = next_id.to_string();
                        send(&mut write,&StreamRequest {version:PROTOCOL_VERSION,owner:owner.clone(),id:id.clone(),operation:request.operation}).await?;
                        pending = Some((id,request.reply,tokio::time::Instant::now()+Duration::from_secs(15)));
                    },
                }
            };
            let frame = parse(&bytes, &owner)?;
            match frame.payload {
                StreamPayload::Reply { id, outcome } => {
                    let (expected, reply, _) =
                        pending.take().ok_or("unexpected-stream-response")?;
                    if id != expected {
                        return Err("stream-response-id-mismatch".into());
                    }
                    let _ = reply.send(result(outcome));
                }
                _ => on_frame(frame)?,
            }
        }
    };
    // Watch the whole operation so a blocked write cannot defer cancellation.
    // Cancelling drops the partially written request; it is never replayed.
    let result = tokio::select! {
        biased;
        _ = cancelled.changed() => Err("host-stream-closed".into()),
        _ = host_cancelled.changed() => Err("host-disconnected".into()),
        result = operation => result,
    };
    // Deliver EOF before closing the SSH channel/process so the helper can
    // reap its connector. HERDR server and panes are never owned by this task.
    let _ = tokio::time::timeout(Duration::from_secs(1), write.shutdown()).await;
    result
}

#[tauri::command]
pub async fn host_stream_open(
    state: tauri::State<'_, HostState>,
    ssh: tauri::State<'_, crate::ssh_service::SshState>,
    owner: ConnectionOwner,
    config: StreamConfig,
    on_event: tauri::ipc::Channel<HostStreamEvent>,
) -> Result<HostStreamOpened, String> {
    let connection = state.0.connection(&owner)?;
    let _permit = connection.acquire_stream_opening().await?;
    if let (
        crate::host_service::HostTarget::Ssh { session_id },
        StreamConfig::Events {
            binary,
            session_name,
            pane_ids,
        },
        true,
    ) = (
        &connection.target,
        &config,
        connection.platform.get().is_none_or(|os| os != "windows"),
    ) {
        return open_ssh_events(
            &connection,
            ssh.0.clone(),
            session_id,
            binary,
            session_name,
            pane_ids,
            on_event,
        )
        .await;
    }
    let mut host_cancelled = connection.cancelled.subscribe();
    let io = tokio::select! {
        _ = host_cancelled.changed() => return Err("host-disconnected".into()),
        io = open_stream(&connection.target,&connection.helper,crate::host_service::HostLane::Stream,&ssh.0) => io?,
    };
    let stream_id = uuid::Uuid::new_v4().to_string();
    let (sender, requests) = mpsc::channel(STREAM_QUEUE_CAPACITY);
    let (cancelled, cancel_rx) = watch::channel(false);
    let stream = Arc::new(HostStreamSession { sender, cancelled });
    {
        let mut streams = connection.streams.lock().unwrap();
        if *connection.cancelled.borrow() {
            return Err("host-disconnected".into());
        }
        if streams.len() >= 32 {
            return Err("too-many-host-streams".into());
        }
        streams.insert(stream_id.clone(), stream.clone());
    }
    let resources = connection.streams.clone();
    let actor_id = stream_id.clone();
    tokio::spawn(async move {
        let status = run(
            io,
            owner.clone(),
            requests,
            cancel_rx,
            host_cancelled,
            |frame| {
                on_event
                    .send(HostStreamEvent::Frame {
                        stream_id: actor_id.clone(),
                        frame,
                    })
                    .map_err(|e| e.to_string())
            },
        )
        .await;
        resources.lock().unwrap().remove(&actor_id);
        let _ = on_event.send(HostStreamEvent::Closed {
            stream_id: actor_id,
            owner,
            reason: status.err().unwrap_or_else(|| "stream-closed".into()),
        });
    });
    let value = stream.request(StreamCommand::Open { config }).await?;
    Ok(HostStreamOpened { stream_id, value })
}

async fn open_ssh_events(
    connection: &Arc<crate::host_service::HostConnection>,
    ssh: Arc<crate::ssh_service::SshManager>,
    session_id: &str,
    binary: &str,
    session_name: &str,
    pane_ids: &[String],
    on_event: tauri::ipc::Channel<HostStreamEvent>,
) -> Result<HostStreamOpened, String> {
    use yuzora_host::herdr_limits::MAX_NDJSON_LINE_BYTES;
    use yuzora_host::herdr_service::{
        subscription_request, validate_api_response, HerdrSubscriptionEvent,
    };
    let runtime = connection.ssh_runtime(binary, ssh.clone())?;
    let session_name = session_name.to_owned();
    let mut host_cancelled = connection.cancelled.subscribe();
    if *host_cancelled.borrow() {
        return Err("host-disconnected".into());
    }
    let prepare = async {
        let caps = tokio::task::spawn_blocking(move || {
            runtime.capabilities_for_session(Some(&session_name))
        })
        .await
        .map_err(|e| e.to_string())?;
        if !caps.api.events_subscribe {
            return Err(caps
                .api
                .reason
                .unwrap_or_else(|| "events-unavailable".into()));
        }
        let socket = caps.server.socket_path.ok_or("herdr-socket-unavailable")?;
        let io = ssh.open_host_socket(session_id, &socket).await?;
        let mut reader = BufReader::new(io);
        let id = uuid::Uuid::new_v4().to_string();
        let mut request =
            serde_json::to_vec(&subscription_request(&id, pane_ids)?).map_err(|e| e.to_string())?;
        request.push(b'\n');
        reader
            .get_mut()
            .write_all(&request)
            .await
            .map_err(|e| e.to_string())?;
        reader.get_mut().flush().await.map_err(|e| e.to_string())?;
        let bytes = yuzora_host::wire::read_frame_limit(&mut reader, MAX_NDJSON_LINE_BYTES + 1)
            .await?
            .ok_or("herdr-socket-closed")?;
        let ack =
            validate_api_response(serde_json::from_slice(&bytes).map_err(|e| e.to_string())?)?;
        if ack.get("id").and_then(Value::as_str) != Some(&id)
            || ack.pointer("/result/type").and_then(Value::as_str) != Some("subscription_started")
        {
            return Err("invalid-subscription-ack".into());
        }
        Ok(reader)
    };
    let reader = tokio::select! {
        biased;
        _ = host_cancelled.changed() => return Err("host-disconnected".into()),
        result = tokio::time::timeout(Duration::from_secs(30), prepare) => result.map_err(|_| "events-open-timeout")??,
    };
    let stream_id = uuid::Uuid::new_v4().to_string();
    let (sender, mut requests) = mpsc::channel::<Queued>(STREAM_QUEUE_CAPACITY);
    let (cancelled, cancel_rx) = watch::channel(false);
    let stream = Arc::new(HostStreamSession { sender, cancelled });
    {
        let mut streams = connection.streams.lock().unwrap();
        if *connection.cancelled.borrow() {
            return Err("host-disconnected".into());
        }
        if streams.len() >= 32 {
            return Err("too-many-host-streams".into());
        }
        streams.insert(stream_id.clone(), stream);
    }
    let owner = connection.owner.clone();
    let resources = connection.streams.clone();
    let actor_id = stream_id.clone();
    tokio::spawn(async move {
        let emit = |event| {
            on_event
                .send(HostStreamEvent::Frame {
                    stream_id: actor_id.clone(),
                    frame: StreamFrame {
                        version: PROTOCOL_VERSION,
                        owner: owner.clone(),
                        payload: StreamPayload::Subscription { event },
                    },
                })
                .map_err(|e| e.to_string())
        };
        let operation = async {
            emit(HerdrSubscriptionEvent::Subscribed {
                subscription_id: actor_id.clone(),
            })?;
            read_ssh_events(reader, &actor_id, cancel_rx, host_cancelled, emit).await
        };
        tokio::pin!(operation);
        let status = loop {
            tokio::select! {
                result = &mut operation => break result,
                command = requests.recv() => {
                    let Some(command) = command else { break Ok(()); };
                    if matches!(command.operation, StreamCommand::Close) {
                        let _ = command.reply.send(Ok(Value::Null));
                        break Ok(());
                    }
                    let _ = command.reply.send(Err("events-stream-is-read-only".into()));
                }
            }
        };
        resources.lock().unwrap().remove(&actor_id);
        let _ = on_event.send(HostStreamEvent::Closed {
            stream_id: actor_id.clone(),
            owner: owner.clone(),
            reason: status.err().unwrap_or_else(|| "events-closed".into()),
        });
    });
    Ok(HostStreamOpened {
        stream_id: stream_id.clone(),
        value: Value::String(stream_id),
    })
}

async fn read_ssh_events(
    mut reader: BufReader<HostStream>,
    id: &str,
    mut cancelled: watch::Receiver<bool>,
    mut host_cancelled: watch::Receiver<bool>,
    on_event: impl Fn(yuzora_host::herdr_service::HerdrSubscriptionEvent) -> Result<(), String>,
) -> Result<(), String> {
    use yuzora_host::herdr_limits::MAX_NDJSON_LINE_BYTES;
    use yuzora_host::herdr_service::{parse_subscription_event_line, HerdrSubscriptionEvent};
    loop {
        if *cancelled.borrow() || *host_cancelled.borrow() {
            return Err("host-disconnected".into());
        }
        let bytes = tokio::select! {
            biased;
            _ = cancelled.changed() => return Ok(()),
            _ = host_cancelled.changed() => return Err("host-disconnected".into()),
            bytes = yuzora_host::wire::read_frame_limit(&mut reader, MAX_NDJSON_LINE_BYTES + 1) => bytes?.ok_or("herdr-socket-closed")?,
        };
        let line = std::str::from_utf8(&bytes).map_err(|_| "herdr-invalid-utf8")?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(event) = parse_subscription_event_line(id, line.trim())? {
            let terminal = matches!(event, HerdrSubscriptionEvent::Error { .. });
            on_event(event)?;
            if terminal {
                return Err("herdr-events-error".into());
            }
        }
    }
}

#[tauri::command]
pub async fn host_stream_command(
    state: tauri::State<'_, HostState>,
    owner: ConnectionOwner,
    stream_id: String,
    operation: StreamCommand,
) -> Result<Value, String> {
    if matches!(operation, StreamCommand::Open { .. }) {
        return Err("stream-already-open".into());
    }
    let connection = state.0.connection(&owner)?;
    let stream = connection
        .streams
        .lock()
        .unwrap()
        .get(&stream_id)
        .cloned()
        .ok_or("host-stream-missing")?;
    stream.request(operation).await
}

#[tauri::command]
pub async fn host_stream_close(
    state: tauri::State<'_, HostState>,
    owner: ConnectionOwner,
    stream_id: String,
) -> Result<(), String> {
    let connection = state.0.connection(&owner)?;
    if let Some(stream) = connection.streams.lock().unwrap().remove(&stream_id) {
        stream.cancelled.send_replace(true);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[allow(
        clippy::assertions_on_constants,
        reason = "This manual performance probe must reject debug binaries before measuring"
    )]
    #[tokio::test]
    #[ignore = "manual release-profile healthy stream throughput guard"]
    async fn performance_healthy_stream_roundtrips() {
        assert!(!cfg!(debug_assertions), "Use --release");
        fn cpu_ms() -> f64 {
            let usage = unsafe {
                let mut value = std::mem::MaybeUninit::<libc::rusage>::uninit();
                assert_eq!(libc::getrusage(libc::RUSAGE_SELF, value.as_mut_ptr()), 0);
                value.assume_init()
            };
            (usage.ru_utime.tv_sec + usage.ru_stime.tv_sec) as f64 * 1000.0
                + (usage.ru_utime.tv_usec + usage.ru_stime.tv_usec) as f64 / 1000.0
        }
        for (scenario, text, repetitions) in [
            ("key", "x".to_string(), 10000),
            ("command", "command argument\r".repeat(2), 10000),
            ("unicode", "中文😀".repeat(64), 2000),
            ("paste-64k", "x".repeat(65536), 128),
        ] {
            let owner = ConnectionOwner {
                host_id: "owned-throughput".into(),
                generation: 1,
            };
            let (client, peer) = tokio::io::duplex(8192);
            let (requests, queued) = mpsc::channel(2);
            let (cancel, cancelled) = watch::channel(false);
            let (host, host_cancelled) = watch::channel(false);
            let task = tokio::spawn(run(
                Box::new(client),
                owner.clone(),
                queued,
                cancelled,
                host_cancelled,
                |_| panic!("unexpected event"),
            ));
            let expected_text = text.clone();
            let peer_task = tokio::spawn(async move {
                let mut peer = BufReader::new(peer);
                let mut count = 0usize;
                while let Some(frame) = yuzora_host::wire::read_frame(&mut peer).await.unwrap() {
                    let request: StreamRequest = serde_json::from_slice(&frame).unwrap();
                    request.validate(&owner).unwrap();
                    count += 1;
                    assert_eq!(request.id, count.to_string());
                    let StreamCommand::Input {
                        text: Some(text),
                        bytes_base64: None,
                    } = request.operation
                    else {
                        panic!("expected input")
                    };
                    assert_eq!(text, expected_text);
                    let mut reply = serde_json::to_vec(&StreamFrame {
                        version: PROTOCOL_VERSION,
                        owner: owner.clone(),
                        payload: StreamPayload::Reply {
                            id: request.id,
                            outcome: Outcome::Ok { value: Value::Null },
                        },
                    })
                    .unwrap();
                    reply.push(b'\n');
                    peer.get_mut().write_all(&reply).await.unwrap();
                }
                count
            });
            for pass in 0..8 {
                let before = cpu_ms();
                let started = std::time::Instant::now();
                for _ in 0..repetitions {
                    let (reply, response) = oneshot::channel();
                    requests
                        .send(Queued {
                            operation: StreamCommand::Input {
                                text: Some(text.clone()),
                                bytes_base64: None,
                            },
                            reply,
                        })
                        .await
                        .unwrap();
                    assert_eq!(response.await.unwrap().unwrap(), Value::Null);
                }
                let wall_ms = started.elapsed().as_secs_f64() * 1000.0;
                println!(
                    "STREAM_THROUGHPUT_MEASUREMENT {}",
                    serde_json::json!({
                        "scenario": scenario, "pass": pass, "warmup": pass == 0,
                        "requests": repetitions, "wallMs": wall_ms, "cpuMs": cpu_ms() - before, "profile": "release"
                    })
                );
            }
            drop(requests);
            task.await.unwrap().unwrap();
            assert_eq!(peer_task.await.unwrap(), repetitions * 8);
            assert_eq!(cancel.receiver_count(), 0);
            assert_eq!(host.receiver_count(), 0);
        }
    }

    #[tokio::test]
    async fn cancellation_releases_blocked_writes_and_queued_replies_repeatedly() {
        use tokio::io::AsyncReadExt;
        for host_disconnect in [false, true] {
            for _ in 0..110 {
                let (client, mut peer) = tokio::io::duplex(64);
                let (requests, queued) = mpsc::channel(2);
                let (cancel, cancelled) = watch::channel(false);
                let (host, host_cancelled) = watch::channel(false);
                let mut task = tokio::spawn(run(
                    Box::new(client),
                    ConnectionOwner {
                        host_id: "owned-lifecycle".into(),
                        generation: 1,
                    },
                    queued,
                    cancelled,
                    host_cancelled,
                    |_| panic!("owned peer sends no frames"),
                ));
                let (reply, response) = oneshot::channel();
                requests
                    .send(Queued {
                        operation: StreamCommand::Input {
                            text: Some("x".repeat(16384)),
                            bytes_base64: None,
                        },
                        reply,
                    })
                    .await
                    .unwrap();
                let (tail_reply, tail_response) = oneshot::channel();
                requests
                    .send(Queued {
                        operation: StreamCommand::Close,
                        reply: tail_reply,
                    })
                    .await
                    .unwrap();
                let mut first = [0; 1];
                tokio::time::timeout(Duration::from_secs(1), peer.read_exact(&mut first))
                    .await
                    .unwrap()
                    .unwrap();
                if host_disconnect {
                    host.send_replace(true);
                } else {
                    cancel.send_replace(true);
                }
                let error = match tokio::time::timeout(Duration::from_millis(500), &mut task).await
                {
                    Ok(joined) => joined.unwrap().unwrap_err(),
                    Err(_) => {
                        task.abort();
                        let _ = task.await;
                        panic!("cancellation did not interrupt the owned blocked write");
                    }
                };
                assert_eq!(
                    error,
                    if host_disconnect {
                        "host-disconnected"
                    } else {
                        "host-stream-closed"
                    }
                );
                assert!(response.await.is_err());
                assert!(tail_response.await.is_err());
                assert!(requests.is_closed());
                assert_eq!(cancel.receiver_count(), 0);
                assert_eq!(host.receiver_count(), 0);
                let mut remainder = Vec::new();
                tokio::time::timeout(Duration::from_secs(1), peer.read_to_end(&mut remainder))
                    .await
                    .unwrap()
                    .unwrap();
                assert!(remainder.len() <= 64);
                assert!(!remainder.contains(&b'\n'));
            }
            println!(
                "STREAM_CANCEL_LIFECYCLE {}",
                serde_json::json!({
                    "hostDisconnect": host_disconnect, "warmupCycles": 10, "measuredCycles": 100,
                    "pendingAndQueuedRepliesClosed": true, "requestReceiverClosed": true,
                    "watchReceiversAfterClose": 0, "completedTailCommands": 0
                })
            );
        }
    }

    #[tokio::test]
    async fn normal_requests_preserve_order_and_fragmented_events() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let owner = ConnectionOwner {
            host_id: "owned-roundtrip".into(),
            generation: 7,
        };
        let (client, peer) = tokio::io::duplex(64);
        let (requests, queued) = mpsc::channel(2);
        let (cancel, cancelled) = watch::channel(false);
        let (host, host_cancelled) = watch::channel(false);
        let events = Arc::new(AtomicUsize::new(0));
        let observed = events.clone();
        let task = tokio::spawn(run(
            Box::new(client),
            owner.clone(),
            queued,
            cancelled,
            host_cancelled,
            move |frame| {
                assert!(matches!(frame.payload, StreamPayload::Files { .. }));
                observed.fetch_add(1, Ordering::Relaxed);
                Ok(())
            },
        ));
        let peer_task = tokio::spawn(async move {
            let mut peer = BufReader::new(peer);
            for index in 1..=2 {
                let bytes = yuzora_host::wire::read_frame(&mut peer)
                    .await
                    .unwrap()
                    .unwrap();
                let request: StreamRequest = serde_json::from_slice(&bytes).unwrap();
                request.validate(&owner).unwrap();
                assert_eq!(request.id, index.to_string());
                let StreamCommand::Input {
                    text: Some(text),
                    bytes_base64: None,
                } = request.operation
                else {
                    panic!("expected input")
                };
                assert_eq!(text, format!("輸入-{index}-😀"));
                for payload in [
                    StreamPayload::Files {
                        workspace_root: "owned".into(),
                        paths: vec!["file.ts".into()],
                    },
                    StreamPayload::Reply {
                        id: request.id,
                        outcome: Outcome::Ok {
                            value: serde_json::json!(index),
                        },
                    },
                ] {
                    let mut frame = serde_json::to_vec(&StreamFrame {
                        version: PROTOCOL_VERSION,
                        owner: owner.clone(),
                        payload,
                    })
                    .unwrap();
                    frame.push(b'\n');
                    for part in frame.chunks(7) {
                        peer.get_mut().write_all(part).await.unwrap();
                        tokio::task::yield_now().await;
                    }
                }
            }
            assert!(yuzora_host::wire::read_frame(&mut peer)
                .await
                .unwrap()
                .is_none());
        });
        let mut responses = Vec::new();
        for index in 1..=2 {
            let (reply, response) = oneshot::channel();
            requests
                .send(Queued {
                    operation: StreamCommand::Input {
                        text: Some(format!("輸入-{index}-😀")),
                        bytes_base64: None,
                    },
                    reply,
                })
                .await
                .unwrap();
            responses.push(response);
        }
        for (index, response) in responses.into_iter().enumerate() {
            assert_eq!(
                tokio::time::timeout(Duration::from_secs(2), response)
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap(),
                serde_json::json!(index + 1)
            );
        }
        drop(requests);
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        peer_task.await.unwrap();
        assert_eq!(events.load(Ordering::Relaxed), 2);
        assert_eq!(cancel.receiver_count(), 0);
        assert_eq!(host.receiver_count(), 0);
    }

    #[tokio::test]
    #[ignore = "manual wall-clock backpressure cancellation measurement"]
    async fn performance_cancel_during_backpressured_stream_write() {
        use tokio::io::AsyncReadExt;
        for host_disconnect in [false, true] {
            for sample in 0..6 {
                let (client, mut peer) = tokio::io::duplex(64);
                let (requests, queued) = mpsc::channel(2);
                let (cancel, cancelled) = watch::channel(false);
                let (host, host_cancelled) = watch::channel(false);
                let task = tokio::spawn(run(
                    Box::new(client),
                    ConnectionOwner {
                        host_id: "owned-perf".into(),
                        generation: 1,
                    },
                    queued,
                    cancelled,
                    host_cancelled,
                    |_| panic!("owned peer sends no frames"),
                ));
                let (reply, response) = oneshot::channel();
                requests
                    .send(Queued {
                        operation: StreamCommand::Input {
                            text: Some("x".repeat(256 * 1024)),
                            bytes_base64: None,
                        },
                        reply,
                    })
                    .await
                    .unwrap();
                let (tail_reply, tail_response) = oneshot::channel();
                requests
                    .send(Queued {
                        operation: StreamCommand::Close,
                        reply: tail_reply,
                    })
                    .await
                    .unwrap();
                let mut first = [0; 1];
                tokio::time::timeout(Duration::from_secs(2), peer.read_exact(&mut first))
                    .await
                    .unwrap()
                    .unwrap();
                let started = std::time::Instant::now();
                if host_disconnect {
                    host.send_replace(true);
                } else {
                    cancel.send_replace(true);
                }
                let error = tokio::time::timeout(Duration::from_secs(12), task)
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap_err();
                let cancel_ms = started.elapsed().as_secs_f64() * 1000.0;
                assert!(response.await.is_err());
                assert!(tail_response.await.is_err());
                let mut remainder = Vec::new();
                tokio::time::timeout(Duration::from_secs(1), peer.read_to_end(&mut remainder))
                    .await
                    .unwrap()
                    .unwrap();
                assert!(remainder.len() <= 64);
                assert!(!remainder.contains(&b'\n'));
                println!(
                    "STREAM_CANCEL_MEASUREMENT {}",
                    serde_json::json!({
                        "hostDisconnect": host_disconnect, "sample": sample, "warmup": sample == 0,
                        "cancelMs": cancel_ms, "error": error, "partialBytes": remainder.len() + 1,
                        "pendingReplyClosed": true, "queuedReplyClosed": true,
                        "profile": if cfg!(debug_assertions) { "debug" } else { "release" }
                    })
                );
            }
        }
    }

    #[tokio::test]
    async fn ssh_events_preserve_fragmented_frames_and_cancel_partial_read() {
        use yuzora_host::herdr_service::HerdrSubscriptionEvent;
        let (client, mut server) = tokio::io::duplex(4096);
        let (cancel, cancelled) = watch::channel(false);
        let (_host, host_cancelled) = watch::channel(false);
        let (events, mut output) = mpsc::channel(2);
        let task = tokio::spawn(read_ssh_events(
            BufReader::new(Box::new(client) as HostStream),
            "host-a",
            cancelled,
            host_cancelled,
            move |event| events.try_send(event).map_err(|e| e.to_string()),
        ));
        server.write_all(b"{\"event\":\"workspace.").await.unwrap();
        tokio::task::yield_now().await;
        server
            .write_all(b"created\",\"data\":{\"workspace_id\":\"same\"}}\n{\"event\":")
            .await
            .unwrap();
        let event = output.recv().await.unwrap();
        assert!(
            matches!(event, HerdrSubscriptionEvent::TopologyChanged { subscription_id, workspace_id: Some(workspace), .. } if subscription_id == "host-a" && workspace == "same")
        );
        cancel.send_replace(true);
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
    #[tokio::test]
    async fn disconnect_interrupts_idle_reader_without_waiting_for_output() {
        let (client, _server) = tokio::io::duplex(1024);
        let (_tx, rx) = mpsc::channel(1);
        let (_cancel, cancelled) = watch::channel(false);
        let (host, host_cancelled) = watch::channel(false);
        let task = tokio::spawn(run(
            Box::new(client),
            ConnectionOwner {
                host_id: "a".into(),
                generation: 1,
            },
            rx,
            cancelled,
            host_cancelled,
            |_| Ok(()),
        ));
        host.send_replace(true);
        let error = tokio::time::timeout(Duration::from_millis(500), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert!(matches!(
            error.as_str(),
            "host-stream-closed" | "host-disconnected"
        ));
    }
}
