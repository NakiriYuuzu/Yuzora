//! A fake herdr-core on a Unix socket, for the service tests: the plain `hello`, the switch to
//! framed mode, channel-0 RPC, one events channel, attach streams and the handful of plain
//! RPCs the adapter calls. It records what the client sent. It is not a core: no credit
//! window, no terminals, no state beyond one document.
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Duration;

use serde_json::{json, Value};
use yuzora_host::herdr_core::caps;
use yuzora_host::herdr_core::frame::{self, Decoder, Frame};

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Default)]
pub struct State {
    pub generation: u32,
    pub pid: u32,
    /// Every plain and channel-0 method, in arrival order.
    pub calls: Vec<String>,
    pub run_params: Vec<Value>,
    pub attach_params: Vec<Value>,
    /// What an attach receives right after its reply.
    pub attach_output: Vec<u8>,
    pub input: Vec<u8>,
    pub resizes: Vec<(u16, u16)>,
    pub focus: Vec<bool>,
    pub facts: Vec<Value>,
    pub credit: u64,
    pub closes: Vec<u32>,
    pub plain_connections: usize,
    pub framed_connections: usize,
    /// `terminal.kill` answers `not_found`.
    pub kill_not_found: bool,
    /// `terminal.attach` has been received but is not answered while this is set (the framed
    /// connection waits with it, as a busy core's does).
    pub hold_attach: bool,
    pub terminals: Vec<Value>,
    doc: Option<(u64, Value)>,
    streams: Vec<UnixStream>,
    framed_writer: Option<Arc<Mutex<UnixStream>>>,
    events_channel: u32,
    next_channel: u32,
}

pub struct FakeCore {
    pub socket: PathBuf,
    pub state: Arc<Mutex<State>>,
    stop: Arc<AtomicBool>,
    _dir: tempfile::TempDir,
}

impl FakeCore {
    pub fn start() -> FakeCore {
        // Short: a macOS socket path holds 103 bytes.
        let dir = tempfile::Builder::new()
            .prefix("hc")
            .tempdir_in("/tmp")
            .unwrap();
        let socket = dir.path().join("c.sock");
        let state = Arc::new(Mutex::new(State {
            generation: 1,
            pid: 4242,
            next_channel: 2,
            attach_output: b"first paint".to_vec(),
            ..State::default()
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let listener = UnixListener::bind(&socket).unwrap();
        listener.set_nonblocking(true).unwrap();
        {
            let (state, stop) = (state.clone(), stop.clone());
            thread::spawn(move || {
                while !stop.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let _ = stream.set_nonblocking(false);
                            lock(&state).streams.extend(stream.try_clone().ok());
                            let state = state.clone();
                            thread::spawn(move || serve(stream, state));
                        }
                        Err(_) => thread::sleep(Duration::from_millis(5)),
                    }
                }
            });
        }
        FakeCore {
            socket,
            state,
            stop,
            _dir: dir,
        }
    }

    pub fn calls(&self) -> Vec<String> {
        lock(&self.state).calls.clone()
    }

    pub fn count(&self, method: &str) -> usize {
        lock(&self.state)
            .calls
            .iter()
            .filter(|call| call.as_str() == method)
            .count()
    }

