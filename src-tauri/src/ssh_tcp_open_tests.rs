//! Deferred direct-TCP opens over owned SSH/TCP listeners. No user host or data.
use super::*;
use russh::server::{self, Auth, Server};
use std::sync::atomic::AtomicUsize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::task::JoinSet;

#[path = "ssh_tcp_cpu_tests.rs"]
mod cpu_attribution;
use cpu_attribution::ServerThread;

#[path = "ssh_rejected_open_tests.rs"]
mod rejected_open;

struct Pending {
    release: oneshot::Sender<()>,
    queued: oneshot::Receiver<()>,
}

struct DeferredTunnel {
    tunnel_id: String,
    client: TcpStream,
    pending: Pending,
}

struct Probe {
    defer_next: AtomicBool,
    opened: AtomicUsize,
    closed: AtomicUsize,
    server_sockets: Arc<AtomicUsize>,
    backend_sockets: Arc<AtomicUsize>,
    pending: mpsc::Sender<Pending>,
    control: Mutex<Option<(server::Handle, russh::ChannelId)>>,
}

struct Active(Arc<AtomicUsize>);
impl Active {
    fn new(counter: Arc<AtomicUsize>) -> Self {
        counter.fetch_add(1, Ordering::SeqCst);
        Self(counter)
    }
}
impl Drop for Active {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

struct FixtureServer {
    password: String,
    endpoint: yuzora_host::tunnel::Endpoint,
    probe: Arc<Probe>,
    jobs: HashMap<russh::ChannelId, tokio::task::JoinHandle<()>>,
}
impl Drop for FixtureServer {
    fn drop(&mut self) {
        for task in self.jobs.values() {
            task.abort();
        }
    }
}
impl server::Server for FixtureServer {
    type Handler = Self;
    fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Self {
        Self {
            password: self.password.clone(),
            endpoint: self.endpoint.clone(),
            probe: self.probe.clone(),
            jobs: HashMap::new(),
        }
    }
}
impl server::Handler for FixtureServer {
    type Error = russh::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        Ok(if user == "fixture" && password == self.password {
            Auth::Accept
        } else {
            Auth::reject()
        })
    }

    async fn channel_open_direct_tcpip(
        &mut self,
        channel: russh::Channel<server::Msg>,
        host: &str,
        port: u32,
        _: &str,
        _: u32,
        reply: server::ChannelOpenHandle,
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        assert_eq!(host, self.endpoint.host);
        assert_eq!(port, u32::from(self.endpoint.port));
        let id = channel.id();
        let deferred = self.probe.defer_next.swap(false, Ordering::SeqCst);
        if !deferred && self.probe.control.lock().unwrap().is_none() {
            *self.probe.control.lock().unwrap() = Some((session.handle(), id));
        }
        let endpoint = self.endpoint.clone();
        let probe = self.probe.clone();
        let task = tokio::spawn(async move {
            let mut socket = TcpStream::connect((endpoint.host.as_str(), endpoint.port))
                .await
                .unwrap();
            let _active = Active::new(probe.server_sockets.clone());
            probe.opened.fetch_add(1, Ordering::SeqCst);
            if deferred {
                let (release, wait) = oneshot::channel();
                let (queued, acknowledged) = oneshot::channel();
                probe
                    .pending
                    .send(Pending {
                        release,
                        queued: acknowledged,
                    })
                    .await
                    .unwrap();
                wait.await.unwrap();
                reply.accept().await;
                let _ = queued.send(());
            } else {
                reply.accept().await;
            }
            let mut stream = channel.into_stream();
            let _ = tokio::io::copy_bidirectional(&mut stream, &mut socket).await;
        });
        assert!(self.jobs.insert(id, task).is_none());
        Ok(())
    }

