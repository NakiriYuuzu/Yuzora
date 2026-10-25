#[path = "support/identity.rs"]
mod identity;
#[path = "support/init.rs"]
mod init_fixture;
use identity::PROXY_SECRET;
use std::time::Duration;
use tempfile::TempDir;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::oneshot,
};
use yuzora_companion::{
    admin::{Loopback, ProxySecret},
    guard::{BODY_LIMIT, RATE_BURST},
    http::{Server, SECURITY_HEADERS},
};

fn gateway() -> std::sync::Arc<yuzora_companion::gateway::Gateway> {
    let init = serde_json::from_str(&init_fixture::json()).unwrap();
    let (events, _writer) = yuzora_companion::admin::spawn_writer(tokio::io::sink());
    yuzora_companion::gateway::Gateway::new(
        &init,
        events,
        yuzora_companion::clock::Clock::default(),
    )
    .unwrap()
}

fn proxy_secret() -> ProxySecret {
    ProxySecret(zeroize::Zeroizing::new(PROXY_SECRET.into()))
}

struct Fixture {
    root: TempDir,
    port: u16,
    stop: Option<oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<bool>,
}
impl Fixture {
    async fn start() -> Self {
        let root = tempfile::tempdir().unwrap();
        for dir in ["assets", "icons"] {
            std::fs::create_dir(root.path().join(dir)).unwrap();
        }
        for file in [
            "index.html",
            "sw.js",
            "manifest.webmanifest",
            "assets/app.js",
            "icons/icon.svg",
        ] {
            std::fs::write(root.path().join(file), format!("fixture:{file}")).unwrap();
        }
        let config = Loopback {
            port: 0,
            public_host: "desktop.tailnet.ts.net:8443".into(),
            required_tailscale_login: "user@example.com".into(),
            proxy_secret: proxy_secret(),
        };
        let server = Server::bind(&config, root.path(), gateway()).await.unwrap();
        let port = server.port().unwrap();
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(server.run(stopped));
        Self {
            root,
            port,
            stop: Some(stop),
            task,
        }
    }
    async fn request(&self, request: &str) -> String {
        tokio::time::timeout(Duration::from_secs(20), async {
            let mut stream = TcpStream::connect(("127.0.0.1", self.port)).await.unwrap();
            stream.write_all(request.as_bytes()).await.unwrap();
            let mut response = Vec::new();
            if let Err(error) = stream.read_to_end(&mut response).await {
                // An early 413 can close while the peer still has an unread body.
                assert!(connection_closed(&error));
                // Windows may discard a queued early 413 when the peer resets.
                if !cfg!(windows) {
                    assert!(!response.is_empty());
                }
            }
            String::from_utf8(response).unwrap()
        })
        .await
        .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        self.task.abort();
    }
}
fn raw_request(method: &str, path: &str, headers: &str) -> String {
    format!("{method} {path} HTTP/1.1\r\n{headers}Connection: close\r\n\r\n")
}
// The sidecar's secret rides along; `login` varies only the peer identity.
fn request(method: &str, path: &str, host: &str, login: &str, extra: &str) -> String {
    raw_request(
        method,
        path,
        &format!("{host}Yzc-Proxy-Secret: {PROXY_SECRET}\r\n{login}{extra}"),
    )
}
const HOST: &str = "Host: desktop.tailnet.ts.net:8443\r\n";
const LOGIN: &str = "Yzc-Peer-Login: user@example.com\r\n";
const ORIGIN: &str = "Origin: https://desktop.tailnet.ts.net:8443\r\n";
fn status(response: &str) -> u16 {
    response.split_whitespace().nth(1).unwrap().parse().unwrap()
}
fn secured(response: &str) {
    let headers = response.split("\r\n\r\n").next().unwrap();
    for &(name, value) in SECURITY_HEADERS {
        assert!(
            headers.contains(&format!("{name}: {value}\r\n"))
                || headers.ends_with(&format!("{name}: {value}")),
            "missing {name}: {response}"
        );
    }
}

