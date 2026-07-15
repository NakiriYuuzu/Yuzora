use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use crate::{logging, process_kill};

pub type OnLine = Arc<dyn Fn(String) + Send + Sync>;
/// (exit code, stderr tail)：tail 讓前端能判別 crash 型態（如 EPIPE）並顯示摘要。
pub type OnExit = Arc<dyn Fn(Option<i32>, Vec<String>) + Send + Sync>;
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
    // initialize handshake 首行 in/out 無條件記錄一次（不受 trace gate）
    handshake_out_logged: AtomicBool,
    handshake_in_logged: AtomicBool,
}

pub struct AgentManager {
    children: Mutex<HashMap<String, Arc<AgentChild>>>,
    log: LogFn,
    trace_enabled: AtomicBool,
}

pub struct AgentProcessState(pub Arc<AgentManager>);

impl Default for AgentManager {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentManager {
    pub fn new() -> Self {
        Self::with_log(Box::new(logging::write_global))
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
        preflight_command(command, cwd)?;

        let shell = crate::pty_service::resolve_shell(None);
        let mut cmd = shell_command(&shell, command);
        cmd.current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // ACP adapters are background stdio processes. On Windows this keeps
        // the killable process group while also suppressing a console window.
        process_kill::configure_background_process(&mut cmd);

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
            handshake_out_logged: AtomicBool::new(false),
            handshake_in_logged: AtomicBool::new(false),
        });
        self.children
            .lock()
            .unwrap()
            .insert(id.clone(), child.clone());
        self.log_spawn(&id, cwd, &shell, command);

