//! Isolated static HTML preview sessions.
//!
//! Each selected HTML file gets an unpredictable token and a dedicated route
//! under a localhost server. Requests are allowlisted, fail closed, and never
//! fall back to serving the parent directory.

use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use tauri::State;
use tiny_http::{Header, Method, Response, Server, StatusCode};

use crate::path_capability::PinnedDir;
use crate::preview_resource_policy::{
    build_allowlist, classify_kind, is_denied_path, relative_from_root, AssetKind, PolicyError,
    MAX_SERVE_BYTES,
};

const SESSION_TTL: Duration = Duration::from_secs(60 * 60);
const HTML_CSP: &str = "default-src 'none'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self' data:; media-src 'self'; connect-src 'none'; form-action 'none'; frame-src 'self'; worker-src 'none'; object-src 'none'; base-uri 'none'; manifest-src 'none'";
const ASSET_CSP: &str = "default-src 'none'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self' data:; media-src 'self'; connect-src 'none'; form-action 'none'; frame-src 'none'; worker-src 'none'; object-src 'none'; base-uri 'none'";

#[derive(serde::Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PreviewSessionInfo {
    pub token: String,
    pub url: String,
}

struct PreviewSession {
    allowlist: std::collections::HashSet<String>,
    root: PinnedDir,
    selected: PathBuf,
    expires_at: Instant,
}

struct ServerControl {
    port: Option<u16>,
    stop: Option<Arc<AtomicBool>>,
    handle: Option<JoinHandle<()>>,
}

pub struct PreviewServerState {
    control: Mutex<ServerControl>,
    sessions: Arc<Mutex<HashMap<String, PreviewSession>>>,
}

