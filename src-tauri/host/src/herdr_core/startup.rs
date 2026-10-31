//! Starting, and only starting, the bundled core.
//!
//! * [`core_command`]: the one place that builds a `herdr-core` child. It clears every `HC_*`
//!   and `HERDR_*` variable the parent inherited and sets only `HC_SESSION` and `HC_SOCKET`.
//!   A core's terminals and its holder inherit the environment of the core's *first* start,
//!   and a stray `HC_SOCKET` or timing variable would follow them for good.
//! * [`Supervisor`]: when to start the bundled core (K1-K4, K3 as amended by P0):
//!   probe first and use a compatible core that is already there; report an incompatible one
//!   without touching it; with no core, start the bundled one at once, except after a core
//!   that had been reachable went away: then wait at least [`GRACE_AFTER_LOSS`] first.
//! * [`StartRecord`]: what a start leaves behind so a later run (K9, after an update) can tell
//!   a core Yuzora started from one it did not.
//!
//! * [`CancelCheck`]: how the owner of a [`Supervisor`] abandons a start that is waiting for
//!   the core to answer, when it stops meanwhile.
//!
//! Never run here: `update` (a bundled copy updating itself), and `server stop` or
//! `server restart` on a core Yuzora did not start.

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::caps::{self, Hello, Refusal, Relation, Verdict};
use super::client::{self, ClientError};
use super::paths::Endpoint;
use crate::herdr_transport::connect_local_stream;
use crate::process_kill;

/// After a core that had been reachable went away, wait this long for it (or another
/// compatible one) before starting the bundled core.
pub const GRACE_AFTER_LOSS: Duration = Duration::from_secs(10);
/// How long a freshly started core may take to answer.
pub const START_TIMEOUT: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(100);
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);
const CLI_TIMEOUT: Duration = Duration::from_secs(30);
/// A core counts as started by this call when it is not older than the call plus this.
const STARTED_BY_US_SLACK_MS: u128 = 2_000;

// ---------------------------------------------------------------------------------- environment

pub const ENV_SESSION: &str = "HC_SESSION";
pub const ENV_SOCKET: &str = "HC_SOCKET";
/// No name list: upstream adds tuning variables with every release (27 at 0.1.14).
const CLEARED_PREFIXES: &[&str] = &["HC_", "HERDR_"];

fn is_cleared(name: &OsStr) -> bool {
    let name = name.to_string_lossy();
    CLEARED_PREFIXES.iter().any(|prefix| {
        if cfg!(windows) {
            // Windows variable names are case-insensitive.
            name.get(..prefix.len())
                .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
        } else {
            name.starts_with(prefix)
        }
    })
}

/// What a `herdr-core` child's environment is changed by: the rest is inherited as it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvPlan {
    /// Inherited `HC_*` / `HERDR_*` variables to remove.
    pub remove: Vec<OsString>,
    pub session: String,
    pub socket: String,
}

/// `inherited`: the names of the variables the child would otherwise get.
pub fn env_plan(inherited: impl IntoIterator<Item = OsString>, endpoint: &Endpoint) -> EnvPlan {
    EnvPlan {
        remove: inherited
            .into_iter()
            .filter(|name| is_cleared(name))
            .filter(|name| name != ENV_SESSION && name != ENV_SOCKET)
            .collect(),
        session: endpoint.session.clone(),
        socket: endpoint.socket.to_string_lossy().into_owned(),
    }
}

pub fn apply_env_plan(command: &mut Command, plan: &EnvPlan) {
    for name in &plan.remove {
        command.env_remove(name);
    }
    command.env(ENV_SESSION, &plan.session);
    command.env(ENV_SOCKET, &plan.socket);
}

/// `herdr-core server start|status|restart` or `--version`: the only things Yuzora runs.
fn allowed(args: &[&str]) -> bool {
    matches!(
        args,
        ["--version" | "-V"] | ["server", "start" | "status" | "restart"]
    )
}

pub(super) fn build_command(
    binary: &Path,
    endpoint: &Endpoint,
    args: &[&str],
    inherited: impl IntoIterator<Item = OsString>,
) -> Command {
    let mut command = Command::new(binary);
    command.args(args);
    apply_env_plan(&mut command, &env_plan(inherited, endpoint));
    command
}

/// A `herdr-core` child pinned to `endpoint`, with the environment described in the module
/// docs. Every `herdr-core` process Yuzora runs, the CLI ones included, comes from here:
/// without the same `HC_SESSION` and `HC_SOCKET` the CLI looks at another socket than the
/// core listens on.
pub fn core_command(
    binary: &Path,
    endpoint: &Endpoint,
    args: &[&str],
) -> Result<Command, StartError> {
    if !allowed(args) {
        return Err(StartError::NotAllowed(args.join(" ")));
    }
    Ok(build_command(
        binary,
        endpoint,
        args,
        std::env::vars_os().map(|(name, _)| name),
    ))
}

// ---------------------------------------------------------------------------------- errors

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartError {
    /// The binary could not be run.
    Spawn(String),
    /// The command ran and failed.
    Exit(Option<i32>),
    /// The command did not return in time.
    Timeout,
    /// The command returned but no core answers.
    NoAnswer,
    /// `core_command` refuses this command line.
    NotAllowed(String),
    /// The core that answers is not the one Yuzora started.
    NotOurs,
}

impl StartError {
    pub fn code(&self) -> &'static str {
        match self {
            StartError::Spawn(_) => "spawn",
            StartError::Exit(_) => "exit",
            StartError::Timeout => "timeout",
            StartError::NoAnswer => "no_answer",
            StartError::NotAllowed(_) => "not_allowed",
            StartError::NotOurs => "not_ours",
        }
    }
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StartError::Spawn(m) => write!(f, "could not run herdr-core: {m}"),
            StartError::Exit(Some(code)) => write!(f, "herdr-core exited with {code}"),
            StartError::Exit(None) => write!(f, "herdr-core was terminated"),
            StartError::Timeout => write!(f, "herdr-core did not return in time"),
            StartError::NoAnswer => write!(f, "no core answered after starting"),
            StartError::NotAllowed(args) => write!(f, "herdr-core {args:?} is not allowed"),
            StartError::NotOurs => write!(f, "the running core was not started by Yuzora"),
        }
    }
}

impl std::error::Error for StartError {}

// ---------------------------------------------------------------------------------- record

/// Written when Yuzora starts a core. K9 matches it against the operating system's view of the
/// process (executable path, pid reuse) before restarting that core with a new binary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartRecord {
    /// Format version of this file.
    pub v: u32,
    /// The core's own pid, from `server.status`.
    pub pid: u32,
    /// When the start was requested (wall clock, ms since the Unix epoch).
    pub started_unix_ms: u64,
    /// The binary that was run, as it was passed (compare after canonicalizing both sides).
    pub exe: String,
    pub version: String,
    pub session: String,
    pub socket: String,
}