#[tokio::test]
async fn guard_malicious_request_table() {
    let f = Fixture::start().await;
    let cases = [
        ("GET", "/", HOST, LOGIN, "", 200),
        ("GET", "/", "Host: evil.example\r\n", LOGIN, "", 421),
        (
            "GET",
            "/",
            "Host: desktop.tailnet.ts.net.:8443\r\n",
            LOGIN,
            "",
            421,
        ),
        ("POST", "/", HOST, LOGIN, "Origin: null\r\n", 403),
        (
            "POST",
            "/",
            HOST,
            LOGIN,
            "Origin: http://desktop.tailnet.ts.net:8443\r\n",
            403,
        ),
        (
            "POST",
            "/",
            HOST,
            LOGIN,
            "Origin: https://desktop.tailnet.ts.net:8443/\r\n",
            403,
        ),
        ("GET", "/", "", LOGIN, "", 421),
        ("GET", "/", "Host: [::1]:8443\r\n", LOGIN, "", 421),
        (
            "GET",
            "/",
            "Host: desktop.tailnet.ts.net\r\n",
            LOGIN,
            "",
            421,
        ),
        (
            "GET",
            "/",
            "Host: DESKTOP.TAILNET.TS.NET:8443\r\n",
            LOGIN,
            "",
            200,
        ),
        ("GET", "/", HOST, "", "", 403),
        (
            "GET",
            "/",
            HOST,
            "Yzc-Peer-Login: other@example.com\r\n",
            "",
            403,
        ),
        (
            "GET",
            "/",
            HOST,
            "Yzc-Peer-Login: User@example.com\r\n",
            "",
            403,
        ),
        (
            "GET",
            "/",
            HOST,
            "yzc-peer-login: =?utf-8?q?user=40example.com?=\r\n",
            "",
            200,
        ),
        (
            "GET",
            "/",
            HOST,
            "Yzc-Peer-Login: =?UTF-8?B?dXNlckBleGFtcGxlLmNvbQ==?=\r\n",
            "",
            200,
        ),
        (
            "GET",
            "/",
            HOST,
            "Yzc-Peer-Login: =?UTF-8?Q?User=40example.com?=\r\n",
            "",
            403,
        ),
        (
            "GET",
            "/",
            HOST,
            "Yzc-Peer-Login: =?UTF-8?Q?=GG?=\r\n",
            "",
            403,
        ),
        ("GET", "/", HOST, LOGIN, LOGIN, 403),
        ("POST", "/api/auth/options", HOST, LOGIN, "", 403),
        (
            "POST",
            "/api/auth/options",
            HOST,
            LOGIN,
            "Origin: https://evil.example\r\n",
            403,
        ),
        ("HEAD", "/", HOST, LOGIN, "", 403),
        ("POST", "/api/auth/options", HOST, LOGIN, ORIGIN, 400),
        ("GET", "/ws", HOST, LOGIN, "Upgrade: websocket\r\n", 403),
        (
            "GET",
            "/ws",
            HOST,
            LOGIN,
            "Upgrade: websocket\r\nOrigin: https://evil.example\r\n",
            403,
        ),
        ("GET", "/ws", HOST, LOGIN, ORIGIN, 400),
        ("GET", "/", HOST, LOGIN, "Upgrade: websocket\r\n", 403),
        ("GET", "https://evil.example/", HOST, LOGIN, "", 421),
        ("GET", "/missing", HOST, LOGIN, "", 404),
    ];
    for (method, path, host, login, extra, expected) in cases {
        let response = f.request(&request(method, path, host, login, extra)).await;
        assert_eq!(
            status(&response),
            expected,
            "{method} {path} {host} {login}: {response}"
        );
        secured(&response);
        if expected >= 400 {
            assert!(response.contains("cache-control: no-store"));
        }
    }
    let response = f
        .request(&request(
            "POST",
            "/",
            HOST,
            LOGIN,
            &format!("{ORIGIN}{ORIGIN}"),
        ))
        .await;
    assert_eq!(status(&response), 403);
    secured(&response);
}

