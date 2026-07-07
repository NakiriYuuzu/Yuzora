use std::collections::{HashMap, VecDeque};
use std::io::{self, Read};
use std::process::{Child, Command, ExitStatus as ProcessExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::process_kill;

const POLL_MS: u64 = 20;
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct ExitStatus {
    pub exit_code: Option<i32>,
    pub signal: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct AgentTerminalOutput {
    pub output: String,
    pub truncated: bool,
    pub exit_status: Option<ExitStatus>,
}

pub struct AgentTerminalState(pub Arc<AgentTerminalManager>);

pub struct AgentTerminalManager {
    terminals: Mutex<HashMap<String, Arc<AgentTerminal>>>,
}

struct AgentTerminal {
    child: Mutex<Option<Child>>,
    buffer: Mutex<OutputBuffer>,
    exit_status: Mutex<Option<ExitStatus>>,
    exit_changed: Condvar,
    reader_handles: Mutex<Vec<JoinHandle<()>>>,
}

struct OutputBuffer {
    bytes: VecDeque<u8>,
    limit: usize,
    truncated: bool,
}

impl Default for AgentTerminalManager {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentTerminalManager {
    pub fn new() -> Self {
        Self {
            terminals: Mutex::new(HashMap::new()),
        }
    }

    pub fn create(
        &self,
        command: &str,
        args: &[String],
        env: Vec<(String, String)>,
        cwd: &str,
        byte_limit: usize,
    ) -> Result<String, String> {
        let mut cmd = Command::new(command);
        cmd.args(args)
            .envs(env)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        process_kill::configure_new_group(&mut cmd);

        let mut child = cmd
            .spawn()
            .map_err(|err| format!("terminal spawn failed: {err}"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "no terminal stdout".to_string())?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "no terminal stderr".to_string())?;

        let id = Self::next_id();
        let terminal = Arc::new(AgentTerminal {
            child: Mutex::new(Some(child)),
            buffer: Mutex::new(OutputBuffer::new(byte_limit)),
            exit_status: Mutex::new(None),
            exit_changed: Condvar::new(),
            reader_handles: Mutex::new(Vec::new()),
        });

        let stdout_terminal = terminal.clone();
        let stdout_handle = std::thread::spawn(move || read_output(stdout, stdout_terminal));
        let stderr_terminal = terminal.clone();
        let stderr_handle = std::thread::spawn(move || read_output(stderr, stderr_terminal));
        *terminal.reader_handles.lock().unwrap() = vec![stdout_handle, stderr_handle];

        self.terminals
            .lock()
            .unwrap()
            .insert(id.clone(), terminal.clone());
        std::thread::spawn(move || watch_child(terminal));

        Ok(id)
    }

    pub fn output(&self, id: &str) -> Result<AgentTerminalOutput, String> {
        let terminal = self
            .terminal(id)
            .ok_or_else(|| format!("no terminal {id}"))?;
        let (output, truncated) = terminal.buffer.lock().unwrap().snapshot();
        let exit_status = terminal.exit_status.lock().unwrap().clone();
        Ok(AgentTerminalOutput {
            output,
            truncated,
            exit_status,
        })
    }

    pub fn wait_for_exit(&self, id: &str) -> Result<ExitStatus, String> {
        let terminal = self
            .terminal(id)
            .ok_or_else(|| format!("no terminal {id}"))?;
        let mut exit_status = terminal.exit_status.lock().unwrap();
        loop {
            if let Some(status) = exit_status.clone() {
                return Ok(status);
            }
            exit_status = terminal.exit_changed.wait(exit_status).unwrap();
        }
    }

    pub fn kill(&self, id: &str) {
        let terminal = self.terminal(id);
        if let Some(terminal) = terminal {
            terminate_terminal(&terminal);
        }
    }

    pub fn release(&self, id: &str) {
        let terminal = self.terminals.lock().unwrap().remove(id);
        if let Some(terminal) = terminal {
            terminate_terminal(&terminal);
        }
    }

    pub fn kill_all(&self) {
        let ids: Vec<String> = self.terminals.lock().unwrap().keys().cloned().collect();
        for id in ids {
            self.kill(&id);
        }
    }

    fn next_id() -> String {
        format!("agent-terminal-{}", NEXT_ID.fetch_add(1, Ordering::Relaxed))
    }

    fn terminal(&self, id: &str) -> Option<Arc<AgentTerminal>> {
        self.terminals.lock().unwrap().get(id).cloned()
    }
}

impl Drop for AgentTerminalManager {
    fn drop(&mut self) {
        self.kill_all();
    }
}

impl AgentTerminal {
    fn set_exit_status(&self, status: ExitStatus) {
        let mut exit_status = self.exit_status.lock().unwrap();
        if exit_status.is_none() {
            *exit_status = Some(status);
            self.exit_changed.notify_all();
        }
    }
}

impl OutputBuffer {
    fn new(limit: usize) -> Self {
        Self {
            bytes: VecDeque::new(),
            limit,
            truncated: false,
        }
    }

    fn push(&mut self, chunk: &[u8]) {
        if chunk.is_empty() {
            return;
        }
        if self.limit == 0 {
            self.truncated = true;
            self.bytes.clear();
            return;
        }
        if chunk.len() >= self.limit {
            let dropped_existing = !self.bytes.is_empty();
            let dropped_chunk = chunk.len() > self.limit;
            self.bytes.clear();
            self.bytes
                .extend(chunk[chunk.len().saturating_sub(self.limit)..].iter());
            if dropped_existing || dropped_chunk {
                self.truncated = true;
            }
            return;
        }
        let overflow = self.bytes.len() + chunk.len();
        if overflow > self.limit {
            for _ in 0..(overflow - self.limit) {
                self.bytes.pop_front();
            }
            self.truncated = true;
        }
        self.bytes.extend(chunk.iter());
    }

    fn snapshot(&self) -> (String, bool) {
        let bytes: Vec<u8> = self.bytes.iter().copied().collect();
        (String::from_utf8_lossy(&bytes).into_owned(), self.truncated)
    }
}

fn read_output<R: Read>(mut reader: R, terminal: Arc<AgentTerminal>) {
    let mut chunk = [0u8; 8192];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => return,
            Ok(n) => terminal.buffer.lock().unwrap().push(&chunk[..n]),
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => return,
        }
    }
}

fn watch_child(terminal: Arc<AgentTerminal>) {
    loop {
        let status = {
            let mut child = terminal.child.lock().unwrap();
            match child.as_mut() {
                Some(process) => match process.try_wait() {
                    Ok(Some(status)) => {
                        *child = None;
                        Some(process_exit_status(status))
                    }
                    Ok(None) => None,
                    Err(_) => {
                        *child = None;
                        Some(unknown_exit_status())
                    }
                },
                None => return,
            }
        };

        if let Some(status) = status {
            join_readers(&terminal);
            terminal.set_exit_status(status);
            return;
        }

        std::thread::sleep(Duration::from_millis(POLL_MS));
    }
}

fn terminate_terminal(terminal: &Arc<AgentTerminal>) {
    let status = {
        let mut child = terminal.child.lock().unwrap();
        match child.as_mut() {
            Some(process) => {
                let _ = process_kill::kill_tree_pid(process.id());
                let status = process
                    .wait()
                    .map(process_exit_status)
                    .unwrap_or_else(|_| unknown_exit_status());
                *child = None;
                Some(status)
            }
            None => None,
        }
    };

    join_readers(terminal);
    if let Some(status) = status {
        terminal.set_exit_status(status);
    }
}

fn join_readers(terminal: &Arc<AgentTerminal>) {
    let handles = std::mem::take(&mut *terminal.reader_handles.lock().unwrap());
    for handle in handles {
        let _ = handle.join();
    }
}

fn process_exit_status(status: ProcessExitStatus) -> ExitStatus {
    ExitStatus {
        exit_code: status.code(),
        signal: process_exit_signal(&status),
    }
}

fn unknown_exit_status() -> ExitStatus {
    ExitStatus {
        exit_code: None,
        signal: None,
    }
}

#[cfg(unix)]
fn process_exit_signal(status: &ProcessExitStatus) -> Option<String> {
    use std::os::unix::process::ExitStatusExt;

    status.signal().map(|signal| signal.to_string())
}

#[cfg(not(unix))]
fn process_exit_signal(_status: &ProcessExitStatus) -> Option<String> {
    None
}

#[tauri::command]
pub async fn agent_terminal_create(
    state: tauri::State<'_, AgentTerminalState>,
    command: String,
    args: Vec<String>,
    env: Vec<(String, String)>,
    cwd: String,
    byte_limit: usize,
) -> Result<String, String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        manager.create(&command, &args, env, &cwd, byte_limit)
    })
    .await
    .map_err(|err| err.to_string())?
}

