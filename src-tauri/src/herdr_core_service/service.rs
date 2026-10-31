//! Owns the one herdr-core connection of this run.
//!
//! * While the experimental switch is on, one controller thread per switch-on keeps a
//!   [`Supervisor`] and a [`Session`] going: probe, start the bundled core when there is none,
//!   connect, wait for the session to end, then `notify_lost` and begin again. Its progress is
//!   the status (`herdr-core:status`).
//! * Attach streams live in a registry on the installed connection. Output reaches the
//!   frontend through the attach's sink; everything the frontend sends back (input, resize,
//!   focus, facts, credit) goes through a per-attach lane thread, so the order of calls is the
//!   order of writes and a slow socket never blocks the IPC thread.
//! * Switched off (or exiting): no probe, no connect, no core start. Turning the switch off
//!   releases every viewer and closes the connection; it never stops the core. A start that is
//!   waiting for the core to answer is abandoned within one poll (the supervisor's cancel
//!   check reads the controller's stop signal), writing no start record. A `server start` that
//!   was already issued cannot be taken back: the core it brings up keeps running, unrecorded,
//!   and a later run finds it as an external core.
//! * A reloaded WebView leaves its viewers and event subscriptions behind and nothing tells
//!   the sinks: a Tauri `Channel::send` only fails once the WebView is gone, not when its
//!   page is replaced. [`HerdrCore::release_for_reload`], called when the main WebView starts
//!   loading a page, releases them.
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, Weak};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};
use yuzora_host::herdr_core::caps;
use yuzora_host::herdr_core::client::{
    Attach, AttachEnd, AttachRequest, AttachSink, ClientError, DisplayFacts, LayoutSnapshot,
    Session, SessionConfig, SessionObserver,
};
use yuzora_host::herdr_core::docs::Op;
use yuzora_host::herdr_core::events::Effect;
use yuzora_host::herdr_core::paths::{self, Endpoint, Env, Os, PathError};
use yuzora_host::herdr_core::startup::{
    CancelCheck, CliLauncher, Launcher, LiveProbe, Origin, Outcome, Ready, StartError, Supervisor,
    SupervisorConfig, SystemClock,
};

use super::binary;
use super::bundled_start::{self, ProcessLookup, SystemProcesses};
use super::error::CoreError;
use super::events::{needs_record, view_effects, AttachEndView, AttachMessage, CoreEvent};
use super::status::{CoreReason, CoreState, CoreStatus, Draft};

/// How long app exit may spend releasing viewers and closing the connection (HERDR's
/// `release_all_connectors` takes at most about 1.5 s as well).
pub const EXIT_RELEASE_DEADLINE: Duration = Duration::from_millis(1500);
/// An incompatible core is looked at again this often (the user may have updated it).
const WAIT_INCOMPATIBLE: Duration = Duration::from_secs(15);
const BACKOFF_MIN: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);
/// A connection that lasted this long was not a failure: the backoff starts over.
const STABLE_AFTER: Duration = Duration::from_secs(10);
/// Env every terminal is started with: the identity programs in it see (`terminal.run.env`).
/// Same as the HERDR path's native client (`herdr_native_client.rs`: `TERM_PROGRAM=yuzora`,
/// lower case), so a program sees one identity whichever runtime hosts it.
pub const TERMINAL_ENV: [(&str, &str); 3] = [
    ("TERM", "xterm-256color"),
    ("COLORTERM", "truecolor"),
    ("TERM_PROGRAM", "yuzora"),
];

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

// ---------------------------------------------------------------------------------- seams

/// Where things live; set once during Tauri setup.
#[derive(Debug, Clone, Default)]
pub struct Paths {
    /// Yuzora's own herdr-core files (the start records) under the app data dir.
    pub data_dir: PathBuf,
    /// Tauri's resource dir, where the bundled core is.
    pub resource_dir: Option<PathBuf>,
}

impl Paths {
    pub fn record_path(&self, session: &str) -> PathBuf {
        self.data_dir.join(format!("start-record-{session}.json"))
    }
}

pub type SupervisorFactory = Box<dyn Fn(SupervisorConfig, Arc<Signal>) -> Supervisor + Send + Sync>;
/// The endpoint of a session name.
pub type EndpointResolver = Box<dyn Fn(&str) -> Result<Endpoint, PathError> + Send + Sync>;

/// Everything the service takes from outside, so the tests need no Tauri app, no home
/// directory and no real core.
pub struct Deps {
    pub emit_status: Box<dyn Fn(&CoreStatus) + Send + Sync>,
    /// The endpoint of a session. Never reads `HC_*`.
    pub endpoint: EndpointResolver,
    pub bundled_binary: Box<dyn Fn(&Paths) -> PathBuf + Send + Sync>,
    pub supervisor: SupervisorFactory,
    pub processes: Box<dyn ProcessLookup>,
}

impl Deps {
    pub fn production(emit_status: impl Fn(&CoreStatus) + Send + Sync + 'static) -> Deps {
        Deps {
            emit_status: Box::new(emit_status),
            endpoint: Box::new(|session| {
                paths::resolve(Os::current(), &Env::from_process(), session)
            }),
            bundled_binary: Box::new(binary::bundled_binary),
            supervisor: Box::new(|config, signal| {
                Supervisor::with_parts(
                    config,
                    Box::new(LiveProbe),
                    Box::new(GuardedLauncher {
                        inner: CliLauncher::default(),
                        signal,
                    }),
                    Box::new(SystemClock),
                )
            }),
            processes: Box::new(SystemProcesses),
        }
    }
}

