//! App-owned connections to Unix hosts. A connection never owns HERDR servers.
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, BufReader, ReadBuf};
use yuzora_host::protocol::{
    ConnectionOwner, Hello, Operation, Outcome, Request, Response, MAX_FRAME_BYTES,
    PROTOCOL_VERSION,
};

pub trait HostIo: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> HostIo for T {}
pub type HostStream = Box<dyn HostIo>;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum HostTarget {
    Local,
    Wsl {
        distro: String,
    },
    Ssh {
        #[serde(rename = "sessionId")]
        session_id: String,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectedHost {
    pub owner: ConnectionOwner,
    pub hello: Hello,
}

struct ProcessStream {
    child: Option<tokio::process::Child>,
    stdin: Option<tokio::process::ChildStdin>,
    stdout: tokio::process::ChildStdout,
}
impl AsyncRead for ProcessStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stdout).poll_read(cx, buffer)
    }
}
impl AsyncWrite for ProcessStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match self.stdin.as_mut() {
            Some(stdin) => Pin::new(stdin).poll_write(cx, bytes),
            None => Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "helper stdin closed",
            ))),
        }
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.stdin.as_mut() {
            Some(stdin) => Pin::new(stdin).poll_flush(cx),
            None => Poll::Ready(Ok(())),
        }
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        if let Some(stdin) = self.stdin.as_mut() {
            match Pin::new(stdin).poll_shutdown(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                Poll::Ready(Ok(())) => {}
            }
        }
        self.stdin.take();
        Poll::Ready(Ok(()))
    }
}
impl Drop for ProcessStream {
    fn drop(&mut self) {
        self.stdin.take(); // EOF lets the helper release its own connectors.
        if let Some(mut child) = self.child.take() {
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    if tokio::time::timeout(Duration::from_secs(3), child.wait())
                        .await
                        .is_err()
                    {
                        let _ = child.kill().await;
                    }
                });
            } else {
                let _ = child.start_kill();
            }
        }
    }
}

pub(crate) fn shell_quote(value: &str) -> Result<String, String> {
    if value.is_empty() || value.contains('\0') {
        return Err("invalid-remote-argument".into());
    }
    Ok(format!("'{}'", value.replace('\'', "'\\''")))
}

#[derive(Clone, Copy)]
pub(crate) enum HostLane {
    Control,
    Stream,
    Tcp,
    Database,
}

impl HostLane {
    pub(crate) fn argument(self) -> &'static str {
        match self {
            Self::Control => "--stdio",
            Self::Stream => "--stream",
            Self::Tcp => "--tcp",
            Self::Database => "--database",
        }
    }
}

pub(crate) async fn open_stream(
    target: &HostTarget,
    helper: &str,
    mode: HostLane,
    ssh: &crate::ssh_service::SshManager,
) -> Result<HostStream, String> {
    if helper.is_empty() || helper.contains('\0') {
        return Err("invalid-helper-path".into());
    }
    let mut command = match target {
        HostTarget::Ssh { session_id } => {
            return ssh.open_host_helper(session_id, helper, mode).await
        }
        HostTarget::Wsl { distro } => {
            if !cfg!(windows) {
                return Err("wsl-requires-windows".into());
            }
            if distro.is_empty() || distro.contains('\0') {
                return Err("invalid-wsl-distro".into());
            }
            let mut command = tokio::process::Command::new("wsl.exe");
            command.args(["--distribution", distro, "--exec", helper]);
            command
        }
        HostTarget::Local => {
            if cfg!(windows) {
                return Err("select-wsl-runtime".into());
            }
            tokio::process::Command::new(helper)
        }
    };
    command.arg(mode.argument());
    spawn_process_stream(&mut command)
}

pub(crate) fn spawn_process_stream(
    command: &mut tokio::process::Command,
) -> Result<HostStream, String> {
    command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let mut child = command
        .spawn()
        .map_err(|e| format!("host-helper-unavailable: {e}"))?;
    let stdin = child.stdin.take().ok_or("helper-stdin-missing")?;
    let stdout = child.stdout.take().ok_or("helper-stdout-missing")?;
    Ok(Box::new(ProcessStream {
        child: Some(child),
        stdin: Some(stdin),
        stdout,
    }))
}

