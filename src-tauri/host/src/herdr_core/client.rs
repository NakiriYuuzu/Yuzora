//! One [`Session`] per herdr-core session, over two connections to the same core.
//!
//! * The **plain** connection only runs RPC (NDJSON, answers matched by id, timeouts). It never
//!   subscribes to events: a plain connection that does stops answering RPC (P0 S1/S2).
//! * The **framed** connection (`hello{framed:true}`) carries channel-0 RPC for the calls that
//!   need framing (`terminal.attach`, `events.subscribe`), one events channel whose data is
//!   credited as soon as it is read, and one channel per attach.
//!
//! Each connection has one reader thread that only decodes and routes; it never calls foreign
//! code and never waits on a consumer:
//!
//! * attach output is appended to that attach's buffer; a pump thread per attach hands it to
//!   the caller's [`AttachSink`] (merged for about 4 ms or 64 KiB) and sends the batched
//!   CREDIT, so one stuck sink delays only its own attach;
//! * events feed the [`EventStore`] and the resulting [`Effect`]s reach the
//!   [`SessionObserver`] from a worker thread, which also re-reads the layout document after
//!   subscribing, after `missed` and on every `doc` event for it.
//!
//! A session never reconnects. Any failure on either connection ends it for good (calls fail
//! with `connection_lost`, streams end, the observer hears once) and the owner connects a new
//! one: reconnecting always means subscribing, attaching and reading documents again (K5).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, Weak};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};

use super::caps::{self, Hello, Refusal, Verdict};
use super::docs::{self, DocError, DocIo, IoFailure, LayoutState, Op, Updated};
use super::events::{Effect, EventStore};
use super::frame::{self, CloseReason, CreditLedger, Decoder, Frame};
use super::paths::Endpoint;
use super::rpc::{self, Incoming, LineSplitter, RpcError};
use crate::herdr_limits::{BoundedNdjsonReadError, HerdrProtocolError};
use crate::herdr_transport::{
    connect_local_stream, poll_local_stream_read, read_local_ndjson_line_with,
    shutdown_local_stream, wait_local_readable, write_local_all_until_shared, LocalStream,
    LocalStreamRead, LocalWaitProfile,
};

/// Default wait for an RPC answer.
pub const RPC_TIMEOUT: Duration = Duration::from_secs(15);
/// Default wait for connecting and for each `hello`.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Attach output is merged for this long (leading edge: output after an idle gap goes out at
/// once, so a keystroke echo is not delayed)...
pub const COALESCE_WINDOW: Duration = Duration::from_millis(4);
/// ... or until this many bytes are waiting.
pub const COALESCE_MAX: usize = 64 * 1024;
/// CREDIT is sent once this many consumed bytes are owed ...
pub const CREDIT_BATCH: u64 = 32 * 1024;
/// ... or this long after the first byte owed.
pub const CREDIT_DELAY: Duration = Duration::from_millis(50);

/// RPC messages are far below this; a peer past it is not speaking the protocol.
const MAX_RPC_LINE_BYTES: usize = 2 * 1024 * 1024;
const READ_BUFFER: usize = 64 * 1024;
/// Readers wake this often to notice that the session ended.
const POLL_SLICE: Duration = Duration::from_millis(100);
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);
/// One DATA frame of typed or pasted input.
const INPUT_CHUNK: usize = 64 * 1024;
/// Attaches whose caller gave up and whose answer is still awaited, kept per session so a late
/// viewer can be closed. A core that never answers would otherwise add one per timeout until
/// the session ends; past this the oldest is forgotten.
const MAX_LATE_ATTACHES: usize = 32;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

// ---------------------------------------------------------------------------------- errors

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientError {
    Io(String),
    Timeout,
    /// The connection ended while the call was waiting, or before it was made.
    ConnectionLost,
    /// The session was closed on purpose.
    Closed,
    /// The core answered with an error.
    Rpc(RpcError),
    /// The peer broke the protocol.
    Protocol(String),
    /// The local view of the core can no longer be trusted (an event gap): connect again.
    Resync(String),
    /// A method this client must not send on this path, or the core did not declare.
    Unsupported(String),
    /// The core answered but Yuzora will not use it.
    Incompatible(Refusal),
}

impl ClientError {
    /// Stable code for the UI layer.
    pub fn code(&self) -> &'static str {
        match self {
            ClientError::Io(_) => "io",
            ClientError::Timeout => "timeout",
            ClientError::ConnectionLost => "connection_lost",
            ClientError::Closed => "closed",
            ClientError::Rpc(_) => "rpc",
            ClientError::Protocol(_) => "protocol",
            ClientError::Resync(_) => "resync",
            ClientError::Unsupported(_) => "unsupported",
            ClientError::Incompatible(_) => "incompatible",
        }
    }
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientError::Io(m) => write!(f, "io: {m}"),
            ClientError::Timeout => write!(f, "timed out"),
            ClientError::ConnectionLost => write!(f, "the core connection was lost"),
            ClientError::Closed => write!(f, "the session was closed"),
            ClientError::Rpc(e) => e.fmt(f),
            ClientError::Protocol(m) => write!(f, "protocol: {m}"),
            ClientError::Resync(m) => write!(f, "out of sync: {m}"),
            ClientError::Unsupported(m) => write!(f, "unsupported: {m}"),
            ClientError::Incompatible(r) => r.fmt(f),
        }
    }
}

impl std::error::Error for ClientError {}

impl From<io::Error> for ClientError {
    fn from(e: io::Error) -> ClientError {
        match e.kind() {
            io::ErrorKind::TimedOut => ClientError::Timeout,
            io::ErrorKind::BrokenPipe
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::NotConnected
            | io::ErrorKind::UnexpectedEof => ClientError::ConnectionLost,
            _ => ClientError::Io(e.to_string()),
        }
    }
}

fn read_error(e: BoundedNdjsonReadError) -> ClientError {
    match e {
        BoundedNdjsonReadError::Io(e) => e.into(),
        BoundedNdjsonReadError::Protocol(HerdrProtocolError::TimedOut) => ClientError::Timeout,
        BoundedNdjsonReadError::Protocol(p) => ClientError::Protocol(p.message().to_string()),
    }
}

// ---------------------------------------------------------------------------------- public types

#[derive(Debug, Clone)]
pub struct SessionConfig {
    pub rpc_timeout: Duration,
    pub connect_timeout: Duration,
    /// The bundled core's version, for [`caps::gate`].
    pub bundled_version: String,
}

impl Default for SessionConfig {
    fn default() -> SessionConfig {
        SessionConfig {
            rpc_timeout: RPC_TIMEOUT,
            connect_timeout: CONNECT_TIMEOUT,
            bundled_version: caps::BUNDLED_VERSION.to_string(),
        }
    }
}

