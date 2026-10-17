//! Write queue belongs to the peer. Only the owned blocking executor calls Host.
use super::*;
use crate::{
    audit::Audit,
    clock::{Clock, Timestamp},
};
use std::time::Duration;
use yuzora_host::herdr_service::HerdrScrollDirection;

const INPUT_BYTES: usize = 4096;
const PROMPT_BYTES: usize = 16 * 1024;
const WRITE_QUEUE: usize = 16;

pub(super) struct WriteStream {
    pub(super) audit: Audit,
    pub(super) announced: bool,
    leased: Arc<AtomicBool>,
    pub(super) takeover: bool,
    bucket: Arc<Mutex<Option<Bucket>>>,
    state: Arc<StreamState>,
}
struct Bucket {
    reservation: crate::admin::EventReservation,
    until: Timestamp,
    bytes: usize,
}
impl WriteStream {
    fn flush(&self, force: bool) {
        let mut bucket = self.bucket.lock().unwrap();
        if bucket
            .as_ref()
            .is_some_and(|b| force || self.audit.clock.now() >= b.until)
        {
            let b = bucket.take().unwrap();
            b.reservation.emit(self.audit.event(
                if self.audit.scope == Scope::Full {
                    "full.input"
                } else {
                    "terminal.input"
                },
                json!({"bytes":b.bytes}),
            ));
        }
    }
}
impl Drop for WriteStream {
    fn drop(&mut self) {
        self.flush(true);
        let reason = self.state.closed.lock().unwrap().unwrap_or("peer-closed");
        self.state.close(reason);
        if self.announced || self.leased.load(Ordering::SeqCst) {
            self.audit.tightened(reason, self.announced);
        } else {
            self.audit.open_failed(reason);
        }
    }
}
struct Validity {
    life: Arc<Life>,
    state: Option<Arc<StreamState>>,
    clock: Clock,
    until: Timestamp,
    cancelled: Arc<AtomicBool>,
}
impl Validity {
    fn start(&self, started: &AtomicBool) -> herdr::Result<()> {
        self.start_checked(started, || {})
    }
    fn start_checked(&self, started: &AtomicBool, between: impl FnOnce()) -> herdr::Result<()> {
        // Paired with SeqCst invalidation then started.load on cancellation.
        // A failed final check may conservatively report unknown, never false dropped.
        started.store(true, Ordering::SeqCst);
        between(); // deterministic test seam; production passes a no-op
        self.check()
    }
    fn check(&self) -> herdr::Result<()> {
        if self.cancelled.load(Ordering::SeqCst)
            || !self.life.active()
            || self.clock.now() >= self.until
            || self.state.as_ref().is_some_and(|s| !s.active())
        {
            Err("input-dropped")
        } else {
            Ok(())
        }
    }
}
struct Work {
    audit: Audit,
    device: String,
    id: Value,
    stream: Option<u32>,
    input: bool,
    started: Arc<AtomicBool>,
    cancelled: Arc<AtomicBool>,
    run: Box<dyn FnOnce() -> herdr::Result<Output> + Send>,
}
struct PendingWrite {
    id: Value,
    stream: Option<u32>,
    input: bool,
    started: Arc<AtomicBool>,
    cancelled: Arc<AtomicBool>,
    audit: Audit,
}
impl PendingWrite {
    fn cancelled(self) -> Value {
        self.cancelled.store(true, Ordering::SeqCst);
        let unknown = self.started.load(Ordering::SeqCst);
        let code = match (self.input, unknown) {
            (true, true) => "input-outcome-unknown",
            (true, false) => "input-dropped",
            (false, true) => "write-outcome-unknown",
            (false, false) => "write-dropped",
        };
        self.audit
            .cancelled(self.input, if unknown { "unknown" } else { "dropped" });
        write_error(self.id, self.stream, code)
    }
}
pub(super) fn write_error(id: Value, stream: Option<u32>, code: &'static str) -> Value {
    json!({"id":id,"stream":stream,"error":{"code":code,"message":code}})
}
struct Watchdog {
    until: Timestamp,
    id: Value,
    stream: Option<u32>,
    audit: Audit,
}
pub(super) struct Queue {
    sender: mpsc::Sender<Work>,
    receiver: mpsc::Receiver<Work>,
    pub(super) active: bool,
    pub(super) stalled: bool,
    watchdog: Option<Watchdog>,
    stalled_stream: Option<u32>,
    active_write: Option<PendingWrite>,
    rejections: Arc<Mutex<crate::audit::Rejections>>,
    cancelled: Vec<Value>,
}
impl Queue {
    pub(super) fn finished(&mut self) -> bool {
        self.active = false;
        self.watchdog = None;
        self.active_write = None;
        std::mem::replace(&mut self.stalled, false)
    }
    pub(super) fn new() -> Self {
        let (sender, receiver) = mpsc::channel(WRITE_QUEUE);
        Self {
            sender,
            receiver,
            active: false,
            stalled: false,
            watchdog: None,
            stalled_stream: None,
            active_write: None,
            rejections: Arc::new(Mutex::new(crate::audit::Rejections::default())),
            cancelled: Vec::new(),
        }
    }
}
pub(crate) fn is_write(method: &str) -> bool {
    matches!(
        method,
        "terminal.control"
            | "terminal.input"
            | "terminal.scroll"
            | "terminal.release"
            | "agent.prompt"
            | "pane.sendKeys"
            | "full.open"
            | "full.input"
            | "full.resize"
            | "full.close"
    )
}
fn method_scope(method: &str) -> Scope {
    if method.starts_with("full.") {
        Scope::Full
    } else {
        Scope::Control
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Control {
    session: String,
    target: String,
    #[serde(default)]
    takeover: bool,
    #[serde(default)]
    confirm_resize: bool,
    #[serde(default, rename = "cols")]
    _cols: Value,
    #[serde(default, rename = "rows")]
    _rows: Value,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Full {
    session: String,
    cols: u16,
    rows: u16,
    #[serde(default)]
    confirm_resize: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Input {
    stream: u32,
    text: Option<String>,
    bytes_b64: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Resize {
    stream: u32,
    cols: u16,
    rows: u16,
    #[serde(default)]
    confirm_resize: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Scroll {
    stream: u32,
    direction: HerdrScrollDirection,
    lines: u32,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Prompt {
    session: String,
    target: String,
    text: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Keys {
    session: String,
    target: String,
    keys: Vec<String>,
}
fn parse<T: serde::de::DeserializeOwned>(value: Value) -> herdr::Result<T> {
    serde_json::from_value(value).map_err(|_| "invalid-params")
}
fn full_size(cols: u16, rows: u16, confirmed: bool) -> herdr::Result<(u16, u16)> {
    if !(20..=400).contains(&cols) || !(8..=200).contains(&rows) {
        return Err("invalid-params");
    }
    if !confirmed {
        return Err("size-confirmation-required");
    }
    Ok((cols, rows))
}
fn input_bytes(p: Input) -> herdr::Result<(u32, Vec<u8>)> {
    let bytes = match (p.text, p.bytes_b64) {
        (Some(text), None) => text.into_bytes(),
        (None, Some(b64)) if b64.len() <= INPUT_BYTES.div_ceil(3) * 4 => {
            B64.decode(b64).map_err(|_| "invalid-params")?
        }
        (None, Some(_)) => return Err("payload-too-large"),
        _ => return Err("invalid-params"),
    };
    if bytes.len() > INPUT_BYTES {
        return Err("payload-too-large");
    }
    Ok((p.stream, bytes))
}
fn mapped_keys(keys: Vec<String>) -> herdr::Result<Vec<String>> {
    if keys.is_empty() || keys.len() > 32 {
        return Err("invalid-params");
    }
    keys.into_iter()
        .map(|k| {
            Ok(match k.as_str() {
                "Enter" => "enter",
                "Escape" => "esc",
                "Tab" => "tab",
                "BackTab" => "shift+tab",
                "ArrowUp" => "up",
                "ArrowDown" => "down",
                "ArrowLeft" => "left",
                "ArrowRight" => "right",
                "Ctrl-C" => "ctrl+c",
                "Ctrl-D" => "ctrl+d",
                _ => return Err("key-not-allowed"),
            }
            .into())
        })
        .collect()
}
fn agent_target(snapshot: &Value, target: &str) -> herdr::Result<String> {
    let pane = snapshot["panes"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|p| p["pane_id"] == target || p["terminal_id"] == target)
        .ok_or("target-not-found")?;
    let id = pane["pane_id"]
        .as_str()
        .filter(|s| herdr::valid_id(s))
        .ok_or("target-not-found")?;
    if !snapshot["agents"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|a| a["pane_id"] == id)
    {
        return Err("no-agent");
    }
    Ok(id.into())
}
impl Reads {
    pub(crate) fn write_call(
        &mut self,
        gateway: &Gateway,
        ctx: &AuthContext,
        id: Value,
        method: &str,
        params: Value,
    ) -> std::result::Result<Option<Value>, ()> {
        let scope = method_scope(method);
        // Tightening cannot depend on a still-fresh elevation or audit capacity.
        let tightening = matches!(method, "terminal.release" | "full.close");
        let result = gateway.with_authorized(
            ctx,
            if tightening { Scope::View } else { scope },
            |identity| {
                let fresh = method == "full.open"
                    || (method == "terminal.control" && params["takeover"] == true);
                let deadline = if tightening {
                    Some(gateway.clock.now())
                } else {
                    identity.write_deadline(ctx, scope, fresh)
                };
                let until = deadline.ok_or("fresh-elevation-required")?;
                let fresh_until = if fresh {
                    identity.freshness_deadline(ctx, scope)
                } else {
                    None
                };
                self.admit_write(gateway, ctx, id.clone(), method, params, until, fresh_until)
            },
        );
        let code = match result {
            Ok(Ok(value)) => return Ok(value),
            Ok(Err(code)) => code,
            Err(crate::webauthn::SCOPE_NOT_GRANTED) => "scope-not-granted",
            Err((axum::http::StatusCode::FORBIDDEN, _)) => "elevation-required",
            Err(_) => return Err(()),
        };
        let audit = Audit::new(
            gateway,
            ctx.device_id(),
            self.peer_id,
            scope,
            String::new(),
            None,
            self.writes.rejections.clone(),
        );
        audit.rejected();
        let message = match code {
            "scope-not-granted" => "Scope not granted",
            "elevation-required" => "Elevation required",
            _ => code,
        };
        Ok(Some(
            json!({"id":id,"error":{"code":code,"message":message}}),
        ))
    }
    #[allow(clippy::too_many_arguments)]
    fn admit_write(
        &mut self,
        gateway: &Gateway,
        ctx: &AuthContext,
        id: Value,
        method: &str,
        params: Value,
        until: Timestamp,
        fresh_until: Option<Timestamp>,
    ) -> herdr::Result<Option<Value>> {
        let scope = method_scope(method);
        if matches!(method, "terminal.release" | "full.close") {
            let p: CloseParams = parse(params)?;
            let s = self.streams.get(&p.stream).ok_or("stream-not-found")?;
            if s.write.as_ref().is_none_or(|w| w.audit.scope != scope) {
                return Err("invalid-stream-type");
            }
            s.state.close("client-closed");
            self.streams.remove(&p.stream);
            self.discard_write_stream(p.stream);
            return Ok(Some(reply(id, json!({"stream":p.stream,"closed":true}))));
        }
        if self.writes.stalled {
            if params["stream"]
                .as_u64()
                .is_some_and(|s| Some(s as u32) == self.writes.stalled_stream)
            {
                return Err("stream-stalled");
            }
            return Err("busy");
        }
        if self.writes.sender.capacity() == 0 {
            return Err("busy");
        }
        if matches!(method, "terminal.control" | "full.open") {
            return self.admit_open(gateway, ctx, id, method, params, until, fresh_until);
        }
        let port = gateway.herdr.port.clone();
        let life = self.life.clone();
        let clock = gateway.clock.clone();
        let work = if matches!(method, "agent.prompt" | "pane.sendKeys") {
            let (session, target, text, keys) = if method == "agent.prompt" {
                let p: Prompt = parse(params)?;
                if p.text.len() > PROMPT_BYTES {
                    return Err("payload-too-large");
                }
                (p.session, p.target, Some(p.text), None)
            } else {
                let p: Keys = parse(params)?;
                (p.session, p.target, None, Some(mapped_keys(p.keys)?))
            };
            if !herdr::valid_id(&target) {
                return Err("invalid-params");
            }
            let reads = gateway.herdr.clone();
            let audit = Audit::new(
                gateway,
                ctx.device_id(),
                self.peer_id,
                scope,
                session.clone(),
                Some(target.clone()),
                self.writes.rejections.clone(),
            );
            let started = Arc::new(AtomicBool::new(false));
            let cancelled = Arc::new(AtomicBool::new(false));
            let action = method.to_owned();
            Work {
                audit: audit.clone(),
                device: ctx.device_id().into(),
                id,
                stream: None,
                input: false,
                started: started.clone(),
                cancelled: cancelled.clone(),
                run: Box::new(move || {
                    let validity = Validity {
                        life,
                        state: None,
                        clock,
                        until,
                        cancelled,
                    };
                    validity.check()?;
                    reads.running(&session)?;
                    let raw = port.snapshot(&session)?;
                    let target = agent_target(&raw.snapshot, &target)?;
                    let summary = if let Some(text) = &text {
                        json!({"prefix":text.chars().take(200).collect::<String>(),"length":text.chars().count(),"bytes":text.len()})
                    } else {
                        json!({"keys":keys.as_ref().unwrap().len()})
                    };
                    audit.admit(&action, summary)?;
                    validity.start(&started)?; // Last boundary before Host's synchronous discovery + IO.
                    if let Some(text) = text {
                        port.prompt(&session, &target, text)?;
                    } else {
                        port.keys(&session, &target, keys.unwrap())?;
                    }
                    Ok(Output::Value(json!({"sent":true})))
                }),
            }
        } else {
            enum Op {
                Input(Vec<u8>),
                Scroll(HerdrScrollDirection, u32),
                Resize((u16, u16)),
            }
            let (stream, op) = match method {
                "terminal.input" | "full.input" => {
                    let (s, b) = input_bytes(parse(params)?)?;
                    (s, Op::Input(b))
                }
                "terminal.scroll" => {
                    let p: Scroll = parse(params)?;
                    if p.lines == 0 || p.lines > 1000 {
                        return Err("invalid-params");
                    }
                    (p.stream, Op::Scroll(p.direction, p.lines))
                }
                "full.resize" => {
                    let p: Resize = parse(params)?;
                    (
                        p.stream,
                        Op::Resize(full_size(p.cols, p.rows, p.confirm_resize)?),
                    )
                }
                _ => return Err("invalid-params"),
            };
            let s = self.streams.get(&stream).ok_or("stream-not-found")?;
            let w = s
                .write
                .as_ref()
                .filter(|w| w.audit.scope == scope)
                .ok_or("invalid-stream-type")?;
            let resource = s.lease.as_ref().ok_or("busy")?.id().to_owned();
            let state = s.state.clone();
            let audit = w.audit.clone();
            let bucket = w.bucket.clone();
            let action = method.to_owned();
            let input = matches!(op, Op::Input(_));
            let started = Arc::new(AtomicBool::new(false));
            let cancelled = Arc::new(AtomicBool::new(false));
            Work {
                audit: audit.clone(),
                device: ctx.device_id().into(),
                id,
                stream: Some(stream),
                input,
                started: started.clone(),
                cancelled: cancelled.clone(),
                run: Box::new(move || {
                    let validity = Validity {
                        life,
                        state: Some(state),
                        clock,
                        until,
                        cancelled,
                    };
                    validity.check()?;
                    if let Op::Input(bytes) = op {
                        // Reserve a future aggregate event before any input. A full
                        // admin queue refuses even when this stream has an old bucket.
                        if !audit.sender.available() {
                            return Err("audit-unavailable");
                        }
                        let mut b = bucket.lock().unwrap();
                        if b.is_none() {
                            if let Some(reservation) = audit.sender.reserve_bucket() {
                                *b = Some(Bucket {
                                    reservation,
                                    until: audit.clock.now().after(Duration::from_secs(10)),
                                    bytes: 0,
                                });
                            }
                        }
                        let count = bytes.len();
                        if let Some(bucket) = b.as_mut() {
                            bucket.bytes += count;
                        } else {
                            // Twelve optional bucket permits: synchronous audit instead.
                            audit.admit(&action, json!({"bytes":count}))?;
                        }
                        drop(b); // Never hold an audit lock across blocking Host IO.
                        validity.start(&started)?;
                        port.input(&resource, bytes)?;
                        Ok(Output::Value(json!({"stream":stream,"bytes":count})))
                    } else {
                        audit.admit(&action, json!({"accepted":true}))?;
                        validity.start(&started)?;
                        match op {
                            Op::Scroll(d, n) => port.scroll(&resource, d, n)?,
                            Op::Resize(size) => port.resize(&resource, size)?,
                            _ => unreachable!(),
                        }
                        Ok(Output::Value(json!({"stream":stream,"sent":true})))
                    }
                }),
            }
        };
        self.writes.sender.try_send(work).map_err(|_| "busy")?;
        Ok(None)
    }
    #[allow(clippy::too_many_arguments)]
    fn admit_open(
        &mut self,
        gateway: &Gateway,
        ctx: &AuthContext,
        id: Value,
        method: &str,
        params: Value,
        until: Timestamp,
        fresh_until: Option<Timestamp>,
    ) -> herdr::Result<Option<Value>> {
        if gateway
            .deferred_audit
            .lock()
            .unwrap()
            .pending_for(ctx.device_id())
        {
            return Err("audit-unavailable");
        }
        if self.streams.len() >= MAX_STREAMS {
            return Err("stream-limit");
        }
        let scope = method_scope(method);
        let (session, target, size, takeover) = if scope == Scope::Control {
            let p: Control = parse(params)?;
            if !herdr::valid_id(&p.target) {
                return Err("invalid-params");
            }
            // No authoritative PTY-size source exists in 0.9.1. Do not cache a
            // guessed size: another direct/full client could already change it.
            if !p.confirm_resize {
                return Err("size-confirmation-required");
            }
            (p.session, Some(p.target), None, p.takeover)
        } else {
            let p: Full = parse(params)?;
            (
                p.session,
                None,
                Some(full_size(p.cols, p.rows, p.confirm_resize)?),
                false,
            )
        };
        let full = if scope == Scope::Full {
            let mut devices = gateway.full_devices.lock().unwrap();
            devices.retain(|_, s| s.strong_count() > 0);
            let semaphore = devices
                .get(ctx.device_id())
                .and_then(std::sync::Weak::upgrade)
                .unwrap_or_else(|| {
                    let s = Arc::new(Semaphore::new(1));
                    devices.insert(ctx.device_id().into(), Arc::downgrade(&s));
                    s
                });
            Some(semaphore.try_acquire_owned().map_err(|_| "full-limit")?)
        } else {
            None
        };
        let reserved = gateway.herdr_jobs.reserve()?;
        let stream = self.next_id;
        self.next_id = self.next_id.checked_add(1).ok_or("stream-limit")?;
        let audit = Audit::new(
            gateway,
            ctx.device_id(),
            self.peer_id,
            scope,
            session.clone(),
            target.clone(),
            self.writes.rejections.clone(),
        );
        let state = Arc::new(StreamState {
            life: self.life.clone(),
            closed: Mutex::new(None),
            ready: AtomicBool::new(false),
            ready_changed: Notify::new(),
            changed: self.changed.clone(),
            slots: Arc::new(Semaphore::new(STREAM_FRAMES)),
            bytes: Arc::new(Semaphore::new(STREAM_BYTES)),
            deadline: Mutex::new(Some((gateway.clock.clone(), until))),
        });
        let sink = Sink {
            id: stream,
            state: state.clone(),
            sender: self.sender.clone(),
            bytes: self.bytes.clone(),
        };
        let reads = gateway.herdr.clone();
        let jobs = gateway.herdr_jobs.clone();
        let life = self.life.clone();
        let clock = gateway.clock.clone();
        let action = method.to_owned();
        let stream_session = session.clone();
        let stream_audit = audit.clone();
        let stream_state = state.clone();
        // Conservatively tracks whether Host may hold a lease, including an in-flight open.
        let leased = Arc::new(AtomicBool::new(false));
        let stream_leased = leased.clone();
        // Queued opens are ordered; full.open coordination occurs in pump only
        // after the preceding control open's completion was published.
        let started = Arc::new(AtomicBool::new(false));
        let cancelled = Arc::new(AtomicBool::new(false));
        let work = Work {
            audit: stream_audit.clone(),
            device: ctx.device_id().into(),
            id,
            stream: Some(stream),
            input: false,
            started: started.clone(),
            cancelled: cancelled.clone(),
            run: Box::new(move || {
                let validity = Validity {
                    life,
                    state: Some(state.clone()),
                    clock,
                    until,
                    cancelled,
                };
                validity.check()?;
                reads.running(&session)?;
                let (target, size) = if let Some(target) = target {
                    let raw = reads.port.snapshot(&session)?;
                    let (target, cols, rows) = herdr::observe_geometry(&raw.snapshot, &target)?;
                    (Some(target), (cols, rows))
                } else {
                    (None, size.unwrap())
                };
                let mut audit = audit;
                audit.target = target.clone();
                let announcement = audit.reserve_announcement()?;
                audit.admit(&action, json!({"takeover":takeover,"resizeConfirmed":true}))?;
                validity.check()?;
                if fresh_until.is_some_and(|fresh| validity.clock.now() > fresh) {
                    return Err("fresh-elevation-required");
                }
                validity.start(&started)?;
                leased.store(true, Ordering::SeqCst);
                let resource = if let Some(target) = &target {
                    reads.port.control(
                        &session,
                        target,
                        size,
                        takeover,
                        Arc::new(move |e| sink.terminal(e)),
                    )
                } else {
                    reads
                        .port
                        .full(&session, size, Arc::new(move |e| sink.terminal(e)))
                }
                .inspect_err(|_| leased.store(false, Ordering::SeqCst))?;
                let mut lease = Lease::new(
                    jobs,
                    reads.port.clone(),
                    Resource::Observe(resource),
                    reserved,
                );
                lease.audit_release(audit.clone());
                if let Some(full) = full {
                    lease.hold_full(full);
                }
                if !validity.life.active() || validity.clock.now() >= until {
                    return Err("input-dropped");
                }
                Ok(Output::Opened(
                    lease,
                    json!({"stream":stream,"cols":size.0,"rows":size.1,"until":until,"target":target}),
                    Some(announcement),
                ))
            }),
        };
        // The sender is the only admission boundary. No Host call occurs while
        // the identity lock is held; try_send never awaits.
        self.writes.sender.try_send(work).map_err(|_| "busy")?;
        let state = stream_state;
        self.streams.insert(
            stream,
            Stream {
                session: stream_session,
                events: false,
                state: state.clone(),
                lease: None,
                write: Some(WriteStream {
                    audit: stream_audit,
                    announced: false,
                    leased: stream_leased,
                    takeover,
                    bucket: Arc::new(Mutex::new(None)),
                    state,
                }),
            },
        );
        Ok(None)
    }
    pub(crate) fn pump_writes(&mut self, gateway: &Gateway) -> Vec<Value> {
        if self.writes.active {
            return Vec::new();
        }
        let Ok(work) = self.writes.receiver.try_recv() else {
            return Vec::new();
        };
        let mut leases = Vec::new();
        let mut closed = Vec::new();
        if let Some(full) = work
            .stream
            .and_then(|id| self.streams.get(&id))
            .filter(|s| {
                s.lease.is_none()
                    && s.write
                        .as_ref()
                        .is_some_and(|w| w.audit.scope == Scope::Full)
            })
        {
            let session = full.session.clone();
            let ids: Vec<_> = self
                .streams
                .iter()
                .filter(|(_, s)| {
                    s.session == session
                        && s.write
                            .as_ref()
                            .is_some_and(|w| w.audit.scope == Scope::Control)
                })
                .map(|(id, _)| *id)
                .collect();
            for id in ids {
                let mut s = self.streams.remove(&id).unwrap();
                s.state.close("full-opened");
                self.discard_write_stream(id);
                if let Some(lease) = s.lease.take() {
                    leases.push(lease);
                }
                closed.push(
                    json!({"event":"stream.closed","data":{"stream":id,"reason":"full-opened"}}),
                );
            }
        }
        let id = work.id.clone();
        let stream = work.stream;
        let input = work.input;
        let run = work.run;
        let audit = work.audit;
        let watchdog = Watchdog {
            until: gateway.clock.now().after(Duration::from_secs(10)),
            id: id.clone(),
            stream,
            audit: audit.clone(),
        };
        let pending_write = Some(PendingWrite {
            id: id.clone(),
            stream,
            input,
            started: work.started.clone(),
            cancelled: work.cancelled,
            audit: audit.clone(),
        });
        match gateway
            .herdr_jobs
            .submit_device(&work.device, true, move || {
                for lease in leases {
                    lease.release_now();
                }
                let result = run().map_err(|code| {
                    if code == "input-dropped" {
                        match (input, work.started.load(Ordering::SeqCst)) {
                            (true, true) => "input-outcome-unknown",
                            (true, false) => "input-dropped",
                            (false, true) => "write-outcome-unknown",
                            (false, false) => "write-dropped",
                        }
                    } else {
                        code
                    }
                });
                if result.is_err() {
                    audit.rejected();
                }
                result
            }) {
            Ok(receiver) => {
                self.writes.active = true;
                self.writes.watchdog = Some(watchdog);
                self.writes.active_write = pending_write;
                self.completed.push(Box::pin(async move {
                    Completion {
                        id,
                        stream,
                        result: receiver.await.unwrap_or(Err("herdr-unavailable")),
                        write: true,
                    }
                }));
            }
            Err(code) => {
                if let Some(stream) = stream {
                    if self.streams.get(&stream).is_some_and(|s| s.lease.is_none()) {
                        self.streams.remove(&stream);
                    }
                }
                closed.push(write_error(id, stream, code));
            }
        }
        closed
    }
    pub(crate) fn write_tick(&mut self, gateway: &Gateway) -> Vec<Value> {
        if self
            .writes
            .watchdog
            .as_ref()
            .is_some_and(|w| gateway.clock.now() >= w.until)
        {
            let watchdog = self.writes.watchdog.take().unwrap();
            self.writes.stalled = true;
            self.writes.stalled_stream = watchdog.stream;
            watchdog.audit.stalled("write");
            if let Some(id) = watchdog.stream {
                if let Some(stream) = self.streams.get(&id) {
                    stream.state.close("stream-stalled");
                }
                if self.streams.get(&id).is_some_and(|s| s.lease.is_none()) {
                    self.streams.remove(&id);
                    self.discard_write_stream(id);
                }
            }
            self.writes
                .cancelled
                .push(if let Some(input) = self.writes.active_write.take() {
                    input.cancelled()
                } else {
                    write_error(watchdog.id, watchdog.stream, "stream-stalled")
                });
        }
        for s in self.streams.values() {
            if let Some(w) = &s.write {
                if s.state
                    .deadline
                    .lock()
                    .unwrap()
                    .as_ref()
                    .is_some_and(|(_, until)| gateway.clock.now() >= *until)
                {
                    s.state.close("elevation-expired");
                }
                w.flush(false);
            }
        }
        self.closed()
    }
    pub(crate) fn write_deadline(&self, gateway: &Gateway) -> Timestamp {
        self.streams
            .values()
            .filter(|s| s.state.closed.lock().unwrap().is_none())
            .filter_map(|s| {
                s.write.as_ref().map(|w| {
                    let expiry = s.state.deadline.lock().unwrap().as_ref().unwrap().1;
                    w.bucket
                        .lock()
                        .unwrap()
                        .as_ref()
                        .map_or(expiry, |b| expiry.min(b.until))
                })
            })
            .chain(self.writes.watchdog.as_ref().map(|w| w.until))
            .min()
            .unwrap_or_else(|| gateway.clock.now().after(Duration::from_secs(3600)))
    }
    pub(crate) fn binary_call(
        &mut self,
        gateway: &Gateway,
        ctx: &AuthContext,
        bytes: &[u8],
    ) -> std::result::Result<Option<Value>, ()> {
        if bytes.len() < 4 {
            return Err(());
        }
        let stream = u32::from_be_bytes(bytes[..4].try_into().unwrap());
        let Some(s) = self.streams.get(&stream) else {
            if stream > 0 && stream < self.next_id {
                return Ok(Some(write_error(
                    Value::Null,
                    Some(stream),
                    "stream-closed",
                )));
            }
            return Err(());
        };
        let scope = s.write.as_ref().ok_or(())?.audit.scope;
        let method = if scope == Scope::Full {
            "full.input"
        } else {
            "terminal.input"
        };
        let result = self.write_call(
            gateway,
            ctx,
            Value::Null,
            method,
            json!({"stream":stream,"bytesB64":B64.encode(&bytes[4..])}),
        )?;
        Ok(result.map(|mut v| {
            v["stream"] = json!(stream);
            v
        }))
    }
    pub(super) fn discard_write_stream(&mut self, stream: u32) {
        let mut retained = Vec::new();
        while let Ok(work) = self.writes.receiver.try_recv() {
            if work.stream == Some(stream) {
                self.writes.cancelled.push(
                    PendingWrite {
                        id: work.id,
                        stream: work.stream,
                        input: work.input,
                        started: work.started,
                        cancelled: work.cancelled,
                        audit: work.audit,
                    }
                    .cancelled(),
                );
            } else {
                retained.push(work);
            }
        }
        for work in retained {
            let _ = self.writes.sender.try_send(work);
        }
    }
    pub(crate) fn cancelled_writes(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.writes.cancelled)
    }
    pub(crate) fn cancel_writes(&mut self, reason: &'static str) -> Vec<Value> {
        self.life.alive.store(false, Ordering::SeqCst);
        for s in self.streams.values() {
            s.state.close(reason);
        }
        let mut replies = self.cancelled_writes();
        if self.writes.active {
            if let Some(input) = self.writes.active_write.take() {
                replies.push(input.cancelled());
            }
        }
        while let Ok(work) = self.writes.receiver.try_recv() {
            replies.push(
                PendingWrite {
                    id: work.id,
                    stream: work.stream,
                    input: work.input,
                    started: work.started,
                    cancelled: work.cancelled,
                    audit: work.audit,
                }
                .cancelled(),
            );
        }
        replies
    }
}

#[cfg(test)]
#[path = "write_bridge_tests.rs"]
mod tests;