#[tokio::test]
async fn guard_proxy_secret_is_the_only_identity_channel() {
    let f = Fixture::start().await;
    let mut same_length = PROXY_SECRET.to_owned();
    same_length.replace_range(..1, "B");
    let secret = format!("Yzc-Proxy-Secret: {PROXY_SECRET}\r\n");
    let wrong = format!("Yzc-Proxy-Secret: {same_length}\r\n");
    let shorter = format!("Yzc-Proxy-Secret: {}\r\n", &PROXY_SECRET[..42]);
    let longer = format!("Yzc-Proxy-Secret: {PROXY_SECRET}A\r\n");
    let cases = [
        ("valid", format!("{secret}{LOGIN}"), 200),
        (
            "header names are case-insensitive",
            format!("yzc-proxy-secret: {PROXY_SECRET}\r\nyzc-peer-login: user@example.com\r\n"),
            200,
        ),
        ("missing secret", LOGIN.to_owned(), 403),
        ("empty secret", format!("Yzc-Proxy-Secret: \r\n{LOGIN}"), 403),
        ("wrong secret, same length", format!("{wrong}{LOGIN}"), 403),
        ("shorter secret", format!("{shorter}{LOGIN}"), 403),
        ("longer secret", format!("{longer}{LOGIN}"), 403),
        (
            "non-ASCII secret",
            format!("Yzc-Proxy-Secret: {}\u{e9}\r\n{LOGIN}", &PROXY_SECRET[..42]),
            403,
        ),
        ("duplicate secret", format!("{secret}{secret}{LOGIN}"), 403),
        ("correct then wrong secret", format!("{secret}{wrong}{LOGIN}"), 403),
        ("wrong then correct secret", format!("{wrong}{secret}{LOGIN}"), 403),
        ("missing login", secret.clone(), 403),
        (
            "wrong login",
            format!("{secret}Yzc-Peer-Login: other@example.com\r\n"),
            403,
        ),
        ("duplicate login", format!("{secret}{LOGIN}{LOGIN}"), 403),
        (
            "RFC 2047 login",
            format!("{secret}Yzc-Peer-Login: =?UTF-8?Q?user=40example.com?=\r\n"),
            200,
        ),
        (
            "old header alone",
            "Tailscale-User-Login: user@example.com\r\n".to_owned(),
            403,
        ),
        (
            "old header instead of the peer login",
            format!("{secret}Tailscale-User-Login: user@example.com\r\n"),
            403,
        ),
        (
            "old header cannot replace a wrong peer login",
            format!(
                "{secret}Yzc-Peer-Login: other@example.com\r\nTailscale-User-Login: user@example.com\r\n"
            ),
            403,
        ),
    ];
    for (name, identity, expected) in cases {
        let response = f
            .request(&raw_request("GET", "/", &format!("{HOST}{identity}")))
            .await;
        assert_eq!(status(&response), expected, "{name}: {response}");
        secured(&response);
        if expected >= 400 {
            assert!(!response.contains(PROXY_SECRET), "{name}");
        }
    }
}

#[tokio::test]
async fn guard_duplicate_host_and_parser_limits() {
    let f = Fixture::start().await;
    let duplicate = f.request(&request("GET", "/", HOST, LOGIN, HOST)).await;
    // Hyper rejects duplicate Host before the router; if accepted by a future
    // parser, the router must still reject it as 421 with security headers.
    assert!([400, 421].contains(&status(&duplicate)));
    if status(&duplicate) == 421 {
        secured(&duplicate);
    }
    for extra in [
        "X-Test: x\r\n".repeat(65),
        format!("X-Test: {}\r\n", "x".repeat(17000)),
    ] {
        let response = f.request(&request("GET", "/", HOST, LOGIN, &extra)).await;
        assert_eq!(status(&response), 431);
    }
}

