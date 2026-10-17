use super::*;

#[test]
fn bridge_scope_send_keys_closed_mapping() {
    assert_eq!(
        mapped_keys(vec!["Enter".into(), "BackTab".into(), "Ctrl-C".into()]).unwrap(),
        vec!["enter", "shift+tab", "ctrl+c"]
    );
    for key in ["a", "Ctrl-B", "ctrl+c", "F1", "Enter\n", "Escape;quit"] {
        assert_eq!(
            mapped_keys(vec!["Enter".into(), key.into()]),
            Err("key-not-allowed")
        );
    }
    assert!(mapped_keys(Vec::new()).is_err());
    assert!(mapped_keys(vec!["Enter".into(); 33]).is_err());
}
#[test]
fn bridge_scope_full_resize_requires_confirmation_and_bounds() {
    assert_eq!(full_size(44, 30, false), Err("size-confirmation-required"));
    assert_eq!(full_size(44, 30, true), Ok((44, 30)));
    for (cols, rows) in [(19, 30), (401, 30), (44, 7), (44, 201)] {
        assert_eq!(full_size(cols, rows, true), Err("invalid-params"));
    }
}
#[test]
fn bridge_scope_input_bound_and_exactly_one_format() {
    assert_eq!(
        input_bytes(Input {
            stream: 1,
            text: Some("x".repeat(4097)),
            bytes_b64: None
        }),
        Err("payload-too-large")
    );
    assert_eq!(
        input_bytes(Input {
            stream: 1,
            text: Some("x".into()),
            bytes_b64: Some("eA==".into())
        }),
        Err("invalid-params")
    );
    assert_eq!(
        input_bytes(Input {
            stream: 1,
            text: None,
            bytes_b64: Some("eA==".into())
        }),
        Ok((1, vec![b'x']))
    );
}
#[test]
fn bridge_scope_keys_require_snapshot_agent_pane() {
    let snapshot = json!({"panes":[{"pane_id":"w:p","terminal_id":"term"}],"agents":[]});
    assert_eq!(agent_target(&snapshot, "w:p"), Err("no-agent"));
    assert_eq!(agent_target(&snapshot, "another"), Err("target-not-found"));
    let mut snapshot = snapshot;
    snapshot["agents"] = json!([{"pane_id":"w:p"}]);
    assert_eq!(agent_target(&snapshot, "term"), Ok("w:p".into()));
}

use crate::{
    admin::{self, Scope},
    clock::Clock,
    herdr::{HerdrPort, Resource},
    http::Server,
    session::AuthContext,
    test_support::authenticator::Authenticator,
};
use futures_util::{SinkExt, StreamExt};
use std::sync::atomic::AtomicUsize;
use tokio::{net::TcpStream, sync::oneshot, task::JoinHandle};
use tokio_tungstenite::{tungstenite::client::IntoClientRequest, WebSocketStream};
use yuzora_host::herdr_service::{
    HerdrNamedSession, HerdrSnapshotResult, OnSubscriptionEvent, OnTerminalEvent,
};
const DEVICE: &str = "AAAAAAAAAAAAAAAAAAAAAA";
#[derive(Default)]
struct Fake {
    records: Mutex<Vec<String>>,
    terminals: Mutex<HashMap<String, OnTerminalEvent>>,
    next: AtomicUsize,
    blocked: AtomicBool,
    release_blocked: AtomicBool,
    conflict: AtomicBool,
    open_error: AtomicBool,
    open_blocked: AtomicBool,
    snapshot_blocked: AtomicBool,
    before_open_returns: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}