#[tauri::command]
pub fn agent_terminal_output(
    state: tauri::State<'_, AgentTerminalState>,
    id: String,
) -> Result<AgentTerminalOutput, String> {
    state.0.output(&id)
}

#[tauri::command]
pub async fn agent_terminal_wait_for_exit(
    state: tauri::State<'_, AgentTerminalState>,
    id: String,
) -> Result<ExitStatus, String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || manager.wait_for_exit(&id))
        .await
        .map_err(|err| err.to_string())?
}

#[tauri::command]
pub async fn agent_terminal_kill(
    state: tauri::State<'_, AgentTerminalState>,
    id: String,
) -> Result<(), String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || manager.kill(&id))
        .await
        .map_err(|err| err.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn agent_terminal_release(
    state: tauri::State<'_, AgentTerminalState>,
    id: String,
) -> Result<(), String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || manager.release(&id))
        .await
        .map_err(|err| err.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn wait_until_output_contains(mgr: &AgentTerminalManager, id: &str, expected: &str) {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let output = mgr.output(id).unwrap();
            if output.output.contains(expected) {
                return;
            }
            if Instant::now() >= deadline {
                panic!("timed out waiting for output containing {expected:?}");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn runs_command_caps_output_and_reports_exit() {
        let mgr = AgentTerminalManager::new();
        let id = mgr
            .create(
                "sh",
                &["-c".into(), "printf 'abcdefgh'; exit 7".into()],
                vec![],
                ".",
                4,
            )
            .unwrap();
        let st = mgr.wait_for_exit(&id).unwrap();
        assert_eq!(st.exit_code, Some(7));
        let o = mgr.output(&id).unwrap();
        assert!(o.truncated, "4-byte cap on 8 bytes must truncate");
        assert_eq!(o.output, "efgh");
        mgr.release(&id);
    }

    #[test]
    fn exact_cap_does_not_report_truncated() {
        let mgr = AgentTerminalManager::new();
        let id = mgr
            .create("sh", &["-c".into(), "printf 'abcd'".into()], vec![], ".", 4)
            .unwrap();
        mgr.wait_for_exit(&id).unwrap();

        let o = mgr.output(&id).unwrap();
        assert_eq!(o.output, "abcd");
        assert!(!o.truncated);
        mgr.release(&id);
    }

    #[test]
    fn kill_preserves_output_and_exit_status_until_release() {
        let mgr = AgentTerminalManager::new();
        let id = mgr
            .create(
                "sh",
                &["-c".into(), "printf ready; sleep 30".into()],
                vec![],
                ".",
                1024,
            )
            .unwrap();
        wait_until_output_contains(&mgr, &id, "ready");

        mgr.kill(&id);

        let status = mgr.wait_for_exit(&id).unwrap();
        assert!(
            status.exit_code.is_some() || status.signal.is_some(),
            "killed terminal should report an exit code or signal"
        );
        let output = mgr.output(&id).unwrap();
        assert_eq!(output.output, "ready");
        assert_eq!(output.exit_status, Some(status));

        mgr.release(&id);
        assert!(mgr.output(&id).is_err());
        assert!(mgr.wait_for_exit(&id).is_err());
    }

    #[test]
    fn merges_stderr_into_output() {
        let mgr = AgentTerminalManager::new();
        let id = mgr
            .create(
                "sh",
                &["-c".into(), "printf stdout; printf stderr >&2".into()],
                vec![],
                ".",
                1024,
            )
            .unwrap();

        mgr.wait_for_exit(&id).unwrap();
        let output = mgr.output(&id).unwrap();
        assert!(output.output.contains("stdout"));
        assert!(output.output.contains("stderr"));
        mgr.release(&id);
    }

    #[test]
    fn injects_env_and_cwd() {
        let mgr = AgentTerminalManager::new();
        let cwd =
            std::env::temp_dir().join(format!("yuzora-agent-terminal-cwd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&cwd);
        std::fs::create_dir_all(&cwd).unwrap();
        let cwd = std::fs::canonicalize(&cwd).unwrap();

        let id = mgr
            .create(
                "sh",
                &[
                    "-c".into(),
                    "printf '%s|' \"$YUZORA_AGENT_TERMINAL_TEST\"; pwd -P".into(),
                ],
                vec![("YUZORA_AGENT_TERMINAL_TEST".into(), "env-value".into())],
                cwd.to_str().unwrap(),
                4096,
            )
            .unwrap();

        mgr.wait_for_exit(&id).unwrap();
        let output = mgr.output(&id).unwrap();
        assert_eq!(
            output.output.trim_end(),
            format!("env-value|{}", cwd.display())
        );
        mgr.release(&id);
        std::fs::remove_dir_all(&cwd).unwrap();
    }
}