#[tokio::test]
async fn guard_body_limits_content_length_and_chunked() {
    let f = Fixture::start().await;
    for (size, expected) in [(BODY_LIMIT, 405), (BODY_LIMIT + 1, 413)] {
        for chunked in [false, true] {
            let (header, body) = if chunked {
                (
                    "Transfer-Encoding: chunked\r\n".to_owned(),
                    format!("{size:x}\r\n{}\r\n0\r\n\r\n", "x".repeat(size)),
                )
            } else {
                (format!("Content-Length: {size}\r\n"), "x".repeat(size))
            };
            let response = f
                .request(
                    &(request(
                        "POST",
                        "/api/test",
                        HOST,
                        LOGIN,
                        &format!("{ORIGIN}{header}"),
                    ) + &body),
                )
                .await;
            if cfg!(windows) && expected == 413 && response.is_empty() {
                continue;
            }
            assert_eq!(status(&response), expected);
            secured(&response);
            assert!(response.contains("cache-control: no-store"));
        }
    }
}

#[tokio::test]
async fn guard_rate_limit_cannot_spoof_forwarded_ip() {
    let f = Fixture::start().await;
    let mut successes = 0;
    let mut limited = false;
    let started = std::time::Instant::now();
    for i in 0..RATE_BURST * 3 {
        let response = f
            .request(&request(
                "GET",
                "/",
                HOST,
                LOGIN,
                &format!("X-Forwarded-For: 10.0.0.{i}\r\n"),
            ))
            .await;
        secured(&response);
        if status(&response) == 429 {
            limited = true;
            break;
        }
        assert_eq!(status(&response), 200);
        successes += 1;
    }
    assert!(limited);
    let elapsed = started.elapsed();
    let upper = 60 + (elapsed.as_secs_f64() * 10.0).ceil() as u32 + 1;
    assert!(
        (60..=upper).contains(&successes),
        "{successes} successes in {elapsed:?}, upper {upper}"
    );
    tokio::time::sleep(Duration::from_millis(120)).await;
    assert_eq!(
        status(&f.request(&request("GET", "/", HOST, LOGIN, "")).await),
        200
    );
}

#[tokio::test]
async fn guard_request_and_header_timeouts() {
    let f = Fixture::start().await;
    let body_request = request(
        "POST",
        "/api/test",
        HOST,
        LOGIN,
        &format!("{ORIGIN}Content-Length: 1\r\n"),
    );
    let (body, header) = tokio::join!(f.request(&body_request), f.request("GET / HTTP/1.1\r\n"));
    assert_eq!(status(&body), 408);
    secured(&body);
    // Hyper's header timeout may silently close instead of writing 408.
    assert!(header.is_empty() || status(&header) == 408);
}

#[tokio::test]
async fn static_traversal_and_allowlist_table() {
    let f = Fixture::start().await;
    for path in [
        "/../Cargo.toml",
        "/assets/../index.html",
        "/assets/%2e%2e/index.html",
        "/assets/%2E%2E/index.html",
        "/assets/%252e%252e/index.html",
        "/assets/..%5cindex.html",
        "/assets/\\index.html",
        "/assets/%00.js",
        "/.env",
        "/api/test",
        "/icons",
        "/assets/",
        "/anything",
    ] {
        let response = f.request(&request("GET", path, HOST, LOGIN, ORIGIN)).await;
        assert_eq!(status(&response), 404, "{path}: {response}");
        secured(&response);
    }
    let raw_nul = f
        .request(&request("GET", "/assets/\0.js", HOST, LOGIN, ""))
        .await;
    assert_eq!(status(&raw_nul), 400); // rejected by the sole HTTP parser
}

