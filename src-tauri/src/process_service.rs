use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use crate::{logging, process_kill, pty_service};

const POLL_MS: u64 = 50;
const TAIL_LINES: usize = 80;
static NEXT_RESERVATION_ID: AtomicU64 = AtomicU64::new(1);

pub type OnOutput = Arc<dyn Fn(String) + Send + Sync>;
type LogFn = Box<dyn Fn(logging::LogEvent) + Send + Sync>;
type EmitFn = Arc<dyn Fn(DevServerInfo) + Send + Sync>;
type KillFn = fn(&mut Child) -> std::io::Result<()>;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase", tag = "status")]
pub enum DevServerStatus {
    Starting,
    Running { port: Option<u16> },
    Exited { code: Option<i32> },
    Failed { reason: String },
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DevServerInfo {
    pub workspace: String,
    pub command: String,
    pub port: Option<u16>,
    pub status: DevServerStatus,
}

struct ServerShared {
    workspace: String,
    info: Mutex<DevServerInfo>,
    // Terminal ownership is guarded by this mutex: the watcher or stopper that
    // records `terminal` first is the only side allowed to publish it.
    child: Mutex<ServerChildState>,
    stopped: AtomicBool,
    on_output: OnOutput,
    stderr_tail: Mutex<VecDeque<String>>,
}

struct ServerChildState {
    child: Option<Child>,
    terminal: Option<DevServerStatus>,
}

enum ServerEntry {
    Reserved(u64),
    Ready(Arc<ServerShared>),
}

pub struct ProcessManager {
    servers: Mutex<HashMap<String, ServerEntry>>,
    log: LogFn,
    emit: EmitFn,
    kill: KillFn,
}

pub struct ProcessState(pub Arc<ProcessManager>);

impl ProcessManager {
    pub fn new(app: tauri::AppHandle) -> Self {
        let log: LogFn = Box::new(logging::write_global);
        let emit: EmitFn = Arc::new(move |info| {
            use tauri::Emitter;
            let _ = app.emit("dev-server:status", info);
        });
        Self::with_seams(log, emit)
    }

    #[cfg(test)]
    fn with_parts() -> Self {
        Self::with_seams(Box::new(|_| {}), Arc::new(|_| {}))
    }

    fn with_seams(log: LogFn, emit: EmitFn) -> Self {
        Self {
            servers: Mutex::new(HashMap::new()),
            log,
            emit,
            kill: process_kill::kill_tree,
        }
    }

    #[cfg(test)]
    fn with_test_kill(log: LogFn, emit: EmitFn, kill: KillFn) -> Self {
        Self {
            servers: Mutex::new(HashMap::new()),
            log,
            emit,
            kill,
        }
    }

    pub fn start(
        self: &Arc<Self>,
        workspace: &str,
        command: &str,
        port: Option<u16>,
        on_output: OnOutput,
    ) -> Result<DevServerInfo, String> {
        let workspace_key = workspace.to_string();
        let reservation_id = NEXT_RESERVATION_ID.fetch_add(1, Ordering::Relaxed);
        {
            let mut map = self.servers.lock().unwrap();
            match map.get(workspace) {
                Some(ServerEntry::Reserved(_)) | Some(ServerEntry::Ready(_)) => {
                    return Err("dev server already starting/running for workspace".into());
                }
                None => {
                    map.insert(workspace_key.clone(), ServerEntry::Reserved(reservation_id));
                }
            }
        }

        let mut cmd = shell_command(command);
        cmd.current_dir(workspace)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        process_kill::configure_new_group(&mut cmd);

        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(err) => {
                self.remove_reservation(&workspace_key, reservation_id);
                return Err(format!("spawn failed: {err}"));
            }
        };
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let pid = child.id();