/// Starts the bundled core, but not after the switch went off (or the app began to exit), and
/// not when the binary is not there.
pub struct GuardedLauncher<L: Launcher> {
    pub inner: L,
    pub signal: Arc<Signal>,
}

impl<L: Launcher> GuardedLauncher<L> {
    fn check(&self, binary: &std::path::Path) -> Result<(), StartError> {
        if self.signal.stopped() {
            return Err(StartError::Spawn("herdr-core was switched off".into()));
        }
        if !binary.is_file() {
            return Err(StartError::Spawn(format!(
                "the bundled herdr-core is missing ({})",
                binary.display()
            )));
        }
        Ok(())
    }
}

impl<L: Launcher> Launcher for GuardedLauncher<L> {
    fn start(&self, binary: &std::path::Path, endpoint: &Endpoint) -> Result<(), StartError> {
        self.check(binary)?;
        self.inner.start(binary, endpoint)
    }

    fn restart(&self, binary: &std::path::Path, endpoint: &Endpoint) -> Result<(), StartError> {
        self.check(binary)?;
        self.inner.restart(binary, endpoint)
    }
}

/// Wakes a controller: the switch went off (`stop`), or its session ended.
#[derive(Default)]
pub struct Signal {
    state: Mutex<SignalState>,
    wake: Condvar,
}

#[derive(Default)]
struct SignalState {
    stop: bool,
    /// The highest epoch whose session has ended. Epochs only grow, so a session that ends
    /// late never looks like the end of a newer one.
    closed_through: u64,
}

impl Signal {
    pub fn new() -> Arc<Signal> {
        Arc::new(Signal::default())
    }

    pub fn stop(&self) {
        lock(&self.state).stop = true;
        self.wake.notify_all();
    }

    pub fn stopped(&self) -> bool {
        lock(&self.state).stop
    }

    pub(super) fn session_closed(&self, epoch: u64) {
        let mut state = lock(&self.state);
        state.closed_through = state.closed_through.max(epoch);
        drop(state);
        self.wake.notify_all();
    }

    /// Waits up to `duration`; true when stopped.
    pub fn sleep(&self, duration: Duration) -> bool {
        let deadline = Instant::now() + duration;
        let mut state = lock(&self.state);
        while !state.stop {
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            state = self
                .wake
                .wait_timeout(state, deadline - now)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        state.stop
    }

    /// Waits for the session of `epoch` to end or for a stop; true when stopped.
    pub(super) fn wait_session_end(&self, epoch: u64) -> bool {
        let mut state = lock(&self.state);
        while !state.stop && state.closed_through < epoch {
            state = self
                .wake
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
        state.stop
    }

    fn wait_stop(&self) {
        let mut state = lock(&self.state);
        while !state.stop {
            state = self
                .wake
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }
}

// ---------------------------------------------------------------------------------- state

/// Delivers an event to a subscriber; false when it can no longer be reached (its WebView is
/// destroyed; a reloaded page is not detected this way, see `release_for_reload`).
pub type EventSink = Arc<dyn Fn(&CoreEvent) -> bool + Send + Sync>;

/// One event subscription. Everything handed to its sink, the initial events of
/// `subscribe` and every broadcast alike, goes through `turn`, so they reach the sink one at
/// a time and the initial ones first.
struct Subscriber {
    sink: EventSink,
    turn: Mutex<()>,
}

impl Subscriber {
    fn deliver(&self, event: &CoreEvent) -> bool {
        let _turn = lock(&self.turn);
        (self.sink)(event)
    }
}

/// Something the frontend sends to one attach. They all go through the attach's lane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaneCommand {
    Input(Vec<u8>),
    Resize(u16, u16),
    Focus(bool),
    Facts(DisplayFacts),
    Credit(usize),
    /// Releases the viewer, after everything queued before it has been written.
    Detach,
}

/// What an attach's channel receives: raw output bytes, and control messages.
#[derive(Debug, Clone, PartialEq)]
pub enum AttachOut {
    Bytes(Vec<u8>),
    Message(AttachMessage),
}

/// Delivers to the frontend; false when it can no longer be reached (the WebView is destroyed;
/// a reloaded page is not detected this way, see `release_for_reload`).
pub type AttachSend = Arc<dyn Fn(AttachOut) -> bool + Send + Sync>;

struct AttachEntry {
    attach: Arc<Attach>,
    lane: Sender<LaneCommand>,
}

struct Connection {
    epoch: u64,
    session: Arc<Session>,
    attaches: HashMap<String, AttachEntry>,
}

struct Run {
    id: u64,
    signal: Arc<Signal>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Config {
    enabled: bool,
    /// The session name this build uses.
    session: String,
}

struct Inner {
    config: Config,
    /// The app is exiting: nothing starts again.
    shutdown: bool,
    run: Option<Run>,
    run_seq: u64,
    status: CoreStatus,
    status_seq: u64,
    /// The epoch of the connection last installed.
    epoch: u64,
    conn: Option<Connection>,
    /// The last layout the session delivered, with the epoch it belongs to.
    layout: Option<(u64, LayoutSnapshot)>,
    subscribers: HashMap<String, Arc<Subscriber>>,
    /// How many times the main WebView has started loading a page (`release_for_reload`). An
    /// attach or subscription is registered only if this is still what it was when the call
    /// began, else the page that made it is gone.
    page: u64,
}

impl Inner {
    fn build_status(&mut self, draft: Draft) -> CoreStatus {
        self.status_seq += 1;
        self.status = CoreStatus {
            seq: self.status_seq,
            enabled: self.config.enabled,
            session: self.config.session.clone(),
            state: draft.state,
            epoch: self.epoch,
            version: draft.version,
            source: draft.source,
            bundled_version: caps::BUNDLED_VERSION.to_string(),
            optional_missing: draft.optional_missing,
            reason: draft.reason,
            retry_in_ms: draft.retry_in_ms,
            socket: draft.socket,
            record_error: draft.record_error,
        };
        self.status.clone()
    }

    fn run_is(&self, run_id: u64) -> bool {
        !self.shutdown && self.run.as_ref().map(|run| run.id) == Some(run_id)
    }
}

struct Shared {
    deps: Deps,
    paths: Mutex<Paths>,
    inner: Mutex<Inner>,
    /// Attach and subscription ids.
    ids: AtomicU64,
    /// One per connection attempt.
    epochs: AtomicU64,
}

/// The attach registry, subscribers and connection of this run. Clone-cheap handle.
#[derive(Clone)]
pub struct HerdrCore {
    shared: Arc<Shared>,
}

/// What `herdr_core_attach` returns.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachInfo {
    pub attach_id: String,
    /// The core's terminal record.
    pub terminal: Value,
}

/// What `herdr_core_layout_apply` returns.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppliedLayout {
    pub version: u64,
    /// Read-apply-write rounds used (1: no conflict).
    pub attempts: usize,
    pub doc: yuzora_host::herdr_core::docs::LayoutDoc,
    /// Terminals whose leaves the operation removed; the caller decides whether to end them.
    pub removed_terminals: Vec<String>,
    /// Leaves the one-terminal-per-leaf rule left vacant.
    pub displaced: Vec<String>,
}

impl HerdrCore {
    pub fn new(deps: Deps) -> HerdrCore {
        let config = Config {
            enabled: false,
            session: "yuzora".into(),
        };
        let status = disabled_status(&config.session);
        HerdrCore {
            shared: Arc::new(Shared {
                deps,
                paths: Mutex::new(Paths::default()),
                inner: Mutex::new(Inner {
                    config,
                    shutdown: false,
                    run: None,
                    run_seq: 0,
                    status,
                    status_seq: 0,
                    epoch: 0,
                    conn: None,
                    layout: None,
                    subscribers: HashMap::new(),
                    page: 0,
                }),
                ids: AtomicU64::new(0),
                epochs: AtomicU64::new(0),
            }),
        }
    }

