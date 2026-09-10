use crate::files::WorkspaceFiles;
use crate::herdr_service::HerdrManager;
use crate::protocol::*;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::process::Stdio;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};

pub use crate::wire::read_frame;

async fn command_json(binary: &str, args: &[&str]) -> Result<Value, String> {
    let mut child = tokio::process::Command::new(binary)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("herdr-unavailable: {e}"))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or("missing-stdout")?
        .take((MAX_FRAME_BYTES + 1) as u64);
    let result = tokio::time::timeout(Duration::from_secs(15), async {
        let mut bytes = Vec::new();
        stdout
            .read_to_end(&mut bytes)
            .await
            .map_err(|e| e.to_string())?;
        if bytes.len() > MAX_FRAME_BYTES {
            return Err("frame-too-large".into());
        }
        let status = child.wait().await.map_err(|e| e.to_string())?;
        if !status.success() {
            return Err(format!("herdr-command-failed: {status}"));
        }
        serde_json::from_slice(&bytes).map_err(|e| format!("herdr-invalid-json: {e}"))
    })
    .await;
    match result {
        Ok(Ok(value)) => Ok(value),
        other => {
            let _ = child.kill().await;
            match other {
                Ok(Err(error)) => Err(error),
                _ => Err("herdr-timeout".into()),
            }
        }
    }
}

pub async fn socket_request(socket: &str, request: Value) -> Result<Value, String> {
    tokio::time::timeout(Duration::from_secs(15), async {
        let mut stream = tokio::net::UnixStream::connect(socket)
            .await
            .map_err(|e| e.to_string())?;
        let mut bytes = serde_json::to_vec(&request).map_err(|e| e.to_string())?;
        if bytes.len() >= MAX_FRAME_BYTES {
            return Err("frame-too-large".into());
        }
        bytes.push(b'\n');
        stream.write_all(&bytes).await.map_err(|e| e.to_string())?;
        let mut reader = BufReader::new(stream);
        let frame = read_frame(&mut reader)
            .await?
            .ok_or("herdr-socket-closed")?;
        serde_json::from_slice(&frame).map_err(|e| e.to_string())
    })
    .await
    .map_err(|_| "herdr-timeout".to_string())?
}

#[derive(Default)]
pub struct HostServer {
    owner: Option<ConnectionOwner>,
    files: WorkspaceFiles,
    sockets: HashSet<String>,
    herdr: HashMap<String, Arc<HerdrManager>>,
    trust: Option<crate::workspace_trust::WorkspaceTrustState>,
    git: Arc<Mutex<crate::git_command::HostGit>>,
    cancelled: Arc<AtomicBool>,
}

impl Drop for HostServer {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

impl HostServer {
    fn runtime(&mut self, binary: &str) -> Result<Arc<HerdrManager>, String> {
        if let Some(runtime) = self.herdr.get(binary) {
            return Ok(runtime.clone());
        }
        if self.herdr.len() >= 4 {
            return Err("too-many-runtime-binaries".into());
        }
        let manager = Arc::new(if binary == "herdr" {
            HerdrManager::new()
        } else {
            HerdrManager::with_binary(binary.into())
        });
        if manager.resolve_binary().is_none() {
            return Err("herdr-unavailable".into());
        }
        self.herdr.insert(binary.into(), manager.clone());
        Ok(manager)
    }
    pub async fn handle(&mut self, request: Request) -> Response {
        let result = if request.version != PROTOCOL_VERSION {
            Err("protocol-mismatch".to_owned())
        } else if self
            .owner
            .as_ref()
            .is_some_and(|owner| owner != &request.owner)
        {
            Err("connection-owner-mismatch".to_owned())
        } else if self.owner.is_none() && !matches!(&request.operation, Operation::Hello) {
            Err("handshake-required".to_owned())
        } else {
            if self.owner.is_none() {
                self.trust = crate::trust_command::host_trust(&request.owner.host_id).ok();
            }
            self.owner.get_or_insert_with(|| request.owner.clone());
            self.dispatch(request.operation).await
        };
        let outcome = match result {
            Ok(value) => Outcome::Ok { value },
            Err(message) => Outcome::Error {
                code: message.split(':').next().unwrap_or("host-error").to_owned(),
                message,
            },
        };
        Response {
            version: PROTOCOL_VERSION,
            id: request.id,
            owner: request.owner,
            outcome,
        }
    }