pub(crate) struct HostConnection {
    executor: tokio::runtime::Handle,
    pub(crate) owner: ConnectionOwner,
    pub(crate) target: HostTarget,
    pub(crate) helper: String,
    pub(crate) cancelled: tokio::sync::watch::Sender<bool>,
    pub(crate) streams: Arc<Mutex<HashMap<String, Arc<crate::host_streams::HostStreamSession>>>>,
    pub(crate) runtimes: Mutex<HashMap<String, Arc<yuzora_host::herdr_service::HerdrManager>>>,
    pub(crate) git_channels: crate::host_git::GitChannels,
    pub(crate) git_jobs: tokio::sync::Semaphore,
    pub(crate) tunnels: crate::host_tunnels::Tunnels,
    pub(crate) tunnel_clients: Arc<tokio::sync::Semaphore>,
    pub(crate) database_slots: Arc<tokio::sync::Semaphore>,
    requests: tokio::sync::Semaphore,
    request_queue: tokio::sync::Semaphore,
    calls: tokio::sync::Semaphore,
    call_queue: tokio::sync::Semaphore,
    pub(crate) stream_openings: tokio::sync::Semaphore,
    pub(crate) io: tokio::sync::Mutex<Option<BufReader<HostStream>>>,
}

impl HostConnection {
    pub(crate) fn new(
        owner: ConnectionOwner,
        target: HostTarget,
        helper: String,
        stream: HostStream,
    ) -> Self {
        Self {
            executor: tokio::runtime::Handle::current(),
            owner,
            target,
            helper,
            cancelled: tokio::sync::watch::channel(false).0,
            streams: Arc::default(),
            runtimes: Mutex::default(),
            requests: tokio::sync::Semaphore::new(16),
            request_queue: tokio::sync::Semaphore::new(32),
            calls: tokio::sync::Semaphore::new(8),
            call_queue: tokio::sync::Semaphore::new(32),
            stream_openings: tokio::sync::Semaphore::new(8),
            git_channels: Default::default(),
            git_jobs: tokio::sync::Semaphore::new(4),
            tunnels: Default::default(),
            tunnel_clients: crate::host_tunnels::client_limit(),
            database_slots: Arc::new(tokio::sync::Semaphore::new(8)),
            io: tokio::sync::Mutex::new(Some(BufReader::new(stream))),
        }
    }

    async fn acquire_call(
        &self,
    ) -> Result<
        (
            tokio::sync::SemaphorePermit<'_>,
            tokio::sync::SemaphorePermit<'_>,
        ),
        String,
    > {
        let queued = self
            .call_queue
            .try_acquire()
            .map_err(|_| "host-call-limit")?;
        let permit = tokio::time::timeout(Duration::from_secs(15), self.calls.acquire())
            .await
            .map_err(|_| "host-call-wait-timeout")?
            .map_err(|_| "host-disconnected")?;
        Ok((queued, permit))
    }

