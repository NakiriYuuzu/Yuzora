// M3 Task 4: LSP process manager — spawn / framing / lifecycle / commands.
//
// Responsibility boundary: Rust only owns process lifecycle and stdio byte
// shuttling (Content-Length JSON-RPC framing). It never parses LSP semantics —
// the `initialize` handshake and all feature requests live in the frontend
// client. Hence there is no "Ready" status here; Ready is derived by the
// frontend after it receives the `initialize` response.
//
// Spike traps (recorded here per the M3 plan):
//   #1 Always spawn via an absolute path resolved by `which` (search
//      ~/.yuzora/servers/ bin dirs first, then PATH). A relative command name is
//      resolved against the child's cwd and yields ENOENT.
//   #6 rust-analyzer cold start can take ~10s; the frontend Ready decision must
//      allow generous time (status bar acceptance line is Ready within 30s).

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use crate::{logging, lsp_adapters, lsp_config};

mod framing;
// Preserve the historical `lsp_service::{frame, parse_frames, which}` paths that
// the manager, lsp_download, and env_path all reference.
pub use framing::{frame, parse_frames, which};

const MAX_RESTARTS: u32 = 3;
const POLL_MS: u64 = 50;
const STDERR_KEEP_LINES: usize = 20;
const TRACE_KEEP: usize = 5;
const TRACE_MAX_BYTES: u64 = 50 * 1024 * 1024;

// ---- wire contract (camelCase; T5/T6 depend on these exact keys) ------------

#[derive(Clone, serde::Serialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "status"
)]
pub enum LspProcessStatus {
    Starting,
    Missing { install_hint: String },
    Crashed { reason: String },
    Stopped,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LspServerInfo {
    // Raw workspace key (as passed to lsp_start / the servers map key). Lets the
    // frontend LspBridge route a "lsp:server-status" emit to the right workspace so
    // a late Crashed from an old workspace can't pollute a newly-switched one (S1).
    pub workspace: String,
    pub language: String,
    pub server_id: String,
    pub command: String,
    pub path: Option<String>,
    pub status: LspProcessStatus,
    pub last_startup_log: Option<String>,
    pub last_error: Option<String>,
    pub restart_count: u32,
}

// ---- manager ----------------------------------------------------------------

type LogFn = Box<dyn Fn(logging::LogEvent) + Send + Sync>;
type EmitFn = Box<dyn Fn(LspServerInfo) + Send + Sync>;
type OnMessage = Arc<dyn Fn(String) + Send + Sync>;

pub struct ResolvedServer {
    server_id: String,
    command: String,
    args: Vec<String>,
    install_hint: String,
}

struct ServerShared {
    workspace: String,
    language: String,
    resolved: ResolvedServer,
    path: String,
    on_message: OnMessage,
    info: Mutex<LspServerInfo>,
    stdin: Mutex<Option<std::process::ChildStdin>>,
    child: Mutex<Option<std::process::Child>>,
    stopped: AtomicBool,
    pid: AtomicU32,
}

#[derive(Default)]
struct TraceState {
    enabled: bool,
    file: Option<std::fs::File>,
}

pub struct LspManager {
    servers: Mutex<HashMap<(String, String), Arc<ServerShared>>>,
    trace: Mutex<TraceState>,
    trace_dir: PathBuf,
    backoff_ms: u64,
    log: LogFn,
    emit: EmitFn,
}

pub struct LspState(pub Arc<LspManager>);

impl LspManager {
    pub fn new(app: tauri::AppHandle) -> Self {
        let sink = Mutex::new(logging::LogSink::new(logging::default_log_dir()));
        let log: LogFn = Box::new(move |ev| {
            if let Ok(mut s) = sink.lock() {
                s.write(ev);
            }
        });
        let emit: EmitFn = Box::new(move |info| {
            use tauri::Emitter;
            let _ = app.emit("lsp:server-status", info);
        });
        Self::with_parts(logging::default_log_dir(), 500, log, emit)
    }

    fn with_parts(trace_dir: PathBuf, backoff_ms: u64, log: LogFn, emit: EmitFn) -> Self {
        Self {
            servers: Mutex::new(HashMap::new()),
            trace: Mutex::new(TraceState::default()),
            trace_dir,
            backoff_ms,
            log,
            emit,
        }
    }

    #[cfg(test)]
    pub fn debug_pid(&self, workspace: &str, language: &str) -> Option<u32> {
        self.servers
            .lock()
            .unwrap()
            .get(&(workspace.to_string(), language.to_string()))
            .map(|s| s.pid.load(Ordering::SeqCst))
            .filter(|p| *p != 0)
    }

