//! Test support: a fake herdr-core on a real local socket (`herdr_transport`'s listener), so the
//! client is exercised through the same sockets or named pipes it uses against a real core.
//!
//! It speaks what the recordings in `tests/fixtures/herdr-core/` show: a plain `hello`, the
//! switch to framed mode, channel-0 RPC, attach streams and an events channel that obey the
//! 256 KiB credit window, `doc.get` / `doc.set` with upstream's version rules, and a plain
//! connection that goes silent once it subscribes. Streams replay recorded bytes (elided
//! stretches are zeros of the recorded length) or generated floods.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use interprocess::local_socket::traits::{Listener as _, Stream as _};
use serde_json::{json, Value};

use super::client::{
    Attach, AttachEnd, AttachRequest, AttachSink, ClientError, DisplayFacts, LayoutSnapshot,
    SessionObserver,
};
use super::events::Effect;
use super::fixtures::{self, Recorded};
use super::frame::{self, Decoder, Dir, Frame};
use super::paths::Endpoint;
use super::rpc;
use crate::herdr_transport::{
    bind_local_listener, connect_local_stream, poll_local_stream_read, wait_local_readable,
    LocalListener, LocalStream, LocalStreamRead, LocalWaitProfile,
};

/// One DATA frame at most, like the core's.
const CHUNK: usize = 32 * 1024;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What an attach on one terminal receives.
#[derive(Clone)]
pub(crate) struct AttachScript {
    /// The `Terminal` record in the attach reply.
    pub terminal: Value,
    /// Everything the core sends on the channel, first paint first, window permitting.
    pub stream: Vec<u8>,
    /// The CLOSE payload sent once the stream is out; `None` keeps the stream open.
    pub close: Option<Value>,
    /// Input the viewer types comes back as stream output.
    pub echo: bool,
    /// Largest DATA frame the core sends.
    pub frame: usize,
}

impl AttachScript {
    /// A stream of `len` bytes: lines of printable text, so merging is visible and a lost or
    /// reordered byte is detectable.
    pub fn flood(terminal: &str, len: usize) -> AttachScript {
        let stream = (0..len).map(|i| b'a' + (i % 23) as u8).collect();
        AttachScript {
            terminal: json!({"address": terminal, "state": "live"}),
            stream,
            close: None,
            echo: false,
            frame: CHUNK,
        }
    }

    /// What the recording's viewer `conn` received on its first attach channel, and how the
    /// stream ended. Stretches the recording elided are zeros of the recorded length.
    pub fn from_recording(rec: &Recorded, conn: &str) -> AttachScript {
        let frames = rec.frames(conn);
        let reply = frames
            .iter()
            .filter(|(d, f)| *d == Dir::In && f.channel == 0 && f.kind == frame::DATA)
            .find_map(|(_, f)| {
                let v: Value = serde_json::from_slice(&f.payload).ok()?;
                v.pointer("/result/channel")?;
                Some(v["result"].clone())
            })
            .expect("an attach reply in the recording");
        let channel = reply["channel"].as_u64().unwrap() as u32;
        let ours = |d: Dir, f: &Frame| f.channel == channel && d == Dir::In;
        let stream = frames
            .iter()
            .filter(|(d, f)| ours(*d, f) && f.kind == frame::DATA)
            .flat_map(|(_, f)| f.payload.clone())
            .collect();
        let close = frames
            .iter()
            .find(|(d, f)| ours(*d, f) && f.kind == frame::CLOSE)
            .and_then(|(_, f)| serde_json::from_slice(&f.payload).ok());
        AttachScript {
            terminal: reply["terminal"].clone(),
            stream,
            close,
            echo: false,
            frame: CHUNK,
        }
    }
}

/// A recording's event stream: the `events.subscribe` snapshot and every notification after it.
pub(crate) fn events_from_recording(rec: &Recorded) -> (Value, Vec<Value>) {
    let notes = rec
        .notifications()
        .into_iter()
        .map(|(method, params)| json!({"jsonrpc": "2.0", "method": method, "params": params}))
        .collect();
    (rec.subscribe_result().expect("a subscribe result"), notes)
}

