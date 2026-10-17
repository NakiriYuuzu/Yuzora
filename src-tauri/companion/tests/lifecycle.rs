use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpStream},
    process::{Child, Command, ExitStatus, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
use tempfile::TempDir;

struct Process {
    child: Child,
    stdout: mpsc::Receiver<String>,
    stderr: Option<thread::JoinHandle<String>>,
    root: TempDir,
    timing: bool,
}
impl Process {
    fn new() -> Self {
        Self::with_stdout_reader(true)
    }
    fn with_stdout_reader(read_stdout: bool) -> Self {
        Self::with_options(read_stdout, false)
    }
    fn with_timing() -> Self {
        Self::with_options(true, true)
    }
    fn with_options(read_stdout: bool, timing: bool) -> Self {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("index.html"), "shell").unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_yuzora-companion"));
        command.env_remove("YZC_TEST_TIMING");
        if timing {
            command.env("YZC_TEST_TIMING", "1");
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let (tx, stdout) = mpsc::channel();
        if read_stdout {
            let output = child.stdout.take().unwrap();
            thread::spawn(move || {
                for line in BufReader::new(output).lines() {
                    if tx.send(line.unwrap()).is_err() {
                        break;
                    }
                }
            });
        }
        let mut errors = child.stderr.take().unwrap();
        let stderr = Some(thread::spawn(move || {
            let mut s = String::new();
            errors.read_to_string(&mut s).unwrap();
            s
        }));
        Self {
            child,
            stdout,
            stderr,
            root,
            timing,
        }
    }
    fn init_value(&self) -> Value {
        json!({"v":1,"type":"init","herdrBinary":self.root.path().join("DO-NOT-EXECUTE-HERDR"),"sessionAllowlist":["stopped-session"],"webRoot":self.root.path(),
            "listeners":{"loopback":{"port":0,"publicHost":"desktop.tailnet.ts.net:8443","requiredTailscaleLogin":"user@example.com"},"lan":null},
            "devices":[],"policy":{"enableUntil":"2099-01-01T00:00:00Z","viewIdleMin":30,"viewAbsoluteHours":12,"elevationMaxMin":15,"fullSessionEnabled":false,"maxDevices":5}})
    }
    fn send(&mut self, value: &Value) {
        self.raw(&(serde_json::to_string(value).unwrap() + "\n"));
    }
    fn raw(&mut self, value: &str) {
        let stdin = self.child.stdin.as_mut().unwrap();
        stdin.write_all(value.as_bytes()).unwrap();
        stdin.flush().unwrap();
    }
    fn event(&self) -> Value {
        let line = self
            .stdout
            .recv_timeout(Duration::from_secs(8))
            .expect("admin event deadline");
        let event: Value = serde_json::from_str(&line).expect("stdout is exclusively NDJSON");
        assert_eq!(event["v"], 1);
        event
    }
    fn init(&mut self) -> u16 {
        self.send(&self.init_value());
        let event = self.event();
        assert_eq!(event["type"], "ready");
        event["ports"]["loopback"]
            .as_u64()
            .unwrap()
            .try_into()
            .unwrap()
    }
    fn exited(&mut self, within: Duration) -> ExitStatus {
        let deadline = Instant::now() + within;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                if self.timing {
                    let stderr = self.stderr();
                    eprintln!(
                        "lifecycle {} pid={} exit={status}:\n{stderr}",
                        thread::current().name().unwrap_or("test"),
                        self.child.id()
                    );
                    for stage in ["server-join", "writer-join", "total"] {
                        assert!(
                            stderr.contains(&format!("stage={stage} elapsed_us=")),
                            "missing {stage}: {stderr}"
                        );
                    }
                }
                return status;
            }
            if Instant::now() >= deadline {
                let _ = self.child.kill();
                let _ = self.child.wait();
                let stderr = self.stderr();
                panic!("process exit deadline ({within:?}); shutdown timing/stderr:\n{stderr}");
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
    fn stderr(&mut self) -> String {
        self.stderr.take().unwrap().join().unwrap()
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if thread::panicking() {
            if let Some(stderr) = self.stderr.take() {
                eprintln!(
                    "lifecycle failure shutdown timing/stderr:\n{}",
                    stderr.join().unwrap_or_default()
                );
            }
        }
    }
}
fn address(port: u16) -> SocketAddr {
    ([127, 0, 0, 1], port).into()
}
fn closed(port: u16, mut connection: TcpStream) {
    assert!(TcpStream::connect_timeout(&address(port), Duration::from_millis(200)).is_err());
    let mut b = [0];
    match connection.read(&mut b) {
        Ok(0) => {}
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted
            ) => {}
        result => panic!("connection survived shutdown: {result:?}"),
    }
}

#[test]
fn lifecycle_no_init_exits_nonzero_in_five_seconds() {
    let started = Instant::now();
    let mut p = Process::new();
    assert_eq!(p.event()["code"], "init-timeout");
    assert!(!p.exited(Duration::from_secs(2)).success());
    assert!(started.elapsed() >= Duration::from_secs(5));
    assert!(started.elapsed() < Duration::from_secs(8));
    assert!(p.stderr().is_empty());
}

#[test]
fn lifecycle_stdin_eof_closes_listener_and_connections() {
    let mut p = Process::with_timing();
    let port = p.init();
    let connection = TcpStream::connect(address(port)).unwrap();
    connection
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    drop(p.child.stdin.take());
    assert!(p.exited(Duration::from_secs(2)).success());
    closed(port, connection);
    assert!(p.stdout.try_iter().next().is_none());
}

#[test]
fn lifecycle_shutdown_closes_idle_and_partial_requests_in_two_seconds() {
    let mut p = Process::with_timing();
    let port = p.init();
    let mut connection = TcpStream::connect(address(port)).unwrap();
    connection
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    connection.write_all(b"GET / HTTP/1.1\r\n").unwrap();
    p.send(&json!({"v":1,"type":"shutdown"}));
    // Keep stdin OPEN: regression test for Tokio's blocking stdin shutdown trap.
    assert!(p.exited(Duration::from_secs(2)).success());
    closed(port, connection);
}

#[test]
fn admin_unknown_errors_preserve_channel_stdout_only_ndjson() {
    let mut p = Process::new();
    p.send(&json!({"v":1,"type":"future","keyPem":"SECRET-UNKNOWN"}));
    assert_eq!(p.event()["code"], "unknown-type");
    p.raw("{malformed SECRET-MALFORMED}\n");
    assert_eq!(p.event()["code"], "invalid-command");
    p.init();
    p.send(&json!({"v":1,"type":"audit"}));
    assert_eq!(p.event()["code"], "unknown-type");
    p.send(&p.init_value());
    assert_eq!(p.event()["code"], "already-initialized");
    p.send(&json!({"v":2,"type":"shutdown"}));
    assert_eq!(p.event()["code"], "unsupported-version");
    p.send(&json!({"v":1,"type":"shutdown"}));
    assert!(p.exited(Duration::from_secs(2)).success());
    assert!(p.stdout.try_iter().next().is_none());
    assert!(!p.stderr().contains("SECRET"));
}

#[test]
fn admin_pem_fields_rejected_without_logging_or_files() {
    for malformed in [false, true] {
        let mut p = Process::new();
        let mut init = p.init_value();
        init["listeners"]["lan"] = json!({"binds":["127.0.0.1"],"port":0,"publicHost":"desktop.local:47443","certPem":"SECRET-CERT-PEM\nline-two","keyPem":"SECRET-KEY-PEM\nline-two"});
        if malformed {
            init["policy"]["viewIdleMin"] = json!("SECRET-INVALID");
        }
        p.send(&init);
        let event = p.event();
        assert_eq!(
            event["code"],
            if malformed {
                "invalid-command"
            } else {
                "lan-not-supported"
            }
        );
        assert!(!event.to_string().contains("SECRET"));
        if malformed {
            p.send(&json!({"v":1,"type":"shutdown"}));
        }
        let status = p.exited(Duration::from_secs(2));
        assert_eq!(status.success(), malformed);
        assert!(!p.stderr().contains("SECRET"));
        assert_eq!(std::fs::read_dir(p.root.path()).unwrap().count(), 1);
    }
}

#[test]
fn lifecycle_port_in_use_is_admin_error_without_fallback() {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let mut p = Process::new();
    let mut init = p.init_value();
    init["listeners"]["loopback"]["port"] = json!(listener.local_addr().unwrap().port());
    p.send(&init);
    assert_eq!(p.event()["code"], "port-in-use");
    assert!(!p.exited(Duration::from_secs(2)).success());
    assert!(p.stdout.try_iter().next().is_none());
}

#[test]
fn lifecycle_broken_stdout_exits_and_closes_listener_with_fixed_stderr() {
    let mut p = Process::with_stdout_reader(false);
    p.send(&p.init_value());
    let mut line = String::new();
    BufReader::new(p.child.stdout.as_mut().unwrap())
        .read_line(&mut line)
        .unwrap();
    let event: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(event["type"], "ready");
    let port = event["ports"]["loopback"].as_u64().unwrap() as u16;
    let connection = TcpStream::connect(address(port)).unwrap();
    connection
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    drop(p.child.stdout.take());
    p.send(&json!({"v":1,"type":"future"}));
    // Leave stdin open: writer failure alone must shut down the gateway.
    assert!(!p.exited(Duration::from_secs(2)).success());
    closed(port, connection);
    assert_eq!(p.stderr(), "Companion admin stdout unavailable\n");
}

#[test]
fn policy_expiry_closes_listener_connections_emits_error_exits_nonzero() {
    let mut p = Process::new();
    let mut init = p.init_value();
    init["policy"]["enableUntil"] = json!((chrono::Utc::now() + chrono::Duration::seconds(2))
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true));
    p.send(&init);
    let ready = p.event();
    assert_eq!(ready["type"], "ready");
    let port = ready["ports"]["loopback"].as_u64().unwrap() as u16;
    let connection = TcpStream::connect(address(port)).unwrap();
    connection
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    assert_eq!(p.event()["code"], "policy-expired");
    assert!(!p.exited(Duration::from_secs(2)).success());
    closed(port, connection);
    assert!(p.stderr().is_empty());
}