    pub fn start(
        self: &Arc<Self>,
        workspace: &str,
        language: &str,
        resolved: ResolvedServer,
        on_message: OnMessage,
    ) -> LspServerInfo {
        let key = (workspace.to_string(), language.to_string());

        // Spike trap #1: resolve to an absolute path or report Missing (no spawn).
        // Resolved before reserving so a Missing result never touches the map.
        let path = match which(&resolved.command) {
            Some(p) => p,
            None => {
                return LspServerInfo {
                    workspace: workspace.to_string(),
                    language: language.to_string(),
                    server_id: resolved.server_id,
                    command: resolved.command,
                    path: None,
                    status: LspProcessStatus::Missing {
                        install_hint: resolved.install_hint,
                    },
                    last_startup_log: None,
                    last_error: None,
                    restart_count: 0,
                };
            }
        };

        let info = LspServerInfo {
            workspace: workspace.to_string(),
            language: language.to_string(),
            server_id: resolved.server_id.clone(),
            command: resolved.command.clone(),
            path: Some(path.clone()),
            status: LspProcessStatus::Starting,
            last_startup_log: None,
            last_error: None,
            restart_count: 0,
        };
        let shared = Arc::new(ServerShared {
            workspace: workspace.to_string(),
            language: language.to_string(),
            resolved,
            path,
            on_message,
            info: Mutex::new(info),
            stdin: Mutex::new(None),
            child: Mutex::new(None),
            stopped: AtomicBool::new(false),
            pid: AtomicU32::new(0),
        });

        // Check-and-reserve atomically (F1): one critical section covers both the
        // existence check and the insert, so two concurrent starts for the same
        // (ws,lang) can never both spawn. The loser returns the reserved info and
        // never spawns; the single-server guarantee holds under concurrency.
        {
            let mut map = self.servers.lock().unwrap();
            let existing_crashed = match map.get(&key) {
                Some(existing) => {
                    let crashed = matches!(
                        existing.info.lock().unwrap().status,
                        LspProcessStatus::Crashed { .. }
                    );
                    // A live/reserved entry stands (return its snapshot). A Crashed
                    // entry is a dead terminal state — its watch_loop has already
                    // returned — so an explicit new lsp_start clears it and starts
                    // fresh (F-A/F-B), rather than echoing the stale crash forever.
                    if !crashed {
                        return existing.info.lock().unwrap().clone();
                    }
                    true
                }
                None => false,
            };
            if existing_crashed {
                map.remove(&key);
            }
            map.insert(key.clone(), shared.clone());
        }

        // Spawn outside the servers lock so other languages aren't blocked.
        if let Err(e) = self.spawn_child(&shared) {
            // F-B: keep the reserved entry but mark it Crashed so a concurrent
            // loser's lsp_status sees the failure; the next explicit lsp_start
            // clears it via the F-A crash-retry path.
            let info = {
                let mut i = shared.info.lock().unwrap();
                i.status = LspProcessStatus::Crashed { reason: e };
                i.clone()
            };
            // F-R4-3: emit so a concurrent loser (who got the Starting snapshot)
            // and the UI get the corrected status, matching crash-backoff.
            (self.emit)(info.clone());
            return info;
        }

        // F-E: a stop that raced this spawn found child=None and could not kill it.
        // Now that the child exists, honor the stop here — kill+reap it and skip
        // the watcher — otherwise a long-lived server would never self-exit and the
        // process + watch_loop thread would leak permanently.
        if shared.stopped.load(Ordering::SeqCst) {
            Self::stop_shared(&shared);
            return shared.info.lock().unwrap().clone();
        }

        // Workers hold a Weak, not a strong Arc, so the manager's Drop (F-R4-2) can
        // run at shutdown — a strong clone here would pin the Arc for the app's
        // lifetime and every server would be orphaned. Mirrors the askpass server.
        let weak = Arc::downgrade(self);
        let sh = shared.clone();
        std::thread::spawn(move || LspManager::watch_loop(weak, sh));

        let info = shared.info.lock().unwrap().clone();
        info
    }

    pub fn send(&self, workspace: &str, language: &str, message: String) -> Result<(), String> {
        let shared = self
            .servers
            .lock()
            .unwrap()
            .get(&(workspace.to_string(), language.to_string()))
            .cloned()
            .ok_or_else(|| format!("no LSP server for {workspace}/{language}"))?;
        let bytes = frame(&message);
        {
            let mut guard = shared.stdin.lock().unwrap();
            let stdin = guard.as_mut().ok_or("server stdin unavailable")?;
            stdin
                .write_all(&bytes)
                .map_err(|e| format!("write failed: {e}"))?;
            stdin.flush().map_err(|e| format!("flush failed: {e}"))?;
        }
        self.trace_message("out", workspace, language, &message);
        Ok(())
    }

    pub fn stop_workspace(&self, workspace: &str) {
        let keys: Vec<(String, String)> = {
            let map = self.servers.lock().unwrap();
            map.keys()
                .filter(|(w, _)| w == workspace)
                .cloned()
                .collect()
        };
        for key in keys {
            self.stop_server(&key);
        }
    }

    /// Stop any running server whose language matches; when `workspace` is Some,
    /// only those whose (raw or canonical) workspace path matches.
    fn stop_matching(&self, workspace: Option<&str>, language: &str) {
        let keys: Vec<(String, String)> = {
            let map = self.servers.lock().unwrap();
            map.keys()
                .filter(|(w, l)| {
                    l == language
                        && match workspace {
                            None => true,
                            Some(ws) => {
                                w == ws || lsp_config::canonicalize(w).as_deref() == Some(ws)
                            }
                        }
                })
                .cloned()
                .collect()
        };
        for key in keys {
            self.stop_server(&key);
        }
    }

    fn stop_server(&self, key: &(String, String)) {
        let shared = self.servers.lock().unwrap().remove(key);
        if let Some(shared) = shared {
            Self::stop_shared(&shared);
        }
    }

    pub fn stop_all(&self) {
        let servers: Vec<Arc<ServerShared>> = self
            .servers
            .lock()
            .unwrap()
            .drain()
            .map(|(_, shared)| shared)
            .collect();
        for shared in servers {
            Self::stop_shared(&shared);
        }
    }

    fn stop_shared(shared: &Arc<ServerShared>) {
        shared.stopped.store(true, Ordering::SeqCst);
        shared.stdin.lock().unwrap().take();
        if let Some(mut child) = shared.child.lock().unwrap().take() {
            match child.try_wait() {
                Ok(Some(_)) => {}
                Ok(None) | Err(_) => {
                    let _ = crate::process_kill::kill_tree(&mut child);
                }
            }
        }
        shared.info.lock().unwrap().status = LspProcessStatus::Stopped;
    }

    pub fn status(&self, workspace: &str) -> Vec<LspServerInfo> {
        self.servers
            .lock()
            .unwrap()
            .iter()
            .filter(|((w, _), _)| w == workspace)
            .map(|(_, sh)| sh.info.lock().unwrap().clone())
            .collect()
    }

