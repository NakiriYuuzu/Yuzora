//! Real loopback WS clients; software WebAuthn ceremonies reuse the 2B fixture.
use super::*;
use futures_util::{SinkExt, StreamExt};
use std::future::Future;
use tokio_tungstenite::{
    tungstenite::{client::IntoClientRequest, Message},
    WebSocketStream,
};
type Ws = WebSocketStream<TcpStream>;

// Keep a paused runtime runnable while actual OS IO settles. A bounded poll
// budget fails deterministically rather than auto-advancing unrelated deadlines.
async fn io<T>(future: impl Future<Output = T>) -> T {
    tokio::pin!(future);
    for _ in 0..100000 {
        tokio::select! {
            biased;
            result = &mut future => return result,
            _ = tokio::task::yield_now() => {},
        }
    }
    panic!("loopback WS IO deadline");
}
async fn settle() {
    for _ in 0..200 {
        tokio::task::yield_now().await;
    }
}
async fn connect(
    f: &Fixture,
    login: Option<&Login>,
) -> Result<Ws, tokio_tungstenite::tungstenite::Error> {
    let mut request = "ws://desktop.tailnet.ts.net:8443/ws"
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("origin", ORIGIN.parse().unwrap());
    request
        .headers_mut()
        .insert("yzc-proxy-secret", PROXY_SECRET.parse().unwrap());
    request
        .headers_mut()
        .insert("yzc-peer-login", "user@example.com".parse().unwrap());
    if let Some(login) = login {
        request
            .headers_mut()
            .insert("cookie", login.cookie.parse().unwrap());
    }
    let socket = io(TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, f.port)))
        .await
        .unwrap();
    io(tokio_tungstenite::client_async(request, socket))
        .await
        .map(|(ws, response)| {
            assert_eq!(response.status(), 101);
            assert_eq!(response.headers()["cache-control"], "no-store");
            ws
        })
}
async fn send(ws: &mut Ws, value: Value) {
    io(ws.send(Message::text(value.to_string()))).await.unwrap();
}
async fn next(ws: &mut Ws) -> Message {
    io(ws.next()).await.unwrap().unwrap()
}
async fn value(ws: &mut Ws) -> Value {
    match next(ws).await {
        Message::Text(text) => serde_json::from_str(&text).unwrap(),
        other => panic!("expected text, got {other:?}"),
    }
}
async fn close(ws: &mut Ws, code: u16) {
    loop {
        match next(ws).await {
            Message::Close(Some(frame)) => {
                assert_eq!(u16::from(frame.code), code);
                assert!(frame.reason.is_empty());
                break;
            }
            Message::Text(_) | Message::Ping(_) | Message::Pong(_) => {}
            other => panic!("expected close, got {other:?}"),
        }
    }
}
// The server half-closes and drains after its close frame, so a peer whose
// input crossed that frame still reads it. Windows would otherwise reset the
// socket and discard the unread close frame.
async fn close_clean(ws: &mut Ws, code: u16) {
    close(ws, code).await;
    assert!(
        io(ws.next()).await.is_none(),
        "close must end in a clean EOF, not a reset"
    );
}
async fn authenticated(f: &Fixture, login: &Login) -> Ws {
    let mut ws = connect(f, Some(login)).await.unwrap();
    send(&mut ws, json!({"type":"auth","binding":login.binding})).await;
    assert_eq!(
        value(&mut ws).await,
        json!({"event":"session.ready","data":{"protocolVersion":1}})
    );
    ws
}
async fn identity(f: &Fixture) -> (Authenticator, String, Login) {
    let auth = Authenticator::new("ES256", 7, false);
    let device = f.register(&auth, None).await;
    let login = f.login(&auth, &device, 11).await;
    (auth, device, login)
}
async fn rpc(ws: &mut Ws, method: &str) -> Value {
    send(ws, json!({"id":7,"method":method,"params":{}})).await;
    let response = value(ws).await;
    assert_eq!(response["id"], 7);
    response
}
async fn tick_pong(ws: &mut Ws) {
    tokio::time::advance(Duration::from_secs(20)).await;
    assert!(matches!(next(ws).await, Message::Ping(_)));
    io(ws.flush()).await.unwrap(); // flush tungstenite's automatic matching pong
    settle().await;
}