/// The layout document as the session last read it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LayoutSnapshot {
    pub version: u64,
    pub state: LayoutState,
}

/// Hears about the session. Called one at a time from the session's worker thread, never from
/// a reader, so it may call back into the [`Session`]. Keep it quick: nothing else is
/// delivered meanwhile.
pub trait SessionObserver: Send + Sync + 'static {
    /// Facts changed; the session's event store is already updated.
    fn on_effects(&self, _effects: &[Effect]) {}
    /// The layout document was read after subscribing, after `missed`, or on a `doc` event for
    /// it, and its version differs from the last one delivered.
    fn on_layout(&self, _layout: &LayoutSnapshot) {}
    /// The session ended; once, last. [`ClientError::Closed`] when closed on purpose.
    fn on_closed(&self, _cause: &ClientError) {}
}

/// Where an attach stream's output goes. Both methods must return quickly (hand off to a
/// channel); a sink that blocks stalls only its own stream.
pub trait AttachSink: Send + Sync + 'static {
    /// Terminal output, merged; the bytes of one stream arrive in order.
    fn output(&self, bytes: Vec<u8>);
    /// The stream ended, after all of its output. Called once.
    fn closed(&self, end: AttachEnd);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachEnd {
    /// The core ended the stream: our detach, the program ending, loss, or a core stop.
    Core {
        reason: CloseReason,
        /// The terminal is kept and comes back with a new shell: wait for
        /// [`Effect::Reattach`] and attach again. Worked out from the terminal's `keep` when the
        /// stream ended ([`CloseReason::reattach_after_restart`]); `false` for our own detach.
        reattach_after_restart: bool,
    },
    /// The session ended under it (the `ClientError::code()` of the cause).
    Session(&'static str),
}

/// What a viewer's terminal looks like; the core answers OSC 10/11 and cell-size queries
/// itself from this.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct DisplayFacts {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bg: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fg: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// Cell size in pixels: `[width, height]`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cell: Option<[u32; 2]>,
}

#[derive(Debug, Clone)]
pub struct AttachRequest {
    /// `t7`, a name, or `machine/t7`.
    pub terminal: String,
    pub cols: u16,
    pub rows: u16,
    /// Scrollback lines to show first (main screen only).
    pub history: u32,
    pub display: DisplayFacts,
}

// ---------------------------------------------------------------------------------- RPC table

type Hook = Box<dyn FnOnce(&Result<Value, RpcError>) + Send>;

struct Waiter {
    tx: SyncSender<Result<Value, ClientError>>,
    /// Runs on the reader thread when the answer arrives, before the caller wakes: state the
    /// first frames after the answer depend on (an attach channel) is in place by then.
    hook: Option<Hook>,
}

#[derive(Default)]
struct TableInner {
    waiters: HashMap<u64, Waiter>,
    /// The ids in `waiters` whose caller gave up (see [`RpcTable::retire`]); ids only grow, so
    /// the first is the oldest.
    retired: BTreeSet<u64>,
    dead: Option<ClientError>,
}

/// Calls waiting for their answer on one connection.
struct RpcTable {
    inner: Mutex<TableInner>,
    next_id: AtomicU64,
}

type Pending = (u64, Receiver<Result<Value, ClientError>>);

impl RpcTable {
    fn new() -> RpcTable {
        RpcTable {
            inner: Mutex::new(TableInner::default()),
            // 1 is the handshake's.
            next_id: AtomicU64::new(2),
        }
    }