        let info = DevServerInfo {
            workspace: workspace.to_string(),
            command: command.to_string(),
            port: None,
            status: DevServerStatus::Starting,
        };
        let return_info = info.clone();
        let shared = Arc::new(ServerShared {
            workspace: workspace.to_string(),
            info: Mutex::new(info),
            child: Mutex::new(ServerChildState {
                child: Some(child),
                terminal: None,
            }),
            stopped: AtomicBool::new(false),
            on_output,
            stderr_tail: Mutex::new(VecDeque::new()),
        });

        let reservation_ready = {
            let mut map = self.servers.lock().unwrap();
            match map.get(workspace) {
                Some(ServerEntry::Reserved(id)) if *id == reservation_id => {
                    map.insert(workspace_key.clone(), ServerEntry::Ready(shared.clone()));
                    true
                }
                _ => false,
            }
        };
        if !reservation_ready {
            if let Some(child) = shared.child.lock().unwrap().child.as_mut() {
                let _ = (self.kill)(child);
            }
            return Err("dev server start was cancelled".into());
        }

        self.log_start(workspace, command, port, pid);
        self.set_running(&shared, port);

        if let Some(stdout) = stdout {
            let manager = Arc::downgrade(self);
            let shared = shared.clone();
            std::thread::spawn(move || reader_loop(manager, shared, stdout, false));
        }
        if let Some(stderr) = stderr {
            let manager = Arc::downgrade(self);
            let shared = shared.clone();
            std::thread::spawn(move || reader_loop(manager, shared, stderr, true));
        }

        let manager = Arc::downgrade(self);
        std::thread::spawn(move || watch_loop(manager, shared));

        Ok(return_info)
    }