    /// Wire the data and resource directories once during Tauri setup, before
    /// [`HerdrCore::apply_config`].
    pub fn configure_paths(&self, data_dir: PathBuf, resource_dir: Option<PathBuf>) {
        *lock(&self.shared.paths) = Paths {
            data_dir,
            resource_dir,
        };
    }

    /// The status as last announced. Not called `status`: the host's spawn-lock scan reads
    /// every `.status()` call as `Command::status`.
    pub fn status_snapshot(&self) -> CoreStatus {
        lock(&self.shared.inner).status.clone()
    }

    /// Whether the data-plane commands may do anything.
    pub fn is_enabled(&self) -> bool {
        let inner = lock(&self.shared.inner);
        inner.config.enabled && !inner.shutdown
    }

    pub fn require_enabled(&self) -> Result<(), CoreError> {
        if self.is_enabled() {
            Ok(())
        } else {
            Err(CoreError::disabled())
        }
    }

    // ------------------------------------------------------------------ switch and session

    /// Applies the switch and session. Idempotent. Off (or another session): every viewer is
    /// released and the connection closed, the core keeps running. On: a controller starts.
    /// May block briefly (the viewers' CLOSE frames), so not for an async executor thread.
    pub fn apply_config(&self, enabled: bool, session: &str) {
        let (status, torn_down) = {
            let mut inner = lock(&self.shared.inner);
            let wanted = Config {
                enabled,
                session: session.to_string(),
            };
            let running = inner.run.is_some();
            if inner.shutdown || (inner.config == wanted && running == enabled) {
                return;
            }
            if let Some(run) = inner.run.take() {
                run.signal.stop();
            }
            let torn_down = inner.conn.take();
            inner.layout = None;
            inner.config = wanted;
            if enabled {
                inner.run_seq += 1;
                let run = Run {
                    id: inner.run_seq,
                    signal: Signal::new(),
                };
                let (run_id, signal) = (run.id, run.signal.clone());
                inner.run = Some(run);
                let shared = self.shared.clone();
                let spawned = thread::Builder::new()
                    .name("hc-controller".into())
                    .spawn(move || Controller::start(shared, run_id, signal));
                let draft = match spawned {
                    Ok(_) => Draft::new(CoreState::Connecting),
                    Err(e) => {
                        inner.run = None;
                        Draft::new(CoreState::StartFailed)
                            .with_reason(CoreReason::new("spawn", e.to_string()))
                    }
                };
                (inner.build_status(draft), torn_down)
            } else {
                inner.subscribers.clear();
                (
                    inner.build_status(Draft::new(CoreState::Disabled)),
                    torn_down,
                )
            }
        };
        if let Some(conn) = torn_down {
            teardown(conn);
        }
        (self.shared.deps.emit_status)(&status);
    }

