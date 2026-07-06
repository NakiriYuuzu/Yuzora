use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use crate::{logging, process_kill};

pub type OnLine = Arc<dyn Fn(String) + Send + Sync>;
pub type OnExit = Arc<dyn Fn(Option<i32>) + Send + Sync>;
type LogFn = Box<dyn Fn(logging::LogEvent) + Send + Sync>;

const POLL_MS: u64 = 20;
const TAIL_LINES: usize = 80;
const MAX_LOG_LINE_CHARS: usize = 500;
const MAX_STDERR_TAIL_CHARS: usize = 2000;
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

struct AgentChild {
    stdin: Arc<Mutex<std::process::ChildStdin>>,
    child: Mutex<Option<Child>>,
    cwd: String,
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
    stdout_seen: Arc<AtomicBool>,
    on_exit: OnExit,
    exited: AtomicBool,
}

pub struct AgentManager {
    children: Mutex<HashMap<String, Arc<AgentChild>>>,
    log: LogFn,
    trace_enabled: AtomicBool,
}

pub struct AgentProcessState(pub Arc<AgentManager>);

impl AgentManager {
    pub fn new() -> Self {
        let sink = Mutex::new(logging::LogSink::new(logging::default_log_dir()));
        let log: LogFn = Box::new(move |event| {
            if let Ok(mut sink) = sink.lock() {
                sink.write(event);
            }
        });
        Self::with_log(log)
    }

    fn with_log(log: LogFn) -> Self {
        Self {
            children: Mutex::new(HashMap::new()),
            log,
            trace_enabled: AtomicBool::new(false),
        }
    }

    #[cfg(test)]
    fn new_for_test() -> Arc<Self> {
        Arc::new(Self::with_log(Box::new(|_| {})))
    }

    #[cfg(test)]
    fn new_for_test_with_log(log: LogFn) -> Arc<Self> {
        Arc::new(Self::with_log(log))
    }

    pub fn next_id() -> String {
        format!("agent-{}", NEXT_ID.fetch_add(1, Ordering::Relaxed))
    }

    pub fn spawn(
        self: &Arc<Self>,
        command: &str,
        cwd: &str,
        on_line: OnLine,
        on_exit: OnExit,
    ) -> Result<String, String> {
        self.spawn_with_id(Self::next_id(), command, cwd, on_line, on_exit)
    }

