use super::*;
use crate::{
    admin::{self, Init},
    clock::Clock,
    herdr::{HerdrPort, Result},
    http::Server,
    test_support::git_repo::{git, Repo, SENTINEL},
};
use futures_util::{SinkExt, StreamExt};
use std::{
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};
use tokio::{net::TcpStream, sync::oneshot, task::JoinHandle};
use tokio_tungstenite::{tungstenite::client::IntoClientRequest, WebSocketStream};
use yuzora_host::herdr_service::{
    HerdrNamedSession, HerdrSnapshotResult, OnSubscriptionEvent, OnTerminalEvent,
};
type Ws = WebSocketStream<TcpStream>;
#[derive(Default)]
struct Fake {
    terminals: Mutex<HashMap<String, OnTerminalEvent>>,
    subscriptions: Mutex<HashMap<String, OnSubscriptionEvent>>,
    calls: Mutex<Vec<String>>,
    sizes: Mutex<Vec<(u16, u16)>>,
    next: AtomicUsize,
    snapshot_label: Mutex<String>,
    attach_blocked: AtomicBool,
    /// A pane appears while the subscription is being set up.
    create_on_subscribe: AtomicBool,
    created: AtomicBool,
}
impl Fake {
    fn snapshot_value(&self) -> Value {
        let mut panes = vec![
            json!({"pane_id":"w:p","terminal_id":"term","workspace_id":"w","tab_id":"w:t","agent_status":"working","cwd":"SECRET"}),
        ];
        if self.created.load(Ordering::SeqCst) {
            panes.push(json!({"pane_id":"w:q","terminal_id":"term-q","workspace_id":"w","tab_id":"w:t","agent_status":"idle","cwd":"SECRET"}));
        }
        json!({"workspaces":[{"workspace_id":"w","label":*self.snapshot_label.lock().unwrap(),"path":"SECRET"}],"tabs":[],"agents":[],"panes":panes,"layouts":[{"panes":[{"pane_id":"w:p","rect":{"width":120,"height":40}}]}]})
    }
    fn emit(&self, text: &str) {
        for (id, sink) in self.terminals.lock().unwrap().iter() {
            let _ = sink(HerdrTerminalEvent::Frame {
                session_id: id.clone(),
                seq: 1,
                full: true,
                encoding: "ansi".into(),
                width: 120,
                height: 40,
                bytes_base64: B64.encode(text),
            });
        }
    }
    fn frame(&self, id: &str, text: &str) {
        if let Some(sink) = self.terminals.lock().unwrap().get(id) {
            let _ = sink(HerdrTerminalEvent::Frame {
                session_id: id.into(),
                seq: 1,
                full: true,
                encoding: "ansi".into(),
                width: 120,
                height: 40,
                bytes_base64: B64.encode(text),
            });
        }
    }
    fn active(&self) -> usize {
        self.terminals.lock().unwrap().len() + self.subscriptions.lock().unwrap().len()
    }
}
impl HerdrPort for Fake {
    crate::test_support::reject_writes!();
    fn sessions(&self) -> Result<Vec<HerdrNamedSession>> {
        self.calls.lock().unwrap().push("list".into());
        Ok([("work", true), ("stopped", false), ("private", true)]
            .into_iter()
            .map(|(name, running)| HerdrNamedSession {
                name: name.into(),
                running,
                default: false,
                session_dir: "SECRET".into(),
                socket_path: "SECRET".into(),
            })
            .collect())
    }
    fn snapshot(&self, session: &str) -> Result<HerdrSnapshotResult> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("snapshot:{session}"));
        Ok(HerdrSnapshotResult {
            protocol: 22,
            version: "0.9.1".into(),
            snapshot: self.snapshot_value(),
        })
    }
    fn observe(
        &self,
        session: &str,
        target: &str,
        cols: u16,
        rows: u16,
        sink: OnTerminalEvent,
    ) -> Result<String> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("observe:{session}:{target}"));
        self.sizes.lock().unwrap().push((cols, rows));
        let id = format!("term-{}", self.next.fetch_add(1, Ordering::SeqCst));
        self.terminals.lock().unwrap().insert(id.clone(), sink);
        self.frame(&id, "initial");
        while self.attach_blocked.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(2));
        }
        Ok(id)
    }
    fn subscribe(
        &self,
        session: &str,
        panes: Vec<String>,
        sink: OnSubscriptionEvent,
    ) -> Result<String> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("subscribe:{session}:{}", panes.join(",")));
        if self.create_on_subscribe.load(Ordering::SeqCst) {
            self.created.store(true, Ordering::SeqCst);
        }
        let id = format!("events-{}", self.next.fetch_add(1, Ordering::SeqCst));
        self.subscriptions.lock().unwrap().insert(id.clone(), sink);
        Ok(id)
    }
    fn release(&self, resource: Resource) {
        match resource {
            Resource::Observe(id) => {
                self.terminals.lock().unwrap().remove(&id);
            }
            Resource::Events(id) => {
                self.subscriptions.lock().unwrap().remove(&id);
            }
        }
    }
}
struct Fixture {
    gateway: Arc<Gateway>,
    fake: Arc<Fake>,
    port: u16,
    cookie: String,
    binding: String,
    /// A second paired device: its cookie and binding.
    tablet: (String, String),
    stop: Option<oneshot::Sender<()>>,
    server: JoinHandle<bool>,
    writer: JoinHandle<std::io::Result<()>>,
    /// Every admin event the gateway wrote, parsed.
    events: Arc<Mutex<Vec<Value>>>,
    drain: JoinHandle<()>,
    /// The workspace trust file the gateway reads.
    trust_store: std::path::PathBuf,
    _root: tempfile::TempDir,
}
impl Fixture {
    async fn new() -> Self {
        let fake = Arc::new(Fake::default());
        Self::with_port(fake.clone(), fake).await
    }
    async fn with_port(port: Arc<dyn HerdrPort>, fake: Arc<Fake>) -> Self {
        let mut init: Init = serde_json::from_str(&crate::test_support::init::json()).unwrap();
        init.session_allowlist = vec!["work".into(), "stopped".into()];
        let phone = include_str!("../tests/fixtures/device.json");
        let tablet = phone
            .replace("AAAAAAAAAAAAAAAAAAAAAA", "AAAAAAAAAAAAAAAAAAAAAQ")
            .replace("\"id\":\"AQ\"", "\"id\":\"Ag\"");
        init.devices = vec![
            serde_json::from_str(phone).unwrap(),
            serde_json::from_str(&tablet).unwrap(),
        ];
        let root = tempfile::tempdir().unwrap();
        init.trust_store = root.path().join("workspace-trust.json");
        let (output, input) = tokio::io::duplex(1 << 20);
        let (events, writer) = admin::spawn_writer(output);
        let log = Arc::new(Mutex::new(Vec::new()));
        let drain = tokio::spawn({
            let log = log.clone();
            async move {
                let mut lines = tokio::io::AsyncBufReadExt::lines(tokio::io::BufReader::new(input));
                while let Ok(Some(line)) = lines.next_line().await {
                    log.lock()
                        .unwrap()
                        .push(serde_json::from_str(&line).unwrap());
                }
            }
        });
        let clock = Clock::at(serde_json::from_str("\"2030-01-01T00:00:00Z\"").unwrap());
        let gateway = Gateway::with_herdr(&init, events, clock, port).unwrap();
        let (id, binding) = gateway
            .identity
            .lock()
            .unwrap()
            .create_for_test("AAAAAAAAAAAAAAAAAAAAAA", "desktop.tailnet.ts.net")
            .unwrap();
        let (tablet_id, tablet_binding) = gateway
            .identity
            .lock()
            .unwrap()
            .create_for_test("AAAAAAAAAAAAAAAAAAAAAQ", "desktop.tailnet.ts.net")
            .unwrap();
        let server = Server::bind(&init.listeners.loopback, root.path(), gateway.clone())
            .await
            .unwrap();
        let port = server.port().unwrap();
        let (stop, stopped) = oneshot::channel();
        Self {
            gateway,
            fake,
            port,
            cookie: crate::session::session_cookie(&id),
            binding,
            tablet: (crate::session::session_cookie(&tablet_id), tablet_binding),
            stop: Some(stop),
            server: tokio::spawn(server.run(stopped)),
            writer,
            events: log,
            drain,
            trust_store: init.trust_store.clone(),
            _root: root,
        }
    }
    async fn connect(&self) -> Ws {
        self.connect_as(&self.cookie, &self.binding).await
    }
    async fn connect_tablet(&self) -> Ws {
        self.connect_as(&self.tablet.0, &self.tablet.1).await
    }
    async fn connect_as(&self, cookie: &str, binding: &str) -> Ws {
        let mut request = "ws://desktop.tailnet.ts.net:8443/ws"
            .into_client_request()
            .unwrap();
        for (key, value) in [
            ("origin", "https://desktop.tailnet.ts.net:8443"),
            (
                "yzc-proxy-secret",
                crate::test_support::identity::PROXY_SECRET,
            ),
            ("yzc-peer-login", "user@example.com"),
            ("cookie", cookie),
        ] {
            request.headers_mut().insert(key, value.parse().unwrap());
        }
        let (mut ws, _) = tokio_tungstenite::client_async(
            request,
            TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, self.port))
                .await
                .unwrap(),
        )
        .await
        .unwrap();
        send(&mut ws, json!({"type":"auth","binding":binding})).await;
        assert_eq!(value(&mut ws).await["event"], "session.ready");
        ws
    }
    async fn released(&self) {
        tokio::time::timeout(Duration::from_secs(3), async {
            while self.fake.active() != 0 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            self.gateway.herdr_jobs.drain().await;
        })
        .await
        .unwrap();
    }
    /// The desktop's `spaces.roots` admin command.
    fn map_spaces(&self, roots: Value) {
        let command =
            serde_json::from_value(json!({"v":1,"type":"spaces.roots","roots":roots})).unwrap();
        self.gateway
            .command(admin::CommandBody::Spaces(command))
            .unwrap();
    }
    /// `git.read` audits written so far, after the admin writer has caught up.
    async fn git_audits(&self) -> Vec<Value> {
        tokio::time::sleep(Duration::from_millis(100)).await;
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter(|event| event["type"] == "audit" && event["action"] == "git.read")
            .cloned()
            .collect()
    }
    fn revoke(&self) {
        self.gateway
            .command(admin::CommandBody::Device(admin::DeviceCommand::Revoke {
                v: 1,
                device_id: "AAAAAAAAAAAAAAAAAAAAAA".into(),
            }))
            .unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.fake.attach_blocked.store(false, Ordering::SeqCst);
        self.server.abort();
        self.writer.abort();
        self.drain.abort();
    }
}
async fn send(ws: &mut Ws, value: Value) {
    ws.send(Message::text(value.to_string())).await.unwrap();
}
async fn next(ws: &mut Ws) -> Message {
    tokio::time::timeout(Duration::from_secs(3), ws.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
}
async fn value(ws: &mut Ws) -> Value {
    match next(ws).await {
        Message::Text(s) => serde_json::from_str(&s).unwrap(),
        other => panic!("expected JSON, got {other:?}"),
    }
}
async fn rpc(ws: &mut Ws, method: &str, params: Value) -> Value {
    send(ws, json!({"id":1,"method":method,"params":params})).await;
    value(ws).await
}
async fn observe(ws: &mut Ws) -> u32 {
    let reply = rpc(
        ws,
        "terminal.observe",
        json!({"session":"work","target":"w:p","cols":1,"rows":"ignored"}),
    )
    .await;
    assert!(reply.get("error").is_none(), "{reply}");
    reply["result"]["stream"].as_u64().unwrap() as u32
}
async fn binary(ws: &mut Ws, id: u32, text: &str) {
    let Message::Binary(bytes) = next(ws).await else {
        panic!("expected binary")
    };
    assert_eq!(&bytes[..4], &id.to_be_bytes());
    assert_eq!(&bytes[4..], text.as_bytes());
}

#[tokio::test]
async fn bridge_read_loopback_sessions_snapshot_chunking_and_payload_error() {
    let f = Fixture::new().await;
    let mut ws = f.connect().await;
    assert_eq!(
        rpc(&mut ws, "sessions.list", json!({})).await["result"],
        json!({"sessions":[{"name":"work","default":false}]})
    );
    assert_eq!(
        rpc(&mut ws, "snapshot.get", json!({"session":"stopped"})).await["error"]["code"],
        "session-stopped"
    );
    assert_eq!(
        rpc(
            &mut ws,
            "terminal.observe",
            json!({"session":"private","target":"w:p"})
        )
        .await["error"]["code"],
        "session-not-allowed"
    );
    assert!(!f
        .fake
        .calls
        .lock()
        .unwrap()
        .iter()
        .any(|s| s.starts_with("observe")));
    let snapshot = rpc(&mut ws, "snapshot.get", json!({"session":"work"})).await;
    assert!(!snapshot.to_string().contains("SECRET"));
    assert_eq!(snapshot["result"]["panes"][0]["status"], "working");
    *f.fake.snapshot_label.lock().unwrap() = "界\\\"".repeat(20000);
    let chunked = rpc(&mut ws, "snapshot.get", json!({"session":"work"})).await;
    let count = chunked["result"]["chunks"].as_u64().unwrap();
    let mut bytes = Vec::new();
    for index in 0..count {
        let event = value(&mut ws).await;
        assert!(event.to_string().len() < 65536);
        assert_eq!(event["event"], "snapshot.chunk");
        assert_eq!(event["data"]["index"], index);
        bytes.extend(B64.decode(event["data"]["data"].as_str().unwrap()).unwrap());
    }
    assert_eq!(bytes.len() as u64, chunked["result"]["bytes"]);
    assert_eq!(
        serde_json::from_slice::<Value>(&bytes).unwrap()["workspaces"][0]["label"],
        *f.fake.snapshot_label.lock().unwrap()
    );
    *f.fake.snapshot_label.lock().unwrap() = "x".repeat(herdr::SNAPSHOT_BYTES);
    assert_eq!(
        rpc(&mut ws, "snapshot.get", json!({"session":"work"})).await["error"]["code"],
        "payload-too-large"
    );
    assert!(rpc(&mut ws, "session.ping", json!({})).await["result"].is_object());
}
#[tokio::test]
async fn bridge_stream_observe_size_limit_close_and_socket_teardown() {
    let f = Fixture::new().await;
    let mut ws = f.connect().await;
    let hello = rpc(&mut ws, "hello", json!({})).await;
    assert_eq!(hello["result"]["capabilities"]["limits"]["streams"], 8);
    let mut streams = Vec::new();
    for _ in 0..8 {
        let id = observe(&mut ws).await;
        binary(&mut ws, id, "initial").await;
        streams.push(id);
    }
    assert!(f.fake.sizes.lock().unwrap().iter().all(|s| *s == (120, 40)));
    assert_eq!(
        rpc(
            &mut ws,
            "terminal.observe",
            json!({"session":"work","target":"w:p"})
        )
        .await["error"]["code"],
        "stream-limit"
    );
    assert_eq!(
        rpc(&mut ws, "stream.close", json!({"stream":streams[0]})).await["result"]["closed"],
        true
    );
    assert_eq!(
        rpc(&mut ws, "stream.close", json!({"stream":9999})).await["error"]["code"],
        "stream-not-found"
    );
    drop(ws);
    f.released().await;
}
#[tokio::test]
async fn bridge_stream_overflow_isolation_keeps_peer_and_other_stream_alive() {
    let f = Fixture::new().await;
    let mut ws = f.connect().await;
    let first = observe(&mut ws).await;
    binary(&mut ws, first, "initial").await;
    let second = observe(&mut ws).await;
    binary(&mut ws, second, "initial").await;
    // Current-thread runtime cannot drain until this deterministic burst finishes.
    for _ in 0..257 {
        f.fake.frame("term-0", "x");
    }
    f.fake.frame("term-1", "healthy");
    let event = value(&mut ws).await;
    assert_eq!(
        event,
        json!({"event":"stream.closed","data":{"stream":first,"reason":"slow-consumer"}})
    );
    binary(&mut ws, second, "healthy").await;
    assert!(rpc(&mut ws, "session.ping", json!({})).await["result"].is_object());
    drop(ws);
    f.released().await;
}
#[tokio::test]
async fn bridge_events_filter_one_subscription_and_bounded_overflow() {
    let f = Fixture::new().await;
    let mut ws = f.connect().await;
    let subscription = rpc(&mut ws, "events.subscribe", json!({"session":"work"})).await;
    assert_eq!(
        rpc(&mut ws, "events.subscribe", json!({"session":"work"})).await,
        subscription
    );
    assert_eq!(f.fake.subscriptions.lock().unwrap().len(), 1);
    let sink = f
        .fake
        .subscriptions
        .lock()
        .unwrap()
        .values()
        .next()
        .unwrap()
        .clone();
    // Agent-status selectors are scoped to the snapshot's panes.
    assert!(f
        .fake
        .calls
        .lock()
        .unwrap()
        .contains(&"subscribe:work:w:p".to_string()));
    sink(HerdrSubscriptionEvent::WorktreeChanged {
        subscription_id: "backend-secret".into(),
        kind: "private".into(),
        workspace_id: None,
    })
    .unwrap();
    sink(HerdrSubscriptionEvent::TopologyChanged {
        subscription_id: "backend-secret".into(),
        kind: "pane.updated".into(),
        workspace_id: Some("SECRET".into()),
        tab_id: None,
    })
    .unwrap();
    sink(HerdrSubscriptionEvent::TopologyChanged {
        subscription_id: "backend-secret".into(),
        kind: "pane.created".into(),
        workspace_id: Some("SECRET".into()),
        tab_id: Some("SECRET".into()),
    })
    .unwrap();
    let stale = value(&mut ws).await;
    assert_eq!(
        stale,
        json!({"event":"snapshot.stale","data":{"session":"work"},"stream":subscription["result"]["stream"]})
    );
    sink(HerdrSubscriptionEvent::AgentStatusChanged {
        subscription_id: "backend-secret".into(),
        pane_id: "w:p".into(),
        workspace_id: "w".into(),
        agent_status: "blocked".into(),
        agent: Some("SECRET".into()),
        display_agent: None,
        title: Some("SECRET".into()),
        state_labels: HashMap::new(),
    })
    .unwrap();
    let event = value(&mut ws).await;
    assert_eq!(event["event"], "agent.status");
    assert_eq!(event["data"]["status"], "blocked");
    assert!(!event.to_string().contains("SECRET"));
    assert!(!event.to_string().contains("backend-secret"));
    for _ in 0..257 {
        let _ = sink(HerdrSubscriptionEvent::PaneExited {
            subscription_id: "backend-secret".into(),
            pane_id: "w:p".into(),
            workspace_id: "w".into(),
        });
    }
    let closed = value(&mut ws).await;
    assert_eq!(closed["event"], "stream.closed");
    assert_eq!(closed["data"]["reason"], "slow-consumer");
    f.released().await;
    assert!(rpc(&mut ws, "session.ping", json!({})).await["result"].is_object());
}
#[tokio::test]
async fn bridge_events_report_a_pane_created_while_subscribing() {
    let f = Fixture::new().await;
    f.fake.create_on_subscribe.store(true, Ordering::SeqCst);
    let mut ws = f.connect().await;
    let subscription = rpc(&mut ws, "events.subscribe", json!({"session":"work"})).await;
    // Subscribed to the panes it saw; the one created meanwhile is not covered.
    assert!(f
        .fake
        .calls
        .lock()
        .unwrap()
        .contains(&"subscribe:work:w:p".to_string()));
    assert_eq!(
        value(&mut ws).await,
        json!({"event":"snapshot.stale","data":{"session":"work"},"stream":subscription["result"]["stream"]})
    );
}
#[tokio::test]
async fn bridge_stream_revoke_stops_all_production_no_frames_after_4401() {
    let f = Fixture::new().await;
    let mut ws = f.connect().await;
    let stream = observe(&mut ws).await;
    binary(&mut ws, stream, "initial").await;
    rpc(&mut ws, "events.subscribe", json!({"session":"work"})).await;
    // No await between enqueue and revoke: the peer has a genuinely queued
    // frame, but the current-thread runtime cannot have written it to TCP yet.
    f.fake.emit("QUEUED-BEFORE-REVOKE");
    let events = f
        .fake
        .subscriptions
        .lock()
        .unwrap()
        .values()
        .next()
        .unwrap()
        .clone();
    events(HerdrSubscriptionEvent::PaneExited {
        subscription_id: "backend".into(),
        pane_id: "w:p".into(),
        workspace_id: "w".into(),
    })
    .unwrap();
    f.revoke();
    f.fake.emit("MUST-NOT-DELIVER");
    let Message::Close(Some(close)) = next(&mut ws).await else {
        panic!("frames after revoke")
    };
    assert_eq!(u16::from(close.code), 4401);
    f.released().await;
}
#[tokio::test]
async fn bridge_stream_pending_attach_orphan_releases_after_revoke_and_shutdown() {
    for shutdown in [false, true] {
        let mut f = Fixture::new().await;
        let mut ws = f.connect().await;
        f.fake.attach_blocked.store(true, Ordering::SeqCst);
        send(
            &mut ws,
            json!({"id":1,"method":"terminal.observe","params":{"session":"work","target":"w:p"}}),
        )
        .await;
        tokio::time::timeout(Duration::from_secs(3), async {
            while f.fake.active() == 0 {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
        if shutdown {
            f.stop.take().unwrap().send(()).unwrap();
        } else {
            f.revoke();
        }
        f.fake.emit("MUST-NOT-DELIVER");
        let Message::Close(Some(close)) = next(&mut ws).await else {
            panic!("orphan frames")
        };
        assert_eq!(u16::from(close.code), if shutdown { 1001 } else { 4401 });
        f.fake.attach_blocked.store(false, Ordering::SeqCst);
        f.released().await;
    }
}

#[cfg(unix)]
use crate::test_support::fake_herdr;

#[cfg(unix)]
#[tokio::test]
async fn bridge_local_manager_fake_binary_api_and_children_reaped_on_all_exits() {
    for ending in ["close", "socket", "revoke", "shutdown"] {
        let fake = Arc::new(Fake::default());
        let local = fake_herdr::FakeHerdr::new(fake.snapshot_value()).await;
        let port = Arc::new(herdr::LocalHerdr::new(local.binary.clone()));
        let mut f = Fixture::with_port(port, fake).await;
        let mut ws = f.connect().await;
        assert_eq!(
            rpc(&mut ws, "sessions.list", json!({})).await["result"]["sessions"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            rpc(
                &mut ws,
                "terminal.observe",
                json!({"session":"stopped","target":"w:p"})
            )
            .await["error"]["code"],
            "session-stopped"
        );
        let stream = observe(&mut ws).await;
        binary(&mut ws, stream, "fake frame").await;
        let subscribed = rpc(&mut ws, "events.subscribe", json!({"session":"work"})).await;
        assert!(
            subscribed["result"]["subscribed"].as_bool().unwrap(),
            "{subscribed}"
        );
        assert_eq!(value(&mut ws).await["event"], "agent.status");
        match ending {
            "close" => {
                assert_eq!(
                    rpc(&mut ws, "stream.close", json!({"stream":stream})).await["result"]
                        ["closed"],
                    true
                );
                assert_eq!(
                    rpc(
                        &mut ws,
                        "stream.close",
                        json!({"stream":subscribed["result"]["stream"]})
                    )
                    .await["result"]["closed"],
                    true
                );
            }
            "revoke" => {
                f.revoke();
                assert!(
                    matches!(next(&mut ws).await,Message::Close(Some(frame)) if u16::from(frame.code)==4401)
                );
            }
            "shutdown" => {
                f.stop.take().unwrap().send(()).unwrap();
                assert!(
                    matches!(next(&mut ws).await,Message::Close(Some(frame)) if u16::from(frame.code)==1001)
                );
            }
            _ => {}
        }
        drop(ws);
        tokio::time::timeout(Duration::from_secs(3), f.gateway.herdr_jobs.drain())
            .await
            .unwrap();
        local.assert_released().await;
        assert_eq!(local.children().len(), 1);
        local.assert_environment();
    }
}

#[cfg(unix)]
struct DelayedLocal {
    local: herdr::LocalHerdr,
    blocked: Arc<AtomicBool>,
}
#[cfg(unix)]
impl HerdrPort for DelayedLocal {
    crate::test_support::reject_writes!();
    fn sessions(&self) -> Result<Vec<HerdrNamedSession>> {
        self.local.sessions()
    }
    fn snapshot(&self, session: &str) -> Result<HerdrSnapshotResult> {
        self.local.snapshot(session)
    }
    fn observe(
        &self,
        session: &str,
        target: &str,
        cols: u16,
        rows: u16,
        sink: OnTerminalEvent,
    ) -> Result<String> {
        let resource = self.local.observe(session, target, cols, rows, sink)?;
        while self.blocked.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(2));
        }
        Ok(resource)
    }
    fn subscribe(
        &self,
        session: &str,
        panes: Vec<String>,
        sink: OnSubscriptionEvent,
    ) -> Result<String> {
        self.local.subscribe(session, panes, sink)
    }
    fn release(&self, resource: Resource) {
        self.local.release(resource)
    }
}
#[cfg(unix)]
#[tokio::test]
async fn bridge_local_pending_attach_orphan_eventually_reaps_real_fake_child() {
    for shutdown in [false, true] {
        let fake = Arc::new(Fake::default());
        let local = fake_herdr::FakeHerdr::new(fake.snapshot_value()).await;
        let blocked = Arc::new(AtomicBool::new(true));
        let port = Arc::new(DelayedLocal {
            local: herdr::LocalHerdr::new(local.binary.clone()),
            blocked: blocked.clone(),
        });
        let mut f = Fixture::with_port(port, fake).await;
        let mut ws = f.connect().await;
        send(
            &mut ws,
            json!({"id":1,"method":"terminal.observe","params":{"session":"work","target":"w:p"}}),
        )
        .await;
        tokio::time::timeout(Duration::from_secs(3), async {
            while local.children().is_empty() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        if shutdown {
            f.stop.take().unwrap().send(()).unwrap();
        } else {
            f.revoke();
        }
        let received = next(&mut ws).await;
        blocked.store(false, Ordering::SeqCst); // Unblock even if the assertion fails.
        assert!(
            matches!(received,Message::Close(Some(frame)) if u16::from(frame.code)==if shutdown{1001}else{4401})
        );
        tokio::time::timeout(Duration::from_secs(3), f.gateway.herdr_jobs.drain())
            .await
            .unwrap();
        local.assert_released().await;
        local.assert_environment();
    }
}

#[tokio::test]
async fn bridge_stream_oversize_is_distinct_error_and_ownership_is_per_peer() {
    let f = Fixture::new().await;
    let mut ws = f.connect().await;
    let mut other = f.connect().await;
    let stream = observe(&mut ws).await;
    binary(&mut ws, stream, "initial").await;
    assert_eq!(
        rpc(&mut other, "stream.close", json!({"stream":stream})).await["error"]["code"],
        "stream-not-found"
    );
    f.fake.frame("term-0", &"x".repeat(65533));
    let closed = value(&mut ws).await;
    assert_eq!(closed["event"], "stream.closed");
    assert_eq!(closed["data"]["reason"], "payload-too-large");
    assert!(rpc(&mut ws, "hello", json!({})).await["result"].is_object());
    f.released().await;
}
#[tokio::test]
async fn bridge_stream_pending_limit_and_control_input_never_reaches_observer() {
    let f = Fixture::new().await;
    let mut ws = f.connect().await;
    f.fake.attach_blocked.store(true, Ordering::SeqCst);
    for id in [1, 2] {
        send(
            &mut ws,
            json!({"id":id,"method":"terminal.observe","params":{"session":"work","target":"w:p"}}),
        )
        .await;
    }
    send(
        &mut ws,
        json!({"id":3,"method":"sessions.list","params":{}}),
    )
    .await;
    let response = value(&mut ws).await;
    assert_eq!(response["id"], 3);
    assert_eq!(response["error"]["code"], "busy");
    assert_eq!(
        rpc(
            &mut ws,
            "terminal.input",
            json!({"stream":1,"text":"MUST-NOT-WRITE"})
        )
        .await["error"]["code"],
        "elevation-required"
    );
    ws.send(Message::binary(
        [1u32.to_be_bytes().as_slice(), b"MUST-NOT-WRITE"].concat(),
    ))
    .await
    .unwrap();
    let received = next(&mut ws).await;
    f.fake.attach_blocked.store(false, Ordering::SeqCst);
    assert!(matches!(received,Message::Close(Some(frame)) if u16::from(frame.code)==1003));
    f.released().await;
    assert!(!f
        .fake
        .calls
        .lock()
        .unwrap()
        .iter()
        .any(|call| call.contains("input") || call.contains("MUST-NOT-WRITE")));
}
#[tokio::test]
async fn bridge_stream_byte_overflow_reserves_space_for_close_and_rpc() {
    let f = Fixture::new().await;
    let mut ws = f.connect().await;
    let stream = observe(&mut ws).await;
    binary(&mut ws, stream, "initial").await;
    for _ in 0..10 {
        f.fake.frame("term-0", &"x".repeat(65500));
    }
    let closed = value(&mut ws).await;
    assert_eq!(closed["data"]["reason"], "slow-consumer");
    assert!(rpc(&mut ws, "session.ping", json!({})).await["result"].is_object());
    f.released().await;
}
#[tokio::test]
async fn bridge_stream_live_closed_reasons_cross_real_ws() {
    for (reason,expected) in [("terminal attach failed: terminal term_65c7a07b973731 already has an attached client; retry with --takeover","owned-elsewhere"),("terminal attach taken over","taken-over"),("server is shutting down","session-stopped"),("private secret reason","stream-closed")] {
        let f=Fixture::new().await;let mut ws=f.connect().await;let stream=observe(&mut ws).await;binary(&mut ws,stream,"initial").await;
        let sink=f.fake.terminals.lock().unwrap().values().next().unwrap().clone();sink(HerdrTerminalEvent::Closed{session_id:"backend-id".into(),reason:Some(reason.into())}).unwrap();
        assert_eq!(value(&mut ws).await,json!({"event":"stream.closed","data":{"stream":stream,"reason":expected}}));f.released().await;
    }
}

#[cfg(unix)]
#[tokio::test]
async fn bridge_local_stopped_inventory_never_invokes_startup_or_attach() {
    let fake = Arc::new(Fake::default());
    let local = fake_herdr::FakeHerdr::new(fake.snapshot_value()).await;
    let path = local.root.path().join("sessions.json");
    let mut inventory: Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    for session in inventory["sessions"].as_array_mut().unwrap() {
        session["running"] = json!(false);
    }
    std::fs::write(path, inventory.to_string()).unwrap();
    std::fs::write(
        local.root.path().join("status.json"),
        json!({"server":{"status":"not_running","running":false}}).to_string(),
    )
    .unwrap();
    let f = Fixture::with_port(Arc::new(herdr::LocalHerdr::new(local.binary.clone())), fake).await;
    let mut ws = f.connect().await;
    assert_eq!(
        rpc(&mut ws, "sessions.list", json!({})).await["result"],
        json!({"sessions":[]})
    );
    assert_eq!(
        rpc(
            &mut ws,
            "terminal.observe",
            json!({"session":"work","target":"w:p"})
        )
        .await["error"]["code"],
        "session-stopped"
    );
    assert!(local.children().is_empty());
    let calls = std::fs::read_to_string(local.root.path().join("calls")).unwrap();
    assert!(
        calls.lines().all(|line| line == "session list --json"),
        "stopped inventory must never probe/start/attach: {calls}"
    );
    local.assert_environment();
    local.assert_released().await;
}

#[cfg(unix)]
#[tokio::test]
async fn bridge_local_snapshot_chunks_and_host_size_error_never_1013() {
    for size in [60000, 1100000] {
        let fake = Arc::new(Fake::default());
        *fake.snapshot_label.lock().unwrap() = "x".repeat(size);
        let local = fake_herdr::FakeHerdr::new(fake.snapshot_value()).await;
        let f =
            Fixture::with_port(Arc::new(herdr::LocalHerdr::new(local.binary.clone())), fake).await;
        let mut ws = f.connect().await;
        let result = rpc(&mut ws, "snapshot.get", json!({"session":"work"})).await;
        if size == 60000 {
            assert_eq!(result["result"]["chunked"], true, "{result}");
            for index in 0..result["result"]["chunks"].as_u64().unwrap() {
                let event = value(&mut ws).await;
                assert_eq!(event["event"], "snapshot.chunk");
                assert_eq!(event["data"]["index"], index);
            }
        } else {
            assert_eq!(result["error"]["code"], "payload-too-large", "{result}");
        }
        assert!(rpc(&mut ws, "session.ping", json!({})).await["result"].is_object());
        local.assert_environment();
        local.assert_released().await;
    }
}

#[tokio::test]
async fn bridge_snapshot_eight_actual_stream_queues_reserve_budget_and_reply_busy() {
    let f = Fixture::new().await;
    let (outbound, mut replies) = crate::bridge::Outbound::new();
    let (_invalidate, invalidated) = watch::channel(false);
    let (_stop, stop) = watch::channel(0);
    let (mut reads, frames) = Reads::new(outbound.bytes.clone(), invalidated.clone(), stop.clone());
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("cookie", f.cookie.parse().unwrap());
    headers.insert("x-yzc-binding", f.binding.parse().unwrap());
    let ctx = f.gateway.authenticate(&headers, Scope::View).unwrap();
    for id in 1..=8 {
        assert!(reads
            .call(
                &f.gateway,
                &ctx,
                json!(id),
                "terminal.observe",
                json!({"session":"work","target":"w:p"})
            )
            .unwrap()
            .is_none());
        let completed = reads.completed.next().await.unwrap();
        assert!(reads.finish(completed)[0]["result"].is_object());
    }
    // Keep the production Frames receiver undrained, as with a stalled reader.
    for id in 0..8 {
        for _ in 0..6 {
            f.fake.frame(&format!("term-{id}"), &"x".repeat(65000));
        }
    }
    assert!(reads.closed().is_empty());
    let free = outbound.bytes.available_permits();
    assert!(free < 1024 * 1024);
    assert_eq!(
        outbound.batch(snapshot_replies(
            json!(9),
            json!({"data":"x".repeat(2*1024*1024-1000)})
        )),
        Err("busy")
    );
    assert!(replies.try_recv().is_err());
    assert_eq!(outbound.bytes.available_permits(), free);
    // Freeze stream readiness to model a consumer that does not drain binary;
    // this is deterministic queue pressure, not an OS socket-buffer claim.
    for stream in reads.streams.values() {
        stream.state.ready.store(false, Ordering::SeqCst);
    }
    *outbound.preloaded_streams.lock().unwrap() = Some((reads, frames));
    *f.fake.snapshot_label.lock().unwrap() = "x".repeat(2 * 1024 * 1024 - 4096);
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let gateway = f.gateway.clone();
    let peer = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let socket = tokio_tungstenite::accept_async(socket).await.unwrap();
        crate::bridge::drive_for_test(socket, &gateway, &ctx, stop, invalidated, outbound, replies)
            .await
    });
    let (mut ws, _) = tokio_tungstenite::client_async(
        format!("ws://{address}/"),
        TcpStream::connect(address).await.unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(value(&mut ws).await["event"], "session.ready");
    assert_eq!(
        rpc(&mut ws, "snapshot.get", json!({"session":"work"})).await["error"]["code"],
        "busy"
    );
    assert!(rpc(&mut ws, "session.ping", json!({})).await["result"].is_object());
    ws.close(None).await.unwrap();
    assert_eq!(peer.await.unwrap(), 1000);
    f.released().await;
}
#[tokio::test]
async fn bridge_snapshot_two_near_two_mib_replies_never_close_1013() {
    let f = Fixture::new().await;
    *f.fake.snapshot_label.lock().unwrap() = "x".repeat(2 * 1024 * 1024 - 4096);
    let mut ws = f.connect().await;
    for id in [91, 92] {
        send(
            &mut ws,
            json!({"id":id,"method":"snapshot.get","params":{"session":"work"}}),
        )
        .await;
    }
    let (mut replies, mut chunks, mut successes) = (0, 0, 0);
    while replies < 2 || chunks > 0 {
        let message = value(&mut ws).await;
        if message["event"] == "snapshot.chunk" {
            assert!(chunks > 0);
            chunks -= 1;
        } else {
            replies += 1;
            if message["result"]["chunked"] == true {
                chunks += message["result"]["chunks"].as_u64().unwrap();
                successes += 1;
            } else {
                assert_eq!(message["error"]["code"], "busy");
            }
        }
    }
    assert!(successes > 0);
    assert!(rpc(&mut ws, "session.ping", json!({})).await["result"].is_object());
}

// Real Sink permits, not a synthetic acquisition; keep Frames undrained.
pub(crate) fn fill_streams_after_snapshot(bytes: Arc<Semaphore>) -> Frames {
    let (_, invalidated) = watch::channel(false);
    let (_, stop) = watch::channel(0);
    let (reads, frames) = Reads::new(bytes.clone(), invalidated, stop);
    for id in 1..=8 {
        let state = Arc::new(StreamState {
            life: reads.life.clone(),
            closed: Mutex::new(None),
            ready: AtomicBool::new(true),
            ready_changed: Notify::new(),
            changed: reads.changed.clone(),
            slots: Arc::new(Semaphore::new(STREAM_FRAMES)),
            bytes: Arc::new(Semaphore::new(STREAM_BYTES)),
            deadline: Mutex::new(None),
        });
        let sink = Sink {
            id,
            state,
            sender: reads.sender.clone(),
            bytes: bytes.clone(),
        };
        loop {
            let available = bytes
                .available_permits()
                .min(sink.state.bytes.available_permits());
            let size = available.min(MESSAGE_BYTES + 16).saturating_sub(16);
            if sink.send(Message::binary(vec![0; size])).is_err() {
                break;
            }
        }
    }
    assert!(bytes.available_permits() < 16);
    frames
}

/// A trusted repository with one commit, mapped as Space `w` of session `work`.
async fn git_fixture() -> (Fixture, Repo) {
    let f = Fixture::new().await;
    let mut repo = Repo::new();
    repo.trust_store = f.trust_store.clone();
    repo.commit("a.txt", "one\ntwo\n", "first");
    repo.trust();
    f.map_spaces(json!([repo.root("work", "w")]));
    (f, repo)
}

#[tokio::test]
async fn bridge_git_reads_a_mapped_space_and_refuses_everything_else_with_fixed_codes() {
    let f = Fixture::new().await;
    let mut repo = Repo::new();
    repo.trust_store = f.trust_store.clone();
    let head = repo.commit("a.txt", "one\ntwo\n", "first");
    repo.trust();
    let mut ws = f.connect().await;
    let mut replies = Vec::new();

    // Nothing is mapped yet: not even a trusted repository is reachable.
    let params = json!({"session":"work","workspaceId":"w"});
    replies.push(rpc(&mut ws, "git.status", params.clone()).await);
    assert_eq!(replies[0]["error"]["code"], "git-unavailable");
    let snapshot = rpc(&mut ws, "snapshot.get", json!({"session":"work"})).await;
    assert_eq!(snapshot["result"]["workspaces"][0]["id"], "w");
    assert_eq!(snapshot["result"]["workspaces"][0]["git"], false);

    // The other session is not on the allowlist, so its entry is dropped.
    f.map_spaces(json!([repo.root("work", "w"), repo.root("private", "w")]));
    let snapshot = rpc(&mut ws, "snapshot.get", json!({"session":"work"})).await;
    assert_eq!(snapshot["result"]["workspaces"][0]["git"], true);
    assert!(!snapshot.to_string().contains(SENTINEL));

    for (method, extra) in [
        ("git.status", json!({})),
        ("git.branches", json!({})),
        ("git.log", json!({"limit":10})),
        ("git.commit", json!({"hash":head})),
        (
            "git.diff",
            json!({"kind":"commit","commit":head,"path":"a.txt"}),
        ),
    ] {
        let mut params = params.clone();
        for (key, value) in extra.as_object().unwrap() {
            params[key] = value.clone();
        }
        let reply = rpc(&mut ws, method, params).await;
        assert!(reply.get("error").is_none(), "{method}: {reply}");
        replies.push(reply);
    }
    assert_eq!(replies[1]["result"]["status"]["branch"], "main");
    assert_eq!(replies[1]["result"]["repo"]["name"], "repo");
    assert_eq!(replies[2]["result"]["local"][0]["name"], "main");
    assert_eq!(replies[3]["result"]["commits"][0]["hash"], head);
    assert_eq!(replies[4]["result"]["subject"], "first");
    assert!(replies[5]["result"]["text"]
        .as_str()
        .unwrap()
        .contains("+one"));

    for (method, params, code) in [
        (
            "git.status",
            json!({"session":"private","workspaceId":"w"}),
            "session-not-allowed",
        ),
        (
            "git.status",
            json!({"session":"nobody","workspaceId":"w"}),
            "session-not-allowed",
        ),
        (
            "git.status",
            json!({"session":"stopped","workspaceId":"w"}),
            "git-unavailable",
        ),
        (
            "git.status",
            json!({"session":"work","workspaceId":"nope"}),
            "git-unavailable",
        ),
        ("git.status", json!({"session":"work"}), "invalid-params"),
        (
            "git.log",
            json!({"session":"work","workspaceId":"w","limit":0}),
            "invalid-params",
        ),
        (
            "git.log",
            json!({"session":"work","workspaceId":"w","cursor":"junk"}),
            "git-cursor-invalid",
        ),
        (
            "git.commit",
            json!({"session":"work","workspaceId":"w","hash":"HEAD"}),
            "invalid-params",
        ),
        (
            "git.diff",
            json!({"session":"work","workspaceId":"w","kind":"worktree","path":"../x"}),
            "git-failed",
        ),
    ] {
        let reply = rpc(&mut ws, method, params).await;
        assert_eq!(reply["error"]["code"], code, "{method}: {reply}");
        assert_eq!(reply["error"]["message"], code);
        replies.push(reply);
    }
    // The stopped session was never started, queried or listed for any of it.
    assert!(!f
        .fake
        .calls
        .lock()
        .unwrap()
        .iter()
        .any(|call| call.contains("stopped")));

    // The desktop revokes trust: the next read, on a cached repository, is refused.
    repo.revoke();
    let reply = rpc(&mut ws, "git.status", params.clone()).await;
    assert_eq!(reply["error"]["code"], "git-untrusted");
    assert_eq!(reply["error"]["message"], "git-untrusted");
    replies.push(reply);
    // A new mapping drops the Space: no Git again until it is mapped.
    f.map_spaces(json!([]));
    let reply = rpc(&mut ws, "git.status", params).await;
    assert_eq!(reply["error"]["code"], "git-unavailable");
    replies.push(reply);

    // No reply carries any part of a path: neither the folder nor the store.
    let all = serde_json::to_string(&replies).unwrap();
    for private in [
        SENTINEL,
        "workspace-trust",
        "challengeId",
        "canonicalPath",
        ".git/",
    ] {
        assert!(!all.contains(private), "{private}");
    }
    assert!(rpc(&mut ws, "session.ping", json!({})).await["result"].is_object());
}

#[tokio::test]
async fn bridge_git_large_replies_use_the_chunked_reply_path() {
    let (f, repo) = git_fixture().await;
    let mut ws = f.connect().await;
    let line = "y".repeat(99) + "\n";
    repo.write("big.txt", line.repeat(4000));
    let reply = rpc(
        &mut ws,
        "git.diff",
        json!({"session":"work","workspaceId":"w","kind":"worktree","path":"big.txt","untracked":true}),
    )
    .await;
    assert_eq!(reply["result"]["chunked"], true, "{reply}");
    assert_eq!(reply["result"]["encoding"], "base64-json-v1");
    let mut bytes = Vec::new();
    for index in 0..reply["result"]["chunks"].as_u64().unwrap() {
        let event = value(&mut ws).await;
        assert!(event.to_string().len() < 65536);
        assert_eq!(event["event"], "snapshot.chunk");
        assert_eq!(event["data"]["id"], 1);
        assert_eq!(event["data"]["index"], index);
        bytes.extend(B64.decode(event["data"]["data"].as_str().unwrap()).unwrap());
    }
    assert_eq!(bytes.len() as u64, reply["result"]["bytes"]);
    let diff: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(diff["truncated"], true);
    assert_eq!(diff["binary"], false);
    assert!(diff["text"].as_str().unwrap().len() <= 262144);
    // Small ones stay inline, and the socket is still healthy.
    let small = rpc(
        &mut ws,
        "git.log",
        json!({"session":"work","workspaceId":"w"}),
    )
    .await;
    assert!(small["result"]["commits"].is_array());
}

#[tokio::test]
async fn bridge_git_read_is_audited_at_most_once_a_minute_per_peer_and_space() {
    let (f, repo) = git_fixture().await;
    f.map_spaces(json!([repo.root("work", "w"), repo.root("work", "x")]));
    let params = |workspace: &str| json!({"session":"work","workspaceId":workspace});
    let mut first = f.connect().await;
    for method in ["git.status", "git.branches", "git.log", "git.status"] {
        assert!(rpc(&mut first, method, params("w"))
            .await
            .get("result")
            .is_some());
    }
    // Refused before the executor: nothing was read, nothing is audited.
    for (session, workspace) in [("work", "missing"), ("private", "w")] {
        let reply = rpc(
            &mut first,
            "git.status",
            json!({"session":session,"workspaceId":workspace}),
        )
        .await;
        assert!(reply.get("error").is_some());
    }
    let audits = f.git_audits().await;
    assert_eq!(audits.len(), 1, "{audits:?}");
    let audit = &audits[0];
    assert_eq!(audit["v"], 1);
    assert_eq!(audit["deviceId"], "AAAAAAAAAAAAAAAAAAAAAA");
    assert_eq!(audit["scope"], "view");
    assert_eq!(audit["session"], "work");
    assert_eq!(audit["target"], "w");
    assert_eq!(audit["summary"], json!({"method":"git.status"}));
    assert!(audit["at"].is_string());
    let peer = audit["peerId"].clone();

    // Another Space has its own minute; another peer does too.
    assert!(rpc(&mut first, "git.log", params("x"))
        .await
        .get("result")
        .is_some());
    let mut second = f.connect().await;
    assert!(rpc(&mut second, "git.status", params("w"))
        .await
        .get("result")
        .is_some());
    let audits = f.git_audits().await;
    assert_eq!(audits.len(), 3, "{audits:?}");
    assert_eq!(audits[1]["target"], "x");
    assert_eq!(audits[1]["peerId"], peer);
    assert_eq!(audits[2]["target"], "w");
    assert_ne!(audits[2]["peerId"], peer);
    assert!(!serde_json::to_string(&audits).unwrap().contains(SENTINEL));
}

#[tokio::test]
async fn bridge_git_shares_the_read_executor_with_every_other_read() {
    let (f, _repo) = git_fixture().await;
    let mut ws = f.connect().await;
    let params = json!({"session":"work","workspaceId":"w"});
    // Both of the device's read slots are busy: Git is refused like any read.
    let (release, held) = std::sync::mpsc::channel::<()>();
    let held = Arc::new(Mutex::new(held));
    let jobs: Vec<_> = (0..2)
        .map(|_| {
            let held = held.clone();
            f.gateway
                .herdr_jobs
                .submit_device("AAAAAAAAAAAAAAAAAAAAAA", false, move || {
                    let _ = held.lock().unwrap().recv();
                })
                .unwrap()
        })
        .collect();
    let reply = rpc(&mut ws, "git.status", params.clone()).await;
    assert_eq!(reply["error"]["code"], "busy");
    assert!(
        f.git_audits().await.is_empty(),
        "a refused read is not audited"
    );
    drop(release);
    for job in jobs {
        job.await.unwrap();
    }
    // A job sends its result before its slots drop; wait until they have.
    f.released().await;
    assert!(rpc(&mut ws, "git.status", params.clone())
        .await
        .get("result")
        .is_some());
    assert_eq!(f.git_audits().await.len(), 1);
}

/// The git params of Space `w` of session `work`, plus `extra`.
fn git_params(extra: Value) -> Value {
    let mut params = json!({"session":"work","workspaceId":"w"});
    for (key, value) in extra.as_object().unwrap() {
        params[key] = value.clone();
    }
    params
}

#[tokio::test]
async fn bridge_git_diff_reads_only_what_git_lists_as_untracked() {
    let (f, repo) = git_fixture().await;
    repo.write(".gitignore", ".env\n");
    git(&repo.dir, &["add", "--", ".gitignore"]);
    git(&repo.dir, &["commit", "-q", "-m", "ignore"]);
    repo.write(".env", "API_KEY=sk-live-123\n");
    let remote = [
        "remote",
        "add",
        "origin",
        "https://user:tok3n@example.com/x.git",
    ];
    git(&repo.dir, &remote);
    repo.write("new.txt", "hello\n");
    let mut ws = f.connect().await;

    // An ignored file, the repository's own config (any case) and a tracked
    // file are not untracked files, whatever the request says.
    let mut replies = Vec::new();
    for path in [".env", ".git/config", ".GIT/config", "a.txt"] {
        let params = git_params(json!({"kind":"worktree","path":path,"untracked":true}));
        let reply = rpc(&mut ws, "git.diff", params).await;
        assert_eq!(reply["error"]["code"], "invalid-params", "{path}: {reply}");
        replies.push(reply);
    }
    for extra in [
        json!({"kind":"worktree","path":".git/config"}),
        json!({"kind":"staged","path":".GIT/config"}),
        json!({"kind":"commit","path":".git/config","commit":repo.head()}),
    ] {
        let reply = rpc(&mut ws, "git.diff", git_params(extra.clone())).await;
        assert_eq!(reply["error"]["code"], "invalid-params", "{extra}: {reply}");
        replies.push(reply);
    }
    let all = serde_json::to_string(&replies).unwrap();
    for secret in ["sk-live-123", "tok3n", "example.com"] {
        assert!(!all.contains(secret), "{secret}");
    }

    let params = git_params(json!({"kind":"worktree","path":"new.txt","untracked":true}));
    let reply = rpc(&mut ws, "git.diff", params).await;
    assert!(reply["result"]["text"].as_str().unwrap().contains("+hello"));
}

#[tokio::test]
async fn bridge_git_diff_of_a_renamed_file_in_a_commit_takes_its_old_path() {
    let (f, repo) = git_fixture().await;
    let body: String = (0..20).map(|n| format!("line {n}\n")).collect();
    repo.commit("old.txt", &body, "add old");
    git(&repo.dir, &["mv", "old.txt", "new.txt"]);
    repo.write("new.txt", body.replace("line 3\n", "line three\n"));
    git(&repo.dir, &["add", "--", "new.txt"]);
    git(&repo.dir, &["commit", "-q", "-m", "rename"]);
    let mut ws = f.connect().await;
    let base = json!({"kind":"commit","path":"new.txt","commit":repo.head()});
    let text = |reply: &Value| reply["result"]["text"].as_str().unwrap().to_owned();

    let added = rpc(&mut ws, "git.diff", git_params(base.clone())).await;
    assert!(text(&added).contains("new file mode"));
    let mut renamed = base;
    renamed["origPath"] = json!("old.txt");
    let renamed = rpc(&mut ws, "git.diff", git_params(renamed)).await;
    assert!(text(&renamed).contains("rename from old.txt\nrename to new.txt\n"));
    assert!(text(&renamed).contains("-line 3\n+line three\n"));
}

#[tokio::test]
async fn bridge_git_log_cursors_are_short_tokens_of_their_own_device_and_space() {
    let (f, repo) = git_fixture().await;
    for n in 2..=5 {
        repo.commit("a.txt", &format!("{n}\n"), &format!("commit {n}"));
    }
    f.map_spaces(json!([repo.root("work", "w"), repo.root("work", "x")]));
    let mut phone = f.connect().await;
    let mut tablet = f.connect_tablet().await;

    let first = rpc(&mut phone, "git.log", git_params(json!({"limit":2}))).await;
    let token = first["result"]["nextCursor"].as_str().unwrap().to_owned();
    assert!(token.len() <= 64, "{token}");
    let host = yuzora_host::git_log::log_page(&repo.dir, None, 2, None, None, None, None)
        .unwrap()
        .next_cursor
        .unwrap();
    assert_ne!(token, host);
    let next = |cursor: &str, workspace: &str| {
        let extra = json!({"limit":2,"cursor":cursor,"workspaceId":workspace});
        git_params(extra)
    };
    let second = rpc(&mut phone, "git.log", next(&token, "w")).await;
    assert_eq!(second["result"]["commits"][0]["subject"], "commit 3");

    // Another device, another Space of the same repository, a raw Host cursor.
    let reply = rpc(&mut tablet, "git.log", next(&token, "w")).await;
    assert_eq!(reply["error"]["code"], "git-cursor-invalid", "{reply}");
    let reply = rpc(&mut phone, "git.log", next(&token, "x")).await;
    assert_eq!(reply["error"]["code"], "git-cursor-invalid", "{reply}");
    let reply = rpc(&mut phone, "git.log", next(&host, "w")).await;
    assert_eq!(reply["error"]["code"], "git-cursor-invalid", "{reply}");
    // Neither refusal costs the device its token.
    let again = rpc(&mut phone, "git.log", next(&token, "w")).await;
    assert_eq!(again["result"]["commits"][0]["subject"], "commit 3");
}

#[tokio::test(start_paused = true)]
async fn bridge_git_log_cursor_lives_thirty_minutes_on_the_gateway_clock() {
    let (f, repo) = git_fixture().await;
    for n in 2..=4 {
        repo.commit("a.txt", &format!("{n}\n"), &format!("commit {n}"));
    }
    let (_invalidate, invalidated) = watch::channel(false);
    let (_stop, stop) = watch::channel(0);
    let (mut reads, _frames) = Reads::new(Arc::new(Semaphore::new(1 << 20)), invalidated, stop);
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("cookie", f.cookie.parse().unwrap());
    headers.insert("x-yzc-binding", f.binding.parse().unwrap());
    let ctx = f.gateway.authenticate(&headers, Scope::View).unwrap();
    let log = async |reads: &mut Reads, extra: Value| {
        let early = reads
            .call(&f.gateway, &ctx, json!(1), "git.log", git_params(extra))
            .unwrap();
        match early {
            Some(reply) => reply,
            None => {
                let completed = reads.completed.next().await.unwrap();
                reads.finish(completed).remove(0)
            }
        }
    };

    let first = log(&mut reads, json!({"limit":1})).await;
    let token = first["result"]["nextCursor"].as_str().unwrap().to_owned();
    tokio::time::advance(Duration::from_secs(29 * 60)).await;
    let page = log(&mut reads, json!({"limit":1,"cursor":token})).await;
    assert_eq!(
        page["result"]["commits"][0]["subject"], "commit 3",
        "{page}"
    );
    // 31 minutes after it was issued, the token is gone; the one issued two
    // minutes ago is not.
    tokio::time::advance(Duration::from_secs(2 * 60)).await;
    let expired = log(&mut reads, json!({"limit":1,"cursor":token})).await;
    assert_eq!(expired["error"]["code"], "git-cursor-invalid", "{expired}");
    let young = page["result"]["nextCursor"].as_str().unwrap();
    let page = log(&mut reads, json!({"limit":1,"cursor":young})).await;
    assert_eq!(
        page["result"]["commits"][0]["subject"], "commit 2",
        "{page}"
    );
}
