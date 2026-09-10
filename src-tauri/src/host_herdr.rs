//! SSH metadata is evaluated on the host; HERDR API uses direct-streamlocal.
use crate::host_service::{HostConnection, HostStream, HostTarget};
use crate::ssh_service::SshManager;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
use tokio::io::{AsyncWriteExt, BufReader};
use yuzora_host::herdr_backend::{HerdrMetadata, HerdrRemoteBackend};
use yuzora_host::herdr_limits::MAX_NDJSON_LINE_BYTES;
use yuzora_host::herdr_service::HerdrManager;
use yuzora_host::protocol::Operation;

pub(crate) struct SshHerdrBackend {
    connection: Weak<HostConnection>,
    ssh: Arc<SshManager>,
    binary: String,
    executor: tokio::runtime::Handle,
    sockets: Mutex<HashSet<String>>,
}

impl SshHerdrBackend {
    pub(crate) fn new(
        connection: &Arc<HostConnection>,
        ssh: Arc<SshManager>,
        binary: &str,
    ) -> Self {
        Self {
            connection: Arc::downgrade(connection),
            ssh,
            binary: binary.into(),
            executor: tokio::runtime::Handle::current(),
            sockets: Mutex::default(),
        }
    }
    fn connection(&self) -> Result<Arc<HostConnection>, String> {
        let connection = self.connection.upgrade().ok_or("host-disconnected")?;
        if *connection.cancelled.borrow() {
            return Err("host-disconnected".into());
        }
        Ok(connection)
    }
}

impl HerdrRemoteBackend for SshHerdrBackend {
    fn metadata(&self, query: HerdrMetadata, session: Option<&str>) -> Result<Value, String> {
        let connection = self.connection()?;
        let value = self
            .executor
            .block_on(connection.request(Operation::HerdrMetadata {
                binary: self.binary.clone(),
                query,
                session: session.map(str::to_owned),
            }))?;
        if matches!(query, HerdrMetadata::Sessions) {
            let sessions = yuzora_host::herdr_service::parse_session_list_json(&value)?;
            *self.sockets.lock().unwrap() = sessions
                .into_iter()
                .filter(|s| {
                    s.running && s.socket_path.starts_with('/') && !s.socket_path.contains('\0')
                })
                .map(|s| s.socket_path)
                .collect();
        }
        Ok(value)
    }
    fn request(&self, socket: &str, method: &str, params: Value) -> Result<Value, String> {
        let connection = self.connection()?;
        if !self.sockets.lock().unwrap().contains(socket) {
            return Err("undiscovered-herdr-socket".into());
        }
        let HostTarget::Ssh { session_id } = &connection.target else {
            return Err("ssh-runtime-required".into());
        };
        let mut cancelled = connection.cancelled.subscribe();
        self.executor.block_on(async {
            tokio::select! {
                biased;
                _ = cancelled.changed() => Err("host-disconnected".into()),
                result = tokio::time::timeout(Duration::from_secs(15), async {
                    let io = self.ssh.open_host_socket(session_id, socket).await?;
                    socket_transaction(io, method, params).await
                }) => result.map_err(|_| "herdr-socket-timeout".to_owned())?,
            }
        })
    }
}

/// The channel is never returned to a pool. A cancelled or ambiguous write is
/// discarded, so subsequent requests cannot consume its reply or replay it.
async fn socket_transaction(
    mut io: HostStream,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    let id = format!("yuzora:{}", uuid::Uuid::new_v4());
    let request = serde_json::json!({"id":id,"method":method,"params":params});
    let mut frame = serde_json::to_vec(&request).map_err(|e| e.to_string())?;
    if frame.len() > MAX_NDJSON_LINE_BYTES {
        return Err("herdr-request-too-large".into());
    }
    frame.push(b'\n');
    io.write_all(&frame).await.map_err(|e| e.to_string())?;
    io.flush().await.map_err(|e| e.to_string())?;
    let frame =
        yuzora_host::wire::read_frame_limit(&mut BufReader::new(io), MAX_NDJSON_LINE_BYTES + 1)
            .await?
            .ok_or("herdr-socket-closed")?;
    let response: Value = serde_json::from_slice(&frame).map_err(|e| e.to_string())?;
    if response.get("id") != Some(&Value::String(id)) {
        return Err("herdr-response-id-mismatch".into());
    }
    yuzora_host::herdr_service::validate_api_response(response)
}