pub const RECORD_VERSION: u32 = 1;

impl StartRecord {
    pub fn exe_path(&self) -> PathBuf {
        PathBuf::from(&self.exe)
    }

    /// `Ok(None)` when there is no record. A record this build cannot read is an error:
    /// the caller must not guess which core is Yuzora's.
    pub fn read(path: &Path) -> io::Result<Option<StartRecord>> {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        let record: StartRecord = serde_json::from_slice(&bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        if record.v != RECORD_VERSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("start record version {}", record.v),
            ));
        }
        Ok(Some(record))
    }

    /// Atomically replaces the file.
    pub fn write(&self, path: &Path) -> io::Result<()> {
        use std::io::Write as _;
        let dir = path.parent().filter(|p| !p.as_os_str().is_empty());
        let dir = dir.unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(dir)?;
        let mut file = tempfile::NamedTempFile::new_in(dir)?;
        file.write_all(&serde_json::to_vec_pretty(self).map_err(io::Error::other)?)?;
        file.as_file().sync_all()?;
        file.persist(path).map_err(|e| e.error)?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------------- seams

/// Time, injectable so the grace period and start timeout can be tested without waiting.
pub trait Clock: Send + Sync {
    fn now(&self) -> Instant;
    fn unix_ms(&self) -> u64;
    fn sleep(&self, duration: Duration);
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }

    fn unix_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64)
    }

    fn sleep(&self, duration: Duration) {
        thread::sleep(duration);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreInfo {
    pub hello: Hello,
    /// From `server.status`.
    pub pid: Option<u32>,
    pub uptime_ms: Option<u64>,
}

/// Why something at the endpoint could not be talked to: a stable `code` for the UI layer (the
/// [`ClientError::code`] of the failed step: `io`, `timeout`, `connection_lost`, `protocol`,
/// ...) and the English text for logs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeFailure {
    pub code: &'static str,
    pub message: String,
}

impl ProbeFailure {
    pub fn new(code: &'static str, message: impl Into<String>) -> ProbeFailure {
        ProbeFailure {
            code,
            message: message.into(),
        }
    }
}

impl From<ClientError> for ProbeFailure {
    fn from(error: ClientError) -> ProbeFailure {
        ProbeFailure::new(error.code(), error.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeResult {
    /// Nothing listens at the endpoint.
    NoCore,
    Core(CoreInfo),
    /// Something is there but did not answer sensibly. Never a reason to start a core.
    Failed(ProbeFailure),
}

pub trait Probe: Send + Sync {
    fn probe(&self, endpoint: &Endpoint) -> ProbeResult;
}

/// Connects, says `hello`, asks `server.status`, hangs up.
pub struct LiveProbe;

impl Probe for LiveProbe {
    fn probe(&self, endpoint: &Endpoint) -> ProbeResult {
        let deadline = Instant::now() + PROBE_TIMEOUT;
        let stream = match connect_local_stream(&endpoint.transport_name(), deadline) {
            Ok(stream) => stream,
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
                ) =>
            {
                return ProbeResult::NoCore;
            }
            Err(e) => return ProbeResult::Failed(ClientError::from(e).into()),
        };
        let mut pending = Vec::new();
        let hello = match client::hello_call(&stream, &mut pending, false, deadline) {
            Ok(hello) => hello,
            Err(e) => return ProbeResult::Failed(e.into()),
        };
        let status = hello
            .has("server.status")
            .then(|| {
                client::blocking_call(&stream, &mut pending, "server.status", json!({}), deadline)
            })
            .and_then(Result::ok)
            .unwrap_or(Value::Null);
        ProbeResult::Core(CoreInfo {
            pid: status
                .get("pid")
                .and_then(Value::as_u64)
                .and_then(|p| u32::try_from(p).ok()),
            uptime_ms: status.get("uptime_ms").and_then(Value::as_u64),
            hello,
        })
    }
}

/// Runs `herdr-core server start|restart`.
pub trait Launcher: Send + Sync {
    fn start(&self, binary: &Path, endpoint: &Endpoint) -> Result<(), StartError>;
    fn restart(&self, binary: &Path, endpoint: &Endpoint) -> Result<(), StartError>;
}

pub struct CliLauncher {
    pub timeout: Duration,
}

impl Default for CliLauncher {
    fn default() -> CliLauncher {
        CliLauncher {
            timeout: CLI_TIMEOUT,
        }
    }
}

impl Launcher for CliLauncher {
    fn start(&self, binary: &Path, endpoint: &Endpoint) -> Result<(), StartError> {
        run_command(
            core_command(binary, endpoint, &["server", "start"])?,
            self.timeout,
        )
    }

    fn restart(&self, binary: &Path, endpoint: &Endpoint) -> Result<(), StartError> {
        run_command(
            core_command(binary, endpoint, &["server", "restart"])?,
            self.timeout,
        )
    }
}

/// Runs a CLI command that leaves a daemon behind: no pipes (a daemon would hold them open),
/// its own process group, and a deadline.
pub(super) fn run_command(mut command: Command, timeout: Duration) -> Result<(), StartError> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    process_kill::configure_background_process(&mut command);
    let mut child =
        process_kill::spawn_std(&mut command).map_err(|e| StartError::Spawn(e.to_string()))?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => return Err(StartError::Exit(status.code())),
            Ok(None) => {}
            Err(e) => {
                let _ = process_kill::terminate_direct_child_and_reap(&mut child);
                return Err(StartError::Spawn(e.to_string()));
            }
        }
        if Instant::now() >= deadline {
            let _ = process_kill::terminate_direct_child_and_reap(&mut child);
            return Err(StartError::Timeout);
        }
        thread::sleep(Duration::from_millis(20));
    }
}

// ---------------------------------------------------------------------------------- supervisor

#[derive(Debug, Clone)]
pub struct SupervisorConfig {
    /// The bundled core.
    pub binary: PathBuf,
    pub endpoint: Endpoint,
    pub bundled_version: String,
    /// Where to persist the [`StartRecord`], if anywhere.
    pub record_path: Option<PathBuf>,
    pub grace_after_loss: Duration,
    pub start_timeout: Duration,
    /// Asked before each step of a start and between probes while waiting for the core: when
    /// it says `true` the start is abandoned with [`Outcome::Cancelled`].
    pub cancel: Option<CancelCheck>,
}

/// Tells a start in progress that its owner no longer wants it. Called on the thread running
/// the [`Supervisor`], about every [`POLL`] while it waits: keep it a flag read.
#[derive(Clone)]
pub struct CancelCheck(Arc<dyn Fn() -> bool + Send + Sync>);

