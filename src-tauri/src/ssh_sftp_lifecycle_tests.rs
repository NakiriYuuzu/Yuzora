//! Helper and SFTP startup cancellation over owned loopback SSH sessions.
use super::*;
use russh::server::{self, Auth};
use std::future::Future;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::task::JoinSet;

// Dropping a pending reply enqueues rejection. Keep it alive until the
// client-initiated SSH connection has finished closing in cancellation tests.
struct DeferredOpen {
    channel: russh::Channel<server::Msg>,
    reply: server::ChannelOpenHandle,
}

struct FixtureServer {
    password: String,
    pending: mpsc::UnboundedSender<DeferredOpen>,
    executed: mpsc::UnboundedSender<Vec<u8>>,
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

    async fn channel_open_session(
        &mut self,
        channel: russh::Channel<server::Msg>,
        reply: server::ChannelOpenHandle,
        _: &mut server::Session,
    ) -> Result<(), Self::Error> {
        // Keep the confirmation pending without blocking the SSH event loop.
        let _ = self.pending.send(DeferredOpen { channel, reply });
        Ok(())
    }

    async fn subsystem_request(
        &mut self,
        channel: russh::ChannelId,
        name: &str,
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        assert_eq!(name, "sftp");
        session.channel_success(channel)?;
        Ok(())
    }

    async fn exec_request(
        &mut self,
        channel: russh::ChannelId,
        command: &[u8],
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        // Record only: the fixture never executes a shell or helper command.
        let _ = self.executed.send(command.to_vec());
        session.channel_success(channel)?;
        Ok(())
    }
}

struct Fixture {
    _root: tempfile::TempDir,
    manager: Arc<SshManager>,
    session: String,
    pending: mpsc::UnboundedReceiver<DeferredOpen>,
    executed: mpsc::UnboundedReceiver<Vec<u8>>,
    control: server::Handle,
    server: JoinSet<Result<(), russh::Error>>,
}

impl Fixture {
    async fn new() -> Self {
        Self::with_manager(None).await
    }

    async fn with_manager(existing: Option<Arc<SshManager>>) -> Self {
        let root = tempfile::tempdir().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let key = russh::keys::PrivateKey::from_openssh(tests::TEST_HOST_KEY).unwrap();
        let fingerprint = fingerprint_sha256(key.public_key());
        let config = Arc::new(server::Config {
            keys: vec![key],
            ..Default::default()
        });
        let password = uuid::Uuid::new_v4().to_string();
        let (pending_tx, pending) = mpsc::unbounded_channel();
        let (executed_tx, executed) = mpsc::unbounded_channel();
        let handler = FixtureServer {
            password: password.clone(),
            pending: pending_tx,
            executed: executed_tx,
        };
        let (control_tx, control_rx) = oneshot::channel();
        let mut server = JoinSet::new();
        server.spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let running = server::run_stream(config, stream, handler).await?;
            let _ = control_tx.send(running.handle());
            running.await
        });
        let manager = existing.unwrap_or_else(|| {
            Arc::new(SshManager::with_parts(
                Box::new(|_| {}),
                root.path().join("known-hosts.json"),
                Arc::new(|_| {}),
                Duration::from_secs(2),
                Arc::new(StdHostKeyIo),
            ))
        });
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
        Self {
            _root: root,
            manager,
            session: connected.session_id,
            pending,
            executed,
            control: control_rx.await.unwrap(),
            server,
        }
    }

    async fn next_open(&mut self) -> DeferredOpen {
        tokio::time::timeout(Duration::from_secs(2), self.pending.recv())
            .await
            .expect("fixture must receive a channel open")
            .unwrap()
    }

    async fn finish(mut self) {
        let result = tokio::time::timeout(Duration::from_secs(2), self.server.join_next())
            .await
            .expect("owned SSH server must finish")
            .unwrap()
            .unwrap();
        // Dropping the last handle at shutdown produces EOF; disconnect during
        // SFTP INIT may reset a socket with unread bytes. Still await the server.
        match result {
            Ok(()) => {}
            Err(russh::Error::IO(error))
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::UnexpectedEof
                ) => {}
            other => panic!("unexpected fixture server result: {other:?}"),
        }
    }
}

