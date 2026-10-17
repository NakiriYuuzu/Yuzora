use crate::{
    admin::{self, EventSender, Init},
    clock::Clock,
    gateway::Gateway,
};
use serde_json::Value;
use std::sync::Arc;
use tokio::{
    io::{AsyncBufReadExt, BufReader, DuplexStream},
    task::JoinHandle,
};

pub struct Fixture {
    pub gateway: Arc<Gateway>,
    pub events: EventSender,
    pub reader: BufReader<DuplexStream>,
    writer: JoinHandle<std::io::Result<()>>,
}
impl Fixture {
    pub fn init() -> Init {
        serde_json::from_str(&crate::test_support::init::json()).unwrap()
    }
    pub fn new() -> Self {
        Self::with_init(Self::init())
    }
    pub fn with_init(init: Init) -> Self {
        let (output, input) = tokio::io::duplex(65536);
        let (events, writer) = admin::spawn_writer(output);
        let clock = Clock::at(serde_json::from_str("\"2030-01-01T00:00:00Z\"").unwrap());
        let gateway = Gateway::new(&init, events.clone(), clock).unwrap();
        Self {
            gateway,
            events,
            reader: BufReader::new(input),
            writer,
        }
    }
    pub async fn event(&mut self) -> Value {
        let mut line = String::new();
        self.reader.read_line(&mut line).await.unwrap();
        serde_json::from_str(&line).unwrap()
    }
    pub async fn create(&mut self) -> (String, String) {
        self.gateway
            .pairings
            .command(admin::PairingCommand::Create {
                v: 1,
                path: "tailscale".into(),
                add_to_device: None,
            })
            .unwrap();
        let event = self.event().await;
        assert_eq!(event["type"], "pairing.created");
        let url = event["url"].as_str().unwrap();
        assert!(url.starts_with("https://desktop.tailnet.ts.net:8443/pair#p="));
        let (id, secret) = url.split_once("#p=").unwrap().1.split_once('.').unwrap();
        assert_eq!(id, event["pairingId"]);
        (id.into(), secret.into())
    }
    pub fn claim_request(id: &str, secret: &str) -> crate::pairing::ClaimRequest {
        crate::pairing::ClaimRequest {
            pairing_id: id.into(),
            secret: secret.into(),
            device_label: "Test phone".into(),
            device_nonce: crate::secrets::encode(&[7; 32]),
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.writer.abort();
    }
}

/// Real loopback HTTP with paused-clock-safe bounded polling, for unit-only seams.
pub async fn post(port: u16, path: &str, body: Value, headers: &str) -> (u16, Value, String) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let request = format!("POST {path} HTTP/1.1\r\nHost: desktop.tailnet.ts.net:8443\r\nTailscale-User-Login: user@example.com\r\nOrigin: https://desktop.tailnet.ts.net:8443\r\n{headers}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.to_string().len());
    let task = tokio::spawn(async move {
        let mut socket = tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
            .await
            .unwrap();
        socket.write_all(request.as_bytes()).await.unwrap();
        let mut response = String::new();
        socket.read_to_string(&mut response).await.unwrap();
        response
    });
    for _ in 0..100000 {
        if task.is_finished() {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(task.is_finished(), "unit loopback response deadline");
    let response = task.await.unwrap();
    let (headers, body) = response.split_once("\r\n\r\n").unwrap();
    assert!(headers.contains("cache-control: no-store"));
    (
        headers.split_whitespace().nth(1).unwrap().parse().unwrap(),
        serde_json::from_str(body).unwrap_or(serde_json::json!(body)),
        headers.to_owned(),
    )
}

#[path = "../tests/support/authenticator.rs"]
pub mod authenticator;

// Read-only fakes fail loudly if a new write ever crosses their boundary.
macro_rules! reject_writes {
    () => {
        fn control(
            &self,
            _: &str,
            _: &str,
            _: (u16, u16),
            _: bool,
            _: yuzora_host::herdr_service::OnTerminalEvent,
        ) -> crate::herdr::Result<String> {
            panic!("unexpected control")
        }
        fn full(
            &self,
            _: &str,
            _: (u16, u16),
            _: yuzora_host::herdr_service::OnTerminalEvent,
        ) -> crate::herdr::Result<String> {
            panic!("unexpected full")
        }
        fn input(&self, _: &str, _: Vec<u8>) -> crate::herdr::Result<()> {
            panic!("unexpected input")
        }
        fn scroll(
            &self,
            _: &str,
            _: yuzora_host::herdr_service::HerdrScrollDirection,
            _: u32,
        ) -> crate::herdr::Result<()> {
            panic!("unexpected scroll")
        }
        fn resize(&self, _: &str, _: (u16, u16)) -> crate::herdr::Result<()> {
            panic!("unexpected resize")
        }
        fn prompt(&self, _: &str, _: &str, _: String) -> crate::herdr::Result<()> {
            panic!("unexpected prompt")
        }
        fn keys(&self, _: &str, _: &str, _: Vec<String>) -> crate::herdr::Result<()> {
            panic!("unexpected keys")
        }
    };
}
pub(crate) use reject_writes;

#[path = "../tests/support/init.rs"]
pub mod init;

#[cfg(unix)]
#[path = "../tests/support/fake_herdr.rs"]
pub mod fake_herdr;
