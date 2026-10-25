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

struct Writing {
    bytes: Vec<u8>,
    written: usize,
    deadline: tokio::time::Instant,
}
impl Writing {
    fn new(request: &StreamRequest) -> Result<Self, String> {
        let mut bytes = serde_json::to_vec(request).map_err(|e| e.to_string())?;
        if bytes.len() >= MAX_FRAME_BYTES {
            return Err("frame-too-large".into());
        }
        bytes.push(b'\n');
        Ok(Self {
            bytes,
            written: 0,
            deadline: tokio::time::Instant::now() + Duration::from_secs(10),
        })
    }

    // write(), unlike write_all(), is cancellation-safe in select!. Record each
    // completed chunk before yielding, so incoming frames never replay bytes.
    async fn advance<W: AsyncWrite + Unpin>(&mut self, output: &mut W) -> Result<bool, String> {
        if self.written < self.bytes.len() {
            let count = output
                .write(&self.bytes[self.written..])
                .await
                .map_err(|e| e.to_string())?;
            if count == 0 {
                return Err(std::io::Error::from(std::io::ErrorKind::WriteZero).to_string());
            }
            self.written += count;
            Ok(false)
        } else {
            output.flush().await.map_err(|e| e.to_string())?;
            Ok(true)
        }
    }
}