impl Fake {
    fn record(&self, v: String) {
        self.records.lock().unwrap().push(v);
    }
    fn has(&self, prefix: &str) -> bool {
        self.records
            .lock()
            .unwrap()
            .iter()
            .any(|s| s.starts_with(prefix))
    }
    fn open(&self, kind: &str, sink: OnTerminalEvent) -> herdr::Result<String> {
        while self.open_blocked.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(1));
        }
        let id = format!("{kind}-{}", self.next.fetch_add(1, Ordering::SeqCst));
        self.terminals.lock().unwrap().insert(id.clone(), sink);
        Ok(id)
    }
}
impl HerdrPort for Fake {
    fn sessions(&self) -> herdr::Result<Vec<HerdrNamedSession>> {
        Ok(vec![HerdrNamedSession {
            name: "work".into(),
            running: true,
            default: true,
            session_dir: "private".into(),
            socket_path: "private".into(),
        }])
    }
    fn snapshot(&self, _: &str) -> herdr::Result<HerdrSnapshotResult> {
        self.record("snapshot".into());
        while self.snapshot_blocked.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(1));
        }
        Ok(HerdrSnapshotResult {
            version: "0.9.1".into(),
            protocol: 22,
            snapshot: json!({"panes":[{"pane_id":"w:p","terminal_id":"term"}],"agents":[{"pane_id":"w:p","agent":"fake"}],"layouts":[{"panes":[{"pane_id":"w:p","rect":{"width":120,"height":40}}]}]}),
        })
    }
    fn observe(
        &self,
        _: &str,
        _: &str,
        _: u16,
        _: u16,
        s: OnTerminalEvent,
    ) -> herdr::Result<String> {
        self.open("observe", s)
    }
    fn subscribe(&self, _: &str, _: OnSubscriptionEvent) -> herdr::Result<String> {
        panic!("unexpected subscribe")
    }
    fn release(&self, r: Resource) {
        let Resource::Observe(id) = r else {
            panic!("unexpected subscription")
        };
        self.record(format!("release:{id}"));
        while self.release_blocked.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(1));
        }
        self.terminals.lock().unwrap().remove(&id);
    }
    fn control(
        &self,
        session: &str,
        target: &str,
        size: (u16, u16),
        takeover: bool,
        s: OnTerminalEvent,
    ) -> herdr::Result<String> {
        self.record(format!("control:{session}:{target}:{size:?}:{takeover}"));
        if self.open_error.load(Ordering::SeqCst) {
            return Err("herdr-unavailable");
        }
        if let Some(hook) = self.before_open_returns.lock().unwrap().take() {
            hook();
        }
        if self.conflict.load(Ordering::SeqCst) && !takeover {
            s(HerdrTerminalEvent::Closed{session_id:"fake".into(),reason:Some("terminal attach failed: terminal fake already has an attached client; retry with --takeover".into())}).unwrap();
        }
        self.open("control", s)
    }
    fn full(&self, session: &str, size: (u16, u16), s: OnTerminalEvent) -> herdr::Result<String> {
        self.record(format!("full:{session}:{size:?}"));
        if self.open_error.load(Ordering::SeqCst) {
            return Err("herdr-unavailable");
        }
        self.open("full", s)
    }
    fn input(&self, id: &str, bytes: Vec<u8>) -> herdr::Result<()> {
        self.record(format!("input:{id}:{}", B64.encode(bytes)));
        while self.blocked.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(1));
        }
        Ok(())
    }
    fn scroll(&self, _: &str, _: HerdrScrollDirection, _: u32) -> herdr::Result<()> {
        self.record("scroll".into());
        while self.blocked.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(1));
        }
        Ok(())
    }
    fn resize(&self, id: &str, size: (u16, u16)) -> herdr::Result<()> {
        self.record(format!("resize:{id}:{size:?}"));
        while self.blocked.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(1));
        }
        Ok(())
    }
    fn prompt(&self, session: &str, target: &str, text: String) -> herdr::Result<()> {
        self.record(format!("prompt:{session}:{target}:{}", text.len()));
        while self.blocked.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(1));
        }
        Ok(())
    }
    fn keys(&self, session: &str, target: &str, keys: Vec<String>) -> herdr::Result<()> {
        self.record(format!("keys:{session}:{target}:{keys:?}"));
        while self.blocked.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(1));
        }
        Ok(())
    }
}
struct Capture(Arc<Mutex<Vec<u8>>>);
impl tokio::io::AsyncWrite for Capture {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
        bytes: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        self.0.lock().unwrap().extend(bytes);
        std::task::Poll::Ready(Ok(bytes.len()))
    }
    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }
    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }
}
struct Fixture {
    gateway: Arc<Gateway>,
    fake: Arc<Fake>,
    ctx: AuthContext,
    cookie: String,
    binding: String,
    port: u16,
    auth: Authenticator,
    count: u32,
    events: Arc<Mutex<Vec<u8>>>,
    writer: JoinHandle<std::io::Result<()>>,
    server: JoinHandle<bool>,
    _stop: oneshot::Sender<()>,
    _root: tempfile::TempDir,
}
async fn io<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::pin!(future);
    for _ in 0..200000 {
        tokio::select! {biased;v=&mut future=>return v,_=tokio::task::yield_now()=>{}}
    }
    panic!("fake loopback deadline")
}
impl Fixture {
    async fn new() -> Self {
        let auth = Authenticator::new("ES256", 7, false);
        let rp = crate::webauthn::RelyingParty::new("desktop.tailnet.ts.net:8443").unwrap();
        let (state, options) = rp.registration(DEVICE, "Phone", &[], true).unwrap();
        let (_, record) = rp
            .verify_registration(
                state,
                auth.response(&options, true, 10, "valid"),
                admin::PasskeyBinding::O2,
            )
            .unwrap()
            .into_parts();
        let mut init = crate::test_support::Fixture::init();
        let mut device: crate::devices::Device =
            serde_json::from_str(include_str!("../tests/fixtures/device.json")).unwrap();
        device.credentials = vec![record];
        device.grants.full = true;
        init.devices = vec![device];
        init.policy.full_session_enabled = true;
        init.session_allowlist = vec!["work".into()];
        let events = Arc::new(Mutex::new(Vec::new()));
        let (sender, writer) = admin::spawn_writer(Capture(events.clone()));
        let fake = Arc::new(Fake::default());
        let clock = Clock::at(serde_json::from_str("\"2030-01-01T00:00:00Z\"").unwrap());
        let gateway = Gateway::with_herdr(&init, sender, clock, fake.clone()).unwrap();
        let (id, binding) = gateway
            .identity
            .lock()
            .unwrap()
            .create_for_test(DEVICE, "desktop.tailnet.ts.net")
            .unwrap();
        let cookie = crate::session::session_cookie(&id);
        let mut headers = axum::http::HeaderMap::new();
        headers.insert("cookie", cookie.parse().unwrap());
        headers.insert("x-yzc-binding", binding.parse().unwrap());
        let ctx = gateway.authenticate(&headers, Scope::View).unwrap();
        let root = tempfile::tempdir().unwrap();
        let server = Server::bind(&init.listeners.loopback, root.path(), gateway.clone())
            .await
            .unwrap();
        let port = server.port().unwrap();
        let (stop, stopped) = oneshot::channel();
        Self {
            gateway,
            fake,
            ctx,
            cookie,
            binding,
            port,
            auth,
            count: 10,
            events,
            writer,
            server: tokio::spawn(server.run(stopped)),
            _stop: stop,
            _root: root,
        }
    }
    async fn elevate(&mut self, scope: &str) {
        let headers = format!(
            "Cookie: {}\r\nX-Yzc-Binding: {}\r\n",
            self.cookie, self.binding
        );
        let (status, mut options, _) = crate::test_support::post(
            self.port,
            "/api/auth/elevate/options",
            json!({"scope":scope}),
            &headers,
        )
        .await;
        assert_eq!(status, 200, "{options}");
        options["user"] = json!({"id":DEVICE});
        self.count += 1;
        let credential = self.auth.response(&options, false, self.count, "valid");
        let (status, body, _) = crate::test_support::post(
            self.port,
            "/api/auth/elevate/finish",
            json!({"scope":scope,"credential":credential}),
            &headers,
        )
        .await;
        assert_eq!(status, 200, "{body}");
    }
    async fn connect(&self) -> WebSocketStream<TcpStream> {
        let mut request = "ws://desktop.tailnet.ts.net:8443/ws"
            .into_client_request()
            .unwrap();
        for (k, v) in [
            ("origin", "https://desktop.tailnet.ts.net:8443"),
            ("tailscale-user-login", "user@example.com"),
            ("cookie", &self.cookie),
        ] {
            request.headers_mut().insert(k, v.parse().unwrap());
        }
        let (mut ws, _) = io(tokio_tungstenite::client_async(
            request,
            io(TcpStream::connect((
                std::net::Ipv4Addr::LOCALHOST,
                self.port,
            )))
            .await
            .unwrap(),
        ))
        .await
        .unwrap();
        io(ws.send(Message::text(
            json!({"type":"auth","binding":self.binding}).to_string(),
        )))
        .await
        .unwrap();
        assert_eq!(value(&mut ws).await["event"], "session.ready");
        ws
    }
    fn revoke(&self) {
        self.gateway
            .command(admin::CommandBody::Device(admin::DeviceCommand::Revoke {
                v: 1,
                device_id: DEVICE.into(),
            }))
            .unwrap();
    }
    fn audits(&self) -> Vec<Value> {
        String::from_utf8(self.events.lock().unwrap().clone())
            .unwrap()
            .lines()
            .filter_map(|s| serde_json::from_str(s).ok())
            .collect()
    }
    async fn released(&self) {
        let drain = self.gateway.herdr_jobs.drain();
        tokio::pin!(drain);
        io(async{loop{tokio::select!{biased;_=&mut drain=>break,_=tokio::task::yield_now()=>{tokio::time::advance(Duration::from_millis(5)).await;}}}}).await;
        assert!(self.fake.terminals.lock().unwrap().is_empty());
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.fake.blocked.store(false, Ordering::SeqCst);
        self.fake.open_blocked.store(false, Ordering::SeqCst);
        self.fake.release_blocked.store(false, Ordering::SeqCst);
        self.fake.snapshot_blocked.store(false, Ordering::SeqCst);
        self.server.abort();
        self.writer.abort();
    }
}
async fn value(ws: &mut WebSocketStream<TcpStream>) -> Value {
    loop {
        match io(ws.next()).await.unwrap().unwrap() {
            Message::Text(s) => return serde_json::from_str(&s).unwrap(),
            Message::Ping(_) => {
                io(ws.flush()).await.unwrap();
            }
            other => panic!("expected JSON {other:?}"),
        }
    }
}
async fn rpc(ws: &mut WebSocketStream<TcpStream>, method: &str, params: Value) -> Value {
    io(ws.send(Message::text(
        json!({"id":1,"method":method,"params":params}).to_string(),
    )))
    .await
    .unwrap();
    value(ws).await
}
async fn control(ws: &mut WebSocketStream<TcpStream>) -> u32 {
    let r = rpc(
        ws,
        "terminal.control",
        json!({"session":"work","target":"w:p","confirmResize":true,"cols":1,"rows":"ignored"}),
    )
    .await;
    assert!(r["result"].is_object(), "{r}");
    r["result"]["stream"].as_u64().unwrap() as u32
}
async fn advance_pong(ws: &mut WebSocketStream<TcpStream>, seconds: u64) {
    tokio::time::advance(Duration::from_secs(seconds)).await;
    assert!(matches!(
        io(ws.next()).await.unwrap().unwrap(),
        Message::Ping(_)
    ));
    io(ws.flush()).await.unwrap();
    for _ in 0..100 {
        tokio::task::yield_now().await;
    }
}
async fn full(ws: &mut WebSocketStream<TcpStream>) -> Value {
    rpc(
        ws,
        "full.open",
        json!({"session":"work","cols":44,"rows":30,"confirmResize":true}),
    )
    .await
}