    pub fn spawn_with_id(
        self: &Arc<Self>,
        id: String,
        command: &str,
        cwd: &str,
        on_line: OnLine,
        on_exit: OnExit,
    ) -> Result<String, String> {
        if self.children.lock().unwrap().contains_key(&id) {
            return Err(format!("agent {id} already exists"));
        }

        let mut cmd = shell_command(command);
        cmd.current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        process_kill::configure_new_group(&mut cmd);

        let mut child: Child = cmd
            .spawn()
            .map_err(|err| format!("agent spawn failed: {err}"))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "no agent stdin".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "no agent stdout".to_string())?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "no agent stderr".to_string())?;

        let stdin = Arc::new(Mutex::new(stdin));
        let stderr_tail = Arc::new(Mutex::new(VecDeque::new()));
        let stdout_seen = Arc::new(AtomicBool::new(false));
        let child = Arc::new(AgentChild {
            stdin: stdin.clone(),
            child: Mutex::new(Some(child)),
            cwd: cwd.to_string(),
            stderr_tail: stderr_tail.clone(),
            stdout_seen: stdout_seen.clone(),
            on_exit,
            exited: AtomicBool::new(false),
        });
        self.children
            .lock()
            .unwrap()
            .insert(id.clone(), child.clone());
        self.log_spawn(&id, cwd);

        let weak = Arc::downgrade(self);
        let stdout_id = id.clone();
        let stdout_cwd = cwd.to_string();
        let stdout_seen2 = stdout_seen.clone();
        std::thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                let Ok(line) = line else { break };
                let trimmed = line.trim_end().to_string();
                if is_json_rpc_line(&trimmed) {
                    if let Some(manager) = weak.upgrade() {
                        manager.log_trace_line("acp_trace_in", &stdout_id, &stdout_cwd, &line);
                    }
                    stdout_seen2.store(true, Ordering::SeqCst);
                    on_line(trimmed);
                } else if !trimmed.is_empty() {
                    if let Some(manager) = weak.upgrade() {
                        manager.log_ignored_stdout(&stdout_id, &stdout_cwd, &trimmed);
                    }
                }
            }
        });

        let weak = Arc::downgrade(self);
        let stderr_id = id.clone();
        let stderr_cwd = cwd.to_string();
        let stderr_tail2 = stderr_tail.clone();
        std::thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                let Ok(line) = line else { break };
                push_tail(&stderr_tail2, &line);
                if let Some(manager) = weak.upgrade() {
                    manager.log_stderr(&stderr_id, &stderr_cwd, &line);
                }
            }
        });

        let weak = Arc::downgrade(self);
        let exit_id = id.clone();
        std::thread::spawn(move || watch_child(weak, exit_id, child));

        Ok(id)
    }

    pub fn write(&self, id: &str, chunk: &str) -> Result<(), String> {
        let (stdin, cwd) = {
            let map = self.children.lock().unwrap();
            let child = map.get(id).ok_or_else(|| format!("no agent {id}"))?;
            (child.stdin.clone(), child.cwd.clone())
        };
        let mut stdin = stdin.lock().unwrap();
        stdin
            .write_all(chunk.as_bytes())
            .map_err(|err| err.to_string())?;
        if !chunk.ends_with('\n') {
            stdin.write_all(b"\n").map_err(|err| err.to_string())?;
        }
        stdin.flush().map_err(|err| err.to_string())?;
        drop(stdin);
        self.log_trace_lines("acp_trace_out", id, &cwd, chunk);
        Ok(())
    }

    pub fn kill(&self, id: &str) {
        let child = self.children.lock().unwrap().remove(id);
        if let Some(child) = child {
            self.kill_shared(id, &child);
        }
    }

    pub fn kill_all(&self) {
        let ids: Vec<String> = self.children.lock().unwrap().keys().cloned().collect();
        for id in ids {
            self.kill(&id);
        }
    }

    pub fn list(&self, cwd: &str) -> Vec<String> {
        let mut ids: Vec<String> = self
            .children
            .lock()
            .unwrap()
            .iter()
            .filter_map(|(id, child)| (child.cwd == cwd).then(|| id.clone()))
            .collect();
        ids.sort();
        ids
    }

    pub fn set_trace(&self, enabled: bool) -> Result<(), String> {
        self.trace_enabled.store(enabled, Ordering::SeqCst);
        Ok(())
    }

    fn kill_shared(&self, id: &str, child: &Arc<AgentChild>) {
        let killed = {
            let mut guard = child.child.lock().unwrap();
            if let Some(process) = guard.as_mut() {
                self.log_kill(
                    id,
                    &child.cwd,
                    process.id(),
                    stderr_summary(&child.stderr_tail),
                );
                let _ = process_kill::kill_tree(process);
                *guard = None;
                true
            } else {
                false
            }
        };
        if killed {
            self.finish_exit(id, child, None);
        }
    }

    fn finish_exit(&self, id: &str, child: &Arc<AgentChild>, code: Option<i32>) {
        if child.exited.swap(true, Ordering::SeqCst) {
            return;
        }
        self.remove_child_if_same(id, child);
        self.log_exit(
            id,
            &child.cwd,
            code,
            child.stdout_seen.load(Ordering::SeqCst),
            stderr_summary(&child.stderr_tail),
        );
        (child.on_exit)(code);
    }

    fn remove_child_if_same(&self, id: &str, child: &Arc<AgentChild>) {
        let mut map = self.children.lock().unwrap();
        if matches!(map.get(id), Some(existing) if Arc::ptr_eq(existing, child)) {
            map.remove(id);
        }
    }

    fn log_spawn(&self, id: &str, cwd: &str) {
        (self.log)(logging::LogEvent {
            level: "info".into(),
            kind: "debug".into(),
            source: "acp".into(),
            workspace_path: Some(cwd.to_string()),
            event: "acp_spawn".into(),
            message: format!("ACP agent {id} spawned"),
            metadata: serde_json::json!({
                "id": id,
                "cwd": cwd,
            }),
        });
    }

    fn log_ignored_stdout(&self, id: &str, cwd: &str, line: &str) {
        (self.log)(logging::LogEvent {
            level: "warn".into(),
            kind: "debug".into(),
            source: "acp".into(),
            workspace_path: Some(cwd.to_string()),
            event: "acp_stdout_ignored".into(),
            message: truncate_log_line(line),
            metadata: serde_json::json!({
                "id": id,
                "cwd": cwd,
            }),
        });
    }

    fn log_stderr(&self, id: &str, cwd: &str, line: &str) {
        (self.log)(logging::LogEvent {
            level: "debug".into(),
            kind: "debug".into(),
            source: "acp".into(),
            workspace_path: Some(cwd.to_string()),
            event: "acp_stderr".into(),
            message: truncate_log_line(line),
            metadata: serde_json::json!({
                "id": id,
                "cwd": cwd,
            }),
        });
    }

    fn log_exit(
        &self,
        id: &str,
        cwd: &str,
        code: Option<i32>,
        stdout_seen: bool,
        stderr_summary: Vec<String>,
    ) {
        (self.log)(logging::LogEvent {
            level: if stdout_seen { "info" } else { "warn" }.into(),
            kind: "debug".into(),
            source: "acp".into(),
            workspace_path: Some(cwd.to_string()),
            event: "acp_exit".into(),
            message: format!("ACP agent {id} exited (code {code:?})"),
            metadata: serde_json::json!({
                "id": id,
                "cwd": cwd,
                "exitCode": code,
                "stdoutSeen": stdout_seen,
                "stderrSummary": stderr_summary,
            }),
        });
    }

    fn log_kill(&self, id: &str, cwd: &str, pid: u32, stderr_summary: Vec<String>) {
        (self.log)(logging::LogEvent {
            level: "info".into(),
            kind: "debug".into(),
            source: "acp".into(),
            workspace_path: Some(cwd.to_string()),
            event: "acp_kill".into(),
            message: format!("ACP agent {id} killed"),
            metadata: serde_json::json!({
                "id": id,
                "cwd": cwd,
                "pid": pid,
                "stderrSummary": stderr_summary,
            }),
        });
    }

    fn log_trace_lines(&self, event: &str, id: &str, cwd: &str, chunk: &str) {
        if !self.trace_enabled.load(Ordering::SeqCst) {
            return;
        }
        for line in chunk.lines() {
            if is_json_rpc_line(line) {
                self.log_trace_line(event, id, cwd, line);
            }
        }
    }

    fn log_trace_line(&self, event: &str, id: &str, cwd: &str, line: &str) {
        if !self.trace_enabled.load(Ordering::SeqCst) {
            return;
        }
        (self.log)(logging::LogEvent {
            level: "debug".into(),
            kind: "debug".into(),
            source: "acp".into(),
            workspace_path: Some(cwd.to_string()),
            event: event.into(),
            message: truncate_log_line(line),
            metadata: serde_json::json!({
                "id": id,
                "cwd": cwd,
            }),
        });
    }
}

