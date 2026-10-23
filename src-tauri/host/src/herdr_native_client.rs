//! PTY transport for the official HERDR client, not an alternative shell runtime.
//! Only an explicitly selected, running compatible named Session can be opened.
use super::*;
use base64::Engine;
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use std::io::{Read, Write};
use std::sync::Weak;

fn terminate_native_client(child: &mut dyn portable_pty::Child) -> Result<(), String> {
    if child.try_wait().map_err(|e| e.to_string())?.is_some() {
        return Ok(());
    }
    child.kill().map_err(|e| e.to_string())?;
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        // try_wait reaps the owned process too. Avoid an unbounded wait in the
        // host stream Drop path: a stuck wait would prevent helper shutdown.
        if child.try_wait().map_err(|e| e.to_string())?.is_some() {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            return Err("native-client-exit-timeout".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

pub(super) struct NativeHerdrClient {
    master: Mutex<Box<dyn MasterPty + Send>>,
    input: Mutex<Option<Box<dyn Write + Send>>>,
    child: Mutex<Box<dyn portable_pty::Child + Send + Sync>>,
    reader: Mutex<Option<JoinHandle<()>>>,
    closed: AtomicBool,
    /// Set by a release and never cleared: the reader emits nothing afterwards,
    /// even if it outlived the release's bounded wait. `closed` is also taken
    /// by the reader itself when the client ends by itself.
    released: AtomicBool,
}

impl NativeHerdrClient {
    fn begin_release(&self) {
        self.closed.store(true, Ordering::Release);
        self.released.store(true, Ordering::Release);
    }

    /// Call once the client was terminated. Dropping the PTY writer writes a
    /// newline and EOF into the PTY, which blocks while the PTY is full, so the
    /// drop is handed to a thread of its own; a writer another thread is using
    /// right now is left to that thread.
    pub(super) fn release_input(&self) {
        let Ok(mut input) = self.input.try_lock() else {
            return;
        };
        if let Some(writer) = input.take() {
            drop(input);
            let _ = std::thread::Builder::new()
                .name("herdr-client-input-drop".into())
                .spawn(move || drop(writer));
        }
    }

    /// The release of this client as part of `release_together`.
    pub(super) fn release_target(self: &Arc<Self>) -> ReleaseTarget {
        self.begin_release();
        #[cfg(unix)]
        let child = Box::new(NativeChild {
            client: Arc::clone(self),
            reaped: false,
        }) as Box<dyn ReleaseChild>;
        #[cfg(not(unix))]
        let child = {
            let client = Arc::clone(self);
            Box::new(TerminateNow::new(move || {
                let _ = terminate_native_client(client.child.lock().unwrap().as_mut());
            })) as Box<dyn ReleaseChild>
        };
        ReleaseTarget {
            child: Some(child),
            reader: self.reader.lock().unwrap().take(),
        }
    }
}

/// The only place a native client's pid is signalled.
#[cfg(unix)]
fn signal_pid(pid: libc::pid_t, signal: libc::c_int) {
    #[cfg(test)]
    SIGNALS.with(|signals| {
        if let Some(signals) = signals.borrow_mut().as_mut() {
            signals.push((pid, signal));
        }
    });
    unsafe { libc::kill(pid, signal) };
}

// Test support: the signals this thread sends while `trace_signals` runs.
#[cfg(all(unix, test))]
thread_local! {
    static SIGNALS: std::cell::RefCell<Option<Vec<(libc::pid_t, libc::c_int)>>> =
        const { std::cell::RefCell::new(None) };
}

/// Signals the process of `child` unless the child was reaped; false once it was.
///
/// A client that ended by itself is reaped by its reader's tail before it leaves
/// the map, and portable-pty still reports the pid of a reaped child, which may
/// belong to somebody else by now. So the pid is read and signalled only here,
/// through the borrow of the locked child and right after `try_wait` found the
/// process still there: nobody can reap it in between.
#[cfg(unix)]
fn signal_unless_reaped(child: &mut dyn portable_pty::Child, kill: bool) -> bool {
    if !matches!(child.try_wait(), Ok(None)) {
        // Reaped, or an error: it can never be waited on again.
        return false;
    }
    // What portable-pty's `kill` sends (SIGHUP, then SIGKILL after a grace
    // period), without its waits.
    let signal = if kill { libc::SIGKILL } else { libc::SIGHUP };
    if let Some(pid) = child
        .process_id()
        .map(|pid| pid as libc::pid_t)
        .filter(|&pid| pid > 1)
    {
        signal_pid(pid, signal);
    }
    true
}

/// A PTY client, signalled and reaped without waiting.
#[cfg(unix)]
struct NativeChild {
    client: Arc<NativeHerdrClient>,
    reaped: bool,
}

#[cfg(unix)]
impl ReleaseChild for NativeChild {
    fn signal(&mut self, kill: bool) {
        if self.reaped {
            return;
        }
        // A busy lock means the reader's tail is terminating the child right now.
        let Ok(mut child) = self.client.child.try_lock() else {
            return;
        };
        self.reaped = !signal_unless_reaped(child.as_mut(), kill);
    }

    fn gone(&mut self) -> bool {
        if !self.reaped {
            if let Ok(mut child) = self.client.child.try_lock() {
                // try_wait reaps; an error means it can never be waited on again.
                self.reaped = !matches!(child.try_wait(), Ok(None));
            }
        }
        self.reaped
    }
}

/// What a PTY launch has created so far.
#[derive(Default)]
struct PtyLaunch {
    master: Option<Box<dyn MasterPty + Send>>,
    slave: Option<Box<dyn portable_pty::SlavePty + Send>>,
    output: Option<Box<dyn Read + Send>>,
    input: Option<Box<dyn Write + Send>>,
    child: Option<Box<dyn portable_pty::Child + Send + Sync>>,
}

impl PtyLaunch {
    /// Run under the spawn lock: create the PTY, spawn `command` on its slave
    /// and release the slave, so a failure only leaves values behind in `self`
    /// instead of dropping them (a PTY writer blocks on drop) while locked.
    fn start(&mut self, size: PtySize, command: CommandBuilder) -> Result<(), String> {
        let pair = native_pty_system()
            .openpty(size)
            .map_err(|e| e.to_string())?;
        let master = self.master.insert(pair.master);
        let slave = self.slave.insert(pair.slave);
        self.output = Some(master.try_clone_reader().map_err(|e| e.to_string())?);
        self.input = Some(master.take_writer().map_err(|e| e.to_string())?);
        self.child = Some(slave.spawn_command(command).map_err(|e| e.to_string())?);
        self.slave = None;
        Ok(())
    }
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HerdrClientSize {
    pub cols: u16,
    pub rows: u16,
    pub cell_width: u16,
    pub cell_height: u16,
}
impl HerdrClientSize {
    fn pty_size(&self) -> Result<PtySize, String> {
        if self.cols == 0
            || self.rows == 0
            || self.cols > 1000
            || self.rows > 1000
            || self.cell_width > 128
            || self.cell_height > 256
        {
            return Err("invalid-herdr-client-size".into());
        }
        Ok(PtySize {
            cols: self.cols,
            rows: self.rows,
            pixel_width: self.cols.saturating_mul(self.cell_width),
            pixel_height: self.rows.saturating_mul(self.cell_height),
        })
    }
}

const MAX_NATIVE_CLIENTS: usize = 16;

/// The reader thread of a native client: PTY output becomes frames until the
/// PTY ends or the client is closed, and a client that ended by itself is
/// terminated and reported closed.
fn native_reader_loop(
    client: Arc<NativeHerdrClient>,
    manager: Weak<HerdrManager>,
    mut output: impl Read,
    id: String,
    (cols, rows): (u16, u16),
    on_event: OnTerminalEvent,
) {
    // One guard for the frames and the closing event.
    let on_event = guard_released_events(
        {
            let client = Arc::clone(&client);
            move || client.released.load(Ordering::Acquire)
        },
        on_event,
    );
    let mut bytes = [0u8; 16 * 1024];
    let mut seq = 0u64;
    while !client.closed.load(Ordering::Acquire) {
        let len = match output.read(&mut bytes) {
            Ok(0) | Err(_) => break,
            Ok(len) => len,
        };
        seq += 1;
        if on_event(HerdrTerminalEvent::Frame {
            session_id: id.clone(),
            seq,
            full: false,
            encoding: "ansi".into(),
            width: u32::from(cols),
            height: u32::from(rows),
            bytes_base64: base64::engine::general_purpose::STANDARD.encode(&bytes[..len]),
        })
        .is_err()
        {
            break;
        }
    }
    if !client.closed.swap(true, Ordering::AcqRel) {
        client.input.lock().unwrap().take();
        let mut child = client.child.lock().unwrap();
        let _ = terminate_native_client(child.as_mut());
        drop(child);
        if let Some(manager) = manager.upgrade() {
            manager.native_clients.lock().unwrap().remove(&id);
        }
        let _ = on_event(HerdrTerminalEvent::Closed {
            session_id: id,
            reason: None,
        });
    }
}

impl HerdrManager {
    pub fn open_native_client(
        self: &Arc<Self>,
        session_name: &str,
        size: HerdrClientSize,
        on_event: OnTerminalEvent,
    ) -> Result<HerdrTerminalOpenResult, String> {
        if self.remote.is_some() {
            return Err("native-client-must-run-on-owning-host".into());
        }
        if self.native_clients.lock().unwrap().len() >= MAX_NATIVE_CLIENTS {
            return Err("too-many-native-herdr-clients".into());
        }
        if !self.session_is_compatible(session_name) {
            return Err("native-client-requires-compatible-session".into());
        }
        let (session, _) = self.require_running_session_socket(Some(session_name))?;
        let binary = self.resolve_binary().ok_or("herdr-unavailable")?;
        // The explicit client subcommand never auto-starts a server if the
        // Session stops between the compatibility check and process creation.
        let args = vec![
            "--session".to_string(),
            session.name.clone(),
            "client".to_string(),
        ];
        let id = self.spawn_native_pty(
            &binary,
            &args,
            Some(&session.name),
            &size,
            on_event,
            "too-many-native-herdr-clients",
        )?;
        let (cols, rows) = (size.cols, size.rows);
        Ok(HerdrTerminalOpenResult {
            session_id: id,
            target: session.name,
            mode: HerdrTerminalMode::Control,
            role: HerdrTerminalRole::Controller,
            cols,
            rows,
            takeover: false,
        })
    }

    /// Open a machine-management or machines-sidebar command in a PTY. Local
    /// manager only; shares the native client limit and the `herdr-client-` id.
    pub fn open_native_command(
        self: &Arc<Self>,
        binary: &Path,
        args: &[String],
        size: HerdrClientSize,
        on_event: OnTerminalEvent,
    ) -> Result<String, String> {
        if self.remote.is_some() {
            return Err("machines-local-only".into());
        }
        self.spawn_native_pty(binary, args, None, &size, on_event, "native-client-limit")
    }

    pub fn machines_is_remote(&self) -> bool {
        self.remote.is_some()
    }

    pub fn machines_active_source(&self) -> &'static str {
        match *self.active_source.lock().unwrap() {
            HerdrBinarySource::Default => "default",
            HerdrBinarySource::Global => "global",
            HerdrBinarySource::Custom => "custom",
        }
    }

    /// Shared PTY spawn for the official client and machine commands. The
    /// caller supplies the argv; `session_env` pins `HERDR_SESSION` or clears it.
    fn spawn_native_pty(
        self: &Arc<Self>,
        binary: &Path,
        args: &[String],
        session_env: Option<&str>,
        size: &HerdrClientSize,
        on_event: OnTerminalEvent,
        limit_error: &'static str,
    ) -> Result<String, String> {
        if self.native_clients.lock().unwrap().len() >= MAX_NATIVE_CLIENTS {
            return Err(limit_error.into());
        }
        let pty_size = size.pty_size()?;
        let mut command = CommandBuilder::new(binary);
        command.args(args);
        match session_env {
            Some(name) => command.env("HERDR_SESSION", name),
            // Machine management and the machines sidebar client must not
            // inherit a Session pin from a parent HERDR pane.
            None => command.env_remove("HERDR_SESSION"),
        }
        command.env_remove("HERDR_ENV");
        command.env_remove("HERDR_SOCKET_PATH");
        command.env_remove("HERDR_CLIENT_SOCKET_PATH");
        command.env_remove("HERDR_PANE_ID");
        command.env_remove("HERDR_TAB_ID");
        command.env_remove("HERDR_WORKSPACE_ID");
        // This renderer supports inline Kitty data, not local file/shared-memory
        // transports. A distinct terminal identity keeps HERDR on that path.
        // xterm-kitty itself opts into HERDR's local-file optimization even
        // with a different TERM_PROGRAM. Generic xterm uses inline blobs.
        command.env("TERM", "xterm-256color");
        command.env("TERM_PROGRAM", "yuzora");
        command.env_remove("KITTY_WINDOW_ID");
        // openpty marks the master and slave close-on-exec in a second step, so
        // the whole launch holds the process-wide spawn lock. A launch that
        // fails part-way keeps what it made in `launch`, dropped below only
        // after the lock is released.
        let mut launch = PtyLaunch::default();
        process_kill::with_spawn_lock(|| launch.start(pty_size, command))?;
        let PtyLaunch {
            master: Some(master),
            output: Some(output),
            input: Some(input),
            child: Some(child),
            ..
        } = launch
        else {
            return Err("native-client-launch-incomplete".into());
        };
        let client = Arc::new(NativeHerdrClient {
            master: Mutex::new(master),
            input: Mutex::new(Some(input)),
            child: Mutex::new(child),
            reader: Mutex::new(None),
            closed: AtomicBool::new(false),
            released: AtomicBool::new(false),
        });
        let id = format!(
            "herdr-client-{}",
            NEXT_SESSION_ID.fetch_add(1, Ordering::Relaxed)
        );
        let reader_client = client.clone();
        let manager = Arc::downgrade(self);
        let reader_id = id.clone();
        let cols = size.cols;
        let rows = size.rows;
        {
            // Re-check under the insert lock: concurrent opens pass the early
            // check before either process is registered.
            let mut clients = self.native_clients.lock().unwrap();
            if clients.len() >= MAX_NATIVE_CLIENTS {
                drop(clients);
                let _ = terminate_native_client(client.child.lock().unwrap().as_mut());
                return Err(limit_error.into());
            }
            clients.insert(id.clone(), client.clone());
        }
        let reader = std::thread::Builder::new().name(id.clone()).spawn(move || {
            native_reader_loop(
                reader_client,
                manager,
                output,
                reader_id,
                (cols, rows),
                on_event,
            );
        });
        match reader {
            Ok(reader) => *client.reader.lock().unwrap() = Some(reader),
            Err(error) => {
                self.native_clients.lock().unwrap().remove(&id);
                let _ = terminate_native_client(client.child.lock().unwrap().as_mut());
                return Err(error.to_string());
            }
        }
        Ok(id)
    }

    pub(super) fn native_client_input(
        &self,
        id: &str,
        text: Option<String>,
        bytes_base64: Option<String>,
    ) -> Result<(), String> {
        let bytes = match (text, bytes_base64) {
            (Some(text), None) => text.into_bytes(),
            (None, Some(encoded)) => base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .map_err(|_| "invalid-client-input")?,
            _ => return Err("exactly-one-input-format-required".into()),
        };
        if bytes.len() > 512 * 1024 {
            return Err("client-input-too-large".into());
        }
        let client = self
            .native_clients
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or("herdr-client-closed")?;
        let mut input = client.input.lock().unwrap();
        let input = input.as_mut().ok_or("herdr-client-closed")?;
        input
            .write_all(&bytes)
            .and_then(|()| input.flush())
            .map_err(|e| e.to_string())
    }

    pub(super) fn native_client_resize(
        &self,
        id: &str,
        cols: u16,
        rows: u16,
    ) -> Result<(), String> {
        let client = self
            .native_clients
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or("herdr-client-closed")?;
        let master = client.master.lock().unwrap();
        let previous = master.get_size().map_err(|e| e.to_string())?;
        let size = HerdrClientSize {
            cols,
            rows,
            cell_width: previous.pixel_width.checked_div(previous.cols).unwrap_or(0),
            cell_height: previous
                .pixel_height
                .checked_div(previous.rows)
                .unwrap_or(0),
        };
        master.resize(size.pty_size()?).map_err(|e| e.to_string())
    }

    pub(super) fn release_native_client(&self, id: &str) -> Result<(), String> {
        let Some(client) = self.native_clients.lock().unwrap().remove(id) else {
            return Ok(());
        };
        client.begin_release();
        // Kill only the official UI client. Never attach a process-tree guard:
        // HERDR's persistent server and its pane processes belong to the user.
        let terminated = terminate_native_client(client.child.lock().unwrap().as_mut());
        client.release_input();
        terminated?;
        // Taken out first: no lock is held while waiting.
        let reader = client.reader.lock().unwrap().take();
        if let Some(reader) = reader {
            join_within(reader, RELEASE_READER_GRACE);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::sync::mpsc;

    #[cfg(unix)]
    #[test]
    fn native_client_termination_reaps_a_pty_child_that_ignores_hangup() {
        let pair = native_pty_system().openpty(PtySize::default()).unwrap();
        let mut output = pair.master.try_clone_reader().unwrap();
        let mut command = CommandBuilder::new("/bin/sh");
        command.args(["-c", "trap '' HUP; printf R; while :; do :; done"]);
        let mut child = pair.slave.spawn_command(command).unwrap();
        drop(pair.slave);
        let mut ready = [0u8; 1];
        output.read_exact(&mut ready).unwrap();
        assert_eq!(ready, *b"R");
        let started = std::time::Instant::now();
        terminate_native_client(child.as_mut()).unwrap();
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(child.try_wait().unwrap().is_some());
        terminate_native_client(child.as_mut()).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn pty_launch_closes_its_slave_copy_once_the_child_is_spawned() {
        let mut launch = PtyLaunch::default();
        let mut command = CommandBuilder::new("/bin/sh");
        command.args(["-c", "exit 0"]);
        process_kill::with_spawn_lock(|| launch.start(PtySize::default(), command)).unwrap();
        // The reader only sees the child exit once no slave copy is left here.
        assert!(launch.slave.is_none());
        assert!(launch.master.is_some() && launch.output.is_some() && launch.input.is_some());
        assert!(launch.child.take().unwrap().wait().unwrap().success());
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_pty_launch_hands_back_what_it_created_instead_of_dropping_it() {
        let mut launch = PtyLaunch::default();
        let error = process_kill::with_spawn_lock(|| {
            launch.start(
                PtySize::default(),
                CommandBuilder::new("/nonexistent/yuzora-pty-launch-test"),
            )
        })
        .unwrap_err();
        assert!(!error.is_empty());
        // Dropping these is the caller's job, after the spawn lock is released.
        assert!(launch.master.is_some() && launch.slave.is_some());
        assert!(launch.output.is_some() && launch.input.is_some());
        assert!(launch.child.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn a_native_pty_launch_goes_through_the_spawn_lock() {
        let manager = Arc::new(HerdrManager::with_binary("/bin/sh".into()));
        let on_event: OnTerminalEvent = Arc::new(|_| Ok(()));
        let size = HerdrClientSize {
            cols: 80,
            rows: 24,
            cell_width: 8,
            cell_height: 16,
        };
        let (id, spawns) = process_kill::trace_spawns(|| {
            manager.open_native_command(
                Path::new("/bin/sh"),
                &["-c".to_string(), "exit 0".to_string()],
                size,
                on_event,
            )
        });
        manager.release_native_client(&id.unwrap()).unwrap();
        assert_eq!(spawns, ["with_spawn_lock"]);
    }

    #[cfg(unix)]
    fn size() -> HerdrClientSize {
        HerdrClientSize {
            cols: 80,
            rows: 24,
            cell_width: 8,
            cell_height: 16,
        }
    }

    #[cfg(unix)]
    fn channel_events() -> (OnTerminalEvent, mpsc::Receiver<HerdrTerminalEvent>) {
        let (tx, rx) = mpsc::channel();
        let on_event: OnTerminalEvent = Arc::new(move |event| {
            let _ = tx.send(event);
            Ok(())
        });
        (on_event, rx)
    }

    /// Runs `f` on a thread of its own and gives up after `limit`, so a hang
    /// fails the test instead of freezing it.
    #[cfg(unix)]
    fn within<T: Send + 'static>(
        limit: Duration,
        f: impl FnOnce() -> T + Send + 'static,
    ) -> Option<T> {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(f());
        });
        rx.recv_timeout(limit).ok()
    }

    #[cfg(unix)]
    fn open_sh(manager: &Arc<HerdrManager>, script: &str, on_event: OnTerminalEvent) -> String {
        manager
            .open_native_command(
                Path::new("/bin/sh"),
                &["-c".to_string(), script.to_string()],
                size(),
                on_event,
            )
            .unwrap()
    }

    /// Reads events until every id in `ids` has produced an `R`.
    #[cfg(unix)]
    fn wait_for_r(events: &mpsc::Receiver<HerdrTerminalEvent>, ids: &[String]) {
        let mut seen = std::collections::HashSet::new();
        while seen.len() < ids.len() {
            let event = events.recv_timeout(Duration::from_secs(5)).expect("output");
            if let HerdrTerminalEvent::Frame {
                session_id,
                bytes_base64,
                ..
            } = event
            {
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(bytes_base64)
                    .unwrap();
                if bytes.contains(&b'R') {
                    seen.insert(session_id);
                }
            }
        }
    }

    #[cfg(unix)]
    fn client_of(manager: &HerdrManager, id: &str) -> Arc<NativeHerdrClient> {
        manager
            .native_clients
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .unwrap()
    }

    #[cfg(unix)]
    fn pid_of(client: &NativeHerdrClient) -> libc::pid_t {
        client.child.lock().unwrap().process_id().unwrap() as libc::pid_t
    }

    #[cfg(unix)]
    fn process_exists(pid: libc::pid_t) -> bool {
        if unsafe { libc::kill(pid, 0) } == 0 {
            return true;
        }
        std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
    }

    /// SIGKILLs the process group of every client it holds when dropped, panic
    /// or not: some fixtures ignore hangup or run on, so a test that fails
    /// before its release ends them would leave them behind for good. A client
    /// that was reaped is skipped: its pid may be somebody else's by now.
    #[cfg(unix)]
    #[derive(Default)]
    struct KillClients(Vec<Arc<NativeHerdrClient>>);

    #[cfg(unix)]
    impl KillClients {
        fn track(&mut self, client: Arc<NativeHerdrClient>) -> Arc<NativeHerdrClient> {
            self.0.push(client.clone());
            client
        }
    }

    #[cfg(unix)]
    impl Drop for KillClients {
        fn drop(&mut self) {
            for client in &self.0 {
                let mut child = client
                    .child
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if !matches!(child.try_wait(), Ok(None)) {
                    continue;
                }
                let pid = child.process_id().map(|pid| pid as libc::pid_t);
                if let Some(pid) = pid.filter(|&pid| pid > 1) {
                    // A PTY client leads its own session and group.
                    unsafe {
                        libc::killpg(pid, libc::SIGKILL);
                        libc::kill(pid, libc::SIGKILL);
                    }
                }
            }
        }
    }

    /// `open_sh`, with the new client handed to `cleanup`.
    #[cfg(unix)]
    fn open_tracked(
        cleanup: &mut KillClients,
        manager: &Arc<HerdrManager>,
        script: &str,
        on_event: OnTerminalEvent,
    ) -> String {
        let id = open_sh(manager, script, on_event);
        cleanup.track(client_of(manager, &id));
        id
    }

    /// A client around a PTY child that only sleeps; the tests feed its reader.
    #[cfg(unix)]
    fn idle_client() -> Arc<NativeHerdrClient> {
        let mut command = CommandBuilder::new("/bin/sh");
        command.args(["-c", "sleep 60"]);
        let (pair, child) = process_kill::with_spawn_lock(|| {
            let pair = native_pty_system().openpty(PtySize::default()).unwrap();
            let child = pair.slave.spawn_command(command).unwrap();
            (pair, child)
        });
        drop(pair.slave);
        Arc::new(NativeHerdrClient {
            master: Mutex::new(pair.master),
            input: Mutex::new(None),
            child: Mutex::new(child),
            reader: Mutex::new(None),
            closed: AtomicBool::new(false),
            released: AtomicBool::new(false),
        })
    }

    #[cfg(unix)]
    fn start_reader(
        client: &Arc<NativeHerdrClient>,
        output: std::io::PipeReader,
        on_event: OnTerminalEvent,
    ) -> JoinHandle<()> {
        let client = client.clone();
        std::thread::spawn(move || {
            native_reader_loop(
                client,
                Weak::new(),
                output,
                "herdr-client-test".into(),
                (80, 24),
                on_event,
            );
        })
    }

    #[cfg(unix)]
    #[test]
    fn a_native_reader_reports_its_frames_and_its_own_close() {
        let mut cleanup = KillClients::default();
        let client = cleanup.track(idle_client());
        let (output, mut writer) = std::io::pipe().unwrap();
        let (on_event, events) = channel_events();
        let reader = start_reader(&client, output, on_event);
        writer.write_all(b"one").unwrap();
        drop(writer);
        reader.join().unwrap();
        assert!(matches!(
            events.try_recv(),
            Ok(HerdrTerminalEvent::Frame { seq: 1, .. })
        ));
        assert!(matches!(
            events.try_recv(),
            Ok(HerdrTerminalEvent::Closed { reason: None, .. })
        ));
        // The client ended by itself, so its reader terminated the child.
        assert!(client.child.lock().unwrap().try_wait().unwrap().is_some());
    }

    #[cfg(unix)]
    #[test]
    fn a_native_reader_let_go_by_its_release_reports_nothing_more() {
        let mut cleanup = KillClients::default();
        let client = cleanup.track(idle_client());
        let (output, mut writer) = std::io::pipe().unwrap();
        let (on_event, events) = channel_events();
        let reader = start_reader(&client, output, on_event);
        writer.write_all(b"one").unwrap();
        let first = events.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(first, HerdrTerminalEvent::Frame { seq: 1, .. }));
        // Only the release flag is raised, `closed` stays as it is: the reader
        // sits in its read, wakes up with output later and must keep it to itself.
        client.released.store(true, Ordering::Release);
        writer.write_all(b"two").unwrap();
        drop(writer);
        reader.join().unwrap();
        assert!(
            events.try_recv().is_err(),
            "an event was reported after the release"
        );
        assert!(client.child.lock().unwrap().try_wait().unwrap().is_some());
    }

    #[cfg(unix)]
    #[test]
    fn release_returns_after_its_grace_when_the_reader_is_stuck() {
        let manager = Arc::new(HerdrManager::with_binary("/bin/sh".into()));
        let mut cleanup = KillClients::default();
        // The reader sits inside this callback until the test lets it go.
        let (gate, gate_rx) = mpsc::channel::<()>();
        let gate_rx = Mutex::new(gate_rx);
        let (entered_tx, entered) = mpsc::channel();
        let on_event: OnTerminalEvent = Arc::new(move |_| {
            let _ = entered_tx.send(());
            let _ = gate_rx.lock().unwrap().recv();
            Ok(())
        });
        let id = open_tracked(&mut cleanup, &manager, "printf R; sleep 60", on_event);
        entered.recv_timeout(Duration::from_secs(5)).unwrap();
        let pid = pid_of(&client_of(&manager, &id));

        let started = Instant::now();
        within(Duration::from_secs(10), {
            let manager = manager.clone();
            move || manager.release_native_client(&id)
        })
        .expect("the release waited for its stuck reader")
        .unwrap();
        let elapsed = started.elapsed();
        drop(gate);
        assert!(elapsed >= RELEASE_READER_GRACE, "{elapsed:?}");
        assert!(
            elapsed < RELEASE_READER_GRACE + Duration::from_secs(2),
            "{elapsed:?}"
        );
        assert!(!process_exists(pid));
    }

    #[cfg(unix)]
    #[test]
    fn release_all_gives_every_client_one_shared_wait_before_the_kill() {
        let manager = Arc::new(HerdrManager::with_binary("/bin/sh".into()));
        let mut cleanup = KillClients::default();
        let (on_event, events) = channel_events();
        // Hangup is ignored, so each client only dies to the kill. The loop ends
        // by itself after about 30 s, for a test process that is killed.
        let ids: Vec<String> = (0..3)
            .map(|_| {
                open_tracked(
                    &mut cleanup,
                    &manager,
                    "trap '' HUP; printf R; n=0; while [ $n -lt 600 ]; do sleep 0.05; n=$((n+1)); done",
                    on_event.clone(),
                )
            })
            .collect();
        wait_for_r(&events, &ids);
        let pids: Vec<_> = ids
            .iter()
            .map(|id| pid_of(&client_of(&manager, id)))
            .collect();

        let started = Instant::now();
        within(Duration::from_secs(15), {
            let manager = manager.clone();
            move || manager.release_all_connectors()
        })
        .expect("release_all_connectors hung");
        let elapsed = started.elapsed();
        assert!(elapsed >= RELEASE_ALL_TERM_WAIT, "{elapsed:?}");
        assert!(elapsed < Duration::from_secs(3), "{elapsed:?}");
        assert!(manager.native_clients.lock().unwrap().is_empty());
        for pid in pids {
            assert!(!process_exists(pid), "client {pid} survived");
        }
    }

    #[cfg(unix)]
    #[test]
    fn release_all_ends_cooperative_clients_without_waiting_out_the_deadline() {
        let manager = Arc::new(HerdrManager::with_binary("/bin/sh".into()));
        let mut cleanup = KillClients::default();
        let (on_event, events) = channel_events();
        let ids: Vec<String> = (0..3)
            .map(|_| {
                open_tracked(
                    &mut cleanup,
                    &manager,
                    "printf R; sleep 60",
                    on_event.clone(),
                )
            })
            .collect();
        wait_for_r(&events, &ids);
        let pids: Vec<_> = ids
            .iter()
            .map(|id| pid_of(&client_of(&manager, id)))
            .collect();

        // A deadline far beyond what the test tolerates: only a client that is
        // never asked politely would wait for it.
        let deadline = Duration::from_secs(8);
        let started = Instant::now();
        within(Duration::from_secs(15), {
            let manager = manager.clone();
            move || manager.release_all_connectors_within(deadline, RELEASE_ALL_KILL_WAIT)
        })
        .expect("release_all_connectors hung");
        // Hangup ends them at once, so nothing waits for the deadline.
        assert!(started.elapsed() < Duration::from_secs(4));
        for pid in pids {
            assert!(!process_exists(pid), "client {pid} survived");
        }
    }

    /// What this thread signals while `f` runs, as `(pid, signal)`.
    #[cfg(unix)]
    fn trace_signals<R>(f: impl FnOnce() -> R) -> (R, Vec<(libc::pid_t, libc::c_int)>) {
        SIGNALS.with(|signals| *signals.borrow_mut() = Some(Vec::new()));
        let result = f();
        let signals = SIGNALS.with(|signals| signals.borrow_mut().take());
        (result, signals.unwrap_or_default())
    }

    /// A client that ended by itself is reaped by its reader's tail, which only
    /// then removes it from the map: a release in between must not signal the
    /// pid portable-pty keeps reporting, it may be somebody else's by now.
    #[cfg(unix)]
    #[test]
    fn release_all_never_signals_the_pid_of_a_client_that_was_already_reaped() {
        let manager = Arc::new(HerdrManager::with_binary("/bin/sh".into()));
        let mut cleanup = KillClients::default();
        let reaped = cleanup.track(idle_client());
        let reaped_pid = pid_of(&reaped);
        {
            let mut child = reaped.child.lock().unwrap();
            terminate_native_client(child.as_mut()).unwrap();
            assert!(child.try_wait().unwrap().is_some());
            // The premise: the pid is still reported once the child is reaped.
            assert_eq!(child.process_id(), Some(reaped_pid as u32));
        }
        let live = cleanup.track(idle_client());
        let live_pid = pid_of(&live);
        {
            let mut clients = manager.native_clients.lock().unwrap();
            clients.insert("herdr-client-reaped".into(), reaped);
            clients.insert("herdr-client-live".into(), live);
        }
        let (_, signals) = within(Duration::from_secs(10), {
            let manager = manager.clone();
            move || {
                trace_signals(|| {
                    manager.release_all_connectors_within(
                        Duration::from_secs(2),
                        Duration::from_millis(500),
                    );
                })
            }
        })
        .expect("release_all_connectors hung");
        // Only the live client is asked to stop.
        assert_eq!(signals, [(live_pid, libc::SIGHUP)]);
        assert!(!process_exists(live_pid));
    }

    /// The kill at the deadline goes through the same check as the polite signal.
    #[cfg(unix)]
    #[test]
    fn the_deadline_kill_never_signals_a_reaped_client() {
        let mut cleanup = KillClients::default();
        let client = cleanup.track(idle_client());
        terminate_native_client(client.child.lock().unwrap().as_mut()).unwrap();
        let mut child = NativeChild {
            client,
            reaped: false,
        };
        let (_, signals) = trace_signals(|| child.signal(true));
        assert!(signals.is_empty(), "signalled a reaped pid: {signals:?}");
        assert!(child.gone());
    }

    /// The same window with the real reader: its tail has reaped the child and
    /// waits for the map lock the test holds, so the client is still listed.
    #[cfg(unix)]
    #[test]
    fn release_never_signals_a_client_whose_own_reader_reaped_it() {
        let manager = Arc::new(HerdrManager::with_binary("/bin/sh".into()));
        let mut cleanup = KillClients::default();
        let dir = tempfile::tempdir().unwrap();
        let gate = dir.path().join("gate");
        let (on_event, events) = channel_events();
        // The loop also ends by itself after about 30 s, for a test that fails
        // before it opens the gate and is not around to clean up.
        let script = format!(
            "printf R; n=0; while [ $n -lt 600 ] && [ ! -e '{}' ]; do sleep 0.05; n=$((n+1)); done",
            gate.display()
        );
        let id = open_tracked(&mut cleanup, &manager, &script, on_event);
        wait_for_r(&events, std::slice::from_ref(&id));
        let client = client_of(&manager, &id);
        let clients = manager.native_clients.lock().unwrap();
        std::fs::write(&gate, b"").unwrap();
        // Wait for the reader's own close too: a release that set `closed` first
        // would skip the tail that removes the client from the map.
        let reaped = wait_until(Instant::now() + Duration::from_secs(5), || {
            client.closed.load(Ordering::Acquire)
                && client
                    .child
                    .try_lock()
                    .is_ok_and(|mut child| child.try_wait().is_ok_and(|status| status.is_some()))
        });
        assert!(reaped, "the client never ended");

        let (_, signals) = trace_signals(|| {
            release_together(
                vec![client.release_target()],
                Duration::from_millis(200),
                Duration::from_millis(100),
            );
        });
        assert!(signals.is_empty(), "signalled a reaped pid: {signals:?}");
        // Let the reader's tail finish.
        drop(clients);
        assert!(wait_until(Instant::now() + Duration::from_secs(5), || {
            manager.native_clients.lock().unwrap().is_empty()
        }));
    }

    /// A busy child lock means the reader's tail is terminating the child right
    /// now: the release neither waits for the lock nor signals behind its back.
    #[cfg(unix)]
    #[test]
    fn release_all_leaves_a_client_whose_child_lock_is_busy_to_its_holder() {
        let manager = Arc::new(HerdrManager::with_binary("/bin/sh".into()));
        let mut cleanup = KillClients::default();
        let client = cleanup.track(idle_client());
        manager
            .native_clients
            .lock()
            .unwrap()
            .insert("herdr-client-busy".into(), client.clone());
        let busy = client.child.lock().unwrap();
        let (_, signals) = within(Duration::from_secs(5), {
            let manager = manager.clone();
            move || {
                trace_signals(|| {
                    manager.release_all_connectors_within(
                        Duration::from_millis(200),
                        Duration::from_millis(100),
                    );
                })
            }
        })
        .expect("release_all_connectors waited for the busy child lock");
        assert!(signals.is_empty(), "{signals:?}");
        drop(busy);
        terminate_native_client(client.child.lock().unwrap().as_mut()).unwrap();
    }

    /// A writer another thread holds is in use, say inside a write that is stuck
    /// in a full PTY: a release never waits for it and never takes it, and it
    /// reaches `release_input` also for a client that is still alive. The test
    /// holds the lock itself, which is exact where a real blocked write is ended
    /// by the kill of the client (the macOS tests below).
    #[cfg(unix)]
    #[test]
    fn release_all_leaves_a_writer_in_use_to_its_holder() {
        let manager = Arc::new(HerdrManager::with_binary("/bin/sh".into()));
        let mut cleanup = KillClients::default();
        let (on_event, events) = channel_events();
        let id = open_tracked(&mut cleanup, &manager, "printf R; sleep 60", on_event);
        wait_for_r(&events, std::slice::from_ref(&id));
        let client = client_of(&manager, &id);
        let pid = pid_of(&client);
        let busy = client.input.lock().unwrap();
        let released = within(Duration::from_secs(5), {
            let manager = manager.clone();
            move || {
                manager.release_all_connectors_within(
                    Duration::from_millis(200),
                    Duration::from_millis(100),
                )
            }
        });
        // A release that waits for the lock gets it here and ends.
        drop(busy);
        assert!(
            released.is_some(),
            "release_all_connectors waited for the writer in use"
        );
        assert!(
            client.input.lock().unwrap().is_some(),
            "the writer in use was taken"
        );
        assert!(!process_exists(pid));
    }

    /// The same for a single release.
    #[cfg(unix)]
    #[test]
    fn release_leaves_a_writer_in_use_to_its_holder() {
        let manager = Arc::new(HerdrManager::with_binary("/bin/sh".into()));
        let mut cleanup = KillClients::default();
        let (on_event, events) = channel_events();
        let id = open_tracked(&mut cleanup, &manager, "printf R; sleep 60", on_event);
        wait_for_r(&events, std::slice::from_ref(&id));
        let client = client_of(&manager, &id);
        let busy = client.input.lock().unwrap();
        let released = within(Duration::from_secs(5), {
            let manager = manager.clone();
            move || manager.release_native_client(&id)
        });
        drop(busy);
        released
            .expect("release_native_client waited for the writer in use")
            .unwrap();
        assert!(
            client.input.lock().unwrap().is_some(),
            "the writer in use was taken"
        );
    }

    /// A release terminates the client first and only then waits for its reader.
    /// The child is cooperative and nobody else holds the PTY, so the reader ends
    /// right after the kill; waiting for it before the kill would take the whole
    /// grace, as the client is alive and keeps the PTY open.
    #[cfg(unix)]
    #[test]
    fn release_terminates_the_client_before_it_waits_for_the_reader() {
        let manager = Arc::new(HerdrManager::with_binary("/bin/sh".into()));
        let mut cleanup = KillClients::default();
        let (on_event, events) = channel_events();
        let id = open_tracked(&mut cleanup, &manager, "printf R; sleep 60", on_event);
        wait_for_r(&events, std::slice::from_ref(&id));
        let pid = pid_of(&client_of(&manager, &id));

        let started = Instant::now();
        within(Duration::from_secs(10), {
            let manager = manager.clone();
            move || manager.release_native_client(&id)
        })
        .expect("the release hung")
        .unwrap();
        let elapsed = started.elapsed();
        assert!(elapsed < RELEASE_READER_GRACE / 2, "{elapsed:?}");
        assert!(!process_exists(pid));
        assert!(manager.native_clients.lock().unwrap().is_empty());
    }

    /// A macOS PTY takes about a kilobyte of input from a child that never reads
    /// it; then a writer sits inside `write`, and a droppable writer would too.
    #[cfg(target_os = "macos")]
    #[test]
    fn release_does_not_wait_behind_a_blocked_input_write() {
        let manager = Arc::new(HerdrManager::with_binary("/bin/sh".into()));
        let mut cleanup = KillClients::default();
        let (on_event, events) = channel_events();
        let id = open_tracked(&mut cleanup, &manager, "printf R; sleep 60", on_event);
        wait_for_r(&events, std::slice::from_ref(&id));
        let client = client_of(&manager, &id);
        let writer = {
            let manager = manager.clone();
            let id = id.clone();
            std::thread::spawn(move || {
                let line = format!("{}\n", "x".repeat(100));
                while manager
                    .native_client_input(&id, Some(line.clone()), None)
                    .is_ok()
                {}
            })
        };
        let blocked = wait_until(Instant::now() + Duration::from_secs(10), || {
            client.input.try_lock().is_err() && {
                std::thread::sleep(Duration::from_millis(200));
                client.input.try_lock().is_err()
            }
        });
        assert!(blocked, "the writer never blocked");

        let started = Instant::now();
        within(Duration::from_secs(10), {
            let manager = manager.clone();
            let id = id.clone();
            move || manager.release_native_client(&id)
        })
        .expect("the release waited behind the blocked write")
        .unwrap();
        assert!(started.elapsed() < Duration::from_secs(3));
        // The kill ended the blocked write.
        assert!(wait_until(Instant::now() + Duration::from_secs(5), || {
            writer.is_finished()
        }));
        writer.join().unwrap();
    }

    /// Dropping the PTY writer writes a newline and EOF into the PTY, which
    /// blocks while a child that never reads has the PTY full.
    #[cfg(target_os = "macos")]
    #[test]
    fn release_does_not_drop_the_pty_writer_into_a_full_pty_while_the_client_lives() {
        let manager = Arc::new(HerdrManager::with_binary("/bin/sh".into()));
        let mut cleanup = KillClients::default();
        let (on_event, events) = channel_events();
        let id = open_tracked(&mut cleanup, &manager, "printf R; sleep 60", on_event);
        wait_for_r(&events, std::slice::from_ref(&id));
        let client = client_of(&manager, &id);
        // Fill the PTY through another descriptor on the same master: the
        // client's own writer stays idle, and so droppable.
        let master = client.master.lock().unwrap().as_raw_fd().unwrap();
        let filler_fd = unsafe { libc::dup(master) };
        assert!(filler_fd >= 0);
        let writes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let filler = {
            let writes = writes.clone();
            std::thread::spawn(move || {
                // Whole lines: a canonical-mode PTY only blocks once a complete
                // line is waiting, and drops the characters of an unfinished one.
                let mut line = [b'x'; 101];
                line[100] = b'\n';
                while unsafe { libc::write(filler_fd, line.as_ptr().cast(), line.len()) } > 0 {
                    writes.fetch_add(1, Ordering::Relaxed);
                }
                unsafe { libc::close(filler_fd) };
            })
        };
        // Full once a write has been stuck for a while: they take microseconds.
        let full = wait_until(Instant::now() + Duration::from_secs(10), || {
            let before = writes.load(Ordering::Relaxed);
            std::thread::sleep(Duration::from_millis(300));
            before > 0 && writes.load(Ordering::Relaxed) == before
        });
        assert!(full, "the PTY never filled up");

        let started = Instant::now();
        within(Duration::from_secs(10), {
            let manager = manager.clone();
            let id = id.clone();
            move || manager.release_native_client(&id)
        })
        .expect("the release dropped the writer into a full PTY")
        .unwrap();
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(wait_until(Instant::now() + Duration::from_secs(5), || {
            filler.is_finished()
        }));
        filler.join().unwrap();
    }

    #[test]
    fn native_client_geometry_is_bounded_and_carries_pixel_cells() {
        let value = HerdrClientSize {
            cols: 120,
            rows: 40,
            cell_width: 8,
            cell_height: 16,
        }
        .pty_size()
        .unwrap();
        assert_eq!((value.pixel_width, value.pixel_height), (960, 640));
        assert!(HerdrClientSize {
            cols: 0,
            rows: 10,
            cell_width: 8,
            cell_height: 16
        }
        .pty_size()
        .is_err());
        assert!(HerdrClientSize {
            cols: 1001,
            rows: 10,
            cell_width: 8,
            cell_height: 16
        }
        .pty_size()
        .is_err());
    }
}