    fn spawn_child(self: &Arc<Self>, shared: &Arc<ServerShared>) -> Result<(), String> {
        let mut cmd = std::process::Command::new(&shared.path);
        cmd.args(&shared.resolved.args)
            .current_dir(&shared.workspace)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        crate::process_kill::configure_background_process(&mut cmd);
        let mut child = cmd.spawn().map_err(|e| format!("spawn failed: {e}"))?;
        shared.pid.store(child.id(), Ordering::SeqCst);

        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        *shared.stdin.lock().unwrap() = stdin;

        if let Some(stdout) = stdout {
            // Weak so the reader never pins the manager past shutdown (F-R4-2).
            let weak = Arc::downgrade(self);
            let sh = shared.clone();
            std::thread::spawn(move || LspManager::reader_loop(weak, sh, stdout));
        }
        if let Some(stderr) = stderr {
            let sh = shared.clone();
            std::thread::spawn(move || stderr_loop(sh, stderr));
        }
        *shared.child.lock().unwrap() = Some(child);
        self.log_spawn(shared);
        Ok(())
    }

    fn reader_loop(
        manager: Weak<Self>,
        shared: Arc<ServerShared>,
        mut stdout: std::process::ChildStdout,
    ) {
        let mut buf: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            match stdout.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    buf.extend_from_slice(&chunk[..n]);
                    for body in parse_frames(&mut buf) {
                        // Upgrade briefly; if the manager is gone (shutdown) just skip
                        // tracing — the child will EOF shortly once Drop kills it.
                        if let Some(mgr) = manager.upgrade() {
                            mgr.trace_message("in", &shared.workspace, &shared.language, &body);
                        }
                        // Healthy again once a message flows: reset the backoff.
                        shared.info.lock().unwrap().restart_count = 0;
                        (shared.on_message)(body);
                    }
                }
            }
        }
    }

    /// Single owner of a server's restart lifecycle. Waits for the current child
    /// to exit, then either stops (on request), retries with backoff, or gives up
    /// with Crashed after MAX_RESTARTS.
    fn watch_loop(manager: Weak<Self>, shared: Arc<ServerShared>) {
        loop {
            let status = loop {
                let exited = {
                    let mut guard = shared.child.lock().unwrap();
                    match guard.as_mut() {
                        Some(child) => match child.try_wait() {
                            Ok(Some(s)) => Some(Some(s)),
                            Ok(None) => None,
                            Err(_) => Some(None),
                        },
                        None => Some(None),
                    }
                };
                match exited {
                    Some(s) => break s,
                    None => std::thread::sleep(Duration::from_millis(POLL_MS)),
                }
            };

            if shared.stopped.load(Ordering::SeqCst) {
                return; // stop_server already set status Stopped
            }

            // Upgrade only for the manager-dependent tail; the strong Arc is dropped
            // at the end of each iteration so the inner wait-loop above holds nothing
            // and the manager's Drop (F-R4-2) stays reachable while the server runs.
            // If the manager is already gone, Drop has killed the child — stop here.
            let Some(mgr) = manager.upgrade() else {
                return;
            };

            let count = {
                let mut i = shared.info.lock().unwrap();
                i.restart_count += 1;
                i.restart_count
            };
            mgr.log_exit(&shared, status.as_ref());

            if count >= MAX_RESTARTS {
                // Mirror the retry branch (F4): a concurrent stop must win — do not
                // overwrite Stopped with Crashed nor emit a spurious status.
                if shared.stopped.load(Ordering::SeqCst) {
                    return;
                }
                let info = {
                    let mut i = shared.info.lock().unwrap();
                    let reason = i.last_error.clone().unwrap_or_else(|| match &status {
                        Some(s) => format!("exited: {s}"),
                        None => "process exited".to_string(),
                    });
                    i.status = LspProcessStatus::Crashed { reason };
                    i.clone()
                };
                (mgr.emit)(info);
                return;
            }

            std::thread::sleep(Duration::from_millis(mgr.backoff_ms * count as u64));
            if shared.stopped.load(Ordering::SeqCst) {
                return;
            }
            if let Err(e) = mgr.spawn_child(&shared) {
                let info = {
                    let mut i = shared.info.lock().unwrap();
                    i.status = LspProcessStatus::Crashed { reason: e };
                    i.clone()
                };
                (mgr.emit)(info);
                return;
            }

            // F-R3-1: mirror start()'s F-E guard. A stop that raced this respawn
            // found child=None inside spawn_child and killed only the already-reaped
            // old child. Re-check stopped now that the new child exists — kill+reap
            // it and end the watch_loop — else a healthy respawned server never
            // self-exits and its process + reader/stderr/watch threads leak.
            if shared.stopped.load(Ordering::SeqCst) {
                Self::stop_shared(&shared);
                return;
            }
        }
    }

    fn log_spawn(&self, shared: &ServerShared) {
        (self.log)(logging::LogEvent {
            level: "info".into(),
            kind: "debug".into(),
            source: "lsp".into(),
            workspace_path: Some(shared.workspace.clone()),
            event: "lsp_spawn".into(),
            message: format!("spawn {} → {}", shared.resolved.server_id, shared.path),
            metadata: serde_json::json!({
                "language": shared.language,
                "command": shared.resolved.command,
                "path": shared.path,
                "pid": shared.pid.load(Ordering::SeqCst),
            }),
        });
    }

    fn log_exit(&self, shared: &ServerShared, status: Option<&std::process::ExitStatus>) {
        let code = status.and_then(|s| s.code());
        // Summary only — never the full stderr content (sync spec Log chapter).
        let stderr_summary = shared
            .info
            .lock()
            .unwrap()
            .last_error
            .clone()
            .map(|e| e.chars().take(200).collect::<String>());
        (self.log)(logging::LogEvent {
            level: "warn".into(),
            kind: "debug".into(),
            source: "lsp".into(),
            workspace_path: Some(shared.workspace.clone()),
            event: "lsp_exit".into(),
            message: format!("{} exited (code {code:?})", shared.resolved.server_id),
            metadata: serde_json::json!({
                "language": shared.language,
                "exitCode": code,
                "stderr": stderr_summary,
            }),
        });
    }

    /// A8: JSON-RPC trace lands in its own file, off by default and reset to off
    /// on app restart. Enabling opens a fresh ~/.yuzora/logs/lsp-trace-<ts>.jsonl
    /// and writes a debug metric to the main log referencing its path.
    pub fn set_trace(&self, enabled: bool) -> Result<(), String> {
        let mut t = self.trace.lock().unwrap();
        if !enabled {
            t.enabled = false;
            t.file = None;
            return Ok(());
        }
        std::fs::create_dir_all(&self.trace_dir).ok();
        let ts = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let path = self.trace_dir.join(format!("lsp-trace-{ts}.jsonl"));
        // On open failure (F3): report Err and write no enabled metric — do not
        // claim tracing is on when nothing will be captured.
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| format!("failed to open trace file {}: {e}", path.display()))?;
        t.enabled = true;
        t.file = Some(file);
        drop(t);
        self.cleanup_traces();
        (self.log)(logging::LogEvent {
            level: "info".into(),
            kind: "debug".into(),
            source: "lsp".into(),
            workspace_path: None,
            event: "lsp_trace_enabled".into(),
            message: format!("LSP JSON-RPC trace → {}", path.display()),
            metadata: serde_json::json!({ "path": path.to_string_lossy() }),
        });
        Ok(())
    }

    fn trace_message(&self, dir: &str, workspace: &str, language: &str, body: &str) {
        let mut t = self.trace.lock().unwrap();
        if !t.enabled {
            return;
        }
        if let Some(file) = t.file.as_mut() {
            let line = serde_json::json!({
                "ts": chrono::Local::now().to_rfc3339(),
                "dir": dir,
                "workspace": workspace,
                "language": language,
                "body": body,
            });
            let _ = writeln!(file, "{line}");
        }
    }

    fn cleanup_traces(&self) {
        let mut files: Vec<(PathBuf, u64)> = vec![];
        if let Ok(rd) = std::fs::read_dir(&self.trace_dir) {
            for e in rd.flatten() {
                let p = e.path();
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if name.starts_with("lsp-trace-") && name.ends_with(".jsonl") {
                    let size = e.metadata().map(|m| m.len()).unwrap_or(0);
                    files.push((p, size));
                }
            }
        }
        // Timestamped names sort chronologically; keep only the newest TRACE_KEEP.
        files.sort();
        while files.len() > TRACE_KEEP {
            let (old, _) = files.remove(0);
            let _ = std::fs::remove_file(old);
        }
        // Total-size cap: drop oldest until under the limit.
        let mut total: u64 = files.iter().map(|(_, s)| s).sum();
        let mut i = 0;
        while total > TRACE_MAX_BYTES && i < files.len() {
            let (path, size) = &files[i];
            let _ = std::fs::remove_file(path);
            total -= size;
            i += 1;
        }
    }
}