impl HostConnection {
    pub(crate) fn ssh_runtime(
        self: &Arc<Self>,
        binary: &str,
        ssh: Arc<SshManager>,
    ) -> Result<Arc<HerdrManager>, String> {
        let mut runtimes = self.runtimes.lock().unwrap();
        if let Some(runtime) = runtimes.get(binary) {
            return Ok(runtime.clone());
        }
        if runtimes.len() >= 4 {
            return Err("too-many-runtime-binaries".into());
        }
        let backend = Arc::new(SshHerdrBackend::new(self, ssh, binary));
        let runtime = Arc::new(HerdrManager::with_remote(binary.into(), backend));
        runtimes.insert(binary.into(), runtime.clone());
        Ok(runtime)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncReadExt};
    #[tokio::test]
    async fn socket_write_uses_one_channel_and_preserves_remote_error() {
        let (client, server) = tokio::io::duplex(4096);
        let peer = tokio::spawn(async move {
            let mut reader = BufReader::new(server);
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
            let request: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(request["method"], "tab.close");
            assert_eq!(request["params"]["tab_id"], "same-name");
            let response = serde_json::json!({"id":request["id"],"error":{"code":"conflict","message":"changed"}});
            let frame = format!("{response}\n");
            // Deliberately fragment a response across separate writes.
            for chunk in frame.as_bytes().chunks(3) {
                reader.get_mut().write_all(chunk).await.unwrap();
            }
            let mut remainder = Vec::new();
            reader.read_to_end(&mut remainder).await.unwrap();
            assert!(remainder.is_empty(), "failed writes must never be replayed");
        });
        let error = socket_transaction(
            Box::new(client),
            "tab.close",
            serde_json::json!({"tab_id":"same-name"}),
        )
        .await
        .unwrap_err();
        assert_eq!(error, "conflict: changed");
        peer.await.unwrap();
    }

    #[tokio::test]
    async fn socket_rejects_wrong_response_id() {
        let (client, mut server) = tokio::io::duplex(4096);
        let peer = tokio::spawn(async move {
            let mut reader = BufReader::new(&mut server);
            let mut request = String::new();
            reader.read_line(&mut request).await.unwrap();
            server
                .write_all(b"{\"id\":\"old-generation\",\"result\":{}}\n")
                .await
                .unwrap();
        });
        assert_eq!(
            socket_transaction(Box::new(client), "ping", serde_json::json!({}))
                .await
                .unwrap_err(),
            "herdr-response-id-mismatch"
        );
        peer.await.unwrap();
    }

    #[tokio::test]
    async fn cancelled_socket_request_closes_channel_without_replay() {
        let (client, server) = tokio::io::duplex(4096);
        let (ready, started) = oneshot::channel();
        let peer = tokio::spawn(async move {
            let mut reader = BufReader::new(server);
            let mut request = String::new();
            reader.read_line(&mut request).await.unwrap();
            ready.send(()).unwrap();
            let mut rest = Vec::new();
            assert_eq!(reader.read_to_end(&mut rest).await.unwrap(), 0);
        });
        let request = tokio::spawn(socket_transaction(
            Box::new(client),
            "tab.close",
            serde_json::json!({}),
        ));
        started.await.unwrap();
        request.abort();
        assert!(request.await.unwrap_err().is_cancelled());
        tokio::time::timeout(Duration::from_secs(1), peer)
            .await
            .unwrap()
            .unwrap();
    }
    use tokio::sync::oneshot;
}