impl CancelCheck {
    pub fn new(check: impl Fn() -> bool + Send + Sync + 'static) -> CancelCheck {
        CancelCheck(Arc::new(check))
    }

    fn is_cancelled(&self) -> bool {
        (self.0)()
    }
}

impl std::fmt::Debug for CancelCheck {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CancelCheck(..)")
    }
}

impl SupervisorConfig {
    pub fn new(binary: PathBuf, endpoint: Endpoint) -> SupervisorConfig {
        SupervisorConfig {
            binary,
            endpoint,
            bundled_version: caps::BUNDLED_VERSION.to_string(),
            record_path: None,
            grace_after_loss: GRACE_AFTER_LOSS,
            start_timeout: START_TIMEOUT,
            cancel: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// A core that was already running: left alone.
    External,
    /// Started by this supervisor from the bundled binary.
    StartedByYuzora(StartRecord),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ready {
    pub hello: Hello,
    pub relation: Relation,
    /// Methods the UI must hide because this core lacks them.
    pub optional_missing: Vec<&'static str>,
    pub origin: Origin,
    /// Why the start record could not be written, if it could not.
    pub record_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Connect a [`super::client::Session`].
    Ready(Box<Ready>),
    /// A core answers but is not used. Nothing was restarted or replaced.
    Incompatible {
        version: String,
        reason: Refusal,
    },
    /// No core, and the grace period after losing one is still running: ask again then.
    Waiting {
        retry_in: Duration,
    },
    StartFailed(StartError),
    /// Something at the endpoint did not answer sensibly. Nothing was started.
    Unreachable(ProbeFailure),
    /// The [`SupervisorConfig::cancel`] check fired. No start record was written, and nothing
    /// else changed. A `server start` or `server restart` that had already been issued cannot
    /// be taken back: the core it brings up keeps running, with no record, so a later run
    /// finds it at the endpoint as an external core (K9 will not restart it).
    Cancelled,
}

struct Spawn {
    began: Instant,
    unix_ms: u64,
    binary: PathBuf,
}

/// Owned by one thread: `ensure` and `restart_with` take `&mut self`.
pub struct Supervisor {
    cfg: SupervisorConfig,
    probe: Box<dyn Probe>,
    launcher: Box<dyn Launcher>,
    clock: Box<dyn Clock>,
    /// A compatible core has been reachable in this run.
    ever_ready: bool,
    /// When the core was first noticed missing after that.
    lost_since: Option<Instant>,
    started: Option<StartRecord>,
}

impl Supervisor {
    pub fn new(cfg: SupervisorConfig) -> Supervisor {
        Supervisor::with_parts(
            cfg,
            Box::new(LiveProbe),
            Box::new(CliLauncher::default()),
            Box::new(SystemClock),
        )
    }

    pub fn with_parts(
        cfg: SupervisorConfig,
        probe: Box<dyn Probe>,
        launcher: Box<dyn Launcher>,
        clock: Box<dyn Clock>,
    ) -> Supervisor {
        Supervisor {
            cfg,
            probe,
            launcher,
            clock,
            ever_ready: false,
            lost_since: None,
            started: None,
        }
    }

    /// The core this supervisor started in this run, if it is still the one answering.
    pub fn started(&self) -> Option<&StartRecord> {
        self.started.as_ref()
    }

    /// Call when a session that was running ended, so the grace period counts from then
    /// rather than from the next `ensure`.
    pub fn notify_lost(&mut self) {
        if self.ever_ready && self.lost_since.is_none() {
            self.lost_since = Some(self.clock.now());
        }
    }

    fn cancelled(&self) -> bool {
        self.cfg
            .cancel
            .as_ref()
            .is_some_and(CancelCheck::is_cancelled)
    }

    /// Probes and decides. Returns at once except right after starting a core, when it waits
    /// (bounded by `start_timeout`) for the core to answer; that wait ends within one poll of
    /// the cancel check firing, with [`Outcome::Cancelled`].
    pub fn ensure(&mut self) -> Outcome {
        if self.cancelled() {
            return Outcome::Cancelled;
        }
        match self.probe.probe(&self.cfg.endpoint) {
            ProbeResult::Core(info) => self.adopt(info, None),
            ProbeResult::Failed(failure) => Outcome::Unreachable(failure),
            ProbeResult::NoCore => {
                if self.ever_ready {
                    let now = self.clock.now();
                    let since = *self.lost_since.get_or_insert(now);
                    let waited = now.saturating_duration_since(since);
                    if waited < self.cfg.grace_after_loss {
                        return Outcome::Waiting {
                            retry_in: self.cfg.grace_after_loss - waited,
                        };
                    }
                }
                self.start_core()
            }
        }
    }

    /// K9: restart the core Yuzora started, with a new binary (`new_binary server restart`),
    /// after an update. `record` is the persisted start record, already matched against the
    /// operating system's view of the process; this checks the other half: the core answering
    /// at the endpoint reports that pid. Any other core is `StartFailed(NotOurs)`, untouched.
    /// No grace period: this is Yuzora's own restart. Call from the thread that owns the
    /// supervisor. Cancelled like [`Supervisor::ensure`]; a restart already issued is not
    /// taken back, and the start record then keeps describing the core it was written for.
    pub fn restart_with(&mut self, record: &StartRecord, new_binary: &Path) -> Outcome {
        if self.cancelled() {
            return Outcome::Cancelled;
        }
        let ours = matches!(
            self.probe.probe(&self.cfg.endpoint),
            ProbeResult::Core(info)
                if info.pid == Some(record.pid) && record.session == self.cfg.endpoint.session
        );
        if !ours {
            return Outcome::StartFailed(StartError::NotOurs);
        }
        if self.cancelled() {
            return Outcome::Cancelled;
        }
        let spawn = self.begin(new_binary);
        if let Err(e) = self.launcher.restart(new_binary, &self.cfg.endpoint) {
            return Outcome::StartFailed(e);
        }
        self.await_core(spawn, Some(record.pid))
    }

    fn begin(&self, binary: &Path) -> Spawn {
        Spawn {
            began: self.clock.now(),
            unix_ms: self.clock.unix_ms(),
            binary: binary.to_path_buf(),
        }
    }

    fn start_core(&mut self) -> Outcome {
        // The probe before this can take seconds: ask again before the step that cannot be
        // taken back.
        if self.cancelled() {
            return Outcome::Cancelled;
        }
        let binary = self.cfg.binary.clone();
        let spawn = self.begin(&binary);
        if let Err(e) = self.launcher.start(&binary, &self.cfg.endpoint) {
            return Outcome::StartFailed(e);
        }
        self.await_core(spawn, None)
    }

    /// Polls until a core answers (other than `not_pid`, the one being replaced).
    fn await_core(&mut self, spawn: Spawn, not_pid: Option<u32>) -> Outcome {
        let deadline = spawn.began + self.cfg.start_timeout;
        loop {
            if self.cancelled() {
                return Outcome::Cancelled;
            }
            if let ProbeResult::Core(info) = self.probe.probe(&self.cfg.endpoint) {
                if not_pid.is_none() || info.pid != not_pid {
                    // A probe can take seconds; `adopt` writes the start record, so ask once
                    // more before it.
                    if self.cancelled() {
                        return Outcome::Cancelled;
                    }
                    return self.adopt(info, Some(spawn));
                }
            }
            if self.clock.now() >= deadline {
                return Outcome::StartFailed(StartError::NoAnswer);
            }
            self.clock.sleep(POLL);
        }
    }

    /// A core answered: apply the compatibility gate and work out whose it is.
    fn adopt(&mut self, info: CoreInfo, spawn: Option<Spawn>) -> Outcome {
        let (relation, optional_missing) = match caps::gate(&info.hello, &self.cfg.bundled_version)
        {
            Verdict::Refuse(reason) => {
                return Outcome::Incompatible {
                    version: info.hello.version,
                    reason,
                }
            }
            Verdict::Use {
                relation,
                optional_missing,
            } => (relation, optional_missing),
        };
        self.ever_ready = true;
        self.lost_since = None;
        let mut record_error = None;
        let started_now = spawn.zip(info.pid).and_then(|(spawn, pid)| {
            let age = u128::from(info.uptime_ms?);
            let since_start = self.clock.now().saturating_duration_since(spawn.began);
            // A core older than this start request was started by someone else.
            (age <= since_start.as_millis() + STARTED_BY_US_SLACK_MS).then(|| StartRecord {
                v: RECORD_VERSION,
                pid,
                started_unix_ms: spawn.unix_ms,
                exe: spawn.binary.to_string_lossy().into_owned(),
                version: info.hello.version.clone(),
                session: self.cfg.endpoint.session.clone(),
                socket: self.cfg.endpoint.socket.to_string_lossy().into_owned(),
            })
        });
        let origin = match started_now {
            Some(record) => {
                if let Some(path) = &self.cfg.record_path {
                    if let Err(e) = record.write(path) {
                        record_error = Some(e.to_string());
                    }
                }
                self.started = Some(record.clone());
                Origin::StartedByYuzora(record)
            }
            None => match (&self.started, info.pid) {
                (Some(record), Some(pid)) if record.pid == pid => {
                    Origin::StartedByYuzora(record.clone())
                }
                _ => Origin::External,
            },
        };
        Outcome::Ready(Box::new(Ready {
            hello: info.hello,
            relation,
            optional_missing,
            origin,
            record_error,
        }))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, VecDeque};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::herdr_core::fixtures;
    use crate::herdr_core::paths::{self, Env};

    fn endpoint() -> Endpoint {
        paths::resolve(
            paths::MACOS,
            &Env {
                home: Some("/Users/test".into()),
                ..Env::default()
            },
            "yuzora",
        )
        .unwrap()
    }

    fn names(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    /// The environment `command` would give its child, given the parent's `inherited`.
    fn effective_env(command: &Command, inherited: &[(&str, &str)]) -> BTreeMap<String, String> {
        let mut env: BTreeMap<String, String> = inherited
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        for (key, value) in command.get_envs() {
            let key = key.to_string_lossy().into_owned();
            match value {
                Some(value) => env.insert(key, value.to_string_lossy().into_owned()),
                None => env.remove(&key),
            };
        }
        env
    }

    const PARENT_ENV: &[(&str, &str)] = &[
        ("PATH", "/usr/bin"),
        ("HOME", "/Users/test"),
        ("LANG", "en_US.UTF-8"),
        ("HC_SESSION", "default"),
        ("HC_SOCKET", "/tmp/users-own.sock"),
        ("HC_TERMINAL_ID", "t7"),
        ("HC_TERMINAL_SOCKET", "/tmp/t.sock"),
        ("HC_SCOPE", "x"),
        ("HC_HEARTBEAT_MS", "10"),
        ("HC_SSH", "/tmp/evil"),
        ("HC_FUTURE_KNOB", "1"),
        ("HERDR_SOCKET_PATH", "/tmp/herdr.sock"),
        ("HERDR_RELEASES_URL", "http://example.invalid"),
        ("HERDR_ENV", "1"),
        // Not prefixed: stay.
        ("HCX", "keep"),
        ("HC", "keep"),
        ("HERDR", "keep"),
        ("MY_HC_SESSION", "keep"),
    ];

    #[test]
    fn the_core_environment_is_the_parents_minus_hc_and_herdr_plus_session_and_socket() {
        let ep = endpoint();
        let inherited = PARENT_ENV.iter().map(|(k, _)| OsString::from(k));
        let command = build_command(
            Path::new("/bundle/herdr-core"),
            &ep,
            &["server", "start"],
            inherited,
        );
        let env = effective_env(&command, PARENT_ENV);
        let mut keys: Vec<&str> = env.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "HC",
                "HCX",
                "HC_SESSION",
                "HC_SOCKET",
                "HERDR",
                "HOME",
                "LANG",
                "MY_HC_SESSION",
                "PATH"
            ]
        );
        assert_eq!(env["HC_SESSION"], "yuzora");
        assert_eq!(
            env["HC_SOCKET"],
            "/Users/test/.local/state/herdr-core/yuzora/yuzora.sock"
        );
        // Only HC_SESSION / HC_SOCKET of the HC_* names survive, and as ours.
        let hc_like: Vec<&String> = env
            .keys()
            .filter(|k| k.starts_with("HC_") || k.starts_with("HERDR_"))
            .collect();
        assert_eq!(hc_like.len(), 2, "{hc_like:?}");
        assert_eq!(command.get_program(), "/bundle/herdr-core");
        assert_eq!(command.get_args().collect::<Vec<_>>(), ["server", "start"]);
    }

    #[test]
    fn prefixes_are_case_sensitive_on_unix_and_not_on_windows() {
        let plan = env_plan(names(&["hc_lower", "Herdr_Mixed", "HC_UP"]), &endpoint());
        if cfg!(windows) {
            assert_eq!(plan.remove, names(&["hc_lower", "Herdr_Mixed", "HC_UP"]));
        } else {
            assert_eq!(plan.remove, names(&["HC_UP"]));
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_real_child_gets_the_planned_environment() {
        // The removal must win over a variable set on the command itself, i.e. over whatever
        // the parent passed down.
        let ep = endpoint();
        let mut command = Command::new("/usr/bin/env");
        command
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HC_FOO", "leak")
            .env("HERDR_BAR", "leak")
            .env("KEEP", "yes");
        apply_env_plan(
            &mut command,
            &env_plan(names(&["HC_FOO", "HERDR_BAR"]), &ep),
        );
        let output = command.output().expect("run env");
        let text = String::from_utf8_lossy(&output.stdout);
        let mut lines: Vec<&str> = text.lines().collect();
        lines.sort_unstable();
        assert_eq!(
            lines,
            [
                "HC_SESSION=yuzora",
                "HC_SOCKET=/Users/test/.local/state/herdr-core/yuzora/yuzora.sock",
                "KEEP=yes",
                "PATH=/usr/bin:/bin"
            ]
        );
    }

    #[test]
    fn only_start_status_restart_and_version_are_ever_run() {
        let ep = endpoint();
        let bin = Path::new("/bundle/herdr-core");
        for ok in [
            &["server", "start"][..],
            &["server", "status"],
            &["server", "restart"],
            &["--version"],
            &["-V"],
        ] {
            assert!(core_command(bin, &ep, ok).is_ok(), "{ok:?}");
        }
        for bad in [
            &["update"][..],
            &["update", "--version", "0.2.0"],
            &["server", "stop"],
            &["server"],
            &["server", "start", "--extra"],
            &["machine", "update", "x"],
            &["run", "sh"],
            &[],
        ] {
            assert_eq!(
                core_command(bin, &ep, bad).unwrap_err(),
                StartError::NotAllowed(bad.join(" ")),
                "{bad:?}"
            );
        }
    }

    // ------------------------------------------------------------------ record

    fn record() -> StartRecord {
        StartRecord {
            v: RECORD_VERSION,
            pid: 4242,
            started_unix_ms: 1_700_000_000_000,
            exe: "/Applications/Yuzora.app/Contents/Resources/herdr-core".into(),
            version: "0.1.14".into(),
            session: "yuzora".into(),
            socket: "/tmp/s.sock".into(),
        }
    }

    #[test]
    fn the_start_record_round_trips_and_replaces_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/herdr-core-start.json");
        assert_eq!(StartRecord::read(&path).unwrap(), None);
        record().write(&path).unwrap();
        assert_eq!(StartRecord::read(&path).unwrap(), Some(record()));
        let mut newer = record();
        newer.pid = 9;
        newer.write(&path).unwrap();
        assert_eq!(StartRecord::read(&path).unwrap().unwrap().pid, 9);
        // Nothing but the record is left in the directory.
        assert_eq!(
            std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
            1
        );
        // JSON an operator can read.
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("\"exe\"") && text.contains("\"started_unix_ms\""),
            "{text}"
        );
        assert_eq!(newer.exe_path(), PathBuf::from(&newer.exe));
    }

    #[test]
    fn an_unreadable_record_is_an_error_not_no_record() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rec.json");
        std::fs::write(&path, "{not json").unwrap();
        assert_eq!(
            StartRecord::read(&path).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        let mut future = record();
        future.v = 2;
        std::fs::write(&path, serde_json::to_vec(&future).unwrap()).unwrap();
        assert_eq!(
            StartRecord::read(&path).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    // ------------------------------------------------------------------ supervisor fakes

    fn hello(version: &str) -> Hello {
        let mut hello =
            Hello::parse(&fixtures::json(&format!("v{}/hello.json", fixtures::BUNDLED))["result"])
                .unwrap();
        hello.version = version.into();
        hello
    }

    fn core(pid: u32, uptime_ms: u64, version: &str) -> ProbeResult {
        ProbeResult::Core(CoreInfo {
            hello: hello(version),
            pid: Some(pid),
            uptime_ms: Some(uptime_ms),
        })
    }

    #[derive(Clone)]
    struct FakeClock {
        base: Instant,
        offset: Arc<Mutex<Duration>>,
    }

    impl FakeClock {
        fn new() -> FakeClock {
            FakeClock {
                base: Instant::now(),
                offset: Arc::default(),
            }
        }

        fn advance(&self, by: Duration) {
            *self.offset.lock().unwrap() += by;
        }

        fn elapsed(&self) -> Duration {
            *self.offset.lock().unwrap()
        }
    }

    impl Clock for FakeClock {
        fn now(&self) -> Instant {
            self.base + self.elapsed()
        }

        fn unix_ms(&self) -> u64 {
            1_700_000_000_000 + self.elapsed().as_millis() as u64
        }

        fn sleep(&self, duration: Duration) {
            self.advance(duration);
        }
    }

    /// Answers the scripted results in order; the last one repeats. `on_start` runs when the
    /// launcher is called, so a start can change what the next probes see.
    #[derive(Clone, Default)]
    struct Script {
        probes: Arc<Mutex<VecDeque<ProbeResult>>>,
        probe_count: Arc<Mutex<usize>>,
        launched: Arc<Mutex<Vec<(String, PathBuf)>>>,
        launch_result: Arc<Mutex<Option<StartError>>>,
    }

    impl Script {
        fn set(&self, results: Vec<ProbeResult>) {
            *self.probes.lock().unwrap() = results.into();
        }

        fn launched(&self) -> Vec<(String, PathBuf)> {
            self.launched.lock().unwrap().clone()
        }
    }

    impl Probe for Script {
        fn probe(&self, _: &Endpoint) -> ProbeResult {
            *self.probe_count.lock().unwrap() += 1;
            let mut queue = self.probes.lock().unwrap();
            if queue.len() > 1 {
                queue.pop_front().unwrap()
            } else {
                queue.front().cloned().unwrap_or(ProbeResult::NoCore)
            }
        }
    }

    impl Launcher for Script {
        fn start(&self, binary: &Path, _: &Endpoint) -> Result<(), StartError> {
            self.launched
                .lock()
                .unwrap()
                .push(("start".into(), binary.to_path_buf()));
            self.launch_result
                .lock()
                .unwrap()
                .clone()
                .map_or(Ok(()), Err)
        }

        fn restart(&self, binary: &Path, _: &Endpoint) -> Result<(), StartError> {
            self.launched
                .lock()
                .unwrap()
                .push(("restart".into(), binary.to_path_buf()));
            self.launch_result
                .lock()
                .unwrap()
                .clone()
                .map_or(Ok(()), Err)
        }
    }

    struct Rig {
        supervisor: Supervisor,
        script: Script,
        clock: FakeClock,
        dir: tempfile::TempDir,
    }

    fn rig() -> Rig {
        rig_with(Script::default(), None)
    }

    fn rig_with(script: Script, cancel: Option<CancelCheck>) -> Rig {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = SupervisorConfig::new(PathBuf::from("/bundle/herdr-core"), endpoint());
        cfg.record_path = Some(dir.path().join("start.json"));
        cfg.cancel = cancel;
        let clock = FakeClock::new();
        let supervisor = Supervisor::with_parts(
            cfg,
            Box::new(script.clone()),
            Box::new(script.clone()),
            Box::new(clock.clone()),
        );
        Rig {
            supervisor,
            script,
            clock,
            dir,
        }
    }

    fn ready(outcome: Outcome) -> Ready {
        match outcome {
            Outcome::Ready(ready) => *ready,
            other => panic!("expected Ready, got {other:?}"),
        }
    }

    // ------------------------------------------------------------------ supervisor

    #[test]
    fn a_cold_start_starts_the_bundled_core_at_once_and_records_it() {
        let mut rig = rig();
        rig.script
            .set(vec![ProbeResult::NoCore, core(777, 120, "0.1.14")]);
        let got = ready(rig.supervisor.ensure());
        assert_eq!(
            rig.clock.elapsed(),
            Duration::ZERO,
            "no waiting before the start"
        );
        assert_eq!(
            rig.script.launched(),
            [("start".to_string(), PathBuf::from("/bundle/herdr-core"))]
        );
        assert_eq!(got.relation, Relation::Same);
        let Origin::StartedByYuzora(record) = &got.origin else {
            panic!("{:?}", got.origin)
        };
        assert_eq!(
            (
                record.pid,
                record.exe.as_str(),
                record.version.as_str(),
                record.session.as_str()
            ),
            (777, "/bundle/herdr-core", "0.1.14", "yuzora")
        );
        assert_eq!(record.started_unix_ms, 1_700_000_000_000);
        assert_eq!(record.socket, endpoint().socket.to_string_lossy());
        assert_eq!(got.record_error, None);
        // Persisted for K9, and equal to what was returned.
        let path = rig.dir.path().join("start.json");
        assert_eq!(StartRecord::read(&path).unwrap().as_ref(), Some(record));
        // Later calls keep reporting the core as ours while the same pid answers.
        rig.script.set(vec![core(777, 5_000, "0.1.14")]);
        let again = ready(rig.supervisor.ensure());
        assert!(matches!(again.origin, Origin::StartedByYuzora(r) if r.pid == 777));
        assert_eq!(rig.script.launched().len(), 1);
    }

    #[test]
    fn a_core_that_is_already_running_is_used_and_never_touched() {
        let mut rig = rig();
        rig.script.set(vec![core(55, 3_600_000, "0.1.13")]);
        let got = ready(rig.supervisor.ensure());
        assert_eq!(got.origin, Origin::External);
        assert_eq!(got.relation, Relation::CoreOlder);
        assert!(rig.script.launched().is_empty());
        assert!(
            !rig.dir.path().join("start.json").exists(),
            "no record for a core we did not start"
        );
        // A newer core is fine too.
        rig.script.set(vec![core(55, 3_600_000, "0.2.0")]);
        assert_eq!(ready(rig.supervisor.ensure()).relation, Relation::CoreNewer);
    }

    #[test]
    fn an_incompatible_core_is_reported_and_left_alone() {
        let mut rig = rig();
        rig.script.set(vec![ProbeResult::Core(CoreInfo {
            hello: hello("0.1.5"),
            pid: Some(9),
            uptime_ms: Some(1),
        })]);
        for _ in 0..3 {
            assert_eq!(
                rig.supervisor.ensure(),
                Outcome::Incompatible {
                    version: "0.1.5".into(),
                    reason: Refusal::TooOld {
                        version: "0.1.5".into()
                    }
                }
            );
        }
        assert!(
            rig.script.launched().is_empty(),
            "never restarts or replaces it"
        );
        // Same for a newer protocol generation.
        let mut gen2 = hello("0.2.0");
        gen2.generation = 2;
        rig.script.set(vec![ProbeResult::Core(CoreInfo {
            hello: gen2,
            pid: None,
            uptime_ms: None,
        })]);
        assert!(matches!(
            rig.supervisor.ensure(),
            Outcome::Incompatible {
                reason: Refusal::Generation { core: 2 },
                ..
            }
        ));
        assert!(rig.script.launched().is_empty());
    }

    #[test]
    fn after_a_core_was_lost_nothing_starts_for_ten_seconds() {
        let mut rig = rig();
        rig.script.set(vec![core(55, 60_000, "0.1.14")]);
        ready(rig.supervisor.ensure());
        // The core goes away.
        rig.script.set(vec![ProbeResult::NoCore]);
        assert_eq!(
            rig.supervisor.ensure(),
            Outcome::Waiting {
                retry_in: GRACE_AFTER_LOSS
            }
        );
        rig.clock.advance(Duration::from_secs(4));
        assert_eq!(
            rig.supervisor.ensure(),
            Outcome::Waiting {
                retry_in: Duration::from_secs(6)
            }
        );
        rig.clock.advance(Duration::from_millis(5_999));
        assert!(matches!(rig.supervisor.ensure(), Outcome::Waiting { .. }));
        assert!(rig.script.launched().is_empty(), "still inside the 10 s");
        // 10 s without any core: now it starts the bundled one.
        rig.clock.advance(Duration::from_millis(1));
        rig.script
            .set(vec![ProbeResult::NoCore, core(900, 30, "0.1.14")]);
        let got = ready(rig.supervisor.ensure());
        assert_eq!(rig.script.launched().len(), 1);
        assert!(matches!(got.origin, Origin::StartedByYuzora(r) if r.pid == 900));
    }

    #[test]
    fn a_core_that_returns_inside_the_grace_period_is_used_and_the_clock_resets() {
        let mut rig = rig();
        rig.script.set(vec![core(55, 60_000, "0.1.14")]);
        ready(rig.supervisor.ensure());
        rig.script.set(vec![ProbeResult::NoCore]);
        assert!(matches!(rig.supervisor.ensure(), Outcome::Waiting { .. }));
        rig.clock.advance(Duration::from_secs(9));
        // The user's own `hc` brings one back (same or another pid).
        rig.script.set(vec![core(56, 2_000, "0.1.14")]);
        assert_eq!(ready(rig.supervisor.ensure()).origin, Origin::External);
        assert!(rig.script.launched().is_empty());
        // Losing it again starts a fresh 10 s.
        rig.script.set(vec![ProbeResult::NoCore]);
        rig.clock.advance(Duration::from_secs(30));
        assert_eq!(
            rig.supervisor.ensure(),
            Outcome::Waiting {
                retry_in: GRACE_AFTER_LOSS
            }
        );
    }

    #[test]
    fn notify_lost_starts_the_grace_period_when_the_session_dropped() {
        let mut rig = rig();
        rig.script.set(vec![core(55, 60_000, "0.1.14")]);
        ready(rig.supervisor.ensure());
        rig.supervisor.notify_lost();
        rig.clock.advance(Duration::from_secs(7));
        rig.script.set(vec![ProbeResult::NoCore]);
        assert_eq!(
            rig.supervisor.ensure(),
            Outcome::Waiting {
                retry_in: Duration::from_secs(3)
            }
        );
        // notify_lost before any core was ever reached does nothing: a cold start is immediate.
        let mut cold = self::rig();
        cold.supervisor.notify_lost();
        cold.script
            .set(vec![ProbeResult::NoCore, core(1, 10, "0.1.14")]);
        ready(cold.supervisor.ensure());
        assert_eq!(cold.script.launched().len(), 1);
    }

    #[test]
    fn a_core_started_by_someone_else_a_moment_ago_is_not_ours() {
        let mut rig = rig();
        // We start, but a user's `hc` won the race: the core answering is a minute old.
        rig.script
            .set(vec![ProbeResult::NoCore, core(31, 60_000, "0.1.14")]);
        let got = ready(rig.supervisor.ensure());
        assert_eq!(got.origin, Origin::External);
        assert!(rig.supervisor.started().is_none());
        assert!(!rig.dir.path().join("start.json").exists());
    }

    #[test]
    fn start_failures_are_reported_and_a_silent_start_times_out() {
        let mut rig = rig();
        *rig.script.launch_result.lock().unwrap() = Some(StartError::Exit(Some(1)));
        rig.script.set(vec![ProbeResult::NoCore]);
        assert_eq!(
            rig.supervisor.ensure(),
            Outcome::StartFailed(StartError::Exit(Some(1)))
        );
        // The CLI returned fine, but no core ever answers.
        *rig.script.launch_result.lock().unwrap() = None;
        let before = rig.clock.elapsed();
        assert_eq!(
            rig.supervisor.ensure(),
            Outcome::StartFailed(StartError::NoAnswer)
        );
        let waited = rig.clock.elapsed() - before;
        assert!(
            waited >= START_TIMEOUT && waited < START_TIMEOUT + Duration::from_secs(1),
            "{waited:?}"
        );
    }

    #[test]
    fn something_that_does_not_answer_is_never_a_reason_to_start() {
        let mut rig = rig();
        rig.script.set(vec![ProbeResult::Failed(ProbeFailure::new(
            "io",
            "permission denied",
        ))]);
        assert_eq!(
            rig.supervisor.ensure(),
            Outcome::Unreachable(ProbeFailure::new("io", "permission denied"))
        );
        assert!(rig.script.launched().is_empty());
    }

    // ------------------------------------------------------------------ cancelling a start

    #[test]
    fn a_start_cancelled_before_it_begins_probes_and_launches_nothing() {
        let flag = Arc::new(AtomicBool::new(true));
        let cancel = {
            let flag = flag.clone();
            CancelCheck::new(move || flag.load(Ordering::SeqCst))
        };
        let mut rig = rig_with(Script::default(), Some(cancel));
        rig.script.set(vec![ProbeResult::NoCore]);
        assert_eq!(rig.supervisor.ensure(), Outcome::Cancelled);
        assert_eq!(*rig.script.probe_count.lock().unwrap(), 0);
        assert!(rig.script.launched().is_empty());
        assert!(!rig.dir.path().join("start.json").exists());
        // Not sticky: the check decides every time.
        flag.store(false, Ordering::SeqCst);
        rig.script
            .set(vec![ProbeResult::NoCore, core(777, 50, "0.1.14")]);
        assert!(matches!(rig.supervisor.ensure(), Outcome::Ready(_)));
    }

    #[test]
    fn a_start_cancelled_while_waiting_for_the_core_ends_within_a_poll_and_writes_no_record() {
        let script = Script::default();
        let probes = script.probe_count.clone();
        // Fires once the fourth probe has been made: the first is the decision to start, the
        // rest are the wait for a core that never answers.
        let cancel = CancelCheck::new(move || *probes.lock().unwrap() >= 4);
        let mut rig = rig_with(script, Some(cancel));
        rig.script.set(vec![ProbeResult::NoCore]);
        assert_eq!(rig.supervisor.ensure(), Outcome::Cancelled);
        assert!(
            rig.clock.elapsed() <= 4 * POLL,
            "waited {:?} of {START_TIMEOUT:?}",
            rig.clock.elapsed()
        );
        assert_eq!(*rig.script.probe_count.lock().unwrap(), 4);
        // The `server start` that was issued stays issued...
        assert_eq!(rig.script.launched().len(), 1);
        // ... but nothing says Yuzora started whatever answers later.
        assert!(!rig.dir.path().join("start.json").exists());
        assert!(rig.supervisor.started().is_none());
    }

    #[test]
    fn a_core_that_answers_as_the_start_is_cancelled_is_not_recorded() {
        let script = Script::default();
        let probes = script.probe_count.clone();
        let cancel = CancelCheck::new(move || *probes.lock().unwrap() >= 2);
        let mut rig = rig_with(script, Some(cancel));
        // The second probe finds the core, and the cancel fires with it.
        rig.script
            .set(vec![ProbeResult::NoCore, core(777, 120, "0.1.14")]);
        assert_eq!(rig.supervisor.ensure(), Outcome::Cancelled);
        assert_eq!(rig.script.launched().len(), 1);
        assert!(!rig.dir.path().join("start.json").exists());
        assert!(rig.supervisor.started().is_none());
    }

    #[test]
    fn a_restart_cancelled_after_it_was_issued_keeps_the_old_record() {
        let script = Script::default();
        let launched = script.launched.clone();
        // Fires when the second launch (the restart) is issued.
        let cancel = CancelCheck::new(move || launched.lock().unwrap().len() >= 2);
        let mut rig = rig_with(script, Some(cancel));
        rig.script
            .set(vec![ProbeResult::NoCore, core(777, 120, "0.1.14")]);
        let Origin::StartedByYuzora(record) = ready(rig.supervisor.ensure()).origin else {
            panic!("started")
        };
        rig.script
            .set(vec![core(777, 9_000, "0.1.14"), core(888, 40, "0.1.15")]);
        assert_eq!(
            rig.supervisor
                .restart_with(&record, Path::new("/new/herdr-core")),
            Outcome::Cancelled
        );
        assert_eq!(rig.script.launched().len(), 2, "the restart was issued");
        let path = rig.dir.path().join("start.json");
        assert_eq!(
            StartRecord::read(&path).unwrap(),
            Some(record),
            "the record still describes the core it was written for"
        );
        assert_eq!(rig.supervisor.started().map(|r| r.pid), Some(777));
    }

    #[test]
    fn a_restart_cancelled_before_it_is_issued_launches_nothing() {
        let flag = Arc::new(AtomicBool::new(false));
        let cancel = {
            let flag = flag.clone();
            CancelCheck::new(move || flag.load(Ordering::SeqCst))
        };
        let mut rig = rig_with(Script::default(), Some(cancel));
        rig.script
            .set(vec![ProbeResult::NoCore, core(777, 120, "0.1.14")]);
        let Origin::StartedByYuzora(record) = ready(rig.supervisor.ensure()).origin else {
            panic!("started")
        };
        flag.store(true, Ordering::SeqCst);
        rig.script.set(vec![core(777, 9_000, "0.1.14")]);
        assert_eq!(
            rig.supervisor
                .restart_with(&record, Path::new("/new/herdr-core")),
            Outcome::Cancelled
        );
        assert_eq!(rig.script.launched().len(), 1, "only the original start");
    }

    // ------------------------------------------------------------------ structured failures

    /// A listener at a fresh path; `answer` is what it does with the one connection it takes.
    #[cfg(unix)]
    fn endpoint_answering(
        answer: impl FnOnce(interprocess::local_socket::Stream) + Send + 'static,
    ) -> (Endpoint, thread::JoinHandle<()>) {
        use interprocess::local_socket::traits::Listener as _;
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "yuzora-hc-probe-{}-{}.sock",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let listener = crate::herdr_transport::bind_local_listener(&path).unwrap();
        let mut ep = endpoint();
        ep.socket = path.clone();
        let handle = thread::spawn(move || {
            if let Ok(stream) = listener.accept() {
                answer(stream);
            }
            let _ = std::fs::remove_file(&path);
        });
        (ep, handle)
    }

    #[cfg(unix)]
    #[test]
    fn live_probe_reports_why_something_at_the_endpoint_could_not_be_used() {
        use std::io::{Read, Write};
        // It accepts and hangs up.
        let (ep, server) = endpoint_answering(drop);
        let ProbeResult::Failed(failure) = LiveProbe.probe(&ep) else {
            panic!("expected a failure")
        };
        server.join().unwrap();
        assert_eq!(failure.code, "connection_lost", "{failure:?}");
        assert!(!failure.message.is_empty());
        // It answers, but not in JSON-RPC.
        let (ep, server) = endpoint_answering(|stream| {
            let mut buf = [0u8; 1024];
            let _ = (&stream).read(&mut buf);
            let _ = (&stream).write_all(b"this is not json\n");
        });
        let ProbeResult::Failed(failure) = LiveProbe.probe(&ep) else {
            panic!("expected a failure")
        };
        server.join().unwrap();
        assert_eq!(failure.code, "protocol", "{failure:?}");
        // A failed probe is what the supervisor reports, code included, and never a start.
        let mut rig = rig();
        rig.script.set(vec![ProbeResult::Failed(failure.clone())]);
        assert_eq!(rig.supervisor.ensure(), Outcome::Unreachable(failure));
        assert!(rig.script.launched().is_empty());
    }

    #[test]
    fn restart_with_restarts_only_the_core_it_started() {
        let mut rig = rig();
        rig.script
            .set(vec![ProbeResult::NoCore, core(777, 120, "0.1.14")]);
        let Origin::StartedByYuzora(record) = ready(rig.supervisor.ensure()).origin else {
            panic!("started")
        };
        // The running core is another process (pid reuse or an external core): untouched.
        rig.script.set(vec![core(1234, 600_000, "0.1.14")]);
        assert_eq!(
            rig.supervisor
                .restart_with(&record, Path::new("/new/herdr-core")),
            Outcome::StartFailed(StartError::NotOurs)
        );
        rig.script.set(vec![ProbeResult::NoCore]);
        assert_eq!(
            rig.supervisor
                .restart_with(&record, Path::new("/new/herdr-core")),
            Outcome::StartFailed(StartError::NotOurs)
        );
        // Another session's record does not match either.
        let mut other = record.clone();
        other.session = "default".into();
        rig.script.set(vec![core(777, 5_000, "0.1.14")]);
        assert_eq!(
            rig.supervisor
                .restart_with(&other, Path::new("/new/herdr-core")),
            Outcome::StartFailed(StartError::NotOurs)
        );
        assert_eq!(
            rig.script.launched().len(),
            1,
            "only the original start ran"
        );
        // The recorded core answers: restart with the new binary; the new core is recorded.
        rig.script.set(vec![
            core(777, 9_000, "0.1.14"),
            core(777, 9_100, "0.1.14"),
            core(888, 40, "0.1.15"),
        ]);
        let got = ready(
            rig.supervisor
                .restart_with(&record, Path::new("/new/herdr-core")),
        );
        assert_eq!(
            rig.script.launched().last(),
            Some(&("restart".to_string(), PathBuf::from("/new/herdr-core")))
        );
        let Origin::StartedByYuzora(new_record) = got.origin else {
            panic!("restarted core is ours")
        };
        assert_eq!(
            (
                new_record.pid,
                new_record.exe.as_str(),
                new_record.version.as_str()
            ),
            (888, "/new/herdr-core", "0.1.15")
        );
        let path = rig.dir.path().join("start.json");
        assert_eq!(StartRecord::read(&path).unwrap(), Some(new_record));
        // No grace period was involved.
        assert!(rig.clock.elapsed() < GRACE_AFTER_LOSS);
    }

    #[test]
    fn live_probe_sees_no_core_where_nothing_listens() {
        // A bare name: a pipe on Windows, a path relative to the working directory elsewhere.
        let mut ep = endpoint();
        ep.socket = PathBuf::from(format!("yuzora-test-no-core-{}.sock", std::process::id()));
        assert_eq!(LiveProbe.probe(&ep), ProbeResult::NoCore);
    }

    #[cfg(unix)]
    #[test]
    fn run_command_reports_exit_timeout_and_spawn_failures() {
        let sh = |script: &str| {
            let mut command = Command::new("/bin/sh");
            command.args(["-c", script]);
            command
        };
        assert_eq!(run_command(sh("exit 0"), Duration::from_secs(10)), Ok(()));
        assert_eq!(
            run_command(sh("exit 3"), Duration::from_secs(10)),
            Err(StartError::Exit(Some(3)))
        );
        let started = Instant::now();
        assert_eq!(
            run_command(sh("exec sleep 30"), Duration::from_millis(200)),
            Err(StartError::Timeout)
        );
        assert!(started.elapsed() < Duration::from_secs(10));
        assert!(matches!(
            run_command(
                Command::new("/nonexistent/herdr-core"),
                Duration::from_secs(1)
            ),
            Err(StartError::Spawn(_))
        ));
    }
}
