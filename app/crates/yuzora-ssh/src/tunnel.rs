//! Local port forwards (`ssh -L`): a port on 127.0.0.1 whose connections come
//! out of a host's SSH session toward `remote_host:remote_port`, as the
//! server sees it. Other lanes use this to reach a database behind a host:
//!
//! ```no_run
//! # async fn demo() -> Result<(), yuzora_ssh::SshError> {
//! use yuzora_ssh::tunnel::{self, TunnelSpec};
//! // "lab-01" may be an ssh config alias, a host id ("ssh:lab-01",
//! // "profile:…", "machine:…") or a destination ("me@db.example:2222").
//! let t = tunnel::open(TunnelSpec::new("lab-01", "127.0.0.1", 5432)).await?;
//! let url = format!("postgres://app@127.0.0.1:{}/app", t.local_port());
//! // … connect with `url` …
//! t.close(); // or drop it
//! # Ok(()) }
//! ```
//!
//! [`open`] works from any executor (the SSH work runs on this crate's own
//! runtime) and [`open_blocking`] from a plain thread. The host's session is
//! shared with SFTP and other tunnels; when it drops, the next connection
//! through the tunnel reconnects (without asking anyone). A tunnel stays open
//! until [`Tunnel::close`], its drop, or [`crate::session::Manager::disconnect`]
//! of its host.

use crate::error::{ErrorKind, Result, SshError};
use crate::session::{Interaction, Manager, manager};
use serde::Serialize;
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::watch;

/// What to forward.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TunnelSpec {
    /// The SSH host: id, ssh config alias, profile or machine name, or `user@host:port`.
    pub host: String,
    /// Where the server connects, e.g. "127.0.0.1" for a database on the host itself.
    pub remote_host: String,
    pub remote_port: u16,
    /// The local port; 0 picks a free one.
    pub local_port: u16,
    /// May the first connection ask the person (a new host key, a password)?
    /// Without it, a host that isn't trusted yet or needs a password fails at
    /// once with [`ErrorKind::HostKeyUnknown`] or [`ErrorKind::NeedsInput`].
    pub interactive: bool,
}

impl TunnelSpec {
    /// A forward on a free local port, without asking anyone.
    pub fn new(host: impl Into<String>, remote_host: impl Into<String>, remote_port: u16) -> Self {
        Self { host: host.into(), remote_host: remote_host.into(), remote_port, local_port: 0, interactive: false }
    }

    pub fn local_port(mut self, port: u16) -> Self {
        self.local_port = port;
        self
    }

    pub fn interactive(mut self, yes: bool) -> Self {
        self.interactive = yes;
        self
    }
}

/// A tunnel's numbers, for lists.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelInfo {
    pub id: u64,
    pub host: String,
    pub remote_host: String,
    pub remote_port: u16,
    pub local_port: u16,
    /// Connections open now, and since it opened.
    pub active: u64,
    pub total: u64,
    pub bytes_up: u64,
    pub bytes_down: u64,
    /// ms since the epoch.
    pub opened_at: u64,
}

struct Inner {
    info: Mutex<TunnelInfo>,
    active: AtomicU64,
    total: AtomicU64,
    up: AtomicU64,
    down: AtomicU64,
    stop: watch::Sender<bool>,
}

impl Inner {
    fn info(&self) -> TunnelInfo {
        let mut i = self.info.lock().unwrap_or_else(|e| e.into_inner()).clone();
        i.active = self.active.load(Ordering::Relaxed);
        i.total = self.total.load(Ordering::Relaxed);
        i.bytes_up = self.up.load(Ordering::Relaxed);
        i.bytes_down = self.down.load(Ordering::Relaxed);
        i
    }
}

/// Every open tunnel (the manager owns one).
#[derive(Default)]
pub struct Registry {
    open: Mutex<BTreeMap<u64, Arc<Inner>>>,
    next: AtomicU64,
    /// Tunnels opened from scripts, which close them by id.
    held: Mutex<BTreeMap<u64, Tunnel>>,
}

impl Registry {
    pub fn list(&self) -> Vec<TunnelInfo> {
        self.open.lock().unwrap_or_else(|e| e.into_inner()).values().map(|t| t.info()).collect()
    }

    fn remove(&self, id: u64) -> Option<Arc<Inner>> {
        self.open.lock().unwrap_or_else(|e| e.into_inner()).remove(&id)
    }

    /// Closes one tunnel by id.
    pub fn close(&self, id: u64) -> bool {
        let held = self.held.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
        let inner = self.remove(id);
        if let Some(t) = &inner {
            t.stop.send_replace(true);
        }
        let found = held.is_some() || inner.is_some();
        // Its drop finds nothing left to remove.
        drop(held);
        found
    }

    /// Closes every tunnel through a host (it is being disconnected).
    pub fn close_host(&self, host_id: &str) {
        let ids: Vec<u64> = self.list().into_iter().filter(|t| t.host == host_id).map(|t| t.id).collect();
        for id in ids {
            self.close(id);
        }
    }

    /// Keeps a script's tunnel open until `close(id)`.
    pub fn hold(&self, t: Tunnel) -> u64 {
        let id = t.id;
        self.held.lock().unwrap_or_else(|e| e.into_inner()).insert(id, t);
        id
    }
}