    /// App exit: no new connection ever, every viewer released, the connection closed. Bounded
    /// by `deadline` and safe to call again (`ExitRequested` and `Exit` both arrive). The core
    /// is left running.
    pub fn release_all_within(&self, deadline: Duration) {
        let (run, conn) = {
            let mut inner = lock(&self.shared.inner);
            inner.shutdown = true;
            inner.subscribers.clear();
            inner.layout = None;
            (inner.run.take(), inner.conn.take())
        };
        if let Some(run) = run {
            run.signal.stop();
        }
        let Some(conn) = conn else { return };
        let (done, finished) = mpsc::channel();
        let spawned = thread::Builder::new()
            .name("hc-exit-release".into())
            .spawn(move || {
                teardown(conn);
                let _ = done.send(());
            });
        if spawned.is_ok() {
            // Past the deadline the release carries on in the background; exit does not wait.
            let _ = finished.recv_timeout(deadline);
        }
    }

    // ------------------------------------------------------------------ connection access

    fn live(&self) -> Result<(Arc<Session>, u64), CoreError> {
        let inner = lock(&self.shared.inner);
        if !inner.config.enabled || inner.shutdown {
            return Err(CoreError::disabled());
        }
        inner
            .conn
            .as_ref()
            .map(|conn| (conn.session.clone(), conn.epoch))
            .ok_or_else(CoreError::not_connected)
    }

    fn with_session<T>(
        &self,
        call: impl FnOnce(&Session) -> Result<T, ClientError>,
    ) -> Result<T, CoreError> {
        let (session, _) = self.live()?;
        call(&session).map_err(|e| CoreError::from_client(&e))
    }

    // ------------------------------------------------------------------ terminals

    pub fn terminal_list(&self, all: bool) -> Result<Value, CoreError> {
        self.with_session(|session| session.rpc("terminal.list", json!({ "all": all })))
    }

    /// Starts the user's shell in `cwd` with Yuzora's terminal identity.
    pub fn terminal_run(&self, cwd: Option<String>) -> Result<Value, CoreError> {
        let env: serde_json::Map<String, Value> = TERMINAL_ENV
            .iter()
            .map(|(name, value)| ((*name).to_string(), json!(value)))
            .collect();
        let mut params = json!({ "env": env });
        if let Some(cwd) = cwd.filter(|cwd| !cwd.is_empty()) {
            params["cwd"] = json!(cwd);
        }
        self.with_session(|session| session.rpc("terminal.run", params))
    }

    /// Ends the terminal's program and removes it (Q9: closing a tab or leaf). Closing one
    /// that is already gone is not an error.
    pub fn terminal_close(&self, terminal: &str) -> Result<(), CoreError> {
        if terminal.is_empty() {
            return Err(CoreError::invalid_input("no terminal named"));
        }
        let params = json!({ "terminal": terminal });
        let (session, _) = self.live()?;
        let killed = session.rpc("terminal.kill", params.clone());
        match &killed {
            Err(e) if CoreError::from_client(e).is_not_found() => return Ok(()),
            Err(e @ (ClientError::ConnectionLost | ClientError::Closed | ClientError::Timeout)) => {
                return Err(CoreError::from_client(e));
            }
            _ => {}
        }
        // `terminal.rm` is optional, and refuses a terminal that is still live: when the kill
        // failed for another reason, that is the error worth reporting.
        match session.rpc("terminal.rm", params) {
            Ok(_) | Err(ClientError::Unsupported(_)) => Ok(()),
            Err(e) if CoreError::from_client(&e).is_not_found() => Ok(()),
            Err(e) => Err(match killed {
                Err(kill_error) => CoreError::from_client(&kill_error),
                Ok(_) => CoreError::from_client(&e),
            }),
        }
    }

    // ------------------------------------------------------------------ layout

    pub fn layout_get(&self) -> Result<LayoutSnapshot, CoreError> {
        self.with_session(|session| session.read_layout())
    }

    pub fn layout_apply(&self, op: &Op) -> Result<AppliedLayout, CoreError> {
        let (session, _) = self.live()?;
        let updated = session
            .update_layout(op)
            .map_err(|e| CoreError::from_doc(&e))?;
        Ok(AppliedLayout {
            version: updated.version,
            attempts: updated.attempts,
            doc: updated.doc,
            removed_terminals: updated.applied.removed_terminals,
            displaced: updated.applied.displaced,
        })
    }

    // ------------------------------------------------------------------ attach

    /// Opens a viewer. Output goes to `send` as raw bytes; the end of the stream goes as an
    /// [`AttachMessage::Closed`] after the last byte.
    ///
    /// The viewer belongs to the page that is loaded when this is called: if the main WebView
    /// starts loading another one before the core has answered, the viewer is released again
    /// and this fails with `page_replaced`.
    pub fn attach(
        &self,
        request: &AttachRequest,
        send: AttachSend,
    ) -> Result<AttachInfo, CoreError> {
        let page = lock(&self.shared.inner).page;
        self.attach_from_page(page, request, send)
    }