#[tokio::test]
async fn static_cache_and_security_headers() {
    let f = Fixture::start().await;
    for (path, content, cache) in [
        ("/", "index.html", "no-cache"),
        ("/pair", "index.html", "no-cache"),
        ("/index.html", "index.html", "no-cache"),
        ("/sw.js", "sw.js", "no-cache"),
        ("/manifest.webmanifest", "manifest.webmanifest", "no-cache"),
        ("/icons/icon.svg", "icons/icon.svg", "no-cache"),
        (
            "/assets/app.js",
            "assets/app.js",
            "public, max-age=31536000, immutable",
        ),
    ] {
        let response = f.request(&request("GET", path, HOST, LOGIN, "")).await;
        assert_eq!(status(&response), 200);
        secured(&response);
        assert!(response.contains(&format!("cache-control: {cache}\r\n")));
        assert!(response.ends_with(&format!("fixture:{content}")));
    }
}

#[cfg(unix)]
#[tokio::test]
async fn static_symlink_escape() {
    let f = Fixture::start().await;
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret.js"), "private").unwrap();
    std::os::unix::fs::symlink(outside.path(), f.root.path().join("assets/escape")).unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("secret.js"),
        f.root.path().join("icons/escape.js"),
    )
    .unwrap();
    for path in ["/assets/escape/secret.js", "/icons/escape.js"] {
        let response = f.request(&request("GET", path, HOST, LOGIN, "")).await;
        assert_eq!(status(&response), 404);
        secured(&response);
        assert!(!response.contains("private"));
    }
}

#[tokio::test]
async fn lifecycle_listener_port_in_use() {
    let f = Fixture::start().await;
    let config = Loopback {
        port: f.port,
        public_host: "desktop.tailnet.ts.net:8443".into(),
        required_tailscale_login: "user@example.com".into(),
        proxy_secret: proxy_secret(),
    };
    assert!(matches!(
        Server::bind(&config, f.root.path(), gateway()).await,
        Err("port-in-use")
    ));
}

#[tokio::test]
async fn guard_connection_limit_and_release() {
    let f = Fixture::start().await;
    let mut held = Vec::new();
    for _ in 0..yuzora_companion::http::MAX_CONNECTIONS {
        let mut stream = TcpStream::connect(("127.0.0.1", f.port)).await.unwrap();
        stream.write_all(b"GET / HTTP/1.1\r\n").await.unwrap();
        held.push(stream);
    }
    let mut excess = TcpStream::connect(("127.0.0.1", f.port)).await.unwrap();
    let mut b = [0];
    let result = tokio::time::timeout(Duration::from_secs(2), excess.read(&mut b))
        .await
        .unwrap();
    assert!(matches!(result, Ok(0)) || result.is_err_and(|e| connection_closed(&e)));
    drop(held);
    let response = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let mut stream = TcpStream::connect(("127.0.0.1", f.port)).await.unwrap();
            let mut response = String::new();
            if stream
                .write_all(request("GET", "/", HOST, LOGIN, "").as_bytes())
                .await
                .is_ok()
                && stream.read_to_string(&mut response).await.is_ok()
                && response.starts_with("HTTP/1.1 200")
            {
                break response;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("permit released within 2s");
    assert_eq!(status(&response), 200);
    secured(&response);
}

#[tokio::test]
async fn static_strict_charset_and_content_types() {
    let f = Fixture::start().await;
    for path in [
        "assets/app.js::$DATA",
        "assets/app.js.",
        "assets/.DS_Store",
        "icons/CON",
    ] {
        // Windows disallows some of these names; on Unix create them to prove
        // rejection is lexical rather than an incidental file-not-found.
        #[cfg(unix)]
        std::fs::write(f.root.path().join(path), "private").unwrap();
        let response = f
            .request(&request("GET", &format!("/{path}"), HOST, LOGIN, ""))
            .await;
        assert_eq!(status(&response), 404, "{path}");
        secured(&response);
    }
    for (ext, mime) in [
        ("map", "application/json"),
        ("woff", "font/woff"),
        ("jpg", "image/jpeg"),
        ("jpeg", "image/jpeg"),
        ("txt", "text/plain; charset=utf-8"),
        ("wasm", "application/wasm"),
    ] {
        let path = format!("assets/app.{ext}");
        std::fs::write(f.root.path().join(&path), "fixture").unwrap();
        let response = f
            .request(&request("GET", &format!("/{path}"), HOST, LOGIN, ""))
            .await;
        assert_eq!(status(&response), 200);
        assert!(
            response.contains(&format!("content-type: {mime}\r\n")),
            "{response}"
        );
    }
}

#[tokio::test]
async fn guard_malformed_body_is_bad_request() {
    let f = Fixture::start().await;
    let response = f
        .request(
            &(request(
                "POST",
                "/",
                HOST,
                LOGIN,
                &format!("{ORIGIN}Transfer-Encoding: chunked\r\n"),
            ) + "invalid-chunk\r\n"),
        )
        .await;
    assert_eq!(status(&response), 400);
    secured(&response);
    assert!(response.ends_with("Invalid request body"));
}

fn connection_closed(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted
    )
}

