//! HERDR waits and releases never depend on an EOF a stranger can withhold.
//! The strangers are real processes: each starts in a session of its own (so a
//! group kill cannot reach it), inherits the pipe and keeps it open, like a
//! descriptor captured by a racing spawn would.
use super::*;

const FRAME_1: &str = r#"{"type":"terminal.frame","seq":1,"full":true,"encoding":"ansi","width":40,"height":10,"bytes":"AAA="}"#;

/// Starts a stranger: it records its pid in `@DIR@/holders`, then keeps the
/// inherited stdout open, prints one more frame once `@DIR@/go` exists and
/// sleeps on. It sits in a session of its own, out of reach of a group kill,
/// so it also ends by itself (it stops waiting for `go` after 15 s and sleeps
/// 20 s): a test that fails or is killed before it ends it cannot leave it
/// behind for good.
const START_STRANGER: &str = r#"perl -MPOSIX -e '$| = 1; POSIX::setsid(); open(my $f, ">>", $ARGV[0]) or die; print $f "$$\n"; close $f; select(undef, undef, undef, 0.05) until -e $ARGV[1] or time - $^T > 15; print qq({"type":"terminal.frame","seq":2,"full":false,"encoding":"ansi","width":40,"height":10,"bytes":"AQE="}\n); sleep 20' "@DIR@/holders" "@DIR@/go" &"#;

/// Like `START_STRANGER`, but it holds the inherited stderr: it writes "early
/// stderr line" once `@DIR@/go` exists and "late stderr line" once
/// `@DIR@/go-late` exists (and ends by itself the same way).
const START_STDERR_STRANGER: &str = r#"perl -MPOSIX -e 'POSIX::setsid(); open(my $f, ">>", $ARGV[0]) or die; print $f "$$\n"; close $f; select(undef, undef, undef, 0.05) until -e $ARGV[1] or time - $^T > 15; print STDERR qq(early stderr line\n); select(undef, undef, undef, 0.05) until -e $ARGV[2] or time - $^T > 15; print STDERR qq(late stderr line\n); sleep 20' "@DIR@/holders" "@DIR@/go" "@DIR@/go-late" &"#;

/// A `herdr` that lists one running Session and runs `terminal_branch` for
/// `terminal session <mode> <target> ...` (`$3` is the mode, `$4` the target).
/// Branches that must die to one polite signal end in `exec sleep`: a shell
/// that forks `sleep` can be killed in the very moment it forks, which leaves
/// the new `sleep` alive in the group.
fn write_fake_connector(dir: &Path, terminal_branch: &str) -> PathBuf {
    let script = r#"#!/bin/sh
if [ "$1" = "session" ] && [ "$2" = "list" ] && [ "$3" = "--json" ]; then
  cat <<'JSON'
{"sessions":[{"name":"default","default":true,"running":true,"session_dir":"/tmp/herdr-default","socket_path":"/tmp/herdr.sock"}]}
JSON
  exit 0
fi
if [ "$1" = "terminal" ] && [ "$2" = "session" ]; then
@BRANCH@
fi
exit 2
"#
    .replace("@BRANCH@", terminal_branch)
    .replace("@FRAME1@", FRAME_1)
    .replace("@DIR@", dir.to_str().unwrap());
    let path = dir.join("herdr");
    write_executable_fixture(&path, &script);
    path
}

fn stranger(dir: &Path) -> String {
    START_STRANGER.replace("@DIR@", dir.to_str().unwrap())
}

/// Runs `f` on a thread of its own and gives up after `limit`, so a hang fails
/// the test instead of freezing it.
fn within<T: Send + 'static>(limit: Duration, f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(limit).ok()
}

