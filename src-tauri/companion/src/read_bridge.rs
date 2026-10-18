//! Peer-owned read RPCs and observer resources; callbacks never block or write IO.
use crate::{
    admin::Scope,
    bridge::{MESSAGE_BYTES, OUTBOUND_BYTES},
    gateway::Gateway,
    herdr::{self, Resource},
    herdr_jobs::Lease,
    session::AuthContext,
};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use futures_util::{future::BoxFuture, stream::FuturesUnordered};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
use tokio::sync::{mpsc, watch, Notify, OwnedSemaphorePermit, Semaphore};
use tokio_tungstenite::tungstenite::Message;
use yuzora_host::herdr_service::{HerdrSubscriptionEvent, HerdrTerminalEvent};

pub(crate) const MAX_STREAMS: usize = 8;
pub(crate) const STREAM_FRAMES: usize = 256;
const STREAM_BYTES: usize = 384 * 1024;
pub(crate) const CHUNK_BYTES: usize = 36 * 1024; // base64 <=48 KiB, envelope <64 KiB
const INLINE_BYTES: usize = 48 * 1024;
#[path = "write_bridge.rs"]
mod writes;
pub(crate) use writes::is_write;

pub(crate) fn methods() -> Vec<&'static str> {
    herdr::METHOD_MAPPING
        .iter()
        .map(|(method, _, _)| *method)
        .collect()
}
pub(crate) fn capabilities() -> Value {
    json!({"methods":methods(),"limits":{"messageBytes":MESSAGE_BYTES,"snapshotBytes":herdr::SNAPSHOT_BYTES,"snapshotChunkBytes":CHUNK_BYTES,"streams":MAX_STREAMS,"streamFrames":STREAM_FRAMES,"streamBytes":STREAM_BYTES,"outboundBytes":OUTBOUND_BYTES,"pendingCalls":2,"globalHerdrReadCalls":crate::herdr_jobs::MAX_CALLS,"globalHerdrWriteCalls":2,"globalHerdrCleanupCalls":2,"deviceReadCalls":2,"deviceWriteCalls":1,"writeQueue":16},"snapshotChunks":"base64-json-v1","observeSize":"layout-rect-not-pty","controlSize":"confirmation-required","fullSize":{"minCols":20,"maxCols":400,"minRows":8,"maxRows":200}})
}
pub(crate) fn is_read(method: &str) -> bool {
    matches!(
        method,
        "sessions.list" | "snapshot.get" | "events.subscribe" | "terminal.observe" | "stream.close"
    )
}
fn error(id: Value, code: &'static str) -> Value {
    json!({"id":id,"error":{"code":code,"message":code}})
}
fn reply(id: Value, value: Value) -> Value {
    json!({"id":id,"result":value})
}

struct Life {
    alive: AtomicBool,
    invalidated: watch::Receiver<bool>,
    stop: watch::Receiver<u16>,
}
impl Life {
    fn active(&self) -> bool {
        self.alive.load(Ordering::SeqCst) && !*self.invalidated.borrow() && *self.stop.borrow() == 0
    }
}
struct StreamState {
    life: Arc<Life>,
    closed: Mutex<Option<&'static str>>,
    ready: AtomicBool,
    ready_changed: Notify,
    changed: Arc<Notify>,
    slots: Arc<Semaphore>,
    bytes: Arc<Semaphore>,
    deadline: Mutex<Option<(crate::clock::Clock, crate::clock::Timestamp)>>,
}
impl StreamState {
    fn active(&self) -> bool {
        self.life.active()
            && self.closed.lock().unwrap().is_none()
            && self
                .deadline
                .lock()
                .unwrap()
                .as_ref()
                .is_none_or(|(clock, until)| clock.now() < *until)
    }
    fn close(&self, reason: &'static str) {
        self.closed.lock().unwrap().get_or_insert(reason);
        self.changed.notify_one();
        self.ready_changed.notify_one();
    }
}
pub(crate) struct StreamFrame {
    message: Message,
    state: Arc<StreamState>,
    _bytes: OwnedSemaphorePermit,
    _frame: OwnedSemaphorePermit,
    _stream_bytes: OwnedSemaphorePermit,
}
impl StreamFrame {
    pub(crate) fn active(&self) -> bool {
        self.state.active()
    }
    pub(crate) fn message(&self) -> Message {
        self.message.clone()
    }
}
/// Kept across select iterations: control traffic cannot cancel and drop a frame
/// waiting for the open response. Closed-stream frames are drained, never replayed.
pub(crate) struct Frames {
    receiver: mpsc::Receiver<StreamFrame>,
    held: Option<StreamFrame>,
}
impl Frames {
    pub(crate) async fn next(&mut self) -> Option<StreamFrame> {
        loop {
            if self.held.is_none() {
                self.held = self.receiver.recv().await;
            }
            let frame = self.held.as_ref()?;
            if !frame.active() {
                self.held = None;
                continue;
            }
            if !frame.state.ready.load(Ordering::SeqCst) {
                frame.state.ready_changed.notified().await;
                continue;
            }
            return self.held.take();
        }
    }
}