impl Drop for LspManager {
    /// Best-effort reap on app shutdown (F-R4-2): std `Child`'s own drop does NOT
    /// kill the process, so without this every running server (rust-analyzer can
    /// hold hundreds of MB) would be orphaned when the manager is dropped. Worker
    /// threads hold only `Weak<LspManager>`, so this drop is actually reachable
    /// while servers run. A SIGKILL/force-quit of the app itself cannot run any
    /// destructor and leaking there is a known, unavoidable limit.
    fn drop(&mut self) {
        let servers = match self.servers.lock() {
            Ok(mut map) => map.drain().map(|(_, shared)| shared).collect::<Vec<_>>(),
            Err(_) => return,
        };
        for shared in servers {
            Self::stop_shared(&shared);
        }
    }
}

fn stderr_loop(shared: Arc<ServerShared>, stderr: std::process::ChildStderr) {
    let reader = BufReader::new(stderr);
    let mut kept: Vec<String> = Vec::new();
    for line in reader.lines() {
        let Ok(line) = line else { break };
        if kept.len() < STDERR_KEEP_LINES {
            kept.push(line);
            let joined = kept.join("\n");
            let mut i = shared.info.lock().unwrap();
            i.last_startup_log = Some(joined.clone());
            i.last_error = Some(joined);
        }
    }
}

// ---- tauri commands (thin: resolve config/adapter, bridge Channel) ----------

fn detect_server_info_from(
    cfg: &lsp_config::LspConfig,
    workspace: Option<&str>,
    language: &str,
    active: Option<LspServerInfo>,
    resolve_command: impl Fn(&str) -> Option<String>,
) -> Result<LspServerInfo, String> {
    if let Some(info) = active {
        return Ok(info);
    }

    let ws_canonical =
        workspace.map(|path| lsp_config::canonicalize(path).unwrap_or_else(|| path.to_string()));
    let configured_id = match ws_canonical.as_deref() {
        Some(workspace) => lsp_config::resolve_server(cfg, workspace, language),
        None => cfg.defaults.get(language).cloned(),
    };
    let server_id = configured_id
        .or_else(|| lsp_adapters::adapters_for(language).map(|a| a.default_id.to_string()))
        .ok_or_else(|| format!("no LSP adapter for language {language}"))?;
    let adapter = lsp_adapters::adapter(language, &server_id)
        .ok_or_else(|| format!("unknown server {server_id} for {language}"))?;
    let path = resolve_command(adapter.command);
    let status = if path.is_some() {
        LspProcessStatus::Stopped
    } else {
        LspProcessStatus::Missing {
            install_hint: adapter.install_hint.to_string(),
        }
    };

    Ok(LspServerInfo {
        workspace: workspace.unwrap_or_default().to_string(),
        language: language.to_string(),
        server_id,
        command: adapter.command.to_string(),
        path,
        status,
        last_startup_log: None,
        last_error: None,
        restart_count: 0,
    })
}