#[derive(Default, Clone)]
pub(crate) struct Stats {
    pub plain_connections: usize,
    pub framed_connections: usize,
    /// A plain connection sent `events.subscribe` (the thing the client must never do).
    pub plain_subscribed: bool,
    /// Every channel-0 / plain RPC method, in arrival order.
    pub calls: Vec<String>,
    pub attach_params: Vec<Value>,
    pub sent: HashMap<u32, u64>,
    /// The most the core was ever allowed to have in flight on a channel at once: the initial
    /// window plus every CREDIT it got, minus what it had sent. The core obeys whatever
    /// credit arrives, so this is where a client that credits bytes it was never sent (or
    /// the same bytes twice) shows: above [`frame::INITIAL_CREDIT`], the window was widened.
    pub max_window: HashMap<u32, u64>,
    pub credits: HashMap<u32, Vec<u32>>,
    pub input: HashMap<u32, Vec<u8>>,
    pub resizes: Vec<(u32, u16, u16)>,
    pub focus: Vec<(u32, bool)>,
    pub facts: Vec<(u32, Value)>,
    pub closes_received: Vec<u32>,
    pub doc_gets: usize,
    /// Connections the client ended (as opposed to ones the fake dropped itself). Only the
    /// Unix-only test of `Session::close` reads it.
    #[cfg_attr(not(unix), allow(dead_code))]
    pub client_hangups: usize,
    /// What a failed connect reports: connections accepted, bytes read and written, and why
    /// the accept loop or a connection ended early.
    pub accepted: usize,
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub ended: Vec<String>,
}

impl Stats {
    pub fn summary(&self) -> String {
        format!(
            "fake core: accepted {}, read {} B, wrote {} B, calls {:?}, ended {:?}",
            self.accepted, self.bytes_in, self.bytes_out, self.calls, self.ended
        )
    }

    pub fn credit_total(&self, channel: u32) -> u64 {
        self.credits
            .get(&channel)
            .map_or(0, |c| c.iter().map(|v| u64::from(*v)).sum())
    }
}

struct Inner {
    version: String,
    generation: u32,
    pid: u32,
    started: Instant,
    terminals: Vec<Value>,
    docs: BTreeMap<String, (Value, u64)>,
    competing: Option<(String, Value)>,
    scripts: HashMap<String, AttachScript>,
    subscribe_reply: Option<Value>,
    scripted_events: Vec<Value>,
    /// Live notifications, in order; every events stream reads it from its own cursor.
    event_log: Vec<Vec<u8>>,
    seq: u64,
    next_channel: u32,
    silent: HashSet<String>,
    /// Methods `hello` declares on top of the recorded ones.
    declared: Vec<String>,
    /// Answers to these methods are held back this long (the rest of the connection waits).
    delays: HashMap<String, Duration>,
    /// CLOSE payloads to send on a channel once its output is out.
    closes: HashMap<u32, Vec<u8>>,
    /// Connections accepted before this generation are dropped.
    kill_generation: u64,
    stats: Stats,
}

struct Shared {
    inner: Mutex<Inner>,
    stop: AtomicBool,
}

pub(crate) struct FakeCore {
    path: PathBuf,
    shared: Arc<Shared>,
    acceptor: Option<JoinHandle<()>>,
}

static NEXT: AtomicUsize = AtomicUsize::new(0);