impl PreviewServerState {
    pub fn new() -> Self {
        Self {
            control: Mutex::new(ServerControl {
                port: None,
                stop: None,
                handle: None,
            }),
            sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn create_session(&self, path: &str) -> Result<PreviewSessionInfo, String> {
        let allowlist = build_allowlist(Path::new(path)).map_err(PolicyError::as_code)?;
        let rel = relative_from_root(&allowlist.url_root, &allowlist.selected)
            .ok_or(PolicyError::InvalidPath.as_code())?;
        let token = random_token();
        let port = self.ensure_server()?;
        let selected = allowlist.selected.clone();
        let root = PinnedDir::open_dir(&allowlist.url_root)
            .map_err(|_| PolicyError::NotAccessible.as_code())?;
        let files = allowlist
            .files
            .iter()
            .filter_map(|path| relative_from_root(&allowlist.url_root, path))
            .collect();
        {
            let mut sessions = self
                .sessions
                .lock()
                .map_err(|_| PolicyError::NotAccessible.as_code())?;
            sessions.retain(|_, session| session.selected != selected);
            sessions.insert(
                token.clone(),
                PreviewSession {
                    allowlist: files,
                    root,
                    selected,
                    expires_at: Instant::now() + SESSION_TTL,
                },
            );
        }
        Ok(PreviewSessionInfo {
            url: format!("http://127.0.0.1:{port}/{token}/{rel}"),
            token,
        })
    }

    pub fn revoke_session(&self, token: &str) {
        if let Ok(mut sessions) = self.sessions.lock() {
            sessions.remove(token);
        }
    }

    pub fn stop_all(&self) {
        let handle = {
            let Ok(mut control) = self.control.lock() else {
                return;
            };
            if let Some(stop) = control.stop.take() {
                stop.store(true, Ordering::Relaxed);
            }
            control.port = None;
            control.handle.take()
        };
        if let Some(handle) = handle {
            let _ = handle.join();
        }
        if let Ok(mut sessions) = self.sessions.lock() {
            sessions.clear();
        }
    }

    fn ensure_server(&self) -> Result<u16, String> {
        let mut control = self
            .control
            .lock()
            .map_err(|_| PolicyError::NotAccessible.as_code().to_string())?;
        if let Some(port) = control.port {
            return Ok(port);
        }
        let server =
            Server::http("127.0.0.1:0").map_err(|_| "preview server failed to bind".to_string())?;
        let port = server
            .server_addr()
            .to_ip()
            .map(|addr| addr.port())
            .ok_or_else(|| "preview server failed to bind".to_string())?;
        let stop = Arc::new(AtomicBool::new(false));
        let stop_flag = Arc::clone(&stop);
        let sessions = Arc::clone(&self.sessions);
        let handle = std::thread::spawn(move || serve_loop(server, stop_flag, sessions));
        control.port = Some(port);
        control.stop = Some(stop);
        control.handle = Some(handle);
        Ok(port)
    }

    #[cfg(test)]
    fn expire_session(&self, token: &str) {
        if let Ok(mut sessions) = self.sessions.lock() {
            if let Some(session) = sessions.get_mut(token) {
                session.expires_at = Instant::now() - Duration::from_secs(1);
            }
        }
    }
}

impl Default for PreviewServerState {
    fn default() -> Self {
        Self::new()
    }
}

fn serve_loop(
    server: Server,
    stop: Arc<AtomicBool>,
    sessions: Arc<Mutex<HashMap<String, PreviewSession>>>,
) {
    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        match server.recv_timeout(Duration::from_millis(200)) {
            Ok(Some(request)) => {
                if stop.load(Ordering::Relaxed) {
                    let _ = request.respond(Response::empty(503));
                    break;
                }
                handle_request(request, &sessions);
            }
            Ok(None) => {}
            Err(_) => break,
        }
    }
}

fn handle_request(request: tiny_http::Request, sessions: &Mutex<HashMap<String, PreviewSession>>) {
    if request.method() != &Method::Get && request.method() != &Method::Head {
        let _ = request.respond(ServeOutcome::error(405, "Method not allowed").into_response(true));
        return;
    }
    let head_only = request.method() == &Method::Head;
    let url = request.url().to_string();
    let outcome = match sessions.lock() {
        Ok(mut guard) => serve_path(&url, &mut guard),
        Err(_) => ServeOutcome::error(500, "Internal error"),
    };
    let _ = request.respond(outcome.into_response(head_only));
}

fn serve_path(url: &str, sessions: &mut HashMap<String, PreviewSession>) -> ServeOutcome {
    let Some((token, rel)) = split_token_rel(url) else {
        return ServeOutcome::error(404, "Not found");
    };
    if !is_hex_token(token) {
        return ServeOutcome::error(404, "Not found");
    }
    let Some(session) = sessions.get(token) else {
        return ServeOutcome::error(404, "Not found");
    };
    if session.expires_at <= Instant::now() {
        sessions.remove(token);
        return ServeOutcome::error(410, "Gone");
    }
    if rel.is_empty() {
        return ServeOutcome::error(404, "Not found");
    }
    let decoded = match decode_rel_components(rel) {
        Ok(parts) => parts,
        Err(()) => return ServeOutcome::error(403, "Forbidden"),
    };
    let rel_key = decoded.join("/");
    let rel_path = Path::new(&rel_key);
    if is_denied_path(rel_path) || !session.allowlist.contains(&rel_key) {
        return ServeOutcome::error(403, "Forbidden");
    }
    let opened = match session.root.open_file_names(&decoded) {
        Ok(opened) => opened,
        Err(_) => return ServeOutcome::error(403, "Forbidden"),
    };
    if opened.len > MAX_SERVE_BYTES {
        return ServeOutcome::error(403, "Forbidden");
    }
    let mut bytes = Vec::new();
    let mut file = opened.file;
    if file.read_to_end(&mut bytes).is_err() {
        return ServeOutcome::error(404, "Not found");
    }
    ServeOutcome {
        status: 200,
        body: bytes,
        mime: mime_of(rel_path),
        csp: match classify_kind(rel_path) {
            AssetKind::Html => HTML_CSP,
            _ => ASSET_CSP,
        },
    }
}

struct ServeOutcome {
    status: u16,
    body: Vec<u8>,
    mime: &'static str,
    csp: &'static str,
}

impl ServeOutcome {
    fn error(status: u16, message: &'static str) -> Self {
        Self {
            status,
            body: message.as_bytes().to_vec(),
            mime: "text/plain; charset=utf-8",
            csp: ASSET_CSP,
        }
    }