    pub fn stop(&self, workspace: &str) -> Result<(), String> {
        let shared = self.ready_server(workspace);
        let Some(shared) = shared else {
            return Ok(());
        };

        shared.stopped.store(true, Ordering::SeqCst);
        let mut remove_after_publish = false;
        let terminal = {
            let mut guard = shared.child.lock().unwrap();
            if guard.terminal.is_some() {
                None
            } else if let Some(child) = guard.child.as_mut() {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        let terminal = DevServerStatus::Exited {
                            code: status.code(),
                        };
                        guard.child = None;
                        guard.terminal = Some(terminal.clone());
                        Some(terminal)
                    }
                    Ok(None) => {
                        let pid = child.id();
                        let terminal = match (self.kill)(child) {
                            Ok(()) => DevServerStatus::Exited { code: None },
                            Err(err) => {
                                let reason = format!("kill failed: {err}");
                                self.log_kill_failed(&shared, pid, &err);
                                remove_after_publish = true;
                                DevServerStatus::Failed { reason }
                            }
                        };
                        guard.child = None;
                        guard.terminal = Some(terminal.clone());
                        Some(terminal)
                    }
                    Err(err) => {
                        let terminal = DevServerStatus::Failed {
                            reason: format!("wait failed: {err}"),
                        };
                        guard.child = None;
                        guard.terminal = Some(terminal.clone());
                        Some(terminal)
                    }
                }
            } else {
                let terminal = DevServerStatus::Exited { code: None };
                guard.terminal = Some(terminal.clone());
                Some(terminal)
            }
        };
        if let Some(terminal) = terminal {
            let info = self.update_status(&shared, terminal);
            self.log_stop(&info);
        }
        if remove_after_publish {
            self.remove_ready(workspace, &shared);
        }
        Ok(())
    }

    pub fn stop_workspace(&self, workspace: &str) -> Result<(), String> {
        self.stop(workspace)
    }

    pub fn kill_all(&self) {
        let servers: Vec<Arc<ServerShared>> = self
            .servers
            .lock()
            .unwrap()
            .drain()
            .filter_map(|(_, entry)| match entry {
                ServerEntry::Ready(shared) => Some(shared),
                ServerEntry::Reserved(_) => None,
            })
            .collect();
        for shared in servers {
            shared.stopped.store(true, Ordering::SeqCst);
            let terminal = if let Ok(mut guard) = shared.child.lock() {
                if guard.terminal.is_some() {
                    None
                } else if let Some(child) = guard.child.as_mut() {
                    let pid = child.id();
                    let terminal = match (self.kill)(child) {
                        Ok(()) => DevServerStatus::Exited { code: None },
                        Err(err) => {
                            let reason = format!("kill failed: {err}");
                            self.log_kill_failed(&shared, pid, &err);
                            DevServerStatus::Failed { reason }
                        }
                    };
                    guard.child = None;
                    guard.terminal = Some(terminal.clone());
                    Some(terminal)
                } else {
                    None
                }
            } else {
                None
            };
            if let Some(terminal) = terminal {
                let info = self.update_status(&shared, terminal);
                self.log_stop(&info);
            } else {
                let info = shared.info.lock().unwrap().clone();
                self.log_stop(&info);
            }
        }
    }

    pub fn status_for(&self, workspace: &str) -> Option<DevServerInfo> {
        self.servers
            .lock()
            .unwrap()
            .get(workspace)
            .and_then(|entry| match entry {
                ServerEntry::Ready(shared) => Some(shared.info.lock().unwrap().clone()),
                ServerEntry::Reserved(_) => None,
            })
    }

    fn set_running(&self, shared: &Arc<ServerShared>, port: Option<u16>) {
        self.update_status(shared, DevServerStatus::Running { port });
    }

    fn update_port_from_output(&self, shared: &Arc<ServerShared>, port: u16) {
        let info = {
            if shared.stopped.load(Ordering::SeqCst) {
                return;
            }
            let child_guard = shared.child.lock().unwrap();
            if child_guard.terminal.is_some() {
                return;
            }
            let map = self.servers.lock().unwrap();
            if !matches!(
                map.get(&shared.workspace),
                Some(ServerEntry::Ready(current)) if Arc::ptr_eq(current, shared)
            ) || shared.stopped.load(Ordering::SeqCst)
            {
                return;
            }
            let mut info = shared.info.lock().unwrap();
            info.port = Some(port);
            info.status = DevServerStatus::Running { port: Some(port) };
            drop(child_guard);
            info.clone()
        };
        (self.emit)(info);
    }

    fn update_status(&self, shared: &Arc<ServerShared>, status: DevServerStatus) -> DevServerInfo {
        let info = {
            let mut info = shared.info.lock().unwrap();
            match status {
                DevServerStatus::Running { port } => {
                    info.port = port;
                    info.status = DevServerStatus::Running { port };
                }
                status => {
                    info.status = status;
                }
            }
            info.clone()
        };
        (self.emit)(info.clone());
        info
    }

    #[cfg(test)]
    fn debug_pid(&self, workspace: &str) -> Option<u32> {
        self.servers
            .lock()
            .unwrap()
            .get(workspace)
            .and_then(|entry| match entry {
                ServerEntry::Ready(shared) => shared
                    .child
                    .lock()
                    .unwrap()
                    .child
                    .as_ref()
                    .map(std::process::Child::id),
                ServerEntry::Reserved(_) => None,
            })
    }

    fn ready_server(&self, workspace: &str) -> Option<Arc<ServerShared>> {
        self.servers
            .lock()
            .unwrap()
            .get(workspace)
            .and_then(|entry| match entry {
                ServerEntry::Ready(shared) => Some(shared.clone()),
                ServerEntry::Reserved(_) => None,
            })
    }

    fn remove_reservation(&self, workspace: &str, reservation_id: u64) {
        let mut map = self.servers.lock().unwrap();
        if matches!(
            map.get(workspace),
            Some(ServerEntry::Reserved(id)) if *id == reservation_id
        ) {
            map.remove(workspace);
        }
    }

    fn remove_ready(&self, workspace: &str, shared: &Arc<ServerShared>) {
        let mut map = self.servers.lock().unwrap();
        if matches!(
            map.get(workspace),
            Some(ServerEntry::Ready(current)) if Arc::ptr_eq(current, shared)
        ) {
            map.remove(workspace);
        }
    }

    fn log_start(&self, workspace: &str, command: &str, port: Option<u16>, pid: u32) {
        (self.log)(logging::LogEvent {
            level: "info".into(),
            kind: "debug".into(),
            source: "dev_server".into(),
            workspace_path: Some(workspace.to_string()),
            event: "dev_server_start".into(),
            message: "dev server started".into(),
            metadata: serde_json::json!({
                "workspace": workspace,
                "command": command,
                "port": port,
                "pid": pid,
            }),
        });
    }

    fn log_exit(&self, info: &DevServerInfo, stderr_summary: Vec<String>, code: Option<i32>) {
        (self.log)(logging::LogEvent {
            level: "info".into(),
            kind: "debug".into(),
            source: "dev_server".into(),
            workspace_path: Some(info.workspace.clone()),
            event: "dev_server_exit".into(),
            message: format!("dev server exited (code {code:?})"),
            metadata: serde_json::json!({
                "workspace": info.workspace,
                "command": info.command,
                "port": info.port,
                "exitCode": code,
                "stderrSummary": stderr_summary,
            }),
        });
    }

    fn log_stop(&self, info: &DevServerInfo) {
        (self.log)(logging::LogEvent {
            level: "info".into(),
            kind: "debug".into(),
            source: "dev_server".into(),
            workspace_path: Some(info.workspace.clone()),
            event: "dev_server_stop".into(),
            message: "dev server stopped".into(),
            metadata: serde_json::json!({
                "workspace": info.workspace,
                "command": info.command,
                "port": info.port,
            }),
        });
    }

    fn log_kill_failed(&self, shared: &ServerShared, pid: u32, err: &std::io::Error) {
        let info = shared.info.lock().unwrap().clone();
        (self.log)(logging::LogEvent {
            level: "error".into(),
            kind: "debug".into(),
            source: "dev_server".into(),
            workspace_path: Some(info.workspace.clone()),
            event: "dev_server_kill_failed".into(),
            message: format!("dev server kill failed: {err}"),
            metadata: serde_json::json!({
                "workspace": info.workspace,
                "command": info.command,
                "port": info.port,
                "pid": pid,
                "error": err.to_string(),
            }),
        });
    }
}

