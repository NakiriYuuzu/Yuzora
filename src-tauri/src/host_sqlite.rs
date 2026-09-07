//! Owned remote SQLite lane; concurrent Cancel never waits behind a QueryRun.
use crate::host_service::{open_stream, HostConnection, HostManager, HostStream};
use crate::ssh_service::SshManager;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot, watch};
use yuzora_host::db_remote::*;
use yuzora_host::db_service::{
    ConnectionIdentity, DatabaseOperationalError, DatabaseOperationalErrorCode, DbHandle,
};
use yuzora_host::protocol::{ConnectionOwner, PROTOCOL_VERSION};

struct Call {
    command: SqliteCommand,
    reply: oneshot::Sender<Result<SqliteResult, DatabaseOperationalError>>,
}
struct Proxy {
    connection: Arc<HostConnection>,
    cancelled: watch::Sender<bool>,
    requests: mpsc::Sender<Call>,
}
impl Drop for Proxy {
    fn drop(&mut self) {
        self.abort();
    }
}
struct PendingGuard(Option<watch::Sender<bool>>);
impl Drop for PendingGuard {
    fn drop(&mut self) {
        if let Some(cancelled) = &self.0 {
            cancelled.send_replace(true);
        }
    }
}
impl RemoteSqlite for Proxy {
    fn is_closed(&self) -> bool {
        *self.cancelled.borrow() || *self.connection.cancelled.borrow()
    }
    fn abort(&self) {
        self.cancelled.send_replace(true);
    }
    fn request(&self, command: SqliteCommand) -> SqliteFuture<'_> {
        Box::pin(async move {
            if self.is_closed() {
                return Err(disconnected());
            }
            let (reply, response) = oneshot::channel();
            self.requests
                .try_send(Call { command, reply })
                .map_err(|_| {
                    DatabaseOperationalError::new(
                        DatabaseOperationalErrorCode::ConnectionBusy,
                        "remote SQLite request queue is full",
                    )
                })?;
            let mut guard = PendingGuard(Some(self.cancelled.clone()));
            let result = tokio::time::timeout(Duration::from_secs(120), response).await;
            match result {
                Ok(Ok(result)) if !self.is_closed() => {
                    guard.0 = None;
                    result
                }
                _ => Err(disconnected()),
            }
        })
    }
}

async fn send<W: AsyncWrite + Unpin, T: serde::Serialize>(
    output: &mut W,
    request: &T,
) -> Result<(), String> {
    let bytes = encode(request)?;
    tokio::time::timeout(Duration::from_secs(10), async {
        output.write_all(&bytes).await?;
        output.flush().await
    })
    .await
    .map_err(|_| "sqlite-write-timeout")?
    .map_err(|e| e.to_string())
}

fn parse(bytes: &[u8], owner: &ConnectionOwner) -> Result<SqliteReply, String> {
    let reply: SqliteReply = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    if reply.version != PROTOCOL_VERSION || &reply.owner != owner {
        return Err("sqlite-response-owner-mismatch".into());
    }
    Ok(reply)
}

async fn run(
    io: HostStream,
    owner: ConnectionOwner,
    mut requests: mpsc::Receiver<Call>,
    mut cancelled: watch::Receiver<bool>,
    mut host_cancelled: watch::Receiver<bool>,
) -> Result<(), String> {
    let (read, mut write) = tokio::io::split(io);
    let mut read = BufReader::new(read);
    let mut pending = HashMap::<u64, Call>::new();
    let mut next_id = 0_u64;
    let result = async {
        loop {
            if *cancelled.borrow() || *host_cancelled.borrow() { return Err("sqlite-closed".into()); }
            let next = yuzora_host::wire::read_frame(&mut read);
            tokio::pin!(next);
            let bytes = loop {
                tokio::select! {
                    biased;
                    _ = cancelled.changed() => return Err("sqlite-closed".into()),
                    _ = host_cancelled.changed() => return Err("sqlite-host-disconnected".into()),
                    frame = &mut next => break frame?.ok_or("sqlite-ended")?,
                    call = requests.recv(), if pending.len() < MAX_DATABASE_REQUESTS => {
                        let Some(call) = call else { return Ok(()); };
                        if call.reply.is_closed() { continue; }
                        next_id = next_id.checked_add(1).ok_or("sqlite-request-id-exhausted")?;
                        let request = SqliteRequest { version: PROTOCOL_VERSION, owner: owner.clone(), id: next_id, call: call.command };
                        tokio::select! {
                            biased;
                            _ = cancelled.changed() => return Err("sqlite-closed".into()),
                            _ = host_cancelled.changed() => return Err("sqlite-host-disconnected".into()),
                            result = send(&mut write, &request) => result?,
                        }
                        pending.insert(next_id, Call { command: request.call, reply: call.reply });
                    }
                }
            };
            let response = parse(&bytes, &owner)?;
            let call = pending.remove(&response.id).ok_or("sqlite-unexpected-response-id")?;
            let result = response.result.map_err(Into::into);
            if let Ok(ref result) = result {
                if !call.command.accepts(result) { return Err("sqlite-result-owner-mismatch".into()); }
            }
            let _ = call.reply.send(result);
        }
    }.await;
    let _ = tokio::time::timeout(Duration::from_secs(1), write.shutdown()).await;
    result
}

