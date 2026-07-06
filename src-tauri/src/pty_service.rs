use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};

use crate::{logging, process_kill};
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};

pub type OnEvent = Arc<dyn Fn(PtyEvent) + Send + Sync>;
type LogFn = Box<dyn Fn(logging::LogEvent) + Send + Sync>;
static NEXT_RESERVATION_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PtySessionInfo {
    pub session_id: String,
    pub workspace: String,
    pub shell: String,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum PtyEvent {
    Output { data: String },
    Exit { code: Option<i32> },
}

struct PtySessionShared {
    info: Mutex<PtySessionInfo>,
    writer: Mutex<Box<dyn Write + Send>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    stopped: AtomicBool,
    wait_started: AtomicBool,
    pid: u32,
}

enum PtySessionEntry {
    Reserved(u64),
    Ready(Arc<PtySessionShared>),
}

pub struct PtyManager {
    sessions: Mutex<HashMap<String, PtySessionEntry>>,
    log: LogFn,
}

pub struct PtyState(pub Arc<PtyManager>);

impl PtyManager {
    pub fn new(_app: tauri::AppHandle) -> Self {
        let sink = Mutex::new(logging::LogSink::new(logging::default_log_dir()));
        let log: LogFn = Box::new(move |event| {
            if let Ok(mut sink) = sink.lock() {
                sink.write(event);
            }
        });
        Self::with_log(log)
    }

    #[cfg(test)]
    fn with_parts() -> Self {
        Self::with_log(Box::new(|_| {}))
    }

    fn with_log(log: LogFn) -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            log,
        }
    }

    pub fn open(
        self: &Arc<Self>,
        workspace: &str,
        session_id: &str,
        shell: Option<&str>,
        shell_args: Option<&[String]>,
        cols: u16,
        rows: u16,
        on_event: OnEvent,
    ) -> Result<PtySessionInfo, String> {
        let shell_path = resolve_shell(shell);
        let shell_string = shell_path.to_string_lossy().into_owned();
        let session_key = session_id.to_string();
        let reservation_id = NEXT_RESERVATION_ID.fetch_add(1, Ordering::Relaxed);

        {
            let mut map = self.sessions.lock().unwrap();
            if map.contains_key(session_id) {
                return Err(format!("pty session {session_id} already exists"));
            }
            map.insert(
                session_key.clone(),
                PtySessionEntry::Reserved(reservation_id),
            );
        }

        let opened = (|| {
            let pty_system = native_pty_system();
            let pair = pty_system
                .openpty(PtySize {
                    rows,
                    cols,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .map_err(|e| format!("openpty failed: {e}"))?;
            let reader = pair
                .master
                .try_clone_reader()
                .map_err(|e| format!("clone pty reader failed: {e}"))?;
            let writer = pair
                .master
                .take_writer()
                .map_err(|e| format!("take pty writer failed: {e}"))?;
            let mut cmd = CommandBuilder::new(&shell_path);
            cmd.env("SHELL", &shell_path);
            cmd.cwd(workspace);
            #[cfg(unix)]
            cmd.arg("-l");
            // User-configured shell args are appended after the login flag so
            // G1 login-shell behavior remains intact while still honoring A11.
            if let Some(shell_args) = shell_args {
                for arg in shell_args {
                    cmd.arg(arg);
                }
            }
            let child = pair
                .slave
                .spawn_command(cmd)
                .map_err(|e| format!("spawn shell failed: {e}"))?;
            let pid = child
                .process_id()
                .ok_or_else(|| "spawned shell has no process id".to_string())?;

            let info = PtySessionInfo {
                session_id: session_id.to_string(),
                workspace: workspace.to_string(),
                shell: shell_string,
                cols,
                rows,
            };
            let shared = Arc::new(PtySessionShared {
                info: Mutex::new(info.clone()),
                writer: Mutex::new(writer),
                master: Mutex::new(pair.master),
                stopped: AtomicBool::new(false),
                wait_started: AtomicBool::new(false),
                pid,
            });

            Ok((info, shared, reader, child))
        })();

        let (info, shared, reader, child) = match opened {
            Ok(opened) => opened,
            Err(err) => {
                self.remove_reservation(&session_key, reservation_id);
                return Err(err);
            }
        };

        let reservation_ready = {
            let mut map = self.sessions.lock().unwrap();
            match map.get(session_id) {
                Some(PtySessionEntry::Reserved(id)) if *id == reservation_id => {
                    map.insert(session_key.clone(), PtySessionEntry::Ready(shared.clone()));
                    true
                }
                _ => false,
            }
        };
        if !reservation_ready {
            let _ = process_kill::kill_tree_pid(shared.pid);
            return Err(format!("pty session {session_id} was closed while opening"));
        }

        self.log_open(&info, shared.pid);
        let weak = Arc::downgrade(self);
        let session = session_key;
        std::thread::spawn(move || {
            PtyManager::reader_loop(weak, session, shared, reader, child, on_event)
        });

        Ok(info)
    }

    pub fn write(&self, session_id: &str, data: &str) -> Result<(), String> {
        let shared = self
            .sessions
            .lock()
            .unwrap()
            .get(session_id)
            .and_then(|entry| match entry {
                PtySessionEntry::Ready(shared) => Some(shared.clone()),
                PtySessionEntry::Reserved(_) => None,
            })
            .ok_or_else(|| format!("no pty session {session_id}"))?;
        let mut writer = shared.writer.lock().unwrap();
        writer
            .write_all(data.as_bytes())
            .map_err(|e| format!("pty write failed: {e}"))?;
        writer.flush().map_err(|e| format!("pty flush failed: {e}"))
    }

    pub fn resize(&self, session_id: &str, cols: u16, rows: u16) -> Result<(), String> {
        let shared = self
            .sessions
            .lock()
            .unwrap()
            .get(session_id)
            .and_then(|entry| match entry {
                PtySessionEntry::Ready(shared) => Some(shared.clone()),
                PtySessionEntry::Reserved(_) => None,
            })
            .ok_or_else(|| format!("no pty session {session_id}"))?;
        shared
            .master
            .lock()
            .unwrap()
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| format!("pty resize failed: {e}"))?;
        let mut info = shared.info.lock().unwrap();
        info.cols = cols;
        info.rows = rows;
        Ok(())
    }

    pub fn close(&self, session_id: &str) -> Result<(), String> {
        let closed = {
            let mut map = self.sessions.lock().unwrap();
            let shared = match map.get(session_id) {
                Some(PtySessionEntry::Ready(shared)) => Some(shared.clone()),
                Some(PtySessionEntry::Reserved(_)) | None => None,
            };
            if let Some(shared) = shared {
                map.remove(session_id);
                let should_kill = Self::stop_shared(&shared);
                Some((shared, should_kill))
            } else {
                None
            }
        };
        if let Some((shared, should_kill)) = closed {
            Self::kill_shared_after_unlock(&shared, should_kill);
            let info = shared.info.lock().unwrap().clone();
            self.log_close(&info, shared.pid);
        }
        Ok(())
    }

    pub fn close_workspace(&self, workspace: &str) -> Result<(), String> {
        let session_ids: Vec<String> = {
            let map = self.sessions.lock().unwrap();
            map.iter()
                .filter(|(_, entry)| match entry {
                    PtySessionEntry::Ready(shared) => {
                        shared.info.lock().unwrap().workspace == workspace
                    }
                    PtySessionEntry::Reserved(_) => false,
                })
                .map(|(id, _)| id.clone())
                .collect()
        };
        for session_id in session_ids {
            self.close(&session_id)?;
        }
        Ok(())
    }

    pub fn kill_all(&self) {
        let sessions: Vec<(Arc<PtySessionShared>, bool)> = {
            let mut map = self.sessions.lock().unwrap();
            map.drain()
                .filter_map(|(_, entry)| match entry {
                    PtySessionEntry::Ready(shared) => {
                        let should_kill = Self::stop_shared(&shared);
                        Some((shared, should_kill))
                    }
                    PtySessionEntry::Reserved(_) => None,
                })
                .collect()
        };
        for (shared, should_kill) in sessions {
            Self::kill_shared_after_unlock(&shared, should_kill);
            let info = shared.info.lock().unwrap().clone();
            self.log_close(&info, shared.pid);
        }
    }

    pub fn sessions_for(&self, workspace: &str) -> Vec<PtySessionInfo> {
        self.sessions
            .lock()
            .unwrap()
            .values()
            .filter_map(|entry| match entry {
                PtySessionEntry::Ready(shared)
                    if shared.info.lock().unwrap().workspace == workspace =>
                {
                    Some(shared.info.lock().unwrap().clone())
                }
                PtySessionEntry::Ready(_) | PtySessionEntry::Reserved(_) => None,
            })
            .collect()
    }

    fn remove_reservation(&self, session_id: &str, reservation_id: u64) {
        let mut map = self.sessions.lock().unwrap();
        if matches!(
            map.get(session_id),
            Some(PtySessionEntry::Reserved(id)) if *id == reservation_id
        ) {
            map.remove(session_id);
        }
    }

    fn stop_shared(shared: &PtySessionShared) -> bool {
        // Callers must hold the sessions map lock here so the EOF reader cannot
        // set wait_started between this decision and removing/draining the map entry.
        shared.stopped.store(true, Ordering::SeqCst);
        !shared.wait_started.load(Ordering::SeqCst)
    }

    fn kill_shared_after_unlock(shared: &PtySessionShared, should_kill: bool) {
        // kill_tree_pid waits for a grace timeout, so the map lock must be
        // released before calling it; the pid-reuse decision was made above.
        if should_kill {
            let _ = process_kill::kill_tree_pid(shared.pid);
        }
    }

    fn reader_loop(
        manager: Weak<Self>,
        session_id: String,
        shared: Arc<PtySessionShared>,
        mut reader: Box<dyn Read + Send>,
        mut child: Box<dyn portable_pty::Child + Send + Sync>,
        on_event: OnEvent,
    ) {
        let mut chunker = Utf8Chunker::default();
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if let Some(data) = chunker.push(&buf[..n]) {
                        if !data.is_empty() {
                            on_event(PtyEvent::Output { data });
                        }
                    }
                }
            }
        }

        if let Some(manager) = manager.upgrade() {
            let map = manager.sessions.lock().unwrap();
            if let Some(PtySessionEntry::Ready(existing)) = map.get(&session_id) {
                if Arc::ptr_eq(existing, &shared) {
                    shared.wait_started.store(true, Ordering::SeqCst);
                }
            }
        }

        let code = child.wait().ok().map(|status| status.exit_code() as i32);
        if let Some(data) = chunker.finish_lossy() {
            if !data.is_empty() {
                on_event(PtyEvent::Output { data });
            }
        }
        on_event(PtyEvent::Exit { code });

        if let Some(manager) = manager.upgrade() {
            let mut map = manager.sessions.lock().unwrap();
            if let Some(PtySessionEntry::Ready(existing)) = map.get(&session_id) {
                if Arc::ptr_eq(existing, &shared) {
                    map.remove(&session_id);
                }
            }
            drop(map);
            let info = shared.info.lock().unwrap().clone();
            manager.log_exit(
                &info,
                shared.pid,
                code,
                shared.stopped.load(Ordering::SeqCst),
            );
        }
    }

    #[cfg(test)]
    fn debug_pid(&self, session_id: &str) -> Option<u32> {
        self.sessions
            .lock()
            .unwrap()
            .get(session_id)
            .and_then(|entry| match entry {
                PtySessionEntry::Ready(shared) => Some(shared.pid),
                PtySessionEntry::Reserved(_) => None,
            })
    }

    fn log_open(&self, info: &PtySessionInfo, pid: u32) {
        (self.log)(logging::LogEvent {
            level: "info".into(),
            kind: "debug".into(),
            source: "pty".into(),
            workspace_path: Some(info.workspace.clone()),
            event: "pty_open".into(),
            message: format!("pty session {} opened", info.session_id),
            metadata: serde_json::json!({
                "sessionId": info.session_id,
                "workspace": info.workspace,
                "shell": info.shell,
                "pid": pid,
                "cols": info.cols,
                "rows": info.rows,
            }),
        });
    }

    fn log_close(&self, info: &PtySessionInfo, pid: u32) {
        (self.log)(logging::LogEvent {
            level: "info".into(),
            kind: "debug".into(),
            source: "pty".into(),
            workspace_path: Some(info.workspace.clone()),
            event: "pty_close".into(),
            message: format!("pty session {} closed", info.session_id),
            metadata: serde_json::json!({
                "sessionId": info.session_id,
                "workspace": info.workspace,
                "shell": info.shell,
                "pid": pid,
            }),
        });
    }

    fn log_exit(&self, info: &PtySessionInfo, pid: u32, code: Option<i32>, stopped: bool) {
        (self.log)(logging::LogEvent {
            level: "info".into(),
            kind: "debug".into(),
            source: "pty".into(),
            workspace_path: Some(info.workspace.clone()),
            event: "pty_exit".into(),
            message: format!("pty session {} exited (code {code:?})", info.session_id),
            metadata: serde_json::json!({
                "sessionId": info.session_id,
                "workspace": info.workspace,
                "shell": info.shell,
                "pid": pid,
                "exitCode": code,
                "stopped": stopped,
            }),
        });
    }
}