#[test]
fn policy_invalid_and_expired_init_never_opens_listener() {
    for (field, value, expected) in [
        (
            "enableUntil",
            json!("2020-01-01T00:00:00Z"),
            "policy-expired",
        ),
        ("viewIdleMin", json!(31), "invalid-policy"),
    ] {
        let mut p = Process::new();
        let mut init = p.init_value();
        init["policy"][field] = value;
        p.send(&init);
        assert_eq!(p.event()["code"], expected);
        assert!(!p.exited(Duration::from_secs(2)).success());
        assert!(p.stdout.try_iter().next().is_none());
        assert!(p.stderr().is_empty());
    }
}

#[test]
fn pairing_admin_events_only_protocol_secrets_and_stderr_empty() {
    let mut p = Process::new();
    p.init();
    p.send(&json!({"v":1,"type":"pairing.create","path":"lan"}));
    assert_eq!(p.event()["code"], "admin-command-rejected");
    p.send(&json!({"v":1,"type":"pairing.create","path":"tailscale"}));
    let created = p.event();
    assert_eq!(created["type"], "pairing.created");
    assert!(created["url"].as_str().unwrap().contains("/pair#p="));
    p.send(&json!({"v":1,"type":"pairing.cancel","pairingId":created["pairingId"]}));
    let cancelled = p.event();
    assert_eq!(cancelled["reason"], "cancelled");
    assert!(cancelled.get("url").is_none());
    p.send(&json!({"v":1,"type":"shutdown"}));
    assert!(p.exited(Duration::from_secs(2)).success());
    assert!(p.stderr().is_empty());
}

