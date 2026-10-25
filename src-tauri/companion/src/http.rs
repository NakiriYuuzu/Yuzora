use crate::{
    admin::{Event, EventSender, Loopback, Scope},
    bridge::{self, Upgrades},
    gateway::Gateway,
    guard::{Guard, BODY_LIMIT, HEADER_COUNT, HEADER_LIMIT},
    pairing::{ClaimRequest, StatusRequest},
    session,
};
use axum::{
    body::{to_bytes, Body},
    extract::{Request, State},
    http::{header, HeaderValue, Method, StatusCode},
    response::{IntoResponse, Response},
    Router,
};
use hyper_util::{
    rt::{TokioIo, TokioTimer},
    service::TowerToHyperService,
};
use std::{
    error::Error,
    future::Future,
    net::Ipv4Addr,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::AsyncReadExt,
    net::{TcpListener, TcpStream},
    sync::{oneshot, OwnedSemaphorePermit, Semaphore},
    task::JoinSet,
    time::Instant,
};

pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
pub const CONNECTION_TIMEOUT: Duration = Duration::from_secs(60);
// Header (15s) + body (15s) + long-poll (26s), plus a 4s margin.
pub const CONNECTION_GRACE: Duration = Duration::from_secs(60);
pub const MAX_CONNECTIONS: usize = 64;
/// After the policy expired the loopback port stays bound for at most this long.
pub const EXPIRY_HOLD: Duration = Duration::from_secs(5);
const MAX_STATIC_BYTES: u64 = 16 * 1024 * 1024;
pub const CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; connect-src 'self'; worker-src 'self'; manifest-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'";
pub const SECURITY_HEADERS: &[(&str, &str)] = &[
    ("content-security-policy", CSP),
    ("x-content-type-options", "nosniff"),
    ("referrer-policy", "no-referrer"),
    ("cross-origin-opener-policy", "same-origin"),
    ("cross-origin-resource-policy", "same-origin"),
    (
        "permissions-policy",
        "camera=(), microphone=(), geolocation=()",
    ),
    ("strict-transport-security", "max-age=31536000"),
];

/// Only this table registers handlers. The router remains a single guarded
/// fallback. Public still requires the complete transport/Origin/limits guard.
pub struct Route {
    pub method: Method,
    pub path: &'static str,
    pub access: Access,
    pub deadline: RouteDeadline,
    handler: RouteHandler,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    Public,
    Session(Scope),
    FirstFrame(Scope),
}
#[derive(Clone, Copy, Default)]
pub enum RouteDeadline {
    #[default]
    Standard,
    LongPoll,
}
impl RouteDeadline {
    pub fn duration(self) -> Duration {
        match self {
            Self::Standard => REQUEST_TIMEOUT,
            Self::LongPoll => Duration::from_secs(26),
        }
    }
    async fn run(self, handler: impl Future<Output = Response>) -> Response {
        tokio::time::timeout(self.duration(), handler)
            .await
            .unwrap_or_else(|_| StatusCode::REQUEST_TIMEOUT.into_response())
    }
}
#[derive(Clone, Copy)]
enum RouteHandler {
    Static,
    Upgrade,
    Claim,
    PairStatus,
    Logout,
    RegisterOptions,
    RegisterFinish,
    LoginOptions,
    LoginFinish,
    ElevateOptions,
    ElevateFinish,
    #[cfg(test)]
    Slow,
}
impl Route {
    const fn shell(path: &'static str, handler: RouteHandler) -> Self {
        Self {
            method: Method::GET,
            path,
            access: Access::Public,
            deadline: RouteDeadline::Standard,
            handler,
        }
    }
    fn matches(&self, path: &str) -> bool {
        match self.path.strip_suffix('*') {
            Some(prefix) => path.starts_with(prefix),
            None => self.path == path,
        }
    }
}
pub const ROUTES: &[Route] = &[
    #[cfg(test)]
    Route {
        method: Method::POST,
        path: "/index.html",
        access: Access::Session(Scope::View),
        deadline: RouteDeadline::Standard,
        handler: RouteHandler::Static,
    },
    #[cfg(test)]
    Route::shell("/test/slow", RouteHandler::Slow),
    Route::shell("/", RouteHandler::Static),
    Route::shell("/index.html", RouteHandler::Static),
    Route::shell("/pair", RouteHandler::Static),
    Route::shell("/manifest.webmanifest", RouteHandler::Static),
    Route::shell("/sw.js", RouteHandler::Static),
    Route::shell("/assets/*", RouteHandler::Static),
    Route::shell("/icons/*", RouteHandler::Static),
    Route {
        method: Method::GET,
        path: "/ws",
        access: Access::FirstFrame(Scope::View),
        deadline: RouteDeadline::Standard,
        handler: RouteHandler::Upgrade,
    },
    Route {
        method: Method::POST,
        path: "/api/pair/claim",
        access: Access::Public,
        deadline: RouteDeadline::Standard,
        handler: RouteHandler::Claim,
    },
    Route {
        method: Method::POST,
        path: "/api/pair/status",
        access: Access::Public,
        deadline: RouteDeadline::LongPoll,
        handler: RouteHandler::PairStatus,
    },
    Route {
        method: Method::POST,
        path: "/api/pair/webauthn/options",
        access: Access::Public,
        deadline: RouteDeadline::Standard,
        handler: RouteHandler::RegisterOptions,
    },
    Route {
        method: Method::POST,
        path: "/api/pair/webauthn/finish",
        access: Access::Public,
        deadline: RouteDeadline::Standard,
        handler: RouteHandler::RegisterFinish,
    },
    Route {
        method: Method::POST,
        path: "/api/auth/options",
        access: Access::Public,
        deadline: RouteDeadline::Standard,
        handler: RouteHandler::LoginOptions,
    },
    Route {
        method: Method::POST,
        path: "/api/auth/finish",
        access: Access::Public,
        deadline: RouteDeadline::Standard,
        handler: RouteHandler::LoginFinish,
    },
    Route {
        method: Method::POST,
        path: "/api/auth/elevate/options",
        access: Access::Session(Scope::View),
        deadline: RouteDeadline::Standard,
        handler: RouteHandler::ElevateOptions,
    },
    Route {
        method: Method::POST,
        path: "/api/auth/elevate/finish",
        access: Access::Session(Scope::View),
        deadline: RouteDeadline::Standard,
        handler: RouteHandler::ElevateFinish,
    },
    Route {
        method: Method::POST,
        path: "/api/auth/logout",
        access: Access::Session(Scope::View),
        deadline: RouteDeadline::Standard,
        handler: RouteHandler::Logout,
    },
];

