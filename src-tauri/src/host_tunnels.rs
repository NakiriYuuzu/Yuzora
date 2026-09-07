//! Loopback listeners owned by a host generation and one UI resource.
use crate::host_service::{open_stream, HostConnection, HostState, HostStream, HostTarget};
use crate::ssh_service::SshManager;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::{watch, Semaphore};
use tokio::task::JoinSet;
use yuzora_host::protocol::{ConnectionOwner, Outcome, Response, PROTOCOL_VERSION};
use yuzora_host::tunnel::{Endpoint, TunnelRequest};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelInfo {
    pub tunnel_id: String,
    pub local_port: u16,
}
pub(crate) struct Tunnel {
    resource_owner: String,
    cancelled: watch::Sender<bool>,
}
impl Drop for Tunnel {
    fn drop(&mut self) {
        self.cancelled.send_replace(true);
    }
}
pub(crate) type Tunnels = Arc<Mutex<HashMap<String, Tunnel>>>;

async fn connect(
    connection: &HostConnection,
    ssh: &SshManager,
    endpoint: &Endpoint,
) -> Result<HostStream, String> {
    match &connection.target {
        HostTarget::Ssh { session_id } => ssh.open_host_tcp(session_id, endpoint).await,
        HostTarget::Local => Ok(Box::new(
            tokio::net::TcpStream::connect((endpoint.host.as_str(), endpoint.port))
                .await
                .map_err(|e| e.to_string())?,
        )),
        HostTarget::Wsl { .. } => {
            let stream = open_stream(
                &connection.target,
                &connection.helper,
                crate::host_service::HostLane::Tcp,
                ssh,
            )
            .await?;
            let mut io = BufReader::new(stream);
            let id = uuid::Uuid::new_v4().to_string();
            let mut bytes = serde_json::to_vec(&TunnelRequest {
                version: PROTOCOL_VERSION,
                id: id.clone(),
                owner: connection.owner.clone(),
                endpoint: endpoint.clone(),
            })
            .map_err(|e| e.to_string())?;
            bytes.push(b'\n');
            io.write_all(&bytes).await.map_err(|e| e.to_string())?;
            io.flush().await.map_err(|e| e.to_string())?;
            let bytes = yuzora_host::wire::read_frame(&mut io)
                .await?
                .ok_or("tunnel-helper-ended")?;
            let response: Response = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
            if response.version != PROTOCOL_VERSION
                || response.owner != connection.owner
                || response.id != id
            {
                return Err("tunnel-identity-mismatch".into());
            }
            match response.outcome {
                Outcome::Error { message, .. } => Err(message),
                // Preserve bytes buffered beyond the handshake (e.g. a DB banner).
                Outcome::Ok { .. } => Ok(Box::new(io)),
            }
        }
    }
}

async fn serve(
    listener: TcpListener,
    connection: Arc<HostConnection>,
    ssh: Arc<SshManager>,
    endpoint: Endpoint,
    mut cancelled: watch::Receiver<bool>,
) {
    let mut host_cancelled = connection.cancelled.subscribe();
    let mut jobs = JoinSet::new();
    loop {
        if *cancelled.borrow() || *host_cancelled.borrow() {
            break;
        }
        tokio::select! {
            biased;
            _ = cancelled.changed() => break,
            _ = host_cancelled.changed() => break,
            _ = jobs.join_next(), if !jobs.is_empty() => {},
            accepted = listener.accept() => {
                let Ok((mut socket, _)) = accepted else { break; };
                // Per host, not per listener: many tabs cannot multiply the cap.
                let Ok(permit) = connection.tunnel_clients.clone().try_acquire_owned() else { continue; };
                let connection = connection.clone();
                let ssh = ssh.clone();
                let endpoint = endpoint.clone();
                jobs.spawn(async move {
                    let _permit = permit;
                    let opened = tokio::time::timeout(Duration::from_secs(15), connect(&connection, &ssh, &endpoint)).await;
                    if let Ok(Ok(mut upstream)) = opened {
                        // Fixed 8 KiB buffers in each direction; TCP backpressure
                        // carries through to SSH/stdio. No input is ever replayed.
                        let _ = tokio::io::copy_bidirectional(&mut socket, &mut upstream).await;
                    }
                });
            }
        }
    }
    jobs.abort_all();
    while jobs.join_next().await.is_some() {}
}

pub(crate) async fn open(
    connection: Arc<HostConnection>,
    ssh: Arc<SshManager>,
    resource_owner: String,
    endpoint: Endpoint,
) -> Result<TunnelInfo, String> {
    endpoint.validate()?;
    if resource_owner.is_empty() || resource_owner.len() > 4096 || resource_owner.contains('\0') {
        return Err("invalid-tunnel-owner".into());
    }
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| e.to_string())?;
    let local_port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let tunnel_id = uuid::Uuid::new_v4().to_string();
    let (cancelled, receiver) = watch::channel(false);
    {
        let mut tunnels = connection.tunnels.lock().unwrap();
        if *connection.cancelled.borrow() {
            return Err("host-disconnected".into());
        }
        if tunnels.len() >= 16 {
            return Err("too-many-host-tunnels".into());
        }
        tunnels.insert(
            tunnel_id.clone(),
            Tunnel {
                resource_owner,
                cancelled,
            },
        );
    }
    let resources = connection.tunnels.clone();
    let id = tunnel_id.clone();
    tokio::spawn(async move {
        serve(listener, connection, ssh, endpoint, receiver).await;
        resources.lock().unwrap().remove(&id);
    });
    Ok(TunnelInfo {
        tunnel_id,
        local_port,
    })
}

