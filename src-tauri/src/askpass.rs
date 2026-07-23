// M2 Task 5: askpass 認證橋接（Rust 側）
//
// 主 binary askpass 模式（spike 2026-07-02 定案）：
// - main() 在 tauri 啟動前偵測 YUZORA_ASKPASS_ENDPOINT env → 進 client 模式。
// - app setup 起一個常駐 unix socket server；git spawn 時經 env_for 注入 GIT_ASKPASS/SSH_ASKPASS
//   指向 current_exe，client 連 socket 送 token+prompt、server 回 secret（或空行＝取消）。
// - 憑證只經此 socket 通道，不落盤、不進 argv、不進 log。

/// UI 需要顯示的憑證請求。序列化為 git:askpass-request 事件送前端。
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AskpassRequest {
    pub id: u64,
    pub prompt: String,
    pub kind: String,
}

/// prompt 前綴/子字串 → kind 分類。
#[cfg(unix)]
fn classify(prompt: &str) -> &'static str {
    if prompt.starts_with("Username") {
        "username"
    } else if prompt.contains("assword") {
        "password"
    } else if prompt.starts_with("Enter passphrase") {
        "passphrase"
    } else if prompt.contains("continue connecting") {
        "fingerprint"
    } else {
        "other"
    }
}

/// 快取 key＝prompt 去尾端 `: ` 後全文（保留多行前綴內容）。
#[cfg(unix)]
fn cache_key(prompt: &str) -> String {
    prompt.strip_suffix(": ").unwrap_or(prompt).to_string()
}

#[cfg(unix)]
pub use unix_impl::*;

#[cfg(unix)]
mod unix_impl {
    use super::{cache_key, classify, AskpassRequest};
    use std::collections::HashMap;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::sync::mpsc::{channel, Sender};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    const CACHE_TTL: Duration = Duration::from_secs(60);
    const RECV_TIMEOUT: Duration = Duration::from_secs(120);

    struct Pending {
        next_id: u64,
        waiters: HashMap<u64, Sender<Option<String>>>,
    }

    struct CacheEntry {
        value: String,
        stored: Instant,
    }

    pub struct AskpassServer {
        endpoint: String,
        token: String,
        pending: Mutex<Pending>,
        cache: Mutex<HashMap<String, CacheEntry>>,
        emit: Box<dyn Fn(AskpassRequest) + Send + Sync + 'static>,
    }

