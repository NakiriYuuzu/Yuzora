// P3 (b): a tiny localhost static-file server so a right-clicked HTML file can be
// previewed at a real http origin — relative `<link>`/`<script>` and ES module
// specifiers resolve exactly as they would in a browser (the asset protocol
// breaks them). Servers bind 127.0.0.1 only, are keyed by (canonical) root dir so
// the same folder reuses one port, and are stopped on app exit.
//
// Path-traversal defence lives in `resolve`: the requested path is canonicalized
// and must still sit inside the canonical root, so `GET /../../etc/passwd` (or a
// symlink escape) is rejected before any bytes are read.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use tiny_http::{Header, Request, Response, Server};

const RECV_TIMEOUT_MS: u64 = 200;

pub struct PreviewServerState(pub Mutex<HashMap<PathBuf, PreviewServer>>);

pub struct PreviewServer {
    pub port: u16,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl PreviewServer {
    fn shutdown(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl PreviewServerState {
    /// Serve `dir` over http on 127.0.0.1 and return the port. Reuses the existing
    /// server (and port) when the same canonical directory is already served.
    pub fn serve(&self, dir: &str) -> Result<u16, String> {
        let root = PathBuf::from(dir)
            .canonicalize()
            .map_err(|e| format!("preview directory not accessible: {e}"))?;
        if !root.is_dir() {
            return Err("preview target is not a directory".to_string());
        }

        let mut servers = self.0.lock().map_err(|e| e.to_string())?;
        if let Some(existing) = servers.get(&root) {
            return Ok(existing.port);
        }

        let server = Server::http("127.0.0.1:0").map_err(|e| e.to_string())?;
        let port = server
            .server_addr()
            .to_ip()
            .ok_or_else(|| "preview server did not bind a TCP port".to_string())?
            .port();

        let server = Arc::new(server);
        let stop = Arc::new(AtomicBool::new(false));
        let handle = {
            let server = Arc::clone(&server);
            let stop = Arc::clone(&stop);
            let root = root.clone();
            thread::spawn(move || serve_loop(&server, &stop, &root))
        };

        servers.insert(
            root,
            PreviewServer {
                port,
                stop,
                handle: Some(handle),
            },
        );
        Ok(port)
    }

    pub fn stop_all(&self) {
        let taken: Vec<PreviewServer> = match self.0.lock() {
            Ok(mut servers) => servers.drain().map(|(_, s)| s).collect(),
            Err(_) => return,
        };
        for server in taken {
            server.shutdown();
        }
    }
}

fn serve_loop(server: &Server, stop: &AtomicBool, root: &Path) {
    while !stop.load(Ordering::SeqCst) {
        match server.recv_timeout(Duration::from_millis(RECV_TIMEOUT_MS)) {
            Ok(Some(request)) => handle_request(request, root),
            Ok(None) => continue,
            Err(_) => break,
        }
    }
}

fn handle_request(request: Request, root: &Path) {
    let url = request.url().to_string();
    match resolve(root, &url) {
        Resolved::File(path) => match fs::read(&path) {
            Ok(bytes) => {
                let mut response = Response::from_data(bytes);
                if let Ok(header) =
                    Header::from_bytes(&b"Content-Type"[..], content_type(&path).as_bytes())
                {
                    response.add_header(header);
                }
                let _ = request.respond(response);
            }
            Err(_) => respond_status(request, 404, "Not found"),
        },
        Resolved::NotFound => respond_status(request, 404, "Not found"),
        Resolved::Forbidden => respond_status(request, 403, "Forbidden"),
    }
}

fn respond_status(request: Request, status: u16, body: &str) {
    let _ = request.respond(Response::from_string(body).with_status_code(status));
}

enum Resolved {
    File(PathBuf),
    NotFound,
    Forbidden,
}

/// Map a request URL path to a file inside `root`. `root` must already be
/// canonical. Returns `Forbidden` when the (existing) target resolves outside
/// `root` — the path-traversal / symlink-escape guard.
fn resolve(root: &Path, url_path: &str) -> Resolved {
    let path = url_path.split(['?', '#']).next().unwrap_or("");
    let decoded = percent_decode(path);
    let rel = decoded.trim_start_matches('/');
    let candidate = if rel.is_empty() {
        root.join("index.html")
    } else {
        root.join(rel)
    };

    match candidate.canonicalize() {
        Ok(canonical) => {
            if !canonical.starts_with(root) {
                return Resolved::Forbidden;
            }
            if canonical.is_dir() {
                match canonical.join("index.html").canonicalize() {
                    Ok(index) if index.starts_with(root) => Resolved::File(index),
                    Ok(_) => Resolved::Forbidden,
                    Err(_) => Resolved::NotFound,
                }
            } else {
                Resolved::File(canonical)
            }
        }
        Err(_) => Resolved::NotFound,
    }
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(hi), Some(lo)) = (hi, lo) {
                out.push((hi * 16 + lo) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn content_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("html") | Some("htm") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") | Some("mjs") => "text/javascript; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("wasm") => "application/wasm",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("map") | Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

#[tauri::command]
pub fn preview_serve(
    dir: String,
    state: tauri::State<'_, PreviewServerState>,
) -> Result<u16, String> {
    state.serve(&dir)
}

#[tauri::command]
pub fn preview_stop_all(state: tauri::State<'_, PreviewServerState>) -> Result<(), String> {
    state.stop_all();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn read_body(url: &str) -> (u16, String) {
        let response = ureq::get(url).call();
        match response {
            Ok(resp) => {
                let status = resp.status();
                let mut body = String::new();
                let _ = resp.into_reader().read_to_string(&mut body);
                (status, body)
            }
            Err(ureq::Error::Status(status, resp)) => {
                let mut body = String::new();
                let _ = resp.into_reader().read_to_string(&mut body);
                (status, body)
            }
            Err(_) => (0, String::new()),
        }
    }

    #[test]
    fn resolve_serves_files_inside_root_and_rejects_traversal() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("site");
        fs::create_dir_all(root.join("nested")).unwrap();
        fs::write(root.join("index.html"), "<h1>hi</h1>").unwrap();
        fs::write(root.join("nested/page.html"), "<p>nested</p>").unwrap();
        fs::write(root.join("nested/index.html"), "<p>nested index</p>").unwrap();
        // A secret living OUTSIDE the served root — the traversal target.
        fs::write(tmp.path().join("secret.txt"), "TOP SECRET").unwrap();

        let root = root.canonicalize().unwrap();

        assert!(matches!(resolve(&root, "/index.html"), Resolved::File(_)));
        assert!(matches!(resolve(&root, "/"), Resolved::File(_)));
        assert!(matches!(
            resolve(&root, "/nested/page.html"),
            Resolved::File(_)
        ));
        // A directory resolves to its index.html.
        assert!(matches!(resolve(&root, "/nested"), Resolved::File(_)));
        // Missing file → NotFound (canonicalize fails, no escape possible).
        assert!(matches!(
            resolve(&root, "/missing.html"),
            Resolved::NotFound
        ));
        // Traversal to an EXISTING file outside root → Forbidden (the guard).
        assert!(matches!(
            resolve(&root, "/../secret.txt"),
            Resolved::Forbidden
        ));
        assert!(matches!(
            resolve(&root, "/nested/../../secret.txt"),
            Resolved::Forbidden
        ));
        // Percent-encoded traversal is decoded first, then still rejected.
        assert!(matches!(
            resolve(&root, "/..%2fsecret.txt"),
            Resolved::Forbidden
        ));
    }

    #[test]
    fn serve_reuses_one_port_per_root_and_serves_over_http() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("index.html"), "<title>ok</title>").unwrap();
        let dir = tmp.path().to_str().unwrap().to_string();

        let state = PreviewServerState(Mutex::new(HashMap::new()));
        let port = state.serve(&dir).unwrap();
        // Same dir → same port (registry reuse), no second server spun up.
        assert_eq!(state.serve(&dir).unwrap(), port);
        assert_eq!(state.0.lock().unwrap().len(), 1);

        let (status, body) = read_body(&format!("http://127.0.0.1:{port}/index.html"));
        assert_eq!(status, 200);
        assert!(body.contains("<title>ok</title>"));

        // Root path serves index.html.
        let (root_status, root_body) = read_body(&format!("http://127.0.0.1:{port}/"));
        assert_eq!(root_status, 200);
        assert!(root_body.contains("<title>ok</title>"));

        state.stop_all();
        assert!(state.0.lock().unwrap().is_empty());
    }
}