struct App {
    upgrades: Upgrades,
    guard: Guard,
    root: PathBuf,
    gateway: Arc<Gateway>,
}

pub struct Server {
    upgrades: tokio::sync::mpsc::Receiver<bridge::Upgrade>,
    listener: TcpListener,
    router: Router,
    gateway: Arc<Gateway>,
    expired: Option<oneshot::Sender<()>>,
}
impl Server {
    pub async fn bind(
        config: &Loopback,
        root: &Path,
        gateway: Arc<Gateway>,
    ) -> Result<Self, &'static str> {
        let guard = Guard::new(config)?;
        let root = tokio::fs::canonicalize(root)
            .await
            .map_err(|_| "invalid-web-root")?;
        if !root.is_dir() {
            return Err("invalid-web-root");
        }
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, config.port))
            .await
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::AddrInUse {
                    "port-in-use"
                } else {
                    "listen-failed"
                }
            })?;
        let (upgrades, pending) = Upgrades::new();
        let router = Router::new().fallback(handle).with_state(Arc::new(App {
            upgrades,
            guard,
            root,
            gateway: gateway.clone(),
        }));
        Ok(Self {
            upgrades: pending,
            listener,
            router,
            gateway,
            expired: None,
        })
    }
    /// Fires the moment the policy expires, before any connection is drained.
    pub fn on_expiry(&mut self) -> oneshot::Receiver<()> {
        let (sender, receiver) = oneshot::channel();
        self.expired = Some(sender);
        receiver
    }
    pub fn port(&self) -> std::io::Result<u16> {
        Ok(self.listener.local_addr()?.port())
    }

    /// Shutdown owns all connections, including idle/partial requests. No detached
    /// tasks survive EOF. WS peers receive 1001 (4401 for policy expiry).
    /// Returns true only for policy expiry, after closing the peers. Expiry keeps
    /// the listener bound (accepting and closing) until `shutdown` or `EXPIRY_HOLD`:
    /// the node forwards the tailnet to this port until the desktop has unserved it.
    pub async fn run(mut self, mut shutdown: oneshot::Receiver<()>) -> bool {
        let mut expired = false;
        let mut maintenance = tokio::time::interval(Duration::from_secs(1));
        let permits = Arc::new(Semaphore::new(MAX_CONNECTIONS));
        let mut connections = JoinSet::new();
        let (stop, stopping) = tokio::sync::watch::channel(0u16);
        connections.spawn(crate::push::run(self.gateway.clone(), stopping.clone()));
        loop {
            tokio::select! {
                biased;
                _ = &mut shutdown => break,
                _ = self.gateway.clock.until(self.gateway.policy.enable_until) => { expired = true; break; },
                _ = maintenance.tick() => self.gateway.maintenance(),
                Some(_) = connections.join_next(), if !connections.is_empty() => {},
                Some(upgrade) = self.upgrades.recv() => {
                    connections.spawn(bridge::run(upgrade, self.gateway.clone(), stopping.clone()));
                },
                accepted = self.listener.accept() => {
                    let (stream, _) = match accepted {
                        Ok(connection) => connection,
                        Err(error) => {
                            if let Some(delay) = accept_retry_delay(&error) {
                                tokio::select! {
                                    _ = &mut shutdown => break,
                                    _ = tokio::time::sleep(delay) => {},
                                }
                            }
                            continue;
                        }
                    };
                    let Ok(permit) = permits.clone().try_acquire_owned() else { continue; };
                    let router = self.router.clone();
                    let mut stopping = stopping.clone();
                    connections.spawn(async move {
                        tokio::select! {
                            biased;
                            _ = stopping.changed() => {},
                            _ = serve_connection(stream, router, permit, CONNECTION_TIMEOUT, CONNECTION_GRACE) => {},
                        }
                    });
                }
            }
        }
        let held = if expired {
            if let Some(notice) = self.expired.take() {
                let _ = notice.send(());
            }
            Some((self.listener, Instant::now() + EXPIRY_HOLD))
        } else {
            drop(self.listener);
            None
        };
        self.upgrades.close();
        while self.upgrades.try_recv().is_ok() {}
        if expired {
            self.gateway.maintenance();
        }
        stop.send_replace(if expired { 4401 } else { 1001 });
        // Give writable peers their close frame, then abort every remaining task.
        // main's one-second server join remains an outer bound.
        let teardown = async {
            let _ = tokio::time::timeout(Duration::from_millis(500), async {
                while connections.join_next().await.is_some() {}
                self.gateway.herdr_jobs.drain().await;
            })
            .await;
            connections.abort_all();
            while connections.join_next().await.is_some() {}
        };
        match held {
            Some((listener, until)) => hold_port(listener, teardown, &mut shutdown, until).await,
            None => teardown.await,
        }
        expired
    }
}

