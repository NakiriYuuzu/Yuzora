use std::borrow::Cow;
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PtyActivity {
    Idle,
    Busy,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalProfile {
    pub id: String,
    pub name: String,
    pub shell: String,
    pub args: Vec<String>,
    pub kind: TerminalProfileKind,
    pub cwd_strategy: TerminalCwdStrategy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TerminalProfileKind {
    Cmd,
    Powershell,
    Wsl,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TerminalCwdStrategy {
    Native,
    Wsl,
}

struct PtySessionShared {
    info: Mutex<PtySessionInfo>,
    writer: Mutex<Option<Box<dyn Write + Send>>>,
    master: Mutex<Option<Box<dyn MasterPty + Send>>>,
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

fn shell_spawn_cwd(workspace: &str) -> Cow<'_, str> {
    let Some(path) = workspace.strip_prefix(r"\\?\") else {
        return Cow::Borrowed(workspace);
    };
    if path
        .get(..4)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(r"UNC\"))
    {
        return Cow::Owned(format!(r"\\{}", &path[4..]));
    }

    let bytes = path.as_bytes();
    if bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/')
    {
        Cow::Borrowed(path)
    } else {
        // Other namespaces (for example Volume GUID paths) have no safe
        // non-verbatim equivalent, so keep their operational form intact.
        Cow::Borrowed(workspace)
    }
}

#[derive(Debug, PartialEq, Eq)]
struct ShellSpawnPlan {
    cwd: String,
    args: Vec<String>,
}

fn is_wsl_shell(shell: &std::path::Path) -> bool {
    shell
        .to_string_lossy()
        .rsplit(['/', '\\'])
        .next()
        .is_some_and(|name| {
            name.eq_ignore_ascii_case("wsl") || name.eq_ignore_ascii_case("wsl.exe")
        })
}

fn wsl_requested_distro(args: &[String]) -> Option<&str> {
    args.windows(2).find_map(|pair| {
        (pair[0] == "--distribution" || pair[0] == "-d").then_some(pair[1].as_str())
    })
}

fn wsl_unc_target(workspace: &str) -> Option<(String, String)> {
    let normalized = shell_spawn_cwd(workspace);
    let path = normalized.strip_prefix(r"\\")?;
    let mut segments = path.split(['\\', '/']);
    let server = segments.next()?;
    if !server.eq_ignore_ascii_case("wsl.localhost") && !server.eq_ignore_ascii_case("wsl$") {
        return None;
    }
    let distro = segments.next()?.trim();
    if distro.is_empty() {
        return None;
    }
    let remainder = segments
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    let linux_cwd = if remainder.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", remainder.join("/"))
    };
    Some((distro.to_string(), linux_cwd))
}

fn shell_spawn_plan(
    workspace: &str,
    cwd_strategy: TerminalCwdStrategy,
    configured_args: Option<&[String]>,
) -> Result<ShellSpawnPlan, String> {
    let mut args = configured_args.unwrap_or_default().to_vec();
    if cwd_strategy != TerminalCwdStrategy::Wsl {
        return Ok(ShellSpawnPlan {
            cwd: shell_spawn_cwd(workspace).into_owned(),
            args,
        });
    }

    let Some((workspace_distro, linux_cwd)) = wsl_unc_target(workspace) else {
        return Ok(ShellSpawnPlan {
            cwd: shell_spawn_cwd(workspace).into_owned(),
            args,
        });
    };

    if args
        .iter()
        .any(|arg| arg == "--exec" || arg == "-e" || arg == "--")
    {
        return Err(
            "WSL UNC workspaces cannot combine automatic --cd with a custom exec command"
                .to_string(),
        );
    }
    if let Some(profile_distro) = wsl_requested_distro(&args) {
        if !profile_distro.eq_ignore_ascii_case(&workspace_distro) {
            return Err(format!(
                "WSL profile distro {profile_distro:?} does not match workspace distro {workspace_distro:?}"
            ));
        }
    } else {
        args.splice(
            0..0,
            ["--distribution".to_string(), workspace_distro.clone()],
        );
    }
    args.extend(["--cd".to_string(), linux_cwd]);

    let cwd = std::env::current_dir()
        .map_err(|error| format!("resolve host cwd for WSL workspace failed: {error}"))?
        .to_string_lossy()
        .into_owned();
    Ok(ShellSpawnPlan { cwd, args })
}

#[cfg(any(windows, test))]
fn decode_wsl_list_output(bytes: &[u8]) -> Vec<String> {
    let looks_utf16le =
        bytes.starts_with(&[0xff, 0xfe]) || bytes.chunks_exact(2).take(32).any(|pair| pair[1] == 0);
    let text = if looks_utf16le {
        let start = usize::from(bytes.starts_with(&[0xff, 0xfe])) * 2;
        let units = bytes[start..]
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    };

    text.lines()
        .map(|line| line.trim_matches(['\u{feff}', '\0', ' ', '\t', '\r']))
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

#[cfg(windows)]
fn detect_windows_terminal_profiles() -> Vec<TerminalProfile> {
    let mut profiles = Vec::new();
    let system_root = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    let cmd = std::env::var_os("ComSpec")
        .map(PathBuf::from)
        .unwrap_or_else(|| system_root.join(r"System32\cmd.exe"));
    profiles.push(TerminalProfile {
        id: "cmd".to_string(),
        name: "Command Prompt".to_string(),
        shell: cmd.to_string_lossy().into_owned(),
        args: Vec::new(),
        kind: TerminalProfileKind::Cmd,
        cwd_strategy: TerminalCwdStrategy::Native,
    });

    let windows_powershell = system_root.join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
    if windows_powershell.is_file() {
        profiles.push(TerminalProfile {
            id: "windows-powershell".to_string(),
            name: "Windows PowerShell".to_string(),
            shell: windows_powershell.to_string_lossy().into_owned(),
            args: vec!["-NoLogo".to_string()],
            kind: TerminalProfileKind::Powershell,
            cwd_strategy: TerminalCwdStrategy::Native,
        });
    }

    let pwsh = std::env::var_os("ProgramFiles")
        .map(PathBuf::from)
        .map(|program_files| program_files.join(r"PowerShell\7\pwsh.exe"))
        .filter(|path| path.is_file())
        .or_else(|| {
            std::env::var_os("PATH").and_then(|paths| {
                std::env::split_paths(&paths)
                    .map(|directory| directory.join("pwsh.exe"))
                    .find(|path| path.is_file())
            })
        });
    if let Some(pwsh) = pwsh {
        profiles.push(TerminalProfile {
            id: "powershell-7".to_string(),
            name: "PowerShell 7".to_string(),
            shell: pwsh.to_string_lossy().into_owned(),
            args: vec!["-NoLogo".to_string()],
            kind: TerminalProfileKind::Powershell,
            cwd_strategy: TerminalCwdStrategy::Native,
        });
    }

    let wsl = system_root.join(r"System32\wsl.exe");
    if wsl.is_file() {
        let wsl_shell = wsl.to_string_lossy().into_owned();
        profiles.push(TerminalProfile {
            id: "wsl".to_string(),
            name: "WSL (default distro)".to_string(),
            shell: wsl_shell.clone(),
            args: Vec::new(),
            kind: TerminalProfileKind::Wsl,
            cwd_strategy: TerminalCwdStrategy::Wsl,
        });
        if let Ok(output) = std::process::Command::new(&wsl)
            .args(["--list", "--quiet"])
            .output()
        {
            if output.status.success() {
                for distro in decode_wsl_list_output(&output.stdout) {
                    profiles.push(TerminalProfile {
                        id: format!("wsl:{distro}"),
                        name: format!("WSL: {distro}"),
                        shell: wsl_shell.clone(),
                        args: vec!["--distribution".to_string(), distro],
                        kind: TerminalProfileKind::Wsl,
                        cwd_strategy: TerminalCwdStrategy::Wsl,
                    });
                }
            }
        }
    }
    profiles
}

#[cfg(not(windows))]
fn detect_windows_terminal_profiles() -> Vec<TerminalProfile> {
    Vec::new()
}

#[cfg(unix)]
fn classify_pty_activity(
    shell_pid: u32,
    foreground_process_group: Option<libc::pid_t>,
) -> PtyActivity {
    match foreground_process_group {
        Some(process_group) if process_group == shell_pid as libc::pid_t => PtyActivity::Idle,
        Some(_) => PtyActivity::Busy,
        None => PtyActivity::Unknown,
    }
}

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

    // Args mirror the `pty_open` IPC command shape; grouping into a struct would
    // diverge from the other pty commands and the JS call site.
    #[allow(clippy::too_many_arguments)]
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
        self.open_with_cwd_strategy(
            workspace, session_id, shell, shell_args, None, cols, rows, on_event,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn open_with_cwd_strategy(
        self: &Arc<Self>,
        workspace: &str,
        session_id: &str,
        shell: Option<&str>,
        shell_args: Option<&[String]>,
        cwd_strategy: Option<TerminalCwdStrategy>,
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
            let cwd_strategy = cwd_strategy.unwrap_or_else(|| {
                if is_wsl_shell(&shell_path) {
                    TerminalCwdStrategy::Wsl
                } else {
                    TerminalCwdStrategy::Native
                }
            });
            let spawn_plan = shell_spawn_plan(workspace, cwd_strategy, shell_args)?;
            let mut cmd = CommandBuilder::new(&shell_path);
            #[cfg(unix)]
            cmd.env("SHELL", &shell_path);
            cmd.env("TERM", "xterm-256color");
            cmd.cwd(&spawn_plan.cwd);
            #[cfg(unix)]
            cmd.arg("-l");
            // User-configured shell args are appended after the login flag so
            // G1 login-shell behavior remains intact while still honoring A11.
            for arg in &spawn_plan.args {
                cmd.arg(arg);
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
                writer: Mutex::new(Some(writer)),
                master: Mutex::new(Some(pair.master)),
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
        let writer = writer
            .as_mut()
            .ok_or_else(|| format!("pty session {session_id} is closed"))?;
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
        let master = shared.master.lock().unwrap();
        let master = master
            .as_ref()
            .ok_or_else(|| format!("pty session {session_id} is closed"))?;
        master
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

    pub fn activity(&self, session_id: &str) -> PtyActivity {
        let shared = self
            .sessions
            .lock()
            .unwrap()
            .get(session_id)
            .and_then(|entry| match entry {
                PtySessionEntry::Ready(shared) => Some(shared.clone()),
                PtySessionEntry::Reserved(_) => None,
            });
        let Some(shared) = shared else {
            return PtyActivity::Unknown;
        };

        #[cfg(unix)]
        {
            let foreground_process_group = shared
                .master
                .lock()
                .unwrap()
                .as_ref()
                .and_then(|master| master.process_group_leader());
            classify_pty_activity(shared.pid, foreground_process_group)
        }

        #[cfg(not(unix))]
        {
            let _ = shared;
            PtyActivity::Unknown
        }
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
        Self::release_pty_handles(shared);
    }

    fn release_pty_handles(shared: &PtySessionShared) {
        // Dropping the writer closes ConPTY input. Dropping the master owns the
        // platform PTY teardown; on Windows portable-pty calls ClosePseudoConsole.
        // Take both out of their mutexes first so ClosePseudoConsole may wait
        // while the dedicated reader thread continues draining final output.
        let writer = shared.writer.lock().unwrap().take();
        drop(writer);
        let master = shared.master.lock().unwrap().take();
        drop(master);
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
pub async fn pty_list_profiles() -> Result<Vec<TerminalProfile>, String> {
    tauri::async_runtime::spawn_blocking(detect_windows_terminal_profiles)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn pty_open(
    state: tauri::State<'_, PtyState>,
    workspace: String,
    session_id: String,
    shell: Option<String>,
    shell_args: Option<Vec<String>>,
    cwd_strategy: Option<TerminalCwdStrategy>,
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
        manager.open_with_cwd_strategy(
            &workspace,
            &session_id,
            shell.as_deref(),
            shell_args.as_deref(),
            cwd_strategy,
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
pub async fn pty_activity(
    state: tauri::State<'_, PtyState>,
    session_id: String,
) -> Result<PtyActivity, String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || manager.activity(&session_id))
        .await
        .map_err(|e| e.to_string())
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
    use std::fs;
    #[cfg(windows)]
    use std::path::Path;
    use std::time::{Duration, Instant};

    fn test_manager() -> Arc<PtyManager> {
        Arc::new(PtyManager::with_parts())
    }

    #[cfg(unix)]
    #[test]
    fn activity_uses_the_shell_foreground_process_group() {
        assert_eq!(classify_pty_activity(42, Some(42)), PtyActivity::Idle);
        assert_eq!(classify_pty_activity(42, Some(84)), PtyActivity::Busy);
        assert_eq!(classify_pty_activity(42, None), PtyActivity::Unknown);
    }

    fn retained_test_session(session_id: &str) -> Arc<PtySessionShared> {
        let pair = native_pty_system().openpty(PtySize::default()).unwrap();
        let writer = pair.master.take_writer().unwrap();
        let shared = Arc::new(PtySessionShared {
            info: Mutex::new(PtySessionInfo {
                session_id: session_id.to_string(),
                workspace: "ws-a".to_string(),
                shell: "test-shell".to_string(),
                cols: 80,
                rows: 24,
            }),
            writer: Mutex::new(Some(writer)),
            master: Mutex::new(Some(pair.master)),
            stopped: AtomicBool::new(false),
            // Model the reader thread having reached child.wait(): close must
            // release PTY resources without trying to kill the synthetic pid.
            wait_started: AtomicBool::new(true),
            pid: 0,
        });
        drop(pair.slave);
        shared
    }

    #[test]
    fn close_releases_pty_handles_while_reader_retains_shared() {
        let mgr = test_manager();
        let retained = retained_test_session("retained-close");
        mgr.sessions.lock().unwrap().insert(
            "retained-close".to_string(),
            PtySessionEntry::Ready(retained.clone()),
        );

        mgr.close("retained-close").unwrap();

        assert!(retained.writer.lock().unwrap().is_none());
        assert!(retained.master.lock().unwrap().is_none());
    }

    #[test]
    fn kill_all_releases_pty_handles_while_reader_retains_shared() {
        let mgr = test_manager();
        let retained = retained_test_session("retained-kill-all");
        mgr.sessions.lock().unwrap().insert(
            "retained-kill-all".to_string(),
            PtySessionEntry::Ready(retained.clone()),
        );

        mgr.kill_all();

        assert!(retained.writer.lock().unwrap().is_none());
        assert!(retained.master.lock().unwrap().is_none());
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

    #[test]
    fn shell_spawn_cwd_hides_windows_verbatim_drive_prefix() {
        assert_eq!(
            shell_spawn_cwd(r"\\?\D:\Projects\xxxx").as_ref(),
            r"D:\Projects\xxxx"
        );
    }

    #[test]
    fn shell_spawn_cwd_converts_windows_verbatim_unc_prefix() {
        assert_eq!(
            shell_spawn_cwd(r"\\?\UNC\server\share\project").as_ref(),
            r"\\server\share\project"
        );
    }

    #[test]
    fn shell_spawn_cwd_preserves_normal_and_unknown_namespace_paths() {
        assert_eq!(
            shell_spawn_cwd(r"D:\Projects\xxxx").as_ref(),
            r"D:\Projects\xxxx"
        );
        assert_eq!(
            shell_spawn_cwd(r"\\?\Volume{01234567-89ab-cdef-0123-456789abcdef}\project").as_ref(),
            r"\\?\Volume{01234567-89ab-cdef-0123-456789abcdef}\project"
        );
    }

    #[test]
    fn decodes_utf16le_wsl_distro_output_without_nul_characters() {
        let utf16 = "Ubuntu\r\nDebian\r\n"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();

        assert_eq!(
            decode_wsl_list_output(&utf16),
            vec!["Ubuntu".to_string(), "Debian".to_string()]
        );
    }

    #[test]
    fn parses_wsl_unc_workspace_into_distro_and_linux_cwd() {
        assert_eq!(
            wsl_unc_target(r"\\wsl.localhost\Ubuntu\home\yuuzu\專案"),
            Some(("Ubuntu".to_string(), "/home/yuuzu/專案".to_string()))
        );
        assert_eq!(
            wsl_unc_target(r"\\wsl$\Debian\home\yuuzu"),
            Some(("Debian".to_string(), "/home/yuuzu".to_string()))
        );
        assert_eq!(wsl_unc_target(r"C:\Users\yuuzu\project"), None);
    }

    #[test]
    fn wsl_unc_spawn_plan_injects_matching_distro_and_linux_cwd() {
        let plan = shell_spawn_plan(
            r"\\wsl.localhost\Ubuntu\home\yuuzu\project",
            TerminalCwdStrategy::Wsl,
            None,
        )
        .unwrap();
        assert_eq!(
            plan.args,
            vec!["--distribution", "Ubuntu", "--cd", "/home/yuuzu/project"]
        );

        let selected = vec!["--distribution".to_string(), "Ubuntu".to_string()];
        let plan = shell_spawn_plan(
            r"\\wsl$\Ubuntu\home\yuuzu",
            TerminalCwdStrategy::Wsl,
            Some(&selected),
        )
        .unwrap();
        assert_eq!(
            plan.args,
            vec!["--distribution", "Ubuntu", "--cd", "/home/yuuzu"]
        );
    }

    #[test]
    fn wsl_unc_spawn_plan_rejects_a_profile_for_another_distro() {
        let selected = vec!["--distribution".to_string(), "Debian".to_string()];
        let error = shell_spawn_plan(
            r"\\wsl.localhost\Ubuntu\home\yuuzu",
            TerminalCwdStrategy::Wsl,
            Some(&selected),
        )
        .unwrap_err();
        assert!(error.contains("Debian"));
        assert!(error.contains("Ubuntu"));
    }

    const CWD_FRAME_BEGIN: &str = "__YUZORA_CWD_BEGIN__";
    const CWD_FRAME_END: &str = "__YUZORA_CWD_END__";

    fn framed_cwd(events: &Arc<Mutex<Vec<PtyEvent>>>) -> Option<String> {
        let output = events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|event| match event {
                PtyEvent::Output { data } => Some(data.as_str()),
                PtyEvent::Exit { .. } => None,
            })
            .collect::<String>();
        let framed = output.rsplit_once(CWD_FRAME_BEGIN)?.1;
        let value = framed.split_once(CWD_FRAME_END)?.0;
        Some(value.trim().to_string())
    }

    fn special_character_workspace() -> (tempfile::TempDir, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("專案 空間 #100%");
        fs::create_dir(&workspace).unwrap();
        (root, workspace)
    }

    #[cfg(unix)]
    #[test]
    fn custom_shell_starts_in_existing_workspace_with_special_characters() {
        let (_root, workspace) = special_character_workspace();
        let expected = workspace
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let shell_args = vec![
            "-c".to_string(),
            format!("printf '%s\\n' {CWD_FRAME_BEGIN}; pwd; printf '%s\\n' {CWD_FRAME_END}"),
        ];
        let mgr = test_manager();
        let (on_event, events) = capture_events();

        mgr.open(
            workspace.to_str().unwrap(),
            "cwd-custom-shell",
            Some("/bin/sh"),
            Some(&shell_args),
            80,
            24,
            on_event,
        )
        .unwrap();

        assert!(
            poll_until(Duration::from_secs(5), || framed_cwd(&events).as_deref()
                == Some(expected.as_str())),
            "PTY output did not contain workspace cwd {expected:?}: {:?}",
            events.lock().unwrap()
        );
        mgr.close("cwd-custom-shell").unwrap();
    }

    #[cfg(windows)]
    fn normalized_windows_text(value: &str) -> String {
        value.replace('\r', "").to_lowercase()
    }

    #[cfg(windows)]
    fn assert_windows_shell_starts_in_workspace(
        session_id: &str,
        shell: &Path,
        shell_args: &[String],
    ) {
        let (_root, workspace) = special_character_workspace();
        let workspace = workspace.canonicalize().unwrap();
        let workspace = workspace.to_string_lossy().into_owned();
        assert!(
            workspace.starts_with(r"\\?\"),
            "test setup did not produce a verbatim workspace path: {workspace:?}"
        );
        let expected = normalized_windows_text(shell_spawn_cwd(&workspace).as_ref());
        let mgr = test_manager();
        let (on_event, events) = capture_events();

        mgr.open(
            &workspace,
            session_id,
            Some(shell.to_str().unwrap()),
            Some(shell_args),
            80,
            24,
            on_event,
        )
        .unwrap();

        assert!(
            poll_until(Duration::from_secs(10), || framed_cwd(&events)
                .map(|value| normalized_windows_text(&value))
                .as_deref()
                == Some(expected.as_str())),
            "PTY output did not contain workspace cwd {expected:?}: {:?}",
            events.lock().unwrap()
        );
        mgr.close(session_id).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn powershell_starts_in_existing_workspace_with_special_characters() {
        let system_root = PathBuf::from(std::env::var_os("SystemRoot").unwrap());
        let powershell = system_root.join("System32\\WindowsPowerShell\\v1.0\\powershell.exe");
        let shell_args = vec![
            "-NoLogo".to_string(),
            "-NoProfile".to_string(),
            "-Command".to_string(),
            format!(
                "Write-Output '{CWD_FRAME_BEGIN}'; (Get-Location).Path; Write-Output '{CWD_FRAME_END}'"
            ),
        ];
        assert_windows_shell_starts_in_workspace("cwd-powershell", &powershell, &shell_args);
    }

    #[cfg(windows)]
    #[test]
    fn cmd_starts_in_existing_workspace_with_special_characters() {
        let cmd = PathBuf::from(std::env::var_os("ComSpec").unwrap());
        let shell_args = vec![
            "/D".to_string(),
            "/Q".to_string(),
            "/C".to_string(),
            format!("echo {CWD_FRAME_BEGIN} & cd & echo {CWD_FRAME_END}"),
        ];
        assert_windows_shell_starts_in_workspace("cwd-cmd", &cmd, &shell_args);
    }

    #[cfg(windows)]
    fn child_conhost_pids() -> std::collections::HashSet<sysinfo::Pid> {
        use sysinfo::{get_current_pid, ProcessesToUpdate, System};

        let parent = get_current_pid().unwrap();
        let mut system = System::new();
        system.refresh_processes(ProcessesToUpdate::All, true);
        system
            .processes()
            .iter()
            .filter_map(|(pid, process)| {
                (process.parent() == Some(parent)
                    && process
                        .name()
                        .to_string_lossy()
                        .eq_ignore_ascii_case("conhost.exe"))
                .then_some(*pid)
            })
            .collect()
    }

    #[cfg(windows)]
    fn wait_for_new_conhosts(
        baseline: &std::collections::HashSet<sysinfo::Pid>,
    ) -> std::collections::HashSet<sysinfo::Pid> {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            let current = child_conhost_pids();
            let created: std::collections::HashSet<_> =
                current.difference(baseline).copied().collect();
            if !created.is_empty() {
                return created;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("ConPTY did not create a Yuzora-child conhost.exe within timeout");
    }

    #[cfg(windows)]
    fn wait_for_windows_processes_gone(pids: &[u32]) {
        use sysinfo::{Pid, ProcessesToUpdate, System};

        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            let mut system = System::new();
            system.refresh_processes(ProcessesToUpdate::All, true);
            if pids
                .iter()
                .all(|pid| system.process(Pid::from_u32(*pid)).is_none())
            {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("Windows PTY processes still exist after timeout: {pids:?}");
    }

    #[cfg(windows)]
    fn open_windows_powershell(mgr: &Arc<PtyManager>, workspace: &Path, session_id: &str) -> u32 {
        let system_root = PathBuf::from(std::env::var_os("SystemRoot").unwrap());
        let powershell = system_root.join("System32\\WindowsPowerShell\\v1.0\\powershell.exe");
        let shell_args = vec!["-NoLogo".to_string(), "-NoProfile".to_string()];
        let (on_event, _events) = capture_events();
        mgr.open(
            workspace.to_str().unwrap(),
            session_id,
            Some(powershell.to_str().unwrap()),
            Some(&shell_args),
            80,
            24,
            on_event,
        )
        .unwrap();
        mgr.debug_pid(session_id).unwrap()
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "run natively on Windows with --test-threads=1 to isolate process-tree evidence"]
    fn windows_close_reaps_shell_and_conhost_without_accumulation() {
        let workspace = tempfile::tempdir().unwrap();
        let baseline = child_conhost_pids();
        let mgr = test_manager();

        for index in 0..3 {
            let session_id = format!("conhost-close-{index}");
            let shell_pid = open_windows_powershell(&mgr, workspace.path(), &session_id);
            let conhost_pids = wait_for_new_conhosts(&baseline);

            mgr.close(&session_id).unwrap();

            let mut pids = vec![shell_pid];
            pids.extend(conhost_pids.into_iter().map(|pid| pid.as_u32()));
            wait_for_windows_processes_gone(&pids);
            assert_eq!(child_conhost_pids(), baseline);
        }
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "run natively on Windows with --test-threads=1 to isolate process-tree evidence"]
    fn windows_kill_all_reaps_shells_and_conhosts() {
        let workspace = tempfile::tempdir().unwrap();
        let baseline = child_conhost_pids();
        let mgr = test_manager();
        let first_pid = open_windows_powershell(&mgr, workspace.path(), "conhost-kill-all-1");
        let mut conhost_pids = wait_for_new_conhosts(&baseline);
        let second_baseline = baseline.union(&conhost_pids).copied().collect();
        let second_pid = open_windows_powershell(&mgr, workspace.path(), "conhost-kill-all-2");
        conhost_pids.extend(wait_for_new_conhosts(&second_baseline));

        mgr.kill_all();

        let mut pids = vec![first_pid, second_pid];
        pids.extend(conhost_pids.into_iter().map(|pid| pid.as_u32()));
        wait_for_windows_processes_gone(&pids);
        assert_eq!(child_conhost_pids(), baseline);
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

    #[cfg(unix)]
    #[test]
    fn spawned_terminal_advertises_xterm_256color() {
        const CHILD_MARKER: &str = "YUZORA_PTY_TERM_TEST_CHILD";

        if std::env::var_os(CHILD_MARKER).is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .arg("pty_service::tests::spawned_terminal_advertises_xterm_256color")
                .arg("--exact")
                .arg("--nocapture")
                .env(CHILD_MARKER, "1")
                .env("TERM", "dumb")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "TERM probe failed:\nstdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }

        let mgr = test_manager();
        let (on_event, events) = capture_events();
        let shell_args = vec![
            "-c".to_string(),
            "printf '__YUZORA_TERM__%s__END__\\n' \"$TERM\"".to_string(),
        ];
        mgr.open(
            "ws-a",
            "term-env",
            Some("/bin/sh"),
            Some(&shell_args),
            80,
            24,
            on_event,
        )
        .unwrap();

        assert!(
            poll_until(Duration::from_secs(5), || events
                .lock()
                .unwrap()
                .iter()
                .any(|event| matches!(
                    event,
                    PtyEvent::Output { data }
                        if data.contains("__YUZORA_TERM__xterm-256color__END__")
                ))),
            "spawned shell did not receive TERM=xterm-256color: {:?}",
            events.lock().unwrap()
        );
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