    impl AskpassServer {
        pub fn start(
            emit: impl Fn(AskpassRequest) + Send + Sync + 'static,
        ) -> Result<Arc<AskpassServer>, String> {
            let endpoint = std::env::temp_dir()
                .join(format!(
                    "yz-ap-{}-{:x}.sock",
                    std::process::id(),
                    rand::random::<u64>()
                ))
                .to_string_lossy()
                .into_owned();
            let listener = UnixListener::bind(&endpoint)
                .map_err(|e| format!("askpass socket bind failed: {e}"))?;
            let token = format!("{:x}", rand::random::<u128>());
            let server = Arc::new(AskpassServer {
                endpoint,
                token,
                pending: Mutex::new(Pending {
                    next_id: 1,
                    waiters: HashMap::new(),
                }),
                cache: Mutex::new(HashMap::new()),
                emit: Box::new(emit),
            });
            // Hold a Weak (not a strong Arc) in the accept loop so the server's Drop
            // can actually run — a strong clone here would pin the Arc for the
            // process lifetime and the socket file would never be cleaned up.
            let accept_server = Arc::downgrade(&server);
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    match stream {
                        Ok(stream) => {
                            let Some(s) = accept_server.upgrade() else {
                                break;
                            };
                            std::thread::spawn(move || s.handle_connection(stream));
                        }
                        Err(_) => break,
                    }
                }
            });
            Ok(server)
        }

        pub fn env_for(&self, background: bool) -> Vec<(String, String)> {
            let exe = std::env::current_exe()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default();
            vec![
                ("GIT_ASKPASS".to_string(), exe.clone()),
                ("SSH_ASKPASS".to_string(), exe),
                ("SSH_ASKPASS_REQUIRE".to_string(), "force".to_string()),
                ("YUZORA_ASKPASS_ENDPOINT".to_string(), self.endpoint.clone()),
                ("YUZORA_ASKPASS_TOKEN".to_string(), self.token.clone()),
                (
                    "YUZORA_ASKPASS_BACKGROUND".to_string(),
                    if background { "1" } else { "0" }.to_string(),
                ),
            ]
        }

        /// UI 回覆：Some(secret) 或 None（取消）。喚醒對應連線 thread。
        pub fn respond(&self, id: u64, response: Option<String>) {
            let sender = self.pending.lock().unwrap().waiters.remove(&id);
            if let Some(sender) = sender {
                let _ = sender.send(response);
            }
        }

        fn cache_get(&self, key: &str) -> Option<String> {
            let mut cache = self.cache.lock().unwrap();
            if let Some(entry) = cache.get(key) {
                if entry.stored.elapsed() < CACHE_TTL {
                    return Some(entry.value.clone());
                }
                cache.remove(key);
            }
            None
        }

        fn cache_put(&self, key: String, value: String) {
            self.cache.lock().unwrap().insert(
                key,
                CacheEntry {
                    value,
                    stored: Instant::now(),
                },
            );
        }

        fn handle_connection(&self, mut stream: UnixStream) {
            let mut reader = BufReader::new(match stream.try_clone() {
                Ok(s) => s,
                Err(_) => return,
            });
            let mut line = String::new();
            if reader.read_line(&mut line).is_err() {
                return;
            }
            let secret = self.resolve(line.trim_end_matches('\n'));
            // 一律回一行：secret（可空）＋ \n。空行＝取消/拒絕。
            let _ = stream.write_all(secret.as_bytes());
            let _ = stream.write_all(b"\n");
            let _ = stream.flush();
        }

        /// 協定核心：吃一行 request JSON，回覆 secret（空＝取消/拒絕）。
        fn resolve(&self, request_line: &str) -> String {
            let (token, background, prompt) = match parse_request(request_line) {
                Some(v) => v,
                None => return String::new(),
            };
            if token != self.token {
                return String::new();
            }
            let key = cache_key(&prompt);
            if let Some(hit) = self.cache_get(&key) {
                return hit;
            }
            if background {
                // 背景操作絕不打斷使用者：無快取即失敗，永不 emit。
                return String::new();
            }
            // 配 id、登記 waiter、emit UI 請求，等 UI 回覆。
            let (tx, rx) = channel::<Option<String>>();
            let id = {
                let mut pending = self.pending.lock().unwrap();
                let id = pending.next_id;
                pending.next_id += 1;
                pending.waiters.insert(id, tx);
                id
            };
            (self.emit)(AskpassRequest {
                id,
                prompt: prompt.clone(),
                kind: classify(&prompt).to_string(),
            });
            match rx.recv_timeout(RECV_TIMEOUT) {
                Ok(Some(value)) => {
                    self.cache_put(key, value.clone());
                    value
                }
                // None（取消）或逾時：清掉 waiter、回空行、不入快取。
                _ => {
                    self.pending.lock().unwrap().waiters.remove(&id);
                    String::new()
                }
            }
        }
    }

    impl Drop for AskpassServer {
        fn drop(&mut self) {
            // Best-effort cleanup of the bound socket file so a crashed/exited app
            // doesn't leave stale sockets behind in temp_dir.
            let _ = std::fs::remove_file(&self.endpoint);
        }
    }

    /// 解析 client 送來的一行 JSON `{"token","background","prompt"}`。
    fn parse_request(line: &str) -> Option<(String, bool, String)> {
        let v: serde_json::Value = serde_json::from_str(line).ok()?;
        let token = v.get("token")?.as_str()?.to_string();
        let background = v.get("background")?.as_bool()?;
        let prompt = v.get("prompt")?.as_str()?.to_string();
        Some((token, background, prompt))
    }

    /// client 核心：連 socket、送 request JSON、讀一行回覆。
    /// 參數化（不讀全域 env）以避免測試間 env 競態。
    /// 回 (exit_code, value)：非空回覆→(0, value)；空/錯誤→(1, "")。
    pub fn run_client_impl(
        endpoint: &str,
        token: &str,
        background: bool,
        prompt: &str,
    ) -> (i32, String) {
        let request = serde_json::json!({
            "token": token,
            "background": background,
            "prompt": prompt,
        })
        .to_string();
        let mut stream = match UnixStream::connect(endpoint) {
            Ok(s) => s,
            Err(_) => return (1, String::new()),
        };
        if stream.write_all(request.as_bytes()).is_err()
            || stream.write_all(b"\n").is_err()
            || stream.flush().is_err()
        {
            return (1, String::new());
        }
        let mut response = String::new();
        if stream.read_to_string(&mut response).is_err() {
            return (1, String::new());
        }
        let value = response.trim_end_matches('\n').to_string();
        if value.is_empty() {
            (1, String::new())
        } else {
            (0, value)
        }
    }

    /// main.rs 呼叫的 client 入口：從 env 取 token/background，print secret 到 stdout，回 exit code。
    pub fn run_client(endpoint: &str, prompt: &str) -> i32 {
        let token = std::env::var("YUZORA_ASKPASS_TOKEN").unwrap_or_default();
        let background = std::env::var("YUZORA_ASKPASS_BACKGROUND").as_deref() == Ok("1");
        let (code, value) = run_client_impl(endpoint, &token, background, prompt);
        if code == 0 {
            print!("{value}");
        }
        code
    }

    /// None＝askpass server 啟動失敗（降級）。消費端一律經 env_for；None 回空 env
    /// （git 仍可用系統 credential helper）。
    pub struct AskpassState(pub Option<Arc<AskpassServer>>);

    impl AskpassState {
        /// server 存在→注入 askpass env；不存在（降級）→空 Vec，git 不 panic。
        pub fn env_for(&self, background: bool) -> Vec<(String, String)> {
            match &self.0 {
                Some(server) => server.env_for(background),
                None => Vec::new(),
            }
        }
    }

    #[tauri::command]
    pub fn askpass_respond(
        state: tauri::State<'_, AskpassState>,
        id: u64,
        response: Option<String>,
    ) {
        if let Some(server) = &state.0 {
            server.respond(id, response);
        }
    }
}

