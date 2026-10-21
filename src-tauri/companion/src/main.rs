#![forbid(unsafe_code)]

use std::time::Duration;
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
        .block_on(run_main());
}

async fn run_main() {
    let (events, mut writer) = admin::spawn_writer(tokio::io::stdout());
    let mut shutdown_started = None;
    let mut success = run(
        &events,
        BufReader::new(tokio::io::stdin()),
        &mut shutdown_started,
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
    while success {
        tokio::select! {
            biased;
            _ = events.closed() => success = false,
            // From here the server only holds its port, for the desktop to unserve
            // the node: it ends at `shutdown`, stdin EOF or its own hold deadline.
            notice = &mut expiry, if !expiry_announced => {
                expiry_announced = true;
                if notice.is_ok() {
                    success = events.emit(Event::error("policy-expired")).await;
                }
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
        let task =
            tokio::spawn(
                async move { run(&events, BufReader::new(command_input), &mut None).await },
            );
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
        let task =
            tokio::spawn(
                async move { run(&events, BufReader::new(command_input), &mut None).await },
            );
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
}