    pub(crate) async fn request(&self, operation: Operation) -> Result<serde_json::Value, String> {
        self.request_with_timeout(operation, Duration::from_secs(30))
            .await
    }
    async fn acquire_request(
        &self,
    ) -> Result<
        (
            tokio::sync::SemaphorePermit<'_>,
            tokio::sync::SemaphorePermit<'_>,
        ),
        String,
    > {
        let queued = self
            .request_queue
            .try_acquire()
            .map_err(|_| "host-request-limit")?;
        let mut cancelled = self.cancelled.subscribe();
        if *cancelled.borrow() {
            return Err("host-disconnected".into());
        }
        let permit = tokio::select! {
            _ = cancelled.changed() => return Err("host-disconnected".into()),
            result = tokio::time::timeout(Duration::from_secs(15), self.requests.acquire()) => {
                result.map_err(|_| "host-request-wait-timeout")?
                    .map_err(|_| "host-disconnected")?
            }
        };
        Ok((queued, permit))
    }
    pub(crate) async fn request_with_timeout(
        &self,
        operation: Operation,
        timeout: Duration,
    ) -> Result<serde_json::Value, String> {
        let _permits = self.acquire_request().await?;
        static NEXT_REQUEST: AtomicU64 = AtomicU64::new(1);
        let id = NEXT_REQUEST.fetch_add(1, Ordering::Relaxed).to_string();
        let request = Request {
            version: PROTOCOL_VERSION,
            id: id.clone(),
            owner: self.owner.clone(),
            operation,
        };
        let mut bytes = serde_json::to_vec(&request).map_err(|e| e.to_string())?;
        if bytes.len() >= MAX_FRAME_BYTES {
            return Err("request-too-large".into());
        }
        bytes.push(b'\n');
        let mut slot = self.io.lock().await;
        // Take the stream out while in flight. Cancellation drops it rather than
        // leaving a partially consumed response for a subsequent request.
        let mut io = slot.take().ok_or("host-disconnected")?;
        let mut cancelled = self.cancelled.subscribe();
        if *cancelled.borrow() {
            return Err("host-disconnected".into());
        }
        let transaction = tokio::time::timeout(timeout, async {
            io.get_mut()
                .write_all(&bytes)
                .await
                .map_err(|e| e.to_string())?;
            io.get_mut().flush().await.map_err(|e| e.to_string())?;
            let frame = yuzora_host::wire::read_frame(&mut io)
                .await?
                .ok_or("host-disconnected")?;
            let response: Response = serde_json::from_slice(&frame).map_err(|e| e.to_string())?;
            if response.version != PROTOCOL_VERSION
                || response.id != id
                || response.owner != self.owner
            {
                return Err("host-response-identity-mismatch".into());
            }
            Ok(response.outcome)
        });
        let result: Result<Outcome, String> = tokio::select! {
            _ = cancelled.changed() => return Err("host-disconnected".into()),
            result = transaction => result.map_err(|_|"host-request-timeout".to_owned())?,
        };
        let outcome = result?;
        *slot = Some(io);
        match outcome {
            Outcome::Ok { value } => Ok(value),
            Outcome::Error { message, .. } => Err(message),
        }
    }
}

impl Drop for HostConnection {
    fn drop(&mut self) {
        // russh ChannelStream schedules its Close in Drop. App exit can drop
        // the final connection on the native UI thread, outside Tokio.
        let _context = self.executor.enter();
        self.io.get_mut().take();
    }
}

#[derive(Default)]
pub struct HostManager {
    connections: Mutex<HashMap<String, Arc<HostConnection>>>,
    next_generation: AtomicU64,
    opening: AtomicUsize,
    shutting_down: AtomicBool,
}

struct Opening<'a>(&'a AtomicUsize);
impl Drop for Opening<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

impl HostManager {
    pub async fn connect(
        &self,
        host_id: String,
        target: HostTarget,
        helper: String,
        ssh: &crate::ssh_service::SshManager,
    ) -> Result<ConnectedHost, String> {
        if self.shutting_down.load(Ordering::Acquire) {
            return Err("host-manager-shutting-down".into());
        }
        let opening = self.opening.fetch_add(1, Ordering::Relaxed);
        let _opening = Opening(&self.opening);
        if opening >= 32 {
            return Err("too-many-host-openings".into());
        }
        if let HostTarget::Wsl { distro } = &target {
            crate::host_wsl::verify_identity(&host_id, distro).await?;
        }
        if host_id.is_empty() || host_id.len() > 256 || host_id.contains('\0') {
            return Err("invalid-host-id".into());
        }
        if self.connections.lock().unwrap().contains_key(&host_id) {
            return Err("host-already-connected".into());
        }
        let stream = open_stream(&target, &helper, HostLane::Control, ssh).await?;
        let owner = ConnectionOwner {
            host_id: host_id.clone(),
            generation: self.next_generation.fetch_add(1, Ordering::Relaxed) + 1,
        };
        let connection = Arc::new(HostConnection::new(owner.clone(), target, helper, stream));
        let hello: Hello = serde_json::from_value(connection.request(Operation::Hello).await?)
            .map_err(|e| e.to_string())?;
        if hello.protocol != PROTOCOL_VERSION || !matches!(hello.os.as_str(), "linux" | "macos") {
            return Err("unsupported-host-platform-or-protocol".into());
        }
        let mut connections = self.connections.lock().unwrap();
        if self.shutting_down.load(Ordering::Acquire) {
            return Err("host-manager-shutting-down".into());
        }
        if connections.contains_key(&host_id) {
            return Err("host-already-connected".into());
        }
        if connections.len() >= 32 {
            return Err("too-many-connected-hosts".into());
        }
        connections.insert(host_id, connection);
        Ok(ConnectedHost { owner, hello })
    }