/// Accepts and drops every connection while `teardown` runs and then until
/// `shutdown` or `until`; the port is released when this returns.
async fn hold_port(
    listener: TcpListener,
    teardown: impl Future<Output = ()>,
    shutdown: &mut oneshot::Receiver<()>,
    until: Instant,
) {
    tokio::pin!(teardown);
    let mut torn_down = false;
    loop {
        tokio::select! {
            _ = &mut teardown, if !torn_down => torn_down = true,
            _ = &mut *shutdown => break,
            _ = tokio::time::sleep_until(until) => break,
            accepted = listener.accept() => {
                if let Err(error) = accepted {
                    if let Some(delay) = accept_retry_delay(&error) {
                        tokio::time::sleep(delay).await;
                    }
                }
            }
        }
    }
    if !torn_down {
        teardown.await;
    }
}

/// Connection-local failures retry immediately; resource/unknown errors back off
/// without ever terminating the listener or spinning on fd/memory exhaustion.
fn accept_retry_delay(error: &std::io::Error) -> Option<Duration> {
    use std::io::ErrorKind;
    match error.kind() {
        ErrorKind::ConnectionAborted
        | ErrorKind::ConnectionReset
        | ErrorKind::Interrupted
        | ErrorKind::WouldBlock => None,
        _ => Some(Duration::from_millis(50)),
    }
}

async fn serve_connection(
    stream: TcpStream,
    router: Router,
    _permit: OwnedSemaphorePermit,
    lifetime: Duration,
    grace: Duration,
) {
    // Hyper is the ONLY HTTP parser. Its pre-router errors may lack security
    // headers. Upgrade completion transfers IO to a separate tracked WS task.
    let mut builder = hyper::server::conn::http1::Builder::new();
    builder
        .timer(TokioTimer::new())
        .header_read_timeout(REQUEST_TIMEOUT)
        .max_headers(HEADER_COUNT)
        .max_buf_size(HEADER_LIMIT);
    let connection = builder
        .serve_connection(TokioIo::new(stream), TowerToHyperService::new(router))
        .with_upgrades();
    tokio::pin!(connection);
    tokio::select! {
        _ = &mut connection => {},
        _ = tokio::time::sleep(lifetime) => {
            connection.as_mut().graceful_shutdown();
            // Disable keep-alive immediately, but finish the in-flight request.
            // Stalled response readers cannot retain the permit past this bound.
            let _ = tokio::time::timeout(grace, connection).await;
        }
    }
}