#[tokio::test(start_paused = true)]
async fn bridge_write_control_confirmation_conflict_takeover_freshness_taken_over() {
    let mut f = Fixture::new().await;
    f.elevate("control").await;
    let mut ws = f.connect().await;
    assert_eq!(
        rpc(
            &mut ws,
            "terminal.control",
            json!({"session":"work","target":"w:p"})
        )
        .await["error"]["code"],
        "size-confirmation-required"
    );
    assert!(!f.fake.has("control:"));
    f.fake.conflict.store(true, Ordering::SeqCst);
    assert_eq!(
        rpc(
            &mut ws,
            "terminal.control",
            json!({"session":"work","target":"w:p","confirmResize":true})
        )
        .await["error"]["code"],
        "owned-elsewhere"
    );
    // A conflict can also deliver stream.closed; drain it before the next RPC.
    assert_eq!(value(&mut ws).await["data"]["reason"], "owned-elsewhere");
    tokio::time::advance(Duration::from_secs(121)).await;
    assert_eq!(
        rpc(
            &mut ws,
            "terminal.control",
            json!({"session":"work","target":"w:p","confirmResize":true,"takeover":true})
        )
        .await["error"]["code"],
        "fresh-elevation-required"
    );
    f.elevate("control").await;
    let r = rpc(
        &mut ws,
        "terminal.control",
        json!({"session":"work","target":"w:p","confirmResize":true,"takeover":true}),
    )
    .await;
    assert!(r["result"].is_object(), "{r}");
    assert!(f.fake.has("control:work:w:p:(120, 40):true"));
    let sink = f
        .fake
        .terminals
        .lock()
        .unwrap()
        .values()
        .next()
        .unwrap()
        .clone();
    sink(HerdrTerminalEvent::Closed {
        session_id: "fake".into(),
        reason: Some("terminal attach taken over".into()),
    })
    .unwrap();
    assert_eq!(value(&mut ws).await["data"]["reason"], "taken-over");
    f.released().await;
    assert!(f
        .audits()
        .iter()
        .any(|a| a["type"] == "control.state" && a["takeover"] == true));
}
#[tokio::test(start_paused = true)]
async fn bridge_write_rpc_binary_ownership_and_scope() {
    let mut f = Fixture::new().await;
    f.elevate("control").await;
    let mut ws = f.connect().await;
    let s = control(&mut ws).await;
    assert_eq!(
        rpc(
            &mut ws,
            "terminal.input",
            json!({"stream":s,"text":"password"})
        )
        .await["result"]["bytes"],
        8
    );
    io(ws.send(Message::binary(
        [s.to_be_bytes().as_slice(), b"abc"].concat(),
    )))
    .await
    .unwrap();
    assert_eq!(value(&mut ws).await["result"]["bytes"], 3);
    let mut other = f.connect().await;
    io(other.send(Message::binary(
        [s.to_be_bytes().as_slice(), b"bad"].concat(),
    )))
    .await
    .unwrap();
    assert!(
        matches!(io(other.next()).await.unwrap().unwrap(),Message::Close(Some(c)) if u16::from(c.code)==1003)
    );
    assert_eq!(
        rpc(&mut ws, "full.input", json!({"stream":s,"text":"bad"})).await["error"]["code"],
        "elevation-required"
    );
    assert_eq!(
        rpc(&mut ws, "terminal.release", json!({"stream":s})).await["result"]["closed"],
        true
    );
    f.released().await;
    assert!(!f
        .audits()
        .iter()
        .any(|a| a.to_string().contains("password")));
    assert!(f
        .audits()
        .iter()
        .any(|a| a["action"] == "terminal.input" && a["summary"]["bytes"] == 11));
}
#[tokio::test(start_paused = true)]
async fn bridge_write_revoke_queued_input_dropped_before_host_no_replay() {
    let mut f = Fixture::new().await;
    f.elevate("control").await;
    let mut ws = f.connect().await;
    let s = control(&mut ws).await;
    f.fake.blocked.store(true, Ordering::SeqCst);
    io(ws.send(Message::text(
        json!({"id":2,"method":"terminal.scroll","params":{"stream":s,"direction":"up","lines":1}})
            .to_string(),
    )))
    .await
    .unwrap();
    io(async {
        while !f.fake.has("scroll") {
            tokio::task::yield_now().await;
        }
    })
    .await;
    io(ws.send(Message::text(
        json!({"id":3,"method":"terminal.input","params":{"stream":s,"text":"MUST-NOT-WRITE"}})
            .to_string(),
    )))
    .await
    .unwrap();
    assert!(rpc(&mut ws, "session.ping", json!({})).await["result"].is_object());
    f.revoke();
    let active = value(&mut ws).await;
    assert_eq!(active["id"], 2);
    assert_eq!(active["error"]["code"], "write-outcome-unknown");
    let dropped = value(&mut ws).await;
    assert_eq!(dropped["id"], 3);
    assert_eq!(dropped["error"]["code"], "input-dropped");
    assert!(
        matches!(io(ws.next()).await.unwrap().unwrap(),Message::Close(Some(c)) if u16::from(c.code)==4401)
    );
    f.fake.blocked.store(false, Ordering::SeqCst);
    f.released().await;
    assert!(!f.fake.has("input:"));
}
#[tokio::test(start_paused = true)]
async fn bridge_write_natural_expiry_releases_without_rpc() {
    let mut f = Fixture::new().await;
    f.elevate("control").await;
    let mut ws = f.connect().await;
    let s = control(&mut ws).await;
    for _ in 0..44 {
        tokio::time::advance(Duration::from_secs(20)).await;
        assert!(matches!(
            io(ws.next()).await.unwrap().unwrap(),
            Message::Ping(_)
        ));
        io(ws.flush()).await.unwrap();
        for _ in 0..50 {
            tokio::task::yield_now().await;
        }
    }
    tokio::time::advance(Duration::from_secs(20)).await;
    let closed = value(&mut ws).await;
    assert_eq!(
        closed["data"],
        json!({"stream":s,"reason":"elevation-expired"})
    );
    f.released().await;
    assert!(rpc(&mut ws, "session.ping", json!({})).await["result"].is_object());
}
#[tokio::test(start_paused = true)]
async fn bridge_write_full_freshness_validity_coordination_and_device_limit() {
    let mut f = Fixture::new().await;
    f.elevate("control").await;
    f.elevate("full").await;
    let mut ws = f.connect().await;
    let control = control(&mut ws).await;
    io(ws.send(Message::text(json!({"id":5,"method":"full.open","params":{"session":"work","cols":44,"rows":30,"confirmResize":true}}).to_string()))).await.unwrap();
    assert_eq!(
        value(&mut ws).await["data"],
        json!({"stream":control,"reason":"full-opened"})
    );
    let opened = value(&mut ws).await;
    let s = opened["result"]["stream"].as_u64().unwrap();
    let records = f.fake.records.lock().unwrap().clone();
    assert!(
        records
            .iter()
            .position(|s| s.starts_with("release:"))
            .unwrap()
            < records.iter().position(|s| s.starts_with("full:")).unwrap()
    );
    let mut other = f.connect().await;
    assert_eq!(full(&mut other).await["error"]["code"], "full-limit");
    drop(other);
    advance_pong(&mut ws, 121).await;
    assert_eq!(
        rpc(&mut ws, "full.input", json!({"stream":s,"text":"hi"})).await["result"]["bytes"],
        2
    );
    assert_eq!(
        full(&mut ws).await["error"]["code"],
        "fresh-elevation-required"
    );
    // Keep transport liveness distinct from the fixed Full token deadline.
    for _ in 0..38 {
        advance_pong(&mut ws, 20).await;
    }
    tokio::time::advance(Duration::from_secs(19)).await;
    assert_eq!(value(&mut ws).await["data"]["reason"], "elevation-expired");
    f.released().await;
    let events = f.audits();
    assert!(events
        .iter()
        .any(|a| a["type"] == "full.state" && a["open"] == true));
    assert!(events
        .iter()
        .any(|a| a["type"] == "full.state" && a["open"] == false));
}
#[tokio::test(start_paused = true)]
async fn audit_prompt_summary_keys_allowlist_and_input_aggregation() {
    let mut f = Fixture::new().await;
    f.elevate("control").await;
    let mut ws = f.connect().await;
    assert_eq!(
        rpc(
            &mut ws,
            "agent.prompt",
            json!({"session":"work","target":"term","text":"界".repeat(201)})
        )
        .await["result"]["sent"],
        true
    );
    assert_eq!(
        rpc(
            &mut ws,
            "agent.prompt",
            json!({"session":"work","target":"w:p","text":"x".repeat(16385)})
        )
        .await["error"]["code"],
        "payload-too-large"
    );
    assert_eq!(
        rpc(
            &mut ws,
            "pane.sendKeys",
            json!({"session":"work","target":"w:p","keys":["Enter","Ctrl-C"]})
        )
        .await["result"]["sent"],
        true
    );
    assert_eq!(
        rpc(
            &mut ws,
            "pane.sendKeys",
            json!({"session":"work","target":"w:p","keys":["Ctrl-B"]})
        )
        .await["error"]["code"],
        "key-not-allowed"
    );
    let s = control(&mut ws).await;
    for _ in 0..3 {
        assert_eq!(
            rpc(
                &mut ws,
                "terminal.input",
                json!({"stream":s,"text":"private"})
            )
            .await["result"]["bytes"],
            7
        );
    }
    assert!(!f.audits().iter().any(|a| a["action"] == "terminal.input"));
    tokio::time::advance(Duration::from_secs(10)).await;
    rpc(&mut ws, "session.ping", json!({})).await;
    let audits = f.audits();
    let prompt = audits
        .iter()
        .find(|a| a["action"] == "agent.prompt" && a["summary"]["prefix"].is_string())
        .unwrap();
    assert_eq!(
        prompt["summary"]["prefix"]
            .as_str()
            .unwrap()
            .chars()
            .count(),
        200
    );
    assert_eq!(prompt["summary"]["length"], 201);
    assert!(audits
        .iter()
        .any(|a| a["action"] == "terminal.input" && a["summary"]["bytes"] == 21));
    assert!(!audits.iter().any(|a| a.to_string().contains("private")));
}