    pub async fn request(
        &self,
        owner: ConnectionOwner,
        operation: Operation,
    ) -> Result<serde_json::Value, String> {
        let connection = self.connection(&owner)?;
        let result = connection.request(operation).await;
        self.connection(&owner)?;
        result
    }

    pub(crate) fn connection_for_host(&self, host_id: &str) -> Result<Arc<HostConnection>, String> {
        let connection = self
            .connections
            .lock()
            .unwrap()
            .get(host_id)
            .cloned()
            .ok_or("host-disconnected")?;
        if *connection.cancelled.borrow() {
            return Err("host-disconnected".into());
        }
        Ok(connection)
    }

    pub(crate) fn connection(
        &self,
        owner: &ConnectionOwner,
    ) -> Result<Arc<HostConnection>, String> {
        let connection = self
            .connections
            .lock()
            .unwrap()
            .get(&owner.host_id)
            .cloned()
            .ok_or("host-disconnected")?;
        if &connection.owner != owner {
            return Err("stale-host-generation".into());
        }
        if *connection.cancelled.borrow() {
            return Err("host-disconnected".into());
        }
        Ok(connection)
    }

    pub async fn disconnect(&self, owner: &ConnectionOwner) -> Result<(), String> {
        let connection = {
            let mut connections = self.connections.lock().unwrap();
            if let Some(current) = connections.get(&owner.host_id) {
                if current.owner != *owner {
                    return Err("stale-host-generation".into());
                }
            }
            connections.remove(&owner.host_id)
        };
        if let Some(connection) = connection {
            connection.cancelled.send_replace(true);
            connection.streams.lock().unwrap().clear();
            connection.tunnels.lock().unwrap().clear();
            connection.close_all_git();
            connection.io.lock().await.take();
        }
        Ok(())
    }

    pub fn disconnect_all(&self) {
        self.shutting_down.store(true, Ordering::Release);
        for (_, connection) in self.connections.lock().unwrap().drain() {
            connection.cancelled.send_replace(true);
            connection.streams.lock().unwrap().clear();
            connection.tunnels.lock().unwrap().clear();
            connection.close_all_git();
        }
    }
}

#[derive(Default)]
pub struct HostState(pub Arc<HostManager>);

#[tauri::command]
pub async fn host_connect(
    state: tauri::State<'_, HostState>,
    ssh: tauri::State<'_, crate::ssh_service::SshState>,
    host_id: String,
    target: HostTarget,
    helper: String,
) -> Result<ConnectedHost, String> {
    state.0.connect(host_id, target, helper, &ssh.0).await
}
#[tauri::command]
pub async fn host_request(
    state: tauri::State<'_, HostState>,
    ssh: tauri::State<'_, crate::ssh_service::SshState>,
    owner: ConnectionOwner,
    operation: Operation,
) -> Result<serde_json::Value, String> {
    if let Operation::Git {
        workspace,
        repository_root,
        call,
    } = operation
    {
        let connection = state.0.connection(&owner)?;
        let result = connection
            .git_request(&ssh.0, workspace, repository_root, call)
            .await;
        state.0.connection(&owner)?;
        return result;
    }
    if let Operation::WorkspaceClose { workspace } = &operation {
        state.0.connection(&owner)?.close_git(workspace);
    }
    if let Operation::HerdrCall { binary, call } = operation {
        let connection = state.0.connection(&owner)?;
        if matches!(connection.target, HostTarget::Ssh { .. }) {
            let _permits = connection.acquire_call().await?;
            state.0.connection(&owner)?;
            let runtime = connection.ssh_runtime(&binary, ssh.0.clone())?;
            let result = tokio::task::spawn_blocking(move || call.execute(&runtime))
                .await
                .map_err(|e| e.to_string())?;
            state.0.connection(&owner)?;
            return result;
        }
        return connection
            .request(Operation::HerdrCall { binary, call })
            .await;
    }
    state.0.request(owner, operation).await
}
#[tauri::command]
pub async fn host_disconnect(
    state: tauri::State<'_, HostState>,
    owner: ConnectionOwner,
) -> Result<(), String> {
    state.0.disconnect(&owner).await
}