    /// Drops every connection, as a core crash does. New connections are still accepted.
    pub fn drop_connections(&self) {
        let mut state = lock(&self.state);
        for stream in state.streams.drain(..) {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
        state.framed_writer = None;
    }

    /// The core goes away: nothing listens any more.
    pub fn shut_down(&self) {
        self.stop.store(true, Ordering::Release);
        self.drop_connections();
        let _ = std::fs::remove_file(&self.socket);
    }

    /// A notification on the events channel.
    pub fn push_event(&self, params: Value) {
        let state = lock(&self.state);
        let (Some(writer), channel) = (state.framed_writer.clone(), state.events_channel) else {
            panic!("no framed connection to push on");
        };
        drop(state);
        let line = format!(
            "{}\n",
            json!({"jsonrpc": "2.0", "method": "event", "params": params})
        );
        write_frame(&writer, &Frame::data(channel, line.into_bytes()));
    }

    /// Output on the attach channel (the first attach is channel 2).
    pub fn push_output(&self, channel: u32, bytes: &[u8]) {
        let writer = lock(&self.state).framed_writer.clone().unwrap();
        write_frame(&writer, &Frame::data(channel, bytes.to_vec()));
    }

    /// The program ended: the core closes the stream with a reason.
    pub fn end_stream(&self, channel: u32, reason: Value) {
        let writer = lock(&self.state).framed_writer.clone().unwrap();
        write_frame(
            &writer,
            &Frame::new(channel, frame::CLOSE, reason.to_string().into_bytes()),
        );
    }
}

impl Drop for FakeCore {
    fn drop(&mut self) {
        self.shut_down();
    }
}

fn write_frame(writer: &Arc<Mutex<UnixStream>>, frame: &Frame) {
    let _ = lock(writer).write_all(&frame.encode());
}

fn reply(id: &Value, result: Value) -> Vec<u8> {
    format!(
        "{}\n",
        json!({"jsonrpc": "2.0", "id": id, "result": result})
    )
    .into_bytes()
}

fn error(id: &Value, kind: &str) -> Vec<u8> {
    format!(
        "{}\n",
        json!({"jsonrpc": "2.0", "id": id,
            "error": {"code": -32000, "message": kind, "data": {"kind": kind}}})
    )
    .into_bytes()
}

fn hello_result(state: &State, framed: bool) -> Value {
    let methods: Vec<&str> = caps::REQUIRED_METHODS
        .iter()
        .chain(caps::OPTIONAL_METHODS)
        .copied()
        .collect();
    json!({
        "version": "0.1.14",
        "generation": state.generation,
        "methods": methods,
        "features": caps::REQUIRED_FEATURES,
        "core": {"id": "fake"},
        "framed": framed,
    })
}

/// One answer for a method both modes understand; `None` for a method only framed mode has.
fn plain_method(
    state: &mut State,
    method: &str,
    params: &Value,
) -> Option<Result<Value, &'static str>> {
    state.calls.push(method.to_string());
    Some(match method {
        "server.status" => Ok(json!({"core": {"id": "fake"}, "version": "0.1.14",
            "pid": state.pid, "uptime_ms": 1_000_000})),
        "doc.get" => Ok(match &state.doc {
            Some((version, value)) => json!({"version": version, "value": value}),
            None => json!({"version": 0, "value": null}),
        }),
        "doc.set" => {
            let current = state.doc.as_ref().map_or(0, |(version, _)| *version);
            if params["version"].as_u64() != Some(current) {
                Err("conflict")
            } else {
                state.doc = Some((current + 1, params["value"].clone()));
                Ok(json!({"version": current + 1}))
            }
        }
        "terminal.list" => Ok(json!({"terminals": state.terminals, "hidden_finished": 0})),
        "terminal.run" => {
            state.run_params.push(params.clone());
            Ok(json!({"address": "t1", "state": "live", "cols": 80, "rows": 24}))
        }
        "terminal.kill" if state.kill_not_found => Err("not_found"),
        "terminal.kill" => Ok(json!({"address": params["terminal"], "state": "finished"})),
        "terminal.rm" => Ok(json!({"removed": params["terminal"]})),
        _ => return None,
    })
}

