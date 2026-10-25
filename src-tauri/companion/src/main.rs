#![forbid(unsafe_code)]

mod parent;

use std::{future::Future, time::Duration};
use tokio::{
    io::{AsyncBufRead, BufReader},
    sync::oneshot,
    task::JoinSet,
    time::{timeout, timeout_at, Instant},
};
use yuzora_companion::{
    admin::{self, CommandBody, Event, EventSender},
    clock::Clock,
    gateway::Gateway,
    http::Server,
};

fn main() {
    // First: the pid this process was started under, before anything else can
    // take long (the first command may arrive five seconds from now).
    let parent = parent::probe();
    // Must run before the Tokio runtime (or any other thread) is created.
    // Host connector commands otherwise inherit pane-local routing overrides.
    for (name, _) in std::env::vars_os() {
        if name
            .to_string_lossy()
            .to_ascii_uppercase()
            .starts_with("HERDR_")
        {
            std::env::remove_var(name);
        }
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Companion runtime")
        .block_on(run_main(parent));
}

async fn run_main(parent: Option<parent::Probe>) {
    let (events, mut writer) = admin::spawn_writer(tokio::io::stdout());
    let mut shutdown_started = None;
    let mut success = run(
        &events,
        BufReader::new(tokio::io::stdin()),
        &mut shutdown_started,
        parent::watch(parent, parent::POLL),
    )
    .await;
    let writer_started = std::time::Instant::now();
    let shutdown_started = shutdown_started.unwrap_or(writer_started);
    timing("writer-join-start", writer_started);
    drop(events);
    let writer_ok = match timeout(Duration::from_secs(1), &mut writer).await {
        Ok(result) => matches!(result, Ok(Ok(()))),
        Err(_) => {
            writer.abort();
            let _ = writer.await;
            false
        }
    };
    timing("writer-join", writer_started);
    timing("total", shutdown_started);
    if !writer_ok {
        eprintln!("Companion admin stdout unavailable");
        success = false;
    }
    // Tokio stdin uses a non-cancellable blocking read. Explicit exit after
    // listener/task teardown avoids waiting forever for that thread on shutdown.
    std::process::exit(if success { 0 } else { 1 });
}

// Opt-in test diagnostics only: fixed stage names/durations, no command data.
fn timing(stage: &str, started: std::time::Instant) {
    if std::env::var("YZC_TEST_TIMING").as_deref() == Ok("1") {
        eprintln!(
            "YZC_TEST_TIMING stage={stage} elapsed_us={}",
            started.elapsed().as_micros()
        );
    }
}

async fn run(
    events: &EventSender,
    mut input: impl AsyncBufRead + Unpin,
    shutdown_started: &mut Option<std::time::Instant>,
    parent_gone: impl Future<Output = ()>,
) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    let init = loop {
        match timeout_at(deadline, admin::read(&mut input)).await {
            Ok(Ok(Some(command))) => match command.body {
                CommandBody::Init(init) => break init,
                CommandBody::Shutdown => return true,
                other => {
                    let code = if matches!(other, CommandBody::Unknown) {
                        "unknown-type"
                    } else {
                        "not-initialized"
                    };
                    if !events.emit(Event::error(code)).await {
                        return false;
                    }
                }
            },
            Ok(Ok(None)) => return false,
            Ok(Err(code)) => {
                if !events.emit(Event::error(code)).await || code == "invalid-frame" {
                    return false;
                }
            }
            Err(_) => {
                events.emit(Event::error("init-timeout")).await;
                return false;
            }
        }
    };
    if init.listeners.lan.is_some() {
        events.emit(Event::error("lan-not-supported")).await;
        return false;
    }
    let gateway = match Gateway::new(&init, events.clone(), Clock::default()) {
        Ok(gateway) => gateway,
        Err(code) => {
            events.emit(Event::error(code)).await;
            return false;
        }
    };
    let mut server =
        match Server::bind(&init.listeners.loopback, &init.web_root, gateway.clone()).await {
            Ok(server) => server,
            Err(code) => {
                events.emit(Event::error(code)).await;
                return false;
            }
        };
    let Ok(port) = server.port() else {
        return false;
    };
    // Construction pins the selected binary but performs no Herdr IO. Only an
    // authorized read RPC can request discovery; nothing starts a server.
    drop(init);
    let mut expiry = server.on_expiry();
    let (stop, stopped) = oneshot::channel();
    let mut tasks = JoinSet::new();
    tasks.spawn(server.run(stopped));
    let mut success = events.emit(Event::ready(port)).await;
    let mut expiry_announced = false;
    tokio::pin!(parent_gone);
    while success {
        tokio::select! {
            biased;
            _ = events.closed() => success = false,
            // From here the server only holds its port, for the desktop to unserve
            // the node: it ends at `shutdown`, stdin EOF, its own hold deadline or a gone
            // parent (after GONE_GRACE).
            notice = &mut expiry, if !expiry_announced => {
                expiry_announced = true;
                if notice.is_ok() {
                    success = events.emit(Event::error("policy-expired")).await;
                }
            }
            // Stdin EOF without the EOF: the desktop is gone but a leaked copy of the
            // pipe keeps it open. One future polled by `&mut`, never a ticker arm: an
            // arm that completes drops `admin::read`, and a half-read frame is lost.
            _ = &mut parent_gone => {
                tokio::time::sleep(parent::GONE_GRACE).await;
                break;
            }
            result = tasks.join_next() => {
                if !matches!(result, Some(Ok(true))) {
                    eprintln!("Companion listener stopped unexpectedly");
                }
                return false;
            }
            command = admin::read(&mut input) => match command {
                Ok(None) => break,
                Ok(Some(command)) => match command.body {
                    CommandBody::Shutdown => break,
                    CommandBody::Init(_) => success = events.emit(Event::error("already-initialized")).await,
                    CommandBody::Unknown => success = events.emit(Event::error("unknown-type")).await,
                    command => {
                        match gateway.command(command) {
                            Ok(result) => if let Some(event) = result.deferred_event() {
                                success = events.emit(event).await;
                            },
                            Err((status, _)) => {
                                let code = if status == axum::http::StatusCode::SERVICE_UNAVAILABLE { "admin-queue-unavailable" } else { "admin-command-rejected" };
                                success = events.emit(Event::error(code)).await;
                            }
                        }
                    }
                },
                Err(code) => {
                    success = events.emit(Event::error(code)).await;
                    if code == "invalid-frame" { success = false; }
                }
            }
        }
    }
    let started = std::time::Instant::now();
    *shutdown_started = Some(started);
    timing("server-join-start", started);
    let _ = stop.send(());
    if !matches!(
        timeout(Duration::from_secs(1), tasks.join_next()).await,
        Ok(Some(Ok(false)))
    ) {
        tasks.abort_all();
        success = false;
    }
    timing("server-join", started);
    success
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

    fn init(root: &std::path::Path) -> serde_json::Value {
        serde_json::json!({
            "v":1, "type":"init", "herdrBinary":root.join("DO-NOT-EXECUTE-HERDR"),
            "sessionAllowlist":[], "webRoot":root, "devices":[],
            "listeners":{"loopback":{"port":0,"publicHost":"desktop.tailnet.ts.net:8443","requiredTailscaleLogin":"user@example.com","proxySecret":"AQIDBAUGBwgJCgsMDQ4PEBESExQVFhcYGRobHB0eHyA"},"lan":null},
            "policy":{"enableUntil":"2099-01-01T00:00:00Z","viewIdleMin":30,"viewAbsoluteHours":12,"elevationMaxMin":15,"fullSessionEnabled":false,"maxDevices":5},
            "trustStore":root.join("workspace-trust.json"), "spaceRoots":[]
        })
    }

    #[tokio::test]
    async fn gateway_survives_slow_live_stdout_with_lifecycle_backlog() {
        let root = tempfile::tempdir().unwrap();
        let (output, input) = tokio::io::duplex(1);
        let (events, writer) = admin::spawn_writer(output);
        for _ in 0..4 {
            assert!(events.try_emit(Event::error("test-backlog")));
        }
        let (mut commands, command_input) = tokio::io::duplex(4096);
        let init = init(root.path());
        commands
            .write_all(format!("{init}\n").as_bytes())
            .await
            .unwrap();
        let task = tokio::spawn(async move {
            let input = BufReader::new(command_input);
            run(&events, input, &mut None, std::future::pending()).await
        });
        let reader = tokio::spawn(async move {
            let mut input = BufReader::new(input);
            for _ in 0..4 {
                tokio::time::sleep(Duration::from_millis(300)).await;
                let mut line = String::new();
                input.read_line(&mut line).await.unwrap();
                assert_eq!(
                    serde_json::from_str::<serde_json::Value>(&line).unwrap()["code"],
                    "test-backlog"
                );
            }
            let mut line = String::new();
            input.read_line(&mut line).await.unwrap();
            let ready: serde_json::Value = serde_json::from_str(&line).unwrap();
            assert_eq!(ready["type"], "ready");
            (ready["ports"]["loopback"].as_u64().unwrap() as u16, input)
        });
        tokio::time::sleep(Duration::from_millis(1100)).await;
        assert!(
            !task.is_finished(),
            "queue delay must not shut down gateway"
        );
        let (port, _input) = timeout(Duration::from_secs(2), reader)
            .await
            .unwrap()
            .unwrap();
        let _connection = tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
            .await
            .unwrap();
        commands
            .write_all(b"{\"v\":1,\"type\":\"shutdown\"}\n")
            .await
            .unwrap();
        assert!(timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap());
        writer.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn gateway_stops_when_http_event_breaks_stdout_while_stdin_idle() {
        let root = tempfile::tempdir().unwrap();
        let (output, input) = tokio::io::duplex(1);
        let (events, writer) = admin::spawn_writer(output);
        let http_events = events.clone();
        let (mut commands, command_input) = tokio::io::duplex(4096);
        commands
            .write_all(format!("{}\n", init(root.path())).as_bytes())
            .await
            .unwrap();
        let task = tokio::spawn(async move {
            let input = BufReader::new(command_input);
            run(&events, input, &mut None, std::future::pending()).await
        });
        let mut reader = BufReader::new(input);
        let mut ready = String::new();
        timeout(Duration::from_secs(2), reader.read_line(&mut ready))
            .await
            .unwrap()
            .unwrap();
        let ready: serde_json::Value = serde_json::from_str(&ready).unwrap();
        let port = ready["ports"]["loopback"].as_u64().unwrap() as u16;
        drop(reader);
        // No lifecycle ACK or further stdin command can wake supervision.
        assert!(http_events.try_emit(Event::error("http-event")));
        assert!(!timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap());
        assert!(writer.await.unwrap().is_err());
        assert!(
            tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
                .await
                .is_err()
        );
    }

    /// A gateway over in-memory pipes, already past `ready`.
    struct Running {
        commands: tokio::io::DuplexStream,
        events: BufReader<tokio::io::DuplexStream>,
        task: tokio::task::JoinHandle<bool>,
        port: u16,
        _root: tempfile::TempDir,
    }
    impl Running {
        async fn start(parent_gone: impl Future<Output = ()> + Send + 'static) -> Self {
            let root = tempfile::tempdir().unwrap();
            let (output, input) = tokio::io::duplex(64 * 1024);
            let (events, _writer) = admin::spawn_writer(output);
            let (mut commands, command_input) = tokio::io::duplex(64 * 1024);
            commands
                .write_all(format!("{}\n", init(root.path())).as_bytes())
                .await
                .unwrap();
            let task = tokio::spawn(async move {
                let input = BufReader::new(command_input);
                run(&events, input, &mut None, parent_gone).await
            });
            let mut running = Self {
                commands,
                events: BufReader::new(input),
                task,
                port: 0,
                _root: root,
            };
            let ready = running.event().await;
            assert_eq!(ready["type"], "ready");
            running.port = ready["ports"]["loopback"].as_u64().unwrap() as u16;
            running
        }
        async fn event(&mut self) -> serde_json::Value {
            let mut line = String::new();
            timeout(Duration::from_secs(5), self.events.read_line(&mut line))
                .await
                .expect("admin event deadline")
                .unwrap();
            serde_json::from_str(&line).unwrap()
        }
    }

    /// A root that is absolute on every platform ("/srv/x" is relative on Windows).
    fn absolute_root(name: &str) -> String {
        let root = std::env::temp_dir().join(name);
        assert!(root.is_absolute(), "{root:?} is not absolute here");
        root.to_str().unwrap().to_owned()
    }

    /// A `spaces.roots` command of about 20 KiB whose last root is `last`.
    fn roots_frame(last: &str) -> String {
        let filler = "a".repeat(100);
        let mut roots: Vec<_> = (0..120)
            .map(|n| serde_json::json!({"session":"s","workspaceId":format!("w{n}"),"root":absolute_root(&format!("{filler}{n}"))}))
            .collect();
        roots.push(serde_json::json!({"session":"s","workspaceId":"last","root":last}));
        let frame = serde_json::json!({"v":1,"type":"spaces.roots","roots":roots}).to_string();
        assert!(frame.len() > 2 * 8192);
        frame + "\n"
    }

    #[tokio::test]
    async fn parent_polls_never_cut_a_command_frame_in_two() {
        let probe: parent::Probe = Box::new(|| false);
        let mut running =
            Running::start(parent::watch(Some(probe), Duration::from_millis(20))).await;
        // Each frame arrives in two writes five poll intervals apart.
        for last in [absolute_root("last"), "relative".to_owned()] {
            let frame = roots_frame(&last);
            let (head, tail) = frame.split_at(frame.len() / 2);
            running.commands.write_all(head.as_bytes()).await.unwrap();
            tokio::time::sleep(Duration::from_millis(100)).await;
            running.commands.write_all(tail.as_bytes()).await.unwrap();
            if last == "relative" {
                // Judged as a whole: the relative last root is what rejects it.
                assert_eq!(running.event().await["code"], "admin-command-rejected");
            }
        }
        // Nothing else was emitted for either frame, `invalid-command` above all.
        running
            .commands
            .write_all(b"{\"v\":1,\"type\":\"future\"}\n")
            .await
            .unwrap();
        assert_eq!(running.event().await["code"], "unknown-type");
        running
            .commands
            .write_all(b"{\"v\":1,\"type\":\"shutdown\"}\n")
            .await
            .unwrap();
        assert!(timeout(Duration::from_secs(2), running.task)
            .await
            .unwrap()
            .unwrap());
    }

    #[tokio::test]
    async fn peers_list_is_answered_over_the_admin_channel_with_its_request_id() {
        let mut running = Running::start(std::future::pending()).await;
        running
            .commands
            .write_all(b"{\"v\":1,\"type\":\"peers.list\",\"requestId\":41}\n")
            .await
            .unwrap();
        let snapshot = running.event().await;
        assert_eq!(
            snapshot,
            serde_json::json!({"v":1,"type":"peers.snapshot","requestId":41,"peers":[],"streams":[]})
        );
        // An ack nobody waits for gets no reply and does not stop the gateway.
        running
            .commands
            .write_all(b"{\"v\":1,\"type\":\"push.ack\",\"requestId\":1,\"stored\":true}\n{\"v\":1,\"type\":\"peers.list\",\"requestId\":42}\n")
            .await
            .unwrap();
        assert_eq!(running.event().await["requestId"], 42);
        running
            .commands
            .write_all(b"{\"v\":1,\"type\":\"shutdown\"}\n")
            .await
            .unwrap();
        assert!(timeout(Duration::from_secs(2), running.task)
            .await
            .unwrap()
            .unwrap());
    }

    #[tokio::test]
    async fn gone_parent_stops_the_gateway_after_a_grace_while_stdin_stays_open() {
        let gone = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let seen = gone.clone();
        let probe: parent::Probe = Box::new(move || seen.load(std::sync::atomic::Ordering::SeqCst));
        let mut running =
            Running::start(parent::watch(Some(probe), Duration::from_millis(10))).await;
        let port = running.port;
        let _connection = tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!running.task.is_finished(), "a live parent stops nothing");
        let flipped = std::time::Instant::now();
        gone.store(true, std::sync::atomic::Ordering::SeqCst);
        // Stopped like stdin EOF (a clean `true`) with stdin still open, but only
        // after the grace that lets the tsnet sidecar unserve first.
        assert!(timeout(Duration::from_secs(3), &mut running.task)
            .await
            .unwrap()
            .unwrap());
        assert!(flipped.elapsed() >= parent::GONE_GRACE);
        assert!(
            tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
                .await
                .is_err()
        );
        drop(running.commands);
    }
}