    fn register(&self, hook: Option<Hook>) -> Result<Pending, ClientError> {
        let mut inner = lock(&self.inner);
        if let Some(dead) = &inner.dead {
            return Err(dead.clone());
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::sync_channel(1);
        inner.waiters.insert(id, Waiter { tx, hook });
        Ok((id, rx))
    }

    fn forget(&self, id: u64) {
        let mut inner = lock(&self.inner);
        inner.retired.remove(&id);
        inner.waiters.remove(&id);
    }

    /// The caller of `id` stopped waiting, but the answer, if it ever comes, still has work to
    /// do (an attach's opened a viewer nobody owns): `late` replaces the hook that was
    /// registered with the call. The original holds the session and whatever the caller handed
    /// in, and a core that never answers would keep both until the session ends; `late` must
    /// hold neither. At most [`MAX_LATE_ATTACHES`] are kept: past that the oldest is forgotten,
    /// and a viewer it would have closed stays open until the session ends and the core
    /// releases it with the connection.
    fn retire(&self, id: u64, late: Hook) {
        let (replaced, forgotten) = {
            let mut inner = lock(&self.inner);
            let Some(waiter) = inner.waiters.get_mut(&id) else {
                // Answered, or the connection ended, in the meantime.
                return;
            };
            let replaced = waiter.hook.replace(late);
            inner.retired.insert(id);
            let mut forgotten = Vec::new();
            while inner.retired.len() > MAX_LATE_ATTACHES {
                let Some(oldest) = inner.retired.pop_first() else {
                    break;
                };
                forgotten.extend(inner.waiters.remove(&oldest));
            }
            (replaced, forgotten)
        };
        // Outside the lock: what the old hook held (the caller's sink, the session) is let go
        // of without the table locked.
        drop((replaced, forgotten));
    }

    /// An answer arrived. Unknown ids (the caller gave up) are dropped.
    fn complete(&self, id: u64, result: Result<Value, RpcError>) {
        let waiter = {
            let mut inner = lock(&self.inner);
            inner.retired.remove(&id);
            inner.waiters.remove(&id)
        };
        if let Some(waiter) = waiter {
            if let Some(hook) = waiter.hook {
                hook(&result);
            }
            let _ = waiter.tx.send(result.map_err(ClientError::Rpc));
        }
    }

    /// The connection is over: every waiting call gets `error`, later calls fail at once.
    fn fail_all(&self, error: ClientError) {
        let waiters = {
            let mut inner = lock(&self.inner);
            inner.dead.get_or_insert_with(|| error.clone());
            inner.retired.clear();
            std::mem::take(&mut inner.waiters)
        };
        for (_, waiter) in waiters {
            let _ = waiter.tx.send(Err(error.clone()));
        }
    }
}

/// What the answer of an attach does once its caller has given up (see [`RpcTable::retire`]):
/// the viewer it opened is closed again. The session is held weakly: if it is gone the
/// connection is too, and the core has released the viewer already.
fn close_late_viewer(core: Weak<Core>) -> Hook {
    Box::new(move |result| {
        let Ok(reply) = result else { return };
        let Some(channel) = reply
            .get("channel")
            .and_then(Value::as_u64)
            .and_then(|c| u32::try_from(c).ok())
        else {
            return;
        };
        if let Some(core) = core.upgrade() {
            let _ = core.write_frame(&Frame::close(channel));
        }
    })
}

fn wait_reply(
    rx: &Receiver<Result<Value, ClientError>>,
    timeout: Duration,
    forget: impl FnOnce(),
) -> Result<Value, ClientError> {
    match rx.recv_timeout(timeout) {
        Ok(result) => result,
        Err(RecvTimeoutError::Timeout) => {
            forget();
            Err(ClientError::Timeout)
        }
        Err(RecvTimeoutError::Disconnected) => Err(ClientError::ConnectionLost),
    }
}

// ---------------------------------------------------------------------------------- handshake

/// One blocking request/answer on a connection no reader thread owns yet (the handshake, the
/// probe). Answers to other ids are skipped.
pub(crate) fn blocking_call(
    stream: &LocalStream,
    pending: &mut Vec<u8>,
    method: &str,
    params: Value,
    deadline: Instant,
) -> Result<Value, ClientError> {
    const ID: u64 = 1;
    let line = rpc::line(&rpc::request(ID, method, params));
    write_local_all_until_shared(stream, &line, deadline)?;
    loop {
        let line = read_local_ndjson_line_with(
            stream,
            pending,
            Some(deadline),
            MAX_RPC_LINE_BYTES,
            LocalWaitProfile::Interactive,
        )
        .map_err(read_error)?
        .ok_or(ClientError::ConnectionLost)?;
        match rpc::parse(line.trim().as_bytes()).map_err(ClientError::Protocol)? {
            Incoming::Response { id: ID, result } => return result.map_err(ClientError::Rpc),
            Incoming::Response { .. } | Incoming::Notification { .. } => continue,
        }
    }
}

/// `hello`, optionally switching the connection to framed mode. No compatibility decision.
pub(crate) fn hello_call(
    stream: &LocalStream,
    pending: &mut Vec<u8>,
    framed: bool,
    deadline: Instant,
) -> Result<Hello, ClientError> {
    let mut params = json!({"generation": caps::GENERATION});
    if framed {
        params["framed"] = json!(true);
    }
    let result = blocking_call(stream, pending, "hello", params, deadline)?;
    Hello::parse(&result).map_err(ClientError::Protocol)
}

// ---------------------------------------------------------------------------------- session core

enum Notice {
    Effects(Vec<Effect>),
    ReloadDocs,
    Closed(ClientError),
}

struct PlainConn {
    stream: LocalStream,
    write: Mutex<()>,
    table: RpcTable,
}

#[derive(Default)]
struct EventsState {
    channel: Option<u32>,
    store: Option<EventStore>,
    lines: Option<LineSplitter>,
}

struct FramedConn {
    stream: LocalStream,
    write: Mutex<()>,
    table: RpcTable,
    attaches: Mutex<HashMap<u32, Arc<AttachInner>>>,
    events: Mutex<EventsState>,
}

/// Everything the session's threads share.
struct Core {
    hello: Hello,
    config: SessionConfig,
    plain: PlainConn,
    framed: FramedConn,
    /// The first cause that ended the session.
    closed: Mutex<Option<ClientError>>,
    stop: AtomicBool,
    notices: Sender<Notice>,
    /// Test seam: how long the attach hook (reader thread) waits between taking the answer and
    /// registering the stream, to widen the window a `close` can land in.
    #[cfg(test)]
    attach_hook_pause: Mutex<Duration>,
}

impl Core {
    fn is_closed(&self) -> bool {
        self.stop.load(Ordering::Acquire)
    }

    /// Ends the session: once, whichever thread notices first.
    fn fail(&self, cause: ClientError) {
        {
            let mut closed = lock(&self.closed);
            if closed.is_some() {
                return;
            }
            *closed = Some(cause.clone());
        }
        self.stop.store(true, Ordering::Release);
        // Waiting calls get a stable code; the detail goes to the observer.
        let calls = match cause {
            ClientError::Closed => ClientError::Closed,
            _ => ClientError::ConnectionLost,
        };
        self.plain.table.fail_all(calls.clone());
        self.framed.table.fail_all(calls);
        // `stop` is set above, before the table is taken. An attach that registers after this
        // take (its answer was already in flight) sees `stop` and ends itself.
        let attaches = std::mem::take(&mut *lock(&self.framed.attaches));
        for attach in attaches.values() {
            attach.finish(AttachEnd::Session(cause.code()));
        }
        let _ = self.notices.send(Notice::Closed(cause));
        // Unix: the core hears the end now (viewers released, subscriptions gone) and the
        // readers wake at once, instead of both waiting for the last thread to let go of the
        // sockets. Windows: a named pipe client has no half-close, so this does nothing and
        // the pipes end when the last holder of this `Core` drops it (see [`Session::close`]).
        shutdown_local_stream(&self.plain.stream);
        shutdown_local_stream(&self.framed.stream);
    }

    /// The code of the cause that ended the session (`closed` before it has ended).
    fn closed_code(&self) -> &'static str {
        lock(&self.closed)
            .as_ref()
            .map_or("closed", ClientError::code)
    }

    /// Whether the terminal comes back after a loss: its `keep` flag, from the event store when
    /// it knows the terminal (`keep` events update it), else from the record the attach returned.
    fn is_kept(&self, attached: &Value) -> bool {
        let address = attached.get("address").and_then(Value::as_str);
        let events = lock(&self.framed.events);
        let current = address.and_then(|a| events.store.as_ref()?.terminals.get(a));
        current
            .unwrap_or(attached)
            .get("keep")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }

    fn write_plain(&self, bytes: &[u8]) -> Result<(), ClientError> {
        let _guard = lock(&self.plain.write);
        write_local_all_until_shared(&self.plain.stream, bytes, Instant::now() + WRITE_TIMEOUT)
            .map_err(ClientError::from)
    }

    fn write_frame(&self, frame: &Frame) -> Result<(), ClientError> {
        let result = {
            let _guard = lock(&self.framed.write);
            write_local_all_until_shared(
                &self.framed.stream,
                &frame.encode(),
                Instant::now() + WRITE_TIMEOUT,
            )
        };
        result.map_err(|e| {
            let error = ClientError::from(e);
            self.fail(error.clone());
            error
        })
    }