/// An open forward; closes when dropped.
pub struct Tunnel {
    id: u64,
    local: SocketAddr,
    manager: Arc<Manager>,
}

impl Tunnel {
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Where to connect: always on 127.0.0.1.
    pub fn local_addr(&self) -> SocketAddr {
        self.local
    }

    pub fn local_port(&self) -> u16 {
        self.local.port()
    }

    /// False once closed or its host was disconnected.
    pub fn is_open(&self) -> bool {
        self.manager.tunnels.open.lock().unwrap_or_else(|e| e.into_inner()).contains_key(&self.id)
    }

    pub fn info(&self) -> Option<TunnelInfo> {
        self.manager.tunnels.open.lock().unwrap_or_else(|e| e.into_inner()).get(&self.id).map(|t| t.info())
    }

    /// Stops listening and ends its connections.
    pub fn close(self) {}
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        if let Some(t) = self.manager.tunnels.remove(self.id) {
            t.stop.send_replace(true);
        }
    }
}

impl std::fmt::Debug for Tunnel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tunnel").field("id", &self.id).field("local", &self.local).finish()
    }
}

/// Opens a tunnel with the app's manager. Works from any executor.
pub async fn open(spec: TunnelSpec) -> Result<Tunnel> {
    let m = manager();
    crate::runtime::bridge(async move { m.open_tunnel(spec).await }).await.unwrap_or_else(|| Err(SshError::closed()))
}

/// [`open`] for a plain thread (not inside an async task).
pub fn open_blocking(spec: TunnelSpec) -> Result<Tunnel> {
    let m = manager();
    crate::runtime::block_on(async move { m.open_tunnel(spec).await }).unwrap_or_else(|| Err(SshError::closed()))
}

/// Every open tunnel of the app's manager.
pub fn list() -> Vec<TunnelInfo> {
    manager().tunnels.list()
}

impl Manager {
    /// Connects (or reuses the host's session), binds the local port and starts forwarding.
    /// Must run on the crate's runtime ([`open`] takes care of that).
    pub async fn open_tunnel(self: &Arc<Self>, spec: TunnelSpec) -> Result<Tunnel> {
        if spec.remote_port == 0 || spec.remote_host.trim().is_empty() {
            return Err(SshError::text(ErrorKind::Invalid, "port", &[]));
        }
        let interaction = if spec.interactive { Interaction::Ask } else { Interaction::Never };
        let session = self.session(&spec.host, interaction).await?;
        let host_id = session.host_id.clone();
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", spec.local_port)).await.map_err(|e| SshError::text(ErrorKind::Io, "bind", &[("port", spec.local_port.to_string()), ("why", e.to_string())]))?;
        let local = listener.local_addr().map_err(SshError::io)?;
        let id = self.tunnels.next.fetch_add(1, Ordering::Relaxed) + 1;
        let (stop, stopped) = watch::channel(false);
        let inner = Arc::new(Inner {
            info: Mutex::new(TunnelInfo {
                id,
                host: host_id.clone(),
                remote_host: spec.remote_host.clone(),
                remote_port: spec.remote_port,
                local_port: local.port(),
                active: 0,
                total: 0,
                bytes_up: 0,
                bytes_down: 0,
                opened_at: SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0),
            }),
            active: AtomicU64::new(0),
            total: AtomicU64::new(0),
            up: AtomicU64::new(0),
            down: AtomicU64::new(0),
            stop,
        });
        self.tunnels.open.lock().unwrap_or_else(|e| e.into_inner()).insert(id, inner.clone());
        let me = self.clone();
        tokio::spawn(async move {
            let mut stopped_accept = stopped.clone();
            loop {
                let accepted = tokio::select! {
                    _ = stopped_accept.wait_for(|s| *s) => break,
                    accepted = listener.accept() => accepted,
                };
                let Ok((socket, _)) = accepted else {
                    // Out of descriptors and the like: wait instead of spinning.
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                };
                let (me, inner, spec, host_id) = (me.clone(), inner.clone(), spec.clone(), host_id.clone());
                let mut stopped_conn = stopped.clone();
                tokio::spawn(async move {
                    inner.total.fetch_add(1, Ordering::Relaxed);
                    inner.active.fetch_add(1, Ordering::Relaxed);
                    let _ = socket.set_nodelay(true);
                    let forward = async {
                        // The session may have dropped since: reconnect without asking.
                        let session = me.session(&host_id, Interaction::Never).await?;
                        let channel = session.direct_tcpip(&spec.remote_host, spec.remote_port).await?;
                        let mut remote = channel.into_stream();
                        let mut socket = socket;
                        let (up, down) = tokio::io::copy_bidirectional(&mut socket, &mut remote).await.map_err(SshError::io)?;
                        inner.up.fetch_add(up, Ordering::Relaxed);
                        inner.down.fetch_add(down, Ordering::Relaxed);
                        Ok::<(), SshError>(())
                    };
                    tokio::select! {
                        _ = stopped_conn.wait_for(|s| *s) => {}
                        _ = forward => {}
                    }
                    inner.active.fetch_sub(1, Ordering::Relaxed);
                });
            }
        });
        Ok(Tunnel { id, local, manager: self.clone() })
    }
}