#[tauri::command]
pub fn lsp_detect_server(
    state: tauri::State<'_, LspState>,
    workspace: Option<String>,
    language: String,
) -> Result<LspServerInfo, String> {
    let active = workspace.as_deref().and_then(|workspace| {
        state
            .0
            .status(workspace)
            .into_iter()
            .find(|info| info.language == language)
    });
    let cfg = lsp_config::load_from(&lsp_config::config_path());
    detect_server_info_from(&cfg, workspace.as_deref(), &language, active, which)
}

#[tauri::command]
pub fn lsp_start(
    state: tauri::State<'_, LspState>,
    workspace: String,
    language: String,
    on_message: tauri::ipc::Channel<String>,
) -> Result<LspServerInfo, String> {
    let manager = state.0.clone();
    let cfg = lsp_config::load_from(&lsp_config::config_path());
    let ws_canonical = lsp_config::canonicalize(&workspace).unwrap_or_else(|| workspace.clone());
    let id = lsp_config::resolve_server(&cfg, &ws_canonical, &language)
        .or_else(|| lsp_adapters::adapters_for(&language).map(|a| a.default_id.to_string()))
        .ok_or_else(|| format!("no LSP adapter for language {language}"))?;
    let adapter = lsp_adapters::adapter(&language, &id)
        .ok_or_else(|| format!("unknown server {id} for {language}"))?;
    let resolved = ResolvedServer {
        server_id: id,
        command: adapter.command.to_string(),
        args: adapter.args.iter().map(|s| s.to_string()).collect(),
        install_hint: adapter.install_hint.to_string(),
    };
    let channel = on_message;
    let on_message: OnMessage = Arc::new(move |body| {
        let _ = channel.send(body);
    });
    Ok(manager.start(&workspace, &language, resolved, on_message))
}

#[tauri::command]
pub fn lsp_send(
    state: tauri::State<'_, LspState>,
    workspace: String,
    language: String,
    message: String,
) -> Result<(), String> {
    state.0.send(&workspace, &language, message)
}

#[tauri::command]
pub fn lsp_stop_workspace(
    state: tauri::State<'_, LspState>,
    workspace: String,
) -> Result<(), String> {
    state.0.stop_workspace(&workspace);
    Ok(())
}

#[tauri::command]
pub fn lsp_status(
    state: tauri::State<'_, LspState>,
    workspace: String,
) -> Result<Vec<LspServerInfo>, String> {
    Ok(state.0.status(&workspace))
}

#[tauri::command]
pub fn lsp_config_get() -> Result<lsp_config::LspConfig, String> {
    Ok(lsp_config::load_from(&lsp_config::config_path()))
}

#[tauri::command]
pub fn lsp_config_set_server(
    state: tauri::State<'_, LspState>,
    workspace: Option<String>,
    language: String,
    server_id: String,
) -> Result<lsp_config::LspConfig, String> {
    let path = lsp_config::config_path();
    let mut cfg = lsp_config::load_from(&path);
    let ws_canonical = apply_set_server(&mut cfg, workspace.as_deref(), &language, &server_id);
    lsp_config::save_to(&path, &cfg)?;
    // Running server for this (ws,lang) is now stale — stop it; frontend restarts.
    state.0.stop_matching(ws_canonical.as_deref(), &language);
    Ok(cfg)
}

/// Write the (workspace,language)→server_id mapping and return the workspace key
/// actually used. F2: a `Some(workspace)` that cannot be canonicalized (e.g. a
/// path that no longer exists) falls back to the raw string — it must never
/// degrade to `None`, which would clobber the global defaults and stop every
/// server of that language. Mirrors the `lsp_start` raw fallback.
fn apply_set_server(
    cfg: &mut lsp_config::LspConfig,
    workspace: Option<&str>,
    language: &str,
    server_id: &str,
) -> Option<String> {
    let ws_canonical =
        workspace.map(|p| lsp_config::canonicalize(p).unwrap_or_else(|| p.to_string()));
    lsp_config::set_server(cfg, ws_canonical.as_deref(), language, server_id);
    ws_canonical
}

#[tauri::command]
pub fn lsp_config_stale() -> Result<Vec<String>, String> {
    Ok(lsp_config::stale_workspaces(&lsp_config::load_from(
        &lsp_config::config_path(),
    )))
}

#[tauri::command]
pub fn lsp_config_clear_stale(workspace: String) -> Result<lsp_config::LspConfig, String> {
    let path = lsp_config::config_path();
    let mut cfg = lsp_config::load_from(&path);
    lsp_config::clear_workspace(&mut cfg, &workspace);
    lsp_config::save_to(&path, &cfg)?;
    Ok(cfg)
}