    fn plain_rpc(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, ClientError> {
        let (id, rx) = self.plain.table.register(None)?;
        let line = rpc::line(&rpc::request(id, method, params));
        if let Err(e) = self.write_plain(&line) {
            self.plain.table.forget(id);
            self.fail(e.clone());
            return Err(e);
        }
        wait_reply(&rx, timeout, || self.plain.table.forget(id))
    }

    /// Sends a request on the framed connection's channel 0 and registers its answer.
    fn framed_send(
        &self,
        method: &str,
        params: Value,
        hook: Option<Hook>,
    ) -> Result<Pending, ClientError> {
        let (id, rx) = self.framed.table.register(hook)?;
        let message = rpc::request(id, method, params).to_string().into_bytes();
        if let Err(e) = self.write_frame(&Frame::data(0, message)) {
            self.framed.table.forget(id);
            return Err(e);
        }
        Ok((id, rx))
    }

    /// RPC on the framed connection's channel 0. After a timeout a late answer is dropped.
    fn framed_rpc(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
        hook: Option<Hook>,
    ) -> Result<Value, ClientError> {
        let (id, rx) = self.framed_send(method, params, hook)?;
        wait_reply(&rx, timeout, || self.framed.table.forget(id))
    }

    fn read_layout(&self) -> Result<LayoutSnapshot, ClientError> {
        let result = self.plain_rpc(
            "doc.get",
            json!({"name": docs::DOC_NAME}),
            self.config.rpc_timeout,
        )?;
        Ok(LayoutSnapshot {
            version: result.get("version").and_then(Value::as_u64).unwrap_or(0),
            state: docs::decode(result.get("value").unwrap_or(&Value::Null)),
        })
    }
}

// ---------------------------------------------------------------------------------- readers

fn spawn_named(name: &str, f: impl FnOnce() + Send + 'static) -> Result<(), ClientError> {
    thread::Builder::new()
        .name(name.to_string())
        .spawn(f)
        .map(|_| ())
        .map_err(ClientError::from)
}

/// Plain connection: answers to calls.
fn plain_reader(core: Arc<Core>, mut pending: Vec<u8>) {
    let cause = loop {
        if core.is_closed() {
            return;
        }
        let line = match read_local_ndjson_line_with(
            &core.plain.stream,
            &mut pending,
            Some(Instant::now() + POLL_SLICE),
            MAX_RPC_LINE_BYTES,
            LocalWaitProfile::Interactive,
        ) {
            Ok(Some(line)) => line,
            Ok(None) => break ClientError::ConnectionLost,
            Err(BoundedNdjsonReadError::Protocol(HerdrProtocolError::TimedOut)) => continue,
            Err(e) => break read_error(e),
        };
        match rpc::parse(line.trim().as_bytes()) {
            Ok(Incoming::Response { id, result }) => core.plain.table.complete(id, result),
            // This connection never subscribes, so there is nothing else to hear.
            Ok(Incoming::Notification { .. }) => {}
            Err(e) => break ClientError::Protocol(e),
        }
    };
    core.fail(cause);
}

/// Work one read's frames produced, delivered after the whole read was routed.
#[derive(Default)]
struct Batch {
    events_credit: u64,
    effects: Vec<Effect>,
}

/// Framed connection: routes frames by channel.
fn framed_reader(core: Arc<Core>, leftover: Vec<u8>) {
    let mut decoder = Decoder::default();
    let mut buf = vec![0u8; READ_BUFFER];
    let mut attempt = 0u32;
    let cause = 'read: {
        // Bytes read past the `hello` answer are the start of the first frames.
        if !leftover.is_empty() {
            if let Err(e) = ingest(&core, &mut decoder, &leftover) {
                break 'read e;
            }
        }
        loop {
            if core.is_closed() {
                return;
            }
            match poll_local_stream_read(&core.framed.stream, &mut buf) {
                Ok(LocalStreamRead::Data(n)) => {
                    attempt = 0;
                    if let Err(e) = ingest(&core, &mut decoder, &buf[..n]) {
                        break 'read e;
                    }
                }
                Ok(LocalStreamRead::Pending) => {
                    let wait = wait_local_readable(
                        &core.framed.stream,
                        Some(Instant::now() + POLL_SLICE),
                        LocalWaitProfile::Interactive,
                        attempt,
                    );
                    if let Err(e) = wait {
                        break 'read ClientError::from(e);
                    }
                    attempt = attempt.saturating_add(1);
                }
                Ok(LocalStreamRead::Closed) => break 'read ClientError::ConnectionLost,
                Err(e) => break 'read ClientError::from(e),
            }
        }
    };
    core.fail(cause);
}

/// Decodes and routes everything one read produced, then credits and reports it together.
fn ingest(core: &Arc<Core>, decoder: &mut Decoder, bytes: &[u8]) -> Result<(), ClientError> {
    let frames = decoder
        .push(bytes)
        .map_err(|e| ClientError::Protocol(e.to_string()))?;
    let mut batch = Batch::default();
    for frame in frames {
        route(core, frame, &mut batch)?;
    }
    finish_batch(core, batch)
}

fn route(core: &Arc<Core>, frame: Frame, batch: &mut Batch) -> Result<(), ClientError> {
    let channel = frame.channel;
    if channel == 0 {
        if frame.kind == frame::DATA {
            match rpc::parse(&frame.payload).map_err(ClientError::Protocol)? {
                Incoming::Response { id, result } => core.framed.table.complete(id, result),
                Incoming::Notification { .. } => {}
            }
        }
        return Ok(());
    }
    let attach = lock(&core.framed.attaches).get(&channel).cloned();
    if let Some(attach) = attach {
        match frame.kind {
            frame::DATA => attach.on_data(frame.payload),
            frame::CLOSE => {
                lock(&core.framed.attaches).remove(&channel);
                let reason = CloseReason::parse(&frame.payload);
                let reattach_after_restart =
                    reason.reattach_after_restart(core.is_kept(&attach.terminal));
                attach.finish(AttachEnd::Core {
                    reason,
                    reattach_after_restart,
                });
            }
            _ => {}
        }
        return Ok(());
    }
    let mut events = lock(&core.framed.events);
    if events.channel != Some(channel) {
        // Late data of a stream already detached.
        return Ok(());
    }
    match frame.kind {
        frame::DATA => {
            batch.events_credit += frame.payload.len() as u64;
            let EventsState { store, lines, .. } = &mut *events;
            let lines = lines
                .get_or_insert_with(|| LineSplitter::new(MAX_RPC_LINE_BYTES))
                .push(&frame.payload)
                .map_err(|e| ClientError::Protocol(format!("events line of {} bytes", e.0)))?;
            for line in lines {
                match rpc::parse(&line).map_err(ClientError::Protocol)? {
                    Incoming::Notification { method, params } => {
                        if let Some(store) = store.as_mut() {
                            batch.effects.extend(store.apply(&method, &params));
                        }
                    }
                    Incoming::Response { .. } => {}
                }
            }
            Ok(())
        }
        frame::CLOSE => Err(ClientError::ConnectionLost),
        _ => Ok(()),
    }
}