struct Pending {
    id: String,
    reply: Reply,
    deadline: Option<tokio::time::Instant>,
    // A peer may reply before our flush finishes. Do not expose success until
    // the write also succeeds, and keep subsequent commands queued meanwhile.
    early_outcome: Option<Outcome>,
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
    let mut pending: Option<Pending> = None;
    let mut writing: Option<Writing> = None;
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
                let deadline = writing
                    .as_ref()
                    .map(|w| w.deadline)
                    .or_else(|| pending.as_ref().and_then(|p| p.deadline));
                tokio::select! {
                    biased;
                    _ = tokio::time::sleep_until(deadline.unwrap_or_else(tokio::time::Instant::now)), if deadline.is_some() => {
                        return Err(if writing.is_some() { "stream-write-timeout" } else { "stream-request-timeout" }.into());
                    },
                    completed = async { writing.as_mut().unwrap().advance(&mut write).await }, if writing.is_some() => {
                        if completed? {
                            writing = None;
                            let request = pending.as_mut().unwrap();
                            if let Some(outcome) = request.early_outcome.take() {
                                let request = pending.take().unwrap();
                                let _ = request.reply.send(result(outcome));
                            } else {
                                request.deadline = Some(tokio::time::Instant::now() + Duration::from_secs(15));
                            }
                        }
                    },
                    request = requests.recv(), if pending.is_none() => {
                        let Some(request) = request else {return Ok(());};
                        if request.reply.is_closed() { continue; }
                        next_id += 1;
                        let id = next_id.to_string();
                        writing = Some(Writing::new(&StreamRequest {version:PROTOCOL_VERSION,owner:owner.clone(),id:id.clone(),operation:request.operation})?);
                        pending = Some(Pending {id,reply:request.reply,deadline:None,early_outcome:None});
                    },
                    // Drain output even while input is backpressured. Keep this
                    // same read future until its entire frame has been received.
                    frame = &mut next => break frame?.ok_or("host-stream-ended")?,
                }
            };
            let frame = parse(&bytes, &owner)?;
            match frame.payload {
                StreamPayload::Reply { id, outcome } => {
                    let request = pending.as_mut().ok_or("unexpected-stream-response")?;
                    if id != request.id {
                        return Err("stream-response-id-mismatch".into());
                    }
                    if writing.is_some() {
                        if request.early_outcome.replace(outcome).is_some() {
                            return Err("unexpected-stream-response".into());
                        }
                    } else {
                        let request = pending.take().unwrap();
                        let _ = request.reply.send(result(outcome));
                    }
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

    #[tokio::test]
    async fn early_reply_waits_for_flush_and_cannot_hide_write_failure() {
        use futures_util::task::AtomicWaker;
        use std::pin::Pin;
        use std::sync::atomic::{AtomicU8, Ordering};
        use std::task::{Context, Poll};
        use tokio::io::{AsyncRead, ReadBuf};

        struct FlushGate {
            inner: tokio::io::DuplexStream,
            state: Arc<AtomicU8>,
            wake: Arc<AtomicWaker>,
        }
        impl AsyncRead for FlushGate {
            fn poll_read(
                mut self: Pin<&mut Self>,
                cx: &mut Context<'_>,
                buf: &mut ReadBuf<'_>,
            ) -> Poll<std::io::Result<()>> {
                Pin::new(&mut self.inner).poll_read(cx, buf)
            }
        }
        impl AsyncWrite for FlushGate {
            fn poll_write(
                mut self: Pin<&mut Self>,
                cx: &mut Context<'_>,
                buf: &[u8],
            ) -> Poll<std::io::Result<usize>> {
                Pin::new(&mut self.inner).poll_write(cx, buf)
            }
            fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
                self.wake.register(cx.waker());
                match self.state.load(Ordering::SeqCst) {
                    0 => Poll::Pending,
                    1 => Poll::Ready(Ok(())),
                    _ => Poll::Ready(Err(std::io::Error::other("owned-flush-failed"))),
                }
            }
            fn poll_shutdown(
                mut self: Pin<&mut Self>,
                cx: &mut Context<'_>,
            ) -> Poll<std::io::Result<()>> {
                Pin::new(&mut self.inner).poll_shutdown(cx)
            }
        }

        for fail_flush in [false, true] {
            let owner = ConnectionOwner {
                host_id: "owned-flush".into(),
                generation: 1,
            };
            let (client, peer) = tokio::io::duplex(4096);
            let state = Arc::new(AtomicU8::new(0));
            let wake = Arc::new(AtomicWaker::new());
            let (requests, queued) = mpsc::channel(2);
            let (_cancel, cancelled) = watch::channel(false);
            let (_host, host_cancelled) = watch::channel(false);
            let (events, mut observed) = mpsc::channel(1);
            let task = tokio::spawn(run(
                Box::new(FlushGate {
                    inner: client,
                    state: state.clone(),
                    wake: wake.clone(),
                }),
                owner.clone(),
                queued,
                cancelled,
                host_cancelled,
                move |frame| events.try_send(frame).map_err(|e| e.to_string()),
            ));
            let (reply, mut response) = oneshot::channel();
            requests
                .try_send(Queued {
                    operation: StreamCommand::Resize { cols: 80, rows: 24 },
                    reply,
                })
                .unwrap();
            let mut peer = BufReader::new(peer);
            let bytes = tokio::time::timeout(
                Duration::from_secs(1),
                yuzora_host::wire::read_frame(&mut peer),
            )
            .await
            .unwrap()
            .unwrap()
            .unwrap();
            let request: StreamRequest = serde_json::from_slice(&bytes).unwrap();
            request.validate(&owner).unwrap();
            for payload in [
                StreamPayload::Reply {
                    id: request.id,
                    outcome: Outcome::Ok { value: Value::Null },
                },
                StreamPayload::Files {
                    workspace_root: "owned".into(),
                    paths: vec!["after-reply".into()],
                    git_relevant: true,
                },
            ] {
                let mut bytes = serde_json::to_vec(&StreamFrame {
                    version: PROTOCOL_VERSION,
                    owner: owner.clone(),
                    payload,
                })
                .unwrap();
                bytes.push(b'\n');
                peer.get_mut().write_all(&bytes).await.unwrap();
            }
            // Seeing the following frame proves the ack was read while flush waits.
            let frame = tokio::time::timeout(Duration::from_secs(1), observed.recv())
                .await
                .unwrap()
                .unwrap();
            assert!(
                matches!(frame.payload, StreamPayload::Files { paths, .. } if paths == ["after-reply"])
            );
            assert!(matches!(
                response.try_recv(),
                Err(oneshot::error::TryRecvError::Empty)
            ));
            state.store(if fail_flush { 2 } else { 1 }, Ordering::SeqCst);
            wake.wake();
            let reply = tokio::time::timeout(Duration::from_secs(1), response)
                .await
                .unwrap();
            drop(requests);
            let status = tokio::time::timeout(Duration::from_secs(1), task)
                .await
                .unwrap()
                .unwrap();
            if fail_flush {
                assert!(reply.is_err(), "ack cannot conceal failed flush");
                assert_eq!(status.unwrap_err(), "owned-flush-failed");
            } else {
                assert_eq!(reply.unwrap().unwrap(), Value::Null);
                status.unwrap();
            }
        }
    }

    #[tokio::test]
    async fn simultaneous_backpressure_keeps_both_directions_live() {
        use std::cell::Cell;

        const CAPACITY: usize = 256;
        let owner = ConnectionOwner {
            host_id: "owned-two-way".into(),
            generation: 1,
        };
        let payload = StreamPayload::Files {
            workspace_root: "owned".into(),
            paths: vec!["x".repeat(200)],
            git_relevant: true,
        };
        let mut event = serde_json::to_vec(&StreamFrame {
            version: PROTOCOL_VERSION,
            owner: owner.clone(),
            payload,
        })
        .unwrap();
        event.push(b'\n');
        assert!(CAPACITY < event.len() && event.len() <= CAPACITY * 2);
        let (client, mut peer) = tokio::io::duplex(CAPACITY);
        // The helper is already blocked emitting an event before it reads input.
        peer.write_all(&event[..CAPACITY]).await.unwrap();
        let (requests, queued) = mpsc::channel(1);
        let (reply, response) = oneshot::channel();
        requests
            .try_send(Queued {
                operation: StreamCommand::Input {
                    text: Some("paste".repeat(256)),
                    bytes_base64: None,
                },
                reply,
            })
            .unwrap();
        let (_cancel, cancelled) = watch::channel(false);
        let (_host, host_cancelled) = watch::channel(false);
        let seen = Cell::new(0);
        let actor = run(
            Box::new(client),
            owner.clone(),
            queued,
            cancelled,
            host_cancelled,
            |frame| {
                let StreamPayload::Files { paths, .. } = frame.payload else {
                    panic!("expected event");
                };
                if paths == ["end"] {
                    return Err("owned-complete".into());
                }
                assert_eq!(paths, ["x".repeat(200)]);
                seen.set(seen.get() + 1);
                Ok(())
            },
        );
        let peer = async {
            peer.write_all(&event[CAPACITY..]).await.unwrap();
            let mut peer = BufReader::new(peer);
            let bytes = yuzora_host::wire::read_frame(&mut peer)
                .await
                .unwrap()
                .unwrap();
            assert!(bytes.len() > CAPACITY);
            let request: StreamRequest = serde_json::from_slice(&bytes).unwrap();
            request.validate(&owner).unwrap();
            assert!(
                matches!(request.operation, StreamCommand::Input { text: Some(text), .. } if text == "paste".repeat(256))
            );
            for payload in [
                StreamPayload::Reply {
                    id: request.id,
                    outcome: Outcome::Ok { value: Value::Null },
                },
                StreamPayload::Files {
                    workspace_root: "owned".into(),
                    paths: vec!["end".into()],
                    git_relevant: true,
                },
            ] {
                let mut frame = serde_json::to_vec(&StreamFrame {
                    version: PROTOCOL_VERSION,
                    owner: owner.clone(),
                    payload,
                })
                .unwrap();
                frame.push(b'\n');
                peer.get_mut().write_all(&frame).await.unwrap();
            }
        };
        let (status, ()) = tokio::time::timeout(Duration::from_secs(1), async {
            tokio::join!(biased; actor, peer)
        })
        .await
        .expect("both directions must progress without waiting for the10s write timeout");
        assert_eq!(status.unwrap_err(), "owned-complete");
        assert_eq!(response.await.unwrap().unwrap(), Value::Null);
        assert_eq!(seen.get(), 1);
    }

    #[tokio::test]
    async fn ready_output_does_not_starve_a_queued_input() {
        use std::cell::Cell;
        use tokio::io::AsyncReadExt;

        let owner = ConnectionOwner {
            host_id: "owned-ready-burst".into(),
            generation: 1,
        };
        let mut frame = serde_json::to_vec(&StreamFrame {
            version: PROTOCOL_VERSION,
            owner: owner.clone(),
            payload: StreamPayload::Files {
                workspace_root: "owned".into(),
                paths: vec!["changed.ts".into()],
                git_relevant: true,
            },
        })
        .unwrap();
        frame.push(b'\n');
        const FRAMES: usize = 64;
        let burst = frame.repeat(FRAMES);
        let (client, mut peer) = tokio::io::duplex(burst.len() + 1024);
        peer.write_all(&burst).await.unwrap();
        let (requests, queued) = mpsc::channel(1);
        let (reply, response) = oneshot::channel();
        requests
            .try_send(Queued {
                operation: StreamCommand::Input {
                    text: Some("\u{3}".into()),
                    bytes_base64: None,
                },
                reply,
            })
            .unwrap();
        let (_cancel, cancelled) = watch::channel(false);
        let (_host, host_cancelled) = watch::channel(false);
        let seen = Cell::new(0);
        let result = run(
            Box::new(client),
            owner.clone(),
            queued,
            cancelled,
            host_cancelled,
            |_| {
                seen.set(seen.get() + 1);
                if seen.get() == FRAMES {
                    Err("owned-burst-complete".into())
                } else {
                    Ok(())
                }
            },
        )
        .await;
        assert_eq!(result.unwrap_err(), "owned-burst-complete");
        let mut sent = Vec::new();
        tokio::time::timeout(Duration::from_secs(1), peer.read_to_end(&mut sent))
            .await
            .unwrap()
            .unwrap();
        assert!(
            response.await.is_err(),
            "fixture ends without acknowledging the command"
        );
        println!(
            "STREAM_READY_BURST frames={} dispatchedBytes={}",
            seen.get(),
            sent.len()
        );
        assert!(
            !sent.is_empty(),
            "queued input was never dispatched while64frames were ready"
        );
        assert_eq!(sent.iter().filter(|byte| **byte == b'\n').count(), 1);
        let request: StreamRequest = serde_json::from_slice(&sent).unwrap();
        request.validate(&owner).unwrap();
        assert_eq!(request.id, "1");
        assert!(
            matches!(request.operation, StreamCommand::Input { text: Some(text), bytes_base64: None } if text == "\u{3}")
        );
    }

    #[tokio::test]
    async fn queued_input_and_resize_preserve_replies_during_ready_output() {
        use std::cell::Cell;
        use tokio::io::AsyncReadExt;

        let owner = ConnectionOwner {
            host_id: "owned-mixed-burst".into(),
            generation: 2,
        };
        let mut bytes = Vec::new();
        let mut append = |payload| {
            bytes.extend(
                serde_json::to_vec(&StreamFrame {
                    version: PROTOCOL_VERSION,
                    owner: owner.clone(),
                    payload,
                })
                .unwrap(),
            );
            bytes.push(b'\n');
        };
        const EVENTS_PER_REPLY: usize = 16;
        for id in 1..=STREAM_QUEUE_CAPACITY {
            for _ in 0..EVENTS_PER_REPLY {
                append(StreamPayload::Files {
                    workspace_root: "owned".into(),
                    paths: vec!["pulse".into()],
                    git_relevant: true,
                });
            }
            append(StreamPayload::Reply {
                id: id.to_string(),
                outcome: Outcome::Ok {
                    value: serde_json::json!(id),
                },
            });
        }
        append(StreamPayload::Files {
            workspace_root: "owned".into(),
            paths: vec!["end".into()],
            git_relevant: true,
        });
        let (client, mut peer) = tokio::io::duplex(bytes.len() + 1024);
        peer.write_all(&bytes).await.unwrap();
        let (requests, queued) = mpsc::channel(STREAM_QUEUE_CAPACITY);
        let mut responses = Vec::new();
        for index in 0..STREAM_QUEUE_CAPACITY {
            let (reply, response) = oneshot::channel();
            let operation = if index % 2 == 0 {
                StreamCommand::Input {
                    text: Some(format!("輸入-{index}-😀")),
                    bytes_base64: None,
                }
            } else {
                StreamCommand::Resize {
                    cols: 80 + index as u16,
                    rows: 24,
                }
            };
            requests.try_send(Queued { operation, reply }).unwrap();
            responses.push(response);
        }
        let (cancel, cancelled) = watch::channel(false);
        let (host, host_cancelled) = watch::channel(false);
        let events = Cell::new(0);
        let result = run(
            Box::new(client),
            owner.clone(),
            queued,
            cancelled,
            host_cancelled,
            |frame| {
                let StreamPayload::Files { paths, .. } = frame.payload else {
                    panic!("expected event")
                };
                if paths == ["end"] {
                    return Err("owned-burst-complete".into());
                }
                events.set(events.get() + 1);
                Ok(())
            },
        )
        .await;
        assert_eq!(result.unwrap_err(), "owned-burst-complete");
        for (index, response) in responses.into_iter().enumerate() {
            assert_eq!(
                response.await.unwrap().unwrap(),
                serde_json::json!(index + 1)
            );
        }
        let mut sent = Vec::new();
        tokio::time::timeout(Duration::from_secs(1), peer.read_to_end(&mut sent))
            .await
            .unwrap()
            .unwrap();
        let commands: Vec<StreamRequest> = sent
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_slice(line).unwrap())
            .collect();
        assert_eq!(commands.len(), STREAM_QUEUE_CAPACITY);
        for (index, command) in commands.into_iter().enumerate() {
            command.validate(&owner).unwrap();
            assert_eq!(command.id, (index + 1).to_string());
            match command.operation {
                StreamCommand::Input {
                    text: Some(text),
                    bytes_base64: None,
                } => {
                    assert_eq!(index % 2, 0);
                    assert_eq!(text, format!("輸入-{index}-😀"));
                }
                StreamCommand::Resize { cols, rows } => {
                    assert_eq!(index % 2, 1);
                    assert_eq!((cols, rows), (80 + index as u16, 24));
                }
                _ => panic!("unexpected command"),
            }
        }
        assert_eq!(events.get(), EVENTS_PER_REPLY * STREAM_QUEUE_CAPACITY);
        assert_eq!(cancel.receiver_count(), 0);
        assert_eq!(host.receiver_count(), 0);
        assert!(requests.is_closed());
    }

    #[tokio::test]
    async fn abandoned_queued_inputs_are_not_replayed_before_live_input() {
        use tokio::io::AsyncReadExt;

        let owner = ConnectionOwner {
            host_id: "owned-abandoned-input".into(),
            generation: 3,
        };
        let mut frame = serde_json::to_vec(&StreamFrame {
            version: PROTOCOL_VERSION,
            owner: owner.clone(),
            payload: StreamPayload::Files {
                workspace_root: "owned".into(),
                paths: vec!["end".into()],
                git_relevant: true,
            },
        })
        .unwrap();
        frame.push(b'\n');
        let (client, mut peer) = tokio::io::duplex(4096);
        peer.write_all(&frame).await.unwrap();
        let (requests, queued) = mpsc::channel(STREAM_QUEUE_CAPACITY);
        for _ in 0..STREAM_QUEUE_CAPACITY - 1 {
            let (reply, response) = oneshot::channel();
            drop(response);
            requests
                .try_send(Queued {
                    operation: StreamCommand::Input {
                        text: Some("abandoned".into()),
                        bytes_base64: None,
                    },
                    reply,
                })
                .unwrap();
        }
        let (reply, response) = oneshot::channel();
        requests
            .try_send(Queued {
                operation: StreamCommand::Input {
                    text: Some("live".into()),
                    bytes_base64: None,
                },
                reply,
            })
            .unwrap();
        let (_cancel, cancelled) = watch::channel(false);
        let (_host, host_cancelled) = watch::channel(false);
        let result = run(
            Box::new(client),
            owner.clone(),
            queued,
            cancelled,
            host_cancelled,
            |_| Err("owned-burst-complete".into()),
        )
        .await;
        assert_eq!(result.unwrap_err(), "owned-burst-complete");
        assert!(response.await.is_err());
        let mut sent = Vec::new();
        tokio::time::timeout(Duration::from_secs(1), peer.read_to_end(&mut sent))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(sent.iter().filter(|byte| **byte == b'\n').count(), 1);
        let command: StreamRequest = serde_json::from_slice(&sent).unwrap();
        command.validate(&owner).unwrap();
        assert_eq!(command.id, "1");
        assert!(
            matches!(command.operation, StreamCommand::Input { text: Some(text), bytes_base64: None } if text == "live")
        );
    }

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
                        git_relevant: true,
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