#[tokio::test]
async fn disconnect_cancels_withheld_sftp_open() {
    let mut fixture = Fixture::new().await;
    let mut operations = JoinSet::new();
    let manager = fixture.manager.clone();
    let session = fixture.session.clone();
    operations.spawn(async move {
        (
            "listing",
            manager.sftp_list_dir(&session, ".").await.map(|_| ()),
        )
    });
    let deferred = fixture.next_open().await;
    let manager = fixture.manager.clone();
    let session = fixture.session.clone();
    operations.spawn(async move { ("disconnect", manager.disconnect(&session).await) });

    // Record the verdict before dropping/rejecting the pending confirmation.
    let verdict = tokio::time::timeout(Duration::from_secs(1), async {
        let mut outcomes = Vec::new();
        while let Some(result) = operations.join_next().await {
            outcomes.push(result.unwrap());
        }
        outcomes
    })
    .await;
    operations.abort_all();
    while operations.join_next().await.is_some() {}
    fixture.manager.disconnect(&fixture.session).await.unwrap();
    assert!(fixture.manager.sessions.lock().unwrap().is_empty());
    fixture.finish().await;
    drop(deferred);

    let outcomes =
        verdict.expect("disconnect and SFTP open must finish while confirmation is withheld");
    assert!(outcomes
        .iter()
        .any(|(name, result)| *name == "disconnect" && result.is_ok()));
    assert!(outcomes
        .iter()
        .any(|(name, result)| *name == "listing" && result.is_err()));
}

#[tokio::test]
async fn disconnect_cancels_withheld_helper_open() {
    let mut fixture = Fixture::new().await;
    let mut operations = JoinSet::new();
    let manager = fixture.manager.clone();
    let session = fixture.session.clone();
    operations.spawn(async move {
        (
            "helper",
            manager
                .open_host_helper(
                    &session,
                    "/owned/helper",
                    crate::host_service::HostLane::Control,
                )
                .await
                .map(|_| ()),
        )
    });
    let deferred = fixture.next_open().await;
    let manager = fixture.manager.clone();
    let session = fixture.session.clone();
    operations.spawn(async move { ("disconnect", manager.disconnect(&session).await) });
    let verdict = tokio::time::timeout(Duration::from_secs(1), async {
        let mut outcomes = Vec::new();
        while let Some(result) = operations.join_next().await {
            outcomes.push(result.unwrap());
        }
        outcomes
    })
    .await;
    operations.abort_all();
    while operations.join_next().await.is_some() {}
    fixture.manager.disconnect(&fixture.session).await.unwrap();
    fixture.finish().await;
    drop(deferred);
    let outcomes =
        verdict.expect("disconnect and helper open must finish with confirmation withheld");
    assert!(outcomes
        .iter()
        .any(|(name, result)| *name == "disconnect" && result.is_ok()));
    assert!(outcomes
        .iter()
        .any(|(name, result)| *name == "helper" && result.is_err()));
}

#[tokio::test]
async fn disconnect_and_shutdown_cancel_queued_helper_opens() {
    for shutdown in [false, true] {
        let mut fixture = Fixture::new().await;
        let mut operations = JoinSet::new();
        let manager = fixture.manager.clone();
        let session = fixture.session.clone();
        operations.spawn(async move {
            manager
                .open_host_helper(
                    &session,
                    "/owned/helper",
                    crate::host_service::HostLane::Control,
                )
                .await
                .map(|_| ())
        });
        let deferred = fixture.next_open().await;
        let manager = fixture.manager.clone();
        let session = fixture.session.clone();
        let (queued_tx, queued_rx) = oneshot::channel();
        operations.spawn(async move {
            let mut queued = Box::pin(manager.open_host_helper(
                &session,
                "/owned/helper",
                crate::host_service::HostLane::Stream,
            ));
            std::future::poll_fn(|cx| {
                assert!(queued.as_mut().poll(cx).is_pending());
                std::task::Poll::Ready(())
            })
            .await;
            let _ = queued_tx.send(());
            queued.await.map(|_| ())
        });
        queued_rx.await.unwrap();
        let verdict = tokio::time::timeout(Duration::from_secs(1), async {
            if shutdown {
                fixture.manager.kill_all();
            } else {
                fixture.manager.disconnect(&fixture.session).await.unwrap();
            }
            let mut errors = Vec::new();
            while let Some(result) = operations.join_next().await {
                errors.push(result.unwrap().is_err());
            }
            errors
        })
        .await;
        operations.abort_all();
        while operations.join_next().await.is_some() {}
        fixture.manager.disconnect(&fixture.session).await.unwrap();
        fixture.finish().await;
        drop(deferred);
        assert_eq!(verdict.unwrap(), vec![true, true], "shutdown={shutdown}");
    }
}