/// Credits what the events channel delivered and hands its effects to the worker.
fn finish_batch(core: &Arc<Core>, batch: Batch) -> Result<(), ClientError> {
    if batch.events_credit > 0 {
        let channel = lock(&core.framed.events).channel;
        if let Some(channel) = channel {
            let mut owed = batch.events_credit;
            while owed > 0 {
                let part = owed.min(u64::from(u32::MAX)) as u32;
                core.write_frame(&Frame::credit(channel, part))?;
                owed -= u64::from(part);
            }
        }
    }
    if batch.effects.is_empty() {
        return Ok(());
    }
    let resync = batch.effects.iter().find_map(|e| match e {
        Effect::NeedResnapshot(why) => Some(why.clone()),
        _ => None,
    });
    let reload = batch.effects.iter().any(|e| match e {
        Effect::Resnapshot => true,
        Effect::DocChanged { name, .. } => name == docs::DOC_NAME,
        _ => false,
    });
    let _ = core.notices.send(Notice::Effects(batch.effects));
    if reload {
        let _ = core.notices.send(Notice::ReloadDocs);
    }
    match resync {
        Some(why) => Err(ClientError::Resync(why)),
        None => Ok(()),
    }
}

/// Delivers notices to the observer, one at a time.
fn worker(core: Arc<Core>, observer: Arc<dyn SessionObserver>, notices: Receiver<Notice>) {
    let mut delivered: Option<u64> = None;
    while let Ok(first) = notices.recv() {
        let mut reload = false;
        let mut closed = None;
        let mut next = Some(first);
        while let Some(notice) = next.take() {
            match notice {
                Notice::Effects(effects) => observer.on_effects(&effects),
                Notice::ReloadDocs => reload = true,
                Notice::Closed(cause) => {
                    closed = Some(cause);
                    break;
                }
            }
            next = notices.try_recv().ok();
        }
        if closed.is_none() && reload {
            if let Ok(layout) = core.read_layout() {
                if delivered != Some(layout.version) {
                    delivered = Some(layout.version);
                    observer.on_layout(&layout);
                }
            }
        }
        if let Some(cause) = closed {
            observer.on_closed(&cause);
            return;
        }
    }
}

// ---------------------------------------------------------------------------------- attach

/// The next delivery for the sink: everything waiting, but never more than `COALESCE_MAX`
/// (a pump that was scheduled late finds more than that; the rest follows at once).
pub(super) fn take_delivery(buf: &mut Vec<u8>) -> Vec<u8> {
    if buf.len() <= COALESCE_MAX {
        std::mem::take(buf)
    } else {
        buf.drain(..COALESCE_MAX).collect()
    }
}

struct AttachState {
    buf: Vec<u8>,
    last_flush: Option<Instant>,
    /// Bytes handed to the sink so far: the most the caller can have consumed.
    delivered: u64,
    ledger: CreditLedger,
    credit_due: Option<Instant>,
    /// Set once: by the core's CLOSE or by the session ending.
    end: Option<AttachEnd>,
    detach_sent: bool,
}

struct AttachInner {
    channel: u32,
    terminal: Value,
    core: Arc<Core>,
    sink: Arc<dyn AttachSink>,
    state: Mutex<AttachState>,
    wake: Condvar,
}

impl AttachInner {
    fn on_data(&self, payload: Vec<u8>) {
        let mut state = lock(&self.state);
        if state.end.is_some() {
            return;
        }
        let was_empty = state.buf.is_empty();
        if was_empty {
            state.buf = payload;
        } else {
            state.buf.extend_from_slice(&payload);
        }
        if was_empty || state.buf.len() >= COALESCE_MAX {
            self.wake.notify_one();
        }
    }

    fn finish(&self, end: AttachEnd) {
        let mut state = lock(&self.state);
        if state.end.is_none() {
            state.end = Some(end);
        }
        self.wake.notify_one();
    }

    /// Hands the buffer to the sink, sends owed credit and reports the end, on its own thread.
    fn pump(self: Arc<Self>) {
        loop {
            let mut state = lock(&self.state);
            let (output, credit) = loop {
                let now = Instant::now();
                let ending = state.end.is_some();
                let window_end = state.last_flush.map(|t| t + COALESCE_WINDOW);
                let flush = !state.buf.is_empty()
                    && (ending
                        || state.buf.len() >= COALESCE_MAX
                        || window_end.is_none_or(|t| now >= t));
                let credit_due = state.credit_due.is_some_and(|t| now >= t);
                if flush || credit_due || (ending && state.buf.is_empty()) {
                    let output = flush.then(|| take_delivery(&mut state.buf));
                    if let Some(bytes) = &output {
                        state.last_flush = Some(now);
                        state.delivered += bytes.len() as u64;
                    }
                    let credit = if credit_due && !ending {
                        state.credit_due = None;
                        state.ledger.flush()
                    } else {
                        None
                    };
                    break (output, credit);
                }
                let wake = [
                    (!state.buf.is_empty()).then_some(window_end).flatten(),
                    state.credit_due,
                ]
                .into_iter()
                .flatten()
                .min();
                state = match wake {
                    Some(at) => {
                        self.wake
                            .wait_timeout(state, at.saturating_duration_since(now))
                            .unwrap_or_else(PoisonError::into_inner)
                            .0
                    }
                    None => self
                        .wake
                        .wait(state)
                        .unwrap_or_else(PoisonError::into_inner),
                };
            };
            let end = if state.buf.is_empty() {
                state.end.clone()
            } else {
                None
            };
            drop(state);
            if let Some(bytes) = output {
                self.sink.output(bytes);
            }
            if let Some(bytes) = credit {
                // A failed write has already ended the session.
                let _ = self.core.write_frame(&Frame::credit(self.channel, bytes));
            }
            if let Some(end) = end {
                self.sink.closed(end);
                return;
            }
        }
    }
}

/// Where the attach hook (reader thread) and the caller meet.
enum Slot {
    /// The answer has not been handled yet.
    Waiting,
    /// The hook registered the stream.
    Ready(Arc<AttachInner>),
    /// The hook could not start the stream.
    Failed(ClientError),
    /// The caller stopped waiting. An answer that still comes opened a viewer nobody owns:
    /// the hook releases it, or, once the caller has given up (see [`RpcTable::retire`]),
    /// [`close_late_viewer`] in its place.
    Abandoned,
}

type AttachSlot = Arc<Mutex<Slot>>;