impl Drop for PtyManager {
    fn drop(&mut self) {
        self.kill_all();
    }
}

#[derive(Default)]
struct Utf8Chunker {
    pending: Vec<u8>,
}

impl Utf8Chunker {
    fn push(&mut self, bytes: &[u8]) -> Option<String> {
        self.pending.extend_from_slice(bytes);
        let mut output = String::new();

        loop {
            match std::str::from_utf8(&self.pending) {
                Ok(valid) => {
                    output.push_str(valid);
                    self.pending.clear();
                    break;
                }
                Err(err) => {
                    let valid_up_to = err.valid_up_to();
                    if valid_up_to > 0 {
                        let complete = self.pending.drain(..valid_up_to).collect::<Vec<_>>();
                        output.push_str(&String::from_utf8(complete).unwrap_or_default());
                    }

                    if let Some(error_len) = err.error_len() {
                        self.pending.drain(..error_len);
                        output.push('\u{fffd}');
                        continue;
                    }

                    break;
                }
            }
        }

        if output.is_empty() {
            None
        } else {
            Some(output)
        }
    }

    fn finish_lossy(&mut self) -> Option<String> {
        if self.pending.is_empty() {
            None
        } else {
            let text = String::from_utf8_lossy(&self.pending).into_owned();
            self.pending.clear();
            Some(text)
        }
    }
}

