//! WS v1: closed, scoped local Herdr mapping; no raw request forwarding.
//! Binary frames are u32-BE streamId || bytes. Client input requires an owned
//! Control/Full stream; observer/event streams are never writable.
use crate::{
    admin::{Event, Scope},
    gateway::Gateway,
    session::AuthContext,
};
use axum::{
    body::Body,
    http::{HeaderMap, Request, StatusCode},
    response::{IntoResponse, Response},
};
use futures_util::{SinkExt, StreamExt};
use hyper::upgrade::OnUpgrade;
use hyper_util::rt::TokioIo;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{collections::VecDeque, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::{mpsc, watch, OwnedSemaphorePermit, Semaphore},
};
use tokio_tungstenite::{
    tungstenite::{
        self,
        protocol::{CloseFrame, Role, WebSocketConfig},
        Message,
    },
    WebSocketStream,
};

pub(crate) const MAX_PEERS: usize = 16;
pub(crate) const MESSAGE_BYTES: usize = 64 * 1024;
pub(crate) const OUTBOUND_BYTES: usize = 4 * 1024 * 1024;
const WRITE_BUFFER_BYTES: usize = MESSAGE_BYTES + 1024;
const REPLY_HEADROOM: usize = 128 * 1024;
const AUTH_TIMEOUT: Duration = Duration::from_secs(5);
const PING_INTERVAL: Duration = Duration::from_secs(20);
const PONG_TIMEOUT: Duration = Duration::from_secs(20);
const CLOSE_TIMEOUT: Duration = Duration::from_millis(250);