impl Drop for ProcessManager {
    fn drop(&mut self) {
        self.kill_all();
    }
}

fn shell_command(command: &str) -> Command {
    #[cfg(windows)]
    {
        let shell = std::env::var_os("ComSpec").unwrap_or_else(|| "cmd.exe".into());
        let mut cmd = Command::new(shell);
        cmd.args(["/C", command]);
        cmd
    }
    #[cfg(not(windows))]
    {
        let mut cmd = Command::new(pty_service::resolve_shell(None));
        cmd.arg("-lc").arg(command);
        cmd
    }
}

fn reader_loop(
    manager: Weak<ProcessManager>,
    shared: Arc<ServerShared>,
    stream: impl std::io::Read,
    is_stderr: bool,
) {
    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let Ok(line) = line else { break };
        if is_stderr {
            push_tail(&shared.stderr_tail, line.clone());
        }
        (shared.on_output)(line.clone());
        if let Some(port) = parse_output_port(&line) {
            if let Some(manager) = manager.upgrade() {
                manager.update_port_from_output(&shared, port);
            }
        }
    }
}

fn watch_loop(manager: Weak<ProcessManager>, shared: Arc<ServerShared>) {
    let terminal = loop {
        let terminal = {
            let mut guard = shared.child.lock().unwrap();
            if guard.terminal.is_some() {
                return;
            }
            match guard.child.as_mut() {
                Some(child) => match child.try_wait() {
                    Ok(Some(status)) => {
                        let terminal = DevServerStatus::Exited {
                            code: status.code(),
                        };
                        guard.child = None;
                        guard.terminal = Some(terminal.clone());
                        Some(terminal)
                    }
                    Ok(None) => None,
                    Err(err) => {
                        let terminal = DevServerStatus::Failed {
                            reason: format!("wait failed: {err}"),
                        };
                        guard.child = None;
                        guard.terminal = Some(terminal.clone());
                        break terminal;
                    }
                },
                None => return,
            }
        };

        if let Some(terminal) = terminal {
            break terminal;
        }
        std::thread::sleep(Duration::from_millis(POLL_MS));
    };

    let Some(manager) = manager.upgrade() else {
        return;
    };
    let code = match terminal {
        DevServerStatus::Exited { code } => code,
        _ => None,
    };
    let info = manager.update_status(&shared, terminal);
    if matches!(info.status, DevServerStatus::Exited { .. }) {
        let stderr_summary = shared.stderr_tail.lock().unwrap().iter().cloned().collect();
        manager.log_exit(&info, stderr_summary, code);
    }
}