#[tokio::test]
async fn late_helper_confirmation_closes_without_sending_exec() {
    let mut fixture = Fixture::new().await;
    let transport = fixture.manager.get_handle(&fixture.session).unwrap();
    let mut operations = JoinSet::new();
    let manager = fixture.manager.clone();
    let session = fixture.session.clone();
    operations.spawn(async move {
        manager
            .open_host_helper(
                &session,
                "/owned/helper",
                crate::host_service::HostLane::Control,
            )
            .await
            .map(|_| ())
    });
    let mut deferred = fixture.next_open().await;
    fixture.manager.kill_all();
    let cancelled = tokio::time::timeout(Duration::from_secs(1), operations.join_next()).await;
    operations.abort_all();
    while operations.join_next().await.is_some() {}
    deferred.reply.accept().await;
    let closed = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            match deferred.channel.wait().await {
                None | Some(russh::ChannelMsg::Close) => break,
                _ => {}
            }
        }
    })
    .await;
    let no_exec = fixture.executed.try_recv().is_err();
    transport
        .lock()
        .await
        .disconnect(Disconnect::ByApplication, "", "")
        .await
        .unwrap();
    // The client initiated SSH disconnect. Await it instead of concurrently
    // asking the peer to write another disconnect onto a closing socket.
    fixture.finish().await;
    assert!(cancelled.unwrap().unwrap().unwrap().is_err());
    assert!(closed.is_ok(), "late helper channel must be closed");
    assert!(no_exec, "cancelled helper must not execute later");
}

#[tokio::test]
async fn helper_cancellation_preserves_other_session_data_and_all_lane_arguments() {
    use crate::host_service::HostLane;
    let mut blocked = Fixture::new().await;
    let mut healthy = Fixture::with_manager(Some(blocked.manager.clone())).await;
    let manager = blocked.manager.clone();
    let session = blocked.session.clone();
    let mut operations = JoinSet::new();
    operations.spawn(async move {
        manager
            .open_host_helper(&session, "/owned/helper", HostLane::Control)
            .await
    });
    let deferred = blocked.next_open().await;
    let cancelled = tokio::time::timeout(Duration::from_secs(1), async {
        blocked.manager.disconnect(&blocked.session).await.unwrap();
        operations.join_next().await.unwrap().unwrap().is_err()
    })
    .await;
    operations.abort_all();
    while operations.join_next().await.is_some() {}
    for (lane, argument) in [
        (HostLane::Control, "--stdio"),
        (HostLane::Stream, "--stream"),
        (HostLane::Tcp, "--tcp"),
        (HostLane::Database, "--database"),
    ] {
        let manager = healthy.manager.clone();
        let session = healthy.session.clone();
        operations.spawn(async move {
            manager
                .open_host_helper(&session, "/owned/helper space", lane)
                .await
        });
        let ready = healthy.next_open().await;
        ready.reply.accept().await;
        let command = tokio::time::timeout(Duration::from_secs(1), healthy.executed.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            command,
            format!("exec '/owned/helper space' {argument}").as_bytes()
        );
        let mut peer = ready.channel.into_stream();
        let mut stream = tokio::time::timeout(Duration::from_secs(1), operations.join_next())
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            stream.write_all(b"ping").await.unwrap();
            let mut input = [0; 4];
            peer.read_exact(&mut input).await.unwrap();
            assert_eq!(&input, b"ping");
            peer.write_all(b"pong").await.unwrap();
            stream.read_exact(&mut input).await.unwrap();
            assert_eq!(&input, b"pong");
        })
        .await
        .expect("healthy helper stream must remain bidirectional");
    }
    healthy.manager.disconnect(&healthy.session).await.unwrap();
    blocked.finish().await;
    drop(deferred);
    healthy.finish().await;
    assert!(cancelled.unwrap());
}

async fn open(manager: &SshManager, session: &str, raw: bool) -> Result<(), String> {
    if raw {
        manager.open_sftp_raw(session).await.map(|_| ())
    } else {
        manager.ensure_sftp(session).await.map(|_| ())
    }
}

