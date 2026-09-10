//! Real OpenSSH SFTP processes over a loopback russh transport. All files,
//! host-key pins and credentials belong to this disposable fixture.
use super::*;
use russh::server::{self, Auth, Server};
use std::os::unix::fs::PermissionsExt;
use std::process::Stdio;

struct FixtureServer {
    password: String,
    executable: String,
    helper: String,
    helper_home: PathBuf,
    channels: HashMap<russh::ChannelId, russh::Channel<server::Msg>>,
}
impl server::Server for FixtureServer {
    type Handler = Self;
    fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Self {
        Self {
            password: self.password.clone(),
            executable: self.executable.clone(),
            helper: self.helper.clone(),
            helper_home: self.helper_home.clone(),
            channels: HashMap::new(),
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
    async fn channel_open_session(
        &mut self,
        channel: russh::Channel<server::Msg>,
        reply: server::ChannelOpenHandle,
        _: &mut server::Session,
    ) -> Result<(), Self::Error> {
        self.channels.insert(channel.id(), channel);
        reply.accept().await;
        Ok(())
    }
    async fn subsystem_request(
        &mut self,
        id: russh::ChannelId,
        name: &str,
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        if name != "sftp" {
            session.channel_failure(id)?;
            return Ok(());
        }
        self.spawn_process(
            id,
            &mut tokio::process::Command::new(&self.executable),
            session,
        )
    }
    async fn exec_request(
        &mut self,
        id: russh::ChannelId,
        data: &[u8],
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        let quoted = crate::host_service::shell_quote(&self.helper).unwrap();
        let Some(mode) = ["--stdio", "--stream", "--tcp", "--database"]
            .into_iter()
            .find(|mode| data == format!("exec {quoted} {mode}").as_bytes())
        else {
            session.channel_failure(id)?;
            return Ok(());
        };
        let mut command = tokio::process::Command::new(&self.helper);
        command.arg(mode).env("HOME", &self.helper_home);
        self.spawn_process(id, &mut command, session)
    }
}
impl FixtureServer {
    fn spawn_process(
        &mut self,
        id: russh::ChannelId,
        command: &mut tokio::process::Command,
        session: &mut server::Session,
    ) -> Result<(), russh::Error> {
        let Some(channel) = self.channels.remove(&id) else {
            session.channel_failure(id)?;
            return Ok(());
        };
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()?;
        let mut input = child.stdin.take().unwrap();
        let mut output = child.stdout.take().unwrap();
        session.channel_success(id)?;
        tokio::spawn(async move {
            let (mut read, mut write) = tokio::io::split(channel.into_stream());
            let inbound = async {
                tokio::io::copy(&mut read, &mut input).await?;
                input.shutdown().await
            };
            let outbound = async {
                tokio::io::copy(&mut output, &mut write).await?;
                write.shutdown().await
            };
            let _ = tokio::try_join!(inbound, outbound);
            let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
        });
        Ok(())
    }
}

#[tokio::test]
#[ignore = "requires YUZORA_SFTP_SERVER and freshly built YUZORA_HOST_TEST_BINARY"]
async fn real_sftp_overwrite_conflict_cancel_download_and_disconnect() {
    let executable = std::env::var("YUZORA_SFTP_SERVER").expect("OpenSSH SFTP server binary");
    let helper = std::env::var("YUZORA_HOST_TEST_BINARY").expect("fresh host helper");
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let local = root.join("local");
    let remote = root.join("中文 remote");
    std::fs::create_dir_all(&local).unwrap();
    std::fs::create_dir_all(&remote).unwrap();
    let source = local.join("file.bin");
    let target = remote.join("file.bin");
    let bytes = vec![b'x'; 2 * 1024 * 1024];
    std::fs::write(&source, &bytes).unwrap();
    std::fs::write(&target, b"original").unwrap();
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o750)).unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let key = russh::keys::PrivateKey::from_openssh(tests::TEST_HOST_KEY).unwrap();
    let fingerprint = fingerprint_sha256(key.public_key());
    let config = Arc::new(server::Config {
        keys: vec![key],
        ..Default::default()
    });
    let password = uuid::Uuid::new_v4().to_string();
    let mut fixture_server = FixtureServer {
        password: password.clone(),
        executable,
        helper: helper.clone(),
        helper_home: root.clone(),
        channels: HashMap::new(),
    };
    let server = tokio::spawn(async move { fixture_server.run_on_socket(config, &listener).await });
    let manager = SshManager::with_parts(
        Box::new(|_| {}),
        root.join("known-hosts.json"),
        Arc::new(|_| {}),
        Duration::from_secs(2),
        Arc::new(StdHostKeyIo),
    );
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
    let session = &connected.session_id;
    let selected = path_capability::SelectedPathRegistry::default();
    let workspaces = path_capability::WorkspacePathRegistry::default();
    let trust = crate::workspace_trust::WorkspaceTrustState::at(root.join("trust.json"));
    let source_grant = || SftpUploadSource::Selected {
        capability_id: selected.grant(source.to_str().unwrap()).unwrap(),
    };
    let upload_request = |id: &str, expected: Option<&str>| SftpUploadRequest {
        transfer_id: id.into(),
        source: source_grant(),
        remote_dir: remote.to_str().unwrap().into(),
        expected_revision: expected.map(str::to_owned),
    };
    let sftp = manager.ensure_sftp(session).await.unwrap();
    let expected = crate::sftp_edit::remote_revision(&sftp, target.to_str().unwrap())
        .await
        .unwrap()
        .unwrap();