#[tokio::test(start_paused = true)]
async fn ws_guarded_upgrade_and_query_rejection() {
    let f = Fixture::new("o2", 5).await;
    for (omit, path, expected) in [
        ("origin", "/ws", 403),
        ("login", "/ws", 403),
        ("host", "/ws", 421),
        ("", "/ws?binding=DO-NOT-ECHO", 400),
    ] {
        let mut headers = String::new();
        if omit != "host" {
            headers.push_str("Host: desktop.tailnet.ts.net:8443\r\n");
        }
        if omit != "login" {
            headers.push_str(&format!(
                "Yzc-Proxy-Secret: {PROXY_SECRET}\r\nYzc-Peer-Login: user@example.com\r\n"
            ));
        }
        if omit != "origin" {
            headers.push_str(&format!("Origin: {ORIGIN}\r\n"));
        }
        let mut socket = io(TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, f.port)))
            .await
            .unwrap();
        let request = format!("GET {path} HTTP/1.1\r\n{headers}Connection: upgrade, close\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n");
        io(socket.write_all(request.as_bytes())).await.unwrap();
        let mut response = String::new();
        io(socket.read_to_string(&mut response)).await.unwrap();
        assert!(
            response.starts_with(&format!("HTTP/1.1 {expected}")),
            "{response}"
        );
        assert!(!response.contains("DO-NOT-ECHO"));
    }
}
#[tokio::test(start_paused = true)]
async fn ws_first_frame_auth_required_timeout_garbage_cookie_only_wrong_binding() {
    let f = Fixture::new("o2", 5).await;
    let (_, _, login) = identity(&f).await;
    for case in [
        "timeout",
        "garbage",
        "wrong-type",
        "cookie-only",
        "wrong-binding",
        "rpc-first",
        "binary",
        "duplicate-binding",
    ] {
        let mut ws = connect(&f, Some(&login)).await.unwrap();
        match case {
            "timeout" => {
                settle().await;
                tokio::time::advance(Duration::from_secs(4)).await;
                settle().await;
                use futures_util::FutureExt;
                assert!(
                    ws.next().now_or_never().is_none(),
                    "first-frame deadline is five seconds"
                );
                tokio::time::advance(Duration::from_secs(1)).await;
            }
            "garbage" => io(ws.send(Message::text("{bad"))).await.unwrap(),
            "wrong-type" => send(&mut ws, json!({"type":"not-auth","binding":login.binding})).await,
            "cookie-only" => send(&mut ws, json!({"type":"auth"})).await,
            "wrong-binding" => {
                send(&mut ws, json!({"type":"auth","binding":B64.encode([0;32])})).await
            }
            "rpc-first" => send(&mut ws, json!({"id":7,"method":"hello","params":{}})).await,
            "binary" => io(ws.send(Message::binary(vec![0; 4]))).await.unwrap(),
            "duplicate-binding" => io(ws.send(Message::text(format!(
                "{{\"type\":\"auth\",\"binding\":\"{}\",\"binding\":\"{}\"}}",
                login.binding, login.binding
            ))))
            .await
            .unwrap(),
            _ => send(&mut ws, json!({"type":"auth","binding":login.binding})).await,
        }
        close(&mut ws, 4401).await;
    }
}
#[tokio::test(start_paused = true)]
async fn ws_hello_unknown_method_rpc_scope_checked_each_call_with_real_elevation() {
    let f = Fixture::new("o2", 5).await;
    let a = Authenticator::new("ES256", 7, false);
    let device = f.register(&a, None).await;
    f.grant(&device);
    let login = f.login(&a, &device, 11).await;
    let mut ws = authenticated(&f, &login).await;
    let hello = rpc(&mut ws, "hello").await;
    assert_eq!(hello["result"]["protocolVersion"], 1);
    assert_eq!(hello["result"]["scopes"], json!(["view"]));
    assert_eq!(hello["result"]["serverTime"], "2030-01-01T00:00:00Z");
    assert_eq!(
        rpc(&mut ws, "server.stop").await["error"]["code"],
        "unknown-method"
    );
    assert!(rpc(&mut ws, "session.ping").await["result"]["serverTime"].is_string());
    assert_eq!(
        rpc(&mut ws, "terminal.control").await["error"]["code"],
        "elevation-required"
    );
    assert_eq!(f.elevate(&a, &login, "control", 12).await.status, 200);
    assert_eq!(
        rpc(&mut ws, "terminal.control").await["error"]["code"],
        "invalid-params"
    );
    assert_eq!(
        rpc(&mut ws, "hello").await["result"]["scopes"],
        json!(["view", "control"])
    );
    assert_eq!(
        rpc(&mut ws, "full.open").await["error"]["code"],
        "elevation-required"
    );
    // Elevation expires on the SAME socket; never cache the original scope proof.
    for _ in 0..45 {
        tick_pong(&mut ws).await;
    }
    assert_eq!(
        rpc(&mut ws, "terminal.control").await["error"]["code"],
        "elevation-required"
    );
    assert_eq!(
        rpc(&mut ws, "hello").await["result"]["scopes"],
        json!(["view"])
    );
}
#[tokio::test(start_paused = true)]
async fn ws_active_revoke_downgrade_unlock_logout_and_added_credential_close_without_rpc() {
    for action in ["revoke", "downgrade", "unlock", "logout", "add-credential"] {
        let f = Fixture::new("o2", 5).await;
        let a = Authenticator::new("ES256", 7, false);
        let device = f.register(&a, None).await;
        f.grant(&device);
        let login = f.login(&a, &device, 11).await;
        let mut ws = authenticated(&f, &login).await;
        match action {
            "revoke" => f.admin(json!({"v":1,"type":"device.revoke","deviceId":device})),
            "downgrade" => f.admin(json!({"v":1,"type":"device.update","deviceId":device,"grants":{"control":null,"full":false}})),
            "unlock" => f.admin(json!({"v":1,"type":"device.unlock","deviceId":device})),
            "logout" => assert_eq!(f.post(&format!("{AUTH}logout"), json!({}), Some(&login)).await.status, 204),
            _ => { f.register(&Authenticator::new("EdDSA", 8, false), Some(&device)).await; },
        }
        close(&mut ws, 4401).await;
        let opened = f.event("session.opened").await;
        let closed = f.event("session.closed").await;
        assert_eq!(opened["deviceId"], device);
        assert_eq!(opened["peerId"], closed["peerId"]);
        assert_eq!(closed["reason"], "session-invalid");
        for event in [opened, closed] {
            let text = event.to_string();
            assert!(!text.contains(&login.binding));
            assert!(!text.contains(login.cookie.split('=').nth(1).unwrap()));
        }
    }
}
#[tokio::test(start_paused = true)]
async fn ws_active_lockout_five_failed_assertions_and_rollback() {
    for rollback in [false, true] {
        let f = Fixture::new("o2", 5).await;
        let (a, device, login) = identity(&f).await;
        let mut ws = authenticated(&f, &login).await;
        for _ in 0..if rollback { 1 } else { 5 } {
            let credential = f
                .assertion(
                    &a,
                    &device,
                    if rollback { 10 } else { 12 },
                    if rollback { "valid" } else { "origin" },
                )
                .await;
            assert_eq!(
                f.post(
                    &format!("{AUTH}finish"),
                    json!({"credential":credential}),
                    None
                )
                .await
                .status,
                403
            );
        }
        assert!(
            f.gateway
                .identity
                .lock()
                .unwrap()
                .device(&device)
                .unwrap()
                .locked
        );
        close(&mut ws, 4401).await;
    }
}
#[tokio::test(start_paused = true)]
async fn ws_active_fifth_session_eviction_without_next_rpc() {
    let f = Fixture::new("o2", 5).await;
    let (a, device, login) = identity(&f).await;
    let mut ws = authenticated(&f, &login).await;
    for count in 12..=15 {
        tokio::time::advance(Duration::from_secs(1)).await;
        f.login(&a, &device, count).await;
    }
    close(&mut ws, 4401).await;
}
#[tokio::test(start_paused = true)]
async fn ws_active_idle_absolute_and_enable_until_expiry() {
    for case in ["idle", "absolute", "enable"] {
        let mut init: Value = serde_json::from_str(&init_fixture::json()).unwrap();
        init["policy"]["viewIdleMin"] = json!(1);
        init["policy"]["viewAbsoluteHours"] = json!(1);
        if case == "enable" {
            init["policy"]["enableUntil"] = json!("2030-01-01T00:00:45Z");
        }
        let f = Fixture::from_init(init).await;
        let (_, _, login) = identity(&f).await;
        let mut ws = authenticated(&f, &login).await;
        match case {
            "enable" => {
                tick_pong(&mut ws).await;
                tick_pong(&mut ws).await;
                tokio::time::advance(Duration::from_secs(5)).await;
            }
            "idle" => {
                tick_pong(&mut ws).await;
                tick_pong(&mut ws).await;
                tokio::time::advance(Duration::from_secs(20)).await;
            }
            _ => {
                for _ in 0..179 {
                    tick_pong(&mut ws).await;
                    rpc(&mut ws, "session.ping").await;
                }
                tokio::time::advance(Duration::from_secs(20)).await;
            }
        }
        close(&mut ws, 4401).await;
        if case == "enable" {
            settle().await;
            // The peers are gone but the port stays bound until the hold ends:
            // the node forwards the tailnet to it until the desktop unserves.
            assert!(!f.server.is_finished());
            assert!(std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, f.port)).is_err());
            tokio::time::advance(yuzora_companion::http::EXPIRY_HOLD).await;
            settle().await;
            assert!(f.server.is_finished());
            // Not io(): Windows retries a refused loopback connect for about
            // two seconds, beyond the yield budget. The server has finished.
            assert!(TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, f.port))
                .await
                .is_err());
        }
    }
}
#[tokio::test(start_paused = true)]
async fn ws_shutdown_1001_and_http_lifetime_does_not_apply() {
    let f = Fixture::new("o2", 5).await;
    let (_, _, login) = identity(&f).await;
    let mut ws = authenticated(&f, &login).await;
    for _ in 0..4 {
        tick_pong(&mut ws).await;
    }
    // 80s > HTTP 60s lifetime, with no intervening RPC that could refresh it.
    assert_eq!(rpc(&mut ws, "hello").await["result"]["protocolVersion"], 1);
    // Drop fixture's shutdown sender without aborting the server task.
    let mut f = f;
    let (replacement, _) = oneshot::channel();
    std::mem::replace(&mut f._stop, replacement)
        .send(())
        .unwrap();
    close(&mut ws, 1001).await;
    settle().await;
    assert!(f.server.is_finished());
    assert_eq!(f.event("session.closed").await["reason"], "shutdown");
}
#[tokio::test(start_paused = true)]
async fn ws_ping_pong_timeout_oversize_rate_abuse_and_binary_reserved() {
    for case in ["pong-timeout", "oversize", "rate", "binary"] {
        let f = Fixture::new("o2", 5).await;
        let (_, _, login) = identity(&f).await;
        let mut ws = authenticated(&f, &login).await;
        let code = match case {
            "pong-timeout" => {
                tokio::time::advance(Duration::from_secs(20)).await;
                assert!(matches!(next(&mut ws).await, Message::Ping(_)));
                // Do NOT flush the client's automatic pong.
                tokio::time::advance(Duration::from_secs(20)).await;
                settle().await;
                1001
            }
            "oversize" => {
                io(ws.send(Message::text("x".repeat(65537)))).await.unwrap();
                1009
            }
            "rate" => {
                for _ in 0..51 {
                    send(&mut ws, json!({"id":1,"method":"session.ping","params":{}})).await;
                }
                1008
            }
            _ => {
                io(ws.send(Message::binary(vec![0; 4]))).await.unwrap();
                1003
            }
        };
        close_clean(&mut ws, code).await;
    }
}
#[tokio::test(start_paused = true)]
async fn ws_per_device_permits_release_after_close_error_timeout_and_revoke() {
    let f = Fixture::new("o2", 5).await;
    let (a, device, login) = identity(&f).await;
    let other_login = f.login(&a, &device, 12).await;
    let mut first = authenticated(&f, &login).await;
    let second = authenticated(&f, &other_login).await;
    let mut third = connect(&f, Some(&login)).await.unwrap();
    send(&mut third, json!({"type":"auth","binding":login.binding})).await;
    close_clean(&mut third, 1008).await;
    io(first.close(None)).await.unwrap();
    settle().await;
    let replacement = authenticated(&f, &login).await;
    drop(second); // TCP error/drop must release the device slot as well.
    settle().await;
    let mut replacement2 = authenticated(&f, &login).await;
    drop(replacement);
    settle().await;
    tokio::time::advance(Duration::from_secs(20)).await;
    settle().await;
    tokio::time::advance(Duration::from_secs(20)).await;
    close_clean(&mut replacement2, 1001).await;
    settle().await;
    let _after_timeout = authenticated(&f, &login).await;
}
#[tokio::test(start_paused = true)]
async fn ws_global_permits_include_preauth_release_and_http_separate() {
    let f = Fixture::new("o2", 5).await;
    let (_, _, login) = identity(&f).await;
    let mut sockets = Vec::new();
    for _ in 0..16 {
        sockets.push(connect(&f, Some(&login)).await.unwrap());
    }
    let error = connect(&f, Some(&login)).await.unwrap_err();
    assert!(
        matches!(error, tokio_tungstenite::tungstenite::Error::Http(response) if response.status() == 503)
    );
    // WS saturation does not consume the 64 HTTP slots.
    assert_eq!(
        f.post(&format!("{AUTH}options"), json!({}), None)
            .await
            .status,
        200
    );
    // All 64 HTTP slots remain available even while the 16 WS slots are full.
    let mut http = Vec::new();
    for _ in 0..64 {
        let mut connection = io(TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, f.port)))
            .await
            .unwrap();
        io(connection.write_all(b"GET / HTTP/1.1\r\n"))
            .await
            .unwrap();
        http.push(connection);
    }
    let mut last = http.pop().unwrap();
    io(last.write_all(format!("Host: desktop.tailnet.ts.net:8443\r\nYzc-Proxy-Secret: {PROXY_SECRET}\r\nYzc-Peer-Login: user@example.com\r\nConnection: close\r\n\r\n").as_bytes())).await.unwrap();
    let mut response = String::new();
    io(last.read_to_string(&mut response)).await.unwrap();
    assert!(
        response.starts_with("HTTP/1.1 404"),
        "HTTP pool is independent: {response}"
    );
    drop(http);
    drop(sockets.pop());
    settle().await;
    sockets.push(connect(&f, Some(&login)).await.unwrap());
    settle().await;
    tokio::time::advance(Duration::from_secs(5)).await;
    for ws in &mut sockets {
        close(ws, 4401).await;
    }
    settle().await;
    let _replacement = connect(&f, Some(&login)).await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn ws_scope_not_granted_distinct_from_elevation_and_unauthenticated() {
    for disabled in [false, true] {
        let mut init: Value = serde_json::from_str(&init_fixture::json()).unwrap();
        init["policy"]["fullSessionEnabled"] = json!(!disabled);
        let f = Fixture::from_init(init).await;
        let a = Authenticator::new("ES256", 7, false);
        let device = f.register(&a, None).await;
        if disabled {
            f.grant(&device);
        }
        let login = f.login(&a, &device, 11).await;
        let mut ws = authenticated(&f, &login).await;
        for scope in if disabled {
            vec!["full"]
        } else {
            vec!["control", "full"]
        } {
            let reply = f
                .post(
                    &format!("{AUTH}elevate/options"),
                    json!({"scope":scope}),
                    Some(&login),
                )
                .await;
            assert_eq!(reply.status, 403);
            assert_eq!(reply.body, "Scope not granted");
            let finish = f
                .post(
                    &format!("{AUTH}elevate/finish"),
                    json!({"scope":scope,"credential":{}}),
                    Some(&login),
                )
                .await;
            assert_eq!(finish.status, 403);
            assert_eq!(finish.body, "Scope not granted");
            let method = if scope == "control" {
                "terminal.control"
            } else {
                "full.open"
            };
            let reply = rpc(&mut ws, method).await;
            assert_eq!(reply["error"]["code"], "scope-not-granted");
            assert_eq!(reply["error"]["message"], "Scope not granted");
            assert_eq!(
                f.post(
                    &format!("{AUTH}elevate/options"),
                    json!({"scope":scope}),
                    None
                )
                .await
                .status,
                401
            );
        }
        f.admin(json!({"v":1,"type":"device.revoke","deviceId":device}));
        close(&mut ws, 4401).await;
    }
}

