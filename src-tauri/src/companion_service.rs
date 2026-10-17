//! Companion sidecar supervisor (plan 4.2). Yuzora owns the gateway process:
//! it writes the init frame, relays admin events to the main window, persists
//! device changes before they count, and stops the process on disable or exit.
//! Never log or emit the init frame, the pairing URL or its QR code.

use crate::companion_store::{device_id, Devices};
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    sync::{mpsc, oneshot, watch},
};

const MAX_RESTARTS: u32 = 3;

#[derive(Clone, Copy)]
struct Timing {
    command: Duration,
    stop: Duration,
    /// Doubled for each restart.
    backoff: Duration,
}
const TIMING: Timing = Timing {
    command: Duration::from_secs(5),
    stop: Duration::from_secs(2),
    backoff: Duration::from_secs(1),
};
/// HERDR finds its sessions through HOME (and XDG on Linux); nothing else
/// from the desktop environment, such as imported API keys, reaches the gateway.
const ENV_ALLOWLIST: &[&str] = &[
    "PATH",
    "HOME",
    "TMPDIR",
    "USER",
    "LOGNAME",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "XDG_RUNTIME_DIR",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_STATE_HOME",
    "XDG_CACHE_HOME",
    "SystemRoot",
    "SystemDrive",
    "windir",
    "USERPROFILE",
    "USERNAME",
    "HOMEDRIVE",
    "HOMEPATH",
    "APPDATA",
    "LOCALAPPDATA",
    "ProgramData",
    "TEMP",
    "TMP",
    "PATHEXT",
    "ComSpec",
];

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum Status {
    #[default]
    Disabled,
    Starting,
    Running {
        port: u16,
    },
    Stopping,
    Failed {
        code: String,
    },
}

#[derive(Clone)]
pub struct Launch {
    pub program: PathBuf,
    /// The init frame without `devices`; they are read from disk at every start.
    pub init: Value,
}

type Emit = Arc<dyn Fn(Value) + Send + Sync>;
type Stopped = Arc<dyn Fn() + Send + Sync>;

pub struct CompanionManager {
    dir: PathBuf,
    emit: Emit,
    /// Runs after the gateway ends on its own (expiry, failure), not on a
    /// requested stop, whose caller does its own cleanup.
    stopped: Stopped,
    inner: Mutex<Inner>,
    /// The running gateway's device list. Separate from `inner` so a slow
    /// fsync never blocks status reads; always locked before `inner`.
    store: Mutex<Option<Devices>>,
    commands: tokio::sync::Mutex<()>,
    timing: Timing,
}

#[derive(Default)]
struct Inner {
    status: Status,
    generation: u64,
    process: Option<Process>,
    launch: Option<Launch>,
    restarts: u32,
    ready: bool,
    expired: bool,
    last_error: Option<String>,
    /// Set when this side ends the process; `on_exit` applies it instead of restarting.
    terminal: Option<Status>,
    pending: Option<Pending>,
}

struct Process {
    input: mpsc::UnboundedSender<Vec<u8>>,
    kill: Option<oneshot::Sender<()>>,
    exited: watch::Receiver<bool>,
}

struct Pending {
    want: Want,
    reply: oneshot::Sender<Result<Value, String>>,
}

enum Want {
    Pairing,
    /// Resolved by the event that reports this command's own change.
    Device {
        id: String,
        done: &'static str,
        frame: Value,
    },
}

impl CompanionManager {
    pub fn new(dir: PathBuf, emit: Emit, stopped: Stopped) -> Arc<Self> {
        Arc::new(Self {
            dir,
            emit,
            stopped,
            inner: Mutex::default(),
            store: Mutex::new(None),
            commands: tokio::sync::Mutex::new(()),
            timing: TIMING,
        })
    }

    pub fn status(&self) -> Status {
        self.inner.lock().unwrap().status.clone()
    }

    /// Nothing can write devices.json: stopped, and a halted process is gone.
    pub fn idle(&self) -> bool {
        let inner = self.inner.lock().unwrap();
        inner.process.is_none() && matches!(inner.status, Status::Disabled | Status::Failed { .. })
    }

