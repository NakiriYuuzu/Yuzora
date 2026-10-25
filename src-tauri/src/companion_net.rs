//! Embedded Tailscale node supervisor (plan 4.2). The `yuzora-companion-net`
//! sidecar is a tsnet node: it joins the owner's tailnet, terminates TLS and
//! forwards only the owner's requests to the gateway on loopback. Yuzora owns
//! the process: it writes the init frame, follows the node's state for the UI
//! and stops it on disable or exit. Never log or emit the proxy secret.

use crate::{
    companion_service::{RecordReader, ENV_ALLOWLIST, STDERR_RECORD_BYTES},
    companion_store::{create_private_dir, NetIdentity},
};
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader},
    sync::{mpsc, oneshot, watch},
};

/// Every frame the sidecar writes is at most this long.
const MAX_FRAME_BYTES: u64 = 64 * 1024;
/// tsnet's own state. It is written as soon as the node starts, before any
/// login, so on its own it does not say the node is registered.
const STATE_FILE: &str = "tailscaled.state";
/// What one sidecar may put in the log per window. A peer that makes the node
/// complain on every connection must not write the log at its own pace.
const STDERR_LINES: usize = 60;
const STDERR_WINDOW: Duration = Duration::from_secs(60);
/// Only the node needs these, to reach Tailscale from behind a proxy; the
/// gateway never gets them.
const PROXY_ENV: &[&str] = &[
    "HTTPS_PROXY",
    "HTTP_PROXY",
    "NO_PROXY",
    "ALL_PROXY",
    "https_proxy",
    "http_proxy",
    "no_proxy",
    "all_proxy",
];