pub(crate) fn close(
    connection: &HostConnection,
    resource_owner: &str,
    tunnel_id: &str,
) -> Result<(), String> {
    let mut tunnels = connection.tunnels.lock().unwrap();
    if let Some(tunnel) = tunnels.get(tunnel_id) {
        if tunnel.resource_owner != resource_owner {
            return Err("tunnel-owner-mismatch".into());
        }
    }
    tunnels.remove(tunnel_id);
    Ok(())
}

pub(crate) fn client_limit() -> Arc<Semaphore> {
    Arc::new(Semaphore::new(32))
}

#[tauri::command]
pub async fn host_tunnel_open(
    state: tauri::State<'_, HostState>,
    ssh: tauri::State<'_, crate::ssh_service::SshState>,
    owner: ConnectionOwner,
    resource_owner: String,
    endpoint: Endpoint,
) -> Result<TunnelInfo, String> {
    let connection = state.0.connection(&owner)?;
    let opened = open(
        connection.clone(),
        ssh.0.clone(),
        resource_owner.clone(),
        endpoint,
    )
    .await?;
    if let Err(error) = state.0.connection(&owner) {
        let _ = close(&connection, &resource_owner, &opened.tunnel_id);
        return Err(error);
    }
    Ok(opened)
}

#[tauri::command]
pub async fn host_tunnel_close(
    state: tauri::State<'_, HostState>,
    owner: ConnectionOwner,
    resource_owner: String,
    tunnel_id: String,
) -> Result<(), String> {
    let connection = state.0.connection(&owner)?;
    close(&connection, &resource_owner, &tunnel_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;
    fn connection() -> Arc<HostConnection> {
        let (io, _) = tokio::io::duplex(1024);
        Arc::new(HostConnection::new(
            ConnectionOwner {
                host_id: "local-fixture".into(),
                generation: 1,
            },
            HostTarget::Local,
            "unused".into(),
            Box::new(io),
        ))
    }

    #[tokio::test]
    async fn tunnels_preserve_bytes_and_closing_one_owner_keeps_others_connected() {
        let backend = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = Endpoint {
            host: "127.0.0.1".into(),
            port: backend.local_addr().unwrap().port(),
        };
        let echo = tokio::spawn(async move {
            loop {
                let (mut socket, _) = backend.accept().await.unwrap();
                tokio::spawn(async move {
                    let (mut reader, mut writer) = socket.split();
                    let _ = tokio::io::copy(&mut reader, &mut writer).await;
                });
            }
        });
        let connection = connection();
        let ssh = Arc::new(SshManager::for_test());
        let first = open(
            connection.clone(),
            ssh.clone(),
            "preview-a".into(),
            endpoint.clone(),
        )
        .await
        .unwrap();
        let second = open(connection.clone(), ssh, "database-b".into(), endpoint)
            .await
            .unwrap();
        assert!(close(&connection, "database-b", &first.tunnel_id).is_err());
        close(&connection, "preview-a", &first.tunnel_id).unwrap();
        let mut socket = tokio::net::TcpStream::connect(("127.0.0.1", second.local_port))
            .await
            .unwrap();
        let payload = vec![0xff; 128 * 1024];
        let (mut read, mut write) = socket.split();
        let sent = async {
            write.write_all(&payload).await.unwrap();
            write.shutdown().await.unwrap();
        };
        let received = async {
            let mut body = Vec::new();
            read.read_to_end(&mut body).await.unwrap();
            body
        };
        let (_, body) = tokio::time::timeout(Duration::from_secs(3), async {
            tokio::join!(sent, received)
        })
        .await
        .unwrap();
        assert_eq!(body, payload);
        close(&connection, "database-b", &second.tunnel_id).unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            while connection.tunnel_clients.available_permits() != 32 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        echo.abort();
    }

    #[tokio::test]
    async fn host_disconnect_closes_listeners_and_active_clients_with_bounded_tunnels() {
        let backend = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = Endpoint {
            host: "127.0.0.1".into(),
            port: backend.local_addr().unwrap().port(),
        };
        let connection = connection();
        let ssh = Arc::new(SshManager::for_test());
        let mut infos = Vec::new();
        for _ in 0..16 {
            infos.push(
                open(
                    connection.clone(),
                    ssh.clone(),
                    "owner".into(),
                    endpoint.clone(),
                )
                .await
                .unwrap(),
            );
        }
        assert!(
            open(connection.clone(), ssh.clone(), "overflow".into(), endpoint)
                .await
                .unwrap_err()
                .contains("too-many")
        );
        let mut client = tokio::net::TcpStream::connect(("127.0.0.1", infos[0].local_port))
            .await
            .unwrap();
        let (_remote, _) = tokio::time::timeout(Duration::from_secs(3), backend.accept())
            .await
            .unwrap()
            .unwrap();
        connection.cancelled.send_replace(true);
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(3), client.read_u8())
                .await
                .unwrap()
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::UnexpectedEof
        );
        tokio::time::timeout(Duration::from_secs(3), async {
            while !connection.tunnels.lock().unwrap().is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        for info in infos {
            assert!(
                tokio::net::TcpStream::connect(("127.0.0.1", info.local_port))
                    .await
                    .is_err()
            );
        }
    }
}
