// M2 Task 5 + F7: operation-scoped askpass 認證橋接（Rust 側）
//
// 主 binary askpass 模式（spike 2026-07-02 定案，F7 改為 per-operation capability）：
// - main() 在 tauri 啟動前偵測 YUZORA_ASKPASS_ENDPOINT env → 進 client 模式。
// - app setup 起一個常駐 unix socket server；每次 fetch/pull/push/probe 經
//   begin_operation 注入只屬於該次操作的 token/env。
// - 憑證只經此 socket 通道，不落盤、不進 argv、不進 log。
// - prompt 文字只當顯示用，絕不作 authority / cache identity。

/// UI 需要顯示的憑證請求。序列化為 git:askpass-request 事件送前端。
/// `prompt` 是 Git 送來的未信任顯示文字；其餘欄位來自 backend operation context。
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AskpassRequest {
    pub id: u64,
    pub prompt: String,
    pub kind: String,
    pub repository_display: String,
    pub repository_canonical: String,
    pub operation: String,
    pub remote_display: Option<String>,
    pub background: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AskpassOperationKind {
    Fetch,
    Pull,
    Push,
    Probe,
}

impl AskpassOperationKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fetch => "fetch",
            Self::Pull => "pull",
            Self::Push => "push",
            Self::Probe => "probe",
        }
    }
}

#[derive(Clone, Debug)]
pub struct AskpassOperationContext {
    pub repository_display: String,
    pub repository_canonical: String,
    pub remote_display: Option<String>,
    pub remote_fingerprint: Option<String>,
    pub operation: AskpassOperationKind,
    pub background: bool,
}

/// RAII capability：Drop 時 revoke token、釋放 waiter。
pub struct AskpassOperationGuard {
    env: Vec<(String, String)>,
    revoke: Option<Box<dyn FnOnce() + Send>>,
    bind: Option<std::sync::Arc<dyn Fn(u32) + Send + Sync>>,
}

impl AskpassOperationGuard {
    fn empty() -> Self {
        Self {
            env: Vec::new(),
            revoke: None,
            bind: None,
        }
    }

    pub fn env(&self) -> &[(String, String)] {
        &self.env
    }

    pub fn bind_root_pid(&self, pid: u32) {
        if let Some(bind) = &self.bind {
            bind(pid);
        }
    }
}