    /// [`HerdrCore::attach`] for a call that began when the loaded page was number `page`.
    pub(super) fn attach_from_page(
        &self,
        page: u64,
        request: &AttachRequest,
        send: AttachSend,
    ) -> Result<AttachInfo, CoreError> {
        let (session, epoch) = self.live()?;
        let id = format!("a{}", self.shared.ids.fetch_add(1, Ordering::Relaxed) + 1);
        let ended = Arc::new(AtomicBool::new(false));
        let sink = Arc::new(ChannelSink {
            id: id.clone(),
            epoch,
            shared: Arc::downgrade(&self.shared),
            send,
            ended: ended.clone(),
            dead: AtomicBool::new(false),
        });
        let attach = Arc::new(
            session
                .attach(request, sink)
                .map_err(|e| CoreError::from_client(&e))?,
        );
        let terminal = attach.terminal().clone();
        let (lane, commands) = mpsc::channel();
        let worker = {
            let attach = attach.clone();
            thread::Builder::new()
                .name("hc-attach-lane".into())
                .spawn(move || run_lane(attach, commands))
        };
        if let Err(e) = worker {
            attach.detach();
            return Err(CoreError::internal(format!("attach lane: {e}")));
        }
        {
            let mut inner = lock(&self.shared.inner);
            let (shutdown, replaced) = (inner.shutdown, inner.page != page);
            match inner.conn.as_mut() {
                Some(conn) if conn.epoch == epoch && !shutdown && !replaced => {
                    conn.attaches
                        .insert(id.clone(), AttachEntry { attach, lane });
                }
                // The page that asked was replaced while the core was answering: its reload
                // released what it had registered, and this one was not registered yet. Nobody
                // is left to use it (its output would stall at the credit window), so it is
                // released like the one below.
                _ if replaced => return Err(CoreError::page_replaced()),
                // The connection went away while the core was answering: dropping the lane
                // sender ends its thread, which drops the attach and so detaches it.
                _ => return Err(CoreError::not_connected()),
            }
        }
        // The stream may have ended before it was registered; its end could not remove it.
        if ended.load(Ordering::SeqCst) {
            self.shared.remove_attach(epoch, &id);
        }
        Ok(AttachInfo {
            attach_id: id,
            terminal,
        })
    }

    /// Queues one command for the attach. Failures of the write itself end the connection
    /// and arrive as the attach's `closed` message.
    pub fn lane(&self, attach_id: &str, command: LaneCommand) -> Result<(), CoreError> {
        let lane = {
            let inner = lock(&self.shared.inner);
            if !inner.config.enabled || inner.shutdown {
                return Err(CoreError::disabled());
            }
            inner
                .conn
                .as_ref()
                .and_then(|conn| conn.attaches.get(attach_id))
                .map(|entry| entry.lane.clone())
        };
        lane.ok_or_else(|| CoreError::unknown_attach(attach_id))?
            .send(command)
            .map_err(|_| CoreError::unknown_attach(attach_id))
    }

    /// Releases one viewer; the terminal keeps running. Releasing one that is already gone is
    /// not an error. The id is gone at once; the CLOSE is written by the viewer's lane, after
    /// everything the frontend queued before this, and this call never touches the socket.
    pub fn detach(&self, attach_id: &str) -> Result<(), CoreError> {
        self.require_enabled()?;
        let entry = {
            let mut inner = lock(&self.shared.inner);
            inner
                .conn
                .as_mut()
                .and_then(|conn| conn.attaches.remove(attach_id))
        };
        if let Some(entry) = entry {
            // The entry's end of the queue closes when it drops, which ends the lane once it
            // has worked through what is queued, the `Detach` last.
            let _ = entry.lane.send(LaneCommand::Detach);
        }
        Ok(())
    }

    /// Frontend start-up: whatever a WebView that has since reloaded left behind (viewers and
    /// event subscriptions) is released. The connection and the terminals stay.
    pub fn release_all(&self) -> Result<(), CoreError> {
        self.require_enabled()?;
        for entry in &self.take_viewers() {
            entry.attach.detach();
        }
        Ok(())
    }

    /// The main WebView began to load a page, so the viewers and subscriptions of the page it
    /// replaces are unreachable. They leave the registry now (so nothing the new page creates
    /// is caught up in it); their CLOSE frames are written on another thread, because this is
    /// called on the event loop and a stalled socket must not stall the window.
    ///
    /// It also starts a new page: an attach or subscription that began before this and is
    /// registered after it belongs to the page that is gone, and releases itself (see
    /// [`HerdrCore::attach`]). Counting and draining happen under one lock, so each is either
    /// drained here or sees the new page when it registers.
    pub fn release_for_reload(&self) {
        let entries = {
            let mut inner = lock(&self.shared.inner);
            inner.page += 1;
            drain_viewers(&mut inner)
        };
        if entries.is_empty() {
            return;
        }
        // Without a thread the entries drop here, and each viewer is released by its lane.
        let _ = thread::Builder::new()
            .name("hc-reload-release".into())
            .spawn(move || {
                for entry in &entries {
                    entry.attach.detach();
                }
            });
    }

    /// Every viewer and subscription out of the registry. Memory only, no I/O.
    fn take_viewers(&self) -> Vec<AttachEntry> {
        drain_viewers(&mut lock(&self.shared.inner))
    }

    /// Test seam: the registered attaches and subscriptions.
    #[cfg(test)]
    pub(super) fn registered(&self) -> (usize, usize) {
        let inner = lock(&self.shared.inner);
        (
            inner.conn.as_ref().map_or(0, |conn| conn.attaches.len()),
            inner.subscribers.len(),
        )
    }

    // ------------------------------------------------------------------ events

    /// Starts an event subscription: the current snapshot and layout first, then everything
    /// that follows (also across reconnects, until the switch goes off or it is released).
    ///
    /// The subscriber's turn is taken before it is visible to broadcasts and kept until the
    /// initial events are out, so a broadcast that comes in meanwhile (a newer layout, say)
    /// waits and arrives after them. It may repeat what the initial events already carried:
    /// the frontend drops a layout whose version is not greater than the one it knows.
    ///
    /// Like an attach, it belongs to the page that is loaded when this is called, and fails
    /// with `page_replaced` (registering nothing, telling the sink nothing) if another page
    /// has started loading by the time it would be registered.
    pub fn subscribe(&self, sink: EventSink) -> Result<String, CoreError> {
        let page = lock(&self.shared.inner).page;
        self.subscribe_from_page(page, sink)
    }