pub fn resolve_shell(override_shell: Option<&str>) -> PathBuf {
    resolve_shell_from(
        override_shell,
        std::env::var_os("SHELL").map(PathBuf::from),
        passwd_shell(),
    )
}

fn resolve_shell_from(
    override_shell: Option<&str>,
    env_shell: Option<PathBuf>,
    fallback_shell: Option<PathBuf>,
) -> PathBuf {
    if let Some(shell) = override_shell.filter(|s| !s.trim().is_empty()) {
        return PathBuf::from(shell);
    }
    if let Some(shell) = env_shell {
        return shell;
    }
    fallback_shell.unwrap_or_else(default_shell)
}

#[cfg(unix)]
fn passwd_shell() -> Option<PathBuf> {
    use std::ffi::CStr;

    let entry = unsafe { libc::getpwuid(libc::getuid()) };
    if entry.is_null() {
        return None;
    }
    let shell = unsafe { CStr::from_ptr((*entry).pw_shell) };
    let path = PathBuf::from(shell.to_string_lossy().into_owned());
    if is_executable(&path) {
        Some(path)
    } else {
        None
    }
}

#[cfg(not(unix))]
fn passwd_shell() -> Option<PathBuf> {
    None
}

#[cfg(unix)]
fn is_executable(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

fn default_shell() -> PathBuf {
    #[cfg(windows)]
    {
        std::env::var_os("ComSpec")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("cmd.exe"))
    }
    #[cfg(all(unix, target_os = "macos"))]
    {
        PathBuf::from("/bin/zsh")
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        PathBuf::from("/bin/sh")
    }
}