#[tauri::command]
pub fn lsp_set_trace(state: tauri::State<'_, LspState>, enabled: bool) -> Result<(), String> {
    state.0.set_trace(enabled)
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- test harness: manager with capturing log/emit + injected trace dir ---

    struct Captured {
        logs: Arc<Mutex<Vec<logging::LogEvent>>>,
        emits: Arc<Mutex<Vec<LspServerInfo>>>,
    }

    fn test_manager(trace_dir: PathBuf, backoff_ms: u64) -> (Arc<LspManager>, Captured) {
        let logs: Arc<Mutex<Vec<logging::LogEvent>>> = Default::default();
        let emits: Arc<Mutex<Vec<LspServerInfo>>> = Default::default();
        let l2 = logs.clone();
        let e2 = emits.clone();
        let log: LogFn = Box::new(move |ev| l2.lock().unwrap().push(ev));
        let emit: EmitFn = Box::new(move |info| e2.lock().unwrap().push(info));
        let mgr = Arc::new(LspManager::with_parts(trace_dir, backoff_ms, log, emit));
        (mgr, Captured { logs, emits })
    }

    fn resolved(command: &str, args: &[&str]) -> ResolvedServer {
        ResolvedServer {
            server_id: "test".into(),
            command: command.into(),
            args: args.iter().map(|s| s.to_string()).collect(),
            install_hint: "install it".into(),
        }
    }

    fn noop_on_message() -> OnMessage {
        Arc::new(|_| {})
    }

    fn poll_until<F: Fn() -> bool>(timeout: Duration, f: F) -> bool {
        let start = std::time::Instant::now();
        while start.elapsed() < timeout {
            if f() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        f()
    }

    fn pid_alive(pid: u32) -> bool {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    // --- lifecycle ---

    #[test]
    fn start_missing_command_does_not_spawn() {
        let tmp = tempfile::tempdir().unwrap();
        let (mgr, _cap) = test_manager(tmp.path().to_path_buf(), 5);
        let info = mgr.start(
            tmp.path().to_str().unwrap(),
            "typescript",
            resolved("definitely-not-a-binary-xyz", &[]),
            noop_on_message(),
        );
        assert!(matches!(info.status, LspProcessStatus::Missing { .. }));
        assert!(info.path.is_none());
        // Not inserted into the map (no server was spawned).
        assert!(mgr.status(tmp.path().to_str().unwrap()).is_empty());
    }

    #[test]
    fn echo_through_cat_reaches_on_message() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().to_str().unwrap().to_string();
        let (mgr, _cap) = test_manager(tmp.path().to_path_buf(), 5);
        let got: Arc<Mutex<Vec<String>>> = Default::default();
        let g2 = got.clone();
        let on_message: OnMessage = Arc::new(move |b| g2.lock().unwrap().push(b));
        let info = mgr.start(&ws, "typescript", resolved("cat", &[]), on_message);
        assert!(matches!(info.status, LspProcessStatus::Starting));
        // cat echoes the framed bytes verbatim; the reader re-parses them.
        mgr.send(&ws, "typescript", "{\"echo\":true}".into())
            .unwrap();
        let ok = poll_until(Duration::from_secs(3), || !got.lock().unwrap().is_empty());
        assert!(ok, "on_message never received the echoed body");
        assert_eq!(got.lock().unwrap()[0], "{\"echo\":true}");
        mgr.stop_workspace(&ws);
    }

    #[test]
    fn stop_workspace_kills_process_and_clears_map() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().to_str().unwrap().to_string();
        let (mgr, _cap) = test_manager(tmp.path().to_path_buf(), 5);
        mgr.start(&ws, "python", resolved("sleep", &["30"]), noop_on_message());
        assert!(poll_until(Duration::from_secs(3), || mgr
            .debug_pid(&ws, "python")
            .is_some()));
        let pid = mgr.debug_pid(&ws, "python").unwrap();
        assert!(pid_alive(pid));
        mgr.stop_workspace(&ws);
        assert!(mgr.status(&ws).is_empty());
        assert!(
            poll_until(Duration::from_secs(3), || !pid_alive(pid)),
            "process {pid} still alive after stop"
        );
    }

    #[test]
    fn stop_all_kills_every_workspace_process_and_clears_map() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace_a = tmp.path().join("a");
        let workspace_b = tmp.path().join("b");
        std::fs::create_dir_all(&workspace_a).unwrap();
        std::fs::create_dir_all(&workspace_b).unwrap();
        let (mgr, _cap) = test_manager(tmp.path().to_path_buf(), 5);
        mgr.start(
            workspace_a.to_str().unwrap(),
            "python",
            resolved("sleep", &["30"]),
            noop_on_message(),
        );
        mgr.start(
            workspace_b.to_str().unwrap(),
            "rust",
            resolved("sleep", &["30"]),
            noop_on_message(),
        );
        assert!(poll_until(Duration::from_secs(3), || {
            mgr.debug_pid(workspace_a.to_str().unwrap(), "python")
                .is_some()
                && mgr
                    .debug_pid(workspace_b.to_str().unwrap(), "rust")
                    .is_some()
        }));
        let pids = [
            mgr.debug_pid(workspace_a.to_str().unwrap(), "python")
                .unwrap(),
            mgr.debug_pid(workspace_b.to_str().unwrap(), "rust")
                .unwrap(),
        ];

        mgr.stop_all();

        assert!(mgr.status(workspace_a.to_str().unwrap()).is_empty());
        assert!(mgr.status(workspace_b.to_str().unwrap()).is_empty());
        for pid in pids {
            assert!(
                poll_until(Duration::from_secs(3), || !pid_alive(pid)),
                "process {pid} still alive after stop_all"
            );
        }
    }

    #[test]
    fn drop_manager_reaps_running_servers() {
        // F-R4-2: dropping the manager (app shutdown) must kill running servers, not
        // orphan them. Workers hold only Weak, so the last strong Arc dropping here
        // actually runs LspManager::drop.
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().to_str().unwrap().to_string();
        let (mgr, _cap) = test_manager(tmp.path().to_path_buf(), 5);
        mgr.start(&ws, "python", resolved("sleep", &["30"]), noop_on_message());
        assert!(poll_until(Duration::from_secs(3), || mgr
            .debug_pid(&ws, "python")
            .is_some()));
        let pid = mgr.debug_pid(&ws, "python").unwrap();
        assert!(pid_alive(pid));
        drop(mgr);
        assert!(
            poll_until(Duration::from_secs(3), || !pid_alive(pid)),
            "process {pid} still alive after manager drop"
        );
    }

    #[test]
    fn crash_backoff_reaches_crashed_after_max_restarts() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().to_str().unwrap().to_string();
        let (mgr, cap) = test_manager(tmp.path().to_path_buf(), 5);
        mgr.start(&ws, "rust", resolved("false", &[]), noop_on_message());
        let crashed = poll_until(Duration::from_secs(5), || {
            mgr.status(&ws)
                .first()
                .map(|i| matches!(i.status, LspProcessStatus::Crashed { .. }))
                .unwrap_or(false)
        });
        assert!(crashed, "server never reached Crashed");
        let info = &mgr.status(&ws)[0];
        assert_eq!(info.restart_count, MAX_RESTARTS);
        assert!(
            !cap.emits.lock().unwrap().is_empty(),
            "no lsp:server-status emitted on crash"
        );
    }

    #[test]
    fn start_after_crashed_entry_respawns_fresh() {
        // F-A: once a server reaches Crashed it lingers in the map; a subsequent
        // lsp_start must clear it and spawn fresh (restart_count back to 0), not
        // echo the stale Crashed snapshot.
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().to_str().unwrap().to_string();
        let (mgr, _cap) = test_manager(tmp.path().to_path_buf(), 5);
        mgr.start(&ws, "rust", resolved("false", &[]), noop_on_message());
        assert!(poll_until(Duration::from_secs(5), || mgr
            .status(&ws)
            .first()
            .map(|i| matches!(i.status, LspProcessStatus::Crashed { .. }))
            .unwrap_or(false)));

        // Retry with a live command → fresh spawn.
        let info = mgr.start(&ws, "rust", resolved("cat", &[]), noop_on_message());
        assert!(matches!(info.status, LspProcessStatus::Starting));
        assert_eq!(info.restart_count, 0);
        assert_eq!(mgr.status(&ws).len(), 1);
        assert!(poll_until(Duration::from_secs(3), || mgr
            .debug_pid(&ws, "rust")
            .is_some()));
        mgr.stop_workspace(&ws);
    }

    #[test]
    fn spawn_failure_keeps_crashed_entry_then_retry_succeeds() {
        // F-B: a spawn failure (nonexistent cwd, while `which` still resolves the
        // binary) keeps a Crashed entry in the map rather than removing it; a later
        // start (after the cwd exists) takes the F-A retry path and succeeds.
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().join("later-created");
        let ws = ws.to_str().unwrap().to_string();
        let (mgr, cap) = test_manager(tmp.path().to_path_buf(), 5);

        let info = mgr.start(&ws, "typescript", resolved("cat", &[]), noop_on_message());
        assert!(
            matches!(info.status, LspProcessStatus::Crashed { .. }),
            "spawn into a missing cwd should fail"
        );
        let st = mgr.status(&ws);
        assert_eq!(st.len(), 1, "Crashed entry must be retained (F-B)");
        assert!(matches!(st[0].status, LspProcessStatus::Crashed { .. }));

        // F-R4-3: the spawn failure must be emitted so a concurrent loser / the UI
        // gets the corrected status.
        assert!(
            cap.emits
                .lock()
                .unwrap()
                .iter()
                .any(|i| matches!(i.status, LspProcessStatus::Crashed { .. })),
            "spawn failure did not emit a Crashed status"
        );

        // Make the cwd exist, then retry → fresh spawn succeeds.
        std::fs::create_dir_all(&ws).unwrap();
        let info2 = mgr.start(&ws, "typescript", resolved("cat", &[]), noop_on_message());
        assert!(matches!(info2.status, LspProcessStatus::Starting));
        assert_eq!(mgr.status(&ws).len(), 1);
        mgr.stop_workspace(&ws);
    }

    #[test]
    fn concurrent_start_spawns_exactly_once() {
        // F1: two concurrent lsp_start for the same (ws,lang) must not both spawn.
        // A slow spawn (sleep before cat) widens the race window; the atomic
        // check-and-reserve still yields exactly one server and one spawn — the
        // other thread returns the reserved entry. `lsp_spawn` log events are a
        // precise leak detector: a duplicated (untracked, un-stoppable) process
        // would show a second spawn.
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().to_str().unwrap().to_string();
        let (mgr, cap) = test_manager(tmp.path().to_path_buf(), 5);

        let m1 = mgr.clone();
        let ws1 = ws.clone();
        let m2 = mgr.clone();
        let ws2 = ws.clone();
        let t1 = std::thread::spawn(move || {
            m1.start(
                &ws1,
                "typescript",
                resolved("sh", &["-c", "sleep 0.2; cat"]),
                noop_on_message(),
            );
        });
        let t2 = std::thread::spawn(move || {
            m2.start(
                &ws2,
                "typescript",
                resolved("sh", &["-c", "sleep 0.2; cat"]),
                noop_on_message(),
            );
        });
        t1.join().unwrap();
        t2.join().unwrap();

        assert_eq!(mgr.status(&ws).len(), 1, "more than one server entry");
        let spawns = cap
            .logs
            .lock()
            .unwrap()
            .iter()
            .filter(|e| e.event == "lsp_spawn")
            .count();
        assert_eq!(spawns, 1, "expected exactly one spawn, got {spawns}");
        mgr.stop_workspace(&ws);
    }

    // --- trace tee ---

    #[test]
    fn trace_on_writes_file_and_main_log_metric() {
        let tmp = tempfile::tempdir().unwrap();
        let (mgr, cap) = test_manager(tmp.path().to_path_buf(), 5);
        mgr.set_trace(true).unwrap();
        mgr.trace_message("in", "/ws", "typescript", "{\"m\":1}");

        // Trace file written under the injected dir.
        let files: Vec<PathBuf> = std::fs::read_dir(tmp.path())
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.starts_with("lsp-trace-") && n.ends_with(".jsonl"))
                    .unwrap_or(false)
            })
            .collect();
        assert_eq!(files.len(), 1, "expected one trace file");
        let content = std::fs::read_to_string(&files[0]).unwrap();
        let v: serde_json::Value = serde_json::from_str(content.trim()).unwrap();
        assert_eq!(v["dir"], "in");
        assert_eq!(v["language"], "typescript");
        assert_eq!(v["body"], "{\"m\":1}");

        // Main-log metric event referencing the trace path.
        let logs = cap.logs.lock().unwrap();
        let metric = logs
            .iter()
            .find(|e| e.event == "lsp_trace_enabled")
            .expect("no trace metric event");
        assert_eq!(metric.kind, "debug");
        assert!(metric.message.contains("lsp-trace-"));
    }

    #[test]
    fn trace_off_writes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let (mgr, _cap) = test_manager(tmp.path().to_path_buf(), 5);
        // trace defaults to off
        mgr.trace_message("in", "/ws", "typescript", "{\"m\":1}");
        let any = std::fs::read_dir(tmp.path())
            .map(|rd| rd.flatten().count())
            .unwrap_or(0);
        assert_eq!(any, 0, "trace file written while trace disabled");
    }

    #[test]
    fn set_trace_open_failure_errs_without_metric() {
        // F3: a file at the trace_dir path makes create_dir_all + open fail. The
        // call must return Err and write no lsp_trace_enabled metric.
        let tmp = tempfile::tempdir().unwrap();
        let blocker = tmp.path().join("blocker");
        std::fs::write(&blocker, "x").unwrap();
        let (mgr, cap) = test_manager(blocker, 5);
        assert!(mgr.set_trace(true).is_err());
        assert!(
            cap.logs
                .lock()
                .unwrap()
                .iter()
                .all(|e| e.event != "lsp_trace_enabled"),
            "trace-enabled metric written despite open failure"
        );
    }

    // --- config set-server fallback ---

    #[test]
    fn detect_installed_but_not_started_returns_stopped_with_path() {
        let mut cfg = lsp_config::LspConfig::default();
        cfg.defaults
            .insert("typescript".into(), "typescript-language-server".into());

        let info = detect_server_info_from(&cfg, None, "typescript", None, |command| {
            (command == "typescript-language-server")
                .then(|| "/managed/typescript-language-server".to_string())
        })
        .unwrap();

        assert_eq!(info.server_id, "typescript-language-server");
        assert_eq!(
            info.path.as_deref(),
            Some("/managed/typescript-language-server")
        );
        assert!(matches!(info.status, LspProcessStatus::Stopped));
    }

    #[test]
    fn detect_uses_workspace_override_and_reports_missing_without_spawning() {
        let mut cfg = lsp_config::LspConfig::default();
        cfg.defaults.insert("python".into(), "pyright".into());
        cfg.workspaces.insert(
            "/workspace".into(),
            std::collections::BTreeMap::from([("python".into(), "pylsp".into())]),
        );

        let info =
            detect_server_info_from(&cfg, Some("/workspace"), "python", None, |_| None).unwrap();

        assert_eq!(info.workspace, "/workspace");
        assert_eq!(info.server_id, "pylsp");
        assert!(info.path.is_none());
        assert!(matches!(
            info.status,
            LspProcessStatus::Missing { ref install_hint }
                if install_hint == "pip install python-lsp-server"
        ));
    }

    #[test]
    fn detect_prefers_active_server_info_over_filesystem_probe() {
        let active = LspServerInfo {
            workspace: "/workspace".into(),
            language: "rust".into(),
            server_id: "rust-analyzer".into(),
            command: "rust-analyzer".into(),
            path: Some("/active/rust-analyzer".into()),
            status: LspProcessStatus::Starting,
            last_startup_log: None,
            last_error: None,
            restart_count: 0,
        };

        let info = detect_server_info_from(
            &lsp_config::LspConfig::default(),
            Some("/workspace"),
            "rust",
            Some(active),
            |_| panic!("active info must bypass filesystem detection"),
        )
        .unwrap();

        assert!(matches!(info.status, LspProcessStatus::Starting));
        assert_eq!(info.path.as_deref(), Some("/active/rust-analyzer"));
    }

    #[test]
    fn apply_set_server_some_uncanonicalizable_keeps_raw_workspace() {
        // F2: a Some(workspace) that cannot be canonicalized (nonexistent path)
        // must land under workspaces[raw], never degrade to the global defaults.
        let mut cfg = lsp_config::LspConfig::default();
        let raw = "/no/such/path/ws";
        let used = apply_set_server(&mut cfg, Some(raw), "python", "pylsp");
        assert_eq!(used.as_deref(), Some(raw));
        assert_eq!(
            cfg.workspaces.get(raw).and_then(|m| m.get("python")),
            Some(&"pylsp".to_string())
        );
        assert!(cfg.defaults.is_empty(), "defaults must be untouched");
    }

    // --- serde wire contract ---

    #[test]
    fn status_serializes_camel_case_with_status_tag() {
        assert_eq!(
            serde_json::to_string(&LspProcessStatus::Starting).unwrap(),
            r#"{"status":"starting"}"#
        );
        assert_eq!(
            serde_json::to_string(&LspProcessStatus::Missing {
                install_hint: "npm i".into()
            })
            .unwrap(),
            r#"{"status":"missing","installHint":"npm i"}"#
        );
        assert_eq!(
            serde_json::to_string(&LspProcessStatus::Crashed {
                reason: "boom".into()
            })
            .unwrap(),
            r#"{"status":"crashed","reason":"boom"}"#
        );
        assert_eq!(
            serde_json::to_string(&LspProcessStatus::Stopped).unwrap(),
            r#"{"status":"stopped"}"#
        );
    }

    #[test]
    fn info_serializes_camel_case_keys() {
        let info = LspServerInfo {
            workspace: "/ws/a".into(),
            language: "rust".into(),
            server_id: "rust-analyzer".into(),
            command: "rust-analyzer".into(),
            path: Some("/abs/rust-analyzer".into()),
            status: LspProcessStatus::Starting,
            last_startup_log: None,
            last_error: None,
            restart_count: 2,
        };
        let v: serde_json::Value = serde_json::to_value(&info).unwrap();
        assert_eq!(v["workspace"], "/ws/a");
        assert!(v.get("serverId").is_some());
        assert!(v.get("lastStartupLog").is_some());
        assert!(v.get("lastError").is_some());
        assert_eq!(v["restartCount"], 2);
    }
}