/// A viewer on one terminal. Dropping it detaches.
pub struct Attach {
    inner: Arc<AttachInner>,
}

impl Attach {
    /// The `Terminal` record the core returned when attaching.
    pub fn terminal(&self) -> &Value {
        &self.inner.terminal
    }

    pub fn channel(&self) -> u32 {
        self.inner.channel
    }

    /// `bytes` of output were consumed (written into the renderer, dropped paths included).
    /// Credit is batched: sent at [`CREDIT_BATCH`] owed bytes or [`CREDIT_DELAY`] after the
    /// first. Not crediting stalls the stream at the 256 KiB window.
    ///
    /// A report is limited to what the sink was handed and has not been reported yet: bytes
    /// reported twice, or more than were delivered, would widen the core's window past the
    /// 256 KiB it granted.
    pub fn credit(&self, bytes: usize) {
        let mut state = lock(&self.inner.state);
        if state.end.is_some() {
            return;
        }
        let unreported = state.delivered.saturating_sub(state.ledger.received());
        let bytes = (bytes as u64).min(unreported) as usize;
        if bytes == 0 {
            return;
        }
        match state.ledger.consumed(bytes) {
            Some(owed) => {
                state.credit_due = None;
                drop(state);
                let _ = self
                    .inner
                    .core
                    .write_frame(&Frame::credit(self.inner.channel, owed));
            }
            None => {
                if state.credit_due.is_none() {
                    state.credit_due = Some(Instant::now() + CREDIT_DELAY);
                    self.inner.wake.notify_one();
                }
            }
        }
    }

    fn live(&self) -> Result<(), ClientError> {
        let state = lock(&self.inner.state);
        if state.end.is_some() || state.detach_sent {
            Err(ClientError::Closed)
        } else {
            Ok(())
        }
    }

    /// Typed or pasted bytes. The core never encodes keys: a viewer sends terminal bytes.
    pub fn input(&self, bytes: &[u8]) -> Result<(), ClientError> {
        self.live()?;
        for chunk in bytes.chunks(INPUT_CHUNK) {
            self.inner
                .core
                .write_frame(&Frame::data(self.inner.channel, chunk))?;
        }
        Ok(())
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<(), ClientError> {
        self.live()?;
        self.inner
            .core
            .write_frame(&Frame::resize(self.inner.channel, cols, rows))
    }

    pub fn focus(&self, focused: bool) -> Result<(), ClientError> {
        self.live()?;
        self.inner
            .core
            .write_frame(&Frame::focus(self.inner.channel, focused))
    }

    pub fn facts(&self, facts: &DisplayFacts) -> Result<(), ClientError> {
        self.live()?;
        let value =
            serde_json::to_value(facts).map_err(|e| ClientError::Protocol(e.to_string()))?;
        self.inner
            .core
            .write_frame(&Frame::facts(self.inner.channel, &value))
    }

    /// Releases the viewer: the core answers with an empty CLOSE, which ends the stream
    /// (`AttachEnd::Core(Detached)`). The terminal keeps running. Safe to call again.
    pub fn detach(&self) {
        let send = {
            let mut state = lock(&self.inner.state);
            let send = state.end.is_none() && !state.detach_sent;
            state.detach_sent = true;
            send
        };
        if send {
            let _ = self
                .inner
                .core
                .write_frame(&Frame::close(self.inner.channel));
        }
    }
}

impl Drop for Attach {
    fn drop(&mut self) {
        self.detach();
    }
}

// ---------------------------------------------------------------------------------- session

/// A live connection to one core session. Dropping it closes it.
pub struct Session {
    core: Arc<Core>,
}

impl Session {
    /// Connects both connections, refuses an incompatible core, subscribes to events and
    /// starts the first read of the layout document. Never starts or touches a core process.
    pub fn connect(
        endpoint: &Endpoint,
        config: SessionConfig,
        observer: Arc<dyn SessionObserver>,
    ) -> Result<Session, ClientError> {
        let name = endpoint.transport_name();
        let deadline = Instant::now() + config.connect_timeout;
        let gate = |hello: &Hello| match caps::gate(hello, &config.bundled_version) {
            Verdict::Refuse(reason) => Err(ClientError::Incompatible(reason)),
            Verdict::Use { .. } => Ok(()),
        };

        let plain_stream = connect_local_stream(&name, deadline)?;
        let mut plain_pending = Vec::new();
        let hello = hello_call(&plain_stream, &mut plain_pending, false, deadline)?;
        gate(&hello)?;

        let framed_stream = connect_local_stream(&name, deadline)?;
        let mut framed_pending = Vec::new();
        let framed_hello = hello_call(&framed_stream, &mut framed_pending, true, deadline)?;
        gate(&framed_hello)?;
        if !framed_hello.framed {
            return Err(ClientError::Protocol(
                "the core did not switch to framed mode".into(),
            ));
        }

        let (notices, notice_rx) = mpsc::channel();
        let core = Arc::new(Core {
            hello,
            config,
            plain: PlainConn {
                stream: plain_stream,
                write: Mutex::new(()),
                table: RpcTable::new(),
            },
            framed: FramedConn {
                stream: framed_stream,
                write: Mutex::new(()),
                table: RpcTable::new(),
                attaches: Mutex::new(HashMap::new()),
                events: Mutex::new(EventsState::default()),
            },
            closed: Mutex::new(None),
            stop: AtomicBool::new(false),
            notices,
            #[cfg(test)]
            attach_hook_pause: Mutex::new(Duration::ZERO),
        });
        let session = Session { core };
        let core = &session.core;
        spawn_named("hc-plain-reader", {
            let core = core.clone();
            move || plain_reader(core, plain_pending)
        })?;
        spawn_named("hc-framed-reader", {
            let core = core.clone();
            move || framed_reader(core, framed_pending)
        })?;
        spawn_named("hc-session-worker", {
            let core = core.clone();
            move || worker(core, observer, notice_rx)
        })?;

        // Subscribe before reading documents (upstream: "Subscribe before reading").
        session.subscribe_events()?;
        let _ = core.notices.send(Notice::ReloadDocs);
        Ok(session)
    }

    fn subscribe_events(&self) -> Result<(), ClientError> {
        let core = self.core.clone();
        // The channel and snapshot are stored on the reader thread before any event frame is
        // routed, so no event of the new channel can arrive unclaimed.
        let hook: Hook = Box::new(move |result| {
            let Ok(reply) = result else { return };
            let channel = reply
                .get("channel")
                .and_then(Value::as_u64)
                .and_then(|c| u32::try_from(c).ok());
            let store = EventStore::from_snapshot(reply, BTreeMap::new(), false).ok();
            if let (Some(channel), Some(store)) = (channel, store) {
                let mut events = lock(&core.framed.events);
                events.channel = Some(channel);
                events.store = Some(store);
            }
        });
        self.core.framed_rpc(
            "events.subscribe",
            json!({}),
            self.core.config.rpc_timeout,
            Some(hook),
        )?;
        if lock(&self.core.framed.events).store.is_none() {
            let error = ClientError::Protocol("events.subscribe reply has no channel".into());
            self.core.fail(error.clone());
            return Err(error);
        }
        Ok(())
    }