    pub fn devices(&self) -> Result<Vec<Value>, String> {
        Ok(Devices::load(&self.dir)?
            .all()
            .iter()
            .map(device_summary)
            .collect())
    }

    pub fn start(self: &Arc<Self>, launch: Launch) -> Result<(), String> {
        let expected = {
            let mut inner = self.inner.lock().unwrap();
            if inner.process.is_some()
                || matches!(inner.status, Status::Starting | Status::Stopping)
            {
                return Err("companion-already-running".into());
            }
            inner.restarts = 0;
            inner.expired = false;
            inner.status = Status::Starting;
            inner.generation
        };
        (self.emit)(status_event(&Status::Starting));
        self.spawn(launch, expected)
    }

    /// `expected` is the generation seen when this start was decided; a stop
    /// that lands while the process is being created cancels it.
    fn spawn(self: &Arc<Self>, launch: Launch, expected: u64) -> Result<(), String> {
        let started = Devices::load(&self.dir).and_then(|devices| {
            let mut frame = launch.init.clone();
            frame["devices"] = json!(devices.all());
            let mut bytes = serde_json::to_vec(&frame).map_err(|e| e.to_string())?;
            bytes.push(b'\n');
            let mut command = tokio::process::Command::new(&launch.program);
            command
                .env_clear()
                .envs(
                    ENV_ALLOWLIST
                        .iter()
                        .filter_map(|key| std::env::var_os(key).map(|value| (*key, value))),
                )
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);
            #[cfg(windows)]
            command.creation_flags(0x0800_0000);
            let child = command
                .spawn()
                .map_err(|e| format!("companion-unavailable: {e}"))?;
            Ok((devices, bytes, child))
        });
        let (devices, init, mut child) = match started {
            Ok(started) => started,
            Err(code) => {
                let current = {
                    let mut inner = self.inner.lock().unwrap();
                    let current = inner.generation == expected && inner.status == Status::Starting;
                    if current {
                        inner.status = Status::Failed { code: code.clone() };
                    }
                    current
                };
                if current {
                    (self.emit)(status_event(&Status::Failed { code: code.clone() }));
                }
                return Err(code);
            }
        };
        let (input, mut frames) = mpsc::unbounded_channel::<Vec<u8>>();
        let (kill, killed) = oneshot::channel();
        let (exited_tx, exited) = watch::channel(false);
        let generation = {
            let mut store = self.store.lock().unwrap();
            let mut inner = self.inner.lock().unwrap();
            if inner.generation != expected || inner.status != Status::Starting {
                // Stopped meanwhile: dropping the child kills it.
                return Err("companion-cancelled".into());
            }
            *store = Some(devices);
            inner.generation += 1;
            inner.process = Some(Process {
                input: input.clone(),
                kill: Some(kill),
                exited,
            });
            inner.launch = Some(launch);
            inner.ready = false;
            inner.last_error = None;
            inner.terminal = None;
            inner.generation
        };
        // The gateway gives up without init after five seconds.
        let _ = input.send(init);
        let mut stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        // Stdin stays open while the process lives: EOF is how the gateway
        // learns that Yuzora crashed.
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
            // The gateway stops itself after a one-second stdout stall, so this
            // loop never waits on anything but the pipe.
            let mut lines = BufReader::new(stdout);
            let mut line = Vec::new();
            let limit = yuzora_host::protocol::MAX_FRAME_BYTES as u64;
            loop {
                line.clear();
                match (&mut lines)
                    .take(limit + 1)
                    .read_until(b'\n', &mut line)
                    .await
                {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                if line.len() as u64 > limit {
                    break;
                }
                if let Ok(event) = serde_json::from_slice::<Value>(&line) {
                    reader.on_event(generation, event);
                }
            }
            let _ = read_done.send(());
        });
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                log_stderr(&line);
            }
        });
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
            // Handle every event it wrote (ready, policy-expired, a startup
            // error) before deciding what the exit means. A kill already decided.
            if exited_alone {
                let _ = tokio::time::timeout(Duration::from_secs(1), reader_finished).await;
            }
            let _ = exited_tx.send(true);
            waiter.on_exit(generation);
        });
        Ok(())
    }

    fn on_event(self: &Arc<Self>, generation: u64, event: Value) {
        // Held for the whole event so a newer process cannot swap the list
        // between the generation check and the write.
        let mut store = self.store.lock().unwrap();
        if self.inner.lock().unwrap().generation != generation {
            return;
        }
        let kind = event["type"].as_str().unwrap_or_default().to_owned();
        let mut outgoing = Vec::new();
        match kind.as_str() {
            "device.registered" | "device.updated" | "device.revoked" => {
                let id = if kind == "device.revoked" {
                    event["deviceId"].as_str().map(str::to_owned)
                } else {
                    device_id(&event["device"]).map(str::to_owned)
                };
                let saved = match (store.as_mut(), &id) {
                    (Some(devices), Some(id)) if kind == "device.revoked" => devices.remove(id),
                    (Some(devices), Some(_)) => devices.upsert(event["device"].clone()),
                    _ => Err("companion-device-invalid".into()),
                };
                let mut inner = self.inner.lock().unwrap();
                match saved {
                    // A device change that is not on disk would come back at
                    // the next start. Stop the gateway rather than drift.
                    Err(code) => {
                        halt(&mut inner, &code);
                        outgoing.push(status_event(&inner.status));
                    }
                    Ok(()) => {
                        let id = id.unwrap_or_default();
                        resolve(
                            &mut inner,
                            |want| matches!(want, Want::Device { id: wanted, done, .. } if *wanted == id && *done == kind),
                            Ok(event.clone()),
                        );
                        outgoing.push(match kind.as_str() {
                            "device.revoked" => event.clone(),
                            _ => json!({"type": kind, "device": device_summary(&event["device"])}),
                        });
                    }
                }
            }
            // The gateway already applied a tightening or neutral change but
            // could not report it yet. Persist it now: a later stop would
            // otherwise lose the retry and restore the old device at init.
            "command.deferred" => {
                let target = event["target"].as_str().unwrap_or_default().to_owned();
                // A late deferral of a timed-out command must not complete
                // the next command on the same device.
                let frame = match &self.inner.lock().unwrap().pending {
                    Some(Pending {
                        want: Want::Device { id, frame, .. },
                        ..
                    }) if *id == target && frame["type"] == event["command"] => Some(frame.clone()),
                    _ => None,
                };
                if let Some(frame) = frame {
                    let saved = match store.as_mut() {
                        Some(devices) => devices.apply(&frame),
                        None => Err("companion-device-invalid".into()),
                    };
                    let mut inner = self.inner.lock().unwrap();
                    match saved {
                        Err(code) => {
                            halt(&mut inner, &code);
                            outgoing.push(status_event(&inner.status));
                        }
                        Ok(()) => resolve(
                            &mut inner,
                            |want| matches!(want, Want::Device { id, frame, .. } if *id == target && frame["type"] == event["command"]),
                            Ok(event.clone()),
                        ),
                    }
                }
                outgoing.push(event);
            }
            _ => {
                let mut inner = self.inner.lock().unwrap();
                match kind.as_str() {
                    "ready" if inner.process.is_some() && inner.terminal.is_none() => {
                        let port = event["ports"]["loopback"].as_u64().unwrap_or_default() as u16;
                        inner.ready = true;
                        inner.status = Status::Running { port };
                        outgoing.push(status_event(&inner.status));
                    }
                    "pairing.created" => {
                        resolve(&mut inner, |want| matches!(want, Want::Pairing), Ok(event))
                    }
                    // Errors carry no request id; one arriving while a command
                    // waits is reported to that command. State never depends on it.
                    "error" => {
                        let code = event["code"]
                            .as_str()
                            .unwrap_or("companion-error")
                            .to_owned();
                        if code == "policy-expired" {
                            inner.expired = true;
                        }
                        if let Some(pending) = inner.pending.take() {
                            let _ = pending.reply.send(Err(code.clone()));
                        }
                        inner.last_error = Some(code.clone());
                        outgoing.push(json!({"type": "error", "code": code}));
                    }
                    _ => outgoing.push(event),
                }
            }
        }
        drop(store);
        for event in outgoing {
            (self.emit)(event);
        }
    }

    fn on_exit(self: &Arc<Self>, generation: u64) {
        let (status, restart, stopped) = {
            let mut inner = self.inner.lock().unwrap();
            if inner.generation != generation {
                return;
            }
            inner.process = None;
            if let Some(pending) = inner.pending.take() {
                let _ = pending.reply.send(Err("companion-exited".into()));
            }
            let requested = inner.terminal == Some(Status::Disabled);
            let (status, restart) = if let Some(terminal) = inner.terminal.take() {
                (terminal, false)
            } else if inner.expired {
                (Status::Disabled, false)
            } else if !inner.ready {
                // Configuration errors (port in use, web root) do not heal by retrying.
                let code = inner
                    .last_error
                    .clone()
                    .unwrap_or_else(|| "companion-exited".into());
                (Status::Failed { code }, false)
            } else if inner.restarts < MAX_RESTARTS {
                inner.restarts += 1;
                (Status::Starting, true)
            } else {
                (
                    Status::Failed {
                        code: "companion-crashed".into(),
                    },
                    false,
                )
            };
            inner.status = status.clone();
            let restart = restart.then(|| (inner.launch.clone(), inner.restarts));
            let stopped = !requested && !matches!(status, Status::Starting);
            (status, restart, stopped)
        };
        (self.emit)(status_event(&status));
        if stopped {
            (self.stopped)();
        }
        if let Some((Some(launch), attempt)) = restart {
            let manager = self.clone();
            let delay = self.timing.backoff * (1 << (attempt - 1));
            tokio::spawn(async move {
                tokio::time::sleep(delay).await;
                let expected = {
                    let inner = manager.inner.lock().unwrap();
                    (inner.generation == generation && inner.status == Status::Starting)
                        .then_some(inner.generation)
                };
                // A restart keeps the original enableUntil from the same init.
                if let Some(expected) = expected {
                    if manager.spawn(launch, expected).is_err()
                        && matches!(manager.status(), Status::Failed { .. })
                    {
                        (manager.stopped)();
                    }
                }
            });
        }
    }

    /// Requested stop: shutdown frame, bounded wait, then kill.
    pub async fn stop(&self) {
        let exited = {
            let mut inner = self.inner.lock().unwrap();
            // Bumping the generation also cancels a scheduled restart.
            let Some(process) = inner.process.as_mut() else {
                inner.generation += 1;
                inner.status = Status::Disabled;
                drop(inner);
                (self.emit)(status_event(&Status::Disabled));
                return;
            };
            let _ = process
                .input
                .send(b"{\"v\":1,\"type\":\"shutdown\"}\n".to_vec());
            let kill = process.kill.take();
            let exited = process.exited.clone();
            inner.terminal = Some(Status::Disabled);
            inner.status = Status::Stopping;
            (exited, kill)
        };
        (self.emit)(status_event(&Status::Stopping));
        let (mut exited, kill) = exited;
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

    /// For the app-exit branch, which runs outside any async context.
    pub fn stop_blocking(self: &Arc<Self>) {
        if self.inner.lock().unwrap().status == Status::Disabled {
            return;
        }
        let (done, wait) = std::sync::mpsc::sync_channel(1);
        let manager = self.clone();
        tauri::async_runtime::spawn(async move {
            manager.stop().await;
            let _ = done.send(());
        });
        let _ = wait.recv_timeout(self.timing.stop * 2 + Duration::from_millis(500));
    }

    /// Admin commands carry no request id: run one at a time and match the
    /// reply by its event type and device, or take the next error.
    async fn request(&self, frame: Value, want: Want) -> Result<Value, String> {
        let _turn = self.commands.lock().await;
        let reply = {
            let mut inner = self.inner.lock().unwrap();
            let Some(process) = inner.process.as_ref().filter(|_| inner.ready) else {
                return Err("companion-not-running".into());
            };
            let mut bytes = serde_json::to_vec(&frame).map_err(|e| e.to_string())?;
            bytes.push(b'\n');
            process
                .input
                .send(bytes)
                .map_err(|_| "companion-not-running".to_string())?;
            let (reply, wait) = oneshot::channel();
            inner.pending = Some(Pending { want, reply });
            wait
        };
        match tokio::time::timeout(self.timing.command, reply).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err("companion-exited".into()),
            Err(_) => {
                self.inner.lock().unwrap().pending = None;
                Err("companion-timeout".into())
            }
        }
    }

    /// Returns the pairing id, URL and expiry. The URL carries the secret:
    /// callers turn it into a QR code and drop it.
    pub async fn pairing_create(&self, add_to_device: Option<String>) -> Result<Value, String> {
        self.request(
            json!({"v": 1, "type": "pairing.create", "path": "tailscale", "addToDevice": add_to_device}),
            Want::Pairing,
        )
        .await
    }

    /// Approve, reject and cancel report back through `pairing.state` events.
    pub fn pairing_decide(&self, kind: &str, pairing_id: &str) -> Result<(), String> {
        let inner = self.inner.lock().unwrap();
        let process = inner
            .process
            .as_ref()
            .filter(|_| inner.ready)
            .ok_or("companion-not-running")?;
        let frame = json!({"v": 1, "type": kind, "pairingId": pairing_id});
        let mut bytes = serde_json::to_vec(&frame).map_err(|e| e.to_string())?;
        bytes.push(b'\n');
        process
            .input
            .send(bytes)
            .map_err(|_| "companion-not-running".to_string())
    }

    /// `device.update`, `device.revoke` and `device.unlock`; resolves once the
    /// change is persisted, or when the gateway defers its notification.
    pub async fn device_command(&self, mut frame: Value) -> Result<Value, String> {
        let id = frame["deviceId"]
            .as_str()
            .ok_or("companion-device-invalid")?
            .to_owned();
        let done = match frame["type"].as_str() {
            Some("device.revoke") => "device.revoked",
            Some("device.update" | "device.unlock") => "device.updated",
            _ => return Err("companion-device-invalid".into()),
        };
        frame["v"] = json!(1);
        let want = Want::Device {
            id,
            done,
            frame: frame.clone(),
        };
        self.request(frame, want).await
    }
}

