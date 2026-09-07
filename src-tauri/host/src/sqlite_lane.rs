//! One app-owned SQLite connection per dedicated stdio lane.
use crate::db_connection_actor::ProductionConnectionActor;
use crate::db_remote::*;
use crate::db_result_session::ResultSessionState;
use crate::db_service::*;
use crate::path_capability::PinnedDir;
use crate::protocol::PROTOCOL_VERSION;
use crate::wire::read_frame;
use std::future::Future;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

struct Database {
    state: DbState,
    sessions: ResultSessionState,
    config: SqliteOpen,
    root: PinnedDir,
    file_id: (u64, u64),
}
impl Drop for Database {
    fn drop(&mut self) {
        self.state.stop_accepting();
        if let Ok(actors) = self.state.0.lock() {
            for actor in actors.values() {
                let _ = actor.request_lifecycle_teardown();
            }
        }
    }
}
impl Database {
    async fn open(config: SqliteOpen) -> Result<Self, DatabaseOperationalError> {
        let invalid = || {
            DatabaseOperationalError::new(
                DatabaseOperationalErrorCode::SqlitePathInvalid,
                "SQLite requires an existing file in the trusted source workspace",
            )
        };
        if config.version != PROTOCOL_VERSION
            || config.owner.host_id.is_empty()
            || config.identity.descriptor_id.0.is_empty()
            || config.identity.connection_id.0.is_empty()
            || config.identity.connection_generation.0.is_empty()
        {
            return Err(invalid());
        }
        let trust =
            crate::trust_command::host_trust(&config.owner.host_id).map_err(|_| invalid())?;
        let identity = trust
            .require_trusted(&config.workspace_path)
            .map_err(|_| invalid())?;
        if identity.canonical_path != config.workspace_path {
            return Err(invalid());
        }
        let canonical = validate_existing_sqlite_path(&config.database_path)?;
        if !canonical.starts_with(&config.workspace_path)
            || canonical == Path::new(&config.workspace_path)
        {
            return Err(invalid());
        }
        let root = PinnedDir::open_dir(Path::new(&config.workspace_path)).map_err(|_| invalid())?;
        let metadata = std::fs::metadata(&canonical).map_err(|_| invalid())?;
        let mut database = Self {
            state: DbState::default(),
            sessions: ResultSessionState::default(),
            config,
            root,
            file_id: (metadata.dev(), metadata.ino()),
        };
        database.config.database_path = canonical.to_str().ok_or_else(invalid)?.into();
        let handle = open_unregistered(DbOpenConfig::Sqlite {
            workspace: None,
            path: database.config.database_path.clone(),
        })
        .await?;
        database.check_source()?;
        register_actor(
            &database.state,
            Arc::new(ProductionConnectionActor::new(
                database.config.identity.clone(),
                handle,
            )),
        )?;
        Ok(database)
    }

    fn check_source(&self) -> Result<(), DatabaseOperationalError> {
        let unchanged = || -> Result<bool, String> {
            crate::trust_command::host_trust(&self.config.owner.host_id)?
                .require_trusted(&self.config.workspace_path)?;
            let root = PinnedDir::open_dir(Path::new(&self.config.workspace_path))?;
            let file = std::fs::metadata(&self.config.database_path).map_err(|e| e.to_string())?;
            Ok(root.id_key() == self.root.id_key() && (file.dev(), file.ino()) == self.file_id)
        };
        if unchanged().unwrap_or(false) {
            Ok(())
        } else {
            Err(disconnected())
        }
    }

    async fn call(&self, call: SqliteCommand) -> Result<SqliteResult, DatabaseOperationalError> {
        // Cancellation/release must remain available after trust or path changes.
        if !matches!(call, SqliteCommand::Cancel(_) | SqliteCommand::Release(_)) {
            self.check_source()?;
        }
        match call {
            SqliteCommand::Probe => {
                sqlite_version_in_state(&self.state, self.config.identity.clone())
                    .await
                    .map(SqliteResult::Version)
            }
            SqliteCommand::ListTables { identity } => list_tables_in_state(&self.state, identity)
                .await
                .map(SqliteResult::Tables),
            SqliteCommand::TableColumns { identity, table } => {
                table_columns_in_state(&self.state, identity, table)
                    .await
                    .map(SqliteResult::Columns)
            }
            SqliteCommand::QueryRun(request) => {
                query_run_in_state(&self.state, &self.sessions, request)
                    .await
                    .map(SqliteResult::Run)
            }
            SqliteCommand::Cancel(owner) => query_cancel_in_state(&self.state, owner)
                .await
                .map(SqliteResult::Cancelled),
            SqliteCommand::Page(request) => {
                result_page_in_state(&self.state, &self.sessions, request)
                    .await
                    .map(SqliteResult::Page)
            }
            SqliteCommand::Release(owner) => {
                result_session_release_in_state(&self.state, &self.sessions, owner)
                    .await
                    .map(SqliteResult::Page)
            }
        }
    }
}