/// The pids in `path`, once there are `count` of them.
fn wait_for_pids(path: &Path, count: usize) -> Vec<libc::pid_t> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let pids: Vec<libc::pid_t> = fs::read_to_string(path)
            .unwrap_or_default()
            .split_whitespace()
            .filter_map(|pid| pid.parse().ok())
            .collect();
        if pids.len() >= count {
            return pids;
        }
        assert!(
            Instant::now() < deadline,
            "{} never listed {count} pids",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// SIGKILLs the pids listed in a file, and their process groups (a connector
/// and a stranger lead their own), when dropped, panic or not. A test disarms
/// it once the pids were reaped: a dead pid may be someone else's.
struct KillListed(Option<PathBuf>);

impl KillListed {
    fn new(path: PathBuf) -> Self {
        Self(Some(path))
    }

    fn disarm(&mut self) {
        self.0 = None;
    }
}

impl Drop for KillListed {
    fn drop(&mut self) {
        let Some(path) = &self.0 else {
            return;
        };
        for pid in fs::read_to_string(path)
            .unwrap_or_default()
            .split_whitespace()
            .filter_map(|pid| pid.parse::<libc::pid_t>().ok())
        {
            unsafe {
                libc::killpg(pid, libc::SIGKILL);
                libc::kill(pid, libc::SIGKILL);
            }
        }
    }
}

/// SIGKILLs the process group of every connector `manager` still holds when
/// dropped, panic or not. It needs no pid written by the fixture (a test can
/// fail before the connector got to write it), and a connector that was reaped
/// is skipped: its pid may be somebody else's by now. One that a release took
/// over is the release's to end.
struct KillSessions(Arc<HerdrManager>);

impl KillSessions {
    fn new(manager: &Arc<HerdrManager>) -> Self {
        Self(manager.clone())
    }
}

impl Drop for KillSessions {
    fn drop(&mut self) {
        let sessions: Vec<_> = self
            .0
            .sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .values()
            .cloned()
            .collect();
        for session in sessions {
            let mut child = session
                .child
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(child) = child.as_mut() else {
                continue;
            };
            if !matches!(child.try_wait(), Ok(None)) {
                continue;
            }
            // A connector leads its own group.
            let pid = child.id() as libc::pid_t;
            unsafe {
                libc::killpg(pid, libc::SIGKILL);
                libc::kill(pid, libc::SIGKILL);
            }
        }
    }
}

/// The (single) pid in the guard's file must be gone; then the guard is disarmed.
fn assert_gone_and_disarm(guard: &mut KillListed) {
    let pids: Vec<libc::pid_t> = fs::read_to_string(guard.0.as_ref().unwrap())
        .unwrap()
        .split_whitespace()
        .filter_map(|pid| pid.parse().ok())
        .collect();
    assert_eq!(pids.len(), 1);
    assert!(
        !unix_pid_exists(pids[0] as u32),
        "connector {} survived",
        pids[0]
    );
    guard.disarm();
}

fn channel_events() -> (OnTerminalEvent, mpsc::Receiver<HerdrTerminalEvent>) {
    let (tx, rx) = mpsc::channel();
    let on_event: OnTerminalEvent = Arc::new(move |event| {
        let _ = tx.send(event);
        Ok(())
    });
    (on_event, rx)
}

fn open(
    manager: &Arc<HerdrManager>,
    target: &str,
    mode: HerdrTerminalMode,
    on_event: OnTerminalEvent,
) -> String {
    manager
        .open_terminal(target.into(), mode, false, 40, 10, None, on_event)
        .unwrap()
        .session_id
}

fn first_frame(rx: &mpsc::Receiver<HerdrTerminalEvent>) {
    let event = rx.recv_timeout(Duration::from_secs(5)).expect("a frame");
    assert!(
        matches!(event, HerdrTerminalEvent::Frame { seq: 1, .. }),
        "{event:?}"
    );
}

// ── wait_bounded_child ──────────────────────────────────────────────────────

fn spawn_piped_sh(script: &str) -> (Child, process_kill::ProcessTreeGuard) {
    let mut command = Command::new("/bin/sh");
    command
        .args(["-c", script])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    process_kill::configure_background_process(&mut command);
    let mut child = process_kill::spawn_std(&mut command).unwrap();
    let tree = process_kill::attach_process_tree(&mut child).unwrap();
    (child, tree)
}

#[test]
fn a_child_whose_background_job_holds_the_pipes_returns_what_it_wrote() {
    let dir = tempfile::tempdir().unwrap();
    let holder = dir.path().join("holder.pid");
    let _cleanup = KillListed::new(holder.clone());
    let (mut child, mut tree) = spawn_piped_sh(&format!(
        "sleep 30 & echo $! > '{}'; printf ok; printf err >&2; exit 3",
        holder.display()
    ));
    let timeout = Duration::from_secs(5);
    let started = Instant::now();
    let result = within(Duration::from_secs(10), move || {
        wait_bounded_child(&mut child, &mut tree, timeout, MAX_NDJSON_LINE_BYTES)
    })
    .expect("the wait hung on the background job's pipe");
    let (stdout, stderr, status) = result.unwrap();
    assert!(started.elapsed() < timeout + Duration::from_secs(1));
    assert_eq!((stdout, stderr), (b"ok".to_vec(), b"err".to_vec()));
    assert_eq!(status.code(), Some(3));
    let background: libc::pid_t = fs::read_to_string(&holder).unwrap().trim().parse().unwrap();
    assert!(
        unix_pid_exists(background as u32),
        "the background job must still hold the pipes, or this proves nothing"
    );
}

#[test]
fn output_larger_than_the_pipe_buffer_survives_a_held_pipe() {
    let dir = tempfile::tempdir().unwrap();
    let holder = dir.path().join("holder.pid");
    let _cleanup = KillListed::new(holder.clone());
    let (mut child, mut tree) = spawn_piped_sh(&format!(
        "sleep 30 & echo $! > '{}'; head -c 300000 /dev/zero | tr '\\0' x",
        holder.display()
    ));
    let (stdout, _, status) = within(Duration::from_secs(10), move || {
        wait_bounded_child(
            &mut child,
            &mut tree,
            Duration::from_secs(5),
            MAX_NDJSON_LINE_BYTES,
        )
    })
    .expect("the wait hung on the background job's pipe")
    .unwrap();
    assert!(status.success());
    assert_eq!(stdout.len(), 300_000);
    assert!(stdout.iter().all(|&byte| byte == b'x'));
}

#[test]
fn a_failed_reap_still_returns_while_a_stranger_holds_the_pipes() {
    let dir = tempfile::tempdir().unwrap();
    let holders = dir.path().join("holders");
    let _cleanup = KillListed::new(holders.clone());
    // The stranger leaves the child's group and session; the child waits for
    // it to be up before it exits.
    let (mut child, mut tree) = spawn_piped_sh(&format!(
        "{}\nn=0; while [ ! -s '{}' ] && [ $n -lt 1000 ]; do sleep 0.02; n=$((n+1)); done; printf ok",
        stranger(dir.path()),
        holders.display()
    ));
    let stranger_pid = wait_for_pids(&holders, 1)[0];
    // Reap the child behind std's back: its own try_wait then fails, so the
    // wait loop, the cleanup and the reap all report errors.
    let mut raw_status = 0;
    assert_eq!(
        unsafe { libc::waitpid(child.id() as libc::pid_t, &mut raw_status, 0) },
        child.id() as libc::pid_t
    );
    let started = Instant::now();
    let result = within(Duration::from_secs(10), move || {
        wait_bounded_child(
            &mut child,
            &mut tree,
            Duration::from_secs(5),
            MAX_NDJSON_LINE_BYTES,
        )
    })
    .expect("the wait hung on the stranger's pipe");
    let error = result.unwrap_err();
    assert!(error.contains("cleanup failed"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(3));
    assert!(
        unix_pid_exists(stranger_pid as u32),
        "the stranger must still hold the pipes, or this proves nothing"
    );
}

// ── connector readers ───────────────────────────────────────────────────────

/// A reader that blocks until the test sends it bytes and reports EOF once the
/// sender is gone: the stdout of a connector that a stranger holds open.
struct ChannelReader {
    incoming: mpsc::Receiver<Vec<u8>>,
    pending: std::io::Cursor<Vec<u8>>,
}

impl ChannelReader {
    fn new() -> (mpsc::Sender<Vec<u8>>, Self) {
        let (tx, incoming) = mpsc::channel();
        let reader = Self {
            incoming,
            pending: std::io::Cursor::new(Vec::new()),
        };
        (tx, reader)
    }
}

impl std::io::Read for ChannelReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        loop {
            let read = std::io::Read::read(&mut self.pending, buf)?;
            if read > 0 {
                return Ok(read);
            }
            match self.incoming.recv() {
                Ok(bytes) => self.pending = std::io::Cursor::new(bytes),
                Err(_) => return Ok(0),
            }
        }
    }
}

fn session_with(mode: HerdrTerminalMode, stdin: Option<ChildStdin>) -> Arc<ConnectorSession> {
    Arc::new(ConnectorSession {
        id: "herdr-term-bounded".into(),
        mode,
        cols: Mutex::new(40),
        rows: Mutex::new(10),
        child: Mutex::new(None),
        process_tree: Mutex::new(None),
        stdin: Mutex::new(stdin),
        reader: Mutex::new(None),
        closed: Mutex::new(false),
        released: AtomicBool::new(false),
        on_event: None,
    })
}

fn session_without_a_child() -> Arc<ConnectorSession> {
    session_with(HerdrTerminalMode::Observe, None)
}

fn frame_line(seq: u64) -> Vec<u8> {
    format!(
        r#"{{"type":"terminal.frame","seq":{seq},"full":{},"encoding":"ansi","width":40,"height":10,"bytes":"AAA="}}
"#,
        seq == 1
    )
    .into_bytes()
}