async fn direct_control(f: &Fixture) -> Reads {
    let (_, invalidated) = f
        .gateway
        .with_authorized(&f.ctx, Scope::View, |i| i.register_peer(&f.ctx))
        .unwrap()
        .unwrap();
    let (_, stop) = tokio::sync::watch::channel(0);
    let (mut reads, _) = Reads::new(Arc::new(Semaphore::new(4 * 1024 * 1024)), invalidated, stop);
    assert!(reads
        .write_call(
            &f.gateway,
            &f.ctx,
            json!(1),
            "terminal.control",
            json!({"session":"work","target":"w:p","confirmResize":true})
        )
        .unwrap()
        .is_none());
    assert!(reads.pump_writes(&f.gateway).is_empty());
    let completed = io(reads.completed.next()).await.unwrap();
    assert!(reads.finish(completed)[0]["result"].is_object());
    reads
}
#[tokio::test(start_paused = true)]
async fn bridge_write_last_boundary_rechecks_revoke_and_stream_epoch() {
    for revoke in [false, true] {
        let mut f = Fixture::new().await;
        f.elevate("control").await;
        let mut reads = direct_control(&f).await;
        assert!(reads
            .write_call(
                &f.gateway,
                &f.ctx,
                json!(2),
                "terminal.input",
                json!({"stream":1,"text":"NEVER"})
            )
            .unwrap()
            .is_none());
        let work = reads.writes.receiver.try_recv().unwrap();
        if revoke {
            f.revoke();
        } else {
            reads.streams[&1].state.close("client-closed");
        }
        assert!(matches!((work.run)(), Err("input-dropped")));
        assert!(!f.fake.has("input:"));
        drop(reads);
        f.released().await;
    }
}
#[tokio::test(start_paused = true)]
async fn audit_full_queue_refuses_input_but_release_defers_and_retries() {
    let mut f = Fixture::new().await;
    f.elevate("control").await;
    let mut reads = direct_control(&f).await;
    reads
        .write_call(
            &f.gateway,
            &f.ctx,
            json!(0),
            "terminal.input",
            json!({"stream":1,"text":"first"}),
        )
        .unwrap();
    let first = reads.writes.receiver.try_recv().unwrap();
    assert!((first.run)().is_ok());
    f.fake.records.lock().unwrap().clear();
    while f.gateway.events.try_emit(admin::Event::error("filler")) {}
    reads
        .write_call(
            &f.gateway,
            &f.ctx,
            json!(2),
            "terminal.input",
            json!({"stream":1,"text":"NEVER"}),
        )
        .unwrap();
    let work = reads.writes.receiver.try_recv().unwrap();
    assert!(matches!((work.run)(), Err("audit-unavailable")));
    assert!(!f.fake.has("input:"));
    assert_eq!(
        reads
            .write_call(
                &f.gateway,
                &f.ctx,
                json!(3),
                "terminal.release",
                json!({"stream":1})
            )
            .unwrap()
            .unwrap()["result"]["closed"],
        true
    );
    f.released().await;
    for _ in 0..200 {
        tokio::task::yield_now().await;
    }
    f.gateway.maintenance();
    for _ in 0..200 {
        tokio::task::yield_now().await;
    }
    assert!(f
        .audits()
        .iter()
        .any(|a| a["action"] == "terminal.release" && a["summary"]["reason"] == "client-closed"));
}
#[tokio::test(start_paused = true)]
async fn audit_full_queue_expiry_still_releases_and_overflow_is_accounted() {
    let mut f = Fixture::new().await;
    f.elevate("control").await;
    let mut reads = direct_control(&f).await;
    // Move the immutable stream deadline to now, not the login clock or grants.
    reads.streams[&1]
        .state
        .deadline
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .1 = f.gateway.clock.now();
    while f.gateway.events.try_emit(admin::Event::error("filler")) {}
    assert_eq!(
        reads.write_tick(&f.gateway)[0]["data"]["reason"],
        "elevation-expired"
    );
    f.released().await;
    for key in 100..170 {
        f.gateway.deferred_audit.lock().unwrap().retain(
            key,
            DEVICE.into(),
            vec![admin::Event::error("deferred")],
            f.gateway.clock.now(),
        );
    }
    for _ in 0..200 {
        tokio::task::yield_now().await;
    }
    for _ in 0..3 {
        f.gateway.maintenance();
        for _ in 0..200 {
            tokio::task::yield_now().await;
        }
    }
    assert!(f
        .audits()
        .iter()
        .any(|a| a["type"] == "audit.dropped" && a["lostTightening"].as_u64().unwrap() > 0));
}