fn push_tail(tail: &Mutex<VecDeque<String>>, line: String) {
    let mut tail = tail.lock().unwrap();
    if tail.len() == TAIL_LINES {
        tail.pop_front();
    }
    tail.push_back(line);
}

fn parse_output_port(line: &str) -> Option<u16> {
    let trimmed = line.trim();
    if let Some(port) = parse_localhost_url_port(trimmed) {
        if localhost_url_occupies_line(trimmed) {
            return Some(port);
        }
    }

    let lower = line.to_ascii_lowercase();
    let url_index = first_localhost_url_index(&lower)?;
    let prefix = &lower[..url_index];
    if ["local:", "listening on", "ready", "running at", "served at"]
        .iter()
        .any(|marker| prefix.contains(marker))
    {
        return parse_localhost_url_port(&line[url_index..]);
    }
    None
}

fn first_localhost_url_index(line: &str) -> Option<usize> {
    ["http://localhost:", "http://127.0.0.1:", "http://[::1]:"]
        .iter()
        .filter_map(|prefix| line.find(prefix))
        .min()
}

fn parse_localhost_url_port(text: &str) -> Option<u16> {
    for marker in ["http://localhost:", "http://127.0.0.1:", "http://[::1]:"] {
        if let Some(rest) = text.strip_prefix(marker) {
            let digits: String = rest.chars().take_while(|ch| ch.is_ascii_digit()).collect();
            if let Ok(port) = digits.parse::<u16>() {
                return Some(port);
            }
        }
    }
    None
}

fn localhost_url_occupies_line(line: &str) -> bool {
    for marker in ["http://localhost:", "http://127.0.0.1:", "http://[::1]:"] {
        if let Some(rest) = line.strip_prefix(marker) {
            let digit_count = rest.chars().take_while(|ch| ch.is_ascii_digit()).count();
            if digit_count == 0 {
                return false;
            }
            let suffix = &rest[digit_count..];
            return suffix.is_empty() || suffix == "/";
        }
    }
    false
}