#[tokio::test]
async fn guard_every_registered_route_rejects_missing_identity() {
    let f = Fixture::start().await;
    for route in yuzora_companion::http::ROUTES {
        let path = route.path.replace('*', "fixture.js");
        // Exercise each declared method, plus the non-GET guard for every path
        // even while Phase 1 registers only GET shell endpoints.
        for (method, host, login, origin, expected) in [
            (route.method.as_str(), "", LOGIN, ORIGIN, 421),
            (route.method.as_str(), HOST, "", ORIGIN, 403),
            ("POST", HOST, LOGIN, "", 403),
        ] {
            let response = f
                .request(&request(method, &path, host, login, origin))
                .await;
            assert_eq!(status(&response), expected, "{method} {path}");
            secured(&response);
        }
        // A peer login without the sidecar's secret is no identity on any route.
        let response = f
            .request(&raw_request(
                route.method.as_str(),
                &path,
                &format!("{HOST}{LOGIN}{ORIGIN}"),
            ))
            .await;
        assert_eq!(status(&response), 403, "{path}");
        secured(&response);
        if route.method != axum::http::Method::GET {
            assert_eq!(
                status(
                    &f.request(&request(route.method.as_str(), &path, HOST, LOGIN, ""))
                        .await
                ),
                403
            );
        }
    }
}

#[tokio::test]
async fn guard_trickled_headers_have_total_deadline() {
    let f = Fixture::start().await;
    let mut stream = TcpStream::connect(("127.0.0.1", f.port)).await.unwrap();
    stream
        .write_all(b"GET / HTTP/1.1\r\nX-Trickle: ")
        .await
        .unwrap();
    let (mut reader, mut writer) = stream.into_split();
    let trickle = tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(500)).await;
            if writer.write_all(b"x").await.is_err() {
                break;
            }
        }
    });
    let started = tokio::time::Instant::now();
    let mut response = String::new();
    let result = tokio::time::timeout(
        Duration::from_secs(18),
        reader.read_to_string(&mut response),
    )
    .await;
    trickle.abort();
    let result = result.expect("trickle must not refresh header deadline");
    assert!(result.is_ok() || result.is_err_and(|e| connection_closed(&e)));
    assert!(started.elapsed() >= Duration::from_secs(14));
    assert!(response.is_empty() || status(&response) == 408);
}

#[tokio::test]
async fn guard_keep_alive_idle_uses_header_deadline() {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let f = Fixture::start().await;
    let mut stream = TcpStream::connect(("127.0.0.1", f.port)).await.unwrap();
    let started = tokio::time::Instant::now();
    stream
        .write_all(
            format!("GET / HTTP/1.1\r\n{HOST}Yzc-Proxy-Secret: {PROXY_SECRET}\r\n{LOGIN}\r\n")
                .as_bytes(),
        )
        .await
        .unwrap();
    let mut reader = BufReader::new(stream);
    let mut headers = String::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        headers.push_str(&line);
        if line == "\r\n" {
            break;
        }
    }
    assert_eq!(status(&headers), 200);
    let mut body = vec![0; "fixture:index.html".len()];
    reader.read_exact(&mut body).await.unwrap();
    let mut byte = [0];
    // Pinned Hyper applies the header-read timeout to idle keep-alive too.
    // Lock this behavior down separately from the 60s total write/lifetime cap.
    let result = tokio::time::timeout(Duration::from_secs(18), reader.read(&mut byte))
        .await
        .unwrap();
    assert!(matches!(result, Ok(0)) || result.is_err_and(|e| connection_closed(&e)));
    assert!(started.elapsed() >= Duration::from_secs(14));
}