#[test]
fn a_reader_let_go_by_its_release_reports_nothing_more() {
    let session = session_without_a_child();
    let (stdout_tx, stdout) = ChannelReader::new();
    let (on_event, events) = channel_events();
    let reader = {
        let session = session.clone();
        std::thread::spawn(move || {
            connector_reader_loop(session, stdout, None::<std::io::Empty>, on_event);
        })
    };
    stdout_tx.send(frame_line(1)).unwrap();
    first_frame(&events);
    // Only the release flag is raised, `closed` stays as it is: the reader sits
    // in its read, wakes up with a frame later and must keep it to itself.
    session.released.store(true, Ordering::Release);
    stdout_tx.send(frame_line(2)).unwrap();
    drop(stdout_tx);
    reader.join().unwrap();
    assert!(
        events.try_recv().is_err(),
        "an event was reported after the release"
    );
}

#[test]
fn stderr_lines_arriving_after_the_connector_ended_are_still_reported() {
    let session = session_without_a_child();
    let (stdout_tx, stdout) = ChannelReader::new();
    let (stderr_tx, stderr) = ChannelReader::new();
    let (on_event, events) = channel_events();
    let reader = {
        let session = session.clone();
        std::thread::spawn(move || connector_reader_loop(session, stdout, Some(stderr), on_event))
    };
    drop(stdout_tx);
    reader.join().unwrap();
    assert!(*session.closed.lock().unwrap());
    // The connector's last words on stderr come after its stdout ended, and
    // that is not a release.
    stderr_tx
        .send(b"connector failed: bad thing\n".to_vec())
        .unwrap();
    loop {
        match events.recv_timeout(Duration::from_secs(5)).expect("events") {
            HerdrTerminalEvent::Error { code, message, .. } => {
                assert_eq!(code, "connector_stderr");
                assert_eq!(message, "connector failed: bad thing");
                break;
            }
            HerdrTerminalEvent::Closed { .. } => {}
            other => panic!("unexpected event {other:?}"),
        }
    }
}

#[test]
fn release_is_bounded_while_a_stranger_holds_the_connector_stdout() {
    let dir = tempfile::tempdir().unwrap();
    let branch = format!(
        r#"  echo $$ > "@DIR@/connector.pid"
  {}
  printf '%s\n' '@FRAME1@'
  while IFS= read -r line; do :; done"#,
        stranger(dir.path())
    )
    .replace("@DIR@", dir.path().to_str().unwrap());
    let manager = Arc::new(HerdrManager::with_binary(write_fake_connector(
        dir.path(),
        &branch,
    )));
    let _sessions = KillSessions::new(&manager);
    let _cleanup = KillListed::new(dir.path().join("holders"));
    let mut connector_guard = KillListed::new(dir.path().join("connector.pid"));
    let (on_event, events) = channel_events();
    let id = open(&manager, "term", HerdrTerminalMode::Control, on_event);
    first_frame(&events);
    wait_for_pids(&dir.path().join("holders"), 1);
    let connector = wait_for_pids(&dir.path().join("connector.pid"), 1)[0];

    let started = Instant::now();
    let released = within(Duration::from_secs(10), {
        let manager = manager.clone();
        move || manager.terminal_release(&id)
    })
    .expect("the release hung on the stranger's pipe");
    let elapsed = started.elapsed();
    released.unwrap();
    // The reader stays stuck in its read (the stranger holds the pipe), so the
    // release waited for it exactly as long as its grace period.
    assert!(elapsed >= RELEASE_READER_GRACE, "{elapsed:?}");
    assert!(
        elapsed < RELEASE_READER_GRACE + Duration::from_secs(2),
        "{elapsed:?}"
    );
    assert!(
        !unix_pid_exists(connector as u32),
        "the connector must be gone"
    );
    connector_guard.disarm();

    // The stranger now writes a frame into the pipe the let-go reader sits on.
    while events.try_recv().is_ok() {}
    fs::write(dir.path().join("go"), "").unwrap();
    assert!(
        events.recv_timeout(Duration::from_millis(700)).is_err(),
        "the let-go reader reported an event"
    );
}

#[test]
fn stderr_lines_arriving_after_the_release_are_not_reported() {
    let dir = tempfile::tempdir().unwrap();
    let branch = format!(
        r#"  echo $$ > "@DIR@/connector.pid"
  {}
  printf '%s\n' '@FRAME1@'
  while IFS= read -r line; do :; done"#,
        START_STDERR_STRANGER.replace("@DIR@", dir.path().to_str().unwrap())
    )
    .replace("@DIR@", dir.path().to_str().unwrap());
    let manager = Arc::new(HerdrManager::with_binary(write_fake_connector(
        dir.path(),
        &branch,
    )));
    let _sessions = KillSessions::new(&manager);
    let _cleanup = KillListed::new(dir.path().join("holders"));
    let mut connector_guard = KillListed::new(dir.path().join("connector.pid"));
    let (on_event, events) = channel_events();
    let id = open(&manager, "term", HerdrTerminalMode::Control, on_event);
    first_frame(&events);
    wait_for_pids(&dir.path().join("holders"), 1);

    // Control: before the release the stranger's stderr line is reported, so
    // the silence below is the release's doing and not a dead stderr path.
    fs::write(dir.path().join("go"), "").unwrap();
    match events.recv_timeout(Duration::from_secs(5)).expect("events") {
        HerdrTerminalEvent::Error { code, message, .. } => {
            assert_eq!(code, "connector_stderr");
            assert_eq!(message, "early stderr line");
        }
        other => panic!("unexpected event {other:?}"),
    }

    within(Duration::from_secs(10), {
        let manager = manager.clone();
        move || manager.terminal_release(&id)
    })
    .expect("the release hung on the stranger's pipe")
    .unwrap();
    while events.try_recv().is_ok() {}
    // The stranger still holds the connector's stderr: its next line reaches
    // the thread that reads it, which must keep it to itself.
    fs::write(dir.path().join("go-late"), "").unwrap();
    assert!(
        events.recv_timeout(Duration::from_millis(700)).is_err(),
        "the stderr thread reported a line after the release"
    );
    assert_gone_and_disarm(&mut connector_guard);
}