    // Without an exact expected revision, an existing target is never removed.
    let id = manager.transfers.reserve(session).unwrap();
    assert_eq!(
        manager
            .sftp_upload(
                &|_, _, _| {},
                &selected,
                &workspaces,
                &trust,
                session,
                upload_request(&id, None),
            )
            .await
            .unwrap_err(),
        "file-conflict"
    );
    assert_eq!(std::fs::read(&target).unwrap(), b"original");

    // Replace only after a complete transfer. The result is byte exact.
    let id = manager.transfers.reserve(session).unwrap();
    manager
        .sftp_upload(
            &|_, _, _| {},
            &selected,
            &workspaces,
            &trust,
            session,
            upload_request(&id, Some(&expected)),
        )
        .await
        .unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), bytes);
    assert_eq!(
        std::fs::metadata(&target).unwrap().permissions().mode() & 0o777,
        0o750
    );

    // A change during transfer must survive the failed promotion.
    let expected = crate::sftp_edit::remote_revision(&sftp, target.to_str().unwrap())
        .await
        .unwrap()
        .unwrap();
    let id = manager.transfers.reserve(session).unwrap();
    let changed = AtomicBool::new(false);
    let progress = |done, _, _| {
        if done > 0 && !changed.swap(true, Ordering::SeqCst) {
            std::fs::write(&target, b"external").unwrap();
        }
    };
    assert_eq!(
        manager
            .sftp_upload(
                &progress,
                &selected,
                &workspaces,
                &trust,
                session,
                upload_request(&id, Some(&expected)),
            )
            .await
            .unwrap_err(),
        "file-conflict"
    );
    assert_eq!(std::fs::read(&target).unwrap(), b"external");

    let original_source = std::fs::read(&source).unwrap();
    let expected_source_change = crate::sftp_edit::remote_revision(&sftp, target.to_str().unwrap())
        .await
        .unwrap()
        .unwrap();
    let id = manager.transfers.reserve(session).unwrap();
    let changed = AtomicBool::new(false);
    let progress = |done, _, _| {
        if done > 0 && !changed.swap(true, Ordering::SeqCst) {
            std::fs::write(&source, b"changed source").unwrap();
        }
    };
    assert_eq!(
        manager
            .sftp_upload(
                &progress,
                &selected,
                &workspaces,
                &trust,
                session,
                upload_request(&id, Some(&expected_source_change)),
            )
            .await
            .unwrap_err(),
        "sftp-source-changed"
    );
    assert_eq!(std::fs::read(&target).unwrap(), b"external");
    std::fs::write(&source, original_source).unwrap();

    let expected = crate::sftp_edit::remote_revision(&sftp, target.to_str().unwrap())
        .await
        .unwrap()
        .unwrap();
    let id = manager.transfers.reserve(session).unwrap();
    let progress = |done, _, _| {
        if done > 0 {
            manager.transfers.cancel(session, &id).unwrap();
        }
    };
    assert_eq!(
        manager
            .sftp_upload(
                &progress,
                &selected,
                &workspaces,
                &trust,
                session,
                upload_request(&id, Some(&expected)),
            )
            .await
            .unwrap_err(),
        "sftp-transfer-cancelled"
    );
    assert_eq!(std::fs::read(&target).unwrap(), b"external");
    assert_eq!(
        std::fs::read_dir(&remote).unwrap().count(),
        1,
        "cancel and conflict clean every scratch sibling"
    );

    // Cancelling a download preserves an already-existing destination too.
    std::fs::write(&target, &bytes).unwrap();
    let destination = local.join("download.bin");
    std::fs::write(&destination, b"keep-local").unwrap();
    let destinations = path_capability::DownloadDestinationRegistry::default();
    let grant = destinations.grant(&destination).unwrap();
    let id = manager.transfers.reserve(session).unwrap();
    let progress = |done, _, _| {
        if done > 0 {
            manager.transfers.cancel(session, &id).unwrap();
        }
    };
    assert_eq!(
        manager
            .sftp_download(
                &progress,
                session,
                &id,
                target.to_str().unwrap(),
                &grant.id,
                &destinations
            )
            .await
            .unwrap_err(),
        "sftp-transfer-cancelled"
    );
    assert_eq!(std::fs::read(&destination).unwrap(), b"keep-local");
    assert_eq!(std::fs::read_dir(&local).unwrap().count(), 2);
    let grant = destinations.grant(&destination).unwrap();
    let id = manager.transfers.reserve(session).unwrap();
    manager
        .sftp_download(
            &|_, _, _| {},
            session,
            &id,
            target.to_str().unwrap(),
            &grant.id,
            &destinations,
        )
        .await
        .unwrap();
    assert_eq!(std::fs::read(&destination).unwrap(), bytes);
    verify_ssh_sqlite(&manager, session, &helper, &root, &remote).await;
    crate::sftp_tree::verify_tree_transfers(&manager, session, &root).await;
    let pending = manager.transfers.reserve(session).unwrap();
    manager.disconnect(session).await.unwrap();
    assert!(manager.transfers.start(session, &pending).is_err());
    server.abort();
}