impl FakeCore {
    pub fn start() -> FakeCore {
        let path = std::env::temp_dir().join(format!(
            "yuzora-hc-fake-{}-{}.sock",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let listener = bind_local_listener(&path).expect("bind the fake core");
        let shared = Arc::new(Shared {
            inner: Mutex::new(Inner {
                version: fixtures::BUNDLED.to_string(),
                generation: 1,
                pid: 4242,
                started: Instant::now(),
                terminals: Vec::new(),
                docs: BTreeMap::new(),
                competing: None,
                scripts: HashMap::new(),
                subscribe_reply: None,
                scripted_events: Vec::new(),
                event_log: Vec::new(),
                seq: 0,
                next_channel: 1,
                silent: HashSet::new(),
                declared: Vec::new(),
                delays: HashMap::new(),
                closes: HashMap::new(),
                kill_generation: 0,
                stats: Stats::default(),
            }),
            stop: AtomicBool::new(false),
        });
        let acceptor = {
            let shared = shared.clone();
            thread::spawn(move || accept_loop(listener, shared))
        };
        FakeCore {
            path,
            shared,
            acceptor: Some(acceptor),
        }
    }

    pub fn endpoint(&self) -> Endpoint {
        Endpoint {
            session: "fake".into(),
            socket: self.path.clone(),
            holder_socket: self.path.clone(),
            state: self.path.clone(),
        }
    }

    pub fn stats(&self) -> Stats {
        lock(&self.shared.inner).stats.clone()
    }

    pub fn set_version(&self, version: &str, generation: u32) {
        let mut inner = lock(&self.shared.inner);
        inner.version = version.into();
        inner.generation = generation;
    }

    pub fn script_attach(&self, terminal: &str, script: AttachScript) {
        let mut inner = lock(&self.shared.inner);
        inner.terminals.push(script.terminal.clone());
        inner.scripts.insert(terminal.to_string(), script);
    }

    /// The next `events.subscribe` answers with `snapshot` and then streams `notes`.
    pub fn script_events(&self, snapshot: Value, notes: Vec<Value>) {
        let mut inner = lock(&self.shared.inner);
        inner.subscribe_reply = Some(snapshot);
        inner.scripted_events = notes;
    }

    /// A live notification for every subscriber (and nobody before they subscribe).
    pub fn push_event(&self, note: &Value) {
        lock(&self.shared.inner).event_log.push(rpc::line(note));
    }

    /// Another client writes the document (no version check), and a `doc` event follows.
    pub fn set_doc(&self, name: &str, value: Value) {
        let mut inner = lock(&self.shared.inner);
        write_doc(&mut inner, name, value);
    }

    /// Right before the next `doc.set` another client writes `name`.
    pub fn competing_write_before_next_set(&self, name: &str, value: Value) {
        lock(&self.shared.inner).competing = Some((name.into(), value));
    }

    pub fn doc(&self, name: &str) -> (Value, u64) {
        lock(&self.shared.inner)
            .docs
            .get(name)
            .cloned()
            .unwrap_or((Value::Null, 0))
    }

    /// `hello` declares `method` as well (the core is newer than the recording).
    pub fn declare(&self, method: &str) {
        lock(&self.shared.inner).declared.push(method.into());
    }

    /// The core never answers `method`.
    pub fn stay_silent_on(&self, method: &str) {
        lock(&self.shared.inner).silent.insert(method.into());
    }

    /// The core answers `method` only after `delay`, holding up everything else on that
    /// connection meanwhile (a core that is busy, or slow to open a viewer).
    pub fn delay_reply(&self, method: &str, delay: Duration) {
        lock(&self.shared.inner).delays.insert(method.into(), delay);
    }

    /// Ends the stream on `channel` with `reason` once everything scripted for it was sent.
    pub fn close_channel(&self, channel: u32, reason: &Value) {
        lock(&self.shared.inner)
            .closes
            .insert(channel, reason.to_string().into_bytes());
    }

    /// Drops every connection made so far (a crashed or stopped core); new ones are accepted.
    pub fn drop_connections(&self) {
        lock(&self.shared.inner).kill_generation += 1;
    }

    /// Polls `stats` until `done` or ten seconds pass.
    pub fn wait_until(&self, what: &str, done: impl Fn(&Stats) -> bool) -> Stats {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let stats = self.stats();
            if done(&stats) {
                return stats;
            }
            assert!(
                Instant::now() < deadline,
                "fake core: timed out waiting for {what}"
            );
            thread::sleep(Duration::from_millis(5));
        }
    }
}

impl Drop for FakeCore {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        // Wake the blocking accept.
        let _ = connect_local_stream(
            &self.path.to_string_lossy(),
            Instant::now() + Duration::from_secs(2),
        );
        if let Some(acceptor) = self.acceptor.take() {
            let _ = acceptor.join();
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

fn write_doc(inner: &mut Inner, name: &str, value: Value) -> u64 {
    let version = inner.docs.get(name).map_or(0, |d| d.1) + 1;
    inner.docs.insert(name.into(), (value, version));
    inner.seq += 1;
    let note = json!({"jsonrpc": "2.0", "method": "event", "params": {
        "seq": inner.seq, "type": "doc", "data": {"name": name, "version": version}}});
    inner.event_log.push(rpc::line(&note));
    version
}

fn accept_loop(listener: LocalListener, shared: Arc<Shared>) {
    loop {
        let stream = match listener.accept() {
            Ok(stream) => stream,
            Err(e) => {
                lock(&shared.inner).stats.ended.push(format!("accept: {e}"));
                return;
            }
        };
        if shared.stop.load(Ordering::Acquire) {
            return;
        }
        let born = {
            let mut inner = lock(&shared.inner);
            inner.stats.accepted += 1;
            inner.kill_generation
        };
        let shared = shared.clone();
        thread::spawn(move || {
            if let Err(e) = serve(stream, &shared, born) {
                lock(&shared.inner).stats.ended.push(format!("serve: {e}"));
            }
        });
    }
}

// ---------------------------------------------------------------------------------- connection

#[derive(PartialEq)]
enum Mode {
    Plain,
    Framed,
}

#[derive(PartialEq)]
enum Kind {
    Attach,
    Events,
}

struct Out {
    kind: Kind,
    pending: Vec<u8>,
    /// Credit left: the initial window plus what the viewer sent.
    window: u64,
    sent: u64,
    close: Option<Vec<u8>>,
    echo: bool,
    frame: usize,
    /// Events: how far into the shared log this stream has read.
    cursor: usize,
}

struct Conn<'a> {
    shared: &'a Shared,
    mode: Mode,
    lines: Vec<u8>,
    decoder: Decoder,
    out: Vec<u8>,
    streams: BTreeMap<u32, Out>,
    /// A plain connection that subscribed answers nothing more, like the real core.
    silenced: bool,
}

fn serve(stream: LocalStream, shared: &Shared, born: u64) -> io::Result<()> {
    stream.set_nonblocking(true)?;
    let mut conn = Conn {
        shared,
        mode: Mode::Plain,
        lines: Vec::new(),
        decoder: Decoder::default(),
        out: Vec::new(),
        streams: BTreeMap::new(),
        silenced: false,
    };
    let mut buf = vec![0u8; 64 * 1024];
    lock(&shared.inner).stats.plain_connections += 1;
    loop {
        if shared.stop.load(Ordering::Acquire) || lock(&shared.inner).kill_generation != born {
            return Ok(());
        }
        let mut busy = false;
        match poll_local_stream_read(&stream, &mut buf)? {
            LocalStreamRead::Data(n) => {
                busy = true;
                lock(&shared.inner).stats.bytes_in += n as u64;
                conn.ingest(&buf[..n]);
            }
            LocalStreamRead::Pending => {}
            LocalStreamRead::Closed => {
                lock(&shared.inner).stats.client_hangups += 1;
                return Ok(());
            }
        }
        busy |= conn.pump();
        match conn.flush(&stream) {
            Ok(wrote) => busy |= wrote,
            // The client hung up while the core was answering (a call still in flight when
            // it closed): the same event as reading its end.
            Err(e) if is_hangup(&e) => {
                lock(&shared.inner).stats.client_hangups += 1;
                return Ok(());
            }
            Err(e) => return Err(e),
        }
        if !busy || !conn.out.is_empty() {
            wait_local_readable(
                &stream,
                Some(Instant::now() + Duration::from_millis(2)),
                LocalWaitProfile::Interactive,
                0,
            )?;
        }
    }
}

fn is_hangup(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::BrokenPipe
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::NotConnected
    )
}

impl Conn<'_> {
    fn ingest(&mut self, bytes: &[u8]) {
        match self.mode {
            Mode::Plain => {
                self.lines.extend_from_slice(bytes);
                while let Some(at) = self.lines.iter().position(|b| *b == b'\n') {
                    let line: Vec<u8> = self.lines.drain(..=at).collect();
                    if let Ok(msg) = serde_json::from_slice::<Value>(&line) {
                        self.request(&msg);
                    }
                    if self.mode == Mode::Framed {
                        // Whatever followed the hello line is already framed.
                        let rest = std::mem::take(&mut self.lines);
                        return self.ingest(&rest);
                    }
                }
            }
            Mode::Framed => {
                let frames = self.decoder.push(bytes).expect("a frame within limits");
                for frame in frames {
                    self.frame(frame);
                }
            }
        }
    }