pub async fn serve<
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
>(
    input: R,
    mut output: W,
) -> Result<(), String> {
    let mut input = BufReader::new(input);
    let first = tokio::time::timeout(Duration::from_secs(15), read_frame(&mut input))
        .await
        .map_err(|_| "sqlite-open-timeout")??
        .ok_or("sqlite-open-required")?;
    let config: SqliteOpen = serde_json::from_slice(&first).map_err(|e| e.to_string())?;
    let owner = config.owner.clone();
    let database = match Database::open(config).await {
        Ok(database) => Arc::new(database),
        Err(error) => {
            let bytes = encode(&SqliteReply {
                version: PROTOCOL_VERSION,
                owner,
                id: 0,
                result: Err(error.into()),
            })?;
            tokio::time::timeout(Duration::from_secs(10), output.write_all(&bytes))
                .await
                .map_err(|_| "sqlite-write-timeout")?
                .map_err(|e| e.to_string())?;
            return Ok(());
        }
    };
    let (sender, mut replies) =
        tokio::sync::mpsc::channel::<SqliteReply>(MAX_DATABASE_REQUESTS + 1);
    let mut writer = tokio::spawn(async move {
        while let Some(reply) = replies.recv().await {
            let (bytes, terminal) = tokio::task::spawn_blocking(move || {
                match encode(&reply) {
                    Ok(bytes) => Ok((bytes, false)),
                    Err(_) => encode(&SqliteReply {
                        version: reply.version, owner: reply.owner, id: reply.id,
                        result: Err(DatabaseOperationalError::new(DatabaseOperationalErrorCode::ServerDisconnected, "SQLite reply exceeded the transport limit; connection closed. SQL may have executed and was not replayed").into()),
                    }).map(|bytes| (bytes, true)),
                }
            }).await.map_err(|e| e.to_string())??;
            tokio::time::timeout(Duration::from_secs(10), async {
                output.write_all(&bytes).await?;
                output.flush().await
            })
            .await
            .map_err(|_| "sqlite-write-timeout")?
            .map_err(|e| e.to_string())?;
            // The client cannot release a cursor whose identity did not fit on
            // the wire. End this lane so normal teardown interrupts the actor
            // and releases every source cursor, without replaying any SQL.
            if terminal {
                return Err("sqlite-response-limit".into());
            }
        }
        Ok::<_, String>(())
    });
    sender
        .try_send(SqliteReply {
            version: PROTOCOL_VERSION,
            owner: owner.clone(),
            id: 0,
            result: Ok(SqliteResult::Opened(database.config.identity.clone())),
        })
        .map_err(|_| "sqlite-output-closed")?;
    let mut jobs = tokio::task::JoinSet::new();
    let mut last_id = 0;
    let result = async {
        loop {
            let next = read_frame(&mut input);
            tokio::pin!(next);
            let bytes = loop {
                tokio::select! {
                    biased;
                    _ = &mut writer => return Err("sqlite-output-closed".into()),
                    completed = jobs.join_next(), if !jobs.is_empty() => {
                        let reply = completed.ok_or("sqlite-worker-missing")?.map_err(|e| e.to_string())?;
                        sender.try_send(reply).map_err(|_| "sqlite-output-backpressure")?;
                    }
                    frame = &mut next => break frame?,
                }
            };
            let Some(bytes) = bytes else { return Ok(()); };
            let request: SqliteRequest = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
            request.validate(&owner, last_id)?;
            last_id = request.id;
            if jobs.len() >= MAX_DATABASE_REQUESTS { return Err("sqlite-request-limit".into()); }
            let database = database.clone();
            let owner = owner.clone();
            let mut future = Box::pin(async move {
                SqliteReply { version: PROTOCOL_VERSION, owner, id: request.id, result: database.call(request.call).await.map_err(Into::into) }
            });
            // Poll once in arrival order so a subsequent Cancel cannot overtake
            // acquisition of the exact query lease.
            let started = std::future::poll_fn(|cx| std::task::Poll::Ready(future.as_mut().poll(cx))).await;
            match started {
                std::task::Poll::Ready(reply) => sender.try_send(reply).map_err(|_| "sqlite-output-backpressure")?,
                std::task::Poll::Pending => { jobs.spawn(future); }
            }
        }
    }.await;
    database.state.stop_accepting();
    let _ = shutdown_all_connections(&database.state, DatabaseShutdownTimeouts::default()).await;
    jobs.abort_all();
    writer.abort();
    result
}