    async fn channel_close(
        &mut self,
        id: russh::ChannelId,
        _: &mut server::Session,
    ) -> Result<(), Self::Error> {
        if let Some(task) = self.jobs.remove(&id) {
            task.abort();
            let _ = task.await;
        }
        self.probe.closed.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

struct SpawnedServers {
    endpoint: yuzora_host::tunnel::Endpoint,
    port: u16,
    fingerprint: String,
    ssh_task: tokio::task::JoinHandle<()>,
    backend_task: tokio::task::JoinHandle<()>,
}

async fn spawn_owned_servers(probe: Arc<Probe>, password: String) -> SpawnedServers {
    let backend = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = yuzora_host::tunnel::Endpoint {
        host: "127.0.0.1".into(),
        port: backend.local_addr().unwrap().port(),
    };
    let backend_probe = probe.clone();
    let backend_task = tokio::spawn(async move {
        let mut jobs = JoinSet::new();
        loop {
            tokio::select! {
                biased;
                done = jobs.join_next(), if !jobs.is_empty() => { done.unwrap().unwrap(); }
                accepted = backend.accept() => {
                    let (mut socket, _) = accepted.unwrap();
                    let probe = backend_probe.clone();
                    jobs.spawn(async move {
                        let _active = Active::new(probe.backend_sockets.clone());
                        let (mut read, mut write) = socket.split();
                        let _ = tokio::io::copy(&mut read, &mut write).await;
                    });
                }
            }
        }
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let key = russh::keys::PrivateKey::from_openssh(tests::TEST_HOST_KEY).unwrap();
    let fingerprint = fingerprint_sha256(key.public_key());
    let config = Arc::new(server::Config {
        keys: vec![key],
        ..Default::default()
    });
    let mut server = FixtureServer {
        password: password.clone(),
        endpoint: endpoint.clone(),
        probe: probe.clone(),
        jobs: HashMap::new(),
    };
    let ssh_task = tokio::spawn(async move {
        let _ = server.run_on_socket(config, &listener).await;
    });
    SpawnedServers {
        endpoint,
        port,
        fingerprint,
        ssh_task,
        backend_task,
    }
}

struct Fixture {
    _root: tempfile::TempDir,
    manager: Arc<SshManager>,
    session: String,
    host: Arc<crate::host_service::HostConnection>,
    endpoint: yuzora_host::tunnel::Endpoint,
    probe: Arc<Probe>,
    pending: mpsc::Receiver<Pending>,
    control: crate::host_service::HostStream,
    ssh_task: tokio::task::JoinHandle<()>,
    backend_task: tokio::task::JoinHandle<()>,
}

impl Fixture {
    async fn new() -> Self {
        Self::new_with_server_thread(false).await.0
    }

    async fn new_with_server_thread(split_server: bool) -> (Self, Option<ServerThread>) {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().canonicalize().unwrap();
        let (pending_tx, pending) = mpsc::channel(4);
        let probe = Arc::new(Probe {
            defer_next: AtomicBool::new(false),
            opened: AtomicUsize::new(0),
            closed: AtomicUsize::new(0),
            server_sockets: Arc::default(),
            backend_sockets: Arc::default(),
            pending: pending_tx,
            control: Mutex::new(None),
        });
        let password = uuid::Uuid::new_v4().to_string();
        let (servers, server_thread) = if split_server {
            let (servers, thread) = ServerThread::start(probe.clone(), password.clone()).await;
            (servers, Some(thread))
        } else {
            (
                spawn_owned_servers(probe.clone(), password.clone()).await,
                None,
            )
        };
        let SpawnedServers {
            endpoint,
            port,
            fingerprint,
            ssh_task,
            backend_task,
        } = servers;
        let manager = Arc::new(SshManager::with_parts(
            Box::new(|_| {}),
            path.join("known-hosts.json"),
            Arc::new(|_| {}),
            Duration::from_secs(2),
            Arc::new(StdHostKeyIo),
        ));
        manager
            .host_keys
            .persist_pin(&canonical_endpoint("127.0.0.1", port), &fingerprint)
            .unwrap();
        let connected = manager
            .connect(
                "127.0.0.1".into(),
                port,
                "fixture".into(),
                SshAuth::Password { password },
            )
            .await
            .unwrap();
        let session = connected.session_id;
        let control = manager.open_host_tcp(&session, &endpoint).await.unwrap();
        let (dummy, _) = tokio::io::duplex(1024);
        let host = Arc::new(crate::host_service::HostConnection::new(
            yuzora_host::protocol::ConnectionOwner {
                host_id: "owned-ssh-tunnel".into(),
                generation: 1,
            },
            crate::host_service::HostTarget::Ssh {
                session_id: session.clone(),
            },
            "unused-owned-helper".into(),
            Box::new(dummy),
        ));
        let mut fixture = Self {
            _root: root,
            manager,
            session,
            host,
            endpoint,
            probe,
            pending,
            control,
            ssh_task,
            backend_task,
        };
        fixture.fence().await;
        (fixture, server_thread)
    }

    async fn fence(&mut self) {
        let (handle, id) = self.probe.control.lock().unwrap().as_ref().unwrap().clone();
        // russh drains queued open replies before Handle::data messages, including
        // in its receiver select arm. The echo back orders the client close too.
        handle.data(id, b"fence!".to_vec()).await.unwrap();
        let mut bytes = [0; 6];
        self.control.read_exact(&mut bytes).await.unwrap();
        assert_eq!(&bytes, b"fence!");
        self.control.write_all(&bytes).await.unwrap();
        self.control.read_exact(&mut bytes).await.unwrap();
        assert_eq!(&bytes, b"fence!");
    }

    async fn begin_deferred_open(&mut self) -> DeferredTunnel {
        self.probe.defer_next.store(true, Ordering::SeqCst);
        let tunnel = crate::host_tunnels::open(
            self.host.clone(),
            self.manager.clone(),
            "owned-preview".into(),
            self.endpoint.clone(),
        )
        .await
        .unwrap();
        let client = TcpStream::connect(("127.0.0.1", tunnel.local_port))
            .await
            .unwrap();
        let pending = tokio::time::timeout(Duration::from_secs(5), self.pending.recv())
            .await
            .unwrap()
            .unwrap();
        DeferredTunnel {
            tunnel_id: tunnel.tunnel_id,
            client,
            pending,
        }
    }

    async fn cancel_owner(&self, deferred: DeferredTunnel) -> Pending {
        let DeferredTunnel {
            tunnel_id,
            mut client,
            pending,
        } = deferred;
        crate::host_tunnels::close(&self.host, "owned-preview", &tunnel_id).unwrap();
        let ended = tokio::time::timeout(Duration::from_secs(5), client.read_u8())
            .await
            .unwrap()
            .unwrap_err();
        assert_eq!(ended.kind(), std::io::ErrorKind::UnexpectedEof);
        tokio::time::timeout(Duration::from_secs(5), async {
            while self.host.tunnel_clients.available_permits() != 32 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        pending
    }

    async fn begin_cancelled_open(&mut self) -> Pending {
        let deferred = self.begin_deferred_open().await;
        self.cancel_owner(deferred).await
    }

    async fn confirm_pending(&mut self, pending: Pending) {
        pending.release.send(()).unwrap();
        pending.queued.await.unwrap();
        self.fence().await;
    }

    async fn finish_pending(&mut self, pending: Pending) {
        self.confirm_pending(pending).await;
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    async fn cancelled_open(&mut self) {
        let pending = self.begin_cancelled_open().await;
        self.finish_pending(pending).await;
    }

    async fn queued_cancelled_open(&mut self) {
        self.probe.defer_next.store(true, Ordering::SeqCst);
        let manager = self.manager.clone();
        let session = self.session.clone();
        let endpoint = self.endpoint.clone();
        let mut opening = Box::pin(manager.open_host_tcp(&session, &endpoint));
        std::future::poll_fn(|cx| {
            assert!(std::future::Future::poll(opening.as_mut(), cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        let pending = self.pending.recv().await.unwrap();
        pending.release.send(()).unwrap();
        pending.queued.await.unwrap();
        // The future stays unpolled while the existing channel proves that
        // the SSH client has already received and queued the confirmation.
        self.fence().await;
        drop(opening);
        self.fence().await;
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    async fn normal_open(&mut self, bytes: &[u8], received: &mut [u8]) {
        let closed = self.probe.closed.load(Ordering::SeqCst);
        let mut stream = self
            .manager
            .open_host_tcp(&self.session, &self.endpoint)
            .await
            .unwrap();
        stream.write_all(bytes).await.unwrap();
        stream.read_exact(received).await.unwrap();
        assert_eq!(bytes, received);
        drop(stream);
        self.fence().await;
        tokio::time::timeout(Duration::from_secs(5), async {
            while self.probe.closed.load(Ordering::SeqCst) != closed + 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    async fn assert_only_control(&mut self) {
        self.fence().await;
        tokio::time::timeout(Duration::from_secs(2), async {
            while self.probe.server_sockets.load(Ordering::SeqCst) != 1
                || self.probe.backend_sockets.load(Ordering::SeqCst) != 1
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("cancelled SSH channels must release both owned TCP sockets");
        assert_eq!(self.host.tunnels.lock().unwrap().len(), 0);
        assert_eq!(self.host.tunnel_clients.available_permits(), 32);
    }

    fn sample(&self, cycle: usize, memory: &mut sysinfo::System) -> ResourceSample {
        let pid = sysinfo::get_current_pid().unwrap();
        memory.refresh_processes_specifics(
            sysinfo::ProcessesToUpdate::Some(&[pid]),
            true,
            sysinfo::ProcessRefreshKind::nothing().with_memory(),
        );
        ResourceSample {
            cycle,
            opened: self.probe.opened.load(Ordering::SeqCst),
            closed: self.probe.closed.load(Ordering::SeqCst),
            server_sockets: self.probe.server_sockets.load(Ordering::SeqCst),
            backend_sockets: self.probe.backend_sockets.load(Ordering::SeqCst),
            host_tunnels: self.host.tunnels.lock().unwrap().len(),
            host_permits: self.host.tunnel_clients.available_permits(),
            fds: std::fs::read_dir("/dev/fd").unwrap().count(),
            alive_tasks: tokio::runtime::Handle::current()
                .metrics()
                .num_alive_tasks(),
            rss_bytes: memory.process(pid).unwrap().memory(),
        }
    }

    async fn stop(self) {
        drop(self.control);
        self.manager.disconnect(&self.session).await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while self.probe.server_sockets.load(Ordering::SeqCst) != 0
                || self.probe.backend_sockets.load(Ordering::SeqCst) != 0
            {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
        self.ssh_task.abort();
        self.backend_task.abort();
        let _ = self.ssh_task.await;
        let _ = self.backend_task.await;
    }
}

#[derive(Clone, Copy, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ResourceSample {
    cycle: usize,
    opened: usize,
    closed: usize,
    server_sockets: usize,
    backend_sockets: usize,
    host_tunnels: usize,
    host_permits: usize,
    fds: usize,
    alive_tasks: usize,
    rss_bytes: u64,
}

fn process_cpu_ns() -> u64 {
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    assert_eq!(unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) }, 0);
    [usage.ru_utime, usage.ru_stime]
        .iter()
        .map(|t| t.tv_sec as u64 * 1_000_000_000 + t.tv_usec as u64 * 1_000)
        .sum()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_tcp_open_releases_late_confirmation_without_blocking_sibling() {
    tokio::time::timeout(Duration::from_secs(15), async {
        let mut fixture = Fixture::new().await;
        let pending = fixture.begin_cancelled_open().await;
        // Complete a later open before releasing the first confirmation.
        // This also proves cancellation released SshManager's handle lock.
        fixture.normal_open(b"sibling", &mut [0; 7]).await;
        fixture.finish_pending(pending).await;
        fixture.assert_only_control().await;
        fixture.stop().await;
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_tcp_open_releases_already_queued_confirmation() {
    tokio::time::timeout(Duration::from_secs(15), async {
        let mut fixture = Fixture::new().await;
        fixture.queued_cancelled_open().await;
        fixture.assert_only_control().await;
        fixture.normal_open(b"still healthy", &mut [0; 13]).await;
        fixture.assert_only_control().await;
        fixture.stop().await;
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "manual owned SSH deferred TCP-open lifecycle probe"]
async fn performance_owned_deferred_tcp_open() {
    tokio::time::timeout(Duration::from_secs(120), async {
        let mode = std::env::var("YUZORA_038_MODE").unwrap_or_else(|_| "late".into());
        assert!(["late", "queued", "normal4k", "normal64k"].contains(&mode.as_str()));
        let warmup = 100;
        let cycles = 200;
        let payload = vec![0x5a; if mode == "normal64k" { 65536 } else { 4096 }];
        let mut received = vec![0; payload.len()];
        let mut samples = vec![ResourceSample::default(); 11];
        let mut latencies = vec![0_u64; cycles];
        let mut fixture = Fixture::new().await;
        let mut memory = sysinfo::System::new();
        for _ in 0..warmup {
            match mode.as_str() {
                "late" => fixture.cancelled_open().await,
                "queued" => fixture.queued_cancelled_open().await,
                _ => fixture.normal_open(&payload, &mut received).await,
            }
        }
        samples[0] = fixture.sample(0, &mut memory);
        let cpu = process_cpu_ns();
        let wall = std::time::Instant::now();
        for cycle in 1..=cycles {
            let start = std::time::Instant::now();
            match mode.as_str() {
                "late" => fixture.cancelled_open().await,
                "queued" => fixture.queued_cancelled_open().await,
                _ => fixture.normal_open(&payload, &mut received).await,
            }
            latencies[cycle - 1] = start.elapsed().as_nanos() as u64;
            if cycle % 20 == 0 {
                samples[cycle / 20] = fixture.sample(cycle, &mut memory);
            }
        }
        let wall_ns = wall.elapsed().as_nanos() as u64;
        let cpu_ns = process_cpu_ns() - cpu;
        // Capture before teardown; disconnect must not hide abandoned channels.
        println!(
            "SSH_OPEN_RESULT {}",
            serde_json::json!({
                "mode": mode, "warmup": warmup, "cycles": cycles,
                "cpuNs": cpu_ns, "wallNs": wall_ns,
                "latencyNs": latencies, "samples": samples,
            })
        );
        fixture.stop().await;
        println!("SSH_OPEN_END {{\"ownedSocketsReleasedAfterFullDisconnect\":true}}");
    })
    .await
    .expect("owned deferred-open deadline");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejected_session_open_preserves_existing_tcp_channel() {
    tokio::time::timeout(Duration::from_secs(15), async {
        let mut fixture = Fixture::new().await;
        let handle = fixture.manager.get_handle(&fixture.session).unwrap();
        // The fixture's default session handler rejects without creating a
        // shell, process, or extra socket. Verify the shared open/error path.
        let result = handle.lock().await.channel_open_session().await;
        assert!(matches!(
            result,
            Err(russh::Error::ChannelOpenFailure(
                russh::ChannelOpenFailure::AdministrativelyProhibited
            ))
        ));
        fixture.normal_open(b"after rejection", &mut [0; 15]).await;
        fixture.assert_only_control().await;
        fixture.stop().await;
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_tcp_open_does_not_block_disconnect_without_confirmation() {
    tokio::time::timeout(Duration::from_secs(15), async {
        let mut fixture = Fixture::new().await;
        let pending = fixture.begin_cancelled_open().await;
        // Keep the server's confirmation gated while disconnecting. There
        // must be no cleanup future holding the SSH handle lock indefinitely.
        fixture.stop().await;
        drop(pending);
    })
    .await
    .unwrap();
}