pub(crate) async fn open(
    hosts: &HostManager,
    ssh: &SshManager,
    workspace: SqliteWorkspace,
    path: String,
    identity: ConnectionIdentity,
) -> Result<DbHandle, DatabaseOperationalError> {
    let connection = hosts
        .connection_for_host(&workspace.host_id)
        .map_err(|_| disconnected())?;
    let hello = connection
        .request(yuzora_host::protocol::Operation::Hello)
        .await
        .map_err(|_| disconnected())?;
    if !hello["methods"]
        .as_array()
        .is_some_and(|methods| methods.iter().any(|method| method == "sqlite"))
    {
        return Err(DatabaseOperationalError::new(
            DatabaseOperationalErrorCode::SqliteOpenFailed,
            "source helper does not support SQLite; set up this host again",
        ));
    }
    let permit = connection
        .database_slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| {
            DatabaseOperationalError::new(
                DatabaseOperationalErrorCode::ConnectionBusy,
                "remote SQLite connection limit reached",
            )
        })?;
    let owner = connection.owner.clone();
    let config = SqliteOpen {
        version: PROTOCOL_VERSION,
        owner: owner.clone(),
        workspace_path: workspace.canonical_path,
        database_path: path,
        identity: identity.clone(),
    };
    let mut host_cancelled = connection.cancelled.subscribe();
    let operation = async {
        let mut io = open_stream(
            &connection.target,
            &connection.helper,
            crate::host_service::HostLane::Database,
            ssh,
        )
        .await
        .map_err(|_| disconnected())?;
        send(&mut io, &config).await.map_err(|_| disconnected())?;
        // Preserve any prefetched bytes when handing off to the request pump.
        let mut io = BufReader::new(io);
        let bytes = yuzora_host::wire::read_frame(&mut io)
            .await
            .map_err(|_| disconnected())?
            .ok_or_else(disconnected)?;
        let reply = parse(&bytes, &owner).map_err(|_| unexpected_reply())?;
        if reply.id != 0 {
            return Err(unexpected_reply());
        }
        match reply.result.map_err(DatabaseOperationalError::from)? {
            SqliteResult::Opened(opened) if opened == identity => {}
            _ => return Err(unexpected_reply()),
        }
        Ok::<HostStream, DatabaseOperationalError>(Box::new(io))
    };
    let io = tokio::select! {
        biased;
        _ = host_cancelled.changed() => return Err(disconnected()),
        result = tokio::time::timeout(Duration::from_secs(30), operation) => result.map_err(|_| disconnected())??,
    };
    hosts.connection(&owner).map_err(|_| disconnected())?;
    let (cancelled, receiver) = watch::channel(false);
    let (requests, queue) = mpsc::channel(MAX_DATABASE_REQUESTS);
    let finished = cancelled.clone();
    tokio::spawn(async move {
        let _permit = permit;
        let _ = run(io, owner, queue, receiver, host_cancelled).await;
        finished.send_replace(true);
    });
    Ok(DbHandle::RemoteSqlite(Arc::new(Proxy {
        connection,
        cancelled,
        requests,
    })))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::host_service::HostTarget;
    use std::os::unix::fs::PermissionsExt;
    use yuzora_host::db_connection_actor::ProductionConnectionActor;
    use yuzora_host::db_result_session::ResultSessionState;
    use yuzora_host::db_service::*;

    #[tokio::test]
    #[ignore = "requires a freshly built YUZORA_HOST_TEST_BINARY"]
    async fn real_sqlite_proxy_uses_source_actor_and_releases_slots_on_close_and_disconnect() {
        use sha2::{Digest, Sha256};
        let helper = std::env::var("YUZORA_HOST_TEST_BINARY").expect("fresh helper binary");
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("中文 project");
        std::fs::create_dir(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let file = root.join("db.sqlite");
        rusqlite::Connection::open(&file).unwrap().execute_batch("CREATE TABLE sample(x INTEGER); WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x < 1201) INSERT INTO sample SELECT x FROM n;").unwrap();
        let host_id = "sqlite-proxy-test";
        let namespace = Sha256::digest(host_id.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        let trust = yuzora_host::workspace_trust::WorkspaceTrustState::at(
            home.path()
                .join(".yuzora/hosts")
                .join(namespace)
                .join("workspace-trust.json"),
        );
        let challenge = trust
            .0
            .issue_workspace_challenge(root.to_str().unwrap())
            .unwrap();
        trust.0.grant(&challenge.challenge_id).unwrap();
        let quote = |value: &str| format!("'{}'", value.replace('\'', "'\\''"));
        let wrapper = home.path().join("host-fixture");
        std::fs::write(
            &wrapper,
            format!(
                "#!/bin/sh\nexport HOME={}\nexec {} \"$@\"\n",
                quote(home.path().to_str().unwrap()),
                quote(&helper)
            ),
        )
        .unwrap();
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
        let hosts = HostManager::default();
        let ssh = SshManager::for_test();
        let connected = hosts
            .connect(
                host_id.into(),
                HostTarget::Local,
                wrapper.to_str().unwrap().into(),
                &ssh,
            )
            .await
            .unwrap();
        let connection = hosts.connection(&connected.owner).unwrap();
        let identity = ConnectionIdentity {
            descriptor_id: DescriptorId("profile-a".into()),
            connection_id: ConnectionId("database-a".into()),
            connection_generation: ConnectionGeneration("db-generation-a".into()),
        };
        let workspace = SqliteWorkspace {
            host_id: host_id.into(),
            canonical_path: root.to_str().unwrap().into(),
        };
        let handle = open(
            &hosts,
            &ssh,
            workspace.clone(),
            file.to_str().unwrap().into(),
            identity.clone(),
        )
        .await
        .unwrap();
        let state = DbState::default();
        let sessions = ResultSessionState::default();
        register_actor(
            &state,
            Arc::new(ProductionConnectionActor::new(identity.clone(), handle)),
        )
        .unwrap();
        assert_eq!(connection.database_slots.available_permits(), 7);
        assert_eq!(
            list_tables_in_state(&state, identity.clone())
                .await
                .unwrap()
                .len(),
            1
        );
        let request = |id: &str, sql: &str| QueryRunRequest {
            descriptor_id: identity.descriptor_id.clone(),
            connection_id: identity.connection_id.clone(),
            connection_generation: identity.connection_generation.clone(),
            query_run_id: QueryRunId(id.into()),
            mode: QueryRunMode::Primary,
            statements: NonEmptyVec::try_from(vec![QueryExecutionUnit {
                sql: sql.into(),
                transaction_boundary: TransactionBoundary::None,
            }])
            .unwrap(),
        };
        let run = query_run_in_state(
            &state,
            &sessions,
            request("paged", "SELECT x FROM sample ORDER BY x"),
        )
        .await
        .unwrap();
        let StatementExecutionResult::Rows {
            result_session: Some(result),
            ..
        } = &run.statements[0].result
        else {
            panic!("expected source result session")
        };
        assert_eq!(result.initial_page.rows.len(), 500);
        let page = result_page_in_state(
            &state,
            &sessions,
            ResultPageRequest {
                owner: result.owner.clone(),
                direction: ResultPageDirection::Next,
            },
        )
        .await
        .unwrap();
        assert_eq!((page.page_index, page.rows.len()), (1, 500));
        result_session_release_in_state(&state, &sessions, result.owner.clone())
            .await
            .unwrap();
        assert_eq!(
            sessions.lock().unwrap().session_count(),
            0,
            "desktop does not own a second pager"
        );
        let slow = request("cancel", "WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x < 1000000000) SELECT sum(x) FROM n");
        let owner = QueryRunOwner {
            descriptor_id: identity.descriptor_id.clone(),
            connection_id: identity.connection_id.clone(),
            connection_generation: identity.connection_generation.clone(),
            query_run_id: slow.query_run_id.clone(),
        };
        let active_state = state.clone();
        let active_sessions = sessions.clone();
        let query =
            tokio::spawn(
                async move { query_run_in_state(&active_state, &active_sessions, slow).await },
            );
        tokio::time::sleep(Duration::from_millis(50)).await;
        query_cancel_in_state(&state, owner).await.unwrap();
        let run = tokio::time::timeout(Duration::from_secs(3), query)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(matches!(
            run.statements[0].result,
            StatementExecutionResult::Cancelled { .. }
        ));
        close_exact_in_state(&state, &identity).unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            while connection.database_slots.available_permits() != 8 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let mut replacement = identity.clone();
        replacement.connection_generation.0 = "db-generation-b".into();
        let handle = open(
            &hosts,
            &ssh,
            workspace,
            file.to_str().unwrap().into(),
            replacement.clone(),
        )
        .await
        .unwrap();
        register_actor(
            &state,
            Arc::new(ProductionConnectionActor::new(replacement.clone(), handle)),
        )
        .unwrap();
        assert_eq!(
            list_tables_in_state(&state, identity)
                .await
                .unwrap_err()
                .code,
            DatabaseOperationalErrorCode::StaleConnection
        );
        hosts.disconnect(&connected.owner).await.unwrap();
        assert!(!has_exact_actor(&state, &replacement));
        assert_eq!(
            list_tables_in_state(&state, replacement)
                .await
                .unwrap_err()
                .code,
            DatabaseOperationalErrorCode::ServerDisconnected
        );
        tokio::time::timeout(Duration::from_secs(3), async {
            while connection.database_slots.available_permits() != 8 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }
}