type Socket = WebSocketStream<TokioIo<hyper::upgrade::Upgraded>>;
pub(crate) struct Upgrade {
    upgraded: OnUpgrade,
    headers: HeaderMap,
    _permit: OwnedSemaphorePermit,
}
#[derive(Clone)]
pub(crate) struct Upgrades {
    pending: mpsc::Sender<Upgrade>,
    permits: Arc<Semaphore>,
}
impl Upgrades {
    pub(crate) fn new() -> (Self, mpsc::Receiver<Upgrade>) {
        let (pending, receiver) = mpsc::channel(MAX_PEERS);
        (
            Self {
                pending,
                permits: Arc::new(Semaphore::new(MAX_PEERS)),
            },
            receiver,
        )
    }
    pub(crate) fn respond(&self, mut request: Request<Body>, gateway: &Gateway) -> Response {
        // Hyper already parsed HTTP; the library validates RFC 6455 headers and
        // computes the SHA-1 accept key. No second HTTP parser or detached task.
        let Ok(response) =
            tungstenite::handshake::server::create_response_with_body(&request, Body::empty)
        else {
            return (StatusCode::BAD_REQUEST, "Invalid WebSocket upgrade").into_response();
        };
        // Cheap pre-auth DoS filter only; this neither refreshes login idle nor
        // authorizes the peer. First-frame binding and registration revalidate.
        if !gateway
            .identity
            .lock()
            .is_ok_and(|identity| identity.has_session_cookie(request.headers()))
        {
            return (StatusCode::UNAUTHORIZED, "Authentication required").into_response();
        }
        let Ok(permit) = self.permits.clone().try_acquire_owned() else {
            return (StatusCode::SERVICE_UNAVAILABLE, "WebSocket limit reached").into_response();
        };
        // Only cookies survive the handshake. A supplied binding HEADER cannot
        // substitute for first-frame auth, and no URL/query ever carries binding.
        let mut headers = HeaderMap::new();
        for cookie in request.headers().get_all("cookie") {
            headers.append("cookie", cookie.clone());
        }
        let job = Upgrade {
            upgraded: hyper::upgrade::on(&mut request),
            headers,
            _permit: permit,
        };
        if self.pending.try_send(job).is_err() {
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
        response
    }
}

#[derive(Clone, Copy)]
struct Close {
    code: u16,
    reason: &'static str,
}
const INVALID: Close = Close {
    code: 4401,
    reason: "session-invalid",
};
const SHUTDOWN: Close = Close {
    code: 1001,
    reason: "shutdown",
};
const SLOW: Close = Close {
    code: 1013,
    reason: "slow-consumer",
};
const TRANSPORT: Close = Close {
    code: 1001,
    reason: "transport-closed",
};
fn close_frame(close: Close) -> Message {
    // Reasons go only to admin, never expose authentication failure details.
    Message::Close(Some(CloseFrame {
        code: close.code.into(),
        reason: "".into(),
    }))
}
/// Windows resets a socket closed with unread input and discards a close frame
/// the peer has not read yet. Half-close, then drain within the close budget.
/// Callers free the peer's slots first; shutdown keeps its own bounded close.
async fn linger<S: AsyncRead + AsyncWrite + Unpin>(
    mut socket: WebSocketStream<S>,
    stop: &mut watch::Receiver<u16>,
) {
    if *stop.borrow() != 0 {
        return;
    }
    let io = socket.get_mut();
    let drain = async {
        if io.shutdown().await.is_err() {
            return;
        }
        let mut buffer = [0; 4096];
        while matches!(io.read(&mut buffer).await, Ok(n) if n > 0) {}
    };
    tokio::select! {
        biased;
        _ = stop.changed() => {},
        _ = tokio::time::timeout(CLOSE_TIMEOUT, drain) => {},
    }
}
fn stop_reason(stop: &watch::Receiver<u16>) -> Close {
    if *stop.borrow() == 4401 {
        INVALID
    } else {
        SHUTDOWN
    }
}

// serde derive also decodes positional JSON arrays into structs; frames are objects.
fn from_object<T: serde::de::DeserializeOwned>(text: &str) -> Option<T> {
    text.trim_start()
        .starts_with('{')
        .then(|| serde_json::from_str(text).ok())
        .flatten()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthFrame {
    #[serde(rename = "type")]
    kind: String,
    binding: String,
}
fn authenticate(
    gateway: &Gateway,
    headers: &mut HeaderMap,
    message: Message,
) -> Option<AuthContext> {
    let Message::Text(text) = message else {
        return None;
    };
    let frame: AuthFrame = from_object(&text)?;
    if frame.kind != "auth" {
        return None;
    }
    headers.insert("x-yzc-binding", frame.binding.parse().ok()?);
    gateway.authenticate(headers, Scope::View)
}

// Registration owns cleanup even when JoinSet aborts the socket task. Peer IDs
// are non-secret connection counters, never cookie/session identifiers.
struct Peer {
    gateway: Arc<Gateway>,
    id: u64,
    device: String,
    reason: &'static str,
}
impl Drop for Peer {
    fn drop(&mut self) {
        if let Ok(mut identity) = self.gateway.identity.lock() {
            identity.unregister_peer(self.id);
        }
        self.gateway.events.try_emit(Event::session_closed(
            self.id,
            self.device.clone(),
            self.reason,
        ));
    }
}

pub(crate) async fn run(job: Upgrade, gateway: Arc<Gateway>, mut stop: watch::Receiver<u16>) {
    let Upgrade {
        upgraded,
        mut headers,
        _permit: permit,
    } = job;
    let io = tokio::select! {
        biased;
        _ = stop.changed() => return,
        result = tokio::time::timeout(AUTH_TIMEOUT, upgraded) => match result {
            Ok(Ok(io)) => io,
            _ => return,
        },
    };
    let config = WebSocketConfig::default()
        .read_buffer_size(4096)
        .write_buffer_size(0)
        .max_write_buffer_size(WRITE_BUFFER_BYTES)
        .max_message_size(Some(MESSAGE_BYTES))
        .max_frame_size(Some(MESSAGE_BYTES));
    let mut socket =
        WebSocketStream::from_raw_socket(TokioIo::new(io), Role::Server, Some(config)).await;
    let auth = tokio::select! {
        biased;
        _ = stop.changed() => Err(stop_reason(&stop)),
        frame = tokio::time::timeout(AUTH_TIMEOUT, socket.next()) => match frame {
            Ok(Some(Ok(message))) => authenticate(&gateway, &mut headers, message).ok_or(INVALID),
            _ => Err(INVALID),
        },
    };
    drop(headers);
    let ctx = match auth {
        Ok(ctx) => ctx,
        Err(close) => {
            let _ = tokio::time::timeout(CLOSE_TIMEOUT, socket.send(close_frame(close))).await;
            drop(permit);
            linger(socket, &mut stop).await;
            return;
        }
    };
    let registration =
        gateway.with_authorized(&ctx, Scope::View, |identity| identity.register_peer(&ctx));
    let (id, invalidated) = match registration {
        Ok(Some(registration)) => registration,
        result => {
            let close = if result.is_err() {
                INVALID
            } else {
                Close {
                    code: 1008,
                    reason: "peer-limit",
                }
            };
            let _ = tokio::time::timeout(CLOSE_TIMEOUT, socket.send(close_frame(close))).await;
            drop(permit);
            linger(socket, &mut stop).await;
            return;
        }
    };
    let mut peer = Peer {
        gateway: gateway.clone(),
        id,
        device: ctx.device_id().into(),
        reason: "aborted",
    };
    gateway
        .events
        .try_emit(Event::session_opened(id, peer.device.clone()));
    let (close, socket) = serve(socket, &gateway, &ctx, id, stop.clone(), invalidated).await;
    peer.reason = close.reason;
    drop(peer);
    drop(permit);
    linger(socket, &mut stop).await;
}

// Includes queued AND in-flight message bytes (+ conservative frame overhead).
// Reserve the separate tungstenite write-buffer budget inside the same 4 MiB cap.
// Queue overflow or a write stalled for 20s closes the whole socket with 1013.
// Phase 3B adds per-stream policies; no silent dropping or replay of messages.
pub(crate) struct Queued {
    message: Message,
    _bytes: OwnedSemaphorePermit,
    _batch: Option<Arc<OwnedSemaphorePermit>>,
}
pub(crate) struct Outbound {
    sender: mpsc::Sender<Queued>,
    pub(crate) bytes: Arc<Semaphore>,
    reply_bytes: Arc<Semaphore>,
    chunked: Arc<Semaphore>,
    #[cfg(test)]
    pub(crate) preloaded_streams:
        std::sync::Mutex<Option<(crate::read_bridge::Reads, crate::read_bridge::Frames)>>,
}
impl Outbound {
    pub(crate) fn new() -> (Self, mpsc::Receiver<Queued>) {
        let (sender, receiver) = mpsc::channel(256);
        (
            Self {
                sender,
                bytes: Arc::new(Semaphore::new(
                    OUTBOUND_BYTES - WRITE_BUFFER_BYTES - REPLY_HEADROOM,
                )),
                reply_bytes: Arc::new(Semaphore::new(REPLY_HEADROOM)),
                chunked: Arc::new(Semaphore::new(1)),
                #[cfg(test)]
                preloaded_streams: std::sync::Mutex::new(None),
            },
            receiver,
        )
    }
    fn send(&self, message: Message) -> Result<(), Close> {
        let size = message.len();
        if size > MESSAGE_BYTES {
            return Err(Close {
                code: 1009,
                reason: "payload-too-large",
            });
        }
        let permit = self.reply_permit(size + 16).map_err(|_| SLOW)?;
        self.sender
            .try_send(Queued {
                message,
                _bytes: permit,
                _batch: None,
            })
            .map_err(|_| SLOW)
    }
    fn reply_permit(&self, size: usize) -> Result<OwnedSemaphorePermit, &'static str> {
        // Only small replies/pings may use this reserve. Stream sinks and
        // chunked snapshots only see bytes, never reply_bytes.
        self.reply_bytes
            .clone()
            .try_acquire_many_owned(size as u32)
            .or_else(|_| self.bytes.clone().try_acquire_many_owned(size as u32))
            .map_err(|_| "busy")
    }
    /// Atomically reserve a complete response batch. No partial snapshot, no
    /// slow-consumer disconnect for an otherwise healthy peer under read load.
    pub(crate) fn batch(&self, replies: Vec<Value>) -> Result<(), &'static str> {
        if replies.is_empty() {
            return Ok(());
        }
        let chunked = replies[0]["result"]["chunked"] == true;
        let batch = if chunked {
            Some(Arc::new(
                self.chunked
                    .clone()
                    .try_acquire_owned()
                    .map_err(|_| "busy")?,
            ))
        } else {
            None
        };
        let messages: Vec<_> = replies
            .into_iter()
            .map(|v| Message::text(v.to_string()))
            .collect();
        if messages.iter().any(|m| m.len() > MESSAGE_BYTES) {
            return Err("payload-too-large");
        }
        if self.sender.capacity() < messages.len() {
            return Err("busy");
        }
        let total = messages.iter().map(|m| m.len() + 16).sum::<usize>();
        let mut permits = if chunked {
            self.bytes
                .clone()
                .try_acquire_many_owned(total as u32)
                .map_err(|_| "busy")?
        } else {
            self.reply_permit(total)?
        };
        for message in messages {
            let bytes = permits
                .split(message.len() + 16)
                .expect("reserved batch bytes");
            self.sender
                .try_send(Queued {
                    message,
                    _bytes: bytes,
                    _batch: batch.clone(),
                })
                .map_err(|_| "busy")?;
        }
        Ok(())
    }
    fn replies(&self, replies: Vec<Value>) -> Result<(), Close> {
        let id = replies
            .first()
            .map(|v| v["id"].clone())
            .unwrap_or(Value::Null);
        if let Err(code) = self.batch(replies) {
            // Small fixed reply fits the space reserved from stream producers.
            // Never label a rejected RPC batch as a slow consumer.
            self.batch(vec![rpc_error(id, code, code)])
                .map_err(|_| Close {
                    code: 1008,
                    reason: "reply-queue-full",
                })?;
        }
        Ok(())
    }
    fn event(&self, event: &'static str, data: Value) -> Result<(), Close> {
        self.send(Message::text(
            json!({"event":event,"data":data}).to_string(),
        ))
    }
}
async fn serve(
    socket: Socket,
    gateway: &Gateway,
    ctx: &AuthContext,
    peer_id: u64,
    mut stop: watch::Receiver<u16>,
    mut invalidated: watch::Receiver<bool>,
) -> (Close, Socket) {
    let (outbound, pending) = Outbound::new();
    drive(
        socket,
        gateway,
        ctx,
        peer_id,
        &mut stop,
        &mut invalidated,
        outbound,
        pending,
    )
    .await
}
#[allow(clippy::too_many_arguments)]
async fn drive<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    socket: WebSocketStream<S>,
    gateway: &Gateway,
    ctx: &AuthContext,
    peer_id: u64,
    stop: &mut watch::Receiver<u16>,
    invalidated: &mut watch::Receiver<bool>,
    outbound: Outbound,
    mut pending: mpsc::Receiver<Queued>,
) -> (Close, WebSocketStream<S>) {
    let (mut writer, mut reader) = socket.split();
    #[cfg(not(test))]
    let (mut reads, mut frames) =
        crate::read_bridge::Reads::new(outbound.bytes.clone(), invalidated.clone(), stop.clone());
    #[cfg(test)]
    let (mut reads, mut frames) = outbound
        .preloaded_streams
        .lock()
        .unwrap()
        .take()
        .unwrap_or_else(|| {
            crate::read_bridge::Reads::new(
                outbound.bytes.clone(),
                invalidated.clone(),
                stop.clone(),
            )
        });
    reads.peer_id = peer_id;
    let writing_invalidated = invalidated.clone();
    let writing_stop = stop.clone();
    let close = {
        let reading = async {
            // Explicit auth acknowledgement uses the v1 server event envelope.
            outbound.event("session.ready", json!({"protocolVersion":1}))?;
            let mut ping = tokio::time::interval_at(
                tokio::time::Instant::now() + PING_INTERVAL,
                PING_INTERVAL,
            );
            ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            let mut pong_due = None;
            let mut messages = VecDeque::from([tokio::time::Instant::now()]);
            loop {
                // subscribe + validation were atomic; retain covers invalidation
                // that arrived before this task first polled changed().
                if *invalidated.borrow() {
                    return Ok(INVALID);
                }
                if *stop.borrow() != 0 {
                    return Ok(stop_reason(stop));
                }
                outbound.replies(reads.cancelled_writes())?;
                for reply in reads.pump_writes(gateway) {
                    outbound.replies(vec![reply])?;
                }
                let write_deadline = reads.write_deadline(gateway);
                let pong_deadline =
                    pong_due.unwrap_or_else(|| tokio::time::Instant::now() + PONG_TIMEOUT);
                tokio::select! {
                    biased;
                    _ = stop.changed() => return Ok(stop_reason(stop)),
                    _ = invalidated.changed() => return Ok(INVALID),
                    _ = tokio::time::sleep_until(pong_deadline), if pong_due.is_some() => return Ok(Close { code: 1001, reason: "pong-timeout" }),
                    frame = reader.next() => {
                        let frame = match frame {
                            Some(Ok(frame)) => frame,
                            Some(Err(tungstenite::Error::Capacity(_))) => return Ok(Close { code: 1009, reason: "message-too-large" }),
                            Some(Err(tungstenite::Error::Protocol(_))) => return Ok(Close { code: 1002, reason: "invalid-frame" }),
                            Some(Err(tungstenite::Error::Utf8(_))) => return Ok(Close { code: 1007, reason: "invalid-utf8" }),
                            _ => return Ok(TRANSPORT),
                        };
                        // tungstenite yields complete messages, not continuation
                        // frames: this rate cap does NOT bound fragment parsing CPU.
                        // Fragmented payload memory remains bounded by 64 KiB.
                        let now = tokio::time::Instant::now();
                        while messages.front().is_some_and(|at| now.duration_since(*at) >= Duration::from_secs(1)) {
                            messages.pop_front();
                        }
                        if messages.len() >= 50 { return Ok(Close { code: 1008, reason: "message-rate" }); }
                        messages.push_back(now);
                        // Authenticated application activity refreshes login idle,
                        // including future binary input. Browser automatic pongs
                        // only maintain WS liveness and cannot keep a session alive.
                        if matches!(frame, Message::Text(_) | Message::Binary(_)) {
                            gateway.with_authorized(ctx, Scope::View, |_| ()).map_err(|_| INVALID)?;
                        }
                        match frame {
                            Message::Text(text) => {
                                let response = rpc(gateway, ctx, &text)?;
                                let call = serde_json::from_str::<Call>(&text).ok();
                                if let Some(call) = call.filter(|c| crate::read_bridge::is_read(&c.method) && response["error"]["code"] == "not-implemented") {
                                    if let Some(response) = reads.call(gateway, ctx, call.id, &call.method, call.params).map_err(|_| INVALID)? {
                                        outbound.replies(vec![response])?;
                                    }
                                } else if let Some(call) = serde_json::from_str::<Call>(&text).ok().filter(|c| crate::read_bridge::is_write(&c.method) && response["error"]["code"] != "invalid-request") {
                                    if let Some(reply) = reads.write_call(gateway, ctx, call.id, &call.method, call.params).map_err(|_| INVALID)? { outbound.replies(vec![reply])?; }
                                } else {
                                    outbound.replies(vec![response])?;
                                }
                            },
                            Message::Pong(bytes) if bytes.as_ref() == b"yzc" => pong_due = None,
                            Message::Ping(_) => {}, // reader flushes the library's automatic Pong
                            Message::Close(_) => return Ok(Close { code: 1000, reason: "client-closed" }),
                            Message::Binary(bytes) => {
                                match reads.binary_call(gateway,ctx,&bytes) {
                                    Ok(Some(reply))=>outbound.replies(vec![reply])?,
                                    Ok(None)=>{},
                                    Err(())=>return Ok(Close {code:1003,reason:"invalid-input-stream"}),
                                }
                            },
                            _ => {},
                        }
                    },
                    Some(completed) = reads.completed.next(), if !reads.completed.is_empty() => {
                        outbound.replies(reads.finish(completed))?;
                        outbound.replies(reads.closed())?;
                    },
                    _ = gateway.clock.until(write_deadline) => {
                        for reply in reads.write_tick(gateway) { outbound.replies(vec![reply])?; }
                    },
                    _ = reads.changed.notified() => {
                        for event in reads.closed() { outbound.replies(vec![event])?; }
                    },
                    _ = ping.tick(), if pong_due.is_none() => {
                        outbound.send(Message::Ping(b"yzc".as_slice().into()))?;
                        pong_due = Some(tokio::time::Instant::now() + PONG_TIMEOUT);
                    },
                }
            }
        };
        let writing = async {
            loop {
                // Control responses have priority, including stream-open replies.
                // Permits stay alive through the in-flight write.
                let (message, _bytes, _frame) = tokio::select! {
                    biased;
                    queued = pending.recv() => match queued {
                        Some(queued) => (queued.message.clone(), Some(queued), None),
                        None => return TRANSPORT,
                    },
                    frame = frames.next() => match frame {
                        Some(frame) if frame.active() => (frame.message(), None, Some(frame)),
                        Some(_) => continue,
                        None => return TRANSPORT,
                    },
                };
                if *writing_invalidated.borrow() {
                    return INVALID;
                }
                if *writing_stop.borrow() != 0 {
                    return stop_reason(&writing_stop);
                }
                match tokio::time::timeout(PONG_TIMEOUT, writer.send(message)).await {
                    Ok(Ok(())) => {}
                    Err(_) => return SLOW,
                    _ => return TRANSPORT,
                }
            }
        };
        tokio::select! {
            biased;
            close = reading => close.unwrap_or_else(|close| close),
            close = writing => close,
        }
    };
    let dropped = reads.cancel_writes(close.reason);
    drop(reads); // invalidates callbacks and queues owned resource cleanup
                 // Only content-free cancellation errors may precede 4401. Never flush old
                 // data or successful replies after invalidation. One bounded best-effort window.
    let _ = tokio::time::timeout(CLOSE_TIMEOUT, async {
        for reply in dropped {
            let _ = writer.send(Message::text(reply.to_string())).await;
        }
    })
    .await;
    drop(frames);
    drop(outbound);
    drop(pending);
    // Best effort: a peer that never drains TCP cannot be guaranteed a close
    // frame. Always reclaim task/permits within the shutdown budget regardless.
    if close.reason == "client-closed" {
        // Receiving Close already queued the library's reply. A new send would
        // fail SendAfterClosing before flushing it. Echo None as None (1005 on
        // browser APIs), or the supplied code; admin records only the reason.
        let _ = tokio::time::timeout(CLOSE_TIMEOUT, writer.close()).await;
    } else {
        let _ = tokio::time::timeout(CLOSE_TIMEOUT, writer.send(close_frame(close))).await;
    }
    let socket = reader.reunite(writer).expect("halves of one socket");
    (close, socket)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Call {
    id: Value,
    method: String,
    #[serde(default)]
    params: Value,
}
struct Method {
    name: &'static str,
    scope: Scope,
}
const METHODS: &[Method] = &[
    Method {
        name: "hello",
        scope: Scope::View,
    },
    Method {
        name: "session.ping",
        scope: Scope::View,
    },
    Method {
        name: "sessions.list",
        scope: Scope::View,
    },
    Method {
        name: "snapshot.get",
        scope: Scope::View,
    },
    Method {
        name: "events.subscribe",
        scope: Scope::View,
    },
    Method {
        name: "terminal.observe",
        scope: Scope::View,
    },
    Method {
        name: "stream.close",
        scope: Scope::View,
    },
    Method {
        name: "push.subscribe",
        scope: Scope::View,
    },
    Method {
        name: "push.unsubscribe",
        scope: Scope::View,
    },
    Method {
        name: "push.status",
        scope: Scope::View,
    },
    Method {
        name: "git.status",
        scope: Scope::View,
    },
    Method {
        name: "git.branches",
        scope: Scope::View,
    },
    Method {
        name: "git.log",
        scope: Scope::View,
    },
    Method {
        name: "git.commit",
        scope: Scope::View,
    },
    Method {
        name: "git.diff",
        scope: Scope::View,
    },
    Method {
        name: "terminal.control",
        scope: Scope::Control,
    },
    Method {
        name: "terminal.input",
        scope: Scope::Control,
    },
    Method {
        name: "terminal.scroll",
        scope: Scope::Control,
    },
    Method {
        name: "terminal.release",
        scope: Scope::Control,
    },
    Method {
        name: "agent.prompt",
        scope: Scope::Control,
    },
    Method {
        name: "pane.sendKeys",
        scope: Scope::Control,
    },
    Method {
        name: "full.open",
        scope: Scope::Full,
    },
    Method {
        name: "full.input",
        scope: Scope::Full,
    },
    Method {
        name: "full.resize",
        scope: Scope::Full,
    },
    Method {
        name: "full.close",
        scope: Scope::Full,
    },
];
fn rpc_error(id: Value, code: &'static str, message: &'static str) -> Value {
    json!({"id":id,"error":{"code":code,"message":message}})
}
fn rpc(gateway: &Gateway, ctx: &AuthContext, text: &str) -> Result<Value, Close> {
    let call = from_object::<Call>(text).filter(|c| {
        (c.id.as_u64().is_some() || c.id.as_str().is_some_and(|id| id.len() <= 128))
            && (c.params.is_null() || c.params.is_object())
    });
    let Some(call) = call else {
        gateway
            .with_authorized(ctx, Scope::View, |_| ())
            .map_err(|_| INVALID)?;
        return Ok(rpc_error(Value::Null, "invalid-request", "Invalid request"));
    };
    let method = METHODS.iter().find(|m| m.name == call.method);
    let decision = gateway.with_authorized(ctx, method.map_or(Scope::View, |m| m.scope), |_| {
        match method.map(|m| m.name) {
            None => rpc_error(call.id.clone(), "unknown-method", "Unknown method"),
            Some("hello") => json!({"id":call.id,"result":{"protocolVersion":1,"serverTime":gateway.clock.now()}}),
            Some("session.ping") => json!({"id":call.id,"result":{"serverTime":gateway.clock.now()}}),
            Some(name @ ("push.subscribe" | "push.unsubscribe" | "push.status")) => {
                match gateway.push.call(gateway, ctx.device_id(), name, &call.params) {
                    Ok(result) => json!({"id":call.id,"result":result}),
                    Err(code) => rpc_error(call.id.clone(), code, code),
                }
            }
            _ => rpc_error(call.id.clone(), "not-implemented", "Not implemented"),
        }
    });
    match decision {
        Ok(mut reply) => {
            if call.method == "hello" {
                let mut scopes = Vec::new();
                for scope in [Scope::View, Scope::Control, Scope::Full] {
                    match gateway.with_authorized(ctx, scope, |_| ()) {
                        Ok(()) => scopes.push(scope),
                        Err((StatusCode::FORBIDDEN, _)) => {}
                        Err(_) => return Err(INVALID),
                    }
                }
                reply["result"]["scopes"] = json!(scopes);
                reply["result"]["capabilities"] = crate::read_bridge::capabilities();
                reply["result"]["push"] = gateway.push.hello();
            }
            Ok(reply)
        }
        Err(crate::webauthn::SCOPE_NOT_GRANTED) => {
            Ok(rpc_error(call.id, "scope-not-granted", "Scope not granted"))
        }
        Err((StatusCode::FORBIDDEN, _)) => Ok(rpc_error(
            call.id,
            "elevation-required",
            "Elevation required",
        )),
        Err(_) => Err(INVALID),
    }
}

#[cfg(test)]
pub(crate) async fn drive_for_test<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    socket: WebSocketStream<S>,
    gateway: &Gateway,
    ctx: &AuthContext,
    mut stop: watch::Receiver<u16>,
    mut invalidated: watch::Receiver<bool>,
    outbound: Outbound,
    pending: mpsc::Receiver<Queued>,
) -> u16 {
    drive(
        socket,
        gateway,
        ctx,
        0,
        &mut stop,
        &mut invalidated,
        outbound,
        pending,
    )
    .await
    .0
    .code
}

#[cfg(test)]
#[path = "bridge_tests.rs"]
mod tests;