#[tauri::command]
pub async fn pty_open(
    state: tauri::State<'_, PtyState>,
    workspace: String,
    session_id: String,
    shell: Option<String>,
    shell_args: Option<Vec<String>>,
    cols: u16,
    rows: u16,
    on_event: tauri::ipc::Channel<PtyEvent>,
) -> Result<PtySessionInfo, String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let channel = on_event;
        let on_event: OnEvent = Arc::new(move |event| {
            let _ = channel.send(event);
        });
        manager.open(
            &workspace,
            &session_id,
            shell.as_deref(),
            shell_args.as_deref(),
            cols,
            rows,
            on_event,
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn pty_write(
    state: tauri::State<'_, PtyState>,
    session_id: String,
    data: String,
) -> Result<(), String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || manager.write(&session_id, &data))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn pty_resize(
    state: tauri::State<'_, PtyState>,
    session_id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || manager.resize(&session_id, cols, rows))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn pty_close(
    state: tauri::State<'_, PtyState>,
    session_id: String,
) -> Result<(), String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || manager.close(&session_id))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn pty_close_workspace(
    state: tauri::State<'_, PtyState>,
    workspace: String,
) -> Result<(), String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || manager.close_workspace(&workspace))
        .await
        .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn test_manager() -> Arc<PtyManager> {
        Arc::new(PtyManager::with_parts())
    }

    fn capture_events() -> (OnEvent, Arc<Mutex<Vec<PtyEvent>>>) {
        let events: Arc<Mutex<Vec<PtyEvent>>> = Default::default();
        let e2 = events.clone();
        let on_event: OnEvent = Arc::new(move |event| e2.lock().unwrap().push(event));
        (on_event, events)
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
    fn lifecycle_open_write_echo_close_and_close_workspace() {
        let mgr = test_manager();
        let (on_event, events) = capture_events();
        mgr.open("ws-a", "s1", Some("/bin/sh"), None, 80, 24, on_event)
            .unwrap();

        mgr.write("s1", "echo hi\n").unwrap();
        assert!(poll_until(Duration::from_secs(3), || events
            .lock()
            .unwrap()
            .iter()
            .any(
                |event| matches!(event, PtyEvent::Output { data } if data.contains("hi"))
            )));

        let pid = mgr.debug_pid("s1").unwrap();
        mgr.close("s1").unwrap();
        assert!(mgr.sessions_for("ws-a").is_empty());
        #[cfg(unix)]
        assert!(poll_until(Duration::from_secs(3), || !process_exists(pid)));

        let (on_event2, _events2) = capture_events();
        mgr.open("ws-a", "s2", Some("/bin/sh"), None, 80, 24, on_event2)
            .unwrap();
        let pid2 = mgr.debug_pid("s2").unwrap();
        mgr.close_workspace("ws-a").unwrap();
        assert!(mgr.sessions_for("ws-a").is_empty());
        #[cfg(unix)]
        assert!(poll_until(Duration::from_secs(3), || !process_exists(pid2)));
    }

    #[test]
    fn resize_is_reflected_in_sessions_for() {
        let mgr = test_manager();
        let (on_event, _events) = capture_events();
        mgr.open("ws-a", "s1", Some("/bin/sh"), None, 80, 24, on_event)
            .unwrap();
        mgr.resize("s1", 120, 40).unwrap();
        let sessions = mgr.sessions_for("ws-a");
        assert_eq!(sessions[0].cols, 120);
        assert_eq!(sessions[0].rows, 40);
        mgr.close("s1").unwrap();
    }

    #[test]
    fn resolve_shell_prefers_override_then_env_then_existing_fallback() {
        assert_eq!(
            resolve_shell_from(Some("/custom/shell"), Some(PathBuf::from("/bin/sh")), None),
            PathBuf::from("/custom/shell")
        );
        assert_eq!(
            resolve_shell_from(None, Some(PathBuf::from("/env/shell")), None),
            PathBuf::from("/env/shell")
        );
        let fallback = resolve_shell_from(None, None, Some(PathBuf::from("/bin/sh")));
        assert!(
            fallback.exists(),
            "fallback shell should exist: {fallback:?}"
        );
    }

    #[test]
    fn kill_all_clears_map_and_kills_processes() {
        let mgr = test_manager();
        let (on_event, _events) = capture_events();
        mgr.open("ws-a", "s1", Some("/bin/sh"), None, 80, 24, on_event)
            .unwrap();
        let pid = mgr.debug_pid("s1").unwrap();
        mgr.kill_all();
        assert!(mgr.sessions_for("ws-a").is_empty());
        #[cfg(unix)]
        assert!(poll_until(Duration::from_secs(3), || !process_exists(pid)));
    }

    #[test]
    fn duplicate_open_returns_err_and_keeps_original_session_usable() {
        let mgr = test_manager();
        let (on_event, events) = capture_events();
        mgr.open("ws-a", "s1", Some("/bin/sh"), None, 80, 24, on_event)
            .unwrap();

        let (duplicate_on_event, _duplicate_events) = capture_events();
        let duplicate = mgr.open(
            "ws-b",
            "s1",
            Some("/bin/sh"),
            None,
            120,
            40,
            duplicate_on_event,
        );
        assert!(duplicate.is_err());

        mgr.write("s1", "echo still-here\n").unwrap();
        assert!(poll_until(Duration::from_secs(3), || events
            .lock()
            .unwrap()
            .iter()
            .any(
                |event| matches!(event, PtyEvent::Output { data } if data.contains("still-here"))
            )));
        let sessions = mgr.sessions_for("ws-a");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].cols, 80);
        assert!(mgr.sessions_for("ws-b").is_empty());
        mgr.close("s1").unwrap();
    }

    #[test]
    fn close_is_idempotent() {
        let mgr = test_manager();
        let (on_event, _events) = capture_events();
        mgr.open("ws-a", "s1", Some("/bin/sh"), None, 80, 24, on_event)
            .unwrap();
        mgr.close("s1").unwrap();
        mgr.close("s1").unwrap();
        mgr.close_workspace("ws-a").unwrap();
        mgr.close_workspace("missing").unwrap();
    }

    #[test]
    fn close_after_natural_exit_is_idempotent() {
        let mgr = test_manager();
        let (on_event, events) = capture_events();
        mgr.open("ws-a", "s1", Some("/bin/sh"), None, 80, 24, on_event)
            .unwrap();
        mgr.write("s1", "exit 0\n").unwrap();

        assert!(poll_until(Duration::from_secs(5), || events
            .lock()
            .unwrap()
            .iter()
            .any(|event| matches!(event, PtyEvent::Exit { code: Some(0) }))));

        mgr.close("s1").unwrap();
        assert!(mgr.sessions_for("ws-a").is_empty());
    }

    #[test]
    fn output_events_arrive_before_exit_and_exit_code_is_sourced() {
        let mgr = test_manager();
        let (on_event, events) = capture_events();
        mgr.open("ws-a", "s1", Some("/bin/sh"), None, 80, 24, on_event)
            .unwrap();
        mgr.write("s1", "echo hi\nexit 3\n").unwrap();

        assert!(poll_until(Duration::from_secs(5), || events
            .lock()
            .unwrap()
            .iter()
            .any(|event| matches!(event, PtyEvent::Exit { code: Some(3) }))));

        let events = events.lock().unwrap();
        let output_index = events
            .iter()
            .position(|event| matches!(event, PtyEvent::Output { data } if data.contains("hi")))
            .expect("output containing hi");
        let exit_index = events
            .iter()
            .position(|event| matches!(event, PtyEvent::Exit { code: Some(3) }))
            .expect("exit 3 event");
        assert!(output_index < exit_index);
    }

    #[test]
    fn shell_args_reach_spawn_command() {
        let mgr = test_manager();
        let (on_event, events) = capture_events();
        let shell_args = vec!["-c".to_string(), "echo shell-args-ok".to_string()];
        mgr.open(
            "ws-a",
            "args",
            Some("/bin/sh"),
            Some(&shell_args),
            80,
            24,
            on_event,
        )
        .unwrap();

        assert!(poll_until(Duration::from_secs(5), || events
            .lock()
            .unwrap()
            .iter()
            .any(
                |event| matches!(event, PtyEvent::Output { data } if data.contains("shell-args-ok"))
            )));
        assert!(poll_until(Duration::from_secs(5), || events
            .lock()
            .unwrap()
            .iter()
            .any(|event| matches!(event, PtyEvent::Exit { code: Some(0) }))));
    }

    #[test]
    fn utf8_boundary_chunker_reassembles_split_multibyte_char() {
        let mut chunker = Utf8Chunker::default();
        let euro = "€".as_bytes();
        assert_eq!(chunker.push(&euro[..1]), None);
        assert_eq!(chunker.push(&euro[1..]), Some("€".to_string()));
        assert_eq!(chunker.finish_lossy(), None);
    }

    #[test]
    fn utf8_chunker_preserves_incomplete_tail_after_invalid_bytes() {
        let mut chunker = Utf8Chunker::default();
        let euro = "€".as_bytes();
        let first = [b'a', 0xff, euro[0], euro[1]];

        assert_eq!(chunker.push(&first), Some("a\u{fffd}".to_string()));
        assert_eq!(chunker.push(&euro[2..]), Some("€".to_string()));
        assert_eq!(chunker.finish_lossy(), None);
    }

    #[test]
    fn missing_session_errors_and_nonexistent_shell_returns_err() {
        let mgr = test_manager();
        assert!(mgr.write("missing", "x").is_err());
        assert!(mgr.resize("missing", 80, 24).is_err());
        let (on_event, _events) = capture_events();
        assert!(mgr
            .open(
                "ws-a",
                "bad",
                Some("/definitely/not/a/shell"),
                None,
                80,
                24,
                on_event,
            )
            .is_err());
    }
}