    fn reply(&mut self, msg: &Value) {
        match self.mode {
            Mode::Plain => self.out.extend(rpc::line(msg)),
            Mode::Framed => self
                .out
                .extend(Frame::data(0, msg.to_string().into_bytes()).encode()),
        }
    }

    fn frame(&mut self, frame: Frame) {
        if frame.channel == 0 {
            if frame.kind == frame::DATA {
                if let Ok(msg) = serde_json::from_slice::<Value>(&frame.payload) {
                    self.request(&msg);
                }
            }
            return;
        }
        let mut inner = lock(&self.shared.inner);
        let channel = frame.channel;
        match frame.kind {
            frame::DATA => {
                inner
                    .stats
                    .input
                    .entry(channel)
                    .or_default()
                    .extend(&frame.payload);
                if let Some(stream) = self.streams.get_mut(&channel) {
                    if stream.echo {
                        stream.pending.extend(&frame.payload);
                    }
                }
            }
            frame::CREDIT => {
                let value = frame.credit_value().unwrap_or(0);
                inner.stats.credits.entry(channel).or_default().push(value);
                if let Some(stream) = self.streams.get_mut(&channel) {
                    stream.window += u64::from(value);
                    let max = inner.stats.max_window.entry(channel).or_default();
                    *max = (*max).max(stream.window);
                }
            }
            frame::RESIZE => {
                let (cols, rows) = frame.resize_value().unwrap_or((0, 0));
                inner.stats.resizes.push((channel, cols, rows));
            }
            frame::FOCUS => inner
                .stats
                .focus
                .push((channel, frame.payload.first() == Some(&1))),
            frame::FACTS => {
                let facts = serde_json::from_slice(&frame.payload).unwrap_or(Value::Null);
                inner.stats.facts.push((channel, facts));
            }
            frame::CLOSE => {
                inner.stats.closes_received.push(channel);
                if self.streams.remove(&channel).is_some() {
                    self.out.extend(Frame::close(channel).encode());
                }
            }
            _ => {}
        }
    }