async fn handle(State(app): State<Arc<App>>, request: Request) -> Response {
    let api = request.uri().path().starts_with("/api/") || request.uri().path() == "/ws";
    let mut response = dispatch(&app, request).await;
    for &(name, value) in SECURITY_HEADERS {
        response
            .headers_mut()
            .insert(name, HeaderValue::from_static(value));
    }
    if api || !response.status().is_success() {
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    response
}

async fn dispatch(app: &App, request: Request) -> Response {
    if let Err(status) = app
        .guard
        .check(request.method(), request.uri(), request.headers())
    {
        return status.into_response();
    }
    if app.gateway.clock.now() >= app.gateway.policy.enable_until {
        return (StatusCode::FORBIDDEN, "Policy expired").into_response();
    }
    let (parts, body) = request.into_parts();
    // No API consumes query parameters: tokens/secret material belong in bodies.
    if (parts.uri.path().starts_with("/api/") || parts.uri.path() == "/ws")
        && parts.uri.query().is_some()
    {
        return (StatusCode::BAD_REQUEST, "Invalid API request").into_response();
    }
    if parts
        .headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .is_some_and(|len| len > BODY_LIMIT as u64)
    {
        return (StatusCode::PAYLOAD_TOO_LARGE, "Request body too large").into_response();
    }
    // Body collection and the route handler have independent deadlines. A
    // future 25s long-poll must not consume the 15s header/body allowance.
    let body = match tokio::time::timeout(REQUEST_TIMEOUT, to_bytes(body, BODY_LIMIT)).await {
        Err(_) => return StatusCode::REQUEST_TIMEOUT.into_response(),
        Ok(Err(error)) => return body_error_response(&error),
        Ok(Ok(body)) => body,
    };
    let Some(route) = ROUTES
        .iter()
        .find(|route| route.matches(parts.uri.path()) && route.method == parts.method)
    else {
        return if parts.method == Method::GET {
            StatusCode::NOT_FOUND.into_response()
        } else {
            StatusCode::METHOD_NOT_ALLOWED.into_response()
        };
    };
    if let Access::FirstFrame(_) = route.access {
        return app
            .upgrades
            .respond(Request::from_parts(parts, Body::empty()), &app.gateway);
    }
    let auth = if let Access::Session(scope) = route.access {
        let Some(ctx) = app.gateway.authenticate(&parts.headers, scope) else {
            return (StatusCode::UNAUTHORIZED, "Authentication required").into_response();
        };
        if let Err(error) = app.gateway.with_authorized(&ctx, scope, |_| ()) {
            return error.into_response();
        }
        Some(ctx)
    } else {
        None
    };
    route
        .deadline
        .run(async {
            match route.handler {
                RouteHandler::Static => static_file(&app.root, parts.uri.path()).await,
                RouteHandler::Upgrade => unreachable!("first-frame route handled above"),
                RouteHandler::Claim => {
                    let Ok(request) = serde_json::from_slice::<ClaimRequest>(&body) else {
                        return invalid_json();
                    };
                    let user_agent = match parts.headers.get("user-agent") {
                        Some(value) => match value.to_str() {
                            Ok(value) => value,
                            Err(_) => return invalid_json(),
                        },
                        None => "",
                    };
                    json_result(app.gateway.pairings.claim(request, user_agent))
                }
                RouteHandler::PairStatus => {
                    let Ok(request) = serde_json::from_slice::<StatusRequest>(&body) else {
                        return invalid_json();
                    };
                    json_result(app.gateway.pairings.status(request).await)
                }
                RouteHandler::RegisterOptions => {
                    let Ok(request) =
                        serde_json::from_slice::<crate::webauthn::RegistrationOptions>(&body)
                    else {
                        return invalid_json();
                    };
                    json_result(app.gateway.registration_options(request))
                }
                RouteHandler::RegisterFinish => {
                    let Ok(request) =
                        serde_json::from_slice::<crate::webauthn::RegistrationFinish>(&body)
                    else {
                        return invalid_json();
                    };
                    json_result(app.gateway.registration_finish(request))
                }
                RouteHandler::LoginOptions => {
                    if serde_json::from_slice::<crate::webauthn::LoginOptions>(&body).is_err() {
                        return invalid_json();
                    }
                    json_result(app.gateway.login_options())
                }
                RouteHandler::LoginFinish => {
                    let Ok(request) = serde_json::from_slice::<crate::webauthn::LoginFinish>(&body)
                    else {
                        return invalid_json();
                    };
                    match app.gateway.login_finish(request) {
                        Ok(session::AssertionResult::LoggedIn {
                            session: id,
                            binding,
                            device_id,
                        }) => {
                            let mut response = json_result(Ok(crate::webauthn::LoginResponse {
                                binding,
                                device_id,
                                scopes: [Scope::View],
                            }));
                            response.headers_mut().insert(
                                header::SET_COOKIE,
                                HeaderValue::from_str(&session::session_cookie(&id))
                                    .expect("safe cookie"),
                            );
                            response
                        }
                        Ok(_) => invalid_json(),
                        Err(error) => error.into_response(),
                    }
                }
                RouteHandler::ElevateOptions => {
                    let Ok(request) =
                        serde_json::from_slice::<crate::webauthn::ElevationOptions>(&body)
                    else {
                        return invalid_json();
                    };
                    json_result(
                        app.gateway.elevation_options(
                            auth.as_ref().expect("session route"),
                            request.scope,
                        ),
                    )
                }
                RouteHandler::ElevateFinish => {
                    let Ok(request) =
                        serde_json::from_slice::<crate::webauthn::ElevationFinish>(&body)
                    else {
                        return invalid_json();
                    };
                    json_result(
                        app.gateway
                            .elevation_finish(auth.as_ref().expect("session route"), request),
                    )
                }
                RouteHandler::Logout => {
                    if !body.is_empty() && body.as_ref() != b"{}" {
                        return invalid_json();
                    }
                    let Some(ctx) = auth.as_ref() else {
                        return (StatusCode::UNAUTHORIZED, "Authentication required")
                            .into_response();
                    };
                    if let Err(error) = app.gateway.logout(ctx) {
                        return error.into_response();
                    }
                    (
                        [(header::SET_COOKIE, session::clear_cookie())],
                        StatusCode::NO_CONTENT,
                    )
                        .into_response()
                }
                #[cfg(test)]
                RouteHandler::Slow => {
                    tokio::time::sleep(Duration::from_millis(600)).await;
                    "complete slow response".into_response()
                }
            }
        })
        .await
}

fn invalid_json() -> Response {
    (StatusCode::BAD_REQUEST, "Invalid API request").into_response()
}
fn json_result<T: serde::Serialize>(result: Result<T, (StatusCode, &'static str)>) -> Response {
    match result {
        Ok(value) => match serde_json::to_vec(&value) {
            Ok(body) => ([(header::CONTENT_TYPE, "application/json")], body).into_response(),
            Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "Response unavailable").into_response(),
        },
        Err(error) => error.into_response(),
    }
}

/// Handler events only promise enqueue, not delivery. Call before committing
/// widening operations only. Tightening commits first and defers notifications; propagate errors through `handle` for uniform security headers.
pub fn try_emit_event(
    events: &EventSender,
    event: Event,
) -> Result<(), (StatusCode, &'static str)> {
    if events.try_emit(event) {
        Ok(())
    } else {
        Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "Admin event queue unavailable",
        ))
    }
}