#[tauri::command]
pub async fn dev_server_start(
    state: tauri::State<'_, ProcessState>,
    workspace: String,
    command: String,
    port: Option<u16>,
    on_output: tauri::ipc::Channel<String>,
) -> Result<DevServerInfo, String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let channel = on_output;
        let on_output: OnOutput = Arc::new(move |line| {
            let _ = channel.send(line);
        });
        manager.start(&workspace, &command, port, on_output)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn dev_server_stop(
    state: tauri::State<'_, ProcessState>,
    workspace: String,
) -> Result<(), String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || manager.stop(&workspace))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn dev_server_stop_workspace(
    state: tauri::State<'_, ProcessState>,
    workspace: String,
) -> Result<(), String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || manager.stop_workspace(&workspace))
        .await
        .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    fn test_manager() -> Arc<ProcessManager> {
        Arc::new(ProcessManager::with_parts())
    }

    fn capture_output() -> (OnOutput, Arc<Mutex<Vec<String>>>) {
        let output: Arc<Mutex<Vec<String>>> = Default::default();
        let output2 = output.clone();
        let on_output: OnOutput = Arc::new(move |line| output2.lock().unwrap().push(line));
        (on_output, output)
    }

    fn test_manager_with_events() -> (Arc<ProcessManager>, Arc<Mutex<Vec<DevServerInfo>>>) {
        let events: Arc<Mutex<Vec<DevServerInfo>>> = Default::default();
        let events2 = events.clone();
        let manager = Arc::new(ProcessManager::with_seams(
            Box::new(|_| {}),
            Arc::new(move |info| events2.lock().unwrap().push(info)),
        ));
        (manager, events)
    }

    fn terminal_events(events: &Arc<Mutex<Vec<DevServerInfo>>>) -> Vec<DevServerInfo> {
        events
            .lock()
            .unwrap()
            .iter()
            .filter(|info| {
                matches!(
                    info.status,
                    DevServerStatus::Exited { .. } | DevServerStatus::Failed { .. }
                )
            })
            .cloned()
            .collect()
    }

    fn poll_until<F: Fn() -> bool>(timeout: Duration, f: F) -> bool {
        let start = Instant::now();
        while start.elapsed() < timeout {
            if f() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        f()
    }

    #[cfg(unix)]
    fn process_exists(pid: u32) -> bool {
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
    }

    #[test]
    fn start_reads_output_backfills_port_and_stop_marks_exited() {
        let mgr = test_manager();
        let (on_output, output) = capture_output();
        let info = mgr
            .start(
                std::env::current_dir().unwrap().to_str().unwrap(),
                "sh -c 'echo Local: http://localhost:1234; sleep 5'",
                None,
                on_output,
            )
            .unwrap();

        assert_eq!(info.status, DevServerStatus::Starting);
        assert!(poll_until(Duration::from_secs(3), || output
            .lock()
            .unwrap()
            .iter()
            .any(|line| line.contains("localhost:1234"))));
        assert!(poll_until(Duration::from_secs(3), || matches!(
            mgr.status_for(std::env::current_dir().unwrap().to_str().unwrap())
                .unwrap()
                .status,
            DevServerStatus::Running { port: Some(1234) }
        )));

        let pid = mgr
            .debug_pid(std::env::current_dir().unwrap().to_str().unwrap())
            .unwrap();
        mgr.stop(std::env::current_dir().unwrap().to_str().unwrap())
            .unwrap();
        assert!(matches!(
            mgr.status_for(std::env::current_dir().unwrap().to_str().unwrap())
                .unwrap()
                .status,
            DevServerStatus::Exited { .. }
        ));
        #[cfg(unix)]
        assert!(poll_until(Duration::from_secs(3), || !process_exists(pid)));
    }

    #[test]
    fn start_rejects_existing_workspace_without_replacing_process() {
        let mgr = test_manager();
        let (on_output, _output) = capture_output();
        let workspace = std::env::current_dir().unwrap();
        let workspace = workspace.to_str().unwrap();
        mgr.start(workspace, "sh -c 'sleep 5'", None, on_output.clone())
            .unwrap();
        let pid = mgr.debug_pid(workspace).unwrap();

        let err = mgr
            .start(workspace, "sh -c 'sleep 5'", None, on_output)
            .unwrap_err();

        assert_eq!(err, "dev server already starting/running for workspace");
        assert_eq!(mgr.debug_pid(workspace), Some(pid));
        mgr.stop(workspace).unwrap();
    }

    #[test]
    fn output_port_parse_requires_readiness_context() {
        assert_eq!(
            parse_output_port("Local: http://localhost:5173/"),
            Some(5173)
        );
        assert_eq!(
            parse_output_port("fetching http://localhost:9999/api"),
            None
        );
    }

    #[test]
    fn stop_contending_with_natural_exit_publishes_real_exit_once() {
        let (mgr, events) = test_manager_with_events();
        let (on_output, output) = capture_output();
        let workspace = std::env::current_dir().unwrap();
        let workspace = workspace.to_str().unwrap();
        mgr.start(workspace, "sh -c 'echo done; exit 7'", None, on_output)
            .unwrap();

        assert!(poll_until(Duration::from_secs(3), || output
            .lock()
            .unwrap()
            .iter()
            .any(|line| line == "done")));
        mgr.stop(workspace).unwrap();

        assert!(poll_until(Duration::from_secs(3), || terminal_events(
            &events
        )
        .len()
            == 1));
        let events = terminal_events(&events);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].status, DevServerStatus::Exited { code: Some(7) });
    }

    #[test]
    fn stop_kill_failure_publishes_failed_and_logs_error() {
        fn kill_then_fail(child: &mut Child) -> std::io::Result<()> {
            let _ = process_kill::kill_tree(child);
            Err(std::io::Error::other("boom"))
        }

        let events: Arc<Mutex<Vec<DevServerInfo>>> = Default::default();
        let events2 = events.clone();
        let logs: Arc<Mutex<Vec<logging::LogEvent>>> = Default::default();
        let logs2 = logs.clone();
        let mgr = Arc::new(ProcessManager::with_test_kill(
            Box::new(move |event| logs2.lock().unwrap().push(event)),
            Arc::new(move |info| events2.lock().unwrap().push(info)),
            kill_then_fail,
        ));
        let (on_output, _output) = capture_output();
        let workspace = std::env::current_dir().unwrap();
        let workspace = workspace.to_str().unwrap();
        mgr.start(workspace, "sh -c 'sleep 5'", None, on_output)
            .unwrap();

        mgr.stop(workspace).unwrap();

        assert!(mgr.status_for(workspace).is_none());
        assert!(terminal_events(&events).iter().any(|info| matches!(
            info.status,
            DevServerStatus::Failed { ref reason } if reason == "kill failed: boom"
        )));
        assert!(logs.lock().unwrap().iter().any(|event| {
            event.level == "error"
                && event.event == "dev_server_kill_failed"
                && event.metadata["workspace"] == workspace
                && event.metadata["error"] == "boom"
        }));
    }

    #[test]
    fn exit_code_is_captured_for_fast_exiting_command() {
        let mgr = test_manager();
        let (on_output, _output) = capture_output();
        let workspace = std::env::current_dir().unwrap();
        let workspace = workspace.to_str().unwrap();
        mgr.start(workspace, "sh -c 'exit 7'", None, on_output)
            .unwrap();

        assert!(poll_until(Duration::from_secs(3), || matches!(
            mgr.status_for(workspace).unwrap().status,
            DevServerStatus::Exited { code: Some(7) }
        )));
    }

    #[test]
    fn kill_all_clears_map_and_kills_processes() {
        let mgr = test_manager();
        let (on_output, _output) = capture_output();
        let workspace = std::env::current_dir().unwrap();
        let workspace = workspace.to_str().unwrap();
        mgr.start(workspace, "sh -c 'sleep 5'", Some(3000), on_output)
            .unwrap();
        let pid = mgr.debug_pid(workspace).unwrap();

        mgr.kill_all();
        assert!(mgr.status_for(workspace).is_none());
        #[cfg(unix)]
        assert!(poll_until(Duration::from_secs(3), || !process_exists(pid)));
    }

    #[test]
    fn stop_is_idempotent() {
        let mgr = test_manager();
        mgr.stop("missing").unwrap();

        let (on_output, _output) = capture_output();
        let workspace = std::env::current_dir().unwrap();
        let workspace = workspace.to_str().unwrap();
        mgr.start(workspace, "sh -c 'sleep 5'", None, on_output)
            .unwrap();
        mgr.stop(workspace).unwrap();
        mgr.stop(workspace).unwrap();
    }
}