fn resolve(inner: &mut Inner, matches: impl Fn(&Want) -> bool, result: Result<Value, String>) {
    if inner.pending.as_ref().is_some_and(|p| matches(&p.want)) {
        let _ = inner.pending.take().unwrap().reply.send(result);
    }
}

fn halt(inner: &mut Inner, code: &str) {
    if let Some(pending) = inner.pending.take() {
        let _ = pending.reply.send(Err(code.into()));
    }
    let status = Status::Failed { code: code.into() };
    inner.terminal = Some(status.clone());
    inner.status = status;
    if let Some(kill) = inner.process.as_mut().and_then(|p| p.kill.take()) {
        let _ = kill.send(());
    }
}

fn status_event(status: &Status) -> Value {
    json!({"type": "status", "status": status})
}

/// What the UI may see of a device: no credential material.
pub fn device_summary(device: &Value) -> Value {
    json!({
        "deviceId": device["deviceId"],
        "label": device["label"],
        "grants": device["grants"],
        "locked": device["locked"],
        "createdAt": device["createdAt"],
        "lastSeen": device["lastSeen"],
    })
}

fn log_stderr(line: &str) {
    let message: String = line.chars().take(500).collect();
    crate::logging::write_global(crate::logging::LogEvent {
        level: "warn".to_string(),
        kind: "debug".to_string(),
        source: "companion".to_string(),
        workspace_path: None,
        event: "companion_stderr".to_string(),
        message,
        metadata: json!({}),
    });
}

#[cfg(test)]
mod tests;