fn body_error_response(error: &axum::Error) -> Response {
    let mut source: &(dyn Error + 'static) = error;
    loop {
        if source.is::<http_body_util::LengthLimitError>() {
            return (StatusCode::PAYLOAD_TOO_LARGE, "Request body too large").into_response();
        }
        match source.source() {
            Some(next) => source = next,
            None => return (StatusCode::BAD_REQUEST, "Invalid request body").into_response(),
        }
    }
}

fn valid_static_segment(segment: &str) -> bool {
    let mut bytes = segment.bytes();
    if !bytes
        .next()
        .is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        || !bytes.all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        || segment.ends_with('.')
    {
        return false;
    }
    // Windows device names remain aliases even with extensions. Reject on every
    // platform so the same bundle allowlist has the same meaning everywhere.
    let stem = segment
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    !matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        && !(stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && matches!(stem.as_bytes()[3], b'1'..=b'9'))
}

async fn static_file(root: &Path, path: &str) -> Response {
    // Bundle names are literal ASCII paths; encoded aliases are intentionally not
    // served. This rejects single/double percent encodings without a second decoder.
    if path.contains("..")
        || (path != "/"
            && !path
                .strip_prefix('/')
                .is_some_and(|relative| relative.split('/').all(valid_static_segment)))
    {
        return StatusCode::NOT_FOUND.into_response();
    }
    let relative = match path {
        "/" | "/index.html" | "/pair" => "index.html",
        "/manifest.webmanifest" => "manifest.webmanifest",
        "/sw.js" => "sw.js",
        _ if path.starts_with("/assets/") || path.starts_with("/icons/") => &path[1..],
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    let Ok(canonical) = tokio::fs::canonicalize(root.join(relative)).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if !canonical.starts_with(root) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Ok(file) = tokio::fs::File::open(&canonical).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Ok(metadata) = file.metadata().await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if !metadata.is_file() || metadata.len() > MAX_STATIC_BYTES {
        return StatusCode::NOT_FOUND.into_response();
    }
    let content_type = match Path::new(relative).extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("webmanifest") => "application/manifest+json",
        Some("json" | "map") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("ico") => "image/x-icon",
        Some("webp") => "image/webp",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("txt") => "text/plain; charset=utf-8",
        Some("wasm") => "application/wasm",
        _ => "application/octet-stream",
    };
    let cache = if path.starts_with("/assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    let mut response = (
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, cache),
        ],
        Body::from_stream(tokio_util::io::ReaderStream::new(file.take(metadata.len()))),
    )
        .into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_LENGTH, HeaderValue::from(metadata.len()));
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::identity::PROXY_SECRET;
    use tokio::io::AsyncWriteExt;

    fn proxy_secret() -> crate::admin::ProxySecret {
        crate::admin::ProxySecret(zeroize::Zeroizing::new(PROXY_SECRET.into()))
    }

    #[test]
    fn accept_errors_retry_without_terminating_gateway() {
        use std::io::{Error, ErrorKind};
        for kind in [
            ErrorKind::ConnectionAborted,
            ErrorKind::ConnectionReset,
            ErrorKind::Interrupted,
            ErrorKind::WouldBlock,
        ] {
            assert_eq!(accept_retry_delay(&Error::from(kind)), None);
        }
        for kind in [
            ErrorKind::Other,
            ErrorKind::OutOfMemory,
            ErrorKind::PermissionDenied,
        ] {
            assert_eq!(
                accept_retry_delay(&Error::from(kind)),
                Some(Duration::from_millis(50))
            );
        }
        // EMFILE/ENFILE on Unix; WSAEMFILE/WSAENOBUFS on Windows. Unknown
        // platform errno values also intentionally take the backoff branch.
        for code in [23, 24, 10024, 10055] {
            assert_eq!(
                accept_retry_delay(&Error::from_raw_os_error(code)),
                Some(Duration::from_millis(50))
            );
        }
        #[cfg(target_os = "macos")]
        assert_eq!(
            accept_retry_delay(&Error::from_raw_os_error(55)),
            Some(Duration::from_millis(50))
        );
        #[cfg(target_os = "linux")]
        assert_eq!(
            accept_retry_delay(&Error::from_raw_os_error(105)),
            Some(Duration::from_millis(50))
        );
    }

    #[tokio::test(start_paused = true)]
    async fn route_deadlines_reserve_long_poll_without_extending_body_deadline() {
        assert_eq!(REQUEST_TIMEOUT, Duration::from_secs(15));
        assert_eq!(RouteDeadline::default().duration(), REQUEST_TIMEOUT);
        for (deadline, seconds, expected) in [
            (RouteDeadline::Standard, 16, StatusCode::REQUEST_TIMEOUT),
            (RouteDeadline::LongPoll, 25, StatusCode::OK),
            (RouteDeadline::LongPoll, 27, StatusCode::REQUEST_TIMEOUT),
        ] {
            let response = deadline
                .run(async {
                    tokio::time::sleep(Duration::from_secs(seconds)).await;
                    StatusCode::OK.into_response()
                })
                .await;
            assert_eq!(response.status(), expected);
        }
        assert_eq!(RouteDeadline::LongPoll.duration(), Duration::from_secs(26));
    }

    fn test_gateway() -> Arc<Gateway> {
        let init = serde_json::from_str(&crate::test_support::init::json()).unwrap();
        let (events, _writer) = crate::admin::spawn_writer(tokio::io::sink());
        Gateway::new(&init, events, crate::clock::Clock::default()).unwrap()
    }

    async fn test_server() -> (tempfile::TempDir, Server) {
        let root = tempfile::tempdir().unwrap();
        tokio::fs::write(root.path().join("index.html"), "shell")
            .await
            .unwrap();
        let server = Server::bind(
            &Loopback {
                port: 0,
                public_host: "desktop.tailnet.ts.net:8443".into(),
                required_tailscale_login: "user@example.com".into(),
                proxy_secret: proxy_secret(),
            },
            root.path(),
            test_gateway(),
        )
        .await
        .unwrap();
        (root, server)
    }

    fn valid_request(method: &str, path: &str) -> Request {
        Request::builder()
            .method(method)
            .uri(path)
            .header("Host", "desktop.tailnet.ts.net:8443")
            .header("Yzc-Proxy-Secret", PROXY_SECRET)
            .header("Yzc-Peer-Login", "user@example.com")
            .header("Origin", "https://desktop.tailnet.ts.net:8443")
            .body(Body::empty())
            .unwrap()
    }

    #[tokio::test]
    async fn scoped_route_fails_closed_with_valid_transport_identity() {
        let (root, server) = test_server().await;
        let app = Arc::new(App {
            upgrades: Upgrades::new().0,
            guard: Guard::new(&Loopback {
                port: 0,
                public_host: "desktop.tailnet.ts.net:8443".into(),
                required_tailscale_login: "user@example.com".into(),
                proxy_secret: proxy_secret(),
            })
            .unwrap(),
            root: tokio::fs::canonicalize(root.path()).await.unwrap(),
            gateway: server.gateway,
        });
        assert_eq!(
            static_file(&app.root, "/index.html").await.status(),
            StatusCode::OK
        );
        let request = valid_request("POST", "/index.html");
        assert!(app
            .guard
            .check(request.method(), request.uri(), request.headers())
            .is_ok());
        let response = handle(State(app), request).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        for &(name, value) in SECURITY_HEADERS {
            assert_eq!(response.headers()[name], value);
        }
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(
            to_bytes(response.into_body(), BODY_LIMIT).await.unwrap(),
            "Authentication required"
        );
    }

    #[tokio::test]
    async fn handler_event_queue_full_returns_fixed_503_without_waiting() {
        let (output, _input) = tokio::io::duplex(1);
        let (events, writer) = crate::admin::spawn_writer(output);
        for _ in 0..64 {
            assert!(try_emit_event(&events, Event::error("test")).is_ok());
        }
        let started = std::time::Instant::now();
        let response = try_emit_event(&events, Event::error("test"))
            .unwrap_err()
            .into_response();
        assert!(started.elapsed() < Duration::from_millis(100));
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            to_bytes(response.into_body(), BODY_LIMIT).await.unwrap(),
            "Admin event queue unavailable"
        );
        assert!(
            !writer.is_finished(),
            "queue saturation is not broken stdout"
        );
        writer.abort();
        let _ = writer.await;
    }

    async fn read_response(client: &mut TcpStream) -> (String, Vec<u8>) {
        tokio::time::timeout(Duration::from_secs(3), async {
            let mut header = Vec::new();
            while !header.ends_with(b"\r\n\r\n") {
                header.push(client.read_u8().await.unwrap());
            }
            let header = String::from_utf8(header).unwrap();
            let length: usize = header
                .lines()
                .find_map(|line| line.strip_prefix("content-length: "))
                .unwrap()
                .parse()
                .unwrap();
            let mut body = vec![0; length];
            client.read_exact(&mut body).await.unwrap();
            (header, body)
        })
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn reused_connection_finishes_request_during_lifetime_grace() {
        let (_root, server) = test_server().await;
        let mut client = TcpStream::connect((Ipv4Addr::LOCALHOST, server.port().unwrap()))
            .await
            .unwrap();
        let (stream, _) = server.listener.accept().await.unwrap();
        let permits = Arc::new(Semaphore::new(1));
        let permit = permits.clone().acquire_owned().await.unwrap();
        let started = tokio::time::Instant::now();
        let lifetime = Duration::from_secs(1);
        let task = tokio::spawn(serve_connection(
            stream,
            server.router,
            permit,
            lifetime,
            Duration::from_secs(1),
        ));
        client.write_all(format!("GET / HTTP/1.1\r\nHost: desktop.tailnet.ts.net:8443\r\nYzc-Proxy-Secret: {PROXY_SECRET}\r\nYzc-Peer-Login: user@example.com\r\n\r\n").as_bytes()).await.unwrap();
        assert_eq!(read_response(&mut client).await.1, b"shell");
        tokio::time::sleep_until(started + Duration::from_millis(600)).await;
        client.write_all(format!("GET /test/slow HTTP/1.1\r\nHost: desktop.tailnet.ts.net:8443\r\nYzc-Proxy-Secret: {PROXY_SECRET}\r\nYzc-Peer-Login: user@example.com\r\n\r\n").as_bytes()).await.unwrap();
        assert!(
            started.elapsed() < lifetime,
            "request must arrive before lifetime"
        );
        let (header, body) = read_response(&mut client).await;
        assert!(header.starts_with("HTTP/1.1 200"));
        assert_eq!(body, b"complete slow response");
        assert!(started.elapsed() >= lifetime);
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), client.read_u8())
                .await
                .unwrap()
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::UnexpectedEof
        );
        task.await.unwrap();
        assert_eq!(permits.available_permits(), 1);
    }

    #[tokio::test]
    async fn lifetime_closes_idle_connection_without_waiting_for_grace() {
        let (_root, server) = test_server().await;
        let mut client = TcpStream::connect((Ipv4Addr::LOCALHOST, server.port().unwrap()))
            .await
            .unwrap();
        let (stream, _) = server.listener.accept().await.unwrap();
        let permit = Arc::new(Semaphore::new(1)).acquire_owned().await.unwrap();
        let task = tokio::spawn(serve_connection(
            stream,
            server.router,
            permit,
            Duration::from_millis(300),
            Duration::from_secs(60),
        ));
        client.write_all(format!("GET / HTTP/1.1\r\nHost: desktop.tailnet.ts.net:8443\r\nYzc-Proxy-Secret: {PROXY_SECRET}\r\nYzc-Peer-Login: user@example.com\r\n\r\n").as_bytes()).await.unwrap();
        assert_eq!(read_response(&mut client).await.1, b"shell");
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            client.read_u8().await.unwrap_err().kind(),
            std::io::ErrorKind::UnexpectedEof
        );
    }

    #[tokio::test]
    async fn slow_static_reader_releases_connection_permit() {
        let root = tempfile::tempdir().unwrap();
        tokio::fs::create_dir(root.path().join("assets"))
            .await
            .unwrap();
        let file = tokio::fs::File::create(root.path().join("assets/large.js"))
            .await
            .unwrap();
        file.set_len(MAX_STATIC_BYTES).await.unwrap();
        drop(file);
        let server = Server::bind(
            &Loopback {
                port: 0,
                public_host: "desktop.tailnet.ts.net:8443".into(),
                required_tailscale_login: "user@example.com".into(),
                proxy_secret: proxy_secret(),
            },
            root.path(),
            test_gateway(),
        )
        .await
        .unwrap();
        let mut client = TcpStream::connect((Ipv4Addr::LOCALHOST, server.port().unwrap()))
            .await
            .unwrap();
        let (stream, _) = server.listener.accept().await.unwrap();
        let permits = Arc::new(Semaphore::new(1));
        let permit = permits.clone().acquire_owned().await.unwrap();
        assert_eq!(CONNECTION_TIMEOUT, Duration::from_secs(60));
        assert_eq!(CONNECTION_GRACE, Duration::from_secs(60));
        let lifetime = Duration::from_millis(200);
        let grace = Duration::from_millis(400);
        let task = tokio::spawn(serve_connection(
            stream,
            server.router,
            permit,
            lifetime,
            grace,
        ));
        client.write_all(format!("GET /assets/large.js HTTP/1.1\r\nHost: desktop.tailnet.ts.net:8443\r\nYzc-Proxy-Secret: {PROXY_SECRET}\r\nYzc-Peer-Login: user@example.com\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
        let mut header = Vec::new();
        while !header.ends_with(b"\r\n\r\n") {
            header.push(client.read_u8().await.unwrap());
        }
        assert!(header.starts_with(b"HTTP/1.1 200"));
        // Stop reading while the 16 MiB stream is backpressured.
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(permits.available_permits(), 0);
        tokio::time::timeout(Duration::from_secs(4), task)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(permits.available_permits(), 1);
        let mut partial = Vec::new();
        if let Err(error) = client.read_to_end(&mut partial).await {
            assert!(matches!(
                error.kind(),
                std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted
            ));
        }
        assert!(
            (partial.len() as u64) < MAX_STATIC_BYTES,
            "test must exercise a stalled write"
        );
    }
}