#[derive(Clone)]
struct Sink {
    id: u32,
    state: Arc<StreamState>,
    sender: mpsc::Sender<StreamFrame>,
    bytes: Arc<Semaphore>,
}
impl Sink {
    fn send(&self, message: Message) -> std::result::Result<(), String> {
        if !self.state.active() {
            return Err("stream-closed".into());
        }
        if message.len() > MESSAGE_BYTES {
            self.state.close("payload-too-large");
            return Err("payload-too-large".into());
        }
        let permits = self.state.slots.clone().try_acquire_owned().ok().zip(
            self.bytes
                .clone()
                .try_acquire_many_owned((message.len() + 16) as u32)
                .ok(),
        );
        let permits = permits.zip(
            self.state
                .bytes
                .clone()
                .try_acquire_many_owned((message.len() + 16) as u32)
                .ok(),
        );
        let Some(((frame, bytes), stream_bytes)) = permits else {
            self.state.close("slow-consumer");
            return Err("slow-consumer".into());
        };
        if self
            .sender
            .try_send(StreamFrame {
                message,
                state: self.state.clone(),
                _bytes: bytes,
                _frame: frame,
                _stream_bytes: stream_bytes,
            })
            .is_err()
        {
            self.state.close("slow-consumer");
            return Err("slow-consumer".into());
        }
        Ok(())
    }
    fn terminal(&self, event: HerdrTerminalEvent) -> std::result::Result<(), String> {
        match event {
            HerdrTerminalEvent::Frame {
                encoding,
                bytes_base64,
                ..
            } => {
                if encoding != "ansi" {
                    self.state.close("stream-closed");
                    return Err("stream-closed".into());
                }
                if bytes_base64.len() > (MESSAGE_BYTES - 4).div_ceil(3) * 4 {
                    self.state.close("payload-too-large");
                    return Err("payload-too-large".into());
                }
                let bytes = B64.decode(bytes_base64).map_err(|_| {
                    self.state.close("stream-closed");
                    "stream-closed".to_string()
                })?;
                let mut frame = self.id.to_be_bytes().to_vec();
                frame.extend(bytes);
                self.send(Message::binary(frame))
            }
            HerdrTerminalEvent::Closed { reason, .. } => {
                self.state.close(herdr::closed_reason(reason.as_deref()));
                Ok(())
            }
            HerdrTerminalEvent::Error { code, .. } if code == "tooLarge" => {
                self.state.close("payload-too-large");
                Err("payload-too-large".into())
            }
            HerdrTerminalEvent::Error { .. } | HerdrTerminalEvent::Resync { .. } => {
                self.state.close("stream-closed");
                Err("stream-closed".into())
            }
        }
    }
    /// Content-free hint that panes, tabs or Spaces changed: the client
    /// fetches a snapshot and subscribes again.
    fn stale(&self, session: &str) -> std::result::Result<(), String> {
        self.send(Message::text(
            json!({"event":"snapshot.stale","data":{"session":session},"stream":self.id})
                .to_string(),
        ))
    }
    fn event(
        &self,
        session: &str,
        event: HerdrSubscriptionEvent,
    ) -> std::result::Result<(), String> {
        let (event, data) = match event {
            HerdrSubscriptionEvent::AgentStatusChanged {
                pane_id,
                workspace_id,
                agent_status,
                ..
            } => (
                "agent.status",
                json!({"session":session,"pane":pane_id,"workspaceId":workspace_id,"status":match agent_status.as_str(){"idle"|"working"|"blocked"|"done"=>agent_status.as_str(),_=>"unknown"}}),
            ),
            HerdrSubscriptionEvent::PaneExited {
                pane_id,
                workspace_id,
                ..
            } => (
                "pane.exited",
                json!({"session":session,"pane":pane_id,"workspaceId":workspace_id}),
            ),
            // Title/cwd updates are not topology and would only cause refetch storms.
            HerdrSubscriptionEvent::TopologyChanged { kind, .. } if kind != "pane.updated" => {
                return self.stale(session)
            }
            HerdrSubscriptionEvent::Disconnected { .. } | HerdrSubscriptionEvent::Error { .. } => {
                self.state.close("stream-closed");
                return Ok(());
            }
            _ => return Ok(()), // topology, paths, titles and labels are not forwarded
        };
        self.send(Message::text(
            json!({"event":event,"data":data,"stream":self.id}).to_string(),
        ))
    }
}
struct Stream {
    session: String,
    events: bool,
    state: Arc<StreamState>,
    lease: Option<Lease>,
    write: Option<writes::WriteStream>,
}
enum Output {
    Value(Value),
    Snapshot(Value),
    Opened(Lease, Value, Option<crate::admin::EventReservation>),
}
pub(crate) struct Completion {
    id: Value,
    stream: Option<u32>,
    result: herdr::Result<Output>,
    write: bool,
}