    fn into_response(self, head_only: bool) -> Response<Cursor<Vec<u8>>> {
        let body = if head_only { Vec::new() } else { self.body };
        let mut response = Response::from_data(body).with_status_code(StatusCode(self.status));
        add_header(&mut response, "Cache-Control", "no-store");
        add_header(&mut response, "X-Content-Type-Options", "nosniff");
        add_header(&mut response, "Content-Type", self.mime);
        add_header(&mut response, "Content-Security-Policy", self.csp);
        add_header(&mut response, "Referrer-Policy", "no-referrer");
        response
    }
}

fn add_header(response: &mut Response<Cursor<Vec<u8>>>, name: &str, value: &str) {
    if let Ok(header) = Header::from_bytes(name.as_bytes(), value.as_bytes()) {
        response.add_header(header);
    }
}

fn split_token_rel(url: &str) -> Option<(&str, &str)> {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let path = path.trim_start_matches('/');
    path.split_once('/')
}

fn is_hex_token(token: &str) -> bool {
    token.len() == 64 && token.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn decode_rel_components(rel: &str) -> Result<Vec<String>, ()> {
    let decoded = percent_decode(rel)?;
    if decoded.contains('\0') || decoded.contains('\\') {
        return Err(());
    }
    let mut parts = Vec::new();
    for part in decoded.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." || part.contains('\0') || part.starts_with('.') {
            return Err(());
        }
        parts.push(part.to_string());
    }
    if parts.is_empty() {
        return Err(());
    }
    Ok(parts)
}

fn percent_decode(input: &str) -> Result<String, ()> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).map_err(|_| ())?;
            let value = u8::from_str_radix(hex, 16).map_err(|_| ())?;
            if value == 0 {
                return Err(());
            }
            out.push(value);
            index += 3;
            continue;
        }
        if bytes[index] == 0 {
            return Err(());
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8(out).map_err(|_| ())
}

fn random_token() -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let bytes = rand::random::<[u8; 32]>();
    let mut out = String::with_capacity(64);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn mime_of(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" | "cjs" => "text/javascript; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "txt" => "text/plain; charset=utf-8",
        "wasm" => "application/octet-stream",
        _ => "application/octet-stream",
    }
}

#[tauri::command(async)]
pub fn preview_create(
    path: String,
    state: State<'_, PreviewServerState>,
) -> Result<PreviewSessionInfo, String> {
    state.create_session(&path)
}

#[tauri::command(async)]
pub fn preview_revoke(token: String, state: State<'_, PreviewServerState>) -> Result<(), String> {
    state.revoke_session(&token);
    Ok(())
}