    fn request(&mut self, msg: &Value) {
        let method = msg["method"].as_str().unwrap_or_default().to_string();
        let id = msg["id"].clone();
        let params = &msg["params"];
        let mut inner = lock(&self.shared.inner);
        if self.silenced {
            return;
        }
        inner.stats.calls.push(method.clone());
        if inner.silent.contains(&method) {
            return;
        }
        let framed = self.mode == Mode::Framed;
        let ok = |result: Value| json!({"jsonrpc": "2.0", "id": id, "result": result});
        let err = |kind: &str, message: String| {
            json!({"jsonrpc": "2.0", "id": id, "error": {
                "code": -32000, "message": message, "data": {"kind": kind, "hint": "fake"}}})
        };
        let response = match method.as_str() {
            "hello" => {
                let mut result =
                    fixtures::json(&format!("v{}/hello.json", fixtures::BUNDLED))["result"].clone();
                result["version"] = json!(inner.version);
                result["generation"] = json!(inner.generation);
                result["framed"] = json!(params["framed"] == true);
                for method in &inner.declared {
                    result["methods"]
                        .as_array_mut()
                        .expect("methods in the recorded hello")
                        .push(json!(method));
                }
                if params["framed"] == true {
                    inner.stats.framed_connections += 1;
                    inner.stats.plain_connections -= 1;
                }
                ok(result)
            }
            "server.status" => ok(json!({
                "core": {"id": "fake", "name": "fake"}, "version": inner.version,
                "pid": inner.pid, "uptime_ms": inner.started.elapsed().as_millis() as u64,
                "holders": [], "live": inner.terminals.len(), "finished": 0})),
            "terminal.list" => ok(json!({"terminals": inner.terminals, "hidden_finished": 0})),
            "doc.get" => {
                inner.stats.doc_gets += 1;
                let name = params["name"].as_str().unwrap_or_default();
                let (value, version) = inner.docs.get(name).cloned().unwrap_or((Value::Null, 0));
                ok(json!({"value": value, "version": version}))
            }
            "doc.set" => {
                let name = params["name"].as_str().unwrap_or_default().to_string();
                if let Some((other, value)) = inner.competing.take() {
                    write_doc(&mut inner, &other, value);
                }
                let current = inner.docs.get(&name).map_or(0, |d| d.1);
                let version = params["version"].as_u64().unwrap_or(0);
                if version != current {
                    err(
                        "conflict",
                        format!("document \"{name}\" is at version {current}, not {version}"),
                    )
                } else {
                    let new = write_doc(&mut inner, &name, params["value"].clone());
                    ok(json!({"version": new}))
                }
            }
            "events.subscribe" if !framed => {
                inner.stats.plain_subscribed = true;
                self.silenced = true;
                ok(json!({"channel": null, "seq": 0, "terminals": []}))
            }
            "events.subscribe" => {
                let channel = inner.next_channel;
                inner.next_channel += 1;
                let mut result = inner
                    .subscribe_reply
                    .clone()
                    .unwrap_or_else(|| json!({"seq": inner.seq, "terminals": inner.terminals}));
                result["channel"] = json!(channel);
                let pending: Vec<u8> = inner.scripted_events.iter().flat_map(rpc::line).collect();
                let cursor = inner.event_log.len();
                inner
                    .stats
                    .max_window
                    .insert(channel, frame::INITIAL_CREDIT);
                self.streams.insert(
                    channel,
                    Out {
                        kind: Kind::Events,
                        pending,
                        window: frame::INITIAL_CREDIT,
                        sent: 0,
                        close: None,
                        echo: false,
                        frame: CHUNK,
                        cursor,
                    },
                );
                ok(result)
            }
            "terminal.attach" if framed => {
                inner.stats.attach_params.push(params.clone());
                let terminal = params["terminal"].as_str().unwrap_or_default();
                match inner.scripts.get(terminal).cloned() {
                    None => err("not_found", format!("no terminal {terminal}")),
                    Some(script) => {
                        let channel = inner.next_channel;
                        inner.next_channel += 1;
                        inner
                            .stats
                            .max_window
                            .insert(channel, frame::INITIAL_CREDIT);
                        self.streams.insert(
                            channel,
                            Out {
                                kind: Kind::Attach,
                                pending: script.stream.clone(),
                                window: frame::INITIAL_CREDIT,
                                sent: 0,
                                close: script.close.as_ref().map(|c| c.to_string().into_bytes()),
                                echo: script.echo,
                                frame: script.frame,
                                cursor: 0,
                            },
                        );
                        ok(json!({"channel": channel, "terminal": script.terminal}))
                    }
                }
            }
            "terminal.attach" => err("needs_framed", "attach needs a framed connection".into()),
            other => err("unknown_method", format!("unknown method \"{other}\"")),
        };
        let delay = inner.delays.get(&method).copied();
        drop(inner);
        if let Some(delay) = delay {
            thread::sleep(delay);
        }
        self.reply(&response);
        if method == "hello" && params["framed"] == true {
            // The reply above was the last plain line.
            self.mode = Mode::Framed;
        }
    }