#[test]
fn release_does_not_wait_behind_a_blocked_control_write() {
    let dir = tempfile::tempdir().unwrap();
    let branch = r#"  echo $$ > "@DIR@/connector.pid"
  printf '%s\n' '@FRAME1@'
  exec sleep 60"#
        .replace("@DIR@", dir.path().to_str().unwrap());
    let manager = Arc::new(HerdrManager::with_binary(write_fake_connector(
        dir.path(),
        &branch,
    )));
    let _sessions = KillSessions::new(&manager);
    let mut connector_guard = KillListed::new(dir.path().join("connector.pid"));
    let (on_event, events) = channel_events();
    let id = open(&manager, "term", HerdrTerminalMode::Control, on_event);
    first_frame(&events);
    let session = manager.sessions.lock().unwrap().get(&id).cloned().unwrap();

    // The connector never reads its stdin: the writer fills the pipe and then
    // sits in a blocking write holding the stdin lock.
    let writer = {
        let manager = manager.clone();
        let id = id.clone();
        std::thread::spawn(move || {
            let text = "x".repeat(4096);
            while manager
                .terminal_input(&id, Some(text.clone()), None)
                .is_ok()
            {}
        })
    };
    let blocked = wait_until(Instant::now() + Duration::from_secs(10), || {
        session.stdin.try_lock().is_err() && {
            std::thread::sleep(Duration::from_millis(200));
            session.stdin.try_lock().is_err()
        }
    });
    assert!(blocked, "the writer never blocked");

    let started = Instant::now();
    within(Duration::from_secs(10), {
        let manager = manager.clone();
        let id = id.clone();
        move || manager.terminal_release(&id)
    })
    .expect("the release waited behind the blocked write")
    .unwrap();
    assert!(started.elapsed() < Duration::from_secs(3));
    // The `released` flag ended the blocked write (on Unix its poll loop
    // checks it; the kill only matters for the thread variant).
    assert!(wait_until(Instant::now() + Duration::from_secs(5), || {
        writer.is_finished()
    }));
    writer.join().unwrap();
    assert_gone_and_disarm(&mut connector_guard);
}