pub(crate) struct Reads {
    pub(crate) peer_id: u64,
    life: Arc<Life>,
    sender: mpsc::Sender<StreamFrame>,
    bytes: Arc<Semaphore>,
    streams: HashMap<u32, Stream>,
    next_id: u32,
    writes: writes::Queue,
    pending: Arc<Semaphore>,
    pub(crate) completed: FuturesUnordered<BoxFuture<'static, Completion>>,
    pub(crate) changed: Arc<Notify>,
}
impl Drop for Reads {
    fn drop(&mut self) {
        self.life.alive.store(false, Ordering::SeqCst);
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionParams {
    session: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ObserveParams {
    session: String,
    target: String,
    // Ignore even malformed/oversized client dimensions: they have no authority.
    #[serde(default, rename = "cols")]
    _cols: Value,
    #[serde(default, rename = "rows")]
    _rows: Value,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CloseParams {
    stream: u32,
}
impl Reads {
    pub(crate) fn new(
        bytes: Arc<Semaphore>,
        invalidated: watch::Receiver<bool>,
        stop: watch::Receiver<u16>,
    ) -> (Self, Frames) {
        let (sender, receiver) = mpsc::channel(MAX_STREAMS * STREAM_FRAMES);
        (
            Self {
                peer_id: 0,
                life: Arc::new(Life {
                    alive: AtomicBool::new(true),
                    invalidated,
                    stop,
                }),
                sender,
                bytes,
                streams: HashMap::new(),
                next_id: 1,
                writes: writes::Queue::new(),
                pending: Arc::new(Semaphore::new(2)),
                completed: FuturesUnordered::new(),
                changed: Arc::new(Notify::new()),
            },
            Frames {
                receiver,
                held: None,
            },
        )
    }
    pub(crate) fn call(
        &mut self,
        gateway: &Gateway,
        ctx: &AuthContext,
        id: Value,
        method: &str,
        params: Value,
    ) -> std::result::Result<Option<Value>, ()> {
        // The authorization closure only admits a synchronous bounded job.
        gateway
            .with_authorized(ctx, Scope::View, |_| {
                self.enqueue(gateway, ctx.device_id(), id, method, params)
            })
            .map_err(|_| ())
    }
    fn enqueue(
        &mut self,
        gateway: &Gateway,
        device: &str,
        id: Value,
        method: &str,
        params: Value,
    ) -> Option<Value> {
        if method == "stream.close" {
            let Ok(p) = serde_json::from_value::<CloseParams>(params) else {
                return Some(error(id, "invalid-params"));
            };
            if self
                .streams
                .get(&p.stream)
                .is_some_and(|s| s.write.is_some())
            {
                return Some(error(id, "invalid-stream-type"));
            }
            let Some(stream) = self.streams.remove(&p.stream) else {
                return Some(error(id, "stream-not-found"));
            };
            stream.state.close("client-closed");
            return Some(reply(id, json!({"stream":p.stream,"closed":true})));
        }
        let (session, target) = match method {
            "sessions.list"
                if params.is_null() || params.as_object().is_some_and(|o| o.is_empty()) =>
            {
                (String::new(), None)
            }
            "terminal.observe" => match serde_json::from_value::<ObserveParams>(params) {
                Ok(p) if herdr::valid_id(&p.target) => (p.session, Some(p.target)),
                _ => return Some(error(id, "invalid-params")),
            },
            "snapshot.get" | "events.subscribe" => {
                match serde_json::from_value::<SessionParams>(params) {
                    Ok(p) => (p.session, None),
                    Err(_) => return Some(error(id, "invalid-params")),
                }
            }
            _ => return Some(error(id, "invalid-params")),
        };
        if method == "events.subscribe" {
            if let Some((stream, _)) = self
                .streams
                .iter()
                .find(|(_, s)| s.events && s.session == session)
            {
                if self.streams[stream].lease.is_none() {
                    return Some(error(id, "busy"));
                }
                return Some(reply(id, json!({"stream":stream,"subscribed":true})));
            }
        }
        let Ok(pending) = self.pending.clone().try_acquire_owned() else {
            return Some(error(id, "busy"));
        };
        let opening = matches!(method, "events.subscribe" | "terminal.observe");
        if opening && self.streams.len() >= MAX_STREAMS {
            return Some(error(id, "stream-limit"));
        }
        let reserved = if opening {
            match gateway.herdr_jobs.reserve() {
                Ok(p) => Some(p),
                Err(e) => return Some(error(id, e)),
            }
        } else {
            None
        };
        let state = Arc::new(StreamState {
            life: self.life.clone(),
            closed: Mutex::new(None),
            ready: AtomicBool::new(false),
            ready_changed: Notify::new(),
            changed: self.changed.clone(),
            slots: Arc::new(Semaphore::new(STREAM_FRAMES)),
            bytes: Arc::new(Semaphore::new(STREAM_BYTES)),
            deadline: Mutex::new(None),
        });
        let stream_id = if opening {
            let Some(next) = self.next_id.checked_add(1) else {
                return Some(error(id, "stream-limit"));
            };
            let stream = self.next_id;
            self.next_id = next;
            self.streams.insert(
                stream,
                Stream {
                    session: session.clone(),
                    events: method == "events.subscribe",
                    state: state.clone(),
                    lease: None,
                    write: None,
                },
            );
            Some(stream)
        } else {
            None
        };
        let sink = Sink {
            id: stream_id.unwrap_or(0),
            state: state.clone(),
            sender: self.sender.clone(),
            bytes: self.bytes.clone(),
        };
        let reads = gateway.herdr.clone();
        let jobs = gateway.herdr_jobs.clone();
        let method = method.to_owned();
        let work = move || {
            let active = || {
                if state.active() {
                    Ok(())
                } else {
                    Err("stream-closed")
                }
            };
            active()?;
            if method == "sessions.list" {
                return reads.sessions().map(Output::Value);
            }
            reads.running(&session)?;
            active()?;
            if method == "snapshot.get" {
                return reads.snapshot(&session).map(Output::Snapshot);
            }
            let (resource, value) = if let Some(target) = target {
                let snapshot = reads
                    .port
                    .snapshot(&session)
                    .map_err(|e| reads.after_error(&session, e))?;
                active()?;
                let (target, cols, rows) = herdr::observe_geometry(&snapshot.snapshot, &target)?;
                // Fresh running check immediately before attach; never auto-start.
                reads.running(&session)?;
                active()?;
                let terminal = reads
                    .port
                    .observe(
                        &session,
                        &target,
                        cols,
                        rows,
                        Arc::new(move |event| sink.terminal(event)),
                    )
                    .map_err(|e| reads.after_error(&session, e))?;
                (
                    Resource::Observe(terminal),
                    json!({"stream":stream_id,"cols":cols,"rows":rows}),
                )
            } else {
                // Agent-status events need the current pane IDs; the client
                // subscribes again after `snapshot.stale` to cover new panes.
                let snapshot = reads
                    .port
                    .snapshot(&session)
                    .map_err(|e| reads.after_error(&session, e))?;
                active()?;
                let panes = herdr::pane_ids(&snapshot.snapshot);
                let name = session.clone();
                let check = sink.clone();
                let subscription = reads
                    .port
                    .subscribe(
                        &session,
                        panes.clone(),
                        Arc::new(move |event| sink.event(&name, event)),
                    )
                    .map_err(|e| reads.after_error(&session, e))?;
                // A pane created before the subscription took effect is not
                // covered, and its creation event is gone: say so once.
                if reads
                    .port
                    .snapshot(&session)
                    .is_ok_and(|now| herdr::pane_ids(&now.snapshot) != panes)
                {
                    let _ = check.stale(&session);
                }
                (
                    Resource::Events(subscription),
                    json!({"stream":stream_id,"subscribed":true}),
                )
            };
            let lease = Lease::new(
                jobs,
                reads.port.clone(),
                resource,
                reserved.expect("opening permit"),
            );
            if !state.life.active() {
                return Err("stream-closed");
            } // Orphaned attach releases by RAII.
            Ok(Output::Opened(lease, value, None))
        };
        match gateway.herdr_jobs.submit_device(device, false, work) {
            Ok(receive) => {
                self.completed.push(Box::pin(async move {
                    let _pending = pending;
                    Completion {
                        id,
                        stream: stream_id,
                        result: receive.await.unwrap_or(Err("herdr-unavailable")),
                        write: false,
                    }
                }));
                None
            }
            Err(code) => {
                if let Some(stream) = stream_id {
                    self.streams.remove(&stream);
                }
                Some(error(id, code))
            }
        }
    }
    pub(crate) fn finish(&mut self, completion: Completion) -> Vec<Value> {
        let Completion {
            id,
            stream,
            result,
            write,
        } = completion;
        if write && self.writes.finished() {
            return Vec::new(); // Late result after the observation timeout; no replay/second ACK.
        }
        if !self.life.active() {
            return Vec::new();
        }
        let failure = |code| {
            if write {
                writes::write_error(id.clone(), stream, code)
            } else {
                error(id.clone(), code)
            }
        };
        match result {
            Ok(Output::Opened(lease, value, announcement)) => {
                let Some(s) = stream.and_then(|s| self.streams.get_mut(&s)) else {
                    return vec![failure("stream-closed")];
                };
                s.lease = Some(lease);
                if let Some(reason) = *s.state.closed.lock().unwrap() {
                    return vec![failure(reason)];
                }
                if let Some(write) = &mut s.write {
                    write.audit.target = value["target"].as_str().map(str::to_owned);
                    // Host has succeeded. Capacity was reserved before that call;
                    // a newly deferred DIFFERENT stream cannot reverse this key.
                    announcement
                        .expect("write open reservation")
                        .emit(write.audit.state(true, write.takeover));
                    write.announced = true;
                }
                // Caller enqueues these replies synchronously before the writer
                // can poll again; its control queue has priority over stream data.
                s.state.ready.store(true, Ordering::SeqCst);
                s.state.ready_changed.notify_one();
                vec![reply(id, value)]
            }
            Ok(Output::Value(value)) => {
                if serde_json::to_vec(&value).expect("JSON value").len() > INLINE_BYTES {
                    vec![failure("payload-too-large")]
                } else {
                    vec![reply(id, value)]
                }
            }
            Ok(Output::Snapshot(value)) => snapshot_replies(id, value),
            Err(code) => {
                if let Some(s) = stream {
                    // Content-level write failure is not stream teardown. Closed
                    // states are drained by closed(), which sends stream.closed.
                    if self
                        .streams
                        .get(&s)
                        .is_some_and(|s| !write || s.lease.is_none())
                    {
                        if let Some(stream) = self.streams.remove(&s) {
                            stream.state.close(code);
                        }
                    }
                }
                vec![failure(code)]
            }
        }
    }
    pub(crate) fn closed(&mut self) -> Vec<Value> {
        let closed: Vec<_> = self
            .streams
            .iter()
            .filter(|(_, s)| s.lease.is_some())
            .filter_map(|(id, s)| s.state.closed.lock().unwrap().map(|reason| (*id, reason)))
            .collect();
        closed
            .into_iter()
            .map(|(id, reason)| {
                self.streams.remove(&id);
                self.discard_write_stream(id);
                json!({"event":"stream.closed","data":{"stream":id,"reason":reason}})
            })
            .collect()
    }
}
fn snapshot_replies(id: Value, value: Value) -> Vec<Value> {
    let bytes = serde_json::to_vec(&value).expect("JSON value");
    if bytes.len() > herdr::SNAPSHOT_BYTES {
        return vec![error(id, "payload-too-large")];
    }
    if bytes.len() <= INLINE_BYTES {
        return vec![reply(id, value)];
    }
    let mut replies = vec![reply(
        id.clone(),
        json!({"chunked":true,"encoding":"base64-json-v1","bytes":bytes.len(),"chunks":bytes.len().div_ceil(CHUNK_BYTES)}),
    )];
    for (index, chunk) in bytes.chunks(CHUNK_BYTES).enumerate() {
        replies.push(json!({"event":"snapshot.chunk","data":{"id":id,"index":index,"data":B64.encode(chunk)}}));
    }
    replies
}

#[cfg(test)]
#[path = "read_bridge_tests.rs"]
mod tests;
#[cfg(test)]
pub(crate) use tests::fill_streams_after_snapshot;