    /// [`HerdrCore::subscribe`] for a call that began when the loaded page was number `page`.
    pub(super) fn subscribe_from_page(
        &self,
        page: u64,
        sink: EventSink,
    ) -> Result<String, CoreError> {
        let id = format!("s{}", self.shared.ids.fetch_add(1, Ordering::Relaxed) + 1);
        let subscriber = Arc::new(Subscriber {
            sink,
            turn: Mutex::new(()),
        });
        let turn = lock(&subscriber.turn);
        let (session, epoch, layout) = {
            let mut inner = lock(&self.shared.inner);
            if !inner.config.enabled || inner.shutdown {
                return Err(CoreError::disabled());
            }
            if inner.page != page {
                return Err(CoreError::page_replaced());
            }
            inner.subscribers.insert(id.clone(), subscriber.clone());
            let layout = inner
                .layout
                .as_ref()
                .filter(|(epoch, _)| Some(*epoch) == inner.conn.as_ref().map(|c| c.epoch))
                .map(|(_, layout)| layout.clone());
            match inner.conn.as_ref() {
                Some(conn) => (Some(conn.session.clone()), conn.epoch, layout),
                None => (None, inner.epoch, None),
            }
        };
        if let Some(session) = session {
            let initial = CoreEvent::Snapshot {
                epoch,
                terminals: session.terminals(),
            };
            let mut alive = (subscriber.sink)(&initial);
            if let (true, Some(layout)) = (alive, layout) {
                alive = (subscriber.sink)(&CoreEvent::layout(epoch, &layout));
            }
            if !alive {
                lock(&self.shared.inner).subscribers.remove(&id);
            }
        }
        drop(turn);
        Ok(id)
    }

    pub fn unsubscribe(&self, id: &str) -> Result<(), CoreError> {
        self.require_enabled()?;
        match lock(&self.shared.inner).subscribers.remove(id) {
            Some(_) => Ok(()),
            None => Err(CoreError::unknown_subscription(id)),
        }
    }
}

fn drain_viewers(inner: &mut Inner) -> Vec<AttachEntry> {
    inner.subscribers.clear();
    inner
        .conn
        .as_mut()
        .map(|conn| conn.attaches.drain().map(|(_, entry)| entry).collect())
        .unwrap_or_default()
}

fn disabled_status(session: &str) -> CoreStatus {
    CoreStatus {
        seq: 0,
        enabled: false,
        session: session.to_string(),
        state: CoreState::Disabled,
        epoch: 0,
        version: None,
        source: None,
        bundled_version: caps::BUNDLED_VERSION.to_string(),
        optional_missing: Vec::new(),
        reason: None,
        retry_in_ms: None,
        socket: None,
        record_error: None,
    }
}

/// Releases every viewer, then closes the session. Blocks only as long as the writes do.
fn teardown(conn: Connection) {
    for entry in conn.attaches.values() {
        entry.attach.detach();
    }
    conn.session.close();
}

impl Shared {
    /// Updates and announces the status, unless `run_id` is no longer the current run.
    fn publish(&self, run_id: u64, draft: Draft) {
        let status = {
            let mut inner = lock(&self.inner);
            if !inner.run_is(run_id) {
                return;
            }
            inner.build_status(draft)
        };
        (self.deps.emit_status)(&status);
    }

