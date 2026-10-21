#[path = "support/identity.rs"]
mod identity;
#[path = "support/spawn.rs"]
mod spawn;
use identity::PROXY_SECRET;
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
    /// The shutdown timing lines of an exited timing process.
    stages: String,
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
        let mut child = spawn::serialized(
            command
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped()),
        )
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
            stages: String::new(),
        }
    }
    fn init_value(&self) -> Value {
        json!({"v":1,"type":"init","herdrBinary":self.root.path().join("DO-NOT-EXECUTE-HERDR"),"sessionAllowlist":["stopped-session"],"webRoot":self.root.path(),
            "listeners":{"loopback":{"port":0,"publicHost":"desktop.tailnet.ts.net:8443","requiredTailscaleLogin":"user@example.com","proxySecret":PROXY_SECRET},"lan":null},
            "devices":[],"policy":{"enableUntil":"2099-01-01T00:00:00Z","viewIdleMin":30,"viewAbsoluteHours":12,"elevationMaxMin":15,"fullSessionEnabled":false,"maxDevices":5},
            "trustStore":self.root.path().join("workspace-trust.json"),"spaceRoots":[]})
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
                    self.stages = stderr;
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
    /// How long main waited for the server task after the stop request.
    fn server_join(&self) -> Duration {
        let micros = self
            .stages
            .split("stage=server-join elapsed_us=")
            .nth(1)
            .and_then(|rest| rest.lines().next())
            .and_then(|value| value.parse().ok())
            .expect("server-join timing");
        Duration::from_micros(micros)
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
/// The peer's side of a connection the gateway closed: end of stream or a reset.
fn ended(mut connection: TcpStream) {
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
fn closed(port: u16, connection: TcpStream) {
    assert!(TcpStream::connect_timeout(&address(port), Duration::from_millis(200)).is_err());
    ended(connection);
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
fn lifecycle_invalid_proxy_secret_is_a_fixed_fatal_admin_error() {
    let mut other_bit = PROXY_SECRET.to_owned();
    other_bit.replace_range(42.., "B");
    for secret in [
        "".to_owned(),
        PROXY_SECRET[..42].to_owned(),
        format!("{PROXY_SECRET}A"),
        PROXY_SECRET.replace('Q', "+"),
        format!("{PROXY_SECRET}="),
        other_bit,
    ] {
        let mut p = Process::new();
        let mut init = p.init_value();
        init["listeners"]["loopback"]["proxySecret"] = json!(secret);
        p.send(&init);
        // Fixed event body: the rejected value is never echoed.
        assert_eq!(
            p.event(),
            json!({"v":1,"type":"error","code":"invalid-proxy-secret","message":"Companion request rejected"})
        );
        assert!(!p.exited(Duration::from_secs(2)).success());
        assert!(p.stderr().is_empty());
    }
}

#[test]
fn lifecycle_init_without_proxy_secret_is_rejected_then_recovers() {
    let mut p = Process::new();
    let mut init = p.init_value();
    init["listeners"]["loopback"]
        .as_object_mut()
        .unwrap()
        .remove("proxySecret");
    p.send(&init);
    assert_eq!(p.event()["code"], "invalid-command");
    // Same recovery as any other malformed init: the process keeps waiting.
    p.init();
    p.send(&json!({"v":1,"type":"shutdown"}));
    assert!(p.exited(Duration::from_secs(2)).success());
}

#[test]
fn lifecycle_invalid_trust_store_or_space_roots_are_fixed_fatal_admin_errors() {
    let space = |session: &str, workspace: &str, root: &str| json!({"session":session,"workspaceId":workspace,"root":root});
    let absolute = std::env::temp_dir().to_string_lossy().into_owned();
    for (field, value, code) in [
        (
            "trustStore",
            json!("relative/workspace-trust.json"),
            "invalid-trust-store",
        ),
        (
            "spaceRoots",
            json!([space("work", "w", "relative")]),
            "invalid-space-roots",
        ),
        (
            "spaceRoots",
            json!([
                space("stopped-session", "w", &absolute),
                space("stopped-session", "w", &absolute)
            ]),
            "invalid-space-roots",
        ),
    ] {
        let mut p = Process::new();
        let mut init = p.init_value();
        init[field] = value;
        p.send(&init);
        // Fixed event body: no path or entry is echoed.
        assert_eq!(
            p.event(),
            json!({"v":1,"type":"error","code":code,"message":"Companion request rejected"})
        );
        assert!(!p.exited(Duration::from_secs(2)).success());
        assert!(p.stderr().is_empty());
    }
}

#[test]
fn lifecycle_init_without_trust_store_or_space_roots_is_rejected_then_recovers() {
    let mut p = Process::new();
    for field in ["trustStore", "spaceRoots"] {
        let mut init = p.init_value();
        init.as_object_mut().unwrap().remove(field);
        p.send(&init);
        assert_eq!(p.event()["code"], "invalid-command", "{field}");
    }
    // Same recovery as any other malformed init: the process keeps waiting.
    p.init();
    p.send(&json!({"v":1,"type":"shutdown"}));
    assert!(p.exited(Duration::from_secs(2)).success());
}

#[test]
fn lifecycle_spaces_roots_applies_silently_and_a_rejected_one_keeps_the_process_up() {
    let mut p = Process::new();
    let mut init = p.init_value();
    let root = p.root.path().to_string_lossy().into_owned();
    // Entries outside the allowlist are dropped silently, also at init.
    init["spaceRoots"] = json!([
        {"session":"stopped-session","workspaceId":"w","root":root},
        {"session":"unlisted","workspaceId":"w","root":root},
    ]);
    p.send(&init);
    assert_eq!(p.event()["type"], "ready");
    p.send(&json!({"v":1,"type":"spaces.roots","roots":[
        {"session":"stopped-session","workspaceId":"x","root":root},
        {"session":"unlisted","workspaceId":"x","root":root},
    ]}));
    p.send(&json!({"v":1,"type":"spaces.roots","roots":[
        {"session":"stopped-session","workspaceId":"x","root":"relative/SECRET-ROOT"},
    ]}));
    // The applied command said nothing: the first event is the refusal.
    assert_eq!(
        p.event(),
        json!({"v":1,"type":"error","code":"admin-command-rejected","message":"Companion request rejected"})
    );
    p.send(&json!({"v":1,"type":"spaces.roots","roots":[{"extra":1}]}));
    assert_eq!(p.event()["code"], "invalid-command");
    p.send(&json!({"v":1,"type":"shutdown"}));
    assert!(p.exited(Duration::from_secs(2)).success());
    assert!(p.stdout.try_iter().next().is_none());
    assert!(!p.stderr().contains("SECRET"));
}

fn vapid() -> String {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    URL_SAFE_NO_PAD.encode([&[4u8][..], &[1u8; 64]].concat())
}

/// A valid endpoint id (any 32 bytes, unpadded base64url).
fn endpoint_id(n: u8) -> String {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    URL_SAFE_NO_PAD.encode([n; 32])
}
fn holding(device: &str, endpoints: &[u8]) -> Value {
    json!({"deviceId":device,"endpoints":endpoints.iter().map(|n| endpoint_id(*n)).collect::<Vec<_>>()})
}

#[test]
fn lifecycle_invalid_push_init_is_a_fixed_fatal_admin_error() {
    let many: Vec<_> = (0..65)
        .map(|n| holding(&format!("device-{n}"), &[1]))
        .collect();
    for push in [
        json!({"vapidPublicKey":"","devices":[]}),
        json!({"vapidPublicKey":"SECRET-KEY","devices":[]}),
        json!({"vapidPublicKey":vapid().replacen('B', "C", 1),"devices":[]}),
        json!({"vapidPublicKey":format!("{}=", vapid()),"devices":[]}),
        json!({"vapidPublicKey":vapid(),"devices":many}),
        json!({"vapidPublicKey":vapid(),"devices":[holding("device", &[])]}),
        json!({"vapidPublicKey":vapid(),"devices":[holding("device", &[1, 2, 3, 4, 5])]}),
        json!({"vapidPublicKey":vapid(),"devices":[holding("device", &[1]), holding("device", &[2])]}),
        json!({"vapidPublicKey":vapid(),"devices":[{"deviceId":"device","endpoints":["https://fcm.googleapis.com/SECRET-ENDPOINT"]}]}),
    ] {
        let mut p = Process::new();
        let mut init = p.init_value();
        init["push"] = push;
        p.send(&init);
        // Fixed event body: the rejected value is never echoed.
        assert_eq!(
            p.event(),
            json!({"v":1,"type":"error","code":"invalid-push","message":"Companion request rejected"})
        );
        assert!(!p.exited(Duration::from_secs(2)).success());
        assert!(p.stderr().is_empty());
    }
}

#[test]
fn lifecycle_push_devices_applies_silently_and_is_refused_without_push() {
    let mut p = Process::new();
    let mut init = p.init_value();
    init["push"] = json!({"vapidPublicKey":vapid(),"devices":[holding("unknown", &[1, 2])]});
    p.send(&init);
    assert_eq!(p.event()["type"], "ready");
    p.send(&json!({"v":1,"type":"push.devices","devices":[holding("unknown", &[3])]}));
    p.send(&json!({"v":1,"type":"push.devices","devices":[]}));
    // The applied commands said nothing: the first event is the later one.
    p.send(&json!({"v":1,"type":"future.command"}));
    assert_eq!(p.event()["code"], "unknown-type");
    // Over a limit: refused, and the process carries on.
    p.send(&json!({"v":1,"type":"push.devices","devices":[holding("unknown", &[1, 2, 3, 4, 5])]}));
    assert_eq!(
        p.event(),
        json!({"v":1,"type":"error","code":"admin-command-rejected","message":"Companion request rejected"})
    );
    p.send(&json!({"v":1,"type":"push.devices","deviceIds":[]}));
    assert_eq!(p.event()["code"], "invalid-command");
    p.send(&json!({"v":1,"type":"push.devices","devices":[1]}));
    assert_eq!(p.event()["code"], "invalid-command");
    p.send(&json!({"v":1,"type":"shutdown"}));
    assert!(p.exited(Duration::from_secs(2)).success());
    assert!(p.stdout.try_iter().next().is_none());

    let mut p = Process::new();
    p.init();
    p.send(&json!({"v":1,"type":"push.devices","devices":[]}));
    assert_eq!(
        p.event(),
        json!({"v":1,"type":"error","code":"admin-command-rejected","message":"Companion request rejected"})
    );
    p.send(&json!({"v":1,"type":"shutdown"}));
    assert!(p.exited(Duration::from_secs(2)).success());
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
    spawn::close(p.child.stdout.take());
    p.send(&json!({"v":1,"type":"future"}));
    // Leave stdin open: writer failure alone must shut down the gateway.
    assert!(!p.exited(Duration::from_secs(2)).success());
    closed(port, connection);
    assert_eq!(p.stderr(), "Companion admin stdout unavailable\n");
}

/// A gateway whose policy ends in two seconds, with one peer connected.
fn expiring(timing: bool) -> (Process, u16, TcpStream) {
    let mut p = Process::with_options(true, timing);
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
    (p, port, connection)
}
fn bind(port: u16) -> std::io::Result<std::net::TcpListener> {
    std::net::TcpListener::bind(address(port))
}
/// The held port: bound (nobody else can take it), and a new connection is
/// accepted and closed at once, never left waiting and never refused.
fn held(port: u16) {
    assert_eq!(
        bind(port).err().map(|error| error.kind()),
        Some(std::io::ErrorKind::AddrInUse),
        "the port was released"
    );
    let fresh = TcpStream::connect_timeout(&address(port), Duration::from_millis(500))
        .expect("a held port accepts");
    fresh
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    ended(fresh);
}
fn released(port: u16) {
    for _ in 0..50 {
        if bind(port).is_ok() {
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }
    panic!("the port was never released");
}

#[test]
fn policy_expiry_announces_first_keeps_the_port_bound_and_exits_at_shutdown() {
    let (mut p, port, connection) = expiring(true);
    assert_eq!(p.event()["code"], "policy-expired");
    let announced = Instant::now();
    // The event is the desktop's first signal: the port is still ours, and the
    // peers that were connected are gone.
    held(port);
    ended(connection);
    p.send(&json!({"v":1,"type":"shutdown"}));
    assert!(!p.exited(Duration::from_secs(3)).success());
    // Ended by the command, not by the five second hold, and the server task
    // stopped by itself rather than at main's one second abort.
    assert!(announced.elapsed() < Duration::from_millis(4500));
    assert!(p.server_join() < Duration::from_millis(900));
    released(port);
}

#[test]
fn policy_expiry_hold_ends_by_itself_after_five_seconds() {
    let (mut p, port, connection) = expiring(false);
    assert_eq!(p.event()["code"], "policy-expired");
    let announced = Instant::now();
    held(port);
    thread::sleep(Duration::from_secs(2));
    held(port);
    assert!(!p.exited(Duration::from_secs(6)).success());
    let held_for = announced.elapsed();
    assert!(
        held_for >= Duration::from_secs(4),
        "released after {held_for:?}"
    );
    released(port);
    ended(connection);
    assert!(p.stderr().is_empty());
}

#[test]
fn policy_expiry_hold_ends_at_stdin_eof() {
    let (mut p, port, _connection) = expiring(true);
    assert_eq!(p.event()["code"], "policy-expired");
    let announced = Instant::now();
    held(port);
    drop(p.child.stdin.take());
    assert!(!p.exited(Duration::from_secs(3)).success());
    assert!(announced.elapsed() < Duration::from_millis(4500));
    assert!(p.server_join() < Duration::from_millis(900));
    released(port);
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
        write!(connection, "POST {path} HTTP/1.1\r\nHost: desktop.tailnet.ts.net:8443\r\nYzc-Proxy-Secret: {PROXY_SECRET}\r\nYzc-Peer-Login: user@example.com\r\nOrigin: https://desktop.tailnet.ts.net:8443\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}", text.len()).unwrap();
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