#[test]
fn release_does_not_block_on_its_goodbye_when_the_connector_stopped_reading() {
    use std::os::fd::AsRawFd;
    let dir = tempfile::tempdir().unwrap();
    let branch = r#"  echo $$ > "@DIR@/connector.pid"
  printf '%s\n' '@FRAME1@'
  exec sleep 60"#
        .replace("@DIR@", dir.path().to_str().unwrap());
    let manager = Arc::new(HerdrManager::with_binary(write_fake_connector(
        dir.path(),
        &branch,
    )));
    let _sessions = KillSessions::new(&manager);
    let mut connector_guard = KillListed::new(dir.path().join("connector.pid"));
    let (on_event, events) = channel_events();
    let id = open(&manager, "term", HerdrTerminalMode::Control, on_event);
    first_frame(&events);
    let session = manager.sessions.lock().unwrap().get(&id).cloned().unwrap();

    // Fill the connector's stdin through a second descriptor on the same pipe:
    // nobody holds the stdin lock, but the pipe has no room for the goodbye.
    let fd = session.stdin.lock().unwrap().as_ref().unwrap().as_raw_fd();
    let filler_fd = unsafe { libc::dup(fd) };
    assert!(filler_fd >= 0);
    let writes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let filler = {
        let writes = writes.clone();
        std::thread::spawn(move || {
            let chunk = [b'x'; 4096];
            while unsafe { libc::write(filler_fd, chunk.as_ptr().cast(), chunk.len()) } > 0 {
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
    assert!(full, "the pipe never filled up");

    let started = Instant::now();
    within(Duration::from_secs(10), {
        let manager = manager.clone();
        move || manager.terminal_release(&id)
    })
    .expect("the goodbye blocked on the full pipe")
    .unwrap();
    assert!(started.elapsed() < Duration::from_secs(3));
    assert!(wait_until(Instant::now() + Duration::from_secs(5), || {
        filler.is_finished()
    }));
    filler.join().unwrap();
    assert_gone_and_disarm(&mut connector_guard);
}

/// A connector that never reads: its pid is recorded, one frame is shown and it
/// sleeps. `extra` runs before the sleep.
fn silent_connector_branch(dir: &Path, extra: &str) -> String {
    format!(
        "  echo $$ >> \"@DIR@/connector.pid\"\n  printf '%s\\n' '@FRAME1@'\n  {extra}\n  exec sleep 60"
    )
    .replace("@DIR@", dir.to_str().unwrap())
}

/// Writes 4 KiB inputs until one fails; the failure and how long it took.
fn write_until_error(manager: &HerdrManager, id: &str) -> (String, Duration) {
    let text = "x".repeat(4096);
    loop {
        let started = Instant::now();
        if let Err(error) = manager.terminal_input(id, Some(text.clone()), None) {
            return (error, started.elapsed());
        }
    }
}

/// An input that stops moving ends with the typed timeout error, the poisoned
/// connector is gone and its pane can be opened again.
fn assert_input_times_out(extra: &str) {
    let dir = tempfile::tempdir().unwrap();
    let extra = extra.replace("@DIR@", dir.path().to_str().unwrap());
    let extra = extra.as_str();
    let manager = Arc::new(HerdrManager::with_binary(write_fake_connector(
        dir.path(),
        &silent_connector_branch(dir.path(), extra),
    )));
    let _sessions = KillSessions::new(&manager);
    let mut connector_guard = KillListed::new(dir.path().join("connector.pid"));
    let _holders = KillListed::new(dir.path().join("holders"));
    let (on_event, events) = channel_events();
    let id = open(&manager, "term", HerdrTerminalMode::Control, on_event);
    first_frame(&events);
    if !extra.is_empty() {
        wait_for_pids(&dir.path().join("holders"), 1);
    }

    let (error, took) = within(Duration::from_secs(20), {
        let manager = manager.clone();
        let id = id.clone();
        move || write_until_error(&manager, &id)
    })
    .expect("the input write never returned");
    assert!(error.starts_with("ioTimeout:"), "{error}");
    assert!(
        took < INPUT_WRITE_TIMEOUT + Duration::from_secs(4),
        "{took:?}"
    );
    assert!(!manager.sessions.lock().unwrap().contains_key(&id));
    assert_gone_and_disarm(&mut connector_guard);
    // Released already: nothing is left to release, and nothing to write to.
    assert!(manager.terminal_release(&id).is_err());
    assert!(manager.terminal_input(&id, Some("x".into()), None).is_err());

    // The pane can be opened again.
    let (on_event, events) = channel_events();
    let again = open(&manager, "term", HerdrTerminalMode::Control, on_event);
    first_frame(&events);
    assert_ne!(again, id);
    manager.terminal_release(&again).unwrap();
}

#[test]
fn input_to_a_connector_that_stopped_reading_times_out_and_tears_down() {
    assert_input_times_out("");
}

/// A stranger holds a copy of the connector's read end, so the kill alone would
/// never break the pipe: the write has to give up by itself.
#[test]
fn input_times_out_although_a_stranger_holds_the_read_end() {
    // Its stdin is the connector's: an explicit redirect keeps it for a job
    // that is started in the background.
    let stranger = r#"perl -MPOSIX -e 'POSIX::setsid(); open(my $f, ">>", $ARGV[0]) or die; print $f "$$\n"; close $f; sleep 20' "@DIR@/holders" <&0 &"#;
    assert_input_times_out(stranger);
}

/// A second writer queued behind the stuck one is bounded as well.
#[test]
fn a_writer_queued_behind_a_stuck_input_write_returns_within_the_bound() {
    let dir = tempfile::tempdir().unwrap();
    let manager = Arc::new(HerdrManager::with_binary(write_fake_connector(
        dir.path(),
        &silent_connector_branch(dir.path(), ""),
    )));
    let _sessions = KillSessions::new(&manager);
    let mut connector_guard = KillListed::new(dir.path().join("connector.pid"));
    let (on_event, events) = channel_events();
    let id = open(&manager, "term", HerdrTerminalMode::Control, on_event);
    first_frame(&events);
    let session = manager.sessions.lock().unwrap().get(&id).cloned().unwrap();

    let first = {
        let manager = manager.clone();
        let id = id.clone();
        std::thread::spawn(move || write_until_error(&manager, &id))
    };
    let blocked = wait_until(Instant::now() + Duration::from_secs(10), || {
        session.stdin.try_lock().is_err() && {
            std::thread::sleep(Duration::from_millis(100));
            session.stdin.try_lock().is_err()
        }
    });
    assert!(blocked, "the writer never blocked");

    let (error, took) = within(Duration::from_secs(20), {
        let manager = manager.clone();
        let id = id.clone();
        move || {
            let started = Instant::now();
            (
                manager.terminal_input(&id, Some("queued".into()), None),
                started.elapsed(),
            )
        }
    })
    .map(|(result, took)| (result.unwrap_err(), took))
    .expect("the queued writer waited behind the stuck one");
    assert!(
        took < INPUT_WRITE_TIMEOUT + Duration::from_secs(4),
        "{took:?}"
    );
    assert!(!error.is_empty());
    let (first_error, _) = within(Duration::from_secs(20), move || first.join().unwrap())
        .expect("the stuck writer never returned");
    assert!(first_error.starts_with("ioTimeout:"), "{first_error}");
    assert_gone_and_disarm(&mut connector_guard);
}

/// A release while the timeout tears the connector down must stay safe.
#[test]
fn a_release_racing_the_input_timeout_is_safe() {
    let dir = tempfile::tempdir().unwrap();
    let manager = Arc::new(HerdrManager::with_binary(write_fake_connector(
        dir.path(),
        &silent_connector_branch(dir.path(), ""),
    )));
    let _sessions = KillSessions::new(&manager);
    let mut connector_guard = KillListed::new(dir.path().join("connector.pid"));
    let (on_event, events) = channel_events();
    let id = open(&manager, "term", HerdrTerminalMode::Control, on_event);
    first_frame(&events);
    let writer = {
        let manager = manager.clone();
        let id = id.clone();
        std::thread::spawn(move || write_until_error(&manager, &id))
    };
    // Around the moment the clock runs out.
    std::thread::sleep(INPUT_WRITE_TIMEOUT + Duration::from_millis(400));
    let _ = within(Duration::from_secs(10), {
        let manager = manager.clone();
        let id = id.clone();
        move || manager.terminal_release(&id)
    })
    .expect("the release hung");
    let (error, _) =
        within(Duration::from_secs(20), move || writer.join().unwrap()).expect("the writer hung");
    assert!(!error.is_empty());
    assert!(!manager.sessions.lock().unwrap().contains_key(&id));
    assert_gone_and_disarm(&mut connector_guard);
}

/// The thread variant (the only one on Windows) against a pipe nobody reads.
#[test]
fn the_thread_write_gives_up_on_a_pipe_nobody_reads() {
    let mut child = Command::new("sleep")
        .arg("60")
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    let stdin = child.stdin.take().unwrap();
    let started = Instant::now();
    let result = write_on_thread(
        stdin,
        vec![b'x'; 1024 * 1024],
        Duration::from_millis(300),
        "test-input",
    );
    assert!(matches!(result, Err(ThreadWrite::Stalled)));
    assert!(started.elapsed() < Duration::from_secs(5));
    // The kill breaks the pipe and ends the thread's write.
    child.kill().unwrap();
    child.wait().unwrap();
}

/// Forward progress restarts the clock: a slow reader is not cut off.
#[test]
fn the_thread_write_keeps_going_while_the_reader_makes_progress() {
    let mut child = Command::new("sh")
        .args([
            "-c",
            "while dd bs=4096 count=1 of=/dev/null 2>/dev/null; do sleep 0.05; done",
        ])
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    let stdin = child.stdin.take().unwrap();
    let started = Instant::now();
    let result = write_on_thread(
        stdin,
        vec![b'x'; 256 * 1024],
        Duration::from_millis(600),
        "test-input",
    );
    assert!(result.is_ok());
    assert!(
        started.elapsed() > Duration::from_millis(600),
        "too fast to prove anything: {:?}",
        started.elapsed()
    );
    let _ = child.kill();
    let _ = child.wait();
}

/// A release terminates the connector first and only then waits for its reader.
/// The connector is cooperative (it ends on the polite signal) and nobody else
/// holds its stdout, so the reader ends right after the signal; waiting for it
/// before the signal would take the whole grace, as the connector is alive and
/// holds its stdout.
#[test]
fn release_terminates_the_connector_before_it_waits_for_the_reader() {
    for mode in [HerdrTerminalMode::Observe, HerdrTerminalMode::Control] {
        let dir = tempfile::tempdir().unwrap();
        let branch = r#"  echo $$ > "@DIR@/connector.pid"
  printf '%s\n' '@FRAME1@'
  exec sleep 60"#
            .replace("@DIR@", dir.path().to_str().unwrap());
        let manager = Arc::new(HerdrManager::with_binary(write_fake_connector(
            dir.path(),
            &branch,
        )));
        let _sessions = KillSessions::new(&manager);
        let mut connector_guard = KillListed::new(dir.path().join("connector.pid"));
        let (on_event, events) = channel_events();
        let id = open(&manager, "term", mode, on_event);
        first_frame(&events);

        let started = Instant::now();
        within(Duration::from_secs(10), {
            let manager = manager.clone();
            move || manager.terminal_release(&id)
        })
        .expect("the release hung")
        .unwrap();
        let elapsed = started.elapsed();
        assert!(elapsed < RELEASE_READER_GRACE / 2, "{mode:?}: {elapsed:?}");
        assert_gone_and_disarm(&mut connector_guard);
    }
}

#[test]
fn release_all_is_bounded_for_stubborn_and_stranded_connectors() {
    let dir = tempfile::tempdir().unwrap();
    // `stubborn` ignores the polite signal, `stranded` leaves a stranger on its
    // stdout; two kinds, three of each. A stubborn one also ends by itself
    // after about 20 s, so a test that dies before the kill leaves none behind.
    let branch = format!(
        r#"  echo $$ >> "@DIR@/connectors"
  case "$4" in
    stubborn)
      trap '' TERM HUP
      printf '%s\n' '@FRAME1@'
      n=0; while [ $n -lt 200 ]; do sleep 0.1; n=$((n+1)); done ;;
    *)
      {}
      printf '%s\n' '@FRAME1@'
      sleep 30 ;;
  esac"#,
        stranger(dir.path())
    )
    .replace("@DIR@", dir.path().to_str().unwrap());
    let manager = Arc::new(HerdrManager::with_binary(write_fake_connector(
        dir.path(),
        &branch,
    )));
    let _sessions = KillSessions::new(&manager);
    let _cleanup = KillListed::new(dir.path().join("holders"));
    let mut connectors_guard = KillListed::new(dir.path().join("connectors"));
    let (on_event, events) = channel_events();
    for index in 0..6 {
        let target = if index % 2 == 0 {
            "stubborn"
        } else {
            "stranded"
        };
        let mode = if index < 3 {
            HerdrTerminalMode::Observe
        } else {
            HerdrTerminalMode::Control
        };
        open(&manager, target, mode, on_event.clone());
    }
    for _ in 0..6 {
        first_frame(&events);
    }
    let connectors = wait_for_pids(&dir.path().join("connectors"), 6);
    wait_for_pids(&dir.path().join("holders"), 3);

    let started = Instant::now();
    within(Duration::from_secs(15), {
        let manager = manager.clone();
        move || manager.release_all_connectors()
    })
    .expect("release_all_connectors hung");
    let elapsed = started.elapsed();
    // The stubborn ones only die when the kill comes at the end of the shared
    // polite wait; nothing waits for the readers a stranger holds.
    assert!(elapsed >= RELEASE_ALL_TERM_WAIT, "{elapsed:?}");
    assert!(elapsed < Duration::from_secs(3), "{elapsed:?}");
    assert!(manager.sessions.lock().unwrap().is_empty());
    for pid in connectors {
        assert!(!unix_pid_exists(pid as u32), "connector {pid} survived");
    }
    connectors_guard.disarm();
}

#[test]
fn release_all_kills_a_group_member_that_outlives_its_connector() {
    let dir = tempfile::tempdir().unwrap();
    // The connector obeys the polite signal; a member of its group ignores it
    // and keeps the group alive after the connector itself is gone (for about
    // 20 s at most). The member lists itself once its trap is in place, so the
    // release cannot beat the trap.
    let branch = r#"  echo $$ >> "@DIR@/connectors"
  sh -c 'trap "" TERM HUP; echo $$ >> "@DIR@/members"; n=0; while [ $n -lt 200 ]; do sleep 0.1; n=$((n+1)); done' &
  printf '%s\n' '@FRAME1@'
  exec sleep 30"#
        .replace("@DIR@", dir.path().to_str().unwrap());
    let manager = Arc::new(HerdrManager::with_binary(write_fake_connector(
        dir.path(),
        &branch,
    )));
    let _sessions = KillSessions::new(&manager);
    let mut members_guard = KillListed::new(dir.path().join("members"));
    let mut connectors_guard = KillListed::new(dir.path().join("connectors"));
    let (on_event, events) = channel_events();
    open(&manager, "term", HerdrTerminalMode::Observe, on_event);
    first_frame(&events);
    let member = wait_for_pids(&dir.path().join("members"), 1)[0];

    let started = Instant::now();
    within(Duration::from_secs(10), {
        let manager = manager.clone();
        move || manager.release_all_connectors()
    })
    .expect("release_all_connectors hung");
    let elapsed = started.elapsed();
    // The connector is gone at once, but release waits for its whole group
    // until the shared polite wait is over, then kills what is left.
    assert!(elapsed >= RELEASE_ALL_TERM_WAIT, "{elapsed:?}");
    assert!(elapsed < Duration::from_secs(3), "{elapsed:?}");
    assert!(!unix_pid_exists(member as u32), "the member survived");
    assert_gone_and_disarm(&mut connectors_guard);
    members_guard.disarm();
}

#[test]
fn release_all_ends_cooperative_connectors_without_waiting_out_the_deadline() {
    let dir = tempfile::tempdir().unwrap();
    let branch = r#"  echo $$ >> "@DIR@/connectors"
  printf '%s\n' '@FRAME1@'
  exec sleep 30"#
        .replace("@DIR@", dir.path().to_str().unwrap());
    let manager = Arc::new(HerdrManager::with_binary(write_fake_connector(
        dir.path(),
        &branch,
    )));
    let _sessions = KillSessions::new(&manager);
    let mut connectors_guard = KillListed::new(dir.path().join("connectors"));
    let (on_event, events) = channel_events();
    for index in 0..4 {
        let mode = if index % 2 == 0 {
            HerdrTerminalMode::Observe
        } else {
            HerdrTerminalMode::Control
        };
        open(&manager, "term", mode, on_event.clone());
    }
    for _ in 0..4 {
        first_frame(&events);
    }
    let connectors = wait_for_pids(&dir.path().join("connectors"), 4);

    // A deadline far beyond what the test tolerates: only a connector that is
    // never asked politely would wait for it.
    let deadline = Duration::from_secs(8);
    let started = Instant::now();
    within(Duration::from_secs(15), {
        let manager = manager.clone();
        move || manager.release_all_connectors_within(deadline, RELEASE_ALL_KILL_WAIT)
    })
    .expect("release_all_connectors hung");
    // They obey the polite signal, so nothing waits for the deadline.
    let elapsed = started.elapsed();
    assert!(elapsed < Duration::from_secs(4), "{elapsed:?}");
    for pid in connectors {
        assert!(!unix_pid_exists(pid as u32), "connector {pid} survived");
    }
    connectors_guard.disarm();
}

// ── The goodbye of a connector release ──────────────────────────────────────

/// What a connector reads from its stdin when its session is released as `mode`.
/// The connector is a `cat` into a file, which only ends at EOF: it ends only
/// if the release closed the stdin, and the file then holds all it was sent.
/// The session is released directly: a `terminal_release` would signal the
/// connector in the same moment and could end it before it had written.
fn stdin_of_a_connector_released_as(mode: HerdrTerminalMode) -> Vec<u8> {
    let dir = tempfile::tempdir().unwrap();
    let received = dir.path().join("received");
    let mut child = KillChild(spawn_stdin_sh(r#"cat > "$1""#, &received));
    let session = session_with(mode, child.0.stdin.take());
    begin_connector_release(&session);
    let mut status = None;
    let ended = wait_until(Instant::now() + Duration::from_secs(10), || {
        status = child.0.try_wait().unwrap();
        status.is_some()
    });
    assert!(ended, "the connector never saw its stdin close");
    assert!(status.unwrap().success());
    fs::read(&received).unwrap()
}

#[test]
fn a_control_connector_is_told_goodbye_before_its_stdin_closes() {
    // The literal is the wire line HERDR expects, not `CONNECTOR_RELEASE_LINE`.
    assert_eq!(
        stdin_of_a_connector_released_as(HerdrTerminalMode::Control),
        b"{\"type\":\"terminal.release\"}\n"
    );
}

#[test]
fn an_observe_connector_only_sees_its_stdin_close() {
    // Observe has no release command on the wire.
    assert!(stdin_of_a_connector_released_as(HerdrTerminalMode::Observe).is_empty());
}

/// What a Control connector read from its stdin when `release`, one of the
/// release entry points, released its Session. The connector hands its stdin to
/// a stranger in a session of its own: the kill of the connector's group cannot
/// reach it, so it reads on to the EOF whatever the release does, records
/// everything in `received` and then creates `done`. The stranger ends by
/// itself after 20 s if nothing ever closes the stdin.
fn stdin_of_a_connector_released_by(
    release: impl FnOnce(&Arc<HerdrManager>, String) + Send + 'static,
) -> Vec<u8> {
    let dir = tempfile::tempdir().unwrap();
    let branch = r#"  exec 3<&0
  perl -MPOSIX -e 'POSIX::setsid(); alarm 20; open(my $f, ">>", $ARGV[2]) or die; print $f "$$\n"; close $f; binmode STDIN; local $/; my $data = <STDIN>; open(my $o, ">", $ARGV[0]) or die; print $o (defined $data ? $data : ""); close $o; open(my $d, ">", $ARGV[1]) or die; close $d' "@DIR@/received" "@DIR@/done" "@DIR@/holders" <&3 3<&- >/dev/null 2>&1 &
  exec 3<&-
  printf '%s\n' '@FRAME1@'
  exec sleep 60"#
        .replace("@DIR@", dir.path().to_str().unwrap());
    let manager = Arc::new(HerdrManager::with_binary(write_fake_connector(
        dir.path(),
        &branch,
    )));
    let _sessions = KillSessions::new(&manager);
    let mut stranger_guard = KillListed::new(dir.path().join("holders"));
    let (on_event, events) = channel_events();
    let id = open(&manager, "term", HerdrTerminalMode::Control, on_event);
    first_frame(&events);
    // The stranger is up, and out of the connector's group, once it is listed.
    wait_for_pids(&dir.path().join("holders"), 1);

    within(Duration::from_secs(10), {
        let manager = manager.clone();
        move || release(&manager, id)
    })
    .expect("the release hung");
    let done = dir.path().join("done");
    assert!(
        wait_until(Instant::now() + Duration::from_secs(10), || done.exists()),
        "the stranger never saw its stdin close"
    );
    stranger_guard.disarm();
    fs::read(dir.path().join("received")).unwrap()
}

#[test]
fn terminal_release_tells_a_control_connector_goodbye() {
    assert_eq!(
        stdin_of_a_connector_released_by(|manager, id| manager.terminal_release(&id).unwrap()),
        b"{\"type\":\"terminal.release\"}\n"
    );
}

#[test]
fn release_all_connectors_tells_a_control_connector_goodbye() {
    assert_eq!(
        stdin_of_a_connector_released_by(|manager, _| manager.release_all_connectors()),
        b"{\"type\":\"terminal.release\"}\n"
    );
}

// ── Targets that are terminated by a call, not by a signal (Windows) ────────

/// A target whose termination takes `duration` and then raises `done`.
fn slow_target(duration: Duration, done: Arc<AtomicBool>) -> ReleaseTarget {
    ReleaseTarget {
        child: Some(Box::new(TerminateNow::new(move || {
            std::thread::sleep(duration);
            done.store(true, Ordering::Release);
        }))),
        reader: None,
    }
}

#[test]
fn every_target_is_terminated_at_the_same_time_and_the_wait_ends_with_the_last() {
    let flags: Vec<_> = (0..8).map(|_| Arc::new(AtomicBool::new(false))).collect();
    let targets = flags
        .iter()
        .map(|done| slow_target(Duration::from_millis(300), done.clone()))
        .collect();
    let started = Instant::now();
    within(Duration::from_secs(10), move || {
        release_together(targets, Duration::from_secs(5), Duration::from_secs(5));
    })
    .expect("release_together hung");
    // One after the other these would take 8 x 300 ms.
    assert!(started.elapsed() < Duration::from_millis(1500));
    assert!(flags.iter().all(|done| done.load(Ordering::Acquire)));
}

#[test]
fn terminations_that_never_end_are_let_go_at_the_shared_deadlines() {
    let flags: Vec<_> = (0..4).map(|_| Arc::new(AtomicBool::new(false))).collect();
    let targets = flags
        .iter()
        .map(|done| slow_target(Duration::from_secs(3), done.clone()))
        .collect();
    let started = Instant::now();
    within(Duration::from_secs(10), move || {
        release_together(
            targets,
            Duration::from_millis(300),
            Duration::from_millis(200),
        );
    })
    .expect("release_together waited for the terminations");
    let elapsed = started.elapsed();
    assert!(elapsed >= Duration::from_millis(500), "{elapsed:?}");
    assert!(elapsed < Duration::from_millis(2500), "{elapsed:?}");
    // They are only let go: nothing ran to its end yet.
    assert!(flags.iter().all(|done| !done.load(Ordering::Acquire)));
}

/// Kills and reaps its child when dropped, panic or not.
struct KillChild(Child);

impl Drop for KillChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn spawn_stdin_sh(script: &str, arg: &Path) -> Child {
    let mut command = Command::new("/bin/sh");
    command
        .args(["-c", script, "sh"])
        .arg(arg)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    process_kill::spawn_std(&mut command).unwrap()
}

#[test]
fn the_goodbye_on_a_thread_is_written_and_then_closes_the_connector_stdin() {
    let dir = tempfile::tempdir().unwrap();
    let received = dir.path().join("received");
    let mut child = KillChild(spawn_stdin_sh(r#"cat > "$1""#, &received));
    let stdin = child.0.stdin.take().unwrap();
    write_connector_release_on_thread(stdin);
    // `cat` only ends at EOF: the thread wrote the line and dropped the stdin.
    let mut status = None;
    let ended = wait_until(Instant::now() + Duration::from_secs(10), || {
        status = child.0.try_wait().unwrap();
        status.is_some()
    });
    assert!(ended, "the connector never saw its stdin close");
    assert!(status.unwrap().success());
    assert_eq!(fs::read(&received).unwrap(), CONNECTOR_RELEASE_LINE);
}

#[test]
fn the_goodbye_on_a_thread_does_not_wait_for_a_connector_that_stopped_reading() {
    use std::os::fd::AsRawFd;
    let dir = tempfile::tempdir().unwrap();
    let mut child = KillChild(spawn_stdin_sh(r#"exec sleep 60"#, dir.path()));
    let mut stdin = child.0.stdin.take().unwrap();
    // Fill the pipe to the byte, then make it blocking again: the goodbye
    // finds no room, like a Windows pipe.
    let fd = stdin.as_raw_fd();
    set_nonblocking(fd).unwrap();
    for chunk in [4096, 1] {
        let bytes = vec![b'x'; chunk];
        loop {
            match std::io::Write::write(&mut stdin, &bytes) {
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(error) => panic!("{error}"),
            }
        }
    }
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    assert_eq!(
        unsafe { libc::fcntl(fd, libc::F_SETFL, flags & !libc::O_NONBLOCK) },
        0
    );

    let returned = within(Duration::from_secs(10), move || {
        write_connector_release_on_thread(stdin);
    });
    // The thread is stuck in its write; the kill ends it.
    let _ = child.0.kill();
    let _ = child.0.wait();
    returned.expect("the goodbye blocked its caller on the full pipe");
}

// ── Review fixes: busy waiters, the stall event, per-byte progress ──────────

/// Fills `fd`'s pipe to the last byte without ever blocking, then restores its
/// flags: the next write waits for a reader.
fn fill_pipe(fd: std::os::fd::RawFd) {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    assert!(flags != -1);
    assert!(unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } != -1);
    let chunk = [b'x'; 4096];
    for size in [chunk.len(), 1] {
        while unsafe { libc::write(fd, chunk.as_ptr().cast(), size) } > 0 {}
    }
    assert!(unsafe { libc::fcntl(fd, libc::F_SETFL, flags) } != -1);
}

/// A connector that keeps reading its stdin, 4 KiB every 50 ms (about 80 KB/s).
fn slow_reading_connector_branch(dir: &Path) -> String {
    silent_connector_branch(dir, "").replace(
        "exec sleep 60",
        r#"exec perl -e '$| = 1; while (sysread(STDIN, my $b, 4096)) { select(undef, undef, undef, 0.05) }'"#,
    )
}

/// A command queued behind a large paste that is still moving must not take the
/// session down: the holder is bounded by its own stall timer. The waiter only
/// reports busy once its own wait ran out (here 1 s, the paste takes ~3 s).
#[test]
fn a_command_waiting_behind_a_progressing_paste_leaves_the_session_alone() {
    let dir = tempfile::tempdir().unwrap();
    let manager = Arc::new(HerdrManager::with_binary(write_fake_connector(
        dir.path(),
        &slow_reading_connector_branch(dir.path()),
    )));
    let _sessions = KillSessions::new(&manager);
    let mut connector_guard = KillListed::new(dir.path().join("connector.pid"));
    let (on_event, events) = channel_events();
    let id = open(&manager, "term", HerdrTerminalMode::Control, on_event);
    first_frame(&events);
    let session = manager.sessions.lock().unwrap().get(&id).cloned().unwrap();

    let paste = {
        let manager = manager.clone();
        let id = id.clone();
        std::thread::spawn(move || {
            let started = Instant::now();
            let result = manager.terminal_input(&id, Some("x".repeat(256 * 1024)), None);
            (result, started.elapsed())
        })
    };
    let holding = wait_until(Instant::now() + Duration::from_secs(10), || {
        session.stdin.try_lock().is_err()
    });
    assert!(holding, "the paste never started");

    let error = within(Duration::from_secs(20), {
        let manager = manager.clone();
        let id = id.clone();
        move || manager.terminal_resize(&id, 50, 12)
    })
    .expect("the waiting resize never returned")
    .unwrap_err();
    assert_eq!(error, CONTROL_BUSY_ERROR);
    assert!(!session.released.load(Ordering::Acquire));
    assert!(manager.sessions.lock().unwrap().contains_key(&id));

    let (result, took) = within(Duration::from_secs(30), move || paste.join().unwrap())
        .expect("the paste never returned");
    result.expect("the paste was cut off by a waiter");
    assert!(
        took > INPUT_WRITE_TIMEOUT,
        "too fast to prove anything: {took:?}"
    );
    // Nothing was reported to the frontend, and the session still works.
    assert!(events.try_recv().is_err());
    manager
        .terminal_scroll(&id, HerdrScrollDirection::Up, 1, None, None)
        .unwrap();
    manager.terminal_release(&id).unwrap();
    assert_gone_and_disarm(&mut connector_guard);
}

/// A stall that is not an input write (here a scroll) tears the connector down
/// and the frontend is told: the reader is silenced by the release, so the Host
/// itself reports it first.
#[test]
fn a_stalled_scroll_tears_down_and_tells_the_frontend() {
    use std::os::fd::AsRawFd;
    let dir = tempfile::tempdir().unwrap();
    let manager = Arc::new(HerdrManager::with_binary(write_fake_connector(
        dir.path(),
        &silent_connector_branch(dir.path(), ""),
    )));
    let _sessions = KillSessions::new(&manager);
    let mut connector_guard = KillListed::new(dir.path().join("connector.pid"));
    let (on_event, events) = channel_events();
    let id = open(&manager, "term", HerdrTerminalMode::Control, on_event);
    first_frame(&events);
    let session = manager.sessions.lock().unwrap().get(&id).cloned().unwrap();
    fill_pipe(session.stdin.lock().unwrap().as_ref().unwrap().as_raw_fd());

    let error = within(Duration::from_secs(20), {
        let manager = manager.clone();
        let id = id.clone();
        move || manager.terminal_scroll(&id, HerdrScrollDirection::Up, 1, None, None)
    })
    .expect("the scroll never returned")
    .unwrap_err();
    assert!(error.starts_with("ioTimeout:"), "{error}");
    assert!(!manager.sessions.lock().unwrap().contains_key(&id));
    assert_gone_and_disarm(&mut connector_guard);

    let mut told = None;
    while let Ok(event) = events.recv_timeout(Duration::from_secs(2)) {
        if let HerdrTerminalEvent::Resync {
            session_id,
            message,
            ..
        } = event
        {
            told = Some((session_id, message));
            break;
        }
    }
    let (session_id, message) = told.expect("the frontend was never told");
    assert_eq!(session_id, id);
    assert!(message.starts_with("ioTimeout:"), "{message}");
}

/// A reader that takes 512 bytes per 250 ms is slow, not stalled, even though a
/// 4 KiB chunk would need two seconds against the one second limit.
#[test]
fn the_thread_write_counts_progress_per_byte_not_per_large_chunk() {
    use std::os::fd::AsRawFd;
    let dir = tempfile::tempdir().unwrap();
    let go = dir.path().join("go");
    let script = format!(
        "until [ -e '{}' ]; do sleep 0.01; done; while dd bs=512 count=1 of=/dev/null 2>/dev/null; do sleep 0.25; done",
        go.display()
    );
    let mut child = Command::new("sh")
        .args(["-c", &script])
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    let stdin = child.stdin.take().unwrap();
    fill_pipe(stdin.as_raw_fd());
    fs::write(&go, "").unwrap();

    let started = Instant::now();
    let result = write_on_thread(
        stdin,
        vec![b'x'; 4096],
        Duration::from_secs(1),
        "test-input",
    );
    let elapsed = started.elapsed();
    let _ = child.kill();
    let _ = child.wait();
    assert!(result.is_ok(), "judged stalled after {elapsed:?}");
    assert!(
        elapsed > Duration::from_secs(1),
        "too fast to prove anything: {elapsed:?}"
    );
}