    fn next_epoch(&self) -> u64 {
        self.epochs.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Makes the session the connection; false when the run is over (the caller closes it).
    fn install(&self, run_id: u64, epoch: u64, session: &Arc<Session>, draft: Draft) -> bool {
        let status = {
            let mut inner = lock(&self.inner);
            if !inner.run_is(run_id) {
                return false;
            }
            inner.conn = Some(Connection {
                epoch,
                session: session.clone(),
                attaches: HashMap::new(),
            });
            inner.epoch = epoch;
            inner.build_status(draft)
        };
        (self.deps.emit_status)(&status);
        self.broadcast(&CoreEvent::Snapshot {
            epoch,
            terminals: session.terminals(),
        });
        true
    }

    fn uninstall(&self, epoch: u64) -> Option<Connection> {
        let mut inner = lock(&self.inner);
        if inner.conn.as_ref().map(|conn| conn.epoch) != Some(epoch) {
            return None;
        }
        inner.layout = None;
        inner.conn.take()
    }

    fn broadcast(&self, event: &CoreEvent) {
        let subscribers: Vec<(String, Arc<Subscriber>)> = lock(&self.inner)
            .subscribers
            .iter()
            .map(|(id, subscriber)| (id.clone(), subscriber.clone()))
            .collect();
        let dead: Vec<String> = subscribers
            .into_iter()
            .filter(|(_, subscriber)| !subscriber.deliver(event))
            .map(|(id, _)| id)
            .collect();
        if !dead.is_empty() {
            let mut inner = lock(&self.inner);
            for id in dead {
                inner.subscribers.remove(&id);
            }
        }
    }

    fn session_of(&self, epoch: u64) -> Option<Arc<Session>> {
        lock(&self.inner)
            .conn
            .as_ref()
            .filter(|conn| conn.epoch == epoch)
            .map(|conn| conn.session.clone())
    }

    fn remove_attach(&self, epoch: u64, id: &str) {
        let entry = lock(&self.inner)
            .conn
            .as_mut()
            .filter(|conn| conn.epoch == epoch)
            .and_then(|conn| conn.attaches.remove(id));
        // Dropping the entry drops the lane sender; the lane's last reference then detaches.
        drop(entry);
    }
}

// ---------------------------------------------------------------------------------- attach

/// Runs the frontend's commands for one attach, in order, until the attach is released.
fn run_lane(attach: Arc<Attach>, commands: Receiver<LaneCommand>) {
    for command in commands {
        // A failed write has already ended the connection; the sink reports it.
        let _ = match command {
            LaneCommand::Input(bytes) => attach.input(&bytes),
            LaneCommand::Resize(cols, rows) => attach.resize(cols, rows),
            LaneCommand::Focus(focused) => attach.focus(focused),
            LaneCommand::Facts(facts) => attach.facts(&facts),
            LaneCommand::Credit(bytes) => {
                attach.credit(bytes);
                Ok(())
            }
            LaneCommand::Detach => {
                attach.detach();
                Ok(())
            }
        };
    }
    // The last reference goes with this thread: the viewer is released.
}

struct ChannelSink {
    id: String,
    epoch: u64,
    shared: Weak<Shared>,
    send: AttachSend,
    /// Set when the stream's end was reported or the frontend was lost; read by `attach` to
    /// catch a stream that was over before it was registered.
    ended: Arc<AtomicBool>,
    /// The frontend can no longer be reached.
    dead: AtomicBool,
}

impl ChannelSink {
    fn forget(&self) {
        if let Some(shared) = self.shared.upgrade() {
            shared.remove_attach(self.epoch, &self.id);
        }
    }
}

impl AttachSink for ChannelSink {
    fn output(&self, bytes: Vec<u8>) {
        if self.dead.load(Ordering::Acquire) {
            return;
        }
        if !(self.send)(AttachOut::Bytes(bytes)) {
            // The frontend cannot be reached (a Tauri channel fails once its WebView is gone;
            // a reload does not make it fail, see `release_for_reload`): release the viewer
            // rather than let the core fill the credit window for nobody. `ended` also covers
            // an `attach` that has not registered this stream yet.
            self.dead.store(true, Ordering::Release);
            self.ended.store(true, Ordering::SeqCst);
            self.forget();
        }
    }

    fn closed(&self, end: AttachEnd) {
        self.ended.store(true, Ordering::SeqCst);
        if !self.dead.load(Ordering::Acquire) {
            let _ = (self.send)(AttachOut::Message(AttachMessage::Closed {
                end: AttachEndView::from_end(&end),
            }));
        }
        self.forget();
    }
}

// ---------------------------------------------------------------------------------- session

/// Tells the subscribers what the session reports and wakes the controller when it ends.
struct Observer {
    shared: Arc<Shared>,
    epoch: u64,
    signal: Arc<Signal>,
}

impl SessionObserver for Observer {
    fn on_effects(&self, effects: &[Effect]) {
        let session = self.shared.session_of(self.epoch);
        let records = session
            .as_ref()
            .filter(|_| effects.iter().any(needs_record))
            .map(|session| session.terminals());
        self.shared.broadcast(&CoreEvent::Effects {
            epoch: self.epoch,
            effects: view_effects(effects, records.as_deref()),
        });
        if effects.iter().any(|e| matches!(e, Effect::Resnapshot)) {
            // Not installed yet: the install sends a snapshot read after this one.
            if let Some(session) = session {
                self.shared.broadcast(&CoreEvent::Snapshot {
                    epoch: self.epoch,
                    terminals: session.terminals(),
                });
            }
        }
    }

    fn on_layout(&self, layout: &LayoutSnapshot) {
        {
            let mut inner = lock(&self.shared.inner);
            if inner
                .layout
                .as_ref()
                .is_none_or(|(epoch, _)| *epoch <= self.epoch)
            {
                inner.layout = Some((self.epoch, layout.clone()));
            }
        }
        self.shared
            .broadcast(&CoreEvent::layout(self.epoch, layout));
    }

    fn on_closed(&self, _cause: &ClientError) {
        self.signal.session_closed(self.epoch);
    }
}

// ---------------------------------------------------------------------------------- controller

pub(super) struct Backoff {
    next: Duration,
}

impl Backoff {
    pub(super) fn new() -> Backoff {
        Backoff { next: BACKOFF_MIN }
    }

    pub(super) fn take(&mut self) -> Duration {
        let wait = self.next;
        self.next = (self.next * 2).min(BACKOFF_MAX);
        wait
    }

    pub(super) fn reset(&mut self) {
        self.next = BACKOFF_MIN;
    }
}

/// The thread of one switch-on: finds or starts a core, connects, waits, begins again.
struct Controller {
    shared: Arc<Shared>,
    run_id: u64,
    signal: Arc<Signal>,
    endpoint: Endpoint,
    record_path: PathBuf,
    supervisor: Supervisor,
    backoff: Backoff,
    /// Why the last connection ended; shown while reconnecting.
    lost: Option<CoreReason>,
}

impl Controller {
    fn start(shared: Arc<Shared>, run_id: u64, signal: Arc<Signal>) {
        let session = lock(&shared.inner).config.session.clone();
        let endpoint = match (shared.deps.endpoint)(&session) {
            Ok(endpoint) => endpoint,
            Err(e) => {
                // No retry can fix a name or a path: report once and wait to be stopped.
                shared.publish(
                    run_id,
                    Draft::new(CoreState::StartFailed)
                        .with_reason(CoreReason::new(e.code(), e.to_string())),
                );
                signal.wait_stop();
                return;
            }
        };
        let paths = lock(&shared.paths).clone();
        let mut config =
            SupervisorConfig::new((shared.deps.bundled_binary)(&paths), endpoint.clone());
        let record_path = paths.record_path(&session);
        config.record_path = Some(record_path.clone());
        // A start that is waiting for the core to answer ends within one poll of the switch
        // going off. The `server start` already issued is not taken back.
        config.cancel = Some(CancelCheck::new({
            let signal = signal.clone();
            move || signal.stopped()
        }));
        let supervisor = (shared.deps.supervisor)(config, signal.clone());
        Controller {
            shared,
            run_id,
            signal,
            endpoint,
            record_path,
            supervisor,
            backoff: Backoff::new(),
            lost: None,
        }
        .run();
    }

