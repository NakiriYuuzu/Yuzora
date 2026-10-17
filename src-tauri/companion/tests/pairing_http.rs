#[path = "support/init.rs"]
mod init_fixture;
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, DuplexStream},
    net::TcpStream,
    sync::oneshot,
    task::JoinHandle,
};
use yuzora_companion::{
    admin::{self, Event, EventSender, Init, PairingCommand},
    clock::Clock,
    gateway::Gateway,
    http::{Server, SECURITY_HEADERS},
};

struct Fixture {
    _root: tempfile::TempDir,
    gateway: Arc<Gateway>,
    events: EventSender,
    output: BufReader<DuplexStream>,
    port: u16,
    stop: Option<oneshot::Sender<()>>,
    server: JoinHandle<bool>,
    writer: JoinHandle<std::io::Result<()>>,
}
impl Fixture {
    async fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let init: Init = serde_json::from_str(&init_fixture::json()).unwrap();
        // A tiny pipe permits deterministic writer backpressure in the queue test.
        let (output, input) = tokio::io::duplex(1);
        let (events, writer) = admin::spawn_writer(output);
        let gateway = Gateway::new(&init, events.clone(), Clock::default()).unwrap();
        let server = Server::bind(&init.listeners.loopback, root.path(), gateway.clone())
            .await
            .unwrap();
        let port = server.port().unwrap();
        let (stop, stopped) = oneshot::channel();
        Self {
            _root: root,
            gateway,
            events,
            output: BufReader::new(input),
            port,
            stop: Some(stop),
            server: tokio::spawn(server.run(stopped)),
            writer,
        }
    }
    async fn event(&mut self) -> Value {
        let mut line = String::new();
        self.output.read_line(&mut line).await.unwrap();
        serde_json::from_str(&line).unwrap()
    }
    async fn create(&mut self) -> (String, String) {
        self.gateway
            .pairings
            .command(PairingCommand::Create {
                v: 1,
                path: "tailscale".into(),
                add_to_device: None,
            })
            .unwrap();
        let created = self.event().await;
        let (id, secret) = created["url"]
            .as_str()
            .unwrap()
            .split_once("#p=")
            .unwrap()
            .1
            .split_once('.')
            .unwrap();
        (id.into(), secret.into())
    }
    async fn post(&self, path: &str, body: Value) -> (u16, String) {
        self.post_ua(path, body, "Test agent").await
    }
    async fn post_ua(&self, path: &str, body: Value, ua: &str) -> (u16, String) {
        let text = body.to_string();
        let request = format!("POST {path} HTTP/1.1\r\nHost: desktop.tailnet.ts.net:8443\r\nTailscale-User-Login: user@example.com\r\nOrigin: https://desktop.tailnet.ts.net:8443\r\nContent-Type: application/json\r\nUser-Agent: {ua}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}", text.len());
        let mut socket = TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, self.port))
            .await
            .unwrap();
        socket.write_all(request.as_bytes()).await.unwrap();
        let mut response = String::new();
        tokio::time::timeout(Duration::from_secs(2), socket.read_to_string(&mut response))
            .await
            .unwrap()
            .unwrap();
        for (name, value) in SECURITY_HEADERS {
            assert!(response.contains(&format!("{name}: {value}\r\n")));
        }
        assert!(response.contains("cache-control: no-store\r\n"));
        let status = response.split_whitespace().nth(1).unwrap().parse().unwrap();
        (status, response.split_once("\r\n\r\n").unwrap().1.into())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        self.server.abort();
        self.writer.abort();
    }
}
fn claim(id: &str, secret: &str) -> Value {
    json!({"pairingId": id, "secret":secret, "deviceLabel":"Test phone", "deviceNonce":"BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc"})
}
#[tokio::test]
async fn pairing_http_claim_approve_rotating_ticket_second_claim_void() {
    let mut f = Fixture::new().await;
    let (id, secret) = f.create().await;
    let (status, body) = f.post("/api/pair/claim", claim(&id, &secret)).await;
    assert_eq!(status, 200);
    let response: Value = serde_json::from_str(&body).unwrap();
    let event = f.event().await;
    assert_eq!(event["sas"], response["sas"]);
    assert_eq!(event["userAgent"], "Test agent");
    for secret in [&secret, response["claimToken"].as_str().unwrap()] {
        assert!(!event.to_string().contains(secret));
    }
    assert_eq!(
        f.post("/api/pair/status", json!({"pairingId":id})).await.0,
        400
    );
    assert_eq!(
        f.post(
            "/api/pair/status",
            json!({"pairingId":id, "claimToken":"wrong"})
        )
        .await
        .0,
        403
    );
    f.gateway
        .pairings
        .command(PairingCommand::Approve {
            v: 1,
            pairing_id: id.clone(),
        })
        .unwrap();
    f.event().await;
    let poll = json!({"pairingId":id, "claimToken":response["claimToken"]});
    let first: Value =
        serde_json::from_str(&f.post("/api/pair/status", poll.clone()).await.1).unwrap();
    let second: Value = serde_json::from_str(&f.post("/api/pair/status", poll).await.1).unwrap();
    assert_eq!(first["state"], "approved");
    assert_ne!(first["registrationTicket"], second["registrationTicket"]);
    assert!(f
        .gateway
        .pairings
        .consume_ticket(&id, first["registrationTicket"].as_str().unwrap())
        .is_err());
    assert_eq!(f.post("/api/pair/claim", claim(&id, &secret)).await.0, 403);
    assert_eq!(f.event().await["reason"], "second-claim");
    assert!(f
        .gateway
        .pairings
        .consume_ticket(&id, second["registrationTicket"].as_str().unwrap())
        .is_err());
}
#[tokio::test]
async fn pairing_http_queue_full_503_does_not_claim() {
    let mut f = Fixture::new().await;
    let (id, secret) = f.create().await;
    assert!(f.events.try_emit(Event::error("filler")));
    tokio::task::yield_now().await;
    let mut queued = 1;
    while f.events.try_emit(Event::error("filler")) {
        queued += 1;
    }
    let started = std::time::Instant::now();
    let (status, body) = f.post("/api/pair/claim", claim(&id, &secret)).await;
    assert_eq!(status, 503);
    assert_eq!(body, "Admin event queue unavailable");
    assert!(started.elapsed() < Duration::from_millis(500));
    for _ in 0..queued {
        f.event().await;
    }
    assert_eq!(f.post("/api/pair/claim", claim(&id, &secret)).await.0, 200);
    assert_eq!(f.event().await["type"], "pairing.claimed");
}
#[tokio::test]
async fn pairing_http_invalid_json_queries_and_unknown_fields_fixed_errors() {
    let mut f = Fixture::new().await;
    let (id, secret) = f.create().await;
    for (path, body, expected) in [
        (
            "/api/pair/claim?secret=DO-NOT-ECHO",
            claim(&id, &secret),
            400,
        ),
        (
            "/api/pair/claim",
            json!({"pairingId":id,"secret":secret,"deviceLabel":"phone","deviceNonce":"bad","unknown":"DO-NOT-ECHO"}),
            400,
        ),
        (
            "/api/pair/status",
            json!({"pairingId":id,"claimToken":"DO-NOT-ECHO"}),
            403,
        ),
        ("/api/auth/logout", json!({}), 401),
    ] {
        let (status, body) = f.post(path, body).await;
        assert_eq!(status, expected);
        assert!(!body.contains("DO-NOT-ECHO"));
        assert!(!body.contains(&secret));
    }
}

#[tokio::test]
async fn pairing_http_user_agent_overlength_and_non_ascii_rejected_without_claim() {
    let mut f = Fixture::new().await;
    let (id, secret) = f.create().await;
    for ua in ["x".repeat(257), "手機".into()] {
        let (status, body) = f.post_ua("/api/pair/claim", claim(&id, &secret), &ua).await;
        assert_eq!(status, if ua.is_ascii() { 403 } else { 400 });
        assert!(!body.contains(&ua));
        assert!(!body.contains(&secret));
    }
    assert_eq!(f.post("/api/pair/claim", claim(&id, &secret)).await.0, 200);
    assert_eq!(f.event().await["type"], "pairing.claimed");
}