impl Drop for AgentManager {
    fn drop(&mut self) {
        self.kill_all();
    }
}

fn shell_command(command: &str) -> Command {
    #[cfg(windows)]
    {
        let shell = crate::pty_service::resolve_shell(None);
        let mut cmd = Command::new(&shell);
        cmd.args(["/C", command]);
        cmd
    }
    #[cfg(not(windows))]
    {
        let shell = crate::pty_service::resolve_shell(None);
        let mut cmd = Command::new(&shell);
        cmd.env("SHELL", &shell);
        cmd.arg("-lc").arg(command);
        cmd
    }
}

fn watch_child(manager: Weak<AgentManager>, id: String, child: Arc<AgentChild>) {
    loop {
        let code = {
            let mut guard = child.child.lock().unwrap();
            match guard.as_mut() {
                Some(process) => match process.try_wait() {
                    Ok(Some(status)) => {
                        *guard = None;
                        Some(status.code())
                    }
                    Ok(None) => None,
                    Err(_) => {
                        *guard = None;
                        Some(None)
                    }
                },
                None => return,
            }
        };

        if let Some(code) = code {
            if let Some(manager) = manager.upgrade() {
                manager.finish_exit(&id, &child, code);
            } else if !child.exited.swap(true, Ordering::SeqCst) {
                (child.on_exit)(code);
            }
            return;
        }

        std::thread::sleep(Duration::from_millis(POLL_MS));
    }
}