#[tokio::test]
async fn disconnect_and_shutdown_cancel_raw_and_queued_sftp_opens() {
    for shutdown in [false, true] {
        for raw in [false, true] {
            let mut fixture = Fixture::new().await;
            let mut operations = JoinSet::new();
            let manager = fixture.manager.clone();
            let session = fixture.session.clone();
            operations.spawn(async move { open(&manager, &session, raw).await });
            let deferred = fixture.next_open().await;
            let manager = fixture.manager.clone();
            let session = fixture.session.clone();
            let (queued_tx, queued_rx) = oneshot::channel();
            operations.spawn(async move {
                let mut queued = Box::pin(open(&manager, &session, !raw));
                std::future::poll_fn(|cx| {
                    assert!(queued.as_mut().poll(cx).is_pending());
                    std::task::Poll::Ready(())
                })
                .await;
                let _ = queued_tx.send(());
                queued.await
            });
            queued_rx.await.unwrap();
            let result = tokio::time::timeout(Duration::from_secs(1), async {
                if shutdown {
                    fixture.manager.kill_all();
                } else {
                    fixture.manager.disconnect(&fixture.session).await.unwrap();
                }
                let mut errors = Vec::new();
                while let Some(result) = operations.join_next().await {
                    errors.push(result.unwrap().is_err());
                }
                errors
            })
            .await;
            operations.abort_all();
            while operations.join_next().await.is_some() {}
            fixture.manager.disconnect(&fixture.session).await.unwrap();
            let cache_empty = fixture.manager.sessions.lock().unwrap().is_empty();
            fixture.finish().await;
            drop(deferred);
            assert_eq!(
                result.unwrap(),
                vec![true, true],
                "shutdown={shutdown}, raw={raw}"
            );
            assert!(cache_empty);
        }
    }
}

#[tokio::test]
async fn shutdown_cleans_late_confirmation_without_poisoning_transport() {
    let mut fixture = Fixture::new().await;
    // Retain the transport only to observe exact late-channel cleanup after
    // session shutdown. Production callers must not reuse this closed session.
    let transport = fixture.manager.get_handle(&fixture.session).unwrap();
    let mut operations = JoinSet::new();
    let manager = fixture.manager.clone();
    let session = fixture.session.clone();
    operations.spawn(async move { manager.ensure_sftp(&session).await.map(|_| ()) });
    let mut deferred = fixture.next_open().await;
    fixture.manager.kill_all();
    let cancelled = tokio::time::timeout(Duration::from_secs(1), operations.join_next()).await;
    operations.abort_all();
    while operations.join_next().await.is_some() {}
    deferred.reply.accept().await;
    let closed = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            match deferred.channel.wait().await {
                None | Some(russh::ChannelMsg::Close) => break,
                _ => {}
            }
        }
    })
    .await;
    // A subsequent channel on the same transport can still be confirmed.
    let transport_clone = transport.clone();
    operations.spawn(async move {
        transport_clone
            .lock()
            .await
            .channel_open_session()
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    });
    let next = fixture.next_open().await;
    next.reply.accept().await;
    let next_result = tokio::time::timeout(Duration::from_secs(1), operations.join_next()).await;
    operations.abort_all();
    while operations.join_next().await.is_some() {}
    transport
        .lock()
        .await
        .disconnect(Disconnect::ByApplication, "", "")
        .await
        .unwrap();
    fixture.finish().await;
    assert!(cancelled.unwrap().unwrap().unwrap().is_err());
    assert!(
        closed.is_ok(),
        "late-confirmed channel must close at the server"
    );
    assert!(next_result.unwrap().unwrap().unwrap().is_ok());
}