#[tokio::test(start_paused = true)]
async fn ws_client_close_reply_and_clean_eof() {
    use tokio_tungstenite::tungstenite::protocol::CloseFrame;
    let f = Fixture::new("o2", 5).await;
    let (_, _, login) = identity(&f).await;
    for frame in [
        None,
        Some(CloseFrame {
            code: 1000.into(),
            reason: "".into(),
        }),
    ] {
        let mut ws = authenticated(&f, &login).await;
        io(ws.close(frame.clone())).await.unwrap();
        assert_eq!(next(&mut ws).await, Message::Close(frame));
        assert!(
            io(ws.next()).await.is_none(),
            "close handshake must end cleanly"
        );
        assert_eq!(f.event("session.closed").await["reason"], "client-closed");
    }
}

#[tokio::test(start_paused = true)]
async fn ws_preauth_cookie_filter_rejects_without_exhausting_pool() {
    let f = Fixture::new("o2", 5).await;
    let (_, _, login) = identity(&f).await;
    for _ in 0..16 {
        assert!(
            matches!(connect(&f, None).await, Err(tokio_tungstenite::tungstenite::Error::Http(r)) if r.status() == 401)
        );
    }
    for cookie in [
        format!("__Host-yzc={}", B64.encode([0; 32])),
        "__Host-yzc=bad".into(),
        format!("{}; {}", login.cookie, login.cookie),
    ] {
        let fake = Login {
            cookie,
            binding: login.binding.clone(),
            device_id: login.device_id.clone(),
        };
        assert!(
            matches!(connect(&f, Some(&fake)).await, Err(tokio_tungstenite::tungstenite::Error::Http(r)) if r.status() == 401)
        );
    }
    // All 16 slots remain usable; the cookie is not authorization and these
    // peers deliberately never send binding. Device cap applies only after auth.
    let mut sockets = Vec::new();
    for _ in 0..16 {
        sockets.push(connect(&f, Some(&login)).await.unwrap());
    }
    assert!(
        matches!(connect(&f, Some(&login)).await, Err(tokio_tungstenite::tungstenite::Error::Http(r)) if r.status() == 503)
    );
}