fn push_tail(tail: &Mutex<VecDeque<String>>, line: &str) {
    let line = truncate_log_line(line);
    let mut tail = tail.lock().unwrap();
    if tail.len() == TAIL_LINES {
        tail.pop_front();
    }
    tail.push_back(line);
    while tail_char_count(&tail) > MAX_STDERR_TAIL_CHARS {
        tail.pop_front();
    }
}

fn truncate_log_line(line: &str) -> String {
    line.chars().take(MAX_LOG_LINE_CHARS).collect()
}

fn is_json_rpc_line(line: &str) -> bool {
    let json_line = line.trim_start();
    json_line.starts_with('{') || json_line.starts_with('[')
}

fn tail_char_count(tail: &VecDeque<String>) -> usize {
    tail.iter().map(|line| line.chars().count()).sum()
}

fn stderr_summary(tail: &Mutex<VecDeque<String>>) -> Vec<String> {
    tail.lock().unwrap().iter().cloned().collect()
}

fn stdout_event_payload(id: &str, line: String) -> serde_json::Value {
    serde_json::json!({ "id": id, "line": line })
}

fn exit_event_payload(id: &str, code: Option<i32>) -> serde_json::Value {
    serde_json::json!({ "id": id, "code": code })
}

#[tauri::command]
pub async fn agent_spawn(
    app: tauri::AppHandle,
    state: tauri::State<'_, AgentProcessState>,
    command: String,
    cwd: String,
) -> Result<String, String> {
    use tauri::Emitter;

    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let id = AgentManager::next_id();
        let stdout_id = id.clone();
        let exit_id = id.clone();
        let stdout_app = app.clone();
        let exit_app = app;
        manager.spawn_with_id(
            id.clone(),
            &command,
            &cwd,
            Arc::new(move |line| {
                let _ = stdout_app.emit("agent://stdout", stdout_event_payload(&stdout_id, line));
            }),
            Arc::new(move |code| {
                let _ = exit_app.emit("agent://exit", exit_event_payload(&exit_id, code));
            }),
        )?;
        Ok(id)
    })
    .await
    .map_err(|err| err.to_string())?
}

#[tauri::command]
pub async fn agent_write(
    state: tauri::State<'_, AgentProcessState>,
    id: String,
    chunk: String,
) -> Result<(), String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || manager.write(&id, &chunk))
        .await
        .map_err(|err| err.to_string())?
}