    pub fn hello(&self) -> &Hello {
        &self.core.hello
    }

    /// The first cause that ended the session, if it has ended.
    pub fn closed(&self) -> Option<ClientError> {
        lock(&self.core.closed).clone()
    }

    /// Ends the session on purpose: calls fail with [`ClientError::Closed`], every attach
    /// stream ends and the observer hears once. Safe to call again.
    ///
    /// On Unix both connections are also shut down at once, so the core releases every viewer
    /// of this session now and the readers stop without waiting out their poll slice.
    ///
    /// A Windows named pipe client has no half-close: there the pipes end when the last
    /// holder of the session's state lets go, and the core releases the viewers then. The
    /// reader threads notice within one poll slice, and the worker and the attach pumps end
    /// right after, so what remains are the handles the caller holds: the `Session` and every
    /// [`Attach`]. The caller drops those after closing (the app's controller does, as soon as
    /// its connection is torn down).
    pub fn close(&self) {
        self.core.fail(ClientError::Closed);
    }

    /// Test seam, see [`Core::attach_hook_pause`].
    #[cfg(test)]
    pub(super) fn pause_attach_hook(&self, pause: Duration) {
        *lock(&self.core.attach_hook_pause) = pause;
    }

    /// Test seam: the threads and handles that still hold the session's shared state, this
    /// session's own handle included. One once everything else let go.
    #[cfg(test)]
    pub(super) fn holders(&self) -> usize {
        Arc::strong_count(&self.core)
    }

    /// Test seam: attaches whose caller gave up and whose answer is still awaited.
    #[cfg(test)]
    pub(super) fn late_attaches(&self) -> usize {
        lock(&self.core.framed.table.inner).retired.len()
    }

    /// An RPC on the plain connection, with the default timeout.
    pub fn rpc(&self, method: &str, params: Value) -> Result<Value, ClientError> {
        self.rpc_with_timeout(method, params, self.core.config.rpc_timeout)
    }

    /// `events.subscribe` and `terminal.attach` are refused here (the first would silence the
    /// connection, the second needs framing: use [`Session::attach`]), so is any method the
    /// core did not declare in `hello`, and so is everything [`denied`] names for these
    /// `params`. All of them are [`ClientError::Unsupported`] and nothing is sent.
    pub fn rpc_with_timeout(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, ClientError> {
        if matches!(method, "events.subscribe" | "terminal.attach" | "hello")
            || denied(method, &params)
            || !self.core.hello.has(method)
        {
            return Err(ClientError::Unsupported(method.to_string()));
        }
        self.core.plain_rpc(method, params, timeout)
    }

    /// Opens a viewer on a terminal. Output reaches `sink`; the stream starts with the first
    /// paint, and stalls at 256 KiB until [`Attach::credit`] is called.
    ///
    /// The sink hears nothing when this fails. If the core's answer comes after the timeout,
    /// the viewer it opened is closed again (only a bounded number of such attaches are
    /// remembered; a viewer opened for an older one stays until the session ends). If the
    /// session ends while the answer is being handled, this still returns the attach, already
    /// ended: the sink gets [`AttachEnd::Session`] and input fails with
    /// [`ClientError::Closed`].
    pub fn attach(
        &self,
        request: &AttachRequest,
        sink: Arc<dyn AttachSink>,
    ) -> Result<Attach, ClientError> {
        let mut params = json!({
            "terminal": request.terminal,
            "cols": request.cols,
            "rows": request.rows,
            "history": request.history,
        });
        if request.display != DisplayFacts::default() {
            params["display"] = serde_json::to_value(&request.display)
                .map_err(|e| ClientError::Protocol(e.to_string()))?;
        }
        let slot: AttachSlot = Arc::new(Mutex::new(Slot::Waiting));
        let hook: Hook = {
            let (core, slot) = (self.core.clone(), slot.clone());
            Box::new(move |result| {
                #[cfg(test)]
                {
                    let pause = *lock(&core.attach_hook_pause);
                    if !pause.is_zero() {
                        thread::sleep(pause);
                    }
                }
                let Ok(reply) = result else { return };
                let Some(channel) = reply
                    .get("channel")
                    .and_then(Value::as_u64)
                    .and_then(|c| u32::try_from(c).ok())
                else {
                    return;
                };
                // Held until the outcome is published, so the caller giving up cannot fall
                // between the decision to register and the registration.
                let mut slot = lock(&slot);
                if matches!(*slot, Slot::Abandoned) {
                    let _ = core.write_frame(&Frame::close(channel));
                    return;
                }
                let inner = Arc::new(AttachInner {
                    channel,
                    terminal: reply.get("terminal").cloned().unwrap_or(Value::Null),
                    core: core.clone(),
                    sink,
                    state: Mutex::new(AttachState {
                        buf: Vec::new(),
                        last_flush: None,
                        delivered: 0,
                        ledger: CreditLedger::new(CREDIT_BATCH),
                        credit_due: None,
                        end: None,
                        detach_sent: false,
                    }),
                    wake: Condvar::new(),
                });
                // `fail` sets `stop` before it takes this table, so a stream registered after
                // that take sees `stop` here, under the same lock, and nobody else will end it.
                let ended = {
                    let mut attaches = lock(&core.framed.attaches);
                    attaches.insert(channel, inner.clone());
                    core.is_closed()
                };
                if ended {
                    lock(&core.framed.attaches).remove(&channel);
                    inner.finish(AttachEnd::Session(core.closed_code()));
                }
                let pump = inner.clone();
                *slot = match spawn_named("hc-attach-pump", move || pump.pump()) {
                    Ok(()) => Slot::Ready(inner),
                    Err(e) => {
                        lock(&core.framed.attaches).remove(&channel);
                        let _ = core.write_frame(&Frame::close(channel));
                        Slot::Failed(e)
                    }
                };
            })
        };
        // A timeout leaves the request registered (unlike `framed_rpc`): its answer, if it
        // ever comes, still has to close the viewer it opens.
        let (id, rx) = self
            .core
            .framed_send("terminal.attach", params, Some(hook))?;
        let reply = wait_reply(&rx, self.core.config.rpc_timeout, || {});
        let mut slot = lock(&slot);
        match std::mem::replace(&mut *slot, Slot::Abandoned) {
            // Also when the wait just timed out: the answer made it in time after all.
            Slot::Ready(inner) => Ok(Attach { inner }),
            Slot::Failed(e) => Err(e),
            Slot::Waiting | Slot::Abandoned => {
                // Gave up. If the hook has not run it never will for this caller: from here a
                // late answer only closes its viewer, and nothing of this call (the sink, the
                // session) stays behind in the table for it.
                self.core
                    .framed
                    .table
                    .retire(id, close_late_viewer(Arc::downgrade(&self.core)));
                Err(match reply {
                    Err(e) => e,
                    Ok(_) => ClientError::Protocol("terminal.attach reply has no channel".into()),
                })
            }
        }
    }

    /// The terminals as the event stream last described them.
    pub fn terminals(&self) -> Vec<Value> {
        lock(&self.core.framed.events)
            .store
            .as_ref()
            .map(|s| s.terminals.values().cloned().collect())
            .unwrap_or_default()
    }

    /// Reads the layout document now.
    pub fn read_layout(&self) -> Result<LayoutSnapshot, ClientError> {
        self.core.read_layout()
    }

    /// Applies one layout operation to the latest document (see [`docs::update`]).
    pub fn update_layout(&self, op: &Op) -> Result<Updated, DocError> {
        docs::update(
            &mut SessionDocIo { session: self },
            docs::DOC_NAME,
            docs::DEFAULT_ATTEMPTS,
            op,
        )
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.close();
    }
}

/// Requests [`Session::rpc`] never sends, whatever the core declares. The caller is the UI
/// layer, and none of these is something a viewer of terminals does:
///
/// * stopping or restarting the core is only ever Yuzora's own supervised step (K4: never a
///   core it did not start);
/// * touching files is not a terminal's job (prefix `file.`, which also covers methods
///   upstream adds later);
/// * the user's machines are theirs: `machine.add` and `machine.connect` dial out over ssh,
///   `machine.rename` and `machine.rm` change their list, `machine.exec` runs programs there.
///   Only `machine.list` reads (prefix `machine.`, for the same reason as `file.`);
/// * a request that names another machine (`on`, whatever its value) reaches past this core,
///   and remote machines are not part of the first round (`terminal.run`, `doc.*`, ...).
fn denied(method: &str, params: &Value) -> bool {
    matches!(method, "server.stop" | "server.restart")
        || method.starts_with("file.")
        || (method.starts_with("machine.") && method != "machine.list")
        || params.get("on").is_some()
}

struct SessionDocIo<'a> {
    session: &'a Session,
}

fn failure(e: ClientError) -> IoFailure {
    match e {
        ClientError::Rpc(e) => IoFailure::Rpc(e),
        other => IoFailure::Transport(other.code().to_string()),
    }
}

impl DocIo for SessionDocIo<'_> {
    fn get(&mut self, name: &str) -> Result<(Value, u64), IoFailure> {
        let result = self
            .session
            .rpc("doc.get", json!({"name": name}))
            .map_err(failure)?;
        Ok((
            result.get("value").cloned().unwrap_or(Value::Null),
            result.get("version").and_then(Value::as_u64).unwrap_or(0),
        ))
    }