    /// Sends what the windows allow. Returns whether anything moved.
    fn pump(&mut self) -> bool {
        if self.mode != Mode::Framed {
            return false;
        }
        let mut moved = false;
        let mut inner = lock(&self.shared.inner);
        let mut finished = Vec::new();
        for (channel, stream) in &mut self.streams {
            if stream.kind == Kind::Events {
                while stream.cursor < inner.event_log.len() {
                    stream.pending.extend(&inner.event_log[stream.cursor]);
                    stream.cursor += 1;
                }
            }
            while !stream.pending.is_empty() && stream.window > 0 && self.out.len() < 256 * 1024 {
                let n = stream
                    .pending
                    .len()
                    .min(stream.window as usize)
                    .min(stream.frame);
                let payload: Vec<u8> = stream.pending.drain(..n).collect();
                self.out.extend(Frame::data(*channel, payload).encode());
                stream.window -= n as u64;
                stream.sent += n as u64;
                *inner.stats.sent.entry(*channel).or_default() += n as u64;
                moved = true;
            }
            if stream.pending.is_empty() {
                if let Some(reason) = inner.closes.remove(channel) {
                    stream.close = Some(reason);
                }
                if let Some(reason) = stream.close.take() {
                    self.out
                        .extend(Frame::new(*channel, frame::CLOSE, reason).encode());
                    finished.push(*channel);
                    moved = true;
                }
            }
        }
        for channel in finished {
            self.streams.remove(&channel);
        }
        moved
    }