#[test]
fn pairing_complete_protocol_never_leaks_tokens_tickets_or_sas_to_stderr() {
    fn post(port: u16, path: &str, body: Value) -> Value {
        let mut connection = TcpStream::connect(address(port)).unwrap();
        connection
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let text = body.to_string();
        write!(connection, "POST {path} HTTP/1.1\r\nHost: desktop.tailnet.ts.net:8443\r\nTailscale-User-Login: user@example.com\r\nOrigin: https://desktop.tailnet.ts.net:8443\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}", text.len()).unwrap();
        let mut response = String::new();
        connection.read_to_string(&mut response).unwrap();
        assert!(response.starts_with("HTTP/1.1 200"));
        serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap()
    }
    let mut p = Process::new();
    let port = p.init();
    p.send(&json!({"v":1,"type":"pairing.create","path":"tailscale"}));
    let created = p.event();
    let (id, secret) = created["url"]
        .as_str()
        .unwrap()
        .split_once("#p=")
        .unwrap()
        .1
        .split_once('.')
        .unwrap();
    let claim = post(
        port,
        "/api/pair/claim",
        json!({"pairingId":id,"secret":secret,"deviceLabel":"phone","deviceNonce":"BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc"}),
    );
    let claimed = p.event();
    assert_eq!(claimed["sas"], claim["sas"]);
    assert!(!claimed.to_string().contains(secret));
    assert!(!claimed
        .to_string()
        .contains(claim["claimToken"].as_str().unwrap()));
    p.send(&json!({"v":1,"type":"pairing.approve","pairingId":id}));
    let approved = p.event();
    assert_eq!(approved["state"], "approved");
    let status = post(
        port,
        "/api/pair/status",
        json!({"pairingId":id,"claimToken":claim["claimToken"]}),
    );
    assert_eq!(status["state"], "approved");
    for event in [&created, &claimed, &approved] {
        assert!(!event
            .to_string()
            .contains(status["registrationTicket"].as_str().unwrap()));
        assert!(!event
            .to_string()
            .contains(claim["claimToken"].as_str().unwrap()));
    }
    p.send(&json!({"v":1,"type":"shutdown"}));
    assert!(p.exited(Duration::from_secs(2)).success());
    assert!(p.stderr().is_empty());
}