#[derive(Clone, Copy)]
pub struct Timing {
    /// From spawn to the node running.
    pub running: Duration,
    /// The first certificate takes about 30 s to issue.
    pub cert: Duration,
    pub serve: Duration,
    pub unserve: Duration,
    pub logout: Duration,
    pub stop: Duration,
}
pub const TIMING: Timing = Timing {
    running: Duration::from_secs(20),
    cert: Duration::from_secs(120),
    serve: Duration::from_secs(5),
    unserve: Duration::from_secs(2),
    logout: Duration::from_secs(10),
    stop: Duration::from_secs(2),
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum State {
    #[default]
    Stopped,
    Starting,
    NeedsLogin,
    /// Registered, and waiting for an admin to approve the machine
    /// (`NeedsMachineAuth`).
    NeedsApproval,
    Running,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CertState {
    Pending,
    Ready,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Cert {
    pub state: CertState,
    pub not_after: Option<String>,
    pub error: Option<String>,
}

/// What the UI sees of the node. `dns_name` and `login` are the last seen
/// ones while the node is not running, until it has to log in. While the node
/// waits for an admin's approval `dns_name` is the name of the machine to
/// approve, when the node said it: it is not a registered identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetOverview {
    pub state: State,
    /// Why the sidecar runs; `None` while it does not, or is being stopped
    /// together with the Companion (`NetManager::leaving`).
    pub mode: Option<Mode>,
    pub auth_url: Option<String>,
    pub dns_name: Option<String>,
    pub login: Option<String>,
    pub https_capable: Option<bool>,
    pub cert: Option<Cert>,
    pub serving: bool,
    pub logged_in: bool,
    pub error: Option<String>,
}

/// Why the sidecar runs; decides what its first `running` means.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Register the node, remember it, stop.
    Login,
    /// Serve the gateway.
    Enable,
    Logout,
}

pub enum Notice {
    /// The node runs, in whatever mode: it is registered, and settings keep
    /// it. One started to log in is done and can stop.
    Running,
    /// The node has to log in (never did, or its key expired): the identity
    /// it had is gone.
    LoginRequired,
    /// The sidecar ended; `requested` when this side asked for it.
    Exited { requested: bool },
}

/// A running node, as enable needs it.
pub struct Node {
    pub identity: NetIdentity,
    pub https_capable: bool,
}

type Emit = Arc<dyn Fn(Value) + Send + Sync>;
type Notify = Arc<dyn Fn(Notice) + Send + Sync>;

pub struct NetManager {
    /// `<app data>/companion/net`: tsnet's state lives here.
    dir: PathBuf,
    version: String,
    emit: Emit,
    notify: Notify,
    inner: Mutex<Inner>,
    timing: Timing,
}

#[derive(Default)]
struct Inner {
    state: State,
    generation: u64,
    process: Option<Process>,
    auth_url: Option<String>,
    /// Last seen: from settings at startup, from the node once it runs, gone
    /// once it has to log in. Settings follow it (`Notice`).
    identity: Option<NetIdentity>,
    /// The name the node reported with `NeedsMachineAuth`; only meaningful
    /// while the state is `NeedsApproval`.
    pending_name: Option<String>,
    https_capable: Option<bool>,
    cert: Option<Cert>,
    serving: bool,
    /// A `serve` went out and no `unserved` came back since.
    serve_sent: bool,
    error: Option<String>,
    logged_out: bool,
    /// Set when this side ends the process.
    requested: bool,
    /// A teardown has begun while the process still runs: it no longer runs
    /// for anything the UI should wait on.
    leaving: bool,
}

impl Inner {
    /// A node that has to log in is not registered, whatever was seen before.
    fn forget_identity(&mut self) -> Option<Notice> {
        self.identity.take().map(|_| Notice::LoginRequired)
    }
}

struct Process {
    mode: Mode,
    input: mpsc::UnboundedSender<Vec<u8>>,
    kill: Option<oneshot::Sender<()>>,
    exited: watch::Receiver<bool>,
}

impl NetManager {
    pub fn new(
        dir: PathBuf,
        version: String,
        identity: Option<NetIdentity>,
        emit: Emit,
        notify: Notify,
    ) -> Arc<Self> {
        Self::with_timing(dir, version, identity, emit, notify, TIMING)
    }

    pub fn with_timing(
        dir: PathBuf,
        version: String,
        identity: Option<NetIdentity>,
        emit: Emit,
        notify: Notify,
        timing: Timing,
    ) -> Arc<Self> {
        Arc::new(Self {
            dir,
            version,
            emit,
            notify,
            inner: Mutex::new(Inner {
                identity,
                ..Inner::default()
            }),
            timing,
        })
    }

    pub fn overview(&self) -> NetOverview {
        let inner = self.inner.lock().unwrap();
        NetOverview {
            state: inner.state,
            mode: inner
                .process
                .as_ref()
                .filter(|_| !inner.leaving)
                .map(|p| p.mode),
            auth_url: inner.auth_url.clone(),
            dns_name: inner
                .pending_name
                .clone()
                .filter(|_| inner.state == State::NeedsApproval)
                .or_else(|| inner.identity.as_ref().map(|i| i.dns_name.clone())),
            login: inner.identity.as_ref().map(|i| i.login.clone()),
            https_capable: inner.https_capable,
            cert: inner.cert.clone(),
            serving: inner.serving,
            logged_in: self.logged_in(&inner),
            error: inner.error.clone(),
        }
    }

    /// Registered: the node was seen running (and not asked to log in since),
    /// and tsnet still has its state.
    fn logged_in(&self, inner: &Inner) -> bool {
        inner.identity.is_some() && self.dir.join(STATE_FILE).exists()
    }

    /// What the node last said it is: the settings follow this.
    pub fn identity(&self) -> Option<NetIdentity> {
        self.inner.lock().unwrap().identity.clone()
    }

    /// The mode of the running sidecar, if one runs.
    pub fn mode(&self) -> Option<Mode> {
        self.inner.lock().unwrap().process.as_ref().map(|p| p.mode)
    }

    /// The Companion is being taken down, and this sidecar with it. The UI
    /// reads a mode of `enable` without a running gateway as an enable in
    /// progress, so until the sidecar is gone (or an enable starts it again)
    /// the overview reports no mode. `mode()`, which the logic uses, stays.
    pub fn leaving(&self) {
        let changed = {
            let mut inner = self.inner.lock().unwrap();
            inner.process.is_some() && !std::mem::replace(&mut inner.leaving, true)
        };
        if changed {
            self.publish();
        }
    }

    fn publish(&self) {
        (self.emit)(json!({"type": "net", "net": self.overview()}));
    }

    /// Starts the sidecar. One that already runs stays; enable takes over a
    /// login in progress, since both want the same node.
    pub fn start(self: &Arc<Self>, program: &Path, mode: Mode) -> Result<(), String> {
        let mut inner = self.inner.lock().unwrap();
        if inner.requested {
            return Err("companion-net-start-failed".into());
        }
        let leaving = inner.leaving;
        if let Some(process) = inner.process.as_mut() {
            // An enable also takes over a sidecar that was on its way out.
            if mode == Mode::Enable && (process.mode == Mode::Login || leaving) {
                process.mode = Mode::Enable;
                inner.leaving = false;
                drop(inner);
                self.publish();
            }
            return Ok(());
        }
        let failed = |e: &dyn std::fmt::Display| format!("companion-net-start-failed: {e}");
        create_private_dir(&self.dir).map_err(|e| failed(&e))?;
        let state_dir = self
            .dir
            .to_str()
            .ok_or_else(|| failed(&"state dir is not UTF-8"))?;
        let mut init = serde_json::to_vec(&json!({
            "v": 1,
            "type": "init",
            "stateDir": state_dir,
            "hostname": "yuzora",
            "version": self.version,
        }))
        .expect("a JSON value serializes");
        init.push(b'\n');
        let mut command = tokio::process::Command::new(program);
        command
            .env_clear()
            .envs(sidecar_env(|key| std::env::var_os(key)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x0800_0000);
        let mut child = crate::process_kill::spawn_tokio(&mut command).map_err(|e| failed(&e))?;
        let (input, mut frames) = mpsc::unbounded_channel::<Vec<u8>>();
        let (kill, killed) = oneshot::channel();
        let (exited_tx, exited) = watch::channel(false);
        let generation = inner.generation + 1;
        // What the previous run left is not this node's state.
        *inner = Inner {
            state: State::Starting,
            generation,
            identity: inner.identity.take(),
            process: Some(Process {
                mode,
                input: input.clone(),
                kill: Some(kill),
                exited,
            }),
            ..Inner::default()
        };
        drop(inner);
        self.publish();
        // The sidecar gives up without init after five seconds.
        let _ = input.send(init);
        let mut stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        // Stdin stays open while the process lives: EOF is how the sidecar
        // learns that Yuzora crashed (on Unix it also watches its parent pid,
        // for when another process kept a copy of this pipe).
        tokio::spawn(async move {
            while let Some(frame) = frames.recv().await {
                if stdin.write_all(&frame).await.is_err() || stdin.flush().await.is_err() {
                    break;
                }
            }
        });
        let reader = self.clone();
        let (read_done, reader_finished) = oneshot::channel::<()>();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout);
            let mut line = Vec::new();
            loop {
                line.clear();
                match (&mut lines)
                    .take(MAX_FRAME_BYTES + 1)
                    .read_until(b'\n', &mut line)
                    .await
                {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                if line.len() as u64 > MAX_FRAME_BYTES {
                    break;
                }
                if let Ok(event) = serde_json::from_slice::<Value>(&line) {
                    reader.on_event(generation, event);
                }
            }
            let _ = read_done.send(());
        });
        tokio::spawn(relay_stderr(
            stderr,
            STDERR_LINES,
            STDERR_WINDOW,
            |message| log_warning("companion_net_stderr", message),
        ));
        let waiter = self.clone();
        tokio::spawn(async move {
            let exited_alone = tokio::select! {
                _ = child.wait() => true,
                _ = killed => {
                    let _ = child.start_kill();
                    let _ = child.wait().await;
                    false
                },
            };
            // Handle every event it wrote (an error before the exit) first.
            if exited_alone {
                let _ = tokio::time::timeout(Duration::from_secs(1), reader_finished).await;
            }
            // Final state first: whoever waits on `exited` sees it.
            waiter.on_exit(generation);
            let _ = exited_tx.send(true);
        });
        Ok(())
    }

    fn on_event(self: &Arc<Self>, generation: u64, event: Value) {
        let kind = event["type"].as_str().unwrap_or_default();
        let mut notice = None;
        let changed = {
            let mut inner = self.inner.lock().unwrap();
            // Nothing a process says after it was asked to stop is acted on.
            if inner.generation != generation || inner.requested {
                return;
            }
            match kind {
                "state" => match event["backendState"].as_str() {
                    Some("NeedsLogin") => {
                        inner.state = State::NeedsLogin;
                        notice = inner.forget_identity();
                    }
                    // `running` carries the name; until it comes the node is not usable.
                    Some("Running") if inner.https_capable.is_none() => {}
                    Some("Running") => inner.state = State::Running,
                    Some("NeedsMachineAuth") => {
                        inner.state = State::NeedsApproval;
                        inner.auth_url = None;
                        // The machine waiting to be approved, not an identity:
                        // only `running` makes one.
                        inner.pending_name = event["dnsName"]
                            .as_str()
                            .filter(|name| !name.is_empty())
                            .map(str::to_owned);
                    }
                    Some(_) => {
                        inner.state = State::Starting;
                        inner.auth_url = None;
                    }
                    None => {}
                },
                "login-required" => {
                    inner.auth_url = event["authUrl"].as_str().map(str::to_owned);
                    inner.state = State::NeedsLogin;
                    notice = inner.forget_identity();
                }
                "running" => {
                    let name = |key: &str| event[key].as_str().filter(|text| !text.is_empty());
                    let (Some(dns_name), Some(login), Some(https)) = (
                        name("dnsName"),
                        name("login"),
                        event["httpsCapable"].as_bool(),
                    ) else {
                        return;
                    };
                    inner.identity = Some(NetIdentity {
                        dns_name: dns_name.to_owned(),
                        login: login.to_owned(),
                    });
                    inner.https_capable = Some(https);
                    inner.auth_url = None;
                    inner.state = State::Running;
                    notice = Some(Notice::Running);
                }
                "cert" => {
                    let state = match event["state"].as_str() {
                        Some("pending") => CertState::Pending,
                        Some("ready") => CertState::Ready,
                        Some("error") => CertState::Error,
                        _ => return,
                    };
                    let text = |key: &str| event[key].as_str().map(str::to_owned);
                    inner.cert = Some(Cert {
                        state,
                        not_after: text("notAfter"),
                        error: text("error"),
                    });
                }
                "serving" => inner.serving = true,
                "unserved" => {
                    inner.serving = false;
                    inner.serve_sent = false;
                }
                "logged-out" => inner.logged_out = true,
                "error" => {
                    let code = event["code"].as_str().unwrap_or_default();
                    let message = event["message"].as_str().unwrap_or_default();
                    log_warning("companion_net_error", &format!("{code}: {message}"));
                    inner.error = Some(error_code(code).into());
                }
                "peer.rejected" => {
                    let reason = event["reason"].as_str().unwrap_or_default();
                    log_warning("companion_net_peer_rejected", reason);
                    return;
                }
                _ => return,
            }
            true
        };
        if changed {
            self.publish();
        }
        if let Some(notice) = notice {
            (self.notify)(notice);
        }
    }

    /// No automatic restart: a node that comes back by itself would serve
    /// again with nothing deciding whether the gateway behind it is the same.
    fn on_exit(self: &Arc<Self>, generation: u64) {
        let requested = {
            let mut inner = self.inner.lock().unwrap();
            if inner.generation != generation {
                return;
            }
            // Output still buffered from this process must not change anything.
            inner.generation += 1;
            let requested = inner.requested || inner.logged_out;
            inner.requested = false;
            inner.process = None;
            inner.auth_url = None;
            inner.https_capable = None;
            inner.cert = None;
            inner.serving = false;
            if requested {
                inner.state = State::Stopped;
                inner.error = None;
            } else {
                inner.state = State::Failed;
                inner
                    .error
                    .get_or_insert_with(|| "companion-net-crashed".into());
            }
            requested
        };
        self.publish();
        (self.notify)(Notice::Exited { requested });
    }

    /// Requested stop: shutdown frame, bounded wait, then kill.
    pub async fn stop(&self) {
        let (mut exited, kill) = {
            let mut inner = self.inner.lock().unwrap();
            let Some(process) = inner.process.as_mut() else {
                return;
            };
            let _ = process
                .input
                .send(b"{\"v\":1,\"type\":\"shutdown\"}\n".to_vec());
            let kill = process.kill.take();
            let exited = process.exited.clone();
            inner.requested = true;
            (exited, kill)
        };
        if tokio::time::timeout(self.timing.stop, exited.wait_for(|done| *done))
            .await
            .is_err()
        {
            if let Some(kill) = kill {
                let _ = kill.send(());
            }
            let _ = tokio::time::timeout(self.timing.stop, exited.wait_for(|done| *done)).await;
        }
    }

    fn send(&self, frame: Value) -> bool {
        write(&self.inner.lock().unwrap(), frame)
    }

    /// Polls until `check` decides. The sidecar being gone ends every wait.
    async fn wait<T>(
        &self,
        limit: Duration,
        timeout_code: &str,
        check: impl Fn(&Inner) -> Option<Result<T, String>>,
    ) -> Result<T, String> {
        let deadline = tokio::time::Instant::now() + limit;
        loop {
            {
                let inner = self.inner.lock().unwrap();
                if let Some(done) = check(&inner) {
                    return done;
                }
                if inner.process.is_none() {
                    return Err(inner
                        .error
                        .clone()
                        .unwrap_or_else(|| "companion-stopped".into()));
                }
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(timeout_code.into());
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    /// Waits for the node to run. A node that needs a login says so at once.
    pub async fn wait_running(&self) -> Result<Node, String> {
        self.wait(
            self.timing.running,
            "companion-net-start-failed",
            |inner| match (inner.state, &inner.identity) {
                (State::Running, Some(identity)) => Some(Ok(Node {
                    identity: identity.clone(),
                    https_capable: inner.https_capable == Some(true),
                })),
                (State::NeedsLogin, _) => Some(Err("companion-net-login-required".into())),
                (State::NeedsApproval, _) => Some(Err("companion-net-approval-required".into())),
                _ => None,
            },
        )
        .await
    }

    /// Waits for the certificate; the node retries a failed issue by itself.
    /// A node that stops being usable meanwhile (its key expired, or its
    /// machine is no longer approved) says so at once: no certificate comes.
    pub async fn wait_cert(&self) -> Result<(), String> {
        self.wait(self.timing.cert, "companion-net-cert-failed", |inner| {
            match inner.state {
                State::NeedsLogin => return Some(Err("companion-net-login-required".into())),
                State::NeedsApproval => return Some(Err("companion-net-approval-required".into())),
                _ => {}
            }
            inner
                .cert
                .as_ref()
                .is_some_and(|cert| cert.state == CertState::Ready)
                .then_some(Ok(()))
        })
        .await
    }

    /// Opens the tailnet listener in front of the gateway on `target`.
    pub async fn serve(
        &self,
        target: &str,
        proxy_secret: &str,
        required_login: &str,
    ) -> Result<(), String> {
        let sent = {
            let mut inner = self.inner.lock().unwrap();
            // An error seen below is this command's.
            inner.error = None;
            let sent = write(
                &inner,
                json!({
                    "v": 1,
                    "type": "serve",
                    "target": target,
                    "proxySecret": proxy_secret,
                    "requiredLogin": required_login,
                }),
            );
            inner.serve_sent = sent;
            sent
        };
        if !sent {
            return Err("companion-net-serve-failed".into());
        }
        self.wait(self.timing.serve, "companion-net-serve-failed", |inner| {
            if inner.serving {
                Some(Ok(()))
            } else {
                inner.error.clone().map(Err)
            }
        })
        .await
    }

    /// Closes the listener before the gateway behind it goes, and waits until
    /// the node says it has. Nothing happens if no `serve` was sent. True when
    /// nothing forwards to the gateway any more: the node said so, or it is
    /// gone. False when it did not say within its bound; a caller that must not
    /// let the gateway go before that stops the node.
    pub async fn unserve(&self) -> bool {
        let sent = {
            let inner = self.inner.lock().unwrap();
            if !inner.serve_sent {
                return true;
            }
            write(&inner, json!({"v": 1, "type": "unserve"}))
        };
        let confirmed = sent
            && self
                .wait(self.timing.unserve, "", |inner| {
                    (!inner.serve_sent).then_some(Ok(()))
                })
                .await
                .is_ok();
        confirmed || self.inner.lock().unwrap().process.is_none()
    }

    /// Logs the node out of the tailnet, then deletes its state. A node that
    /// is not registered (never logged in, or its key expired) is not asked:
    /// its state is only deleted, and `program` (the sidecar to ask with) is
    /// not even looked for. One whose machine is waiting for approval (a
    /// first registration, or an approval taken back) cannot log out either,
    /// and is not waited for: its state goes the same way.
    pub async fn logout(
        self: &Arc<Self>,
        program: impl FnOnce() -> Result<PathBuf, String> + Send,
    ) -> Result<(), String> {
        let failed = |detail: &str| {
            log_warning("companion_net_logout", detail);
            "companion-net-logout-failed".to_string()
        };
        // A login in progress holds the same state directory.
        self.stop().await;
        if self.logged_in(&self.inner.lock().unwrap()) {
            let program = program()?;
            let outcome = match self.logout_node(&program).await {
                // Its key expired since, or its machine is no longer approved:
                // there is no login left to end.
                Err(code)
                    if code == "companion-net-login-required"
                        || code == "companion-net-approval-required" =>
                {
                    Ok(())
                }
                outcome => outcome,
            };
            self.stop().await;
            outcome.map_err(|e| failed(&e))?;
        }
        match std::fs::remove_dir_all(&self.dir) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(failed(&e.to_string())),
        }
        self.inner.lock().unwrap().identity = None;
        self.publish();
        Ok(())
    }

    async fn logout_node(self: &Arc<Self>, program: &Path) -> Result<(), String> {
        self.start(program, Mode::Logout)?;
        // The control server answers a logout; wait until the node can reach it.
        self.wait_running().await?;
        self.inner.lock().unwrap().error = None;
        if !self.send(json!({"v": 1, "type": "logout"})) {
            return Err("logout frame not sent".into());
        }
        self.wait(self.timing.logout, "logout timed out", |inner| {
            if inner.logged_out {
                Some(Ok(()))
            } else {
                inner.error.clone().map(Err)
            }
        })
        .await
    }
}

/// The sidecar's stderr, line by line, into `log`: masked first, then cut. At
/// most `limit` lines per `window` are passed on (the rest are read and
/// dropped); a window that dropped lines ends with one summary line holding the
/// count, even when nothing else is said after it.
async fn relay_stderr(
    stderr: impl AsyncRead + Unpin,
    limit: usize,
    window: Duration,
    log: impl Fn(&str),
) {
    let mut lines = RecordReader::new(BufReader::new(stderr), STDERR_RECORD_BYTES);
    let (mut opened, mut written, mut dropped) = (tokio::time::Instant::now(), 0, 0usize);
    let summary = |dropped: usize| log(&format!("… {dropped} similar lines suppressed"));
    loop {
        let next = if dropped > 0 {
            tokio::select! {
                line = lines.next() => line,
                _ = tokio::time::sleep_until(opened + window) => {
                    summary(dropped);
                    dropped = 0;
                    continue;
                }
            }
        } else {
            lines.next().await
        };
        let Some(line) = next else { break };
        let now = tokio::time::Instant::now();
        if now >= opened + window {
            if dropped > 0 {
                summary(dropped);
                dropped = 0;
            }
            (opened, written) = (now, 0);
        }
        if written < limit {
            written += 1;
            // A proxy's address is logged with its credentials.
            let line = crate::logging::mask_url_userinfo(&line);
            log(&line.chars().take(500).collect::<String>());
        } else {
            dropped += 1;
        }
    }
    if dropped > 0 {
        summary(dropped);
    }
}

fn write(inner: &Inner, frame: Value) -> bool {
    let mut bytes = serde_json::to_vec(&frame).expect("a JSON value serializes");
    bytes.push(b'\n');
    inner
        .process
        .as_ref()
        .is_some_and(|process| process.input.send(bytes).is_ok())
}

/// The variables the sidecar gets: the gateway's allowlist plus the proxy ones.
fn sidecar_env(lookup: impl Fn(&str) -> Option<OsString>) -> Vec<(&'static str, OsString)> {
    ENV_ALLOWLIST
        .iter()
        .chain(PROXY_ENV)
        .filter_map(|key| lookup(key).map(|value| (*key, value)))
        .collect()
}

/// The codes the UI knows. The sidecar's own are logged.
fn error_code(code: &str) -> &'static str {
    match code {
        "net-not-ready" | "net-serve-invalid" | "net-listen-failed" => "companion-net-serve-failed",
        "net-logout-failed" => "companion-net-logout-failed",
        _ => "companion-net-start-failed",
    }
}

/// `yuzora-companion-net` next to the executable.
pub fn program() -> Result<PathBuf, String> {
    #[cfg(debug_assertions)]
    if let Some(path) = std::env::var_os("YUZORA_COMPANION_NET_BIN") {
        return Ok(path.into());
    }
    let name = if cfg!(windows) {
        "yuzora-companion-net.exe"
    } else {
        "yuzora-companion-net"
    };
    let path = std::env::current_exe()
        .ok()
        .and_then(|exe| Some(exe.parent()?.join(name)))
        .filter(|path| path.is_file());
    path.ok_or_else(|| "companion-net-unavailable".into())
}

fn log_warning(event: &str, message: &str) {
    crate::logging::write_global(crate::logging::LogEvent {
        level: "warn".to_string(),
        kind: "debug".to_string(),
        source: "companion".to_string(),
        workspace_path: None,
        event: event.to_string(),
        message: message.to_string(),
        metadata: json!({}),
    });
}

#[cfg(test)]
mod tests;