fn serve(mut stream: UnixStream, state: Arc<Mutex<State>>) {
    let mut pending = Vec::new();
    let mut buf = [0u8; 4096];
    // Plain mode: NDJSON until a `hello` asks for framing.
    loop {
        let line = loop {
            if let Some(at) = pending.iter().position(|b| *b == b'\n') {
                break pending.drain(..=at).collect::<Vec<u8>>();
            }
            match stream.read(&mut buf) {
                Ok(0) | Err(_) => return,
                Ok(n) => pending.extend_from_slice(&buf[..n]),
            }
        };
        let Ok(request) = serde_json::from_slice::<Value>(&line) else {
            return;
        };
        let (id, method, params) = (
            &request["id"],
            request["method"].as_str().unwrap_or(""),
            &request["params"],
        );
        let mut guard = lock(&state);
        if method == "hello" {
            guard.calls.push("hello".into());
            let framed = params["framed"].as_bool().unwrap_or(false);
            let result = hello_result(&guard, framed);
            if framed {
                guard.framed_connections += 1;
            } else {
                guard.plain_connections += 1;
            }
            drop(guard);
            if stream.write_all(&reply(id, result)).is_err() {
                return;
            }
            if framed {
                return serve_framed(stream, state);
            }
            continue;
        }
        let answer = match plain_method(&mut guard, method, params) {
            Some(Ok(result)) => reply(id, result),
            Some(Err(kind)) => error(id, kind),
            None => error(id, "method_not_found"),
        };
        drop(guard);
        if stream.write_all(&answer).is_err() {
            return;
        }
    }
}

fn serve_framed(mut stream: UnixStream, state: Arc<Mutex<State>>) {
    let writer = Arc::new(Mutex::new(stream.try_clone().unwrap()));
    lock(&state).framed_writer = Some(writer.clone());
    let mut decoder = Decoder::default();
    let mut buf = [0u8; 8192];
    loop {
        let frames = match stream.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(n) => match decoder.push(&buf[..n]) {
                Ok(frames) => frames,
                Err(_) => return,
            },
        };
        for frame in frames {
            handle_frame(&state, &writer, frame);
        }
    }
}

fn handle_frame(state: &Arc<Mutex<State>>, writer: &Arc<Mutex<UnixStream>>, frame: Frame) {
    let mut guard = lock(state);
    if frame.channel == 0 {
        let Ok(request) = serde_json::from_slice::<Value>(&frame.payload) else {
            return;
        };
        let (id, method, params) = (
            request["id"].clone(),
            request["method"].as_str().unwrap_or("").to_string(),
            request["params"].clone(),
        );
        let mut after: Option<Frame> = None;
        let result = match method.as_str() {
            "events.subscribe" => {
                guard.calls.push(method.clone());
                guard.events_channel = 1;
                Ok(json!({"seq": 0, "terminals": guard.terminals, "channel": 1}))
            }
            "terminal.attach" => {
                guard.calls.push(method.clone());
                guard.attach_params.push(params.clone());
                let channel = guard.next_channel;
                guard.next_channel += 1;
                if !guard.attach_output.is_empty() {
                    after = Some(Frame::data(channel, guard.attach_output.clone()));
                }
                Ok(
                    json!({"channel": channel, "terminal": {"address": params["terminal"], "state": "live"}}),
                )
            }
            other => plain_method(&mut guard, other, &params).unwrap_or(Err("method_not_found")),
        };
        drop(guard);
        if method == "terminal.attach" {
            // Bounded, so a test that forgets to let go fails instead of hanging.
            for _ in 0..4000 {
                if !lock(state).hold_attach {
                    break;
                }
                thread::sleep(Duration::from_millis(5));
            }
        }
        let body = match result {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err(kind) => json!({"jsonrpc": "2.0", "id": id,
                "error": {"code": -32000, "message": kind, "data": {"kind": kind}}}),
        };
        write_frame(writer, &Frame::data(0, body.to_string().into_bytes()));
        if let Some(after) = after {
            write_frame(writer, &after);
        }
        return;
    }
    match frame.kind {
        frame::DATA => guard.input.extend_from_slice(&frame.payload),
        frame::CREDIT => guard.credit += u64::from(frame.credit_value().unwrap_or(0)),
        frame::RESIZE => guard.resizes.extend(frame.resize_value()),
        frame::FOCUS => guard.focus.push(frame.payload.first() == Some(&1)),
        frame::FACTS => guard
            .facts
            .extend(serde_json::from_slice::<Value>(&frame.payload).ok()),
        frame::CLOSE => {
            guard.closes.push(frame.channel);
            drop(guard);
            // The viewer detached: the core answers with an empty CLOSE.
            write_frame(writer, &Frame::close(frame.channel));
        }
        _ => {}
    }
}