    fn set(&mut self, name: &str, value: &Value, version: u64) -> Result<u64, IoFailure> {
        let result = self
            .session
            .rpc(
                "doc.set",
                json!({"name": name, "value": value, "version": version}),
            )
            .map_err(failure)?;
        result
            .get("version")
            .and_then(Value::as_u64)
            .ok_or_else(|| IoFailure::Transport("protocol".into()))
    }
}

#[cfg(test)]
mod table_tests {
    use super::*;

    fn retired(table: &RpcTable) -> usize {
        lock(&table.inner).retired.len()
    }

    fn waiting(table: &RpcTable) -> usize {
        lock(&table.inner).waiters.len()
    }

    #[test]
    fn retired_waiters_are_capped_and_the_oldest_is_forgotten_first() {
        let table = RpcTable::new();
        let ran = Arc::new(Mutex::new(Vec::new()));
        let total = MAX_LATE_ATTACHES + 5;
        let mut calls = Vec::new();
        for n in 0..total {
            let (id, rx) = table.register(None).unwrap();
            let ran = ran.clone();
            table.retire(id, Box::new(move |_| lock(&ran).push(n)));
            calls.push((id, rx));
        }
        assert_eq!(waiting(&table), MAX_LATE_ATTACHES);
        assert_eq!(retired(&table), MAX_LATE_ATTACHES);
        for (id, _) in &calls {
            table.complete(*id, Ok(json!({})));
        }
        // The five oldest were forgotten, so their answers found nobody; the rest ran their
        // own hook, in the order they were answered.
        assert_eq!(*lock(&ran), (5..total).collect::<Vec<_>>());
        assert_eq!((waiting(&table), retired(&table)), (0, 0));
    }

    #[test]
    fn a_waiter_that_was_answered_or_ended_is_not_retired() {
        let table = RpcTable::new();
        let ran = Arc::new(AtomicBool::new(false));
        let (answered, _rx) = table.register(None).unwrap();
        table.complete(answered, Ok(json!({})));
        let flag = ran.clone();
        table.retire(
            answered,
            Box::new(move |_| flag.store(true, Ordering::SeqCst)),
        );
        assert_eq!((waiting(&table), retired(&table)), (0, 0));

        let (ended, _rx) = table.register(None).unwrap();
        table.retire(ended, Box::new(|_| {}));
        assert_eq!((waiting(&table), retired(&table)), (1, 1));
        table.fail_all(ClientError::ConnectionLost);
        assert_eq!((waiting(&table), retired(&table)), (0, 0));
        assert!(!ran.load(Ordering::SeqCst));
    }

    #[test]
    fn retiring_replaces_the_hook_and_lets_go_of_what_the_old_one_held() {
        let table = RpcTable::new();
        let held = Arc::new(());
        let ran = Arc::new(Mutex::new(Vec::new()));
        let (id, _rx) = table
            .register(Some({
                let (held, ran) = (held.clone(), ran.clone());
                Box::new(move |_| {
                    let _ = &held;
                    lock(&ran).push("original");
                })
            }))
            .unwrap();
        assert_eq!(Arc::strong_count(&held), 2);
        table.retire(id, {
            let ran = ran.clone();
            Box::new(move |_| lock(&ran).push("late"))
        });
        assert_eq!(Arc::strong_count(&held), 1, "the original hook was dropped");
        table.complete(id, Ok(json!({})));
        assert_eq!(*lock(&ran), ["late"]);
    }
}