#[cfg(test)]
mod tests {
    use super::*;
    struct RuntimeDropStream(tokio::io::DuplexStream);
    impl Drop for RuntimeDropStream {
        fn drop(&mut self) {
            assert!(tokio::runtime::Handle::try_current().is_ok());
        }
    }
    impl AsyncRead for RuntimeDropStream {
        fn poll_read(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            Pin::new(&mut self.0).poll_read(cx, buf)
        }
    }
    impl AsyncWrite for RuntimeDropStream {
        fn poll_write(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            Pin::new(&mut self.0).poll_write(cx, buf)
        }
        fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Pin::new(&mut self.0).poll_flush(cx)
        }
        fn poll_shutdown(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<std::io::Result<()>> {
            Pin::new(&mut self.0).poll_shutdown(cx)
        }
    }

    #[tokio::test]
    async fn connection_can_drop_its_ssh_stream_on_the_native_ui_thread() {
        let (io, _peer) = tokio::io::duplex(1024);
        let connection = HostConnection::new(
            ConnectionOwner {
                host_id: "fixture".into(),
                generation: 1,
            },
            HostTarget::Local,
            "/helper".into(),
            Box::new(RuntimeDropStream(io)),
        );
        std::thread::spawn(move || drop(connection)).join().unwrap();
    }

    #[tokio::test]
    async fn herdr_calls_wait_within_a_bounded_queue() {
        let (io, _peer) = tokio::io::duplex(1024);
        let connection = HostConnection::new(
            ConnectionOwner {
                host_id: "fixture".into(),
                generation: 1,
            },
            HostTarget::Local,
            "/helper".into(),
            Box::new(io),
        );
        let occupied = connection.calls.acquire_many(8).await.unwrap();
        let next = connection.acquire_call();
        tokio::pin!(next);
        assert!(tokio::time::timeout(Duration::from_millis(10), &mut next)
            .await
            .is_err());
        assert_eq!(connection.call_queue.available_permits(), 31);
        drop(occupied);
        let permits = next.await.unwrap();
        assert_eq!(connection.calls.available_permits(), 7);
        drop(permits);
        let queue = connection.call_queue.acquire_many(32).await.unwrap();
        assert!(
            matches!(connection.acquire_call().await, Err(error) if error == "host-call-limit")
        );
        drop(queue);
        assert_eq!(connection.call_queue.available_permits(), 32);
    }

    #[test]
    fn quotes_remote_paths_without_shell_expansion() {
        assert_eq!(
            shell_quote("/home/a b/it's/$(touch x)").unwrap(),
            "'/home/a b/it'\\''s/$(touch x)'"
        );
        assert!(shell_quote("bad\0path").is_err());
    }

    #[tokio::test]
    async fn control_requests_wait_within_a_bounded_cancellable_queue() {
        let (io, _peer) = tokio::io::duplex(1024);
        let connection = HostConnection::new(
            ConnectionOwner {
                host_id: "fixture".into(),
                generation: 1,
            },
            HostTarget::Local,
            "/helper".into(),
            Box::new(io),
        );
        let occupied = connection.requests.acquire_many(16).await.unwrap();
        let next = connection.acquire_request();
        tokio::pin!(next);
        assert!(tokio::time::timeout(Duration::from_millis(10), &mut next)
            .await
            .is_err());
        assert_eq!(connection.request_queue.available_permits(), 31);
        drop(occupied);
        drop(next.await.unwrap());
        let queue = connection.request_queue.acquire_many(32).await.unwrap();
        assert!(
            matches!(connection.acquire_request().await, Err(error) if error == "host-request-limit")
        );
        drop(queue);
        let _occupied = connection.requests.acquire_many(16).await.unwrap();
        let next = connection.acquire_request();
        tokio::pin!(next);
        assert!(tokio::time::timeout(Duration::from_millis(10), &mut next)
            .await
            .is_err());
        connection.cancelled.send_replace(true);
        assert!(matches!(next.await, Err(error) if error == "host-disconnected"));
        assert_eq!(connection.request_queue.available_permits(), 32);
    }
}
