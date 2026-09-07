#![cfg(unix)]
mod support;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncWriteExt, BufReader};
use yuzora_host::db_remote::*;
use yuzora_host::db_service::*;
use yuzora_host::protocol::{ConnectionOwner, PROTOCOL_VERSION};

struct Fixture {
    home: tempfile::TempDir,
    root: PathBuf,
    path: PathBuf,
}
impl Fixture {
    fn new(host: &str, trusted: bool) -> Self {
        use sha2::{Digest, Sha256};
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("中文 workspace");
        std::fs::create_dir(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let path = root.join("sample database.sqlite");
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch("CREATE TABLE sample(id INTEGER PRIMARY KEY, counter INTEGER NOT NULL); WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x < 1201) INSERT INTO sample SELECT x, 0 FROM n;").unwrap();
        drop(conn);
        if trusted {
            let namespace = Sha256::digest(host.as_bytes())
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
        }
        Self { home, root, path }
    }
    fn identity(&self) -> ConnectionIdentity {
        ConnectionIdentity {
            descriptor_id: DescriptorId("same-profile".into()),
            connection_id: ConnectionId("same-connection".into()),
            connection_generation: ConnectionGeneration("db-generation-1".into()),
        }
    }
}
struct Lane {
    child: tokio::process::Child,
    input: Option<tokio::process::ChildStdin>,
    output: BufReader<tokio::process::ChildStdout>,
    owner: ConnectionOwner,
    identity: ConnectionIdentity,
    next: u64,
}
impl Lane {
    async fn start(fixture: &Fixture, host: &str, path: &Path) -> (Self, SqliteReply) {
        let mut child = tokio::process::Command::new(support::helper_binary())
            .arg("--database")
            .env("HOME", fixture.home.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut lane = Self {
            input: child.stdin.take(),
            output: BufReader::new(child.stdout.take().unwrap()),
            child,
            owner: ConnectionOwner {
                host_id: host.into(),
                generation: 5,
            },
            identity: fixture.identity(),
            next: 0,
        };
        let config = SqliteOpen {
            version: PROTOCOL_VERSION,
            owner: lane.owner.clone(),
            workspace_path: fixture.root.to_str().unwrap().into(),
            database_path: path.to_str().unwrap().into(),
            identity: lane.identity.clone(),
        };
        lane.input
            .as_mut()
            .unwrap()
            .write_all(&encode(&config).unwrap())
            .await
            .unwrap();
        let reply = lane.read().await;
        (lane, reply)
    }
    async fn read(&mut self) -> SqliteReply {
        let bytes = tokio::time::timeout(
            Duration::from_secs(10),
            yuzora_host::wire::read_frame(&mut self.output),
        )
        .await
        .unwrap()
        .unwrap()
        .unwrap();
        let reply: SqliteReply = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(reply.version, PROTOCOL_VERSION);
        assert_eq!(reply.owner, self.owner);
        reply
    }
    async fn send(&mut self, call: SqliteCommand) -> u64 {
        self.next += 1;
        let request = SqliteRequest {
            version: PROTOCOL_VERSION,
            owner: self.owner.clone(),
            id: self.next,
            call,
        };
        self.input
            .as_mut()
            .unwrap()
            .write_all(&encode(&request).unwrap())
            .await
            .unwrap();
        self.next
    }
    async fn call(&mut self, call: SqliteCommand) -> Result<SqliteResult, SqliteError> {
        let id = self.send(call).await;
        let response = self.read().await;
        assert_eq!(response.id, id);
        response.result
    }
    fn query(&self, run: &str, sql: &str) -> QueryRunRequest {
        QueryRunRequest {
            descriptor_id: self.identity.descriptor_id.clone(),
            connection_id: self.identity.connection_id.clone(),
            connection_generation: self.identity.connection_generation.clone(),
            query_run_id: QueryRunId(run.into()),
            mode: QueryRunMode::Primary,
            statements: NonEmptyVec::try_from(vec![QueryExecutionUnit {
                sql: sql.into(),
                transaction_boundary: TransactionBoundary::None,
            }])
            .unwrap(),
        }
    }
    async fn eof(mut self, success: bool) {
        drop(self.input.take());
        let status = tokio::time::timeout(Duration::from_secs(5), self.child.wait())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(status.success(), success);
    }
}
fn session(run: &QueryRun) -> ResultSessionOwner {
    match &run.statements.iter().next().unwrap().result {
        StatementExecutionResult::Rows {
            result_session: Some(session),
            ..
        }
        | StatementExecutionResult::ResultLimitReached {
            result_session: session,
            ..
        } => session.owner.clone(),
        other => panic!("expected paged rows, got {other:?}"),
    }
}

#[tokio::test]
async fn sqlite_source_pages_previous_release_and_returning_write_execute_once() {
    let fixture = Fixture::new("sqlite-host", true);
    let (mut lane, opened) = Lane::start(&fixture, "sqlite-host", &fixture.path).await;
    assert!(matches!(opened.result, Ok(SqliteResult::Opened(_))));
    let request = lane.query(
        "returning",
        "UPDATE sample SET counter = counter + 1 RETURNING id, counter",
    );
    let SqliteResult::Run(run) = lane.call(SqliteCommand::QueryRun(request)).await.unwrap() else {
        panic!()
    };
    run.validate_cardinality().unwrap();
    let owner = session(&run);
    let busy = lane.query("busy", "SELECT 1");
    assert_eq!(
        lane.call(SqliteCommand::QueryRun(busy))
            .await
            .unwrap_err()
            .code,
        DatabaseOperationalErrorCode::ConnectionBusy
    );
    for (index, rows, more) in [(1, 500, true), (2, 201, false)] {
        let SqliteResult::Page(page) = lane
            .call(SqliteCommand::Page(ResultPageRequest {
                owner: owner.clone(),
                direction: ResultPageDirection::Next,
            }))
            .await
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(
            (page.page_index, page.rows.len(), page.has_next),
            (index, rows, more)
        );
    }
    let SqliteResult::Page(previous) = lane
        .call(SqliteCommand::Page(ResultPageRequest {
            owner: owner.clone(),
            direction: ResultPageDirection::Previous,
        }))
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!((previous.page_index, previous.rows.len()), (1, 500));
    lane.call(SqliteCommand::Release(owner)).await.unwrap();
    let conn = rusqlite::Connection::open(&fixture.path).unwrap();
    assert_eq!(
        conn.query_row("SELECT min(counter), max(counter) FROM sample", [], |r| Ok(
            (r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)
        ))
        .unwrap(),
        (1, 1)
    );
    lane.eof(true).await;
}

const SLOW: &str = "WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x < 1000000000) SELECT sum(x) FROM n";
#[tokio::test]
async fn sqlite_cancel_overtakes_running_query_and_stale_owner_cannot_touch_next_run() {
    let fixture = Fixture::new("sqlite-cancel", true);
    let (mut lane, opened) = Lane::start(&fixture, "sqlite-cancel", &fixture.path).await;
    opened.result.unwrap();
    let request = lane.query("slow", SLOW);
    let owner = QueryRunOwner {
        descriptor_id: request.descriptor_id.clone(),
        connection_id: request.connection_id.clone(),
        connection_generation: request.connection_generation.clone(),
        query_run_id: request.query_run_id.clone(),
    };
    let run_id = lane.send(SqliteCommand::QueryRun(request)).await;
    let cancel_id = lane.send(SqliteCommand::Cancel(owner)).await;
    let mut seen = std::collections::HashSet::new();
    for _ in 0..2 {
        let reply = lane.read().await;
        seen.insert(reply.id);
        match reply.result.unwrap() {
            SqliteResult::Run(run) => assert!(matches!(
                run.statements.iter().next().unwrap().result,
                StatementExecutionResult::Cancelled { .. }
            )),
            SqliteResult::Cancelled(result) => assert!(matches!(
                result.outcome,
                QueryCancelOutcome::Cancelled | QueryCancelOutcome::AlreadyRequested
            )),
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(seen, [run_id, cancel_id].into_iter().collect());
    let mut stale = lane.identity.clone();
    stale.connection_generation.0 = "old".into();
    assert_eq!(
        lane.call(SqliteCommand::ListTables { identity: stale })
            .await
            .unwrap_err()
            .code,
        DatabaseOperationalErrorCode::StaleConnection
    );
    assert!(matches!(
        lane.call(SqliteCommand::Probe).await.unwrap(),
        SqliteResult::Version(_)
    ));
    // The scoped progress callback must not cancel the next lease.
    let next = lane.query(
        "after-cancel",
        "UPDATE sample SET counter=counter+1 WHERE id=1 RETURNING counter",
    );
    let SqliteResult::Run(run) = lane.call(SqliteCommand::QueryRun(next)).await.unwrap() else {
        panic!()
    };
    let owner = session(&run);
    assert!(matches!(
        run.statements.iter().next().unwrap().result,
        StatementExecutionResult::Rows { .. }
    ));
    lane.call(SqliteCommand::Release(owner)).await.unwrap();
    assert_eq!(
        rusqlite::Connection::open(&fixture.path)
            .unwrap()
            .query_row("SELECT counter FROM sample WHERE id=1", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    lane.eof(true).await;
}

#[tokio::test]
async fn sqlite_oversized_reply_closes_lane_and_releases_source_cursor_without_replay() {
    let fixture = Fixture::new("sqlite-oversized", true);
    let (mut lane, opened) = Lane::start(&fixture, "sqlite-oversized", &fixture.path).await;
    opened.result.unwrap();
    // Fits the source row/session budgets but a 500-row page exceeds 16 MiB
    // after serialization. A RETURNING write must never be retried.
    let request = lane.query(
        "oversized",
        "UPDATE sample SET counter=counter+1 RETURNING hex(zeroblob(20000))",
    );
    let error = lane
        .call(SqliteCommand::QueryRun(request))
        .await
        .unwrap_err();
    assert_eq!(error.code, DatabaseOperationalErrorCode::ServerDisconnected);
    assert!(error.message.contains("not replayed"));
    let status = tokio::time::timeout(Duration::from_secs(5), lane.child.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(!status.success());
    // Still holding stdin open must not keep the source connection busy.
    let conn = rusqlite::Connection::open(&fixture.path).unwrap();
    assert_eq!(
        conn.query_row("SELECT min(counter), max(counter) FROM sample", [], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
        })
        .unwrap(),
        (1, 1)
    );
    conn.execute("UPDATE sample SET counter=2", []).unwrap();
}

#[tokio::test]
async fn sqlite_eof_interrupts_query_and_releases_database_even_when_output_is_unread() {
    for slow in [true, false] {
        let fixture = Fixture::new("sqlite-eof", true);
        let (mut lane, opened) = Lane::start(&fixture, "sqlite-eof", &fixture.path).await;
        opened.result.unwrap();
        let sql = if slow {
            SLOW
        } else {
            "SELECT hex(zeroblob(4096)) FROM sample"
        };
        let query = lane.query("drop", sql);
        lane.send(SqliteCommand::QueryRun(query)).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        lane.eof(true).await;
        let conn = rusqlite::Connection::open(&fixture.path).unwrap();
        conn.execute("UPDATE sample SET counter=1", []).unwrap();
    }
}

#[tokio::test]
async fn sqlite_rejects_untrusted_missing_external_files_and_replayed_writes() {
    let fixture = Fixture::new("sqlite-untrusted", false);
    let (lane, reply) = Lane::start(&fixture, "sqlite-untrusted", &fixture.path).await;
    assert_eq!(
        reply.result.unwrap_err().code,
        DatabaseOperationalErrorCode::SqlitePathInvalid
    );
    lane.eof(true).await;
    let fixture = Fixture::new("sqlite-replay", true);
    let missing = fixture.root.join("missing.sqlite");
    let (lane, reply) = Lane::start(&fixture, "sqlite-replay", &missing).await;
    assert_eq!(
        reply.result.unwrap_err().code,
        DatabaseOperationalErrorCode::SqlitePathMissing
    );
    lane.eof(true).await;
    assert!(!missing.exists());
    let external = fixture.home.path().join("outside.sqlite");
    std::fs::copy(&fixture.path, &external).unwrap();
    let linked = fixture.root.join("linked.sqlite");
    std::os::unix::fs::symlink(&external, &linked).unwrap();
    let (lane, reply) = Lane::start(&fixture, "sqlite-replay", &linked).await;
    assert_eq!(
        reply.result.unwrap_err().code,
        DatabaseOperationalErrorCode::SqlitePathInvalid
    );
    lane.eof(true).await;
    let (mut lane, reply) = Lane::start(&fixture, "sqlite-replay", &fixture.path).await;
    reply.result.unwrap();
    let query = lane.query("write-once", "UPDATE sample SET counter=counter+1");
    lane.call(SqliteCommand::QueryRun(query.clone()))
        .await
        .unwrap();
    let replay = SqliteRequest {
        version: PROTOCOL_VERSION,
        owner: lane.owner.clone(),
        id: lane.next,
        call: SqliteCommand::QueryRun(query),
    };
    lane.input
        .as_mut()
        .unwrap()
        .write_all(&encode(&replay).unwrap())
        .await
        .unwrap();
    lane.eof(false).await;
    assert_eq!(
        rusqlite::Connection::open(&fixture.path)
            .unwrap()
            .query_row("SELECT max(counter) FROM sample", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
}