#[tauri::command]
pub async fn agent_kill(
    state: tauri::State<'_, AgentProcessState>,
    id: String,
) -> Result<(), String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || manager.kill(&id))
        .await
        .map_err(|err| err.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn agent_list(state: tauri::State<'_, AgentProcessState>, cwd: String) -> Vec<String> {
    state.0.list(&cwd)
}

#[tauri::command]
pub fn agent_set_trace(
    state: tauri::State<'_, AgentProcessState>,
    enabled: bool,
) -> Result<(), String> {
    state.0.set_trace(enabled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    fn poll_until<F: Fn() -> bool>(t: Duration, f: F) -> bool {
        let s = Instant::now();
        while s.elapsed() < t {
            if f() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        f()
    }

    #[test]
    fn spawn_pipes_stdout_lines_and_exit() {
        let mgr = AgentManager::new_for_test();
        let lines: Arc<Mutex<Vec<String>>> = Default::default();
        let exits: Arc<Mutex<Vec<Option<i32>>>> = Default::default();
        let l2 = lines.clone();
        let e2 = exits.clone();
        // sh reads one line then echoes a JSON line and exits 0
        let id = mgr
            .spawn(
                "printf '{\"jsonrpc\":\"2.0\"}\\n'",
                ".",
                Arc::new(move |line| l2.lock().unwrap().push(line)),
                Arc::new(move |code| e2.lock().unwrap().push(code)),
            )
            .unwrap();
        assert!(poll_until(Duration::from_secs(5), || lines
            .lock()
            .unwrap()
            .iter()
            .any(|l| l.contains("jsonrpc"))));
        assert!(poll_until(Duration::from_secs(5), || !exits
            .lock()
            .unwrap()
            .is_empty()));
        let _ = id;
    }

    #[test]
    fn spawn_with_id_event_payloads_include_assigned_id() {
        let mgr = AgentManager::new_for_test();
        let events: Arc<Mutex<Vec<serde_json::Value>>> = Default::default();
        let exits = events.clone();
        let lines = events.clone();
        let id = AgentManager::next_id();
        let stdout_id = id.clone();
        let exit_id = id.clone();

        let returned = mgr
            .spawn_with_id(
                id.clone(),
                "printf '{\"jsonrpc\":\"2.0\"}\\n'",
                ".",
                Arc::new(move |line| {
                    lines
                        .lock()
                        .unwrap()
                        .push(stdout_event_payload(&stdout_id, line));
                }),
                Arc::new(move |code| {
                    exits
                        .lock()
                        .unwrap()
                        .push(exit_event_payload(&exit_id, code));
                }),
            )
            .unwrap();

        assert_eq!(returned, id);
        assert!(poll_until(Duration::from_secs(5), || events
            .lock()
            .unwrap()
            .iter()
            .any(|event| event.get("line").is_some())));
        assert!(poll_until(Duration::from_secs(5), || events
            .lock()
            .unwrap()
            .iter()
            .any(|event| event.get("code").is_some())));
        assert!(events.lock().unwrap().iter().all(|event| event["id"] == id));
    }

    #[test]
    fn list_returns_live_agents_for_matching_cwd() {
        let mgr = AgentManager::new_for_test();
        let cwd = std::env::current_dir().unwrap();
        let cwd = cwd.to_str().unwrap();
        let other_cwd = cwd.to_string() + "/other";
        let id = mgr
            .spawn("sleep 5", cwd, Arc::new(|_| {}), Arc::new(|_| {}))
            .unwrap();

        assert_eq!(mgr.list(cwd), vec![id.clone()]);
        assert!(mgr.list(&other_cwd).is_empty());

        mgr.kill(&id);
        assert!(mgr.list(cwd).is_empty());
    }

    #[test]
    fn stdout_reader_drops_non_json_lines_and_logs_them() {
        let logs: Arc<Mutex<Vec<logging::LogEvent>>> = Default::default();
        let logs2 = logs.clone();
        let mgr = AgentManager::new_for_test_with_log(Box::new(move |event| {
            logs2.lock().unwrap().push(event);
        }));
        let lines: Arc<Mutex<Vec<String>>> = Default::default();
        let lines2 = lines.clone();

        mgr.spawn(
            "printf 'profile noise\\n{\"jsonrpc\":\"2.0\"}\\n'",
            ".",
            Arc::new(move |line| lines2.lock().unwrap().push(line)),
            Arc::new(|_| {}),
        )
        .unwrap();

        assert!(poll_until(Duration::from_secs(5), || lines
            .lock()
            .unwrap()
            .iter()
            .any(|line| line.contains("jsonrpc"))));
        assert!(lines
            .lock()
            .unwrap()
            .iter()
            .all(|line| !line.contains("profile noise")));
        assert!(poll_until(Duration::from_secs(5), || logs
            .lock()
            .unwrap()
            .iter()
            .any(|event| event.event == "acp_stdout_ignored"
                && event.source == "acp"
                && event.kind == "debug")));
    }

    #[test]
    fn stderr_and_no_stdout_exit_are_logged() {
        let logs: Arc<Mutex<Vec<logging::LogEvent>>> = Default::default();
        let logs2 = logs.clone();
        let mgr = AgentManager::new_for_test_with_log(Box::new(move |event| {
            logs2.lock().unwrap().push(event);
        }));
        let exits: Arc<Mutex<Vec<Option<i32>>>> = Default::default();
        let exits2 = exits.clone();

        mgr.spawn(
            "printf 'agent failed\\n' >&2; exit 7",
            ".",
            Arc::new(|_| {}),
            Arc::new(move |code| exits2.lock().unwrap().push(code)),
        )
        .unwrap();

        assert!(poll_until(Duration::from_secs(5), || exits
            .lock()
            .unwrap()
            .contains(&Some(7))));
        assert!(logs.lock().unwrap().iter().any(|event| {
            event.event == "acp_stderr"
                && event.source == "acp"
                && event.kind == "debug"
                && event.message.contains("agent failed")
        }));
        assert!(logs.lock().unwrap().iter().any(|event| {
            event.event == "acp_exit"
                && event.level == "warn"
                && event.metadata["stdoutSeen"] == false
                && event.metadata["exitCode"] == 7
        }));
    }

    #[test]
    fn spawn_log_omits_raw_command() {
        let logs: Arc<Mutex<Vec<logging::LogEvent>>> = Default::default();
        let logs2 = logs.clone();
        let mgr = AgentManager::new_for_test_with_log(Box::new(move |event| {
            logs2.lock().unwrap().push(event);
        }));
        let secret = "COMMAND_SECRET_SHOULD_NOT_BE_LOGGED";
        let command = format!("printf '{{\"jsonrpc\":\"2.0\"}}\\n' # {secret}");

        let id = mgr
            .spawn(&command, ".", Arc::new(|_| {}), Arc::new(|_| {}))
            .unwrap();

        assert!(poll_until(Duration::from_secs(5), || logs
            .lock()
            .unwrap()
            .iter()
            .any(|event| event.event == "acp_spawn")));
        let (serialized, metadata) = {
            let logs = logs.lock().unwrap();
            let spawn = logs
                .iter()
                .find(|event| event.event == "acp_spawn")
                .unwrap();
            (
                serde_json::to_string(spawn).unwrap(),
                spawn.metadata.clone(),
            )
        };
        assert!(!serialized.contains(secret));
        assert!(metadata.get("command").is_none());
        assert_eq!(metadata["id"], id);
    }

    #[test]
    fn stderr_summary_is_truncated_before_metadata_logging() {
        let logs: Arc<Mutex<Vec<logging::LogEvent>>> = Default::default();
        let logs2 = logs.clone();
        let mgr = AgentManager::new_for_test_with_log(Box::new(move |event| {
            logs2.lock().unwrap().push(event);
        }));
        let exits: Arc<Mutex<Vec<Option<i32>>>> = Default::default();
        let exits2 = exits.clone();

        mgr.spawn(
            "i=0; while [ $i -lt 6 ]; do printf '%0501d\\n' 0 >&2; i=$((i + 1)); done; sleep 1; exit 7",
            ".",
            Arc::new(|_| {}),
            Arc::new(move |code| exits2.lock().unwrap().push(code)),
        )
        .unwrap();

        assert!(poll_until(Duration::from_secs(5), || exits
            .lock()
            .unwrap()
            .contains(&Some(7))));
        let logs = logs.lock().unwrap();
        let exit = logs.iter().find(|event| event.event == "acp_exit").unwrap();
        let summary = exit.metadata["stderrSummary"].as_array().unwrap();
        assert!(!summary.is_empty());
        let total_chars: usize = summary
            .iter()
            .map(|line| line.as_str().unwrap().chars().count())
            .sum();
        assert!(total_chars <= 2000);
        assert!(summary
            .iter()
            .all(|line| line.as_str().unwrap().chars().count() <= 500));
    }

    #[test]
    fn trace_logs_json_stdin_and_stdout_lines_when_enabled() {
        let logs: Arc<Mutex<Vec<logging::LogEvent>>> = Default::default();
        let logs2 = logs.clone();
        let mgr = AgentManager::new_for_test_with_log(Box::new(move |event| {
            logs2.lock().unwrap().push(event);
        }));
        mgr.set_trace(true).unwrap();

        let line = format!(
            "{{\"jsonrpc\":\"2.0\",\"method\":\"{}\"}}",
            "x".repeat(600)
        );
        let received: Arc<Mutex<Vec<String>>> = Default::default();
        let received2 = received.clone();
        let id = mgr
            .spawn(
                "cat",
                ".",
                Arc::new(move |line| received2.lock().unwrap().push(line)),
                Arc::new(|_| {}),
            )
            .unwrap();

        mgr.write(&id, &line).unwrap();

        assert!(poll_until(Duration::from_secs(5), || {
            let logs = logs.lock().unwrap();
            logs.iter().any(|event| event.event == "acp_trace_out")
                && logs.iter().any(|event| event.event == "acp_trace_in")
        }));
        mgr.kill(&id);

        let expected_message: String = line.chars().take(500).collect();
        let logs = logs.lock().unwrap();
        for event_name in ["acp_trace_out", "acp_trace_in"] {
            let event = logs
                .iter()
                .find(|event| event.event == event_name)
                .unwrap();
            assert_eq!(event.level, "debug");
            assert_eq!(event.source, "acp");
            assert_eq!(event.kind, "debug");
            assert_eq!(event.workspace_path.as_deref(), Some("."));
            assert_eq!(event.metadata["id"], id);
            assert_eq!(event.message, expected_message);
            assert_eq!(event.message.chars().count(), 500);
        }
        assert!(received.lock().unwrap().iter().any(|seen| seen == &line));
    }

    #[test]
    fn trace_off_writes_nothing() {
        let logs: Arc<Mutex<Vec<logging::LogEvent>>> = Default::default();
        let logs2 = logs.clone();
        let mgr = AgentManager::new_for_test_with_log(Box::new(move |event| {
            logs2.lock().unwrap().push(event);
        }));
        let line = "{\"jsonrpc\":\"2.0\",\"method\":\"initialize\"}";
        let received: Arc<Mutex<Vec<String>>> = Default::default();
        let received2 = received.clone();
        let id = mgr
            .spawn(
                "cat",
                ".",
                Arc::new(move |line| received2.lock().unwrap().push(line)),
                Arc::new(|_| {}),
            )
            .unwrap();

        mgr.write(&id, line).unwrap();

        assert!(poll_until(Duration::from_secs(5), || received
            .lock()
            .unwrap()
            .iter()
            .any(|seen| seen == line)));
        mgr.kill(&id);

        let logs = logs.lock().unwrap();
        assert!(logs
            .iter()
            .all(|event| event.event != "acp_trace_out" && event.event != "acp_trace_in"));
        let serialized = serde_json::to_string(&*logs).unwrap();
        assert!(!serialized.contains("initialize"));
    }
}