#[tauri::command(async)]
pub fn preview_stop_all(state: State<'_, PreviewServerState>) -> Result<(), String> {
    state.stop_all();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;

    fn write_file(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let mut file = fs::File::create(path).unwrap();
        file.write_all(contents.as_bytes()).unwrap();
    }

    struct Fixture {
        _root: tempfile::TempDir,
        state: PreviewServerState,
        session: PreviewSessionInfo,
        leak: String,
    }

    impl Fixture {
        fn new() -> Self {
            let root = tempfile::tempdir().unwrap();
            write_file(
                &root.path().join("index.html"),
                r#"<!doctype html>
                <link rel="stylesheet" href="app.css">
                <script type="module" src="app.js"></script>
                <img src="logo.png">
                <img src="nested/photo.jpg">
                "#,
            );
            write_file(
                &root.path().join("app.css"),
                r#"@import url("more.css"); body { background: url(bg.png); }"#,
            );
            write_file(&root.path().join("more.css"), "body{color:red}");
            write_file(
                &root.path().join("app.js"),
                r#"import { x } from "./mod.js";"#,
            );
            write_file(&root.path().join("mod.js"), "export const x = 1;");
            write_file(&root.path().join("logo.png"), "png");
            write_file(&root.path().join("bg.png"), "bg");
            write_file(&root.path().join("nested/photo.jpg"), "jpg");
            write_file(&root.path().join("secret.txt"), "undeclared");
            write_file(&root.path().join(".env"), "SECRET=1");
            write_file(&root.path().join(".git/config"), "[core]");
            write_file(&root.path().join("id_rsa"), "-----BEGIN");
            let leak = root.path().display().to_string();
            let state = PreviewServerState::new();
            let session = state
                .create_session(root.path().join("index.html").to_str().unwrap())
                .expect("session");
            Self {
                _root: root,
                state,
                session,
                leak,
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            self.state.stop_all();
        }
    }

    fn call(url: &str) -> (u16, HashMap<String, String>, String) {
        let response = ureq::get(url)
            .config()
            .http_status_as_error(false)
            .build()
            .call();
        match response {
            Ok(mut resp) => {
                let status = resp.status().as_u16();
                let mut headers = HashMap::new();
                for name in [
                    "cache-control",
                    "x-content-type-options",
                    "content-type",
                    "content-security-policy",
                    "access-control-allow-origin",
                ] {
                    if let Some(value) = resp.headers().get(name) {
                        headers.insert(
                            name.to_string(),
                            value.to_str().unwrap_or_default().to_string(),
                        );
                    }
                }
                let body = resp.body_mut().read_to_string().unwrap_or_default();
                (status, headers, body)
            }
            Err(_) => (0, HashMap::new(), String::new()),
        }
    }

    fn session_url(session: &PreviewSessionInfo, rel: &str) -> String {
        let base = session.url.rsplit_once('/').map(|(head, _)| head).unwrap();
        format!("{base}/{rel}")
    }

    #[test]
    fn declared_assets_are_served_and_undeclared_are_forbidden() {
        let fx = Fixture::new();
        for rel in [
            "index.html",
            "app.css",
            "more.css",
            "app.js",
            "mod.js",
            "logo.png",
            "bg.png",
            "nested/photo.jpg",
        ] {
            let (status, _, _) = call(&session_url(&fx.session, rel));
            assert_eq!(status, 200, "{rel}");
        }
        let (status, _, body) = call(&session_url(&fx.session, "secret.txt"));
        assert_eq!(status, 403);
        assert!(!body.contains(&fx.leak));
    }

    #[test]
    fn secrets_dotfiles_traversal_and_nul_are_forbidden() {
        let fx = Fixture::new();
        for rel in [
            ".env",
            ".git/config",
            "id_rsa",
            "%2e%2e/.env",
            "..%2f.env",
            "index.html%00.env",
            "nested/../.env",
        ] {
            let (status, _, body) = call(&session_url(&fx.session, rel));
            assert_eq!(status, 403, "{rel}");
            assert!(!body.contains(&fx.leak), "{rel}");
        }
    }

    #[test]
    fn query_and_fragment_cannot_escape_allowlist() {
        let fx = Fixture::new();
        let html = session_url(&fx.session, "index.html");
        let (ok_status, _, _) = call(&format!("{html}?../../.env"));
        assert_eq!(ok_status, 200);
        let (frag_status, _, _) = call(&format!("{html}#/../.env"));
        assert_eq!(frag_status, 200);
        let (secret_status, _, _) = call(&format!(
            "{}?file=secret.txt",
            session_url(&fx.session, "secret.txt")
        ));
        assert_eq!(secret_status, 403);
    }

    #[test]
    fn unknown_expired_and_revoked_tokens_fail_closed() {
        let fx = Fixture::new();
        let url = fx.session.url.clone();
        let (unknown, _, _) = call(&url.replace(&fx.session.token, &"ab".repeat(32)));
        assert_eq!(unknown, 404);

        fx.state.expire_session(&fx.session.token);
        let (expired, _, body) = call(&url);
        assert_eq!(expired, 410);
        assert!(!body.contains(&fx.leak));

        let again = fx
            .state
            .create_session(fx._root.path().join("index.html").to_str().unwrap())
            .unwrap();
        assert_ne!(again.token, fx.session.token);
        let (old_status, _, _) = call(&url);
        assert_eq!(old_status, 404);
        fx.state.revoke_session(&again.token);
        let (revoked, _, _) = call(&again.url);
        assert_eq!(revoked, 404);
    }

    #[test]
    fn responses_have_restrictive_headers_and_no_directory_fallback() {
        let fx = Fixture::new();
        let (status, headers, body) = call(&fx.session.url);
        assert_eq!(status, 200);
        assert_eq!(
            headers.get("cache-control").map(String::as_str),
            Some("no-store")
        );
        assert_eq!(
            headers.get("x-content-type-options").map(String::as_str),
            Some("nosniff")
        );
        assert!(headers
            .get("content-type")
            .is_some_and(|value| value.starts_with("text/html")));
        let csp = headers
            .get("content-security-policy")
            .cloned()
            .unwrap_or_default();
        assert!(csp.contains("connect-src 'none'"));
        assert!(csp.contains("default-src 'none'"));
        assert!(!headers.contains_key("access-control-allow-origin"));
        assert!(!body.contains(&fx.leak));

        let prefix = fx
            .session
            .url
            .rsplit_once('/')
            .map(|(head, _)| head)
            .unwrap();
        let (dir_status, _, _) = call(&format!("{prefix}/"));
        assert_ne!(dir_status, 200);
        let (missing_index, _, _) = call(prefix);
        assert_ne!(missing_index, 200);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escape_is_forbidden() {
        let fx = Fixture::new();
        let outside = tempfile::NamedTempFile::new().unwrap();
        std::io::Write::write_all(
            &mut std::fs::File::create(outside.path()).unwrap(),
            b"escaped",
        )
        .ok();
        let link = fx._root.path().join("escape.css");
        std::os::unix::fs::symlink(outside.path(), &link).unwrap();
        let (status, _, body) = call(&session_url(&fx.session, "escape.css"));
        assert_eq!(status, 403);
        assert!(!body.contains(&fx.leak));
        assert!(!body.contains("escaped"));
    }

    #[cfg(unix)]
    #[test]
    fn allowlisted_file_swapped_to_symlink_is_forbidden() {
        let fx = Fixture::new();
        let outside = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(outside.path(), b"escaped-secret").unwrap();
        let target = fx._root.path().join("app.css");
        std::fs::remove_file(&target).unwrap();
        std::os::unix::fs::symlink(outside.path(), &target).unwrap();
        let (status, _, body) = call(&session_url(&fx.session, "app.css"));
        assert_eq!(status, 403);
        assert!(!body.contains(&fx.leak));
        assert!(!body.contains("escaped-secret"));
    }

    #[test]
    fn oversized_graph_fails_closed() {
        let root = tempfile::tempdir().unwrap();
        let mut html = String::from("<!doctype html>");
        for index in 0..70 {
            html.push_str(&format!(r#"<img src="img{index}.png">"#));
            write_file(&root.path().join(format!("img{index}.png")), "x");
        }
        write_file(&root.path().join("index.html"), &html);
        let state = PreviewServerState::new();
        let err = state
            .create_session(root.path().join("index.html").to_str().unwrap())
            .unwrap_err();
        assert_eq!(err, "preview-graph-too-large");
        state.stop_all();
    }
}