#[tokio::test]
async fn disconnect_during_sftp_handshake_does_not_cache_or_close_other_session() {
    let mut fixture = Fixture::new().await;
    let mut healthy = Fixture::with_manager(Some(fixture.manager.clone())).await;
    let mut operations = JoinSet::new();
    let manager = healthy.manager.clone();
    let session = healthy.session.clone();
    operations.spawn(async move { manager.ensure_sftp(&session).await });
    let ready = healthy.next_open().await;
    ready.reply.accept().await;
    let mut healthy_stream = ready.channel.into_stream();
    let mut init = [0; 9];
    tokio::time::timeout(Duration::from_secs(1), healthy_stream.read_exact(&mut init))
        .await
        .expect("SFTP INIT must reach the owned peer")
        .unwrap();
    assert_eq!(init, [0, 0, 0, 5, 1, 0, 0, 0, 3]);
    healthy_stream
        .write_all(&[0, 0, 0, 5, 2, 0, 0, 0, 3])
        .await
        .unwrap();
    let healthy_sftp = tokio::time::timeout(Duration::from_secs(1), operations.join_next())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
        .unwrap();

    let manager = fixture.manager.clone();
    let session = fixture.session.clone();
    operations.spawn(async move { manager.ensure_sftp(&session).await });
    let stalled = fixture.next_open().await;
    stalled.reply.accept().await;
    let mut stalled_stream = stalled.channel.into_stream();
    tokio::time::timeout(Duration::from_secs(1), stalled_stream.read_exact(&mut init))
        .await
        .expect("SFTP INIT must reach the owned peer")
        .unwrap();
    // Withhold SFTP VERSION after the SSH channel was confirmed.
    let result = tokio::time::timeout(Duration::from_secs(1), async {
        fixture.manager.disconnect(&fixture.session).await.unwrap();
        operations.join_next().await.unwrap().unwrap().is_err()
    })
    .await;
    operations.abort_all();
    while operations.join_next().await.is_some() {}
    let absent = !fixture
        .manager
        .sessions
        .lock()
        .unwrap()
        .contains_key(&fixture.session);
    let cached = healthy.manager.ensure_sftp(&healthy.session).await.unwrap();
    let same_cache = Arc::ptr_eq(&cached, &healthy_sftp);
    // Exercise the surviving SFTP channel, rather than only checking its Arc.
    let mut checks = JoinSet::new();
    checks.spawn(async move { cached.canonicalize(".").await });
    let length = tokio::time::timeout(Duration::from_secs(1), healthy_stream.read_u32())
        .await
        .expect("surviving session must send request header")
        .unwrap();
    assert!(length < 128);
    let mut request = vec![0; length as usize];
    tokio::time::timeout(
        Duration::from_secs(1),
        healthy_stream.read_exact(&mut request),
    )
    .await
    .expect("surviving session must send request body")
    .unwrap();
    assert_eq!(request[0], 16); // REALPATH
    let mut response = vec![101]; // STATUS
    response.extend_from_slice(&request[1..5]);
    response.extend_from_slice(&2_u32.to_be_bytes()); // NoSuchFile
    response.extend_from_slice(&[0; 8]); // Empty message and language.
    healthy_stream
        .write_u32(response.len() as u32)
        .await
        .unwrap();
    healthy_stream.write_all(&response).await.unwrap();
    let healthy_error = tokio::time::timeout(Duration::from_secs(1), checks.join_next())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
        .unwrap_err();
    healthy.manager.disconnect(&healthy.session).await.unwrap();
    fixture.finish().await;
    healthy.finish().await;
    assert!(result.unwrap(), "in-flight SFTP init must be cancelled");
    assert!(absent && same_cache);
    assert!(
        matches!(healthy_error, russh_sftp::client::error::Error::Status(status)
        if status.status_code == russh_sftp::protocol::StatusCode::NoSuchFile)
    );
}

type SftpStream = russh::ChannelStream<server::Msg>;

async fn read_packet(stream: &mut SftpStream) -> Vec<u8> {
    tokio::time::timeout(Duration::from_secs(1), async {
        let size = stream.read_u32().await.unwrap();
        assert!(size <= 1024, "fixture packet must stay bounded");
        let mut packet = vec![0; size as usize];
        stream.read_exact(&mut packet).await.unwrap();
        packet
    })
    .await
    .expect("SFTP packet must reach the owned peer")
}

async fn write_packet(stream: &mut SftpStream, packet: &[u8]) {
    tokio::time::timeout(Duration::from_secs(1), async {
        stream.write_u32(packet.len() as u32).await.unwrap();
        stream.write_all(packet).await.unwrap();
    })
    .await
    .expect("owned SFTP peer must send its response");
}

async fn accept_sftp(deferred: DeferredOpen) -> (russh::ChannelId, SftpStream) {
    let id = deferred.channel.id();
    deferred.reply.accept().await;
    let mut stream = deferred.channel.into_stream();
    assert_eq!(read_packet(&mut stream).await, [1, 0, 0, 0, 3]);
    write_packet(&mut stream, &[2, 0, 0, 0, 3]).await;
    (id, stream)
}