    async fn dispatch(&mut self, operation: Operation) -> Result<Value, String> {
        match operation {
            Operation::Trust { call } => call.execute(
                &self.files,
                self.trust.as_ref().ok_or("host-trust-unavailable")?,
            ),
            Operation::WorkspaceAuthorize { workspace } => {
                let path = self.files.canonical_root(&workspace)?;
                let identity = self
                    .trust
                    .as_ref()
                    .ok_or("host-trust-unavailable")?
                    .require_trusted(path)?;
                serde_json::to_value(identity).map_err(|e| e.to_string())
            }
            Operation::Git {
                workspace,
                repository_root,
                call,
            } => {
                let path = self.files.canonical_root(&workspace)?.to_owned();
                let trust = self.trust.as_ref().ok_or("host-trust-unavailable")?.clone();
                let git = self.git.clone();
                let cancelled = self.cancelled.clone();
                tokio::task::spawn_blocking(move || {
                    crate::git_process::with_cancellation(cancelled, || {
                        git.lock().map_err(|e| e.to_string())?.execute_root(
                            &path,
                            &trust,
                            &workspace,
                            repository_root.as_deref(),
                            call,
                        )
                    })
                })
                .await
                .map_err(|e| e.to_string())?
            }
            Operation::Hello => serde_json::to_value(Hello {
                protocol: PROTOCOL_VERSION,
                version: env!("CARGO_PKG_VERSION").into(),
                os: std::env::consts::OS.into(),
                arch: std::env::consts::ARCH.into(),
                home: std::env::var("HOME").map_err(|_| "home-unavailable")?,
                methods: methods(),
            })
            .map_err(|e| e.to_string()),
            Operation::ClipboardImage { png_base64 } => tokio::task::spawn_blocking(move || {
                crate::clipboard_image::stage(&png_base64).map(Value::String)
            })
            .await
            .map_err(|e| e.to_string())?,
            Operation::WorkspaceOpen { path } => self.files.open(&path),
            Operation::WorkspaceClose { workspace } => {
                self.git
                    .lock()
                    .map_err(|e| e.to_string())?
                    .close(&workspace);
                self.files.close(&workspace);
                Ok(Value::Null)
            }
            Operation::FilesList { workspace, path } => self.files.list(&workspace, &path),
            Operation::FilesCreate {
                workspace,
                path,
                directory,
            } => self.files.create(&workspace, &path, directory),
            Operation::FilesRename {
                workspace,
                from,
                to,
            } => self.files.rename(&workspace, &from, &to),
            Operation::FilesDelete { workspace, path } => self.files.delete(&workspace, &path),
            Operation::FilesReadBase64 {
                workspace,
                path,
                max_bytes,
            } => self.files.read_base64(&workspace, &path, max_bytes),
            Operation::FilesRead { workspace, path } => self.files.read(&workspace, &path),
            Operation::FilesWrite {
                workspace,
                path,
                content,
                revision,
            } => self.files.write(&workspace, &path, &content, &revision),
            Operation::HerdrCall { binary, call } => {
                let manager = self.runtime(&binary)?;
                tokio::task::spawn_blocking(move || call.execute(&manager))
                    .await
                    .map_err(|e| e.to_string())?
            }
            Operation::HerdrMetadata {
                binary,
                query,
                session,
            } => {
                let manager = self.runtime(&binary)?;
                tokio::task::spawn_blocking(move || manager.metadata(query, session.as_deref()))
                    .await
                    .map_err(|e| e.to_string())?
            }
            Operation::HerdrStart { binary } => {
                let manager = self.runtime(&binary)?;
                tokio::task::spawn_blocking(move || {
                    manager
                        .ensure_server_running_on_startup()
                        .map(|started| json!({"started":started}))
                })
                .await
                .map_err(|e| e.to_string())?
            }
            Operation::HerdrDiscover { binary } => {
                let sessions = command_json(&binary, &["session", "list", "--json"]).await?;
                let schema = command_json(&binary, &["api", "schema", "--json"]).await?;
                self.sockets.clear();
                let rows = sessions
                    .as_array()
                    .or_else(|| sessions.get("sessions").and_then(Value::as_array));
                if let Some(rows) = rows {
                    for row in rows {
                        if let Some(socket) = row.get("socket_path").and_then(Value::as_str) {
                            self.sockets.insert(socket.into());
                        }
                    }
                }
                Ok(json!({"sessions":sessions,"schema":schema}))
            }
            Operation::HerdrRequest { socket, request } => {
                if !self.sockets.contains(&socket) {
                    return Err("undiscovered-herdr-socket".into());
                }
                socket_request(&socket, request).await
            }
        }
    }
}

pub async fn serve<R, W>(input: R, mut output: W) -> Result<(), String>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut input = BufReader::new(input);
    let mut server = HostServer::default();
    let mut frame = read_frame(&mut input).await?;
    while let Some(bytes) = frame {
        let request: Request =
            serde_json::from_slice(&bytes).map_err(|e| format!("invalid-request: {e}"))?;
        // Poll EOF while a blocking workspace job runs. Keep this same frame
        // reader alive until completion; cancelling a partial read loses bytes.
        let next = read_frame(&mut input);
        tokio::pin!(next);
        let mut prefetched = None;
        let handling = server.handle(request);
        tokio::pin!(handling);
        let response = tokio::select! {
            response = &mut handling => response,
            incoming = &mut next => {
                let incoming = incoming?;
                if incoming.is_none() { return Ok(()); }
                prefetched = Some(incoming);
                handling.await
            }
        };
        let mut bytes = serde_json::to_vec(&response).map_err(|e| e.to_string())?;
        if bytes.len() >= MAX_FRAME_BYTES {
            return Err("response-too-large".into());
        }
        bytes.push(b'\n');
        tokio::time::timeout(Duration::from_secs(10), async {
            output.write_all(&bytes).await?;
            output.flush().await
        })
        .await
        .map_err(|_| "host-write-timeout")?
        .map_err(|e| e.to_string())?;
        frame = match prefetched {
            Some(frame) => frame,
            None => next.await?,
        };
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(generation: u64, operation: Operation) -> Request {
        Request {
            version: PROTOCOL_VERSION,
            id: "id".into(),
            owner: ConnectionOwner {
                host_id: "test".into(),
                generation,
            },
            operation,
        }
    }
    #[tokio::test]
    async fn handshake_and_owner_are_enforced() {
        let mut server = HostServer::default();
        assert!(matches!(
            server
                .handle(request(1, Operation::WorkspaceOpen { path: "/".into() }))
                .await
                .outcome,
            Outcome::Error { .. }
        ));
        assert!(matches!(
            server.handle(request(1, Operation::Hello)).await.outcome,
            Outcome::Ok { .. }
        ));
        assert!(matches!(
            server.handle(request(2, Operation::Hello)).await.outcome,
            Outcome::Error { .. }
        ));
    }
    #[tokio::test]
    async fn rejects_oversized_and_partial_frames() {
        let bytes = vec![b'a'; MAX_FRAME_BYTES + 1];
        assert_eq!(
            read_frame(&mut BufReader::new(bytes.as_slice()))
                .await
                .unwrap_err(),
            "frame-too-large"
        );
        assert_eq!(
            read_frame(&mut BufReader::new(&b"{"[..]))
                .await
                .unwrap_err(),
            "truncated-frame"
        );
    }
}