        let weak = Arc::downgrade(self);
        let stdout_id = id.clone();
        let stdout_cwd = cwd.to_string();
        let stdout_seen2 = stdout_seen.clone();
        let stdout_child = child.clone();
        std::thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                let Ok(line) = line else { break };
                let trimmed = line.trim_end().to_string();
                if is_json_rpc_line(&trimmed) {
                    if let Some(manager) = weak.upgrade() {
                        // 首行 JSON-RPC（initialize response）無條件記錄；其餘受 trace gate
                        let forced = !stdout_child
                            .handshake_in_logged
                            .swap(true, Ordering::SeqCst);
                        if forced || manager.trace_enabled.load(Ordering::SeqCst) {
                            manager.log_trace_line("acp_trace_in", &stdout_id, &stdout_cwd, &line);
                        }
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
        let child = {
            let map = self.children.lock().unwrap();
            map.get(id)
                .cloned()
                .ok_or_else(|| format!("no agent {id}"))?
        };
        {
            let mut stdin = child.stdin.lock().unwrap();
            stdin
                .write_all(chunk.as_bytes())
                .map_err(|err| err.to_string())?;
            if !chunk.ends_with('\n') {
                stdin.write_all(b"\n").map_err(|err| err.to_string())?;
            }
            stdin.flush().map_err(|err| err.to_string())?;
        }
        self.log_trace_out(&child, id, chunk);
        Ok(())
    }

    pub fn kill(&self, id: &str, reason: &str) {
        let child = self.children.lock().unwrap().remove(id);
        if let Some(child) = child {
            self.kill_shared(id, &child, reason);
        }
    }

    pub fn kill_all(&self) {
        let ids: Vec<String> = self.children.lock().unwrap().keys().cloned().collect();
        for id in ids {
            self.kill(&id, "app_exit");
        }
    }

    pub fn stderr_tail(&self, id: &str) -> Result<Vec<String>, String> {
        let map = self.children.lock().unwrap();
        let child = map.get(id).ok_or_else(|| format!("no agent {id}"))?;
        Ok(stderr_summary(&child.stderr_tail))
    }

    pub fn list(&self, cwd: &str) -> Vec<String> {
        let mut ids: Vec<String> = self
            .children
            .lock()
            .unwrap()
            .iter()
            .filter(|&(_id, child)| child.cwd == cwd)
            .map(|(id, _child)| id.clone())
            .collect();
        ids.sort();
        ids
    }

    pub fn set_trace(&self, enabled: bool) -> Result<(), String> {
        self.trace_enabled.store(enabled, Ordering::SeqCst);
        Ok(())
    }

    fn kill_shared(&self, id: &str, child: &Arc<AgentChild>, reason: &str) {
        let killed = {
            let mut guard = child.child.lock().unwrap();
            if let Some(process) = guard.as_mut() {
                self.log_kill(
                    id,
                    &child.cwd,
                    process.id(),
                    reason,
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
        let tail = stderr_summary(&child.stderr_tail);
        self.log_exit(
            id,
            &child.cwd,
            code,
            child.stdout_seen.load(Ordering::SeqCst),
            tail.clone(),
        );
        (child.on_exit)(code, tail);
    }

    fn remove_child_if_same(&self, id: &str, child: &Arc<AgentChild>) {
        let mut map = self.children.lock().unwrap();
        if matches!(map.get(id), Some(existing) if Arc::ptr_eq(existing, child)) {
            map.remove(id);
        }
    }

    fn log_spawn(&self, id: &str, cwd: &str, shell: &std::path::Path, command: &str) {
        // 診斷欄位只記 shell 與第一個 token 及其是否在 PATH 上——
        // 完整 command 可能含祕密（env 前綴等），維持不入 log。
        let first_token = command_first_token(command);
        let first_token_on_path = first_token
            .map(|token| resolve_on_path(token).is_some())
            .unwrap_or(false);
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
                "shell": shell.to_string_lossy(),
                "firstToken": first_token,
                "firstTokenOnPath": first_token_on_path,
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
        // crash stack／error 內容用 error level，Logs pane 篩 error 才看得到
        let level = if line_looks_like_error(line) {
            "error"
        } else {
            "debug"
        };
        (self.log)(logging::LogEvent {
            level: level.into(),
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

    fn log_kill(&self, id: &str, cwd: &str, pid: u32, reason: &str, stderr_summary: Vec<String>) {
        (self.log)(logging::LogEvent {
            level: "info".into(),
            kind: "debug".into(),
            source: "acp".into(),
            workspace_path: Some(cwd.to_string()),
            event: "acp_kill".into(),
            message: format!("ACP agent {id} killed ({reason})"),
            metadata: serde_json::json!({
                "id": id,
                "cwd": cwd,
                "pid": pid,
                "reason": reason,
                "stderrSummary": stderr_summary,
            }),
        });
    }

    fn log_trace_out(&self, child: &AgentChild, id: &str, chunk: &str) {
        for line in chunk.lines() {
            if !is_json_rpc_line(line) {
                continue;
            }
            // initialize handshake 無條件記錄一次；其餘流量受 trace gate。
            // 出問題時（60s timeout 全靜默）才能從 log 判別「有沒有寫出去」。
            let forced = line.contains("\"method\":\"initialize\"")
                && !child.handshake_out_logged.swap(true, Ordering::SeqCst);
            if forced || self.trace_enabled.load(Ordering::SeqCst) {
                self.log_trace_line("acp_trace_out", id, &child.cwd, line);
            }
        }
    }

    fn log_trace_line(&self, event: &str, id: &str, cwd: &str, line: &str) {
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

fn shell_command(shell: &std::path::Path, command: &str) -> Command {
    #[cfg(windows)]
    {
        let mut cmd = Command::new(shell);
        cmd.args(["/C", command]);
        cmd
    }
    #[cfg(not(windows))]
    {
        let mut cmd = Command::new(shell);
        cmd.env("SHELL", shell);
        cmd.arg("-lc").arg(command);
        cmd
    }
}

/// 取指令第一個「非 VAR=value」token；引號開頭（無法簡單解析）回 None。
fn command_first_token(command: &str) -> Option<&str> {
    let token = command.split_whitespace().find(|tok| !tok.contains('='))?;
    if token.starts_with('\'') || token.starts_with('"') {
        return None;
    }
    Some(token)
}

fn has_shell_metachars(command: &str) -> bool {
    command.chars().any(|c| {
        matches!(
            c,
            ';' | '|' | '&' | '<' | '>' | '(' | ')' | '`' | '$' | '\n'
        )
    })
}

/// 在 PATH 上尋找可執行檔；含 '/' 的 token 直接檢查該路徑。
#[cfg(unix)]
fn resolve_on_path(token: &str) -> Option<std::path::PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let is_exec = |p: &std::path::PathBuf| {
        std::fs::metadata(p)
            .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    };
    if token.contains('/') {
        let path = std::path::PathBuf::from(token);
        return is_exec(&path).then_some(path);
    }
    for dir in std::env::split_paths(&std::env::var_os("PATH")?) {
        let candidate = dir.join(token);
        if is_exec(&candidate) {
            return Some(candidate);
        }
    }
    None
}

#[cfg(windows)]
fn resolve_on_path(token: &str) -> Option<std::path::PathBuf> {
    let path_dirs: Vec<_> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    let pathext = std::env::var_os("PATHEXT")
        .unwrap_or_else(|| std::ffi::OsString::from(".COM;.EXE;.BAT;.CMD"));
    resolve_windows_on_path_from(token, &path_dirs, &pathext)
}

#[cfg(not(any(unix, windows)))]
fn resolve_on_path(_token: &str) -> Option<std::path::PathBuf> {
    None
}

#[cfg(any(windows, test))]
fn resolve_windows_on_path_from(
    token: &str,
    path_dirs: &[std::path::PathBuf],
    pathext: &std::ffi::OsStr,
) -> Option<std::path::PathBuf> {
    let token_path = std::path::PathBuf::from(token);
    let candidates = if token_path.extension().is_some() {
        vec![token_path]
    } else {
        let extensions: Vec<String> = pathext
            .to_string_lossy()
            .split(';')
            .filter_map(|extension| {
                let extension = extension.trim();
                if extension.is_empty() {
                    None
                } else if extension.starts_with('.') {
                    Some(extension.to_string())
                } else {
                    Some(format!(".{extension}"))
                }
            })
            .collect();
        extensions
            .into_iter()
            .map(|extension| std::path::PathBuf::from(format!("{token}{extension}")))
            .collect()
    };

    if token.contains(['/', '\\']) {
        return candidates.into_iter().find(|candidate| candidate.is_file());
    }
    path_dirs.iter().find_map(|dir| {
        candidates
            .iter()
            .map(|candidate| dir.join(candidate))
            .find(|candidate| candidate.is_file())
    })
}

/// spawn 前置檢查（unix）：單純指令（無 shell 元字元）的第一個 token 不在 PATH 上
/// 時直接回明確錯誤——歷史上此情況（exit 127）讓使用者等滿 initialize timeout。
/// 原則是 fail open：任何我們無法用與 shell 一致的規則解析的形式（引號、`~`、
/// 相對路徑）都放行，交給 shell 處理，只擋得住「裸字不在 PATH」與「絕對路徑不存在」。
#[cfg(unix)]
fn preflight_command(command: &str, _cwd: &str) -> Result<(), String> {
    if has_shell_metachars(command) || command.contains('"') || command.contains('\'') {
        return Ok(()); // 複合／帶引號指令交給 shell 自行解析與報錯
    }
    let Some(token) = command_first_token(command) else {
        return Ok(());
    };
    // `~` 展開與相對路徑（相對 spawn cwd）是 shell 的事，Rust 端無法等價判定
    if token.starts_with('~') || (token.contains('/') && !token.starts_with('/')) {
        return Ok(());
    }
    if resolve_on_path(token).is_none() {
        return Err(format!(
            "'{token}' was not found on the app PATH; check the installation or customize the agent command in Settings"
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn preflight_command(command: &str, cwd: &str) -> Result<(), String> {
    let path_dirs: Vec<_> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    let pathext = std::env::var_os("PATHEXT")
        .unwrap_or_else(|| std::ffi::OsString::from(".COM;.EXE;.BAT;.CMD"));
    preflight_windows_command_from(command, std::path::Path::new(cwd), &path_dirs, &pathext)
}

#[cfg(any(windows, test))]
fn preflight_windows_command_from(
    command: &str,
    cwd: &std::path::Path,
    path_dirs: &[std::path::PathBuf],
    pathext: &std::ffi::OsStr,
) -> Result<(), String> {
    if has_shell_metachars(command) || command.contains('"') || command.contains('\'') {
        return Ok(());
    }
    let Some(token) = command_first_token(command) else {
        return Ok(());
    };
    let token_path = std::path::Path::new(token);
    if token.starts_with('~') || (token.contains(['/', '\\']) && !token_path.is_absolute()) {
        return Ok(());
    }
    let mut search_dirs = Vec::with_capacity(path_dirs.len() + 1);
    search_dirs.push(cwd.to_path_buf());
    search_dirs.extend_from_slice(path_dirs);
    if resolve_windows_on_path_from(token, &search_dirs, pathext).is_none() {
        return Err(format!(
            "'{token}' was not found on the app PATH; check the installation or customize the agent command in Settings"
        ));
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn preflight_command(_command: &str, _cwd: &str) -> Result<(), String> {
    Ok(())
}

fn line_looks_like_error(line: &str) -> bool {
    line.contains("Error") || line.contains("error:") || line.contains("panicked")
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
                (child.on_exit)(code, stderr_summary(&child.stderr_tail));
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

fn exit_event_payload(id: &str, code: Option<i32>, stderr_tail: &[String]) -> serde_json::Value {
    serde_json::json!({ "id": id, "code": code, "stderrTail": stderr_tail })
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
            Arc::new(move |code, stderr_tail| {
                let _ = exit_app.emit(
                    "agent://exit",
                    exit_event_payload(&exit_id, code, &stderr_tail),
                );
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
    reason: Option<String>,
) -> Result<(), String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        manager.kill(&id, reason.as_deref().unwrap_or("unspecified"))
    })
    .await
    .map_err(|err| err.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn agent_stderr_tail(
    state: tauri::State<'_, AgentProcessState>,
    id: String,
) -> Result<Vec<String>, String> {
    state.0.stderr_tail(&id)
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
                Arc::new(move |code, _tail| e2.lock().unwrap().push(code)),
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
    fn local_fake_acp_process_roundtrips_initialize_session_and_prompt() {
        let executable = std::env::current_exe().unwrap();
        let command = format!(
            "\"{}\" --ignored --exact agent_process::tests::fake_acp_child --nocapture",
            executable.display()
        );
        let mgr = AgentManager::new_for_test();
        let lines: Arc<Mutex<Vec<String>>> = Default::default();
        let lines2 = lines.clone();
        let id = mgr
            .spawn(
                &command,
                ".",
                Arc::new(move |line| lines2.lock().unwrap().push(line)),
                Arc::new(|_, _| {}),
            )
            .unwrap();

        for (request_id, method) in [(1, "initialize"), (2, "session/new"), (3, "session/prompt")] {
            mgr.write(
                &id,
                &serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": request_id,
                    "method": method,
                    "params": {}
                })
                .to_string(),
            )
            .unwrap();
            assert!(
                poll_until(Duration::from_secs(5), || {
                    lines.lock().unwrap().iter().any(|line| {
                        serde_json::from_str::<serde_json::Value>(line)
                            .ok()
                            .is_some_and(|value| value["id"] == request_id)
                    })
                }),
                "fake ACP did not answer {method}"
            );
        }

        let responses: Vec<serde_json::Value> = lines
            .lock()
            .unwrap()
            .iter()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();
        assert_eq!(responses[0]["result"]["protocolVersion"], 1);
        assert_eq!(responses[1]["result"]["sessionId"], "fake-session");
        assert_eq!(responses[2]["result"]["stopReason"], "end_turn");
        mgr.kill(&id, "test");
    }

    #[test]
    #[ignore = "spawned by local_fake_acp_process_roundtrips_initialize_session_and_prompt"]
    fn fake_acp_child() {
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            let request: serde_json::Value = serde_json::from_str(&line.unwrap()).unwrap();
            let result = match request["method"].as_str().unwrap() {
                "initialize" => serde_json::json!({
                    "protocolVersion": 1,
                    "agentCapabilities": {},
                    "authMethods": []
                }),
                "session/new" => serde_json::json!({
                    "sessionId": "fake-session",
                    "configOptions": []
                }),
                "session/prompt" => serde_json::json!({ "stopReason": "end_turn" }),
                method => panic!("unexpected fake ACP method: {method}"),
            };
            println!(
                "{}",
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": request["id"],
                    "result": result
                })
            );
            std::io::stdout().flush().unwrap();
        }
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
                Arc::new(move |code, tail| {
                    exits
                        .lock()
                        .unwrap()
                        .push(exit_event_payload(&exit_id, code, &tail));
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
            .spawn("sleep 5", cwd, Arc::new(|_| {}), Arc::new(|_, _| {}))
            .unwrap();

        assert_eq!(mgr.list(cwd), vec![id.clone()]);
        assert!(mgr.list(&other_cwd).is_empty());

        mgr.kill(&id, "test");
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
            Arc::new(|_, _| {}),
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
            Arc::new(move |code, _tail| exits2.lock().unwrap().push(code)),
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
    fn spawn_log_records_diagnostics_without_raw_command() {
        let logs: Arc<Mutex<Vec<logging::LogEvent>>> = Default::default();
        let logs2 = logs.clone();
        let mgr = AgentManager::new_for_test_with_log(Box::new(move |event| {
            logs2.lock().unwrap().push(event);
        }));
        let secret = "COMMAND_SECRET_SHOULD_NOT_BE_LOGGED";
        let command = format!("printf '{{\"jsonrpc\":\"2.0\"}}\\n' # {secret}");

        let id = mgr
            .spawn(&command, ".", Arc::new(|_| {}), Arc::new(|_, _| {}))
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
        // 完整 command 可能含祕密 → 不入 log；但要有 shell／第一個 token 的診斷欄位
        assert!(!serialized.contains(secret));
        assert!(metadata.get("command").is_none());
        assert_eq!(metadata["id"], id);
        assert!(!metadata["shell"].as_str().unwrap().is_empty());
        assert_eq!(metadata["firstToken"], "printf");
        assert_eq!(metadata["firstTokenOnPath"], true);
    }

    #[test]
    fn spawn_preflight_rejects_missing_binary_for_simple_commands() {
        let mgr = AgentManager::new_for_test();

        let err = mgr
            .spawn_with_id(
                "agent-retry".to_string(),
                "definitely-missing-binary-yz --flag",
                ".",
                Arc::new(|_| {}),
                Arc::new(|_, _| {}),
            )
            .unwrap_err();
        assert!(err.contains("not found on the app PATH"), "{err}");
        assert!(mgr.list(".").is_empty());

        let valid_command = if cfg!(windows) {
            "echo {\"jsonrpc\":\"2.0\"}"
        } else {
            "printf '{\"jsonrpc\":\"2.0\"}\\n'"
        };
        let retry = mgr
            .spawn_with_id(
                "agent-retry".to_string(),
                valid_command,
                ".",
                Arc::new(|_| {}),
                Arc::new(|_, _| {}),
            )
            .unwrap();
        assert_eq!(retry, "agent-retry");
        mgr.kill(&retry, "test");

        // 含 shell 元字元的複合指令不做 preflight，交給 shell 自行報錯
        let id = mgr
            .spawn(
                "definitely-missing-binary-yz; true",
                ".",
                Arc::new(|_| {}),
                Arc::new(|_, _| {}),
            )
            .unwrap();
        let _ = id;
    }

    #[test]
    fn spawn_preflight_fails_open_for_shell_resolved_forms() {
        let mgr = AgentManager::new_for_test();

        // `~`、相對路徑、帶引號 env 值：Rust 端無法與 shell 等價解析 → 放行
        for command in [
            "~/definitely/missing/agent --flag",
            "./definitely/missing/agent",
            "NODE_OPTIONS=\"--foo --bar\" definitely-missing-binary-yz agent",
        ] {
            let result = mgr.spawn(command, ".", Arc::new(|_| {}), Arc::new(|_, _| {}));
            assert!(result.is_ok(), "preflight should fail open for {command:?}");
        }

        // 絕對路徑不存在仍要擋
        let err = mgr
            .spawn(
                "/definitely/missing/agent-binary --flag",
                ".",
                Arc::new(|_| {}),
                Arc::new(|_, _| {}),
            )
            .unwrap_err();
        assert!(err.contains("not found on the app PATH"), "{err}");
    }

    #[test]
    fn windows_path_lookup_honors_pathext_without_leaking_command_data() {
        let temp = tempfile::tempdir().unwrap();
        let bunx = temp.path().join("bunx.CMD");
        std::fs::write(&bunx, "@echo off\r\n").unwrap();

        assert_eq!(
            resolve_windows_on_path_from(
                "bunx",
                &[temp.path().to_path_buf()],
                std::ffi::OsStr::new(".COM;.EXE;.BAT;.CMD"),
            ),
            Some(bunx),
        );
        assert!(resolve_windows_on_path_from(
            "missing-agent",
            &[temp.path().to_path_buf()],
            std::ffi::OsStr::new(".COM;.EXE;.BAT;.CMD"),
        )
        .is_none());
    }

    #[test]
    fn windows_preflight_rejects_missing_simple_command_and_allows_retry() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("bunx.CMD"), "@echo off\r\n").unwrap();
        let path = [temp.path().to_path_buf()];
        let pathext = std::ffi::OsStr::new(".COM;.EXE;.BAT;.CMD");

        let error =
            preflight_windows_command_from("missing-agent --stdio", temp.path(), &path, pathext)
                .unwrap_err();
        assert!(error.contains("'missing-agent' was not found on the app PATH"));
        assert!(error.contains("Settings"));

        assert!(
            preflight_windows_command_from("bunx pi-acp@0.0.31", temp.path(), &path, pathext)
                .is_ok()
        );
        assert!(preflight_windows_command_from(
            "missing-agent && echo handled",
            temp.path(),
            &path,
            pathext,
        )
        .is_ok());
    }

    #[test]
    fn windows_preflight_searches_spawn_cwd_before_path() {
        let cwd = tempfile::tempdir().unwrap();
        std::fs::write(cwd.path().join("workspace-agent.CMD"), "@echo off\r\n").unwrap();

        assert!(preflight_windows_command_from(
            "workspace-agent --stdio",
            cwd.path(),
            &[],
            std::ffi::OsStr::new(".COM;.EXE;.BAT;.CMD"),
        )
        .is_ok());
    }

    #[test]
    fn kill_logs_reason_in_metadata() {
        let logs: Arc<Mutex<Vec<logging::LogEvent>>> = Default::default();
        let logs2 = logs.clone();
        let mgr = AgentManager::new_for_test_with_log(Box::new(move |event| {
            logs2.lock().unwrap().push(event);
        }));
        let id = mgr
            .spawn("sleep 5", ".", Arc::new(|_| {}), Arc::new(|_, _| {}))
            .unwrap();

        mgr.kill(&id, "init_timeout");

        let logs = logs.lock().unwrap();
        let kill = logs.iter().find(|event| event.event == "acp_kill").unwrap();
        assert_eq!(kill.metadata["reason"], "init_timeout");
        assert!(kill.message.contains("init_timeout"));
    }

    #[test]
    fn stderr_tail_is_readable_while_agent_runs() {
        let mgr = AgentManager::new_for_test();
        let id = mgr
            .spawn(
                "printf 'boom line\\n' >&2; sleep 3",
                ".",
                Arc::new(|_| {}),
                Arc::new(|_, _| {}),
            )
            .unwrap();

        assert!(poll_until(Duration::from_secs(5), || {
            mgr.stderr_tail(&id)
                .map(|tail| tail.iter().any(|line| line.contains("boom line")))
                .unwrap_or(false)
        }));
        mgr.kill(&id, "test");
        assert!(mgr.stderr_tail(&id).is_err());
    }

    #[test]
    fn exit_callback_receives_stderr_tail() {
        let mgr = AgentManager::new_for_test();
        let tails: Arc<Mutex<Vec<Vec<String>>>> = Default::default();
        let tails2 = tails.clone();

        mgr.spawn(
            "printf 'crash detail\\n' >&2; exit 3",
            ".",
            Arc::new(|_| {}),
            Arc::new(move |_code, tail| tails2.lock().unwrap().push(tail)),
        )
        .unwrap();

        assert!(poll_until(Duration::from_secs(5), || !tails
            .lock()
            .unwrap()
            .is_empty()));
        let tails = tails.lock().unwrap();
        assert!(tails[0].iter().any(|line| line.contains("crash detail")));
    }

    #[test]
    fn stderr_error_lines_are_logged_at_error_level() {
        let logs: Arc<Mutex<Vec<logging::LogEvent>>> = Default::default();
        let logs2 = logs.clone();
        let mgr = AgentManager::new_for_test_with_log(Box::new(move |event| {
            logs2.lock().unwrap().push(event);
        }));

        mgr.spawn(
            "printf 'Error: write EPIPE\\n' >&2; printf 'plain note\\n' >&2; exit 1",
            ".",
            Arc::new(|_| {}),
            Arc::new(|_, _| {}),
        )
        .unwrap();

        assert!(poll_until(Duration::from_secs(5), || logs
            .lock()
            .unwrap()
            .iter()
            .filter(|event| event.event == "acp_stderr")
            .count()
            >= 2));
        let logs = logs.lock().unwrap();
        let epipe = logs
            .iter()
            .find(|event| event.event == "acp_stderr" && event.message.contains("EPIPE"))
            .unwrap();
        assert_eq!(epipe.level, "error");
        let plain = logs
            .iter()
            .find(|event| event.event == "acp_stderr" && event.message.contains("plain note"))
            .unwrap();
        assert_eq!(plain.level, "debug");
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
            Arc::new(move |code, _tail| exits2.lock().unwrap().push(code)),
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

        let line = format!("{{\"jsonrpc\":\"2.0\",\"method\":\"{}\"}}", "x".repeat(600));
        let received: Arc<Mutex<Vec<String>>> = Default::default();
        let received2 = received.clone();
        let id = mgr
            .spawn(
                "cat",
                ".",
                Arc::new(move |line| received2.lock().unwrap().push(line)),
                Arc::new(|_, _| {}),
            )
            .unwrap();

        mgr.write(&id, &line).unwrap();

        assert!(poll_until(Duration::from_secs(5), || {
            let logs = logs.lock().unwrap();
            logs.iter().any(|event| event.event == "acp_trace_out")
                && logs.iter().any(|event| event.event == "acp_trace_in")
        }));
        mgr.kill(&id, "test");

        let expected_message: String = line.chars().take(500).collect();
        let logs = logs.lock().unwrap();
        for event_name in ["acp_trace_out", "acp_trace_in"] {
            let event = logs.iter().find(|event| event.event == event_name).unwrap();
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
    fn trace_off_logs_only_initialize_handshake() {
        let logs: Arc<Mutex<Vec<logging::LogEvent>>> = Default::default();
        let logs2 = logs.clone();
        let mgr = AgentManager::new_for_test_with_log(Box::new(move |event| {
            logs2.lock().unwrap().push(event);
        }));
        let init_line = "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\"}";
        let prompt_line = "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"session/prompt\",\"params\":\"PROMPT_SECRET\"}";
        let received: Arc<Mutex<Vec<String>>> = Default::default();
        let received2 = received.clone();
        let id = mgr
            .spawn(
                "cat",
                ".",
                Arc::new(move |line| received2.lock().unwrap().push(line)),
                Arc::new(|_, _| {}),
            )
            .unwrap();

        mgr.write(&id, init_line).unwrap();
        assert!(poll_until(Duration::from_secs(5), || received
            .lock()
            .unwrap()
            .iter()
            .any(|seen| seen == init_line)));
        mgr.write(&id, prompt_line).unwrap();
        assert!(poll_until(Duration::from_secs(5), || received
            .lock()
            .unwrap()
            .iter()
            .any(|seen| seen == prompt_line)));
        mgr.kill(&id, "test");

        // trace 關閉時：initialize handshake（首行 out + 首行 in）仍記錄——
        // 這是 60s 全靜默 timeout 唯一的診斷依據；其後流量不記錄。
        let logs = logs.lock().unwrap();
        let trace_out: Vec<_> = logs
            .iter()
            .filter(|event| event.event == "acp_trace_out")
            .collect();
        let trace_in: Vec<_> = logs
            .iter()
            .filter(|event| event.event == "acp_trace_in")
            .collect();
        assert_eq!(trace_out.len(), 1);
        assert!(trace_out[0].message.contains("initialize"));
        assert_eq!(trace_in.len(), 1);
        assert!(trace_in[0].message.contains("initialize"));
        let serialized = serde_json::to_string(&*logs).unwrap();
        assert!(!serialized.contains("PROMPT_SECRET"));
    }
}