#[cfg(windows)]
pub use windows_impl::*;

#[cfg(windows)]
mod windows_impl {
    use super::AskpassRequest;
    use std::sync::Arc;

    pub struct AskpassServer;

    impl AskpassServer {
        pub fn start(
            _emit: impl Fn(AskpassRequest) + Send + Sync + 'static,
        ) -> Result<Arc<AskpassServer>, String> {
            Err("askpass not yet supported on Windows".to_string())
        }

        pub fn env_for(&self, _background: bool) -> Vec<(String, String)> {
            Vec::new()
        }

        pub fn respond(&self, _id: u64, _response: Option<String>) {}
    }

    pub fn run_client(_endpoint: &str, _prompt: &str) -> i32 {
        1
    }

    pub struct AskpassState(pub Option<Arc<AskpassServer>>);

    impl AskpassState {
        pub fn env_for(&self, _background: bool) -> Vec<(String, String)> {
            Vec::new()
        }
    }

    #[tauri::command]
    pub fn askpass_respond(
        _state: tauri::State<'_, AskpassState>,
        _id: u64,
        _response: Option<String>,
    ) {
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    fn value_of(env: &[(String, String)], key: &str) -> String {
        env.iter().find(|(k, _)| k == key).unwrap().1.clone()
    }
    fn start_with_reply(reply: Option<&'static str>) -> std::sync::Arc<AskpassServer> {
        let server_slot: std::sync::Arc<std::sync::Mutex<Option<std::sync::Arc<AskpassServer>>>> =
            Default::default();
        let slot2 = server_slot.clone();
        let server = AskpassServer::start(move |req| {
            let s = slot2.lock().unwrap().clone().unwrap();
            s.respond(req.id, reply.map(String::from));
        })
        .unwrap();
        *server_slot.lock().unwrap() = Some(server.clone());
        server
    }

    #[test]
    fn roundtrip_returns_ui_response() {
        let server = start_with_reply(Some("s3cret"));
        let env = server.env_for(false);
        let endpoint = value_of(&env, "YUZORA_ASKPASS_ENDPOINT");
        let token = value_of(&env, "YUZORA_ASKPASS_TOKEN");
        assert_eq!(
            run_client_impl(&endpoint, &token, false, "Password for 'x': "),
            (0, "s3cret".into())
        );
    }

    #[test]
    fn wrong_token_gets_empty() {
        let server = start_with_reply(Some("nope-should-not-reach"));
        let env = server.env_for(false);
        let endpoint = value_of(&env, "YUZORA_ASKPASS_ENDPOINT");
        assert_eq!(
            run_client_impl(&endpoint, "wrong", false, "Password: ").0,
            1
        );
    }

    #[test]
    fn background_never_emits_and_fails_fast() {
        let emitted = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let e2 = emitted.clone();
        let server = AskpassServer::start(move |_| {
            e2.store(true, std::sync::atomic::Ordering::SeqCst);
        })
        .unwrap();
        let env = server.env_for(true);
        let endpoint = value_of(&env, "YUZORA_ASKPASS_ENDPOINT");
        let token = value_of(&env, "YUZORA_ASKPASS_TOKEN");
        assert_eq!(run_client_impl(&endpoint, &token, true, "Password: ").0, 1);
        assert!(!emitted.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn askpass_state_none_yields_empty_env_without_panic() {
        // 降級路徑：server 啟動失敗時 manage AskpassState(None)。四個 remote command 經
        // env_for 取空 env，不得 panic（純函式層驗證）。
        let degraded = AskpassState(None);
        assert!(degraded.env_for(false).is_empty());
        assert!(degraded.env_for(true).is_empty());
        // 對照：server 存在時 env_for 注入非空 askpass env。
        let server = start_with_reply(None);
        let live = AskpassState(Some(server));
        assert!(!live.env_for(false).is_empty());
    }

    #[test]
    fn drop_removes_socket_file() {
        // The bound socket file must be cleaned up when the server is dropped so a
        // crashed/exited app doesn't leave stale sockets in temp_dir. Uses an empty
        // emit closure so there's no reference cycle keeping the Arc alive.
        let server = AskpassServer::start(|_req| {}).unwrap();
        let endpoint = value_of(&server.env_for(false), "YUZORA_ASKPASS_ENDPOINT");
        assert!(std::path::Path::new(&endpoint).exists());
        drop(server);
        assert!(!std::path::Path::new(&endpoint).exists());
    }

    #[test]
    fn cache_hits_within_ttl_without_second_emit() {
        let count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let c2 = count.clone();
        let server_slot: std::sync::Arc<std::sync::Mutex<Option<std::sync::Arc<AskpassServer>>>> =
            Default::default();
        let slot2 = server_slot.clone();
        let server = AskpassServer::start(move |req| {
            c2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            slot2
                .lock()
                .unwrap()
                .clone()
                .unwrap()
                .respond(req.id, Some("pw".into()));
        })
        .unwrap();
        *server_slot.lock().unwrap() = Some(server.clone());
        let env = server.env_for(false);
        let endpoint = value_of(&env, "YUZORA_ASKPASS_ENDPOINT");
        let token = value_of(&env, "YUZORA_ASKPASS_TOKEN");
        assert_eq!(
            run_client_impl(&endpoint, &token, false, "Password for 'r': ").1,
            "pw"
        );
        assert_eq!(
            run_client_impl(&endpoint, &token, false, "Password for 'r': ").1,
            "pw"
        );
        assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}