#[cfg(unix)]
use crate::test_support::fake_herdr;
#[cfg(unix)]
#[tokio::test]
async fn bridge_write_local_facade_fake_control_features_and_full_pty_reaped() {
    let snapshot = json!({"panes":[{"pane_id":"w:p","terminal_id":"term"}],"agents":[{"pane_id":"w:p","agent":"fake"}],"layouts":[{"panes":[{"pane_id":"w:p","rect":{"width":120,"height":40}}]}]});
    let fake = fake_herdr::FakeHerdr::new(snapshot).await;
    let port = Arc::new(herdr::LocalHerdr::new(fake.binary.clone()));
    let jobs = crate::herdr_jobs::Jobs::new();
    let p = port.clone();
    let j = jobs.clone();
    let control = jobs
        .submit_device("local", true, move || {
            let permit = j.reserve().unwrap();
            let id = p
                .control("work", "w:p", (120, 40), false, Arc::new(|_| Ok(())))
                .unwrap();
            let lease = Lease::new(j, p.clone(), Resource::Observe(id.clone()), permit);
            p.input(&id, b"control-input".to_vec()).unwrap();
            p.scroll(&id, HerdrScrollDirection::Up, 3).unwrap();
            p.prompt("work", "w:p", "test prompt".into()).unwrap();
            p.keys("work", "w:p", vec!["enter".into(), "ctrl+c".into()])
                .unwrap();
            lease
        })
        .unwrap()
        .await
        .unwrap();
    drop(control);
    jobs.drain().await;
    let p = port.clone();
    let j = jobs.clone();
    let full = jobs
        .submit_device("local", true, move || {
            let permit = j.reserve().unwrap();
            let id = p.full("work", (44, 30), Arc::new(|_| Ok(()))).unwrap();
            let lease = Lease::new(j, p.clone(), Resource::Observe(id.clone()), permit);
            p.input(&id, b"full-input\n".to_vec()).unwrap();
            p.resize(&id, (60, 40)).unwrap();
            lease
        })
        .unwrap()
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while !fake.root.path().join("full-input").exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    drop(full);
    jobs.drain().await;
    fake.assert_released().await;
    fake.assert_environment();
    let input = std::fs::read_to_string(fake.root.path().join("input")).unwrap();
    assert!(input.contains("terminal.input"));
    assert!(input.contains("terminal.scroll"));
    let requests = std::fs::read_to_string(fake.root.path().join("api-requests")).unwrap();
    assert!(requests.contains("agent.prompt"));
    assert!(requests.contains("agent.send_keys"));
    assert!(!requests.contains("pane.send_keys"));
    assert_eq!(fake.children().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn bridge_scope_binary_rejects_unowned_token_before_admission() {
    let mut f = Fixture::new().await;
    f.elevate("control").await;
    let mut reads = direct_control(&f).await;
    let bytes = [999u32.to_be_bytes().as_slice(), b"foreign"].concat();
    assert!(reads.binary_call(&f.gateway, &f.ctx, &bytes).is_err());
    assert!(reads.writes.receiver.try_recv().is_err());
    assert!(!f.fake.has("input:"));
    drop(reads);
    f.released().await;
}
#[tokio::test(start_paused = true)]
async fn bridge_write_full_resize_close_and_bad_scope_stream() {
    let mut f = Fixture::new().await;
    f.elevate("control").await;
    f.elevate("full").await;
    let mut ws = f.connect().await;
    let s = full(&mut ws).await["result"]["stream"].as_u64().unwrap();
    assert_eq!(
        rpc(
            &mut ws,
            "full.resize",
            json!({"stream":s,"cols":60,"rows":40})
        )
        .await["error"]["code"],
        "size-confirmation-required"
    );
    assert_eq!(
        rpc(
            &mut ws,
            "full.resize",
            json!({"stream":s,"cols":60,"rows":40,"confirmResize":true})
        )
        .await["result"]["sent"],
        true
    );
    assert_eq!(
        rpc(
            &mut ws,
            "terminal.input",
            json!({"stream":s,"text":"NEVER"})
        )
        .await["error"]["code"],
        "invalid-stream-type"
    );
    assert!(!f.fake.has("input:"));
    assert_eq!(
        rpc(&mut ws, "full.close", json!({"stream":s})).await["result"]["closed"],
        true
    );
    f.released().await;
    assert!(f
        .audits()
        .iter()
        .any(|a| a["type"] == "full.state" && a["open"] == false));
}

#[tokio::test(start_paused = true)]
async fn bridge_write_revoke_in_host_reports_unknown_and_never_replays() {
    let mut f = Fixture::new().await;
    f.elevate("control").await;
    let mut ws = f.connect().await;
    let stream = control(&mut ws).await;
    f.fake.blocked.store(true, Ordering::SeqCst);
    io(ws.send(Message::text(
        json!({"id":9,"method":"terminal.input","params":{"stream":stream,"text":"once"}})
            .to_string(),
    )))
    .await
    .unwrap();
    io(async {
        while !f.fake.has("input:") {
            tokio::task::yield_now().await;
        }
    })
    .await;
    f.revoke();
    assert_eq!(
        value(&mut ws).await["error"]["code"],
        "input-outcome-unknown"
    );
    assert!(
        matches!(io(ws.next()).await.unwrap().unwrap(),Message::Close(Some(c)) if u16::from(c.code)==4401)
    );
    f.fake.blocked.store(false, Ordering::SeqCst);
    f.released().await;
    assert_eq!(
        f.fake
            .records
            .lock()
            .unwrap()
            .iter()
            .filter(|s| s.starts_with("input:"))
            .count(),
        1
    );
    assert!(f
        .audits()
        .iter()
        .any(|a| a["action"] == "input.cancelled" && a["outcome"] == "unknown"));
}

#[tokio::test(start_paused = true)]
async fn bridge_write_expiry_clears_queued_input_without_waiting_for_next_job() {
    let mut f = Fixture::new().await;
    f.elevate("control").await;
    let mut reads = direct_control(&f).await;
    reads
        .write_call(
            &f.gateway,
            &f.ctx,
            json!(55),
            "terminal.input",
            json!({"stream":1,"text":"NEVER"}),
        )
        .unwrap();
    reads.streams[&1]
        .state
        .deadline
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .1 = f.gateway.clock.now();
    assert_eq!(
        reads.write_tick(&f.gateway)[0]["data"]["reason"],
        "elevation-expired"
    );
    let dropped = reads.cancelled_writes();
    assert_eq!(dropped.len(), 1);
    assert_eq!(dropped[0]["id"], 55);
    assert_eq!(dropped[0]["error"]["code"], "input-dropped");
    assert!(reads.writes.receiver.try_recv().is_err());
    assert!(reads.pump_writes(&f.gateway).is_empty());
    assert!(!f.fake.has("input:"));
    f.released().await;
}

#[tokio::test(start_paused = true)]
async fn bridge_write_ten_second_stall_invalidates_stream_keeps_slot_and_reads_live() {
    let mut f = Fixture::new().await;
    f.elevate("control").await;
    let mut ws = f.connect().await;
    let stream = control(&mut ws).await;
    f.fake.blocked.store(true, Ordering::SeqCst);
    io(ws.send(Message::text(
        json!({"id":9,"method":"terminal.input","params":{"stream":stream,"text":"once"}})
            .to_string(),
    )))
    .await
    .unwrap();
    io(async {
        while !f.fake.has("input:") {
            tokio::task::yield_now().await;
        }
    })
    .await;
    tokio::time::advance(Duration::from_secs(10)).await;
    assert_eq!(value(&mut ws).await["data"]["reason"], "stream-stalled");
    assert_eq!(
        value(&mut ws).await["error"]["code"],
        "input-outcome-unknown"
    );
    assert_eq!(
        rpc(
            &mut ws,
            "terminal.input",
            json!({"stream":stream,"text":"NEVER"})
        )
        .await["error"]["code"],
        "stream-stalled"
    );
    assert_eq!(
        rpc(
            &mut ws,
            "terminal.control",
            json!({"session":"work","target":"w:p","confirmResize":true})
        )
        .await["error"]["code"],
        "busy"
    );
    assert!(rpc(&mut ws, "sessions.list", json!({})).await["result"].is_object());
    assert_eq!(
        f.gateway
            .herdr_jobs
            .submit_device(DEVICE, true, || ())
            .unwrap_err(),
        "busy"
    );
    io(async {
        while !f.fake.has("release:") {
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert_eq!(
        f.audits()
            .iter()
            .filter(|a| a["action"] == "herdr-io-stalled" && a["summary"]["phase"] == "write")
            .count(),
        1
    );
    f.fake.blocked.store(false, Ordering::SeqCst);
    f.released().await;
    assert_eq!(
        f.fake
            .records
            .lock()
            .unwrap()
            .iter()
            .filter(|s| s.starts_with("input:"))
            .count(),
        1
    );
    assert!(rpc(&mut ws, "session.ping", json!({})).await["result"].is_object());
}
#[tokio::test(start_paused = true)]
async fn bridge_write_release_stall_reports_once_without_blocking_read_or_write_slots() {
    let mut f = Fixture::new().await;
    f.elevate("control").await;
    let mut ws = f.connect().await;
    let stream = control(&mut ws).await;
    f.fake.release_blocked.store(true, Ordering::SeqCst);
    assert_eq!(
        rpc(&mut ws, "terminal.release", json!({"stream":stream})).await["result"]["closed"],
        true
    );
    io(async {
        while !f.fake.has("release:") {
            tokio::task::yield_now().await;
        }
    })
    .await;
    tokio::time::advance(Duration::from_secs(10)).await;
    assert!(rpc(&mut ws, "sessions.list", json!({})).await["result"].is_object());
    io(f.gateway
        .herdr_jobs
        .submit_device(DEVICE, true, || ())
        .unwrap())
    .await
    .unwrap();
    for _ in 0..100 {
        tokio::task::yield_now().await;
    }
    assert_eq!(
        f.audits()
            .iter()
            .filter(|a| a["action"] == "herdr-io-stalled" && a["summary"]["phase"] == "release")
            .count(),
        1
    );
    assert_eq!(
        f.fake.terminals.lock().unwrap().len(),
        1,
        "logical release does not pretend physical completion"
    );
    f.fake.release_blocked.store(false, Ordering::SeqCst);
    f.released().await;
}

#[tokio::test(start_paused = true)]
async fn review_state_failed_open_and_same_device_conflict_emit_nothing() {
    let mut f = Fixture::new().await;
    f.elevate("control").await;
    let mut first = f.connect().await;
    control(&mut first).await;
    let mut second = f.connect().await;
    f.fake.conflict.store(true, Ordering::SeqCst);
    assert_eq!(
        rpc(
            &mut second,
            "terminal.control",
            json!({"session":"work","target":"term","confirmResize":true})
        )
        .await["error"]["code"],
        "owned-elsewhere"
    );
    value(&mut second).await; // callback stream.closed
    f.fake.open_error.store(true, Ordering::SeqCst);
    assert_eq!(
        rpc(
            &mut second,
            "terminal.control",
            json!({"session":"work","target":"term","confirmResize":true})
        )
        .await["error"]["code"],
        "herdr-unavailable"
    );
    assert_eq!(
        rpc(
            &mut second,
            "terminal.control",
            json!({"session":"missing","target":"term","confirmResize":true})
        )
        .await["error"]["code"],
        "session-not-allowed"
    );
    for _ in 0..100 {
        tokio::task::yield_now().await;
    }
    let states: Vec<_> = f
        .audits()
        .into_iter()
        .filter(|v| v["type"] == "control.state")
        .collect();
    assert_eq!(
        states.len(),
        1,
        "failed opens must not announce or reverse another peer: {states:?}"
    );
    assert_eq!(states[0]["open"], true);
    assert!(states[0]["peerId"].is_u64());
    assert!(states[0]["stream"].is_string());
    assert_eq!(states[0]["seq"], 1);
    assert_eq!(states[0]["target"], "w:p");
}

#[tokio::test(start_paused = true)]
async fn review_state_deferred_tightening_blocks_new_open_until_flushed() {
    let mut f = Fixture::new().await;
    f.elevate("control").await;
    let mut reads = direct_control(&f).await;
    while f.gateway.events.try_emit(admin::Event::error("filler")) {}
    reads
        .write_call(
            &f.gateway,
            &f.ctx,
            json!(2),
            "terminal.release",
            json!({"stream":1}),
        )
        .unwrap();
    for _ in 0..100 {
        tokio::task::yield_now().await;
    }
    let reply = reads
        .write_call(
            &f.gateway,
            &f.ctx,
            json!(3),
            "terminal.control",
            json!({"session":"work","target":"term","confirmResize":true}),
        )
        .unwrap();
    assert_eq!(
        reply.as_ref().map(|v| &v["error"]["code"]),
        Some(&json!("audit-unavailable")),
        "must not widen ahead of deferred state(false)"
    );
    f.gateway.maintenance();
    for _ in 0..100 {
        tokio::task::yield_now().await;
    }
    assert!(reads
        .write_call(
            &f.gateway,
            &f.ctx,
            json!(4),
            "terminal.control",
            json!({"session":"work","target":"term","confirmResize":true})
        )
        .unwrap()
        .is_none());
    reads.pump_writes(&f.gateway);
    let completed = io(reads.completed.next()).await.unwrap();
    assert!(reads.finish(completed)[0]["result"].is_object());
    drop(reads);
    f.released().await;
}

#[tokio::test(start_paused = true)]
async fn review_write_content_errors_keep_stream_and_identify_async_error() {
    let mut f = Fixture::new().await;
    f.elevate("control").await;
    let mut reads = direct_control(&f).await;
    for code in ["audit-unavailable", "herdr-unavailable"] {
        let result = reads.finish(Completion {
            id: Value::Null,
            stream: Some(1),
            result: Err(code),
            write: true,
        });
        assert_eq!(result[0]["stream"], 1);
        assert_eq!(result[0]["error"]["code"], code);
        assert!(
            reads.streams.contains_key(&1),
            "content error tore down stream"
        );
        assert!(reads.closed().is_empty());
    }
    drop(reads);
    f.released().await;
}

#[tokio::test(start_paused = true)]
async fn review_closed_binary_token_keeps_websocket_alive() {
    let mut f = Fixture::new().await;
    f.elevate("control").await;
    let mut ws = f.connect().await;
    let stream = control(&mut ws).await;
    rpc(&mut ws, "terminal.release", json!({"stream":stream})).await;
    io(ws.send(Message::binary(
        [stream.to_be_bytes().as_slice(), b"late"].concat(),
    )))
    .await
    .unwrap();
    let result = value(&mut ws).await;
    assert_eq!(result["id"], Value::Null);
    assert_eq!(result["stream"], stream);
    assert_eq!(result["error"]["code"], "stream-closed");
    assert!(rpc(&mut ws, "session.ping", json!({})).await["result"].is_object());
    f.released().await;
}

#[tokio::test(start_paused = true)]
async fn review_state_self_takeover_reconnect_has_distinct_keys_and_sequences() {
    let mut f = Fixture::new().await;
    f.elevate("control").await;
    let mut old = f.connect().await;
    control(&mut old).await;
    // Reconnected transport while the old peer has not reached pong timeout yet.
    let mut new = f.connect().await;
    let opened = rpc(
        &mut new,
        "terminal.control",
        json!({"session":"work","target":"term","confirmResize":true,"takeover":true}),
    )
    .await;
    let stream = opened["result"]["stream"].as_u64().unwrap();
    let sink = f.fake.terminals.lock().unwrap()["control-0"].clone();
    sink(HerdrTerminalEvent::Closed {
        session_id: "old".into(),
        reason: Some("terminal attach taken over".into()),
    })
    .unwrap();
    assert_eq!(value(&mut old).await["data"]["reason"], "taken-over");
    for _ in 0..100 {
        tokio::task::yield_now().await;
    }
    let states: Vec<_> = f
        .audits()
        .into_iter()
        .filter(|v| v["type"] == "control.state")
        .collect();
    assert_eq!(states.len(), 3, "{states:?}");
    assert_ne!(states[0]["peerId"], states[1]["peerId"]);
    assert_ne!(states[0]["stream"], states[1]["stream"]);
    assert_eq!(states[0]["stream"], states[2]["stream"]);
    assert_eq!(
        (states[0]["seq"].clone(), states[2]["seq"].clone()),
        (json!(1), json!(2))
    );
    assert_eq!(states[1]["target"], "w:p");
    assert_eq!(states[1]["takeover"], true);
    assert_eq!(states[2]["open"], false);
    rpc(&mut new, "terminal.release", json!({"stream":stream})).await;
    f.released().await;
}

#[tokio::test(start_paused = true)]
async fn review_started_final_guard_interleaving_never_reports_false_dropped() {
    let mut f = Fixture::new().await;
    f.elevate("control").await;
    let reads = direct_control(&f).await;
    let started = Arc::new(AtomicBool::new(false));
    let cancelled = Arc::new(AtomicBool::new(false));
    let validity = Validity {
        life: reads.life.clone(),
        state: Some(reads.streams[&1].state.clone()),
        clock: f.gateway.clock.clone(),
        until: f.gateway.clock.now().after(Duration::from_secs(60)),
        cancelled: cancelled.clone(),
    };
    let pending = PendingWrite {
        id: json!(1),
        stream: Some(1),
        input: true,
        started: started.clone(),
        cancelled,
        audit: reads.streams[&1].write.as_ref().unwrap().audit.clone(),
    };
    let result = validity.start_checked(&started, || {
        reads.life.alive.store(false, Ordering::SeqCst);
        assert_eq!(
            pending.cancelled()["error"]["code"],
            "input-outcome-unknown"
        );
    });
    assert_eq!(
        result,
        Err("input-dropped"),
        "final guard stops Host after cancellation"
    );
    assert!(!f.fake.has("input:"));
    drop(reads);
    f.released().await;
}

#[tokio::test(start_paused = true)]
async fn review_all_mutations_report_unknown_after_start_and_dropped_before_start() {
    for method in [
        "terminal.input",
        "full.input",
        "terminal.scroll",
        "full.resize",
        "agent.prompt",
        "pane.sendKeys",
    ] {
        for cancel in ["watchdog", "4401", "queued"] {
            let mut f = Fixture::new().await;
            f.elevate("control").await;
            f.elevate("full").await;
            let mut reads = direct_control(&f).await;
            let stream = if method.starts_with("full.") {
                reads
                    .write_call(
                        &f.gateway,
                        &f.ctx,
                        json!(2),
                        "full.open",
                        json!({"session":"work","cols":44,"rows":30,"confirmResize":true}),
                    )
                    .unwrap();
                reads.pump_writes(&f.gateway);
                let completed = io(reads.completed.next()).await.unwrap();
                let reply = reads.finish(completed);
                reply[0]["result"]["stream"].as_u64().unwrap()
            } else {
                1
            };
            let (params, prefix) = match method {
                "terminal.input" | "full.input" => {
                    (json!({"stream":stream,"text":"once"}), "input:")
                }
                "terminal.scroll" => (
                    json!({"stream":stream,"direction":"up","lines":1}),
                    "scroll",
                ),
                "full.resize" => (
                    json!({"stream":stream,"cols":60,"rows":40,"confirmResize":true}),
                    "resize:",
                ),
                "agent.prompt" => (
                    json!({"session":"work","target":"term","text":"once"}),
                    "prompt:",
                ),
                _ => (
                    json!({"session":"work","target":"term","keys":["Enter"]}),
                    "keys:",
                ),
            };
            f.fake.blocked.store(true, Ordering::SeqCst);
            reads
                .write_call(&f.gateway, &f.ctx, json!(77), method, params)
                .unwrap();
            if cancel != "queued" {
                reads.pump_writes(&f.gateway);
                io(async {
                    while !f.fake.has(prefix) {
                        tokio::task::yield_now().await;
                    }
                })
                .await;
            }
            let cancelled = if cancel == "watchdog" {
                tokio::time::advance(Duration::from_secs(10)).await;
                reads.write_tick(&f.gateway);
                reads.cancelled_writes()
            } else {
                f.revoke();
                reads.cancel_writes("session-invalid")
            };
            assert_eq!(cancelled.len(), 1, "{method}/{cancel}: {cancelled:?}");
            let input = method.ends_with(".input");
            let code = match (input, cancel == "queued") {
                (true, true) => "input-dropped",
                (true, false) => "input-outcome-unknown",
                (false, true) => "write-dropped",
                (false, false) => "write-outcome-unknown",
            };
            assert_eq!(cancelled[0]["error"]["code"], code, "{method}/{cancel}");
            assert!(cancelled[0].get("stream").is_some()); // null for non-stream mutations
            f.fake.blocked.store(false, Ordering::SeqCst);
            if cancel != "queued" {
                let completed = io(reads.completed.next()).await.unwrap();
                assert!(
                    reads.finish(completed).is_empty(),
                    "late result must not ACK twice"
                );
            } else {
                assert!(!f.fake.has(prefix));
            }
            drop(reads);
            f.released().await;
        }
    }
}

#[tokio::test(start_paused = true)]
async fn review_input_bucket_cap_falls_back_to_synchronous_audit() {
    let mut f = Fixture::new().await;
    f.elevate("control").await;
    let mut reads = direct_control(&f).await;
    let held: Vec<_> = (0..12)
        .map(|_| f.gateway.events.reserve_bucket().unwrap())
        .collect();
    assert!(f.gateway.events.reserve_bucket().is_none());
    reads
        .write_call(
            &f.gateway,
            &f.ctx,
            json!(2),
            "terminal.input",
            json!({"stream":1,"text":"once"}),
        )
        .unwrap();
    let work = reads.writes.receiver.try_recv().unwrap();
    assert!((work.run)().is_ok());
    assert!(reads.streams[&1]
        .write
        .as_ref()
        .unwrap()
        .bucket
        .lock()
        .unwrap()
        .is_none());
    assert!(f.fake.has("input:"));
    drop(held);
    drop(reads);
    f.released().await;
    assert!(f
        .audits()
        .iter()
        .any(|v| v["action"] == "terminal.input" && v["summary"]["bytes"] == 4));
}

#[tokio::test(start_paused = true)]
async fn review_nonstream_watchdog_before_start_prevents_late_mutation() {
    let mut f = Fixture::new().await;
    f.elevate("control").await;
    let mut reads = direct_control(&f).await;
    f.fake.records.lock().unwrap().clear();
    f.fake.snapshot_blocked.store(true, Ordering::SeqCst);
    reads
        .write_call(
            &f.gateway,
            &f.ctx,
            json!(3),
            "agent.prompt",
            json!({"session":"work","target":"term","text":"never"}),
        )
        .unwrap();
    reads.pump_writes(&f.gateway);
    io(async {
        while !f.fake.has("snapshot") {
            tokio::task::yield_now().await;
        }
    })
    .await;
    tokio::time::advance(Duration::from_secs(10)).await;
    reads.write_tick(&f.gateway);
    assert_eq!(
        reads.cancelled_writes()[0]["error"]["code"],
        "write-dropped"
    );
    f.fake.snapshot_blocked.store(false, Ordering::SeqCst);
    let completed = io(reads.completed.next()).await.unwrap();
    assert!(reads.finish(completed).is_empty());
    assert!(!f.fake.has("prompt:"));
    drop(reads);
    f.released().await;
}

#[tokio::test(start_paused = true)]
async fn review_async_busy_error_has_stream_and_keeps_stream() {
    let mut f = Fixture::new().await;
    f.elevate("control").await;
    let mut reads = direct_control(&f).await;
    f.fake.blocked.store(true, Ordering::SeqCst);
    let fake = f.fake.clone();
    let blocking = f
        .gateway
        .herdr_jobs
        .submit_device(DEVICE, true, move || {
            while fake.blocked.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(1));
            }
        })
        .unwrap();
    reads
        .write_call(
            &f.gateway,
            &f.ctx,
            Value::Null,
            "terminal.input",
            json!({"stream":1,"text":"never"}),
        )
        .unwrap();
    let result = reads.pump_writes(&f.gateway);
    assert_eq!(result[0]["stream"], 1);
    assert_eq!(result[0]["error"]["code"], "busy");
    assert!(reads.streams.contains_key(&1));
    assert!(!f.fake.has("input:"));
    f.fake.blocked.store(false, Ordering::SeqCst);
    io(blocking).await.unwrap();
    drop(reads);
    f.released().await;
}

#[tokio::test(start_paused = true)]
async fn review_reserved_open_announces_even_if_queue_fills_during_host_call() {
    for later_bundle in [false, true] {
        let mut f = Fixture::new().await;
        f.elevate("control").await;
        let mut reads = direct_control(&f).await;
        let sender = f.gateway.events.clone();
        let deferred = f.gateway.deferred_audit.clone();
        let now = f.gateway.clock.now();
        *f.fake.before_open_returns.lock().unwrap() = Some(Box::new(move || {
            while sender.try_emit(admin::Event::error("filler")) {}
            if later_bundle {
                // A different stream's tightening can appear after reservation.
                deferred.lock().unwrap().retain(
                    u64::MAX,
                    DEVICE.into(),
                    vec![admin::Event::error("tightening-fixture")],
                    now,
                );
            }
        }));
        reads
            .write_call(
                &f.gateway,
                &f.ctx,
                json!(2),
                "terminal.control",
                json!({"session":"work","target":"term","takeover":true,"confirmResize":true}),
            )
            .unwrap();
        let work = reads.writes.receiver.try_recv().unwrap();
        let result = (work.run)();
        assert!(result.is_ok());
        assert!(!f.gateway.events.available());
        let reply = reads.finish(Completion {
            id: json!(2),
            stream: Some(2),
            result,
            write: true,
        });
        assert!(
            reply[0]["result"].is_object(),
            "successful Host open must not become invisible: {reply:?}"
        );
        assert!(reads.streams[&2].write.as_ref().unwrap().announced);
        assert!(reads.streams[&2].state.active());
        for _ in 0..100 {
            tokio::task::yield_now().await;
        }
        let opened: Vec<_> = f
            .audits()
            .into_iter()
            .filter(|v| v["type"] == "control.state" && v["open"] == true)
            .collect();
        assert_eq!(opened.len(), 2);
        assert_ne!(opened[0]["stream"], opened[1]["stream"]);
        assert_eq!(opened[1]["target"], "w:p");
        assert_eq!(opened[1]["seq"], 1);
        assert_eq!(
            f.gateway.deferred_audit.lock().unwrap().pending_for(DEVICE),
            later_bundle
        );
        drop(reads);
        f.released().await;
    }
}

#[tokio::test(start_paused = true)]
async fn review_open_reservation_failure_never_calls_host_mutation() {
    let mut f = Fixture::new().await;
    f.elevate("control").await;
    let mut reads = direct_control(&f).await;
    let reservations: Vec<_> = (0..4)
        .map(|_| f.gateway.events.reserve_announcement().unwrap())
        .collect();
    f.fake.records.lock().unwrap().clear();
    reads
        .write_call(
            &f.gateway,
            &f.ctx,
            json!(2),
            "terminal.control",
            json!({"session":"work","target":"term","confirmResize":true}),
        )
        .unwrap();
    let work = reads.writes.receiver.try_recv().unwrap();
    let result = (work.run)();
    assert!(matches!(result, Err("audit-unavailable")));
    assert!(!f.fake.has("control:"));
    reads.finish(Completion {
        id: json!(2),
        stream: Some(2),
        result,
        write: true,
    });
    drop(reservations);
    drop(reads);
    f.released().await;
    let opened: Vec<_> = f
        .audits()
        .into_iter()
        .filter(|v| v["type"] == "control.state" && v["open"] == true)
        .collect();
    assert_eq!(
        opened.len(),
        1,
        "failed reservation never announced a new stream"
    );
}

#[tokio::test(start_paused = true)]
async fn final_n1_input_buckets_cannot_starve_control_or_full_announcements() {
    for full in [false, true] {
        let mut f = Fixture::new().await;
        f.elevate("control").await;
        f.elevate("full").await;
        let mut reads = direct_control(&f).await;
        // Model sixteen simultaneous input buckets; excess buckets use try_emit.
        let held: Vec<_> = (0..16)
            .filter_map(|_| f.gateway.events.reserve_bucket())
            .collect();
        assert!(held.len() >= 12);
        let (method, params) = if full {
            (
                "full.open",
                json!({"session":"work","cols":44,"rows":30,"confirmResize":true}),
            )
        } else {
            (
                "terminal.control",
                json!({"session":"work","target":"w:p","confirmResize":true}),
            )
        };
        assert!(reads
            .write_call(&f.gateway, &f.ctx, json!(2), method, params)
            .unwrap()
            .is_none());
        let closed = reads.pump_writes(&f.gateway);
        if full {
            assert_eq!(
                closed,
                vec![json!({"event":"stream.closed","data":{"stream":1,"reason":"full-opened"}})]
            );
        } else {
            assert!(closed.is_empty());
        }
        let completed = io(reads.completed.next()).await.unwrap();
        let reply = reads.finish(completed);
        assert!(
            reply[0]["result"].is_object(),
            "input buckets must not starve announcement: {reply:?}"
        );
        assert!(reads.streams[&2].write.as_ref().unwrap().announced);
        assert!(reads.streams[&2].lease.is_some());
        assert!(f.fake.has(if full { "full:" } else { "control:" }));
        assert_eq!(held.len(), 12);
        drop(held);
        drop(reads);
        f.released().await;
    }
}

#[tokio::test(start_paused = true)]
async fn final_n3_unleased_unannounced_open_failure_never_defers_or_blocks_device() {
    for full in [false, true] {
        for queue_full in [false, true] {
            let mut f = Fixture::new().await;
            f.elevate(if full { "full" } else { "control" }).await;
            let (_, invalidated) = f
                .gateway
                .with_authorized(&f.ctx, Scope::View, |i| i.register_peer(&f.ctx))
                .unwrap()
                .unwrap();
            let (_, stop) = tokio::sync::watch::channel(0);
            let (mut reads, _) =
                Reads::new(Arc::new(Semaphore::new(4 * 1024 * 1024)), invalidated, stop);
            let (method, params) = if full {
                (
                    "full.open",
                    json!({"session":"work","cols":44,"rows":30,"confirmResize":true}),
                )
            } else {
                (
                    "terminal.control",
                    json!({"session":"work","target":"w:p","confirmResize":true}),
                )
            };
            assert!(reads
                .write_call(&f.gateway, &f.ctx, json!(1), method, params.clone())
                .unwrap()
                .is_none());
            assert!(!reads.streams[&1].write.as_ref().unwrap().announced);
            assert!(reads.streams[&1].lease.is_none());
            // Cancel before starting the Host job, then exercise the real failure removal.
            drop(reads.writes.receiver.try_recv().unwrap());
            if queue_full {
                while f.gateway.events.try_emit(admin::Event::error("filler")) {}
            }
            let reply = reads.finish(Completion {
                id: json!(1),
                stream: Some(1),
                result: Err("write-dropped"),
                write: true,
            });
            assert_eq!(reply[0]["error"]["code"], "write-dropped");
            assert!(!f.gateway.deferred_audit.lock().unwrap().pending_for(DEVICE));
            if queue_full {
                assert!(!f.gateway.events.available());
            }
            // Admission must not see a fictitious same-device pending tightening.
            assert!(reads
                .write_call(&f.gateway, &f.ctx, json!(2), method, params)
                .unwrap()
                .is_none());
            for _ in 0..100 {
                tokio::task::yield_now().await;
            }
            f.gateway.maintenance();
            for _ in 0..100 {
                tokio::task::yield_now().await;
            }
            let events = f.audits();
            assert!(!events.iter().any(|e| matches!(
                e["action"].as_str(),
                Some("terminal.release" | "full.close" | "expiry" | "revoke")
            ) || matches!(
                e["type"].as_str(),
                Some("control.state" | "full.state")
            )));
            if queue_full {
                let counts = events
                    .iter()
                    .find(|e| e["type"] == "audit.dropped")
                    .unwrap();
                assert_eq!(counts["lost"], 1);
                assert_eq!(counts["lostTightening"], 0);
                assert_eq!(counts["rejectedOmitted"], 0);
            } else {
                let failed = events
                    .iter()
                    .find(|e| e["action"] == "open.failed")
                    .unwrap();
                assert_eq!(failed["reason"], "write-dropped");
            }
            assert!(!f.fake.has("control:") && !f.fake.has("full:"));
            drop(reads);
            f.released().await;
        }
    }
}

#[tokio::test(start_paused = true)]
async fn final_f1_inflight_control_and_full_open_tighten_on_watchdog_and_4401() {
    for full in [false, true] {
        for revoke in [false, true] {
            let mut f = Fixture::new().await;
            f.elevate(if full { "full" } else { "control" }).await;
            let mut ws = f.connect().await;
            f.fake.open_blocked.store(true, Ordering::SeqCst);
            let (method, params, prefix) = if full {
                (
                    "full.open",
                    json!({"session":"work","cols":44,"rows":30,"confirmResize":true}),
                    "full:",
                )
            } else {
                (
                    "terminal.control",
                    json!({"session":"work","target":"w:p","confirmResize":true}),
                    "control:",
                )
            };
            io(ws.send(Message::text(
                json!({"id":77,"method":method,"params":params}).to_string(),
            )))
            .await
            .unwrap();
            io(async {
                while !f.fake.has(prefix) {
                    tokio::task::yield_now().await;
                }
            })
            .await;
            if revoke {
                f.revoke();
            } else {
                tokio::time::advance(Duration::from_secs(10)).await;
            }
            let cancelled = value(&mut ws).await;
            assert_eq!(cancelled["id"], 77);
            assert_eq!(cancelled["error"]["code"], "write-outcome-unknown");
            if revoke {
                assert!(
                    matches!(io(ws.next()).await.unwrap().unwrap(), Message::Close(Some(c)) if u16::from(c.code) == 4401)
                );
            }
            let action = if revoke {
                "revoke"
            } else if full {
                "full.close"
            } else {
                "terminal.release"
            };
            // Observe teardown before allowing Host to publish its late lease.
            io(async {
                while !f
                    .audits()
                    .iter()
                    .any(|e| e["action"] == action || e["action"] == "open.failed")
                {
                    tokio::task::yield_now().await;
                }
            })
            .await;
            assert!(f.fake.terminals.lock().unwrap().is_empty());
            f.fake.open_blocked.store(false, Ordering::SeqCst);
            io(async {
                while !f.fake.has(if full {
                    "release:full-"
                } else {
                    "release:control-"
                }) {
                    tokio::task::yield_now().await;
                }
            })
            .await;
            f.released().await;
            if !revoke {
                // A late open must not produce another ACK before this ping reply.
                assert!(rpc(&mut ws, "session.ping", json!({})).await["result"].is_object());
            }
            let events = f.audits();
            let tightened: Vec<_> = events.iter().filter(|e| e["action"] == action).collect();
            assert_eq!(tightened.len(), 1, "{method}/revoke={revoke}: {events:?}");
            assert_eq!(
                tightened[0]["reason"],
                if revoke {
                    "session-invalid"
                } else {
                    "stream-stalled"
                }
            );
            assert!(!events.iter().any(|e| e["action"] == "open.failed"));
            assert!(!events
                .iter()
                .any(|e| matches!(e["type"].as_str(), Some("control.state" | "full.state"))));
        }
    }
}

#[tokio::test(start_paused = true)]
async fn final_f1_leased_unannounced_open_tightens_without_state_false() {
    for full in [false, true] {
        for host_error in [false, true] {
            let mut f = Fixture::new().await;
            f.elevate(if full { "full" } else { "control" }).await;
            let (_, invalidated) = f
                .gateway
                .with_authorized(&f.ctx, Scope::View, |i| i.register_peer(&f.ctx))
                .unwrap()
                .unwrap();
            let (_, stop) = tokio::sync::watch::channel(0);
            let (mut reads, _) =
                Reads::new(Arc::new(Semaphore::new(4 * 1024 * 1024)), invalidated, stop);
            let (method, params, action) = if full {
                (
                    "full.open",
                    json!({"session":"work","cols":44,"rows":30,"confirmResize":true}),
                    "full.close",
                )
            } else {
                (
                    "terminal.control",
                    json!({"session":"work","target":"w:p","confirmResize":true}),
                    "terminal.release",
                )
            };
            f.fake.open_error.store(host_error, Ordering::SeqCst);
            assert!(reads
                .write_call(&f.gateway, &f.ctx, json!(1), method, params)
                .unwrap()
                .is_none());
            let work = reads.writes.receiver.try_recv().unwrap();
            let result = (work.run)();
            if host_error {
                assert!(matches!(result, Err("herdr-unavailable")));
            } else {
                assert!(matches!(result, Ok(Output::Opened(..))));
            }
            let write = reads.streams[&1].write.as_ref().unwrap();
            assert_eq!(write.leased.load(Ordering::SeqCst), !host_error);
            assert!(!write.announced);
            assert!(reads.streams[&1].lease.is_none());
            // Host has returned, but completion has not announced state(true).
            drop(reads);
            drop(result);
            f.released().await;
            for _ in 0..100 {
                tokio::task::yield_now().await;
            }
            let events = f.audits();
            assert_eq!(
                events.iter().filter(|e| e["action"] == action).count(),
                usize::from(!host_error)
            );
            assert_eq!(
                events
                    .iter()
                    .filter(|e| e["action"] == "open.failed")
                    .count(),
                usize::from(host_error)
            );
            assert!(!events
                .iter()
                .any(|e| matches!(e["type"].as_str(), Some("control.state" | "full.state"))));
            assert_eq!(f.fake.has("release:"), !host_error);
            assert!(!f.gateway.deferred_audit.lock().unwrap().pending_for(DEVICE));
        }
    }
}