#[tokio::test(start_paused = true)]
async fn ws_invalid_utf8_text_closes_1007() {
    let f = Fixture::new("o2", 5).await;
    let (_, _, login) = identity(&f).await;
    let mut ws = authenticated(&f, &login).await;
    // Masked final Text containing invalid UTF-8; the client library rightly
    // cannot construct Message::Text from this payload.
    io(ws.get_mut().write_all(&[0x81, 0x81, 0, 0, 0, 0, 0xff]))
        .await
        .unwrap();
    close(&mut ws, 1007).await;
    assert_eq!(f.event("session.closed").await["reason"], "invalid-utf8");
}

#[tokio::test(start_paused = true)]
async fn ws_client_ping_gets_exactly_one_automatic_pong() {
    let f = Fixture::new("o2", 5).await;
    let (_, _, login) = identity(&f).await;
    let mut ws = authenticated(&f, &login).await;
    io(ws.send(Message::Ping(b"client-probe".as_slice().into())))
        .await
        .unwrap();
    assert_eq!(
        next(&mut ws).await,
        Message::Pong(b"client-probe".as_slice().into())
    );
    settle().await;
    // A following RPC is an ordered barrier: a redundant queued manual Pong
    // must not appear before the RPC response.
    assert!(rpc(&mut ws, "session.ping").await["result"]["serverTime"].is_string());
}