    /// Writes as much as the socket takes. Returns whether anything was written.
    fn flush(&mut self, stream: &LocalStream) -> io::Result<bool> {
        let mut wrote = false;
        let mut socket = stream;
        while !self.out.is_empty() {
            match socket.write(&self.out) {
                Ok(0) => break,
                Ok(n) => {
                    self.out.drain(..n);
                    lock(&self.shared.inner).stats.bytes_out += n as u64;
                    wrote = true;
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(e),
            }
        }
        Ok(wrote)
    }
}

// ---------------------------------------------------------------------------------- shared helpers

#[derive(Default)]
pub(crate) struct Recorder {
    pub effects: Mutex<Vec<Effect>>,
    pub layouts: Mutex<Vec<LayoutSnapshot>>,
    pub closed: Mutex<Vec<ClientError>>,
}

impl SessionObserver for Recorder {
    fn on_effects(&self, effects: &[Effect]) {
        self.effects.lock().unwrap().extend_from_slice(effects);
    }

    fn on_layout(&self, layout: &LayoutSnapshot) {
        self.layouts.lock().unwrap().push(layout.clone());
    }

    fn on_closed(&self, cause: &ClientError) {
        self.closed.lock().unwrap().push(cause.clone());
    }
}

#[derive(Default)]
pub(crate) struct Collect {
    pub chunks: Mutex<Vec<Vec<u8>>>,
    pub end: Mutex<Option<AttachEnd>>,
    pub ends: AtomicUsize,
}

impl AttachSink for Collect {
    fn output(&self, bytes: Vec<u8>) {
        assert!(
            self.end.lock().unwrap().is_none(),
            "output after the stream ended"
        );
        self.chunks.lock().unwrap().push(bytes);
    }

    fn closed(&self, end: AttachEnd) {
        self.ends.fetch_add(1, Ordering::SeqCst);
        *self.end.lock().unwrap() = Some(end);
    }
}

impl Collect {
    pub fn total(&self) -> usize {
        self.chunks.lock().unwrap().iter().map(Vec::len).sum()
    }

    pub fn bytes(&self) -> Vec<u8> {
        self.chunks.lock().unwrap().concat()
    }

    pub fn chunk_count(&self) -> usize {
        self.chunks.lock().unwrap().len()
    }

    pub fn end(&self) -> Option<AttachEnd> {
        self.end.lock().unwrap().clone()
    }
}

pub(crate) fn eventually(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(2));
    }
}

pub(crate) fn request(terminal: &str) -> AttachRequest {
    AttachRequest {
        terminal: terminal.into(),
        cols: 100,
        rows: 30,
        history: 0,
        display: DisplayFacts::default(),
    }
}

/// Plays the renderer: credits what the sink has received until `total` bytes arrived.
pub(crate) fn consume(attach: &Attach, sink: &Collect, total: usize) {
    let mut credited = 0;
    eventually("the stream", || {
        let got = sink.total();
        if got > credited {
            attach.credit(got - credited);
            credited = got;
        }
        got >= total
    });
}