async fn warm_sftp(fixture: &mut Fixture) -> (Arc<SftpSession>, russh::ChannelId, SftpStream) {
    let mut jobs = JoinSet::new();
    let manager = fixture.manager.clone();
    let session = fixture.session.clone();
    jobs.spawn(async move { manager.ensure_sftp(&session).await });
    let (id, stream) = accept_sftp(fixture.next_open().await).await;
    let cached = tokio::time::timeout(Duration::from_secs(1), jobs.join_next())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
        .unwrap();
    (cached, id, stream)
}

fn response(kind: u8, request: &[u8], body: &[u8]) -> Vec<u8> {
    let mut packet = vec![kind];
    packet.extend_from_slice(&request[1..5]);
    packet.extend_from_slice(body);
    packet
}

async fn empty_directory(stream: &mut SftpStream) {
    let realpath = read_packet(stream).await;
    assert_eq!(realpath[0], 16);
    // NAME: one filename/longname pair and empty attributes.
    let mut body = 1_u32.to_be_bytes().to_vec();
    for _ in 0..2 {
        body.extend_from_slice(&6_u32.to_be_bytes());
        body.extend_from_slice(b"/owned");
    }
    body.extend_from_slice(&0_u32.to_be_bytes());
    write_packet(stream, &response(104, &realpath, &body)).await;
    let opendir = read_packet(stream).await;
    assert_eq!(opendir[0], 11);
    write_packet(stream, &response(102, &opendir, &[0, 0, 0, 1, b'h'])).await;
    let readdir = read_packet(stream).await;
    assert_eq!(readdir[0], 12);
    let mut status = [0; 12];
    status[3] = 1; // EOF
    write_packet(stream, &response(101, &readdir, &status)).await;
    let close = read_packet(stream).await;
    assert_eq!(close[0], 4);
    write_packet(stream, &response(101, &close, &[0; 12])).await;
}

#[tokio::test]
async fn explicit_refresh_recovers_dead_sftp_cache_without_replaying_failed_write() {
    let mut fixture = Fixture::new().await;
    let (_cached, id, mut stream) = warm_sftp(&mut fixture).await;
    let mut writes = JoinSet::new();
    let manager = fixture.manager.clone();
    let session = fixture.session.clone();
    writes.spawn(async move { manager.sftp_mkdir(&session, "/owned/new").await });
    let request = read_packet(&mut stream).await;
    assert_eq!(request[0], 14); // MKDIR received, but completion is unknown.
    fixture.control.eof(id).await.unwrap();
    fixture.control.close(id).await.unwrap();
    drop(stream);
    let write_result = tokio::time::timeout(Duration::from_secs(1), writes.join_next()).await;
    writes.abort_all();
    while writes.join_next().await.is_some() {}
    // No fresh channel may be opened on behalf of the failed write.
    let no_replay = matches!(
        fixture.pending.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    );

    let mut reads = JoinSet::new();
    let manager = fixture.manager.clone();
    let session = fixture.session.clone();
    reads.spawn(async move { manager.sftp_list_dir(&session, ".").await });
    let refreshed = tokio::time::timeout(Duration::from_secs(1), async {
        tokio::select! {
            // Red: old implementation immediately returns the dead-cache error.
            result = reads.join_next() => (false, result.unwrap().unwrap()),
            pending = fixture.pending.recv() => {
                let (_, mut stream) = accept_sftp(pending.unwrap()).await;
                empty_directory(&mut stream).await;
                let result = reads.join_next().await.unwrap().unwrap();
                (true, result)
            }
        }
    })
    .await;
    reads.abort_all();
    while reads.join_next().await.is_some() {}
    fixture.manager.disconnect(&fixture.session).await.unwrap();
    fixture.finish().await;

    assert!(write_result
        .unwrap()
        .unwrap()
        .unwrap()
        .unwrap_err()
        .starts_with("無法建立遠端資料夾："));
    assert!(
        no_replay,
        "a write with unknown completion must never be replayed"
    );
    let (reopened, listing) = refreshed.unwrap();
    assert!(
        reopened,
        "explicit refresh must reopen the closed SFTP subsystem"
    );
    let listing = listing.unwrap();
    assert_eq!(listing.cwd, "/owned");
    assert!(listing.entries.is_empty());
}