#[tokio::test]
async fn guard_api_get_requires_origin_or_same_origin_fetch_metadata() {
    let f = Fixture::start().await;
    for (headers, expected) in [
        ("", 403),
        ("Sec-Fetch-Site: cross-site\r\n", 403),
        ("Sec-Fetch-Site: same-site\r\n", 403),
        ("Sec-Fetch-Site: same-origin\r\n", 404),
        (ORIGIN, 404),
        (
            "Origin: https://evil.example\r\nSec-Fetch-Site: same-origin\r\n",
            403,
        ),
        (
            "Sec-Fetch-Site: same-origin\r\nSec-Fetch-Site: same-origin\r\n",
            403,
        ),
    ] {
        let response = f
            .request(&request("GET", "/api/missing", HOST, LOGIN, headers))
            .await;
        assert_eq!(status(&response), expected);
        secured(&response);
        assert!(response.contains("cache-control: no-store"));
    }
}

#[tokio::test]
async fn guard_static_and_api_rate_buckets_are_independent() {
    for exhaust_api in [false, true] {
        let f = Fixture::start().await;
        let (exhaust, unaffected) = if exhaust_api {
            ("/api/missing", "/")
        } else {
            ("/", "/api/missing")
        };
        let mut limited = false;
        for _ in 0..180 {
            if status(
                &f.request(&request("GET", exhaust, HOST, LOGIN, ORIGIN))
                    .await,
            ) == 429
            {
                limited = true;
                break;
            }
        }
        assert!(limited);
        assert_eq!(
            status(
                &f.request(&request("GET", unaffected, HOST, LOGIN, ORIGIN))
                    .await
            ),
            if exhaust_api { 200 } else { 404 }
        );
    }
}

#[test]
fn guard_route_inventory_access_deadline_and_methods() {
    use yuzora_companion::{
        admin::Scope,
        http::{Access, ROUTES},
    };
    let inventory: Vec<_> = ROUTES
        .iter()
        .map(|route| {
            (
                route.method.as_str(),
                route.path,
                route.access,
                route.deadline.duration().as_secs(),
            )
        })
        .collect();
    assert_eq!(
        inventory,
        vec![
            ("GET", "/", Access::Public, 15),
            ("GET", "/index.html", Access::Public, 15),
            ("GET", "/pair", Access::Public, 15),
            ("GET", "/manifest.webmanifest", Access::Public, 15),
            ("GET", "/sw.js", Access::Public, 15),
            ("GET", "/assets/*", Access::Public, 15),
            ("GET", "/icons/*", Access::Public, 15),
            ("GET", "/ws", Access::FirstFrame(Scope::View), 15),
            ("POST", "/api/pair/claim", Access::Public, 15),
            ("POST", "/api/pair/status", Access::Public, 26),
            ("POST", "/api/pair/webauthn/options", Access::Public, 15),
            ("POST", "/api/pair/webauthn/finish", Access::Public, 15),
            ("POST", "/api/auth/options", Access::Public, 15),
            ("POST", "/api/auth/finish", Access::Public, 15),
            (
                "POST",
                "/api/auth/elevate/options",
                Access::Session(Scope::View),
                15
            ),
            (
                "POST",
                "/api/auth/elevate/finish",
                Access::Session(Scope::View),
                15
            ),
            ("POST", "/api/auth/logout", Access::Session(Scope::View), 15),
        ]
    );
}
