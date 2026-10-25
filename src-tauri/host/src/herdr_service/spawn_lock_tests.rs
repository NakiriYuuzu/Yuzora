//! Every HERDR spawn site takes the process-wide spawn lock. Each test runs the
//! site on this thread against a fixture binary and reads back, with
//! `process_kill::trace_spawns`, what this thread spawned through the lock, so a
//! site that went back to a bare `spawn()` is missing from the trace.
use super::*;
use std::os::unix::net::UnixListener;

#[test]
fn herdr_cli_json_spawns_through_the_spawn_lock() {
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), br#"{"ok":true}"#).unwrap();
    let path = file.path().to_str().unwrap();
    let (value, spawns) = process_kill::trace_spawns(|| {
        run_herdr_json_with_session_timeout(
            Path::new("/bin/cat"),
            &[path],
            None,
            Duration::from_secs(2),
        )
    });
    assert_eq!(value.unwrap()["ok"], true);
    assert_eq!(spawns, [format!("/bin/cat {path}")]);
}

#[test]
fn the_startup_server_launch_spawns_through_the_spawn_lock() {
    let dir = tempfile::tempdir().unwrap();
    let binary = write_fake_herdr_startup(dir.path());
    let manager = HerdrManager::with_binary(binary.clone());
    let (started, spawns) =
        process_kill::trace_spawns(|| manager.ensure_server_running_on_startup());
    assert!(started.unwrap());
    assert!(
        spawns.contains(&format!("{} server", binary.display())),
        "{spawns:?}"
    );
}

#[test]
fn a_named_session_start_spawns_its_server_through_the_spawn_lock() {
    let dir = tempfile::tempdir().unwrap();
    let binary = write_fake_herdr_startup(dir.path());
    let manager = HerdrManager::with_binary(binary.clone());
    let (started, spawns) = process_kill::trace_spawns(|| {
        manager.feature("default", HerdrFeatureRequest::SessionStart {})
    });
    assert_eq!(started.unwrap()["started"], true);
    assert!(
        spawns.contains(&format!("{} --session default server", binary.display())),
        "{spawns:?}"
    );
}

#[test]
fn a_feature_cli_command_spawns_through_the_spawn_lock() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("features.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    // The capability probe pings once; the checks after it reuse that identity.
    // Every wait here is bounded: a command that skips the probe must fail the
    // test, not wait for a connection that never comes.
    let returned = Arc::new(AtomicBool::new(false));
    let server = std::thread::spawn({
        let returned = returned.clone();
        move || -> Result<(), String> {
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut stream = loop {
                // Read before the accept: a ping made before the command returned
                // is queued by then, so an empty queue after it means no ping.
                let command_returned = returned.load(Ordering::Acquire);
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        if command_returned || Instant::now() >= deadline {
                            return Err("the command never probed the session".into());
                        }
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => return Err(error.to_string()),
                }
            };
            // An accepted socket may inherit the listener's non-blocking mode.
            stream.set_nonblocking(false).map_err(|e| e.to_string())?;
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .map_err(|e| e.to_string())?;
            let mut request = String::new();
            BufReader::new(stream.try_clone().map_err(|e| e.to_string())?)
                .read_line(&mut request)
                .map_err(|e| e.to_string())?;
            stream
                .write_all(b"{\"id\":\"ping\",\"result\":{\"type\":\"pong\",\"version\":\"0.8.0\",\"protocol\":19}}\n")
                .map_err(|e| e.to_string())
        }
    });
    let status = serde_json::json!({
        "client": {"version": "0.8.0", "channel": "test", "protocol": 19, "binary": "FAKE"},
        "server": {"status": "running", "running": true, "version": "0.8.0",
                   "protocol": 19, "compatible": true, "socket": socket}
    })
    .to_string();
    let schema = serde_json::json!({
        "protocol": 19, "schema_version": 1, "methods": ["session.snapshot", "plugin.list"]
    })
    .to_string();
    let sessions = serde_json::json!({"sessions": [{
        "name": "default", "default": true, "running": true,
        "session_dir": "/tmp/herdr-default", "socket_path": socket
    }]})
    .to_string();
    let binary = write_fake_herdr_with_sessions(dir.path(), &status, &schema, &sessions);
    let manager = HerdrManager::with_binary(binary.clone());
    let (result, spawns) = process_kill::trace_spawns(|| {
        manager.feature(
            "default",
            HerdrFeatureRequest::PluginInstall {
                source: "owner/repo".into(),
                revision: None,
            },
        )
    });
    // The fixture rejects the plugin command; the spawn is what matters here.
    assert!(result.is_err());
    assert!(
        spawns.contains(&format!(
            "{} --session default plugin install owner/repo --yes",
            binary.display()
        )),
        "{result:?} {spawns:?}"
    );
    returned.store(true, Ordering::Release);
    server.join().unwrap().unwrap();
}

#[test]
fn a_terminal_connector_spawns_through_the_spawn_lock() {
    let dir = tempfile::tempdir().unwrap();
    let binary = write_fake_herdr_running_session(dir.path());
    let manager = Arc::new(HerdrManager::with_binary(binary.clone()));
    let on_event: OnTerminalEvent = Arc::new(|_| Ok(()));
    let (opened, spawns) = process_kill::trace_spawns(|| {
        manager.open_terminal(
            "term_test".into(),
            HerdrTerminalMode::Observe,
            false,
            40,
            10,
            None,
            on_event,
        )
    });
    let opened = opened.unwrap();
    manager.terminal_release(&opened.session_id).unwrap();
    assert!(
        spawns.contains(&format!(
            "{} terminal session observe term_test --cols 40 --rows 10",
            binary.display()
        )),
        "{spawns:?}"
    );
}