async fn verify_ssh_sqlite(
    ssh: &SshManager,
    session: &str,
    helper: &str,
    home: &Path,
    root: &Path,
) {
    use crate::host_service::{HostManager, HostTarget};
    use sha2::{Digest, Sha256};
    use yuzora_host::db_remote::{SqliteCommand, SqliteResult, SqliteWorkspace};
    use yuzora_host::db_service::*;

    let host_id = "ssh-sqlite-fixture";
    let namespace: String = Sha256::digest(host_id.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let trust = yuzora_host::workspace_trust::WorkspaceTrustState::at(
        home.join(".yuzora/hosts")
            .join(namespace)
            .join("workspace-trust.json"),
    );
    let challenge = trust
        .0
        .issue_workspace_challenge(root.to_str().unwrap())
        .unwrap();
    trust.0.grant(&challenge.challenge_id).unwrap();
    let database = root.join("fixture.sqlite");
    rusqlite::Connection::open(&database)
        .unwrap()
        .execute_batch("CREATE TABLE sample(value INTEGER); INSERT INTO sample VALUES (42);")
        .unwrap();
    let hosts = HostManager::default();
    let connected = hosts
        .connect(
            host_id.into(),
            HostTarget::Ssh {
                session_id: session.into(),
            },
            helper.into(),
            ssh,
        )
        .await
        .unwrap();
    let connection = hosts.connection(&connected.owner).unwrap();
    let identity = ConnectionIdentity {
        descriptor_id: DescriptorId("ssh-profile".into()),
        connection_id: ConnectionId("ssh-database".into()),
        connection_generation: ConnectionGeneration("first".into()),
    };
    let workspace = SqliteWorkspace {
        host_id: host_id.into(),
        canonical_path: root.to_str().unwrap().into(),
    };
    let open = || {
        crate::host_sqlite::open(
            &hosts,
            ssh,
            workspace.clone(),
            database.to_str().unwrap().into(),
            identity.clone(),
        )
    };
    let DbHandle::RemoteSqlite(proxy) = open().await.unwrap() else {
        panic!("remote proxy")
    };
    assert_eq!(connection.database_slots.available_permits(), 7);
    assert!(matches!(
        proxy.request(SqliteCommand::Probe).await.unwrap(),
        SqliteResult::Version(_)
    ));
    let SqliteResult::Run(run) = proxy
        .request(SqliteCommand::QueryRun(QueryRunRequest {
            descriptor_id: identity.descriptor_id.clone(),
            connection_id: identity.connection_id.clone(),
            connection_generation: identity.connection_generation.clone(),
            query_run_id: QueryRunId("ssh-read".into()),
            mode: QueryRunMode::Primary,
            statements: NonEmptyVec::try_from(vec![QueryExecutionUnit {
                sql: "SELECT value FROM sample".into(),
                transaction_boundary: TransactionBoundary::None,
            }])
            .unwrap(),
        }))
        .await
        .unwrap()
    else {
        panic!("query result")
    };
    let StatementExecutionResult::Rows {
        result_session: Some(result),
        ..
    } = &run.statements[0].result
    else {
        panic!("source rows")
    };
    assert_eq!(result.initial_page.rows.len(), 1);
    proxy.abort();
    wait_database_slots(&connection).await;
    assert!(
        ssh.ensure_sftp(session)
            .await
            .unwrap()
            .metadata(database.to_str().unwrap())
            .await
            .is_ok(),
        "closing SQLite must retain SFTP"
    );

    let DbHandle::RemoteSqlite(proxy) = open().await.unwrap() else {
        panic!("replacement proxy")
    };
    hosts.disconnect(&connected.owner).await.unwrap();
    assert_eq!(
        proxy.request(SqliteCommand::Probe).await.unwrap_err().code,
        DatabaseOperationalErrorCode::ServerDisconnected
    );
    wait_database_slots(&connection).await;
    assert!(
        ssh.ensure_sftp(session)
            .await
            .unwrap()
            .metadata(database.to_str().unwrap())
            .await
            .is_ok(),
        "closing a host provider must retain the shared SSH transport"
    );
}

async fn wait_database_slots(connection: &crate::host_service::HostConnection) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while connection.database_slots.available_permits() != 8 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}
