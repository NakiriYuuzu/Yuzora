#![cfg(unix)]
//! Actual gateway process + isolated fake Herdr. Never browser automation.
#[path = "support/init.rs"]
mod init_fixture;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD as B64, Engine};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Read, Write},
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};
#[path = "support/authenticator.rs"]
mod authenticator;
#[path = "support/fake_herdr.rs"]
mod fake_herdr;
struct Process {
    child: Child,
    events: mpsc::Receiver<Value>,
    reader: Option<std::thread::JoinHandle<()>>,
}
impl Process {
    fn new(root: &std::path::Path) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_yuzora-companion"));
        for (key, _) in std::env::vars_os() {
            if key
                .to_string_lossy()
                .to_ascii_uppercase()
                .starts_with("HERDR_")
            {
                command.env_remove(key);
            }
        }
        // Explicit poison values in the test child only. No live socket/path is
        // inherited; main must remove ALL of these before its runtime exists.
        for key in [
            "HERDR_SOCKET_PATH",
            "HERDR_CLIENT_SOCKET_PATH",
            "HERDR_SESSION",
            "HERDR_PANE_ID",
            "HERDR_ENV",
            "HERDR_CONFIG_PATH",
            "HERDR_UNKNOWN_FUTURE",
            "herdr_socket_path", // Windows case-insensitive environment alias
        ] {
            command.env(key, "FAKE-POISON-DO-NOT-USE");
        }
        for key in [
            "HOME",
            "XDG_CONFIG_HOME",
            "XDG_STATE_HOME",
            "XDG_DATA_HOME",
            "XDG_RUNTIME_DIR",
            "XDG_CACHE_HOME",
        ] {
            command.env(key, root);
        }
        command.env_remove("YZC_TEST_TIMING");
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (send, events) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                if send
                    .send(serde_json::from_str(&line.unwrap()).unwrap())
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            child,
            events,
            reader: Some(reader),
        }
    }
    fn send(&mut self, value: Value) {
        let stdin = self.child.stdin.as_mut().unwrap();
        writeln!(stdin, "{value}").unwrap();
        stdin.flush().unwrap();
    }
    fn event(&self, kind: &str) -> Value {
        loop {
            let event = self.events.recv_timeout(Duration::from_secs(5)).unwrap();
            if event["type"] == kind {
                return event;
            }
            assert_ne!(event["type"], "error", "{event}");
        }
    }
    async fn exited(&mut self) {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Some(status) = self.child.try_wait().unwrap() {
                    assert!(status.success());
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        let mut stderr = String::new();
        self.child
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut stderr)
            .unwrap();
        assert!(stderr.is_empty(), "{stderr}");
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}
async fn post(port: u16, path: &str, body: Value) -> (Value, String) {
    let mut socket = TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
        .await
        .unwrap();
    socket.write_all(format!("POST {path} HTTP/1.1\r\nHost: desktop.tailnet.ts.net:8443\r\nOrigin: https://desktop.tailnet.ts.net:8443\r\nTailscale-User-Login: user@example.com\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.to_string().len()).as_bytes()).await.unwrap();
    let mut text = String::new();
    tokio::time::timeout(Duration::from_secs(3), socket.read_to_string(&mut text))
        .await
        .unwrap()
        .unwrap();
    let (head, body) = text.split_once("\r\n\r\n").unwrap();
    assert!(head.starts_with("HTTP/1.1 200"), "{text}");
    (serde_json::from_str(body).unwrap(), head.into())
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn herdr_process_clears_inherited_routing_env_and_shutdown_reaps_fake_child() {
    let local=fake_herdr::FakeHerdr::new(json!({"workspaces":[],"tabs":[],"agents":[],"panes":[{"pane_id":"w:p","terminal_id":"term","workspace_id":"w","tab_id":"w:t"}],"layouts":[{"panes":[{"pane_id":"w:p","rect":{"width":120,"height":40}}]}]})).await;
    let mut p = Process::new(local.root.path());
    let mut init: Value = serde_json::from_str(&init_fixture::json()).unwrap();
    init["herdrBinary"] = json!(local.binary);
    init["webRoot"] = json!(local.root.path());
    init["sessionAllowlist"] = json!(["work"]);
    p.send(init);
    let port = p.event("ready")["ports"]["loopback"].as_u64().unwrap() as u16;
    p.send(json!({"v":1,"type":"pairing.create","path":"tailscale"}));
    let created = p.event("pairing.created");
    let (id, secret) = created["url"]
        .as_str()
        .unwrap()
        .split_once("#p=")
        .unwrap()
        .1
        .split_once('.')
        .unwrap();
    let (claim,_)=post(port,"/api/pair/claim",json!({"pairingId":id,"secret":secret,"deviceLabel":"Phone","deviceNonce":B64.encode([7;32])})).await;
    p.send(json!({"v":1,"type":"pairing.approve","pairingId":id}));
    p.event("pairing.state");
    let (status, _) = post(
        port,
        "/api/pair/status",
        json!({"pairingId":id,"claimToken":claim["claimToken"]}),
    )
    .await;
    let (options, _) = post(
        port,
        "/api/pair/webauthn/options",
        json!({"pairingId":id,"registrationTicket":status["registrationTicket"]}),
    )
    .await;
    let auth = authenticator::Authenticator::new("ES256", 7, false);
    let (device, _) = post(
        port,
        "/api/pair/webauthn/finish",
        json!({"pairingId":id,"credential":auth.response(&options,true,10,"valid")}),
    )
    .await;
    let (mut options, _) = post(port, "/api/auth/options", json!({})).await;
    options["user"] = json!({"id":device["deviceId"]});
    let (login, headers) = post(
        port,
        "/api/auth/finish",
        json!({"credential":auth.response(&options,false,11,"valid")}),
    )
    .await;
    let cookie = headers
        .lines()
        .find_map(|line| line.strip_prefix("set-cookie: "))
        .unwrap()
        .split(';')
        .next()
        .unwrap();
    let mut request = "ws://desktop.tailnet.ts.net:8443/ws"
        .into_client_request()
        .unwrap();
    for (key, value) in [
        ("origin", "https://desktop.tailnet.ts.net:8443"),
        ("tailscale-user-login", "user@example.com"),
        ("cookie", cookie),
    ] {
        request.headers_mut().insert(key, value.parse().unwrap());
    }
    let (mut ws, _) = tokio_tungstenite::client_async(
        request,
        TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
            .await
            .unwrap(),
    )
    .await
    .unwrap();
    ws.send(Message::text(
        json!({"type":"auth","binding":login["binding"]}).to_string(),
    ))
    .await
    .unwrap();
    assert!(matches!(
        ws.next().await.unwrap().unwrap(),
        Message::Text(_)
    ));
    ws.send(Message::text(json!({"id":1,"method":"terminal.observe","params":{"session":"work","target":"w:p","cols":1,"rows":1}}).to_string())).await.unwrap();
    let Message::Text(reply) = tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
    else {
        panic!("open reply")
    };
    let reply: Value = serde_json::from_str(&reply).unwrap();
    assert_eq!(reply["result"]["cols"], 120, "{reply}");
    let Message::Binary(bytes) = tokio::time::timeout(Duration::from_secs(3), ws.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
    else {
        panic!("fake frame")
    };
    assert_eq!(&bytes[4..], b"fake frame");
    p.send(json!({"v":1,"type":"shutdown"}));
    assert!(
        matches!(ws.next().await.unwrap().unwrap(),Message::Close(Some(frame)) if u16::from(frame.code)==1001)
    );
    p.exited().await;
    local.assert_released().await;
    local.assert_environment();
    assert_eq!(local.children().len(), 1);
}