impl std::fmt::Debug for AskpassOperationGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AskpassOperationGuard")
            .field(
                "env_keys",
                &self
                    .env
                    .iter()
                    .map(|(key, _)| key.as_str())
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl Drop for AskpassOperationGuard {
    fn drop(&mut self) {
        if let Some(revoke) = self.revoke.take() {
            revoke();
        }
    }
}

/// prompt 前綴/子字串 → kind 分類。kind 才是 cache identity，prompt 本文不是。
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

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn random_hex() -> String {
    hex_encode(&rand::random::<[u8; 32]>())
}

#[cfg(unix)]
pub use unix_impl::*;

#[cfg(unix)]
mod unix_impl {
    use super::{
        classify, random_hex, AskpassOperationContext, AskpassOperationGuard, AskpassRequest,
    };
    use secrecy::{ExposeSecret, SecretString};
    use std::collections::HashMap;
    use std::io::{BufRead, Read, Write};
    use std::os::fd::AsRawFd;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::sync::mpsc::{channel, Sender};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};
    use zeroize::Zeroizing;

    const RECV_TIMEOUT: Duration = Duration::from_secs(120);
    const OPERATION_TTL: Duration = Duration::from_secs(180);
    const MAX_REQUEST_LINE: usize = 16 * 1024;
    const MAX_PROMPT: usize = 8 * 1024;

    struct PendingWaiter {
        operation_id: String,
        sender: Sender<Option<SecretString>>,
    }

    struct Pending {
        next_id: u64,
        waiters: HashMap<u64, PendingWaiter>,
    }

    struct OperationRecord {
        token: String,
        context: AskpassOperationContext,
        #[allow(dead_code)]
        created: Instant,
        expires: Instant,
        root_pid: Option<u32>,
    }

    impl OperationRecord {
        fn is_live(&self) -> bool {
            Instant::now() < self.expires
        }
    }

    #[derive(Clone, Copy)]
    struct PeerIdentity {
        pid: Option<u32>,
        uid: Option<u32>,
    }

    pub struct AskpassServer {
        endpoint: String,
        operations: Mutex<HashMap<String, OperationRecord>>,
        pending: Mutex<Pending>,
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
            let server = Arc::new(AskpassServer {
                endpoint,
                operations: Mutex::new(HashMap::new()),
                pending: Mutex::new(Pending {
                    next_id: 1,
                    waiters: HashMap::new(),
                }),
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

        #[cfg(test)]
        pub fn endpoint(&self) -> &str {
            &self.endpoint
        }

        pub fn begin_operation(
            self: &Arc<Self>,
            ctx: AskpassOperationContext,
        ) -> AskpassOperationGuard {
            self.begin_operation_with_ttl(ctx, OPERATION_TTL)
        }

        pub(super) fn begin_operation_with_ttl(
            self: &Arc<Self>,
            ctx: AskpassOperationContext,
            ttl: Duration,
        ) -> AskpassOperationGuard {
            self.sweep_expired();
            let operation_id = random_hex();
            let token = random_hex();
            let now = Instant::now();
            let record = OperationRecord {
                token: token.clone(),
                context: ctx,
                created: now,
                expires: now + ttl,
                root_pid: None,
            };
            self.operations
                .lock()
                .unwrap()
                .insert(operation_id.clone(), record);

            let exe = std::env::current_exe()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default();
            let env = vec![
                ("GIT_ASKPASS".to_string(), exe.clone()),
                ("SSH_ASKPASS".to_string(), exe),
                ("SSH_ASKPASS_REQUIRE".to_string(), "force".to_string()),
                ("YUZORA_ASKPASS_ENDPOINT".to_string(), self.endpoint.clone()),
                ("YUZORA_ASKPASS_TOKEN".to_string(), token),
                ("YUZORA_ASKPASS_OPERATION".to_string(), operation_id.clone()),
            ];

            let revoke_server = Arc::clone(self);
            let revoke_id = operation_id.clone();
            let bind_server = Arc::clone(self);
            let bind_id = operation_id;
            AskpassOperationGuard {
                env,
                revoke: Some(Box::new(move || revoke_server.revoke(&revoke_id))),
                bind: Some(Arc::new(move |pid| {
                    bind_server.bind_root_pid(&bind_id, pid)
                })),
            }
        }

        fn bind_root_pid(&self, operation_id: &str, pid: u32) {
            let mut operations = self.operations.lock().unwrap();
            if let Some(operation) = operations.get_mut(operation_id) {
                if operation.is_live() {
                    operation.root_pid = Some(pid);
                }
            }
        }

        fn revoke(&self, operation_id: &str) {
            let _ = self.operations.lock().unwrap().remove(operation_id);
            let mut pending = self.pending.lock().unwrap();
            let ids: Vec<u64> = pending
                .waiters
                .iter()
                .filter(|(_, waiter)| waiter.operation_id == operation_id)
                .map(|(id, _)| *id)
                .collect();
            for id in ids {
                if let Some(waiter) = pending.waiters.remove(&id) {
                    let _ = waiter.sender.send(None);
                }
            }
        }

        fn sweep_expired(&self) {
            let now = Instant::now();
            let expired: Vec<String> = self
                .operations
                .lock()
                .unwrap()
                .iter()
                .filter(|(_, operation)| now >= operation.expires)
                .map(|(id, _)| id.clone())
                .collect();
            for id in expired {
                self.revoke(&id);
            }
        }

        /// UI 回覆：Some(secret) 或 None（取消）。喚醒對應連線 thread。
        pub fn respond(&self, id: u64, response: Option<String>) {
            let sender = self.pending.lock().unwrap().waiters.remove(&id);
            if let Some(waiter) = sender {
                let _ = waiter
                    .sender
                    .send(response.map(|value| SecretString::from(value)));
            }
        }

        fn handle_connection(&self, mut stream: UnixStream) {
            let peer = peer_identity(&stream);
            let reader = match stream.try_clone() {
                Ok(cloned) => cloned,
                Err(_) => return,
            };
            let Some(line) = read_bounded_line(reader) else {
                let _ = stream.write_all(b"\n");
                let _ = stream.flush();
                return;
            };
            let secret = self.resolve(&line, peer);
            let _ = stream.write_all(secret.as_bytes());
            let _ = stream.write_all(b"\n");
            let _ = stream.flush();
        }

        /// 協定核心：吃一行 request JSON，回覆 secret（空＝取消/拒絕）。
        fn resolve(&self, request_line: &str, peer: PeerIdentity) -> Zeroizing<String> {
            let request = match parse_request(request_line) {
                Some(request) => request,
                None => return Zeroizing::new(String::new()),
            };
            if request.prompt.len() > MAX_PROMPT {
                return Zeroizing::new(String::new());
            }

            self.sweep_expired();

            let mut operations = self.operations.lock().unwrap();
            let Some(operation) = operations.get_mut(&request.operation_id) else {
                return Zeroizing::new(String::new());
            };
            if operation.token != request.token || !operation.is_live() {
                return Zeroizing::new(String::new());
            }
            if !peer_allowed(peer, operation.root_pid) {
                return Zeroizing::new(String::new());
            }

            // Background policy 必須在任何 cache lookup 之前生效。
            if operation.context.background {
                return Zeroizing::new(String::new());
            }

            let kind = classify(&request.prompt);
            let context = operation.context.clone();
            drop(operations);

            let (tx, rx) = channel::<Option<SecretString>>();
            let id = {
                let mut pending = self.pending.lock().unwrap();
                let id = pending.next_id;
                pending.next_id += 1;
                pending.waiters.insert(
                    id,
                    PendingWaiter {
                        operation_id: request.operation_id.clone(),
                        sender: tx,
                    },
                );
                id
            };
            (self.emit)(AskpassRequest {
                id,
                prompt: request.prompt,
                kind: kind.to_string(),
                repository_display: context.repository_display,
                repository_canonical: context.repository_canonical,
                operation: context.operation.as_str().to_string(),
                remote_display: context.remote_display,
                background: context.background,
            });
            match rx.recv_timeout(RECV_TIMEOUT) {
                Ok(Some(value)) => Zeroizing::new(value.expose_secret().to_string()),
                _ => {
                    self.pending.lock().unwrap().waiters.remove(&id);
                    Zeroizing::new(String::new())
                }
            }
        }

        #[cfg(test)]
        pub(super) fn live_operation_count(&self) -> usize {
            self.sweep_expired();
            self.operations.lock().unwrap().len()
        }
    }

    impl Drop for AskpassServer {
        fn drop(&mut self) {
            let ids: Vec<String> = self.operations.lock().unwrap().keys().cloned().collect();
            for id in ids {
                self.revoke(&id);
            }
            // Best-effort cleanup of the bound socket file so a crashed/exited app
            // doesn't leave stale sockets behind in temp_dir.
            let _ = std::fs::remove_file(&self.endpoint);
        }
    }

    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct ClientRequest {
        token: String,
        operation_id: String,
        prompt: String,
    }

    fn parse_request(line: &str) -> Option<ClientRequest> {
        serde_json::from_str(line).ok()
    }

    fn read_bounded_line(stream: UnixStream) -> Option<String> {
        let mut reader = std::io::BufReader::new(stream);
        let mut limited = Read::take(reader.by_ref(), MAX_REQUEST_LINE as u64 + 1);
        let mut line = String::new();
        match limited.read_line(&mut line) {
            Ok(0) => None,
            Ok(_) if line.len() > MAX_REQUEST_LINE => None,
            Ok(_) => Some(line.trim_end_matches(['\r', '\n']).to_string()),
            Err(_) => None,
        }
    }

    fn peer_allowed(peer: PeerIdentity, root_pid: Option<u32>) -> bool {
        let ours = unsafe { libc::getuid() };
        if let Some(uid) = peer.uid {
            if uid != ours {
                return false;
            }
        }
        match (root_pid, peer.pid) {
            (Some(root), Some(pid)) => is_self_or_descendant(pid, root),
            // Credentials must not be returned or prompted until the Git child
            // process tree is bound. Bind happens immediately after spawn.
            (None, _) => false,
            // Platforms that cannot report a peer pid still require the
            // unguessable per-operation token checked above.
            (Some(_), None) => true,
        }
    }

    fn is_self_or_descendant(pid: u32, ancestor: u32) -> bool {
        let mut current = pid;
        for _ in 0..32 {
            if current == ancestor {
                return true;
            }
            match parent_pid(current) {
                Some(parent) if parent != current => current = parent,
                _ => return false,
            }
        }
        false
    }

    fn peer_identity(stream: &UnixStream) -> PeerIdentity {
        PeerIdentity {
            pid: peer_pid(stream),
            uid: peer_uid(stream),
        }
    }

    #[cfg(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    ))]
    fn peer_uid(stream: &UnixStream) -> Option<u32> {
        let mut uid: libc::uid_t = 0;
        let mut gid: libc::gid_t = 0;
        let rc = unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) };
        if rc == 0 {
            Some(uid)
        } else {
            None
        }
    }

    #[cfg(target_os = "linux")]
    fn peer_uid(stream: &UnixStream) -> Option<u32> {
        peer_ucred(stream).map(|cred| cred.uid)
    }

    #[cfg(not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly",
        target_os = "linux"
    )))]
    fn peer_uid(_stream: &UnixStream) -> Option<u32> {
        None
    }

    #[cfg(target_os = "macos")]
    fn peer_pid(stream: &UnixStream) -> Option<u32> {
        let mut pid: libc::pid_t = 0;
        let mut len = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
        let rc = unsafe {
            libc::getsockopt(
                stream.as_raw_fd(),
                libc::SOL_LOCAL,
                libc::LOCAL_PEERPID,
                &mut pid as *mut _ as *mut libc::c_void,
                &mut len,
            )
        };
        if rc == 0 && pid > 0 {
            Some(pid as u32)
        } else {
            None
        }
    }

    #[cfg(target_os = "linux")]
    fn peer_pid(stream: &UnixStream) -> Option<u32> {
        peer_ucred(stream).map(|cred| cred.pid as u32)
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    fn peer_pid(_stream: &UnixStream) -> Option<u32> {
        None
    }

    #[cfg(target_os = "linux")]
    fn peer_ucred(stream: &UnixStream) -> Option<libc::ucred> {
        let mut cred = libc::ucred {
            pid: 0,
            uid: 0,
            gid: 0,
        };
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        let rc = unsafe {
            libc::getsockopt(
                stream.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                &mut cred as *mut _ as *mut libc::c_void,
                &mut len,
            )
        };
        if rc == 0 && cred.pid > 0 {
            Some(cred)
        } else {
            None
        }
    }

    #[cfg(target_os = "linux")]
    fn parent_pid(pid: u32) -> Option<u32> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let close = stat.rfind(')')?;
        let rest = stat.get(close + 2..)?;
        let mut parts = rest.split_whitespace();
        let _state = parts.next()?;
        parts.next()?.parse().ok()
    }

    #[cfg(target_os = "macos")]
    fn parent_pid(pid: u32) -> Option<u32> {
        let mut info = unsafe { std::mem::zeroed::<libc::proc_bsdinfo>() };
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
        let got = unsafe {
            libc::proc_pidinfo(
                pid as i32,
                libc::PROC_PIDTBSDINFO,
                0,
                &mut info as *mut _ as *mut libc::c_void,
                size,
            )
        };
        if got == size && info.pbi_ppid > 0 {
            Some(info.pbi_ppid)
        } else {
            None
        }
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    fn parent_pid(_pid: u32) -> Option<u32> {
        None
    }

    /// client 核心：連 socket、送 request JSON、讀一行回覆。
    /// 參數化（不讀全域 env）以避免測試間 env 競態。
    /// 回 (exit_code, value)：非空回覆→(0, value)；空/錯誤→(1, "")。
    pub fn run_client_impl(
        endpoint: &str,
        token: &str,
        operation_id: &str,
        prompt: &str,
    ) -> (i32, String) {
        if prompt.len() > MAX_PROMPT {
            return (1, String::new());
        }
        run_client_raw(
            endpoint,
            &serde_json::json!({
                "token": token,
                "operationId": operation_id,
                "prompt": prompt,
            })
            .to_string(),
        )
    }

    pub(super) fn run_client_raw(endpoint: &str, request: &str) -> (i32, String) {
        if request.len() > MAX_REQUEST_LINE {
            return (1, String::new());
        }
        let mut stream = match UnixStream::connect(endpoint) {
            Ok(stream) => stream,
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

    /// main.rs 呼叫的 client 入口：從 env 取 token/operation，print secret 到 stdout，回 exit code。
    pub fn run_client(endpoint: &str, prompt: &str) -> i32 {
        let token = std::env::var("YUZORA_ASKPASS_TOKEN").unwrap_or_default();
        let operation_id = std::env::var("YUZORA_ASKPASS_OPERATION").unwrap_or_default();
        let (code, value) = run_client_impl(endpoint, &token, &operation_id, prompt);
        if code == 0 {
            print!("{value}");
        }
        code
    }

    /// None＝askpass server 啟動失敗（降級）。消費端一律經 begin_operation；None 回空 env
    /// （git 仍可用系統 credential helper）。
    #[derive(Clone)]
    pub struct AskpassState(pub Option<Arc<AskpassServer>>);

    impl AskpassState {
        /// server 存在→注入該次 operation 的 askpass env；不存在（降級）→空 Vec，git 不 panic。
        pub fn begin_operation(&self, ctx: AskpassOperationContext) -> AskpassOperationGuard {
            match &self.0 {
                Some(server) => server.begin_operation(ctx),
                None => AskpassOperationGuard::empty(),
            }
        }
    }

    #[tauri::command(async)]
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

#[cfg(not(unix))]
pub use non_unix_impl::*;

#[cfg(not(unix))]
mod non_unix_impl {
    use super::{AskpassOperationContext, AskpassOperationGuard, AskpassRequest};
    use std::sync::Arc;

    // Residual: this host has no askpass IPC and no named-pipe client identity.
    // Authority is still not app-wide — begin_operation never issues a reusable
    // token. Git falls back to the system credential helper.
    pub struct AskpassServer;

    impl AskpassServer {
        pub fn start(
            _emit: impl Fn(AskpassRequest) + Send + Sync + 'static,
        ) -> Result<Arc<AskpassServer>, String> {
            Err("askpass not supported on this platform".to_string())
        }

        pub fn respond(&self, _id: u64, _response: Option<String>) {}
    }

    pub fn run_client(_endpoint: &str, _prompt: &str) -> i32 {
        1
    }

    #[derive(Clone)]
    pub struct AskpassState(pub Option<Arc<AskpassServer>>);

    impl AskpassState {
        pub fn begin_operation(&self, _ctx: AskpassOperationContext) -> AskpassOperationGuard {
            AskpassOperationGuard::empty()
        }
    }

    #[tauri::command(async)]
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
    use std::io::Write;
    use std::os::unix::net::UnixStream;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    fn value_of(env: &[(String, String)], key: &str) -> String {
        env.iter().find(|(k, _)| k == key).unwrap().1.clone()
    }

    fn sample_ctx(background: bool, operation: AskpassOperationKind) -> AskpassOperationContext {
        AskpassOperationContext {
            repository_display: "repo".into(),
            repository_canonical: "/tmp/repo".into(),
            remote_display: Some("origin".into()),
            remote_fingerprint: Some("sha256:abc".into()),
            operation,
            background,
        }
    }

    fn ctx_named(
        name: &str,
        background: bool,
        operation: AskpassOperationKind,
    ) -> AskpassOperationContext {
        let mut ctx = sample_ctx(background, operation);
        ctx.repository_display = name.into();
        ctx.repository_canonical = format!("/tmp/{name}");
        ctx
    }

    fn bind_self(op: &AskpassOperationGuard) {
        op.bind_root_pid(std::process::id());
    }

    fn start_with_reply(reply: Option<&'static str>) -> Arc<AskpassServer> {
        let server_slot: Arc<Mutex<Option<Arc<AskpassServer>>>> = Default::default();
        let slot2 = server_slot.clone();
        let server = AskpassServer::start(move |req| {
            let s = slot2.lock().unwrap().clone().unwrap();
            s.respond(req.id, reply.map(String::from));
        })
        .unwrap();
        *server_slot.lock().unwrap() = Some(server.clone());
        server
    }

    fn wait_for_emits(slot: &Mutex<Vec<AskpassRequest>>, count: usize) -> Vec<AskpassRequest> {
        for _ in 0..200 {
            let current = slot.lock().unwrap();
            if current.len() >= count {
                return current.clone();
            }
            drop(current);
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!(
            "timed out waiting for {count} askpass emits, got {}",
            slot.lock().unwrap().len()
        )
    }

    #[test]
    fn roundtrip_returns_ui_response() {
        let server = start_with_reply(Some("s3cret"));
        let op = server.begin_operation(sample_ctx(false, AskpassOperationKind::Fetch));
        bind_self(&op);
        let env = op.env();
        let endpoint = value_of(env, "YUZORA_ASKPASS_ENDPOINT");
        let token = value_of(env, "YUZORA_ASKPASS_TOKEN");
        let operation_id = value_of(env, "YUZORA_ASKPASS_OPERATION");
        assert_eq!(
            run_client_impl(&endpoint, &token, &operation_id, "Password for 'x': "),
            (0, "s3cret".into())
        );
    }

    #[test]
    fn wrong_token_gets_empty() {
        let server = start_with_reply(Some("nope-should-not-reach"));
        let op = server.begin_operation(sample_ctx(false, AskpassOperationKind::Pull));
        let env = op.env();
        let endpoint = value_of(env, "YUZORA_ASKPASS_ENDPOINT");
        let operation_id = value_of(env, "YUZORA_ASKPASS_OPERATION");
        assert_eq!(
            run_client_impl(&endpoint, "wrong", &operation_id, "Password: ").0,
            1
        );
    }

    #[test]
    fn wrong_context_token_denied() {
        let server = start_with_reply(Some("nope"));
        let a = server.begin_operation(ctx_named("a", false, AskpassOperationKind::Fetch));
        let b = server.begin_operation(ctx_named("b", false, AskpassOperationKind::Push));
        let endpoint = value_of(a.env(), "YUZORA_ASKPASS_ENDPOINT");
        let token_a = value_of(a.env(), "YUZORA_ASKPASS_TOKEN");
        let op_b = value_of(b.env(), "YUZORA_ASKPASS_OPERATION");
        assert_eq!(
            run_client_impl(&endpoint, &token_a, &op_b, "Password: ").0,
            1
        );
    }

    #[test]
    fn background_never_emits_and_fails_fast() {
        let emitted = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let e2 = emitted.clone();
        let server = AskpassServer::start(move |_| {
            e2.store(true, std::sync::atomic::Ordering::SeqCst);
        })
        .unwrap();
        let op = server.begin_operation(sample_ctx(true, AskpassOperationKind::Probe));
        bind_self(&op);
        let env = op.env();
        let endpoint = value_of(env, "YUZORA_ASKPASS_ENDPOINT");
        let token = value_of(env, "YUZORA_ASKPASS_TOKEN");
        let operation_id = value_of(env, "YUZORA_ASKPASS_OPERATION");
        assert_eq!(
            run_client_impl(&endpoint, &token, &operation_id, "Password: ").0,
            1
        );
        assert!(!emitted.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn background_denied_does_not_prompt() {
        let emitted = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let e2 = emitted.clone();
        let server = AskpassServer::start(move |_| {
            e2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        })
        .unwrap();
        let op = server.begin_operation(sample_ctx(true, AskpassOperationKind::Fetch));
        bind_self(&op);
        let env = op.env();
        let endpoint = value_of(env, "YUZORA_ASKPASS_ENDPOINT");
        let token = value_of(env, "YUZORA_ASKPASS_TOKEN");
        let operation_id = value_of(env, "YUZORA_ASKPASS_OPERATION");
        assert_eq!(
            run_client_impl(&endpoint, &token, &operation_id, "Password for 'x': "),
            (1, String::new())
        );
        assert_eq!(emitted.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[test]
    fn operation_b_cannot_read_operation_a_warm_secret() {
        let count = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let c2 = count.clone();
        let server_slot: Arc<Mutex<Option<Arc<AskpassServer>>>> = Default::default();
        let slot2 = server_slot.clone();
        let server = AskpassServer::start(move |req| {
            c2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            slot2
                .lock()
                .unwrap()
                .clone()
                .unwrap()
                .respond(req.id, Some("pw-a".into()));
        })
        .unwrap();
        *server_slot.lock().unwrap() = Some(server.clone());

        let a = server.begin_operation(ctx_named("a", false, AskpassOperationKind::Fetch));
        bind_self(&a);
        let endpoint = value_of(a.env(), "YUZORA_ASKPASS_ENDPOINT");
        let token_a = value_of(a.env(), "YUZORA_ASKPASS_TOKEN");
        let op_a = value_of(a.env(), "YUZORA_ASKPASS_OPERATION");
        assert_eq!(
            run_client_impl(&endpoint, &token_a, &op_a, "Password for 'r': ").1,
            "pw-a"
        );

        let b = server.begin_operation(ctx_named("b", false, AskpassOperationKind::Fetch));
        bind_self(&b);
        let token_b = value_of(b.env(), "YUZORA_ASKPASS_TOKEN");
        let op_b = value_of(b.env(), "YUZORA_ASKPASS_OPERATION");
        assert_eq!(
            run_client_impl(&endpoint, &token_b, &op_b, "Password for 'r': ").1,
            "pw-a"
        );
        // B is a new operation: same prompt text is not authority, so it must
        // prompt again rather than inherit A's warm secret.
        assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 2);

        // Replaying A's token against B's operation id fails.
        assert_eq!(
            run_client_impl(&endpoint, &token_a, &op_b, "Password for 'r': ").0,
            1
        );
    }

    #[test]
    fn askpass_state_none_yields_empty_env_without_panic() {
        let degraded = AskpassState(None);
        let empty = degraded.begin_operation(sample_ctx(false, AskpassOperationKind::Push));
        assert!(empty.env().is_empty());
        let empty_bg = degraded.begin_operation(sample_ctx(true, AskpassOperationKind::Probe));
        assert!(empty_bg.env().is_empty());
        let server = start_with_reply(None);
        let live = AskpassState(Some(server));
        let live_op = live.begin_operation(sample_ctx(false, AskpassOperationKind::Fetch));
        assert!(!live_op.env().is_empty());
    }

    #[test]
    fn drop_removes_socket_file() {
        let server = AskpassServer::start(|_req| {}).unwrap();
        let endpoint = server.endpoint().to_string();
        assert!(std::path::Path::new(&endpoint).exists());
        drop(server);
        assert!(!std::path::Path::new(&endpoint).exists());
    }

    #[test]
    fn same_operation_same_kind_prompts_again() {
        let count = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let c2 = count.clone();
        let server_slot: Arc<Mutex<Option<Arc<AskpassServer>>>> = Default::default();
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
        let op = server.begin_operation(sample_ctx(false, AskpassOperationKind::Pull));
        bind_self(&op);
        let env = op.env();
        let endpoint = value_of(env, "YUZORA_ASKPASS_ENDPOINT");
        let token = value_of(env, "YUZORA_ASKPASS_TOKEN");
        let operation_id = value_of(env, "YUZORA_ASKPASS_OPERATION");
        assert_eq!(
            run_client_impl(&endpoint, &token, &operation_id, "Password for 'r': ").1,
            "pw"
        );
        assert_eq!(
            run_client_impl(&endpoint, &token, &operation_id, "Password for 'other': ").1,
            "pw"
        );
        assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[test]
    fn malicious_descendant_cannot_replay_secret_without_new_prompt() {
        let emits = Arc::new(Mutex::new(Vec::new()));
        let e2 = emits.clone();
        let server = AskpassServer::start(move |req| {
            e2.lock().unwrap().push(req);
        })
        .unwrap();
        let op = server.begin_operation(sample_ctx(false, AskpassOperationKind::Fetch));
        bind_self(&op);
        let env = op.env().to_vec();
        let endpoint = value_of(&env, "YUZORA_ASKPASS_ENDPOINT");
        let token = value_of(&env, "YUZORA_ASKPASS_TOKEN");
        let operation_id = value_of(&env, "YUZORA_ASKPASS_OPERATION");
        let first = {
            let endpoint = endpoint.clone();
            let token = token.clone();
            let operation_id = operation_id.clone();
            std::thread::spawn(move || {
                run_client_impl(&endpoint, &token, &operation_id, "Password for 'x': ")
            })
        };
        let first_req = wait_for_emits(&emits, 1);
        server.respond(first_req[0].id, Some("one-use-secret".into()));
        assert_eq!(first.join().unwrap(), (0, "one-use-secret".into()));

        let replay = {
            let endpoint = endpoint.clone();
            let token = token.clone();
            let operation_id = operation_id.clone();
            std::thread::spawn(move || {
                run_client_impl(&endpoint, &token, &operation_id, "Password for 'x': ")
            })
        };
        let second_req = wait_for_emits(&emits, 2);
        assert_ne!(second_req[1].id, first_req[0].id);
        // A hook descendant that inherited the operation token must not receive
        // the previous secret unless the UI answers again.
        server.respond(second_req[1].id, None);
        assert_eq!(replay.join().unwrap(), (1, String::new()));
    }

    #[test]
    fn expired_and_revoked_tokens_are_denied() {
        let server = start_with_reply(Some("secret"));
        let op = server.begin_operation_with_ttl(
            sample_ctx(false, AskpassOperationKind::Fetch),
            Duration::from_millis(1),
        );
        let env = op.env().to_vec();
        std::thread::sleep(Duration::from_millis(5));
        assert_eq!(
            run_client_impl(
                &value_of(&env, "YUZORA_ASKPASS_ENDPOINT"),
                &value_of(&env, "YUZORA_ASKPASS_TOKEN"),
                &value_of(&env, "YUZORA_ASKPASS_OPERATION"),
                "Password: ",
            )
            .0,
            1
        );

        let live = server.begin_operation(sample_ctx(false, AskpassOperationKind::Push));
        let live_env = live.env().to_vec();
        drop(live);
        assert_eq!(
            run_client_impl(
                &value_of(&live_env, "YUZORA_ASKPASS_ENDPOINT"),
                &value_of(&live_env, "YUZORA_ASKPASS_TOKEN"),
                &value_of(&live_env, "YUZORA_ASKPASS_OPERATION"),
                "Password: ",
            )
            .0,
            1
        );
        assert_eq!(server.live_operation_count(), 0);
    }

    #[test]
    fn child_exit_clears_cache_and_releases_waiter() {
        let emits = Arc::new(Mutex::new(Vec::new()));
        let e2 = emits.clone();
        let server = AskpassServer::start(move |req| {
            e2.lock().unwrap().push(req);
        })
        .unwrap();
        let op = server.begin_operation(sample_ctx(false, AskpassOperationKind::Fetch));
        bind_self(&op);
        let env = op.env().to_vec();
        let endpoint = value_of(&env, "YUZORA_ASKPASS_ENDPOINT");
        let token = value_of(&env, "YUZORA_ASKPASS_TOKEN");
        let operation_id = value_of(&env, "YUZORA_ASKPASS_OPERATION");
        let worker = std::thread::spawn(move || {
            run_client_impl(&endpoint, &token, &operation_id, "Password: ")
        });
        let emitted = wait_for_emits(&emits, 1);
        drop(op);
        assert_eq!(worker.join().unwrap(), (1, String::new()));
        server.respond(emitted[0].id, Some("too-late".into()));
        assert_eq!(
            run_client_impl(
                &value_of(&env, "YUZORA_ASKPASS_ENDPOINT"),
                &value_of(&env, "YUZORA_ASKPASS_TOKEN"),
                &value_of(&env, "YUZORA_ASKPASS_OPERATION"),
                "Password: ",
            )
            .0,
            1
        );
        assert_eq!(server.live_operation_count(), 0);
    }

    #[test]
    fn ui_response_reaches_only_matching_operation() {
        let emits = Arc::new(Mutex::new(Vec::new()));
        let e2 = emits.clone();
        let server = AskpassServer::start(move |req| {
            e2.lock().unwrap().push(req);
        })
        .unwrap();
        let a = server.begin_operation(ctx_named("alpha", false, AskpassOperationKind::Fetch));
        let b = server.begin_operation(ctx_named("beta", false, AskpassOperationKind::Push));
        bind_self(&a);
        bind_self(&b);
        let env_a = a.env().to_vec();
        let env_b = b.env().to_vec();
        let worker_a = std::thread::spawn({
            let endpoint = value_of(&env_a, "YUZORA_ASKPASS_ENDPOINT");
            let token = value_of(&env_a, "YUZORA_ASKPASS_TOKEN");
            let operation_id = value_of(&env_a, "YUZORA_ASKPASS_OPERATION");
            move || run_client_impl(&endpoint, &token, &operation_id, "Password: ")
        });
        let worker_b = std::thread::spawn({
            let endpoint = value_of(&env_b, "YUZORA_ASKPASS_ENDPOINT");
            let token = value_of(&env_b, "YUZORA_ASKPASS_TOKEN");
            let operation_id = value_of(&env_b, "YUZORA_ASKPASS_OPERATION");
            move || run_client_impl(&endpoint, &token, &operation_id, "Password: ")
        });
        let emitted = wait_for_emits(&emits, 2);
        let req_a = emitted
            .iter()
            .find(|req| req.repository_display == "alpha")
            .unwrap();
        let req_b = emitted
            .iter()
            .find(|req| req.repository_display == "beta")
            .unwrap();
        assert_eq!(req_a.operation, "fetch");
        assert_eq!(req_b.operation, "push");
        assert!(!req_a.background);
        server.respond(req_a.id, Some("secret-a".into()));
        server.respond(req_b.id, Some("secret-b".into()));
        assert_eq!(worker_a.join().unwrap(), (0, "secret-a".into()));
        assert_eq!(worker_b.join().unwrap(), (0, "secret-b".into()));
    }

    #[test]
    fn client_background_field_cannot_override_operation_policy() {
        let emitted = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let e2 = emitted.clone();
        let server = AskpassServer::start(move |_| {
            e2.store(true, std::sync::atomic::Ordering::SeqCst);
        })
        .unwrap();
        let op = server.begin_operation(sample_ctx(true, AskpassOperationKind::Probe));
        bind_self(&op);
        let request = serde_json::json!({
            "token": value_of(op.env(), "YUZORA_ASKPASS_TOKEN"),
            "operationId": value_of(op.env(), "YUZORA_ASKPASS_OPERATION"),
            "background": false,
            "prompt": "Password: ",
        })
        .to_string();
        assert_eq!(
            super::unix_impl::run_client_raw(
                &value_of(op.env(), "YUZORA_ASKPASS_ENDPOINT"),
                &request
            )
            .0,
            1
        );
        assert!(!emitted.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn oversized_request_line_is_denied() {
        let emitted = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let e2 = emitted.clone();
        let server = AskpassServer::start(move |_| {
            e2.store(true, std::sync::atomic::Ordering::SeqCst);
        })
        .unwrap();
        let op = server.begin_operation(sample_ctx(false, AskpassOperationKind::Fetch));
        let endpoint = value_of(op.env(), "YUZORA_ASKPASS_ENDPOINT");
        let mut stream = UnixStream::connect(&endpoint).unwrap();
        let huge = "x".repeat(16 * 1024 + 32);
        let _ = stream.write_all(huge.as_bytes());
        let _ = stream.write_all(b"\n");
        let _ = stream.flush();
        let mut response = String::new();
        let _ = std::io::Read::read_to_string(&mut stream, &mut response);
        assert!(response.trim_end_matches('\n').is_empty());
        assert!(!emitted.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn unbound_operation_denies_and_does_not_prompt() {
        let emitted = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let e2 = emitted.clone();
        let server = AskpassServer::start(move |_| {
            e2.store(true, std::sync::atomic::Ordering::SeqCst);
        })
        .unwrap();
        let op = server.begin_operation(sample_ctx(false, AskpassOperationKind::Fetch));
        let result = run_client_impl(
            &value_of(op.env(), "YUZORA_ASKPASS_ENDPOINT"),
            &value_of(op.env(), "YUZORA_ASKPASS_TOKEN"),
            &value_of(op.env(), "YUZORA_ASKPASS_OPERATION"),
            "Password: ",
        );
        assert_eq!(result, (1, String::new()));
        assert!(!emitted.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn bound_pid_rejects_unrelated_peer() {
        let server = start_with_reply(Some("should-not-leak"));
        let op = server.begin_operation(sample_ctx(false, AskpassOperationKind::Fetch));
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        op.bind_root_pid(child.id());
        let env = op.env();
        let result = run_client_impl(
            &value_of(env, "YUZORA_ASKPASS_ENDPOINT"),
            &value_of(env, "YUZORA_ASKPASS_TOKEN"),
            &value_of(env, "YUZORA_ASKPASS_OPERATION"),
            "Password: ",
        );
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(result.0, 1);
    }
}

#[cfg(test)]
mod interface_tests {
    use super::*;

    #[test]
    fn public_askpass_types_are_available_on_this_target() {
        let _ = std::any::type_name::<AskpassServer>();
        let _ = std::any::type_name::<AskpassState>();
        let _ = std::any::type_name::<AskpassOperationGuard>();
        let _run: fn(&str, &str) -> i32 = run_client;
        let _ = AskpassOperationKind::Fetch.as_str();
    }

    #[cfg(not(unix))]
    #[test]
    fn non_unix_askpass_is_fail_closed_without_tokens() {
        assert!(AskpassServer::start(|_| {}).is_err());
        assert_eq!(run_client("endpoint", "Password:"), 1);
        let state = AskpassState(None);
        let op = state.begin_operation(AskpassOperationContext {
            repository_display: "repo".into(),
            repository_canonical: "C:\\repo".into(),
            remote_display: None,
            remote_fingerprint: None,
            operation: AskpassOperationKind::Fetch,
            background: false,
        });
        assert!(op.env().is_empty());
        let live = AskpassState(AskpassServer::start(|_| {}).ok());
        assert!(live
            .begin_operation(AskpassOperationContext {
                repository_display: "repo".into(),
                repository_canonical: "C:\\repo".into(),
                remote_display: None,
                remote_fingerprint: None,
                operation: AskpassOperationKind::Push,
                background: false,
            })
            .env()
            .is_empty());
    }
}