#[tokio::test]
async fn ordinary_errors_and_timeouts_keep_the_live_sftp_cache() {
    for timeout in [false, true] {
        let mut fixture = Fixture::new().await;
        let (cached, _, mut stream) = warm_sftp(&mut fixture).await;
        if timeout {
            cached.set_timeout(0);
        }
        let mut writes = JoinSet::new();
        let manager = fixture.manager.clone();
        let session = fixture.session.clone();
        writes.spawn(async move { manager.sftp_mkdir(&session, "/owned/new").await });
        let mkdir = read_packet(&mut stream).await;
        assert_eq!(mkdir[0], 14);
        let mut denied = [0; 12];
        denied[3] = 3; // PermissionDenied, an application error on a live session.
        if !timeout {
            write_packet(&mut stream, &response(101, &mkdir, &denied)).await;
        }
        let failed = tokio::time::timeout(Duration::from_secs(1), writes.join_next())
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .unwrap_err();
        if timeout {
            // A late response to a timed-out request must not close the session.
            write_packet(&mut stream, &response(101, &mkdir, &denied)).await;
            cached.set_timeout(10);
        }
        let selected = tokio::time::timeout(
            Duration::from_secs(1),
            fixture.manager.ensure_sftp(&fixture.session),
        )
        .await;
        let same =
            selected.is_ok_and(|result| result.is_ok_and(|value| Arc::ptr_eq(&cached, &value)));
        let live = !cached.is_closed();
        let no_reopen = matches!(
            fixture.pending.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        );
        let mut reads = JoinSet::new();
        let manager = fixture.manager.clone();
        let session = fixture.session.clone();
        reads.spawn(async move { manager.sftp_list_dir(&session, ".").await });
        empty_directory(&mut stream).await;
        let listing = tokio::time::timeout(Duration::from_secs(1), reads.join_next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        fixture.manager.disconnect(&fixture.session).await.unwrap();
        fixture.finish().await;
        assert!(same && live && no_reopen, "timeout={timeout}");
        assert_eq!(listing.unwrap().cwd, "/owned");
        if timeout {
            assert!(failed.contains("Timeout"));
        } else {
            assert!(failed.contains("Permission denied"));
        }
    }
}

#[tokio::test]
async fn concurrent_recovery_keeps_the_live_winner_when_older_init_finishes_late() {
    let mut fixture = Fixture::new().await;
    let (old, id, old_stream) = warm_sftp(&mut fixture).await;
    fixture.control.eof(id).await.unwrap();
    fixture.control.close(id).await.unwrap();
    drop(old_stream);
    assert!(
        tokio::time::timeout(Duration::from_secs(1), old.canonicalize("."))
            .await
            .unwrap()
            .is_err()
    );

    let mut jobs = JoinSet::new();
    for _ in 0..2 {
        let manager = fixture.manager.clone();
        let session = fixture.session.clone();
        jobs.spawn(async move { manager.ensure_sftp(&session).await });
    }
    let older = fixture.next_open().await;
    older.reply.accept().await;
    let mut older_stream = older.channel.into_stream();
    assert_eq!(read_packet(&mut older_stream).await, [1, 0, 0, 0, 3]);
    // The second initializer wins while the first is still waiting for VERSION.
    let (_, mut winner_stream) = accept_sftp(fixture.next_open().await).await;
    let winner = tokio::time::timeout(Duration::from_secs(1), jobs.join_next())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
        .unwrap();
    write_packet(&mut older_stream, &[2, 0, 0, 0, 3]).await;
    let late = tokio::time::timeout(Duration::from_secs(1), jobs.join_next())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
        .unwrap();
    let same = Arc::ptr_eq(&late, &winner) && !Arc::ptr_eq(&old, &winner);
    let loser_closed = tokio::time::timeout(Duration::from_secs(1), older_stream.read_u8()).await;
    let mut reads = JoinSet::new();
    let manager = fixture.manager.clone();
    let session = fixture.session.clone();
    reads.spawn(async move { manager.sftp_list_dir(&session, ".").await });
    empty_directory(&mut winner_stream).await;
    let listing = tokio::time::timeout(Duration::from_secs(1), reads.join_next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let no_extra = matches!(
        fixture.pending.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    );
    fixture.manager.disconnect(&fixture.session).await.unwrap();
    fixture.finish().await;
    assert!(same && no_extra);
    assert!(
        loser_closed.unwrap().is_err(),
        "losing SFTP channel must close"
    );
    assert_eq!(listing.unwrap().cwd, "/owned");
}