    fn draft(&self, state: CoreState) -> Draft {
        Draft::new(state).with_socket(&self.endpoint.socket)
    }

    fn run(mut self) {
        loop {
            if self.signal.stopped() {
                return;
            }
            let mut connecting = self.draft(CoreState::Connecting);
            if let Some(reason) = self.lost.take() {
                connecting = connecting.with_reason(reason);
            }
            self.shared.publish(self.run_id, connecting);
            let outcome = self.supervisor.ensure();
            if self.signal.stopped() {
                return;
            }
            let Some(wait) = self.handle(outcome) else {
                return;
            };
            if self.signal.sleep(wait) {
                return;
            }
        }
    }

    /// How long to wait before looking again; `None` when stopped meanwhile.
    fn handle(&mut self, outcome: Outcome) -> Option<Duration> {
        match outcome {
            Outcome::Ready(ready) => self.serve(&ready),
            Outcome::Incompatible { version, reason } => {
                self.shared.publish(
                    self.run_id,
                    self.draft(CoreState::Incompatible)
                        .with_version(version)
                        .with_reason(CoreReason::from_refusal(&reason)),
                );
                Some(WAIT_INCOMPATIBLE)
            }
            Outcome::Waiting { retry_in } => {
                self.shared.publish(
                    self.run_id,
                    self.draft(CoreState::Waiting).with_retry(retry_in),
                );
                Some(retry_in)
            }
            Outcome::StartFailed(error) => {
                self.shared.publish(
                    self.run_id,
                    self.draft(CoreState::StartFailed)
                        .with_reason(CoreReason::from_start_error(&error)),
                );
                Some(self.backoff.take())
            }
            Outcome::Unreachable(failure) => {
                self.shared.publish(
                    self.run_id,
                    self.draft(CoreState::Unreachable)
                        .with_reason(CoreReason::new(failure.code, failure.message)),
                );
                Some(self.backoff.take())
            }
            // The cancel check is this run's stop signal: the owner is stopping and `run`
            // returns. Nothing is published; the run is over.
            Outcome::Cancelled => None,
        }
    }

    /// Connects to the core the supervisor found, serves until the session ends.
    fn serve(&mut self, ready: &Ready) -> Option<Duration> {
        let epoch = self.shared.next_epoch();
        let observer = Arc::new(Observer {
            shared: self.shared.clone(),
            epoch,
            signal: self.signal.clone(),
        });
        let session = match Session::connect(&self.endpoint, SessionConfig::default(), observer) {
            Ok(session) => Arc::new(session),
            Err(ClientError::Incompatible(refusal)) => {
                self.shared.publish(
                    self.run_id,
                    self.draft(CoreState::Incompatible)
                        .with_reason(CoreReason::from_refusal(&refusal)),
                );
                return Some(WAIT_INCOMPATIBLE);
            }
            Err(e) => {
                self.shared.publish(
                    self.run_id,
                    self.draft(CoreState::Unreachable)
                        .with_reason(CoreReason::from_client_error(&e)),
                );
                return Some(self.backoff.take());
            }
        };
        let bundled_earlier =
            matches!(ready.origin, Origin::External) && self.started_by_an_earlier_run(&session);
        let draft = Draft::ready(ready, bundled_earlier).with_socket(&self.endpoint.socket);
        let began = Instant::now();
        if !self.shared.install(self.run_id, epoch, &session, draft) {
            session.close();
            return None;
        }
        let stopped = self.signal.wait_session_end(epoch);
        if let Some(conn) = self.shared.uninstall(epoch) {
            teardown(conn);
        }
        if stopped {
            // Whoever stopped the run took the connection and releases its viewers before
            // closing the session; closing it here first would skip their CLOSE frames.
            return None;
        }
        session.close();
        self.supervisor.notify_lost();
        self.lost = session
            .closed()
            .map(|cause| CoreReason::from_client_error(&cause));
        Some(if began.elapsed() >= STABLE_AFTER {
            self.backoff.reset();
            BACKOFF_MIN
        } else {
            self.backoff.take()
        })
    }

    /// K9: a core this supervisor found already running may still be one an earlier Yuzora
    /// run started from the bundled binary.
    fn started_by_an_earlier_run(&self, session: &Session) -> bool {
        let live_pid = session
            .rpc("server.status", json!({}))
            .ok()
            .and_then(|status| status.get("pid").and_then(Value::as_u64))
            .and_then(|pid| u32::try_from(pid).ok());
        bundled_start::judge_from_file(
            &self.record_path,
            &self.endpoint,
            live_pid,
            self.shared.deps.processes.as_ref(),
        )
        .is_bundled()
    }
}
