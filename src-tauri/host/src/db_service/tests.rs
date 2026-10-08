use super::*;

const ROW_BOUNDARIES: [usize; 7] = [0, 499, 500, 501, 1000, 1001, 1201];
const SQLITE_CANCELLATION_PROBE: &str =
        "WITH RECURSIVE probe(n) AS (VALUES(0) UNION ALL SELECT n + 1 FROM probe WHERE n < 100000000) SELECT sum(n) FROM probe";

fn mem() -> Connection {
    Connection::open_in_memory().unwrap()
}

fn existing_sqlite_file() -> tempfile::NamedTempFile {
    let file = tempfile::NamedTempFile::new().unwrap();
    let connection = Connection::open(file.path()).unwrap();
    connection
        .execute_batch("CREATE TABLE validator_probe (id INTEGER);")
        .unwrap();
    drop(connection);
    file
}

fn registered_sqlite_actor(
    state: &DbState,
    descriptor: &str,
    connection: &str,
) -> Arc<ProductionConnectionActor> {
    let identity = ConnectionIdentity {
        descriptor_id: DescriptorId(descriptor.to_string()),
        connection_id: ConnectionId(connection.to_string()),
        connection_generation: ConnectionGeneration("generation-1".to_string()),
    };
    let sqlite = Connection::open_in_memory().unwrap();
    sqlite
        .execute_batch(&format!(
            "CREATE TABLE {}_table (id INTEGER);",
            descriptor.replace('-', "_")
        ))
        .unwrap();
    let actor = Arc::new(ProductionConnectionActor::new(
        identity,
        DbHandle::Sqlite(Mutex::new(sqlite)),
    ));
    register_actor(state, actor.clone()).unwrap();
    actor
}

fn primary_request(identity: &ConnectionIdentity, run: &str, sql: String) -> QueryRunRequest {
    QueryRunRequest {
        descriptor_id: identity.descriptor_id.clone(),
        connection_id: identity.connection_id.clone(),
        connection_generation: identity.connection_generation.clone(),
        query_run_id: QueryRunId(run.to_string()),
        mode: QueryRunMode::Primary,
        statements: NonEmptyVec::try_from(vec![QueryExecutionUnit {
            sql,
            transaction_boundary: TransactionBoundary::None,
        }])
        .unwrap(),
    }
}

fn row_session(run: &QueryRun) -> &ResultSession {
    match &run.statements[0].result {
        StatementExecutionResult::Rows {
            result_session: Some(session),
            ..
        }
        | StatementExecutionResult::ResultLimitReached {
            result_session: session,
            ..
        } => session,
        other => panic!("expected result session, got {other:?}"),
    }
}

#[tokio::test]
async fn shutdown_signals_stream_worker_then_closes_and_exact_removes_every_actor() {
    let state = DbState::default();
    let streaming = registered_sqlite_actor(
        &state,
        "descriptor-shutdown-stream",
        "connection-shutdown-stream",
    );
    let idle = registered_sqlite_actor(
        &state,
        "descriptor-shutdown-idle",
        "connection-shutdown-idle",
    );
    let run_owner = QueryRunOwner {
        descriptor_id: streaming.identity().descriptor_id.clone(),
        connection_id: streaming.identity().connection_id.clone(),
        connection_generation: streaming.identity().connection_generation.clone(),
        query_run_id: QueryRunId("run-shutdown-stream".to_string()),
    };
    let lease = streaming
        .acquire_execution(run_owner.clone(), CancelCapability::SqliteInterrupt)
        .unwrap();
    let result_owner = ResultSessionOwner {
        descriptor_id: run_owner.descriptor_id.clone(),
        connection_id: run_owner.connection_id.clone(),
        connection_generation: run_owner.connection_generation.clone(),
        query_run_id: run_owner.query_run_id.clone(),
        statement_execution_id: StatementExecutionId("statement-shutdown-stream".to_string()),
        result_session_id: ResultSessionId("session-shutdown-stream".to_string()),
    };
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    streaming
        .install_result_continuation(&lease, result_owner, sender)
        .unwrap();
    let worker_actor = Arc::clone(&streaming);
    let worker_lease = lease.clone();
    let worker = tokio::spawn(async move {
        assert!(receiver.recv().await.is_none());
        worker_actor.settle_execution(&worker_lease).unwrap();
    });

    let report = shutdown_all_connections(
        &state,
        DatabaseShutdownTimeouts {
            per_actor: Duration::from_secs(1),
            overall: Duration::from_secs(1),
        },
    )
    .await;
    worker.await.unwrap();

    assert_eq!(report.snapshot_count, 2);
    assert_eq!(report.registry_remaining, Some(0));
    assert!(!report.has_failures(), "{report:?}");
    assert!(report.actors.iter().all(|actor| {
        actor.removed_from_registry
            && matches!(actor.outcome, DatabaseActorShutdownOutcome::Closed(_))
    }));
    assert!(streaming.teardown_report().closed);
    assert!(idle.teardown_report().closed);

    let repeated = shutdown_all_connections(
        &state,
        DatabaseShutdownTimeouts {
            per_actor: Duration::from_millis(20),
            overall: Duration::from_millis(20),
        },
    )
    .await;
    assert!(repeated.already_started);
    assert_eq!(repeated.snapshot_count, 0);
    assert_eq!(repeated.registry_remaining, Some(0));
    assert!(!repeated.has_failures());
}

#[tokio::test]
async fn shutdown_reports_stuck_actor_timeout_without_hanging_or_claiming_success() {
    let state = DbState::default();
    let actor = registered_sqlite_actor(
        &state,
        "descriptor-shutdown-stuck",
        "connection-shutdown-stuck",
    );
    let lease = actor
        .acquire_execution(
            QueryRunOwner {
                descriptor_id: actor.identity().descriptor_id.clone(),
                connection_id: actor.identity().connection_id.clone(),
                connection_generation: actor.identity().connection_generation.clone(),
                query_run_id: QueryRunId("run-shutdown-stuck".to_string()),
            },
            CancelCapability::SqliteInterrupt,
        )
        .unwrap();

    let started = Instant::now();
    let report = shutdown_all_connections(
        &state,
        DatabaseShutdownTimeouts {
            per_actor: Duration::from_millis(20),
            overall: Duration::from_millis(100),
        },
    )
    .await;

    assert!(started.elapsed() < Duration::from_millis(500));
    assert_eq!(report.snapshot_count, 1);
    assert_eq!(report.registry_remaining, Some(0));
    assert!(report.has_failures());
    assert!(report.actors[0].removed_from_registry);
    assert!(matches!(
        report.actors[0].outcome,
        DatabaseActorShutdownOutcome::TimedOut {
            timeout: DatabaseShutdownTimeoutKind::PerActor,
            final_state: TeardownReport {
                unreleased_execution: true,
                closed: true,
                ..
            },
        }
    ));

    assert_eq!(
        actor.settle_execution(&lease).unwrap(),
        crate::db_connection_actor::Settlement {
            cancel_requested: true,
            release_requested: true,
            connection_termination_required: false,
        }
    );
    assert!(actor.begin_teardown().unwrap().closed);
}

#[tokio::test]
async fn p7_result_page_wire_flags_are_explicit_and_primary_rejects_multiple_units() {
    let owner = ResultSessionOwner {
        descriptor_id: DescriptorId("descriptor-wire".into()),
        connection_id: ConnectionId("connection-wire".into()),
        connection_generation: ConnectionGeneration("generation-wire".into()),
        query_run_id: QueryRunId("run-wire".into()),
        statement_execution_id: StatementExecutionId("statement-wire".into()),
        result_session_id: ResultSessionId("session-wire".into()),
    };
    let page = ResultPage {
        owner,
        page_index: 2,
        columns: vec!["value".into()],
        rows: Vec::new(),
        has_previous: true,
        has_next: false,
        effect_outcome: EffectOutcome::Unknown,
        lifecycle: ResultSessionLifecycle::Released,
        result_limit_reached: true,
        value_too_large: false,
    };
    let json = serde_json::to_value(page).unwrap();
    assert_eq!(json["lifecycle"], "released");
    assert_eq!(json["resultLimitReached"], true);
    assert_eq!(json["valueTooLarge"], false);
    assert_eq!(json["effectOutcome"], "unknown");

    let state = DbState::default();
    let sessions = ResultSessionState::default();
    let error = query_run_in_state(
        &state,
        &sessions,
        QueryRunRequest {
            descriptor_id: DescriptorId("descriptor-wire".into()),
            connection_id: ConnectionId("connection-wire".into()),
            connection_generation: ConnectionGeneration("generation-wire".into()),
            query_run_id: QueryRunId("run-wire".into()),
            mode: QueryRunMode::Primary,
            statements: NonEmptyVec::try_from(vec![
                QueryExecutionUnit {
                    sql: "SELECT 1".into(),
                    transaction_boundary: TransactionBoundary::None,
                },
                QueryExecutionUnit {
                    sql: "SELECT 2".into(),
                    transaction_boundary: TransactionBoundary::None,
                },
            ])
            .unwrap(),
        },
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, DatabaseOperationalErrorCode::QueryFailed);
    assert_eq!(
        error.message,
        "primary query must contain exactly one statement"
    );
}

#[test]
fn network_primary_cancel_error_preserves_connection_termination_semantics() {
    assert_eq!(
        network_primary_cancel_error(true).engine,
        DatabaseErrorEngine::Mssql
    );
    assert_eq!(
        network_primary_cancel_error(false).engine,
        DatabaseErrorEngine::Yuzora
    );
}

#[test]
fn cancel_dispatch_failure_is_success_only_after_atomic_connection_termination() {
    assert_eq!(
        classify_cancel_request(Err(ActorError::CancelFailed), true),
        Ok(QueryCancelOutcome::CancelledConnectionTerminated)
    );
    assert_eq!(
        classify_cancel_request(Err(ActorError::CancelFailed), false),
        Err(ActorError::CancelFailed)
    );
    assert_eq!(
        classify_cancel_request(
            Ok(
                crate::db_connection_actor::CancelRequest::DriverCancellationRequired(
                    crate::db_connection_actor::DriverCancelPrimitive::PostgresCancelToken,
                )
            ),
            true,
        ),
        Ok(QueryCancelOutcome::CancelledConnectionTerminated)
    );
}

#[test]
fn mssql_helper_terminal_response_distinguishes_execute_from_row_stream() {
    assert!(matches!(
        helper_mssql_terminal_response(false, Some("1201".into())),
        WorkerResponse::Execute {
            affected_rows: Some(value)
        } if value == "1201"
    ));
    assert!(matches!(
        helper_mssql_terminal_response(true, Some("1".into())),
        WorkerResponse::End {
            affected_rows: Some(value)
        } if value == "1"
    ));
}

#[tokio::test]
async fn worker_request_pump_preserves_fragmented_control_frames() {
    use tokio::io::AsyncWriteExt;

    fn frame(request: &WorkerRequest) -> Vec<u8> {
        let body = serde_json::to_vec(request).unwrap();
        let mut frame = (body.len() as u32).to_be_bytes().to_vec();
        frame.extend_from_slice(&body);
        frame
    }

    let (mut writer, reader) = tokio::io::duplex(256);
    let (sender, mut requests) = tokio::sync::mpsc::channel(WORKER_REQUEST_QUEUE_DEPTH);
    let pump = tokio::spawn(pump_worker_requests(reader, sender));
    let stop = frame(&WorkerRequest::StopStreaming);
    let query = frame(&WorkerRequest::Query {
        sql: "SELECT 7".into(),
    });

    writer.write_all(&stop[..2]).await.unwrap();
    tokio::task::yield_now().await;
    writer.write_all(&stop[2..]).await.unwrap();
    writer.write_all(&query).await.unwrap();

    assert!(matches!(
        next_worker_request(&mut requests).await.unwrap(),
        WorkerRequest::StopStreaming
    ));
    match next_worker_request(&mut requests).await.unwrap() {
        WorkerRequest::Query { sql } => assert_eq!(sql, "SELECT 7"),
        other => panic!("expected queued query after stop, got {other:?}"),
    }

    drop(writer);
    let _ = pump.await;
}

#[test]
fn network_primary_workers_decode_only_inside_the_helper() {
    let source = include_str!("../db_service.rs");
    let production = source
        .split("mod tests;")
        .next()
        .expect("production source before tests");
    assert!(production.contains("network_run_primary_worker("));
    assert!(production.contains("tokio::spawn(pg_run_primary_worker"));
    assert!(production.contains("tokio::spawn(mssql_run_primary_worker"));
    assert!(production.contains("async fn helper_pg_query"));
    assert!(production.contains("async fn helper_mssql_query"));
    assert!(production.contains("spawn_worker_request_reader"));
    assert!(
        !production.contains("request = read_request(stdin)"),
        "streaming helpers must receive control frames through the cancellation-safe request pump"
    );
    let pg_helper = production
        .split("async fn helper_pg_query")
        .nth(1)
        .and_then(|source| source.split("async fn helper_mssql_query").next())
        .expect("PostgreSQL helper body");
    assert!(pg_helper.contains("statement = live.client.prepare(sql)"));
    assert!(pg_helper.contains("request = next_worker_request(requests)"));
    let mssql_helper = production
        .split("async fn helper_mssql_query")
        .nth(1)
        .and_then(|source| source.split("pub async fn query_worker_loop").next())
        .expect("MSSQL helper body");
    assert!(mssql_helper.contains("let mut stream = tokio::select!"));
    assert!(mssql_helper.contains("stream = client.simple_query(sql)"));
    assert!(mssql_helper.contains("request = next_worker_request(requests)"));
    let network_worker = production
        .split("async fn network_run_primary_worker")
        .nth(1)
        .and_then(|source| source.split("async fn pg_run_primary_worker").next())
        .expect("network primary worker body");
    assert!(network_worker.contains("tokio::pin!(start_query)"));
    assert!(network_worker.contains("request = cancel_rx.recv()"));
    assert!(network_worker.contains("started = &mut start_query"));
    assert!(
        !network_worker.contains("actor.cancel_requested(&lease)"),
        "network completion must arbitrate cancellation atomically while settling the lease"
    );
    assert!(
        !network_worker.contains("settle_primary_guard(&mut settlement_guard)"),
        "network exits must not bypass termination-aware atomic settlement"
    );
    assert!(
        network_worker
            .matches("settle_network_primary_completion")
            .count()
            >= 10,
        "Execute, End, limit, error, release, and cancellation exits must use atomic settlement"
    );
    let execute_cancel = network_worker
        .split("NetworkQueryStart::Execute")
        .nth(1)
        .and_then(|source| source.split("NetworkQueryStart::Rows").next())
        .expect("network Execute cancellation branch");
    assert!(execute_cancel.contains("settle_network_primary_completion"));
    assert!(execute_cancel.contains("worker, true"));
    assert!(execute_cancel.contains("connection_terminated: true"));
    assert!(
        !production.contains("async fn pg_read_primary_page"),
        "parent must not decode PostgreSQL rows in-process"
    );
    assert!(
        !production.contains("async fn mssql_drive_primary_stream"),
        "parent must not drive an in-process MSSQL QueryStream"
    );
    let release = production
        .split("async fn network_run_primary_worker")
        .nth(1)
        .and_then(|source| {
            source
                .split("Some(ResultContinuationCommand::Release")
                .nth(1)
        })
        .expect("network Release branch");
    let cancel = release
        .find("worker.stop_streaming()")
        .expect("Release must stop the helper stream");
    let drain = release
        .find("drain_helper_stream(worker).await")
        .expect("Release must drain the helper before settlement");
    let settle = release
        .find("settle_network_primary_completion")
        .expect("Release must atomically settle its exact lease");
    assert!(cancel < drain && drain < settle);
}

#[tokio::test]
async fn p7_sqlite_primary_boundaries_page_once_without_blank_terminal_pages() {
    for row_count in ROW_BOUNDARIES {
        let state = DbState::default();
        let actor = registered_sqlite_actor(
            &state,
            &format!("descriptor-primary-{row_count}"),
            &format!("connection-primary-{row_count}"),
        );
        let identity = actor.identity().clone();
        let sessions = ResultSessionState::default();
        let sql = if row_count == 0 {
            "SELECT 1 AS value WHERE 0".to_string()
        } else {
            format!(
                    "WITH RECURSIVE rows(value) AS (VALUES(1) UNION ALL SELECT value + 1 FROM rows WHERE value < {row_count}) SELECT value FROM rows"
                )
        };
        let run = query_run_in_state(
            &state,
            &sessions,
            primary_request(&identity, &format!("run-{row_count}"), sql),
        )
        .await
        .unwrap();
        let session = row_session(&run);
        assert_eq!(
            session.initial_page.rows.len(),
            row_count.min(RESULT_PAGE_ROWS)
        );
        assert_eq!(
            session.initial_page.lifecycle,
            if row_count > RESULT_PAGE_ROWS {
                ResultSessionLifecycle::Streaming
            } else {
                ResultSessionLifecycle::Complete
            }
        );
        assert_eq!(session.initial_page.has_next, row_count > RESULT_PAGE_ROWS);

        let mut page = session.initial_page.clone();
        let mut loaded = page.rows.len();
        while page.has_next {
            page = result_page_in_state(
                &state,
                &sessions,
                ResultPageRequest {
                    owner: session.owner.clone(),
                    direction: ResultPageDirection::Next,
                },
            )
            .await
            .unwrap();
            assert!(page.rows.len() <= RESULT_PAGE_ROWS);
            assert!(
                !page.rows.is_empty(),
                "row_count={row_count} exposed a blank page"
            );
            loaded += page.rows.len();
        }
        assert_eq!(loaded, row_count);
        assert_eq!(page.lifecycle, ResultSessionLifecycle::Complete);
        assert!(!page.has_next);
        let metadata = actor
            .acquire_metadata()
            .expect("EOF must settle the primary execution lease");
        actor.settle_metadata(&metadata).unwrap();
    }
}

#[tokio::test]
async fn p7_sqlite_previous_is_cached_and_row_producing_dml_executes_once() {
    let state = DbState::default();
    let actor = registered_sqlite_actor(&state, "descriptor-once", "connection-once");
    if let DbHandle::Sqlite(connection) = actor.handle() {
        connection
                .lock()
                .unwrap()
                .execute_batch(
                    "CREATE TABLE side_effect_rows(id INTEGER PRIMARY KEY, touched INTEGER NOT NULL DEFAULT 0);\
                     WITH RECURSIVE rows(value) AS (VALUES(1) UNION ALL SELECT value + 1 FROM rows WHERE value < 1201)\
                     INSERT INTO side_effect_rows(id) SELECT value FROM rows;",
                )
                .unwrap();
    }
    let identity = actor.identity().clone();
    let sessions = ResultSessionState::default();
    let run = query_run_in_state(
        &state,
        &sessions,
        primary_request(
            &identity,
            "run-once",
            "UPDATE side_effect_rows SET touched = touched + 1 RETURNING id, touched".into(),
        ),
    )
    .await
    .unwrap();
    let owner = row_session(&run).owner.clone();
    let second = result_page_in_state(
        &state,
        &sessions,
        ResultPageRequest {
            owner: owner.clone(),
            direction: ResultPageDirection::Next,
        },
    )
    .await
    .unwrap();
    assert_eq!(second.page_index, 1);
    let previous = result_page_in_state(
        &state,
        &sessions,
        ResultPageRequest {
            owner: owner.clone(),
            direction: ResultPageDirection::Previous,
        },
    )
    .await
    .unwrap();
    assert_eq!(previous.page_index, 0);
    let cached_second = result_page_in_state(
        &state,
        &sessions,
        ResultPageRequest {
            owner: owner.clone(),
            direction: ResultPageDirection::Next,
        },
    )
    .await
    .unwrap();
    assert_eq!(cached_second, second);
    let terminal = result_page_in_state(
        &state,
        &sessions,
        ResultPageRequest {
            owner,
            direction: ResultPageDirection::Next,
        },
    )
    .await
    .unwrap();
    assert_eq!(terminal.rows.len(), 201);
    assert_eq!(terminal.lifecycle, ResultSessionLifecycle::Complete);
    if let DbHandle::Sqlite(connection) = actor.handle() {
        let (count, touched): (i64, i64) = connection
            .lock()
            .unwrap()
            .query_row(
                "SELECT count(*), sum(touched) FROM side_effect_rows",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!((count, touched), (1201, 1201));
    }
}

#[tokio::test]
async fn p7_sqlite_release_preserves_page_and_effect_while_settling_lease() {
    let state = DbState::default();
    let actor = registered_sqlite_actor(&state, "descriptor-release", "connection-release");
    let identity = actor.identity().clone();
    let sessions = ResultSessionState::default();
    let run = query_run_in_state(
            &state,
            &sessions,
            primary_request(
                &identity,
                "run-release",
                "WITH RECURSIVE rows(value) AS (VALUES(1) UNION ALL SELECT value + 1 FROM rows WHERE value < 1201) SELECT value FROM rows".into(),
            ),
        )
        .await
        .unwrap();
    let session = row_session(&run);
    let released = result_session_release_in_state(&state, &sessions, session.owner.clone())
        .await
        .unwrap();
    assert_eq!(released.rows, session.initial_page.rows);
    assert_eq!(released.effect_outcome, EffectOutcome::None);
    assert_eq!(released.lifecycle, ResultSessionLifecycle::Released);
    assert!(!released.has_next);
    let metadata = actor.acquire_metadata().unwrap();
    actor.settle_metadata(&metadata).unwrap();
}

#[tokio::test]
async fn p6_sqlite_runner_orders_units_drains_rows_and_stops_with_skipped_tabs() {
    let state = DbState::default();
    let actor = registered_sqlite_actor(&state, "descriptor-p6", "connection-p6");
    if let DbHandle::Sqlite(connection) = actor.handle() {
        connection
            .lock()
            .unwrap()
            .execute_batch("CREATE TABLE p6_effects(value INTEGER);")
            .unwrap();
    }
    let sessions = crate::db_result_session::ResultSessionState::default();
    let identity = actor.identity().clone();
    let request = QueryRunRequest {
            descriptor_id: identity.descriptor_id.clone(),
            connection_id: identity.connection_id.clone(),
            connection_generation: identity.connection_generation.clone(),
            query_run_id: QueryRunId("run-p6".into()),
            mode: QueryRunMode::Script,
            statements: NonEmptyVec::try_from(vec![
                QueryExecutionUnit {
                    sql: "WITH RECURSIVE rows(value) AS (VALUES(1) UNION ALL SELECT value + 1 FROM rows WHERE value < 1201) SELECT value FROM rows".into(),
                    transaction_boundary: TransactionBoundary::None,
                },
                QueryExecutionUnit {
                    sql: "INSERT INTO p6_effects VALUES (1)".into(),
                    transaction_boundary: TransactionBoundary::None,
                },
                QueryExecutionUnit {
                    sql: "SELECT * FROM missing_p6_table".into(),
                    transaction_boundary: TransactionBoundary::None,
                },
                QueryExecutionUnit {
                    sql: "INSERT INTO p6_effects VALUES (2)".into(),
                    transaction_boundary: TransactionBoundary::None,
                },
            ])
            .unwrap(),
        };

    let run = query_run_in_state(&state, &sessions, request)
        .await
        .unwrap();
    assert_eq!(run.statements.len(), 4);
    let result_owner = match &run.statements[0].result {
        StatementExecutionResult::Rows {
            result_session: Some(session),
            ..
        } => {
            assert_eq!(session.initial_page.rows.len(), 500);
            session.owner.clone()
        }
        other => panic!("expected rows session, got {other:?}"),
    };
    assert_eq!(
        sessions
            .lock()
            .unwrap()
            .page(&result_owner, 2)
            .unwrap()
            .rows
            .len(),
        201
    );
    assert!(matches!(
        run.statements[1].result,
        StatementExecutionResult::Execute { .. }
    ));
    assert!(matches!(
        run.statements[2].result,
        StatementExecutionResult::Error { .. }
    ));
    assert_eq!(run.statements[2].effect_outcome, EffectOutcome::Unknown);
    assert_eq!(run.statements[3].result, StatementExecutionResult::Skipped);
    assert_eq!(run.statements[3].effect_outcome, EffectOutcome::None);
    if let DbHandle::Sqlite(connection) = actor.handle() {
        let count: i64 = connection
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM p6_effects", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1, "the unit after the first error must not execute");
    }
}

#[tokio::test]
async fn p6_sqlite_runner_marks_limit_and_preserves_explicit_transaction_warning() {
    let state = DbState::default();
    let actor = registered_sqlite_actor(&state, "descriptor-limit", "connection-limit");
    let identity = actor.identity().clone();
    let probe_run = QueryRunOwner {
        descriptor_id: identity.descriptor_id.clone(),
        connection_id: identity.connection_id.clone(),
        connection_generation: identity.connection_generation.clone(),
        query_run_id: QueryRunId("run-limit".into()),
    };
    let probe_session = ResultSessionOwner {
        descriptor_id: identity.descriptor_id.clone(),
        connection_id: identity.connection_id.clone(),
        connection_generation: identity.connection_generation.clone(),
        query_run_id: probe_run.query_run_id.clone(),
        statement_execution_id: StatementExecutionId(
            "statement-00000000-0000-0000-0000-000000000000".into(),
        ),
        result_session_id: ResultSessionId("result-00000000-0000-0000-0000-000000000000".into()),
    };
    let accounting_probe = crate::db_result_session::ResultSessionState::default();
    let (fixed_session_bytes, fixed_process_bytes) = {
        let mut registry = accounting_probe.lock().unwrap();
        registry.begin_run(&probe_run).unwrap();
        registry
            .begin_session(probe_session.clone(), vec!["value".to_string()])
            .unwrap();
        (
            registry.session_bytes(&probe_session).unwrap(),
            registry.total_bytes(),
        )
    };
    // Admit the measured retained session/container floor, then leave too
    // little incremental room for the 100-row fixture. This keeps the test
    // about row-cache limiting instead of relying on a pre-accounting magic
    // number that cannot even represent an empty session.
    const ROW_BUDGET_BEYOND_FIXED: usize = 1024;
    let sessions = crate::db_result_session::ResultSessionState::with_limits(
        fixed_session_bytes + ROW_BUDGET_BEYOND_FIXED,
        fixed_process_bytes + ROW_BUDGET_BEYOND_FIXED,
    );
    let limited = query_run_in_state(
            &state,
            &sessions,
            QueryRunRequest {
                descriptor_id: identity.descriptor_id.clone(),
                connection_id: identity.connection_id.clone(),
                connection_generation: identity.connection_generation.clone(),
                query_run_id: QueryRunId("run-limit".into()),
                mode: QueryRunMode::Script,
                statements: NonEmptyVec::try_from(vec![
                    QueryExecutionUnit {
                        sql: "WITH RECURSIVE rows(value) AS (VALUES(1) UNION ALL SELECT value + 1 FROM rows WHERE value < 100) SELECT value FROM rows".into(),
                        transaction_boundary: TransactionBoundary::None,
                    },
                    QueryExecutionUnit {
                        sql: "SELECT 2".into(),
                        transaction_boundary: TransactionBoundary::None,
                    },
                ])
                .unwrap(),
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        limited.statements[0].result,
        StatementExecutionResult::ResultLimitReached { .. }
    ));
    assert_eq!(limited.statements[0].effect_outcome, EffectOutcome::Unknown);
    assert_eq!(
        limited.statements[1].result,
        StatementExecutionResult::Skipped
    );
    assert_eq!(limited.statements[1].effect_outcome, EffectOutcome::None);

    let transaction = query_run_in_state(
        &state,
        &crate::db_result_session::ResultSessionState::default(),
        QueryRunRequest {
            descriptor_id: identity.descriptor_id.clone(),
            connection_id: identity.connection_id.clone(),
            connection_generation: identity.connection_generation.clone(),
            query_run_id: QueryRunId("run-transaction".into()),
            mode: QueryRunMode::Script,
            statements: NonEmptyVec::try_from(vec![
                QueryExecutionUnit {
                    sql: "BEGIN".into(),
                    transaction_boundary: TransactionBoundary::Begin,
                },
                QueryExecutionUnit {
                    sql: "SELECT * FROM missing_inside_transaction".into(),
                    transaction_boundary: TransactionBoundary::None,
                },
                QueryExecutionUnit {
                    sql: "COMMIT".into(),
                    transaction_boundary: TransactionBoundary::Commit,
                },
            ])
            .unwrap(),
        },
    )
    .await
    .unwrap();
    assert!(transaction.transaction_may_be_open);
    assert!(matches!(
        transaction.statements[1].result,
        StatementExecutionResult::Error { .. }
    ));
    assert_eq!(
        transaction.statements[2].result,
        StatementExecutionResult::Skipped
    );
    assert_eq!(
        transaction.statements[2].effect_outcome,
        EffectOutcome::None
    );
    if let DbHandle::Sqlite(connection) = actor.handle() {
        connection
            .lock()
            .unwrap()
            .execute_batch("ROLLBACK")
            .unwrap();
    }
}

#[tokio::test]
async fn sqlite_field_ceiling_rejects_before_clone_and_next_query_recovers() {
    let state = DbState::default();
    let actor = registered_sqlite_actor(&state, "descriptor-field", "connection-field");
    if let DbHandle::Sqlite(connection) = actor.handle() {
        connection
            .lock()
            .unwrap()
            .execute_batch("CREATE TABLE hostile(id INTEGER PRIMARY KEY, payload BLOB NOT NULL);")
            .unwrap();
        let oversized = vec![0u8; crate::db_result_session::DEFAULT_FIELD_BYTES + 1];
        connection
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO hostile(id, payload) VALUES (1, ?1)",
                [&oversized as &[u8]],
            )
            .unwrap();
        connection
            .lock()
            .unwrap()
            .execute("INSERT INTO hostile(id, payload) VALUES (2, x'00')", [])
            .unwrap();
    }
    let identity = actor.identity().clone();
    let sessions = crate::db_result_session::ResultSessionState::default();
    let hostile = query_run_in_state(
        &state,
        &sessions,
        QueryRunRequest {
            descriptor_id: identity.descriptor_id.clone(),
            connection_id: identity.connection_id.clone(),
            connection_generation: identity.connection_generation.clone(),
            query_run_id: QueryRunId("run-hostile-field".into()),
            mode: QueryRunMode::Primary,
            statements: NonEmptyVec::try_from(vec![QueryExecutionUnit {
                sql: "SELECT payload FROM hostile WHERE id = 1".into(),
                transaction_boundary: TransactionBoundary::None,
            }])
            .unwrap(),
        },
    )
    .await
    .unwrap();
    match &hostile.statements[0].result {
        StatementExecutionResult::ResultLimitReached { result_session, .. } => {
            assert!(result_session.initial_page.value_too_large);
            assert!(result_session.initial_page.rows.is_empty());
        }
        other => panic!("expected valueTooLarge limit, got {other:?}"),
    }

    let recovered = query_run_in_state(
        &state,
        &sessions,
        QueryRunRequest {
            descriptor_id: identity.descriptor_id.clone(),
            connection_id: identity.connection_id.clone(),
            connection_generation: identity.connection_generation.clone(),
            query_run_id: QueryRunId("run-hostile-recover".into()),
            mode: QueryRunMode::Primary,
            statements: NonEmptyVec::try_from(vec![QueryExecutionUnit {
                sql: "SELECT id FROM hostile WHERE id = 2".into(),
                transaction_boundary: TransactionBoundary::None,
            }])
            .unwrap(),
        },
    )
    .await
    .unwrap();
    match &recovered.statements[0].result {
        StatementExecutionResult::Rows {
            result_session: Some(session),
            ..
        } => {
            assert_eq!(session.initial_page.rows.len(), 1);
            assert!(!session.initial_page.value_too_large);
            assert_eq!(
                session.initial_page.rows[0][0],
                DbValue::Integer { value: "2".into() }
            );
        }
        other => panic!("expected recovered rows, got {other:?}"),
    }
}

#[tokio::test]
async fn p6_runner_settles_lease_on_registry_failure_and_aborts_partial_session_on_decode_error() {
    let state = DbState::default();
    let actor = registered_sqlite_actor(&state, "descriptor-cleanup", "connection-cleanup");
    let identity = actor.identity().clone();
    let request = |run: &str, sql: &str| QueryRunRequest {
        descriptor_id: identity.descriptor_id.clone(),
        connection_id: identity.connection_id.clone(),
        connection_generation: identity.connection_generation.clone(),
        query_run_id: QueryRunId(run.into()),
        mode: QueryRunMode::Primary,
        statements: NonEmptyVec::try_from(vec![QueryExecutionUnit {
            sql: sql.into(),
            transaction_boundary: TransactionBoundary::None,
        }])
        .unwrap(),
    };

    let poisoned = crate::db_result_session::ResultSessionState::default();
    let poison_target = poisoned.clone();
    std::thread::spawn(move || {
        let _guard = poison_target.0.lock().unwrap();
        panic!("poison result session lock");
    })
    .join()
    .unwrap_err();
    assert!(
        query_run_in_state(&state, &poisoned, request("run-poison", "SELECT 1"))
            .await
            .is_err()
    );
    let metadata = actor
        .acquire_metadata()
        .expect("registry failure must settle the execution lease");
    actor.settle_metadata(&metadata).unwrap();

    let sessions = crate::db_result_session::ResultSessionState::default();
    let run = query_run_in_state(
        &state,
        &sessions,
        request("run-decode", "SELECT CAST(x'80' AS TEXT)"),
    )
    .await
    .unwrap();
    assert!(matches!(
        run.statements[0].result,
        StatementExecutionResult::Error { .. }
    ));
    assert_eq!(sessions.lock().unwrap().session_count(), 0);
    let metadata = actor
        .acquire_metadata()
        .expect("decode failure must settle the execution lease");
    actor.settle_metadata(&metadata).unwrap();
}

#[tokio::test]
async fn p6_sqlite_cancel_command_interrupts_exact_owner_and_waits_for_settlement() {
    let state = DbState::default();
    let actor = registered_sqlite_actor(&state, "descriptor-cancel", "connection-cancel");
    let identity = actor.identity().clone();
    let owner = QueryRunOwner {
        descriptor_id: identity.descriptor_id.clone(),
        connection_id: identity.connection_id.clone(),
        connection_generation: identity.connection_generation.clone(),
        query_run_id: QueryRunId("run-cancel".into()),
    };
    let lease = actor
        .acquire_execution(owner.clone(), CancelCapability::SqliteInterrupt)
        .unwrap();
    let (started_tx, started_rx) = std::sync::mpsc::sync_channel(0);
    let worker_actor = actor.clone();
    let worker = std::thread::spawn(move || {
        let result = match worker_actor.handle() {
            DbHandle::Sqlite(connection) => {
                let connection = connection.lock().unwrap();
                let mut statement = connection.prepare(SQLITE_CANCELLATION_PROBE).unwrap();
                let _cancellation = SqliteCancellationGuard::install(
                    &connection,
                    worker_actor.clone(),
                    lease.clone(),
                )
                .unwrap();
                started_tx.send(()).unwrap();
                statement.query_row([], |row| row.get::<_, i64>(0))
            }
            _ => unreachable!(),
        };
        worker_actor.settle_execution(&lease).unwrap();
        result
    });
    started_rx.recv().unwrap();

    assert_eq!(
        query_cancel_in_state(&state, owner.clone()).await.unwrap(),
        QueryCancelResult {
            outcome: QueryCancelOutcome::Cancelled,
        }
    );
    assert_eq!(
        worker.join().unwrap().unwrap_err().sqlite_error_code(),
        Some(rusqlite::ErrorCode::OperationInterrupted)
    );
    assert!(actor
        .acquire_execution(
            QueryRunOwner {
                query_run_id: QueryRunId("run-b".into()),
                ..owner
            },
            CancelCapability::SqliteInterrupt,
        )
        .is_ok());
}

#[tokio::test]
async fn p6_cancel_keeps_completed_tab_marks_current_cancelled_and_skips_later_units() {
    let state = DbState::default();
    let actor = registered_sqlite_actor(&state, "descriptor-tabs", "connection-tabs");
    let identity = actor.identity().clone();
    let owner = QueryRunOwner {
        descriptor_id: identity.descriptor_id.clone(),
        connection_id: identity.connection_id.clone(),
        connection_generation: identity.connection_generation.clone(),
        query_run_id: QueryRunId("run-tabs".into()),
    };
    let request = QueryRunRequest {
        descriptor_id: owner.descriptor_id.clone(),
        connection_id: owner.connection_id.clone(),
        connection_generation: owner.connection_generation.clone(),
        query_run_id: owner.query_run_id.clone(),
        mode: QueryRunMode::Script,
        statements: NonEmptyVec::try_from(vec![
            QueryExecutionUnit {
                sql: "SELECT 1".into(),
                transaction_boundary: TransactionBoundary::None,
            },
            QueryExecutionUnit {
                sql: SQLITE_CANCELLATION_PROBE.into(),
                transaction_boundary: TransactionBoundary::None,
            },
            QueryExecutionUnit {
                sql: "SELECT 3".into(),
                transaction_boundary: TransactionBoundary::None,
            },
        ])
        .unwrap(),
    };
    let sessions = crate::db_result_session::ResultSessionState::default();
    let run_state = state.clone();
    let run_sessions = sessions.clone();
    let run =
        tokio::spawn(async move { query_run_in_state(&run_state, &run_sessions, request).await });
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            if sessions.lock().unwrap().session_count() >= 2 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("long-running statement did not enter its materialized session");
    assert_eq!(
        query_cancel_in_state(&state, owner).await.unwrap().outcome,
        QueryCancelOutcome::Cancelled
    );
    let run = run.await.unwrap().unwrap();
    assert!(matches!(
        run.statements[0].result,
        StatementExecutionResult::Rows { .. }
    ));
    assert!(matches!(
        run.statements[1].result,
        StatementExecutionResult::Cancelled { .. }
    ));
    assert_eq!(run.statements[1].effect_outcome, EffectOutcome::Unknown);
    assert_eq!(run.statements[2].result, StatementExecutionResult::Skipped);
    assert_eq!(run.statements[2].effect_outcome, EffectOutcome::None);
}

#[test]
fn cancelled_transaction_boundaries_do_not_change_the_open_transaction_warning() {
    let cancelled = StatementExecutionResult::Cancelled {
        error: mssql_cancelled_connection_error(),
    };
    let mut before_begin = false;
    apply_successful_transaction_boundary(
        &mut before_begin,
        TransactionBoundary::Begin,
        &cancelled,
    );
    assert!(
        !before_begin,
        "a cancelled BEGIN never opened a transaction"
    );

    let mut before_commit = true;
    apply_successful_transaction_boundary(
        &mut before_commit,
        TransactionBoundary::Commit,
        &cancelled,
    );
    assert!(
        before_commit,
        "a cancelled COMMIT did not prove the transaction closed"
    );
}

#[tokio::test]
async fn busy_actor_metadata_fails_typed_without_blocking_another_descriptor() {
    let state = DbState::default();
    let actor_a = registered_sqlite_actor(&state, "descriptor-a", "connection-a");
    let actor_b = registered_sqlite_actor(&state, "descriptor-b", "connection-b");
    let identity_a = actor_a.identity().clone();
    let identity_b = actor_b.identity().clone();
    let execution = actor_a
        .acquire_execution(
            QueryRunOwner {
                descriptor_id: identity_a.descriptor_id.clone(),
                connection_id: identity_a.connection_id.clone(),
                connection_generation: identity_a.connection_generation.clone(),
                query_run_id: QueryRunId("query-a".to_string()),
            },
            CancelCapability::SqliteInterrupt,
        )
        .unwrap();

    let busy = tokio::time::timeout(
        std::time::Duration::from_millis(100),
        list_tables_in_state(&state, identity_a.clone()),
    )
    .await
    .expect("metadata must fail fast instead of queueing")
    .unwrap_err();
    assert_eq!(busy.code, DatabaseOperationalErrorCode::ConnectionBusy);
    let busy_columns = table_columns_in_state(
        &state,
        identity_a,
        TableInfo {
            catalog: "main".to_string(),
            schema: "main".to_string(),
            name: "descriptor_a_table".to_string(),
            kind: DatabaseObjectKind::Table,
        },
    )
    .await
    .unwrap_err();
    assert_eq!(
        busy_columns.code,
        DatabaseOperationalErrorCode::ConnectionBusy
    );

    let tables_b = list_tables_in_state(&state, identity_b.clone())
        .await
        .unwrap();
    assert_eq!(tables_b.len(), 1);
    assert_eq!(tables_b[0].name, "descriptor_b_table");
    let columns_b = table_columns_in_state(&state, identity_b, tables_b[0].clone())
        .await
        .unwrap();
    assert_eq!(columns_b.len(), 1);
    assert_eq!(columns_b[0].name, "id");
    actor_a.settle_execution(&execution).unwrap();
}

#[test]
fn mssql_typed_classifier_closes_transport_failures_but_not_sql_or_conversion_errors() {
    let driver_io = MssqlInternalError::Driver(tiberius::error::Error::Io {
        kind: std::io::ErrorKind::ConnectionReset,
        message: "transport reset".to_string(),
    });
    let socket_io = MssqlInternalError::Io(std::io::ErrorKind::BrokenPipe);
    for error in [&driver_io, &socket_io] {
        assert_eq!(
            classify_mssql_live_error(
                error,
                DatabaseOperationalErrorCode::MetadataFailed,
                "database metadata request failed",
            )
            .code,
            DatabaseOperationalErrorCode::ServerDisconnected
        );
    }

    let conversion = MssqlInternalError::Driver(tiberius::error::Error::Conversion(
        std::borrow::Cow::Borrowed("bad value"),
    ));
    assert_eq!(
        classify_mssql_live_error(
            &conversion,
            DatabaseOperationalErrorCode::MetadataFailed,
            "database metadata request failed",
        )
        .code,
        DatabaseOperationalErrorCode::MetadataFailed
    );
    let value = MssqlInternalError::Value(value_decode_error(
        DatabaseErrorEngine::Mssql,
        "MSSQL value",
        "unsupported conversion",
    ));
    assert_eq!(
        classify_mssql_live_error(
            &value,
            DatabaseOperationalErrorCode::QueryFailed,
            "database query failed",
        )
        .code,
        DatabaseOperationalErrorCode::QueryFailed
    );

    let state = DbState::default();
    let actor = registered_sqlite_actor(&state, "descriptor-mssql", "connection-mssql");
    let identity = actor.identity().clone();
    let disconnected = cleanup_server_disconnect(
        &state,
        &identity,
        classify_mssql_live_error(
            &driver_io,
            DatabaseOperationalErrorCode::MetadataFailed,
            "database metadata request failed",
        ),
    );
    assert_eq!(
        disconnected.code,
        DatabaseOperationalErrorCode::ServerDisconnected
    );
    assert!(!has_exact_actor(&state, &identity));
    assert!(state.0.lock().unwrap().is_empty());
}

#[tokio::test]
async fn generation_one_work_cannot_affect_generation_two_reconnect() {
    let state = DbState::default();
    let generation_two = ConnectionIdentity {
        descriptor_id: DescriptorId("descriptor-a".to_string()),
        connection_id: ConnectionId("connection-reused".to_string()),
        connection_generation: ConnectionGeneration("generation-2".to_string()),
    };
    register_actor(
        &state,
        Arc::new(ProductionConnectionActor::new(
            generation_two.clone(),
            DbHandle::Sqlite(Mutex::new(Connection::open_in_memory().unwrap())),
        )),
    )
    .unwrap();
    let generation_one = ConnectionIdentity {
        connection_generation: ConnectionGeneration("generation-1".to_string()),
        ..generation_two.clone()
    };

    assert_eq!(
        query_in_state(
            &state,
            generation_one.clone(),
            QueryRunId("query-generation-1".to_string()),
            "SELECT 1".to_string(),
            None,
        )
        .await
        .unwrap_err()
        .code,
        DatabaseOperationalErrorCode::StaleConnection
    );
    assert!(matches!(
        query_in_state(
            &state,
            generation_two.clone(),
            QueryRunId("query-generation-2".to_string()),
            "SELECT 2".to_string(),
            None,
        )
        .await
        .unwrap(),
        QueryResult::Select { .. }
    ));
    assert_eq!(
        close_exact_in_state(&state, &generation_one)
            .unwrap_err()
            .code,
        DatabaseOperationalErrorCode::StaleConnection
    );
    assert_eq!(state.0.lock().unwrap().len(), 1);
    assert!(
        close_exact_in_state(&state, &generation_two)
            .unwrap()
            .closed
    );
    assert!(state.0.lock().unwrap().is_empty());
}

#[tokio::test]
async fn sqlite_production_open_accepts_an_existing_readable_regular_file() {
    let file = existing_sqlite_file();

    let handle = open_unregistered(DbOpenConfig::Sqlite {
        workspace: None,
        path: file.path().to_string_lossy().into_owned(),
    })
    .await
    .unwrap();

    match handle {
        DbHandle::Sqlite(connection) => {
            let count: i64 = connection
                .into_inner()
                .unwrap()
                .query_row("SELECT count(*) FROM validator_probe", [], |row| row.get(0))
                .unwrap();
            assert_eq!(count, 0);
        }
        _ => panic!("expected SQLite handle"),
    }
}

#[tokio::test]
async fn sqlite_missing_path_is_typed_and_never_created() {
    let directory = tempfile::tempdir().unwrap();
    let missing = directory.path().join("must-not-be-created.sqlite");

    let error = open_unregistered(DbOpenConfig::Sqlite {
        workspace: None,
        path: missing.to_string_lossy().into_owned(),
    })
    .await
    .err()
    .expect("missing SQLite path must fail before returning a handle");

    assert_eq!(error.code, DatabaseOperationalErrorCode::SqlitePathMissing);
    assert!(!missing.exists(), "SQLite open created a missing path");
    assert!(
        !serde_json::to_string(&error)
            .unwrap()
            .contains(&missing.to_string_lossy().to_string()),
        "safe error envelope exposed the raw path"
    );
}

#[test]
fn sqlite_directory_and_memory_targets_are_rejected_before_driver_open() {
    let directory = tempfile::tempdir().unwrap();
    assert_eq!(
        validate_existing_sqlite_path(directory.path())
            .unwrap_err()
            .code,
        DatabaseOperationalErrorCode::SqlitePathNotFile
    );
    assert_eq!(
        validate_existing_sqlite_path(":memory:").unwrap_err().code,
        DatabaseOperationalErrorCode::SqlitePathInvalid
    );
}

#[cfg(unix)]
#[test]
fn sqlite_unreadable_file_is_rejected_with_a_safe_typed_error() {
    use std::os::unix::fs::PermissionsExt;

    let file = existing_sqlite_file();
    let original = std::fs::metadata(file.path()).unwrap().permissions();
    std::fs::set_permissions(file.path(), std::fs::Permissions::from_mode(0o000)).unwrap();
    let result = validate_existing_sqlite_path(file.path());
    std::fs::set_permissions(file.path(), original).unwrap();

    assert_eq!(
        result.unwrap_err().code,
        DatabaseOperationalErrorCode::SqlitePathUnreadable
    );
}

fn pg_numeric_wire(weight: i16, sign: u16, scale: u16, digits: &[u16]) -> Vec<u8> {
    let mut raw = Vec::with_capacity(8 + digits.len() * 2);
    raw.extend_from_slice(&(digits.len() as i16).to_be_bytes());
    raw.extend_from_slice(&weight.to_be_bytes());
    raw.extend_from_slice(&sign.to_be_bytes());
    raw.extend_from_slice(&scale.to_be_bytes());
    for digit in digits {
        raw.extend_from_slice(&digit.to_be_bytes());
    }
    raw
}

/// Shared, deterministic P1 fixture. It deliberately contains more objects
/// than the sidebar's historical happy path, cross-catalog name collisions,
/// pagination boundaries, lossless-value probes, and a side-effect counter.
fn deterministic_sqlite_fixture() -> Connection {
    let conn = mem();
    conn.execute_batch(
        "ATTACH DATABASE ':memory:' AS audit;
             CREATE TABLE main.shared_name (id INTEGER PRIMARY KEY);
             CREATE TABLE audit.shared_name (id INTEGER PRIMARY KEY);
             CREATE TABLE main.side_effect_counter (value INTEGER NOT NULL);
             INSERT INTO main.side_effect_counter VALUES (0);
             CREATE TABLE main.value_extremes (
               big_value BIGINT,
               decimal_value DECIMAL,
               precise_decimal TEXT,
               nullable_value TEXT,
               blob_value BLOB
             );
             INSERT INTO main.value_extremes VALUES (
               9223372036854775807,
               12.125,
               '1234567890.123456789',
               NULL,
               x'0001ff'
             );",
    )
    .unwrap();

    for index in 0..42 {
        conn.execute_batch(&format!(
            "CREATE TABLE main.fixture_object_{index:02} (id INTEGER PRIMARY KEY);"
        ))
        .unwrap();
    }

    for count in ROW_BOUNDARIES {
        conn.execute_batch(&format!(
            "CREATE TABLE main.rows_{count} (id INTEGER PRIMARY KEY);"
        ))
        .unwrap();
        if count > 0 {
            conn.execute_batch(&format!(
                "WITH RECURSIVE seq(n) AS (
                       VALUES(1)
                       UNION ALL
                       SELECT n + 1 FROM seq WHERE n < {count}
                     )
                     INSERT INTO main.rows_{count}(id) SELECT n FROM seq;"
            ))
            .unwrap();
        }
    }
    conn
}

#[test]
fn pg_tls_builds_with_and_without_trust_cert() {
    // 兩種模式都要能建出 rustls connector（不連線，只驗證設定組裝）
    assert!(pg_tls(false).is_ok());
    assert!(pg_tls(true).is_ok());
    assert!(postgres_transport_is_authorized(
        PostgresTransportMode::VerifyFull,
        "db.example",
        5432,
        "alice",
        "app",
        None,
        false,
    ));
    assert!(!postgres_transport_is_authorized(
        PostgresTransportMode::EncryptedTrustServerCert,
        "db.example",
        5432,
        "alice",
        "app",
        None,
        false,
    ));
    assert!(postgres_transport_is_authorized(
        PostgresTransportMode::EncryptedTrustServerCert,
        "db.example",
        5432,
        "alice",
        "app",
        None,
        true,
    ));
    assert!(!postgres_transport_is_authorized(
        PostgresTransportMode::InsecurePlaintext,
        "db.example",
        5432,
        "alice",
        "app",
        None,
        false,
    ));
    assert!(!postgres_transport_is_authorized(
        PostgresTransportMode::InsecurePlaintext,
        "db.example",
        5432,
        "alice",
        "app",
        Some(&PostgresInsecureException::new(
            "other", 5432, "alice", "app",
        )),
        false,
    ));
    assert!(postgres_transport_is_authorized(
        PostgresTransportMode::InsecurePlaintext,
        "db.example",
        5432,
        "alice",
        "app",
        Some(&PostgresInsecureException::new(
            "db.example",
            5432,
            "alice",
            "app",
        )),
        false,
    ));
}

#[test]
fn postgres_numeric_decoder_preserves_unbounded_precision_and_scale() {
    let huge = pg_numeric_wire(
        7,
        0x0000,
        9,
        &[
            12, 3456, 7890, 1234, 5678, 9012, 3456, 7890, 1234, 5678, 9000,
        ],
    );
    assert_eq!(
        decode_pg_numeric(&huge).unwrap(),
        "123456789012345678901234567890.123456789"
    );
    let tiny_negative = pg_numeric_wire(-2, 0x4000, 10, &[1234]);
    assert_eq!(decode_pg_numeric(&tiny_negative).unwrap(), "-0.0000123400");
    assert_eq!(
        decode_pg_numeric(&pg_numeric_wire(0, 0xC000, 0, &[])).unwrap(),
        "NaN"
    );
}

#[test]
fn typed_contracts_serialize_with_ts_field_names_and_conservative_defaults() {
    let object = TableInfo {
        catalog: "app".to_string(),
        schema: "audit".to_string(),
        name: "events".to_string(),
        kind: DatabaseObjectKind::Table,
    };
    assert_eq!(
        serde_json::to_value(object).unwrap(),
        serde_json::json!({
            "catalog": "app",
            "schema": "audit",
            "name": "events",
            "kind": "table"
        })
    );
    assert_eq!(
        serde_json::to_value(DbValue::Integer {
            value: "9223372036854775807".to_string()
        })
        .unwrap(),
        serde_json::json!({ "kind": "integer", "value": "9223372036854775807" })
    );
    assert_eq!(
        serde_json::to_value(DbValue::Decimal {
            value: "1234567890.123456789".to_string()
        })
        .unwrap(),
        serde_json::json!({ "kind": "decimal", "value": "1234567890.123456789" })
    );
    assert_eq!(
        serde_json::to_value(DbValue::Binary {
            hex: "0001ff".to_string()
        })
        .unwrap(),
        serde_json::json!({ "kind": "binary", "hex": "0001ff" })
    );
    let error = DatabaseError {
        engine: DatabaseErrorEngine::Postgres,
        message: "syntax error".to_string(),
        code: Some("42601".to_string()),
        position: Some(Box::new(ErrorPosition {
            offset: Some(17),
            line: None,
            column: None,
        })),
        detail: Some("near FROM".to_string()),
        hint: Some("check the select list".to_string()),
        retryability: Retryability::NotRetryable,
    };
    assert_eq!(
        serde_json::to_value(error).unwrap(),
        serde_json::json!({
            "engine": "postgres",
            "message": "syntax error",
            "code": "42601",
            "position": { "offset": 17, "line": null, "column": null },
            "detail": "near FROM",
            "hint": "check the select list",
            "retryability": "notRetryable"
        })
    );
    let operational = DatabaseOperationalError::new(
        DatabaseOperationalErrorCode::QueryFailed,
        "database query failed",
    )
    .with_database_error(DatabaseError {
        engine: DatabaseErrorEngine::Postgres,
        message: "syntax error".to_string(),
        code: Some("42601".to_string()),
        position: Some(Box::new(ErrorPosition {
            offset: Some(9),
            line: None,
            column: None,
        })),
        detail: Some("detail".to_string()),
        hint: Some("hint".to_string()),
        retryability: Retryability::NotRetryable,
    });
    assert_eq!(
        serde_json::to_value(operational).unwrap(),
        serde_json::json!({
            "code": "queryFailed",
            "message": "database query failed",
            "error": {
                "engine": "postgres",
                "message": "syntax error",
                "code": "42601",
                "position": { "offset": 9, "line": null, "column": null },
                "detail": "detail",
                "hint": "hint",
                "retryability": "notRetryable"
            }
        })
    );
    let profile = ProfileDescriptor {
        descriptor_id: DescriptorId("descriptor-1".to_string()),
        config_generation: 1,
        name: "App".to_string(),
        target: ProfileTarget::postgres(
            "db.internal",
            5432,
            "app",
            "alice",
            PostgresTransportMode::VerifyFull,
        ),
        credential_state: CredentialState::Stored,
    };
    let profile_json = serde_json::to_value(profile).unwrap();
    assert_eq!(profile_json["descriptorId"], "descriptor-1");
    assert_eq!(profile_json["target"]["transportMode"], "verifyFull");
    assert!(profile_json["target"].get("ssl").is_none());
    assert!(profile_json["target"].get("trustCert").is_none());
    assert!(profile_json.get("password").is_none());

    let live = LiveConnection {
        descriptor_id: DescriptorId("descriptor-1".to_string()),
        connection_id: ConnectionId("connection-1".to_string()),
        connection_generation: ConnectionGeneration("generation-1".to_string()),
        engine: LiveDatabaseEngine::Mssql,
    };
    assert_eq!(serde_json::to_value(live).unwrap()["engine"], "mssql");
    assert!(serde_json::from_value::<LiveConnection>(serde_json::json!({
        "descriptorId": "descriptor-1",
        "connectionId": "connection-1",
        "connectionGeneration": "generation-1",
        "engine": "yuzora"
    }))
    .is_err());
    assert_eq!(
        serde_json::from_value::<DatabaseError>(serde_json::json!({
            "engine": "yuzora",
            "message": "local validation failed",
            "code": null,
            "position": null,
            "detail": null,
            "hint": null,
            "retryability": "notRetryable"
        }))
        .unwrap()
        .engine,
        DatabaseErrorEngine::Yuzora
    );

    let legacy: QueryResult = serde_json::from_value(serde_json::json!({
        "kind": "execute",
        "affectedRows": "1"
    }))
    .unwrap();
    match legacy {
        QueryResult::Execute { effect_outcome, .. } => {
            assert_eq!(effect_outcome, EffectOutcome::Unknown)
        }
        QueryResult::Select { .. } => panic!("expected execute result"),
    }

    for (outcome, json) in [
        (EffectOutcome::None, "none"),
        (EffectOutcome::Committed, "committed"),
        (EffectOutcome::RolledBack, "rolledBack"),
        (EffectOutcome::TransactionPending, "transactionPending"),
        (EffectOutcome::Unknown, "unknown"),
    ] {
        assert_eq!(
            serde_json::to_value(outcome).unwrap(),
            serde_json::json!(json)
        );
    }
}

#[test]
fn p6_contracts_serialize_frozen_run_units_statuses_and_cancel_outcomes() {
    let request = QueryRunRequest {
        descriptor_id: DescriptorId("descriptor-1".into()),
        connection_id: ConnectionId("connection-1".into()),
        connection_generation: ConnectionGeneration("generation-1".into()),
        query_run_id: QueryRunId("run-1".into()),
        mode: QueryRunMode::Script,
        statements: NonEmptyVec::try_from(vec![QueryExecutionUnit {
            sql: "BEGIN".into(),
            transaction_boundary: TransactionBoundary::Begin,
        }])
        .unwrap(),
    };
    let json = serde_json::to_value(request).unwrap();
    assert_eq!(json["mode"], "script");
    assert_eq!(json["statements"][0]["sql"], "BEGIN");
    assert_eq!(json["statements"][0]["transactionBoundary"], "begin");

    assert_eq!(
        serde_json::to_value(QueryCancelResult {
            outcome: QueryCancelOutcome::CancelledConnectionTerminated,
        })
        .unwrap(),
        serde_json::json!({ "outcome": "cancelledConnectionTerminated" })
    );
}

#[test]
fn query_run_cardinality_binds_optional_result_session_to_exact_statement_owner() {
    let statement_id = StatementExecutionId("statement-1".to_string());
    let run = QueryRun {
        descriptor_id: DescriptorId("descriptor-1".to_string()),
        connection_id: ConnectionId("connection-1".to_string()),
        connection_generation: ConnectionGeneration("generation-1".to_string()),
        query_run_id: QueryRunId("query-run-1".to_string()),
        statements: NonEmptyVec::try_from(vec![
            StatementExecution {
                statement_execution_id: statement_id.clone(),
                statement_index: 0,
                sql: "SELECT 1".to_string(),
                effect_outcome: EffectOutcome::None,
                result: StatementExecutionResult::Rows {
                    result_session: Some(ResultSession {
                        owner: ResultSessionOwner {
                            descriptor_id: DescriptorId("descriptor-1".to_string()),
                            connection_id: ConnectionId("connection-1".to_string()),
                            connection_generation: ConnectionGeneration("generation-1".to_string()),
                            query_run_id: QueryRunId("query-run-1".to_string()),
                            statement_execution_id: statement_id.clone(),
                            result_session_id: ResultSessionId("result-1".to_string()),
                        },
                        columns: vec!["value".to_string()],
                        initial_page: ResultPage {
                            owner: ResultSessionOwner {
                                descriptor_id: DescriptorId("descriptor-1".to_string()),
                                connection_id: ConnectionId("connection-1".to_string()),
                                connection_generation: ConnectionGeneration(
                                    "generation-1".to_string(),
                                ),
                                query_run_id: QueryRunId("query-run-1".to_string()),
                                statement_execution_id: statement_id,
                                result_session_id: ResultSessionId("result-1".to_string()),
                            },
                            page_index: 0,
                            columns: vec!["value".to_string()],
                            rows: vec![],
                            has_previous: false,
                            has_next: false,
                            effect_outcome: EffectOutcome::None,
                            lifecycle: ResultSessionLifecycle::Complete,
                            result_limit_reached: false,
                            value_too_large: false,
                        },
                    }),
                    affected_rows: None,
                },
            },
            StatementExecution {
                statement_execution_id: StatementExecutionId("statement-2".to_string()),
                statement_index: 1,
                sql: "UPDATE counter SET value = value + 1".to_string(),
                effect_outcome: EffectOutcome::Unknown,
                result: StatementExecutionResult::Execute {
                    affected_rows: Some("1".to_string()),
                },
            },
        ])
        .unwrap(),
        transaction_may_be_open: false,
        connection_terminated: false,
    };
    assert_eq!(run.validate_cardinality(), Ok(()));
    let json = serde_json::to_value(&run).unwrap();
    assert_eq!(json["descriptorId"], "descriptor-1");
    assert_eq!(json["connectionGeneration"], "generation-1");
    assert_eq!(json["statements"].as_array().unwrap().len(), 2);
    assert!(json["statements"][0]["result"]["affectedRows"].is_null());
    assert_eq!(json["statements"][1]["result"]["affectedRows"], "1");

    let mut mismatched = run;
    if let StatementExecutionResult::Rows {
        result_session: Some(session),
        ..
    } = &mut mismatched.statements.first_mut().result
    {
        session.owner.connection_generation = ConnectionGeneration("stale-generation".to_string());
    }
    assert_eq!(
        mismatched.validate_cardinality(),
        Err("result session owner must match its statement execution")
    );
}

#[test]
fn query_run_and_request_reject_empty_statements_at_runtime_and_serde_boundary() {
    assert_eq!(
        NonEmptyVec::<String>::try_from(Vec::new()),
        Err("statements must contain at least one item")
    );

    let empty_request = serde_json::json!({
        "descriptorId": "descriptor-1",
        "connectionId": "connection-1",
        "connectionGeneration": "generation-1",
        "queryRunId": "query-run-1",
        "statements": []
    });
    let request_error = serde_json::from_value::<QueryRunRequest>(empty_request)
        .expect_err("empty request statements must fail deserialization");
    assert!(request_error
        .to_string()
        .contains("statements must contain at least one item"));

    let empty_run = serde_json::json!({
        "descriptorId": "descriptor-1",
        "connectionId": "connection-1",
        "connectionGeneration": "generation-1",
        "queryRunId": "query-run-1",
        "statements": []
    });
    let run_error = serde_json::from_value::<QueryRun>(empty_run)
        .expect_err("empty run statements must fail deserialization");
    assert!(run_error
        .to_string()
        .contains("statements must contain at least one item"));
}

#[test]
fn deterministic_fixture_has_many_objects_and_cross_catalog_name_collision() {
    let conn = deterministic_sqlite_fixture();
    let tables = list_tables(&conn).unwrap();
    assert!(
        tables.len() >= 40,
        "fixture exposed only {} objects",
        tables.len()
    );
    assert!(tables.iter().any(|table| table.name == "fixture_object_41"));

    let shared: Vec<_> = tables
        .iter()
        .filter(|table| table.name == "shared_name")
        .map(|table| (table.catalog.as_str(), table.schema.as_str()))
        .collect();
    assert_eq!(shared, vec![("main", "main"), ("audit", "audit")]);

    for catalog in ["main", "audit"] {
        let count: i64 = conn
                .query_row(
                    &format!(
                        "SELECT count(*) FROM {catalog}.sqlite_master WHERE type = 'table' AND name = 'shared_name'"
                    ),
                    [],
                    |row| row.get(0),
                )
                .unwrap();
        assert_eq!(count, 1, "missing duplicate fixture in {catalog}");
    }
}

#[test]
fn deterministic_fixture_covers_all_row_boundaries() {
    let conn = deterministic_sqlite_fixture();
    for count in ROW_BOUNDARIES {
        let actual: i64 = conn
            .query_row(&format!("SELECT count(*) FROM rows_{count}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(actual, count as i64);

        let result = run_query(
            &conn,
            &format!("SELECT id FROM rows_{count} ORDER BY id"),
            DEFAULT_MAX_ROWS,
        )
        .unwrap();
        match result {
            QueryResult::Select {
                rows, truncated, ..
            } => {
                assert_eq!(rows.len(), count.min(DEFAULT_MAX_ROWS));
                assert_eq!(truncated, count > DEFAULT_MAX_ROWS);
            }
            QueryResult::Execute { .. } => panic!("expected boundary select"),
        }
    }
}

#[test]
fn deterministic_fixture_preserves_precision_null_blob_and_side_effect_counter() {
    let conn = deterministic_sqlite_fixture();
    let result = run_query(
            &conn,
            "SELECT big_value, decimal_value, precise_decimal, nullable_value, blob_value FROM value_extremes",
            DEFAULT_MAX_ROWS,
        )
        .unwrap();
    let row = match result {
        QueryResult::Select { rows, .. } => rows.into_iter().next().unwrap(),
        QueryResult::Execute { .. } => panic!("expected value select"),
    };
    assert_eq!(
        row,
        vec![
            DbValue::Integer {
                value: "9223372036854775807".to_string()
            },
            DbValue::Decimal {
                value: "12.125".to_string()
            },
            DbValue::Text {
                value: "1234567890.123456789".to_string()
            },
            DbValue::Null,
            DbValue::Binary {
                hex: "0001ff".to_string()
            },
        ]
    );

    for _ in 0..2 {
        run_query(
            &conn,
            "UPDATE side_effect_counter SET value = value + 1",
            DEFAULT_MAX_ROWS,
        )
        .unwrap();
    }
    let counter: i64 = conn
        .query_row("SELECT value FROM side_effect_counter", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(counter, 2);

    conn.prepare(SQLITE_CANCELLATION_PROBE).unwrap();
}

#[test]
fn list_tables_reports_tables_and_views_with_kind() {
    let conn = mem();
    conn.execute_batch(
        "CREATE TABLE t (id INTEGER);\
             CREATE VIEW v AS SELECT id FROM t;\
             CREATE TEMP TABLE temp_only (id INTEGER);\
             ATTACH DATABASE ':memory:' AS audit;\
             CREATE TABLE audit.attached_only (id INTEGER);",
    )
    .unwrap();
    let tables = list_tables(&conn).unwrap();
    // sqlite_% internal tables are excluded; ordered by type then name.
    let by_name: Vec<_> = tables
        .iter()
        .map(|t| {
            (
                t.catalog.as_str(),
                t.schema.as_str(),
                t.name.as_str(),
                t.kind,
            )
        })
        .collect();
    assert!(by_name.contains(&("main", "main", "t", DatabaseObjectKind::Table)));
    assert!(by_name.contains(&("main", "main", "v", DatabaseObjectKind::View)));
    assert!(by_name.contains(&("temp", "temp", "temp_only", DatabaseObjectKind::Table)));
    assert!(by_name.contains(&("audit", "audit", "attached_only", DatabaseObjectKind::Table)));
}

#[test]
fn sqlite_qualified_columns_keep_duplicate_names_and_composite_primary_keys_distinct() {
    let conn = mem();
    conn.execute_batch(
            "ATTACH DATABASE ':memory:' AS audit;\
             CREATE TABLE main.shared (tenant INTEGER, id INTEGER, main_only TEXT, PRIMARY KEY (tenant, id));\
             CREATE TABLE audit.shared (audit_only BLOB NOT NULL);",
        )
        .unwrap();
    let objects = list_tables(&conn).unwrap();
    let main = objects
        .iter()
        .find(|object| object.schema == "main" && object.name == "shared")
        .unwrap();
    let audit = objects
        .iter()
        .find(|object| object.schema == "audit" && object.name == "shared")
        .unwrap();

    let main_columns = table_columns(&conn, main).unwrap();
    assert_eq!(
        main_columns
            .iter()
            .map(|column| (column.name.as_str(), column.pk))
            .collect::<Vec<_>>(),
        vec![("tenant", true), ("id", true), ("main_only", false)]
    );
    let audit_columns = table_columns(&conn, audit).unwrap();
    assert_eq!(audit_columns.len(), 1);
    assert_eq!(audit_columns[0].name, "audit_only");
    assert!(audit_columns[0].notnull);
    assert!(!audit_columns[0].pk);
}

#[test]
fn sqlite_ddl_refresh_preserves_existing_qualified_references() {
    let conn = mem();
    conn.execute_batch(
        "ATTACH DATABASE ':memory:' AS audit;\
             CREATE TABLE main.shared (id INTEGER);\
             CREATE TABLE audit.shared (id INTEGER);",
    )
    .unwrap();
    let before = list_tables(&conn).unwrap();
    let stable: Vec<_> = before
        .iter()
        .filter(|object| object.name == "shared")
        .cloned()
        .collect();

    conn.execute_batch("CREATE TABLE audit.added_after_refresh (id INTEGER);")
        .unwrap();
    let after = list_tables(&conn).unwrap();
    for object in stable {
        assert!(after.contains(&object));
    }
    assert!(after.iter().any(|object| {
        object.catalog == "audit"
            && object.schema == "audit"
            && object.name == "added_after_refresh"
    }));
}

#[test]
fn sqlite_invalid_text_is_a_structured_decode_failure() {
    let conn = mem();
    let error = run_query(&conn, "SELECT CAST(x'80' AS TEXT)", DEFAULT_MAX_ROWS)
        .expect_err("invalid SQLite text must not cross as lossy UTF-8");
    assert_eq!(error.engine, DatabaseErrorEngine::Sqlite);
    assert_eq!(error.code.as_deref(), Some("valueDecode"));
    assert!(error.message.contains("SQLite text value"));
}

#[test]
fn sqlite_completion_hooks_are_query_scoped_and_driver_evidenced() {
    let conn = mem();
    conn.execute_batch("CREATE TABLE effects (id INTEGER);")
        .unwrap();

    let select = run_query(&conn, "SELECT * FROM effects", DEFAULT_MAX_ROWS).unwrap();
    assert_eq!(
        match select {
            QueryResult::Select { effect_outcome, .. } => effect_outcome,
            QueryResult::Execute { .. } => panic!("expected rows"),
        },
        EffectOutcome::None
    );

    let committed = run_query(&conn, "INSERT INTO effects VALUES (1)", DEFAULT_MAX_ROWS).unwrap();
    assert_eq!(
        match committed {
            QueryResult::Execute { effect_outcome, .. } => effect_outcome,
            QueryResult::Select { .. } => panic!("expected execute"),
        },
        EffectOutcome::Committed
    );

    let pending = run_query(&conn, "BEGIN", DEFAULT_MAX_ROWS).unwrap();
    assert_eq!(
        match pending {
            QueryResult::Execute { effect_outcome, .. } => effect_outcome,
            QueryResult::Select { .. } => panic!("expected execute"),
        },
        EffectOutcome::TransactionPending
    );
    let pending_write =
        run_query(&conn, "INSERT INTO effects VALUES (2)", DEFAULT_MAX_ROWS).unwrap();
    assert_eq!(
        match pending_write {
            QueryResult::Execute { effect_outcome, .. } => effect_outcome,
            QueryResult::Select { .. } => panic!("expected execute"),
        },
        EffectOutcome::TransactionPending
    );
    let rolled_back = run_query(&conn, "ROLLBACK", DEFAULT_MAX_ROWS).unwrap();
    assert_eq!(
        match rolled_back {
            QueryResult::Execute { effect_outcome, .. } => effect_outcome,
            QueryResult::Select { .. } => panic!("expected execute"),
        },
        EffectOutcome::RolledBack
    );
    assert_eq!(
        conn.query_row("SELECT count(*) FROM effects", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        1,
        "a later query observed hook leakage or failed rollback settlement"
    );
}

#[test]
fn sqlite_completion_probe_clears_callbacks_on_drop_and_unwind() {
    let conn = mem();
    conn.execute_batch("CREATE TABLE hook_scope (id INTEGER);")
        .unwrap();

    let probe = SqliteCompletionProbe::install(&conn).unwrap();
    let dropped_flag = probe.committed.clone();
    drop(probe);
    conn.execute("INSERT INTO hook_scope VALUES (1)", [])
        .unwrap();
    assert!(!dropped_flag.load(Ordering::SeqCst));

    let unwind_flag = Arc::new(Mutex::new(None::<Arc<AtomicBool>>));
    let flag_slot = unwind_flag.clone();
    let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let probe = SqliteCompletionProbe::install(&conn).unwrap();
        *flag_slot.lock().unwrap() = Some(probe.committed.clone());
        panic!("completion probe unwind test");
    }));
    assert!(unwind.is_err());
    conn.execute("INSERT INTO hook_scope VALUES (2)", [])
        .unwrap();
    assert!(!unwind_flag
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .load(Ordering::SeqCst));

    let next = run_query(&conn, "INSERT INTO hook_scope VALUES (3)", DEFAULT_MAX_ROWS).unwrap();
    assert!(matches!(
        next,
        QueryResult::Execute {
            effect_outcome: EffectOutcome::Committed,
            ..
        }
    ));
}

#[test]
fn engine_completion_mapper_never_infers_past_its_input_evidence() {
    for (completion, expected) in [
        (EngineCompletion::NoEffect, EffectOutcome::None),
        (EngineCompletion::Committed, EffectOutcome::Committed),
        (EngineCompletion::RolledBack, EffectOutcome::RolledBack),
        (
            EngineCompletion::TransactionPending,
            EffectOutcome::TransactionPending,
        ),
        (EngineCompletion::Unknown, EffectOutcome::Unknown),
    ] {
        assert_eq!(effect_outcome_from_completion(completion), expected);
    }
}

#[test]
fn network_column_queries_filter_full_identity_and_include_primary_keys() {
    for (sql, placeholders) in [
        (PG_TABLE_COLUMNS_SQL, ["$1", "$2", "$3"]),
        (MSSQL_TABLE_COLUMNS_SQL, ["@P1", "@P2", "@P3"]),
    ] {
        for field in ["table_catalog", "table_schema", "table_name"] {
            assert!(sql.contains(field), "missing {field} from {sql}");
        }
        for placeholder in placeholders {
            assert!(
                sql.contains(placeholder),
                "missing {placeholder} from {sql}"
            );
        }
        assert!(sql.contains("PRIMARY KEY"));
        assert!(sql.contains("column_name"));
        assert!(sql.contains("data_type"));
        assert!(sql.contains("is_nullable"));
    }
    assert!(!PG_LIST_TABLES_SQL.contains("dblink"));
}

#[test]
fn mssql_done_counts_are_checked_and_preserve_zero() {
    assert_eq!(aggregate_mssql_affected_rows(&[]).unwrap(), None);
    assert_eq!(
        aggregate_mssql_affected_rows(&[0]).unwrap().as_deref(),
        Some("0")
    );
    assert_eq!(
        aggregate_mssql_affected_rows(&[2, 3, 0])
            .unwrap()
            .as_deref(),
        Some("5")
    );
}

#[test]
fn mssql_output_shaped_result_keeps_rows_and_done_count() {
    let mut drained = MssqlDrainState::default();
    drained.observe_metadata(0, vec!["id".to_string()]);
    assert_eq!(
        drained.prepare_row(0, 1, DEFAULT_MAX_ROWS),
        MssqlRowAction::Decode
    );
    drained.record_decoded_row(Ok(vec![DbValue::Integer {
        value: "42".to_string(),
    }]));
    let result = drained.finish(&[2]).unwrap();

    assert_eq!(
        serde_json::to_value(&result).unwrap(),
        serde_json::json!({
            "kind": "select",
            "columns": ["id"],
            "rows": [[{ "kind": "integer", "value": "42" }]],
            "truncated": false,
            "affectedRows": "2",
            "effectOutcome": "unknown"
        })
    );
    assert!(matches!(
        result,
        QueryResult::Select {
            affected_rows: Some(ref rows),
            ..
        } if rows == "2"
    ));

    let execute = MssqlDrainState::default().finish(&[]).unwrap();
    assert_eq!(
        serde_json::to_value(execute).unwrap()["affectedRows"],
        serde_json::Value::Null
    );
}

#[test]
fn mssql_drain_rejects_multiple_or_incoherent_result_shapes() {
    let mut multiple = MssqlDrainState::default();
    multiple.observe_metadata(0, vec!["first".to_string()]);
    assert_eq!(multiple.prepare_row(0, 1, 1), MssqlRowAction::Decode);
    multiple.record_decoded_row(Ok(vec![DbValue::Integer {
        value: "1".to_string(),
    }]));
    assert_eq!(multiple.prepare_row(0, 1, 1), MssqlRowAction::DrainOnly);
    assert!(multiple.truncated);
    multiple.observe_metadata(1, vec!["second".to_string(), "third".to_string()]);
    assert_eq!(multiple.prepare_row(1, 2, 1), MssqlRowAction::DrainOnly);
    assert_eq!(
        multiple.rows.len(),
        1,
        "second result rows must not be mixed in"
    );
    let multiple_error = multiple.finish(&[1, 1]).unwrap_err();
    assert_eq!(multiple_error.code.as_deref(), Some("resultShape"));
    assert!(multiple_error
        .detail
        .as_deref()
        .unwrap()
        .contains("cannot represent result set 1"));

    let mut wrong_width = MssqlDrainState::default();
    wrong_width.observe_metadata(0, vec!["only".to_string()]);
    assert_eq!(wrong_width.prepare_row(0, 2, 10), MssqlRowAction::DrainOnly);
    let width_error = wrong_width.finish(&[]).unwrap_err();
    assert_eq!(width_error.code.as_deref(), Some("resultShape"));
    assert!(width_error
        .detail
        .as_deref()
        .unwrap()
        .contains("contains 2"));
}

#[test]
fn mssql_value_error_is_deferred_until_after_drain_items_are_observed() {
    let decode_error = value_decode_error(
        DatabaseErrorEngine::Mssql,
        "MSSQL test value",
        "unsupported conversion",
    );
    let mut drained = MssqlDrainState::default();
    drained.observe_metadata(0, vec!["value".to_string()]);
    assert_eq!(drained.prepare_row(0, 1, 10), MssqlRowAction::Decode);
    drained.record_decoded_row(Err(decode_error));

    // The production loop receives `DrainOnly`, keeps polling items and
    // does not use `?` on the cell conversion result.
    assert_eq!(drained.prepare_row(0, 1, 10), MssqlRowAction::DrainOnly);
    drained.observe_metadata(1, vec!["later".to_string()]);
    assert_eq!(drained.prepare_row(1, 1, 10), MssqlRowAction::DrainOnly);

    let error = drained.finish(&[7]).unwrap_err();
    assert_eq!(error.code.as_deref(), Some("valueDecode"));
    assert!(error
        .detail
        .as_deref()
        .unwrap()
        .contains("unsupported conversion"));
}

#[test]
fn select_serialises_native_and_blob_and_null_types() {
    let conn = mem();
    conn.execute_batch(
        "CREATE TABLE t (i INTEGER, r REAL, s TEXT, b BLOB, n INTEGER);\
             INSERT INTO t VALUES (42, 3.5, 'hi', x'0102030405', NULL);",
    )
    .unwrap();
    let result = run_query(&conn, "SELECT i, r, s, b, n FROM t", DEFAULT_MAX_ROWS).unwrap();
    match result {
        QueryResult::Select {
            columns,
            rows,
            truncated,
            ..
        } => {
            assert_eq!(columns, vec!["i", "r", "s", "b", "n"]);
            assert!(!truncated);
            assert_eq!(rows.len(), 1);
            let row = &rows[0];
            assert_eq!(
                row[0],
                DbValue::Integer {
                    value: "42".to_string()
                }
            );
            assert_eq!(
                row[1],
                DbValue::Decimal {
                    value: "3.5".to_string()
                }
            );
            assert_eq!(
                row[2],
                DbValue::Text {
                    value: "hi".to_string()
                }
            );
            assert_eq!(
                row[3],
                DbValue::Binary {
                    hex: "0102030405".to_string()
                }
            );
            assert_eq!(row[4], DbValue::Null);
        }
        other => panic!(
            "expected Select, got {}",
            serde_json::to_value(other).unwrap()
        ),
    }
}

#[test]
fn select_truncates_at_max_rows() {
    let conn = mem();
    conn.execute_batch(
        "CREATE TABLE t (id INTEGER);\
             INSERT INTO t VALUES (1),(2),(3),(4),(5);",
    )
    .unwrap();
    let result = run_query(&conn, "SELECT id FROM t ORDER BY id", 2).unwrap();
    match result {
        QueryResult::Select {
            rows, truncated, ..
        } => {
            assert_eq!(rows.len(), 2);
            assert!(truncated);
        }
        other => panic!(
            "expected Select, got {}",
            serde_json::to_value(other).unwrap()
        ),
    }
}

#[test]
fn select_exactly_at_cap_is_not_truncated() {
    let conn = mem();
    conn.execute_batch(
        "CREATE TABLE t (id INTEGER);\
             INSERT INTO t VALUES (1),(2);",
    )
    .unwrap();
    let result = run_query(&conn, "SELECT id FROM t", 2).unwrap();
    match result {
        QueryResult::Select {
            rows, truncated, ..
        } => {
            assert_eq!(rows.len(), 2);
            assert!(!truncated);
        }
        other => panic!(
            "expected Select, got {}",
            serde_json::to_value(other).unwrap()
        ),
    }
}

#[test]
fn execute_reports_affected_rows() {
    let conn = mem();
    conn.execute_batch(
        "CREATE TABLE t (id INTEGER);\
             INSERT INTO t VALUES (1),(2),(3);",
    )
    .unwrap();
    let result = run_query(&conn, "UPDATE t SET id = id + 1", DEFAULT_MAX_ROWS).unwrap();
    match result {
        QueryResult::Execute { affected_rows, .. } => {
            assert_eq!(affected_rows.as_deref(), Some("3"))
        }
        other => panic!(
            "expected Execute, got {}",
            serde_json::to_value(other).unwrap()
        ),
    }
}

#[test]
fn sqlite_sql_error_preserves_code_message_and_utf8_byte_offset() {
    let conn = mem();
    let err = run_query(&conn, "SELECT '雪', FROM nope", DEFAULT_MAX_ROWS).unwrap_err();
    assert_eq!(err.engine, DatabaseErrorEngine::Sqlite);
    assert!(err.message.to_lowercase().contains("syntax"));
    assert!(err.code.is_some());
    let offset = err
        .position
        .and_then(|position| position.offset)
        .expect("modern SQLite should report an input byte offset");
    assert_eq!(offset, "SELECT '雪', ".len() as u64);
}

#[test]
fn quote_ident_escapes_embedded_quotes() {
    assert_eq!(quote_ident("plain"), "\"plain\"");
    assert_eq!(quote_ident("a\"b"), "\"a\"\"b\"");
}

#[test]
fn table_columns_handles_quoted_name_and_blocks_injection() {
    let conn = mem();
    // Table whose name itself contains a double quote: only correct escaping
    // makes PRAGMA table_info find it.
    conn.execute_batch("CREATE TABLE \"a\"\"b\" (x INTEGER, y TEXT NOT NULL);")
        .unwrap();
    conn.execute_batch("CREATE TABLE victim (id INTEGER);")
        .unwrap();

    let quoted = TableInfo {
        catalog: "main".to_string(),
        schema: "main".to_string(),
        name: "a\"b".to_string(),
        kind: DatabaseObjectKind::Table,
    };
    let cols = table_columns(&conn, &quoted).unwrap();
    let names: Vec<_> = cols.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, vec!["x", "y"]);
    assert!(cols[1].notnull);

    // An injection attempt in the table name must not execute the DROP: it is
    // quoted into a single (nonexistent) identifier, yielding no columns, and
    // the victim table survives.
    let inject = table_columns(
        &conn,
        &TableInfo {
            name: "x\"); DROP TABLE victim; --".to_string(),
            ..quoted
        },
    )
    .unwrap();
    assert!(inject.is_empty());
    let still_there = run_query(&conn, "SELECT count(*) FROM victim", DEFAULT_MAX_ROWS).unwrap();
    match still_there {
        QueryResult::Select { rows, .. } => assert_eq!(
            rows[0][0],
            DbValue::Integer {
                value: "0".to_string()
            }
        ),
        other => panic!(
            "expected Select, got {}",
            serde_json::to_value(other).unwrap()
        ),
    }
}

#[test]
fn open_close_registry_roundtrip() {
    let state = DbState::default();
    let conn_id = next_conn_id();
    let identity = ConnectionIdentity {
        descriptor_id: DescriptorId("descriptor-roundtrip".to_string()),
        connection_id: ConnectionId(conn_id.clone()),
        connection_generation: ConnectionGeneration("generation-roundtrip".to_string()),
    };
    let actor = Arc::new(ProductionConnectionActor::new(
        identity.clone(),
        DbHandle::Sqlite(Mutex::new(Connection::open_in_memory().unwrap())),
    ));
    register_actor(&state, actor).unwrap();
    let registered = get_exact_actor(&state, &identity).unwrap();
    match registered.handle() {
        DbHandle::Sqlite(conn) => {
            conn.lock()
                .unwrap()
                .execute_batch("CREATE TABLE t (id INTEGER);")
                .unwrap();
            let tables = list_tables(&conn.lock().unwrap()).unwrap();
            assert_eq!(tables.len(), 1);
        }
        _ => panic!("expected sqlite handle"),
    }
    assert_eq!(
        get_actor(&state, "db-999").err().unwrap().code,
        DatabaseOperationalErrorCode::ServerDisconnected
    );
    assert!(close_exact_in_state(&state, &identity).unwrap().closed);
    assert!(state.0.lock().unwrap().is_empty());
}

#[test]
fn classify_pg_type_maps_common_types() {
    assert_eq!(classify_pg_type(&PgType::BOOL), PgColKind::Bool);
    assert_eq!(classify_pg_type(&PgType::INT2), PgColKind::I16);
    assert_eq!(classify_pg_type(&PgType::INT4), PgColKind::I32);
    assert_eq!(classify_pg_type(&PgType::INT8), PgColKind::I64);
    assert_eq!(classify_pg_type(&PgType::FLOAT4), PgColKind::F32);
    assert_eq!(classify_pg_type(&PgType::FLOAT8), PgColKind::F64);
    assert_eq!(classify_pg_type(&PgType::NUMERIC), PgColKind::Numeric);
    assert_eq!(classify_pg_type(&PgType::VARCHAR), PgColKind::Text);
    assert_eq!(classify_pg_type(&PgType::TEXT), PgColKind::Text);
    assert_eq!(classify_pg_type(&PgType::BPCHAR), PgColKind::Text);
    assert_eq!(classify_pg_type(&PgType::UUID), PgColKind::Uuid);
    assert_eq!(classify_pg_type(&PgType::TIMESTAMP), PgColKind::Timestamp);
    assert_eq!(
        classify_pg_type(&PgType::TIMESTAMPTZ),
        PgColKind::TimestampTz
    );
    assert_eq!(classify_pg_type(&PgType::DATE), PgColKind::Date);
    assert_eq!(classify_pg_type(&PgType::TIME), PgColKind::Time);
    assert_eq!(classify_pg_type(&PgType::JSON), PgColKind::Json);
    assert_eq!(classify_pg_type(&PgType::JSONB), PgColKind::Json);
    assert_eq!(classify_pg_type(&PgType::BYTEA), PgColKind::Bytea);
    // An unmapped type (INET) takes the strict unsupported-type path.
    assert_eq!(classify_pg_type(&PgType::INET), PgColKind::Fallback);
}

#[test]
fn postgres_classified_decode_error_is_structured_and_never_null() {
    let invalid_numeric =
        <PgNumericText as tokio_postgres::types::FromSql>::from_sql(&PgType::NUMERIC, &[0, 1, 2])
            .map(Some);
    let error = pg_decode_result(3, "numeric", invalid_numeric, |value| DbValue::Decimal {
        value: value.0,
    })
    .expect_err("invalid classified payload must not become DbValue::Null");
    assert_eq!(error.engine, DatabaseErrorEngine::Postgres);
    assert_eq!(error.code.as_deref(), Some("valueDecode"));
    assert!(error.message.contains("column 3"));

    let null = pg_decode_result::<i64, &str, _>(4, "int8", Ok(None), |value| DbValue::Integer {
        value: value.to_string(),
    })
    .unwrap();
    assert_eq!(null, DbValue::Null);
}

#[test]
fn postgres_json_wire_decoder_preserves_large_numbers_exactly() {
    use tokio_postgres::types::FromSql;

    const EXACT: &str =
        r#"{"beyondU64":18446744073709551616,"precise":-0.123456789012345678901234567890}"#;
    let json = PgJsonText::from_sql(&PgType::JSON, EXACT.as_bytes()).unwrap();
    assert_eq!(json.0, EXACT);

    let mut jsonb_payload = vec![1];
    jsonb_payload.extend_from_slice(EXACT.as_bytes());
    let jsonb = PgJsonText::from_sql(&PgType::JSONB, &jsonb_payload).unwrap();
    assert_eq!(jsonb.0, EXACT);

    let bad_version = PgJsonText::from_sql(&PgType::JSONB, &[2, b'{', b'}']);
    assert!(bad_version
        .as_ref()
        .unwrap_err()
        .to_string()
        .contains("version 2"));
    let structured = pg_decode_result(2, "jsonb", bad_version.map(Some), |value| DbValue::Json {
        value: value.0,
    })
    .expect_err("invalid JSONB versions must become structured decode errors");
    assert_eq!(structured.code.as_deref(), Some("valueDecode"));
    assert_eq!(structured.engine, DatabaseErrorEngine::Postgres);
    assert!(PgJsonText::from_sql(&PgType::JSON, &[0xff]).is_err());
}

#[test]
fn mssql_value_to_db_value_maps_scalars() {
    assert_eq!(
        mssql_value_to_db_value(&ColumnData::I32(Some(42))).unwrap(),
        DbValue::Integer {
            value: "42".to_string()
        }
    );
    assert_eq!(
        mssql_value_to_db_value(&ColumnData::I64(Some(9))).unwrap(),
        DbValue::Integer {
            value: "9".to_string()
        }
    );
    assert_eq!(
        mssql_value_to_db_value(&ColumnData::F64(Some(3.5))).unwrap(),
        DbValue::Decimal {
            value: "3.5".to_string()
        }
    );
    assert_eq!(
        mssql_value_to_db_value(&ColumnData::Numeric(Some(
            tiberius::numeric::Numeric::new_with_scale(i64::MAX.into(), 4)
        )))
        .unwrap(),
        DbValue::Decimal {
            value: "922337203685477.5807".to_string()
        }
    );
    assert_eq!(
        mssql_value_to_db_value(&ColumnData::Numeric(Some(
            tiberius::numeric::Numeric::new_with_scale(12_300, 4)
        )))
        .unwrap(),
        DbValue::Decimal {
            value: "1.2300".to_string()
        }
    );
    assert_eq!(
        mssql_value_to_db_value(&ColumnData::Bit(Some(true))).unwrap(),
        DbValue::Boolean { value: true }
    );
    assert_eq!(
        mssql_value_to_db_value(&ColumnData::String(Some("hi".into()))).unwrap(),
        DbValue::Text {
            value: "hi".to_string()
        }
    );
    assert_eq!(
        mssql_value_to_db_value(&ColumnData::Binary(Some(vec![1, 2, 3].into()))).unwrap(),
        DbValue::Binary {
            hex: "010203".to_string()
        }
    );
    assert_eq!(
        mssql_value_to_db_value(&ColumnData::Numeric(Some(
            tiberius::numeric::Numeric::new_with_scale(-12, 2)
        )))
        .unwrap(),
        DbValue::Decimal {
            value: "-0.12".to_string()
        }
    );
    assert_eq!(
        mssql_value_to_db_value(&ColumnData::Numeric(Some(
            tiberius::numeric::Numeric::new_with_scale(123400, 4)
        )))
        .unwrap(),
        DbValue::Decimal {
            value: "12.3400".to_string()
        }
    );
}

#[test]
fn mssql_value_to_db_value_maps_nulls() {
    assert_eq!(
        mssql_value_to_db_value(&ColumnData::I32(None)).unwrap(),
        DbValue::Null
    );
    assert_eq!(
        mssql_value_to_db_value(&ColumnData::Bit(None)).unwrap(),
        DbValue::Null
    );
    assert_eq!(
        mssql_value_to_db_value(&ColumnData::String(None)).unwrap(),
        DbValue::Null
    );
    assert_eq!(
        mssql_value_to_db_value(&ColumnData::F64(None)).unwrap(),
        DbValue::Null
    );
}

#[test]
fn mssql_date_conversion_error_is_structured_and_never_null() {
    let wrong_tds_type = ColumnData::Time(None);
    let conversion = chrono::NaiveDate::from_sql(&wrong_tds_type);
    let error = mssql_decode_result("date", conversion, |value| DbValue::Date {
        value: value.to_string(),
    })
    .expect_err("tiberius conversion error must not become DbValue::Null");
    assert_eq!(error.engine, DatabaseErrorEngine::Mssql);
    assert_eq!(error.code.as_deref(), Some("valueDecode"));
    assert!(error.message.contains("MSSQL date"));

    assert_eq!(
        mssql_value_to_db_value(&ColumnData::Date(None)).unwrap(),
        DbValue::Null,
        "only a real driver None maps to SQL NULL"
    );
}

#[test]
fn mssql_value_to_db_value_maps_guid_to_string() {
    let uuid = uuid::Uuid::nil();
    let out = mssql_value_to_db_value(&ColumnData::Guid(Some(uuid))).unwrap();
    assert_eq!(
        out,
        DbValue::Text {
            value: "00000000-0000-0000-0000-000000000000".to_string()
        }
    );
}

#[test]
fn database_password_inputs_deserialize_into_redacted_zeroizing_secret_types() {
    use secrecy::ExposeSecret;

    const SENTINEL: &str = "YUZORA_DB_OPEN_SECRET_SENTINEL";
    let config: DbOpenConfig = serde_json::from_value(serde_json::json!({
        "kind": "postgres",
        "host": "localhost",
        "port": 5432,
        "database": "app",
        "user": "alice",
        "password": SENTINEL,
        "transportMode": "verifyFull"
    }))
    .unwrap();
    let DbOpenConfig::Postgres { password, .. } = config else {
        panic!("expected postgres config")
    };
    assert_eq!(password.expose_secret(), SENTINEL);
    assert!(!format!("{password:?}").contains(SENTINEL));

    let credential: CredentialInput = serde_json::from_value(serde_json::json!({
        "password": SENTINEL
    }))
    .unwrap();
    assert_eq!(credential.password.expose_secret(), SENTINEL);
    assert!(!format!("{:?}", credential.password).contains(SENTINEL));
}

// A minimal startup-only PostgreSQL peer exercises the real driver and open
// path without a database server or any production credentials.
async fn pg_maintenance_fixture(
    database: &str,
    user: &str,
    failures: &[&str],
) -> (Result<LivePg, PgConnectFailure>, Vec<String>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    const SECRET: &str = "MAINTENANCE_FIXTURE_SECRET";
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let failures: Vec<String> = failures.iter().map(|code| code.to_string()).collect();
    let (finished, mut done) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        let mut attempts = Vec::new();
        loop {
            let (mut socket, _) = tokio::select! {
                accepted = listener.accept() => accepted.unwrap(),
                _ = &mut done => break,
            };
            let length = socket.read_u32().await.unwrap();
            let mut startup = vec![0; length as usize - 4];
            socket.read_exact(&mut startup).await.unwrap();
            let fields: Vec<_> = startup[4..].split(|byte| *byte == 0).collect();
            let database = fields
                .chunks(2)
                .find(|pair| pair[0] == b"database")
                .map(|pair| String::from_utf8(pair[1].to_vec()).unwrap())
                .unwrap();
            let failure = failures.get(attempts.len());
            attempts.push(database);
            if let Some(code) = failure {
                let fields = format!("SFATAL\0C{code}\0Mfixture {code} {SECRET}\0Ddetail {SECRET}\0Hhint {SECRET}\0\0");
                socket.write_all(b"E").await.unwrap();
                socket
                    .write_all(&((fields.len() + 4) as u32).to_be_bytes())
                    .await
                    .unwrap();
                socket.write_all(fields.as_bytes()).await.unwrap();
            } else {
                socket
                    .write_all(b"R\0\0\0\x08\0\0\0\0K\0\0\0\x0c\0\0\0\x2a\0\0\0\x54Z\0\0\0\x05I")
                    .await
                    .unwrap();
                break;
            }
        }
        attempts
    });
    let result = pg_open_with_timeout(
        DriverEndpoint::direct(address.ip().to_string(), address.port()),
        database.into(),
        user.into(),
        SECRET.to_string().into(),
        PostgresTransportMode::InsecurePlaintext,
        Some(PostgresInsecureException::new(
            address.ip().to_string(),
            address.port(),
            user,
            database,
        )),
        false,
        Duration::from_secs(2),
    )
    .await;
    let _ = finished.send(());
    (result, server.await.unwrap())
}

#[tokio::test]
async fn postgres_maintenance_falls_back_in_order() {
    for (failures, expected) in [
        (vec![], vec!["postgres"]),
        (vec!["42501"], vec!["postgres", "alice"]),
        (vec!["3D000"], vec!["postgres", "alice"]),
    ] {
        let (result, attempts) = pg_maintenance_fixture("  ", "alice", &failures).await;
        assert!(result.is_ok(), "{:?}", result.err());
        assert_eq!(attempts, expected);
    }
}

#[tokio::test]
async fn postgres_maintenance_never_falls_back_to_template1() {
    for user in ["postgres", "template0", "template1", "", "  "] {
        let (result, attempts) = pg_maintenance_fixture("", user, &["3D000"]).await;
        assert_eq!(
            result
                .err()
                .expect("no other candidate")
                .error
                .code
                .as_deref(),
            Some("3D000")
        );
        assert_eq!(attempts, vec!["postgres"]);
    }
}

#[tokio::test]
async fn postgres_maintenance_reports_all_failures_without_secrets() {
    let (result, attempts) = pg_maintenance_fixture("", "alice", &["42501", "3D000"]).await;
    assert_eq!(attempts, vec!["postgres", "alice"]);
    let error = result.err().expect("all candidates failed").error;
    assert_eq!(error.code.as_deref(), Some("3D000"));
    let detail = error.detail.as_deref().unwrap();
    for candidate in ["postgres", "alice"] {
        assert!(detail.contains(candidate));
    }
    assert!(detail.contains("42501"));
    assert!(error.hint.as_deref().unwrap().contains("database"));
    assert!(!serde_json::to_string(&error)
        .unwrap()
        .contains("MAINTENANCE_FIXTURE_SECRET"));
}

#[tokio::test]
async fn postgres_maintenance_shares_connection_deadline() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let length = socket.read_u32().await.unwrap();
        let mut startup = vec![0; length as usize - 4];
        socket.read_exact(&mut startup).await.unwrap();
        tokio::time::sleep(Duration::from_millis(800)).await;
        let fields = b"SFATAL\0C42501\0MCONNECT denied\0\0";
        socket.write_all(b"E").await.unwrap();
        socket
            .write_all(&((fields.len() + 4) as u32).to_be_bytes())
            .await
            .unwrap();
        socket.write_all(fields).await.unwrap();
        // Keep the second startup pending beyond the shared deadline.
        let (_socket, _) = listener.accept().await.unwrap();
        std::future::pending::<()>().await;
    });
    let result = tokio::time::timeout(
        Duration::from_millis(1500),
        pg_open_with_timeout(
            DriverEndpoint::direct(address.ip().to_string(), address.port()),
            "".into(),
            "alice".into(),
            "fixture".to_string().into(),
            PostgresTransportMode::InsecurePlaintext,
            Some(PostgresInsecureException::new(
                address.ip().to_string(),
                address.port(),
                "alice",
                "",
            )),
            false,
            Duration::from_secs(1),
        ),
    )
    .await;
    server.abort();
    let error = result
        .expect("fallback must share the one-second budget, not start another")
        .err()
        .expect("stalled fallback must time out")
        .error;
    assert_eq!(error.code.as_deref(), Some("connectionTimedOut"));
    let detail = error.detail.as_deref().unwrap();
    assert!(detail.contains("42501") && detail.contains("connectionTimedOut"));
}

#[tokio::test]
async fn postgres_maintenance_stops_on_auth_after_database_failure() {
    let (result, attempts) = pg_maintenance_fixture("", "alice", &["3D000", "28P01"]).await;
    assert_eq!(attempts, vec!["postgres", "alice"]);
    let error = result
        .err()
        .expect("authentication must stop discovery")
        .error;
    assert_eq!(error.code.as_deref(), Some("28P01"));
    assert_eq!(error.retryability, Retryability::NotRetryable);
    let detail = error.detail.as_deref().unwrap();
    assert!(detail.contains("3D000") && detail.contains("28P01"));
    assert!(!error.hint.as_deref().unwrap().contains("grant CONNECT"));
}

#[tokio::test]
async fn postgres_maintenance_does_not_retry_auth_or_explicit_database_failures() {
    for (database, code) in [
        ("", "28P01"),
        ("", "28000"),
        ("", "08006"),
        ("app", "42501"),
        ("app", "3D000"),
    ] {
        let (result, attempts) = pg_maintenance_fixture(database, "alice", &[code]).await;
        assert_eq!(
            result.err().expect("must fail").error.code.as_deref(),
            Some(code)
        );
        assert_eq!(
            attempts,
            vec![if database.is_empty() {
                "postgres"
            } else {
                database
            }]
        );
    }
}

#[tokio::test]
async fn postgres_tls_modes_never_fall_back_to_plaintext() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    for mode in [
        PostgresTransportMode::VerifyFull,
        PostgresTransportMode::EncryptedTrustServerCert,
    ] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let length = socket.read_u32().await.unwrap();
            let code = socket.read_u32().await.unwrap();
            assert_eq!(
                (length, code),
                (8, 80_877_103),
                "TLS modes must request TLS first"
            );
            // Decline TLS. A client in sslmode=prefer would continue with a
            // plaintext StartupMessage on the same socket.
            socket.write_all(b"N").await.unwrap();
            let mut next = [0; 1];
            matches!(socket.read(&mut next).await, Ok(0) | Err(_))
        });
        let result = pg_open_with_timeout(
            DriverEndpoint::direct(address.ip().to_string(), address.port()),
            "app".into(),
            "alice".into(),
            "fixture".to_string().into(),
            mode,
            None,
            true,
            Duration::from_secs(2),
        )
        .await;
        assert!(result.is_err(), "{mode:?} must not connect without TLS");
        assert!(
            server.await.unwrap(),
            "{mode:?} sent a plaintext startup after TLS was declined"
        );
    }
}

#[tokio::test]
async fn postgres_tunnel_preserves_source_policy_and_routes_cancel_to_same_endpoint() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut connection, _) = listener.accept().await.unwrap();
        let length = connection.read_u32().await.unwrap();
        assert!((8..4096).contains(&length));
        let mut startup = vec![0; length as usize - 4];
        connection.read_exact(&mut startup).await.unwrap();
        // AuthenticationOk, BackendKeyData, ReadyForQuery. No credentials
        // are requested by this isolated protocol fixture.
        connection
            .write_all(b"R\0\0\0\x08\0\0\0\0K\0\0\0\x0c\0\0\0\x2a\0\0\0\x54Z\0\0\0\x05I")
            .await
            .unwrap();
        let (mut cancellation, _) = listener.accept().await.unwrap();
        let mut request = [0; 16];
        cancellation.read_exact(&mut request).await.unwrap();
        assert_eq!(&request[0..4], &16_u32.to_be_bytes());
        assert_eq!(&request[4..8], &80877102_u32.to_be_bytes());
        assert_eq!(&request[8..12], &42_u32.to_be_bytes());
        assert_eq!(&request[12..16], &84_u32.to_be_bytes());
    });
    let endpoint = DriverEndpoint {
        host: "source-only.invalid".into(),
        port: 5432,
        connect_address: Some(address),
    };
    let exception =
        PostgresInsecureException::new("source-only.invalid", 5432, "fixture", "fixture");
    let live = pg_open_with_timeout(
        endpoint,
        "fixture".into(),
        "fixture".into(),
        "fixture".to_string().into(),
        PostgresTransportMode::InsecurePlaintext,
        Some(exception),
        false,
        Duration::from_secs(3),
    )
    .await
    .unwrap_or_else(|error| panic!("{}", pg_connect_failure_database_error(&error).message));
    tokio::time::timeout(Duration::from_secs(3), live.cancel.cancel())
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), server)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn pg_open_refused_port_surfaces_os_cause() {
    // 連拒絕的本機埠 → transport 錯誤（as_db_error 為 None，走 source() chain 萃取真因）
    let result = pg_open(
        DriverEndpoint::direct("127.0.0.1".into(), 1),
        "d".to_string(),
        "u".to_string(),
        "p".to_string().into(),
        PostgresTransportMode::VerifyFull,
        None,
        false,
    )
    .await;
    let error = match result {
        Err(failure) => pg_connect_failure_database_error(&failure),
        Ok(_) => panic!("expected connection to refused port to fail"),
    };
    assert_eq!(error.code.as_deref(), Some("connectionFailed"));
    // 真因（io 層）必須由 source() chain 帶出，而非只到泛稱 "error connecting to server"。
    assert!(
        error.message.to_lowercase().contains("refused"),
        "expected the OS-level cause to be surfaced, got: {}",
        error.message
    );
}

#[test]
fn postgres_transport_diagnostics_have_stable_windows_categories() {
    assert_eq!(
        pg_transport_error_code("failed to lookup address information: No such host is known"),
        "dnsFailed"
    );
    assert_eq!(
        pg_transport_error_code("invalid peer certificate: UnknownIssuer"),
        "tlsFailed"
    );
    assert_eq!(
        pg_transport_error_code("connection timed out while opening socket"),
        "connectionTimedOut"
    );
    assert_eq!(
        pg_transport_error_code("Connection refused (os error 10061)"),
        "connectionFailed"
    );
}

#[test]
fn postgres_connection_diagnostic_redacts_password_and_url_userinfo() {
    const SECRET: &str = "YUZORA_POSTGRES_SECRET_SENTINEL";
    let redacted = redact_pg_connection_diagnostic(
            "server rejected postgres://alice:YUZORA_POSTGRES_SECRET_SENTINEL@db.example/app and repeated YUZORA_POSTGRES_SECRET_SENTINEL",
            SECRET,
        );
    assert!(!redacted.contains(SECRET));
    assert!(!redacted.contains("alice:"));
    assert!(redacted.contains("postgres://<redacted>@db.example/app"));
}

#[tokio::test]
async fn pg_open_timeout_is_bounded_structured_and_secret_free() {
    const SECRET: &str = "YUZORA_POSTGRES_TIMEOUT_SECRET";
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (_socket, _) = listener.accept().await.unwrap();
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    });

    let result = pg_open_with_timeout(
        DriverEndpoint::direct(address.ip().to_string(), address.port()),
        "app".to_string(),
        "alice".to_string(),
        SECRET.to_string().into(),
        PostgresTransportMode::VerifyFull,
        None,
        false,
        std::time::Duration::from_millis(40),
    )
    .await;
    server.abort();

    let failure = match result {
        Err(failure) => failure,
        Ok(_) => panic!("silent server must hit the connect deadline"),
    };
    let error = pg_connect_failure_database_error(&failure);
    assert_eq!(error.engine, DatabaseErrorEngine::Postgres);
    assert_eq!(error.code.as_deref(), Some("connectionTimedOut"));
    assert_eq!(error.retryability, Retryability::Retryable);
    assert!(!format!("{error:?}").contains(SECRET));
}

#[tokio::test]
async fn postgres_auth_sqlstate_survives_connect_failure_without_secret() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    const SECRET: &str = "YUZORA_POSTGRES_AUTH_SECRET";
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut length = [0_u8; 4];
        socket.read_exact(&mut length).await.unwrap();
        let startup_len = u32::from_be_bytes(length) as usize;
        let mut startup = vec![0_u8; startup_len.saturating_sub(4)];
        socket.read_exact(&mut startup).await.unwrap();

        let fields = format!("SFATAL\0C28P01\0Mpassword authentication failed: {SECRET}\0\0");
        socket.write_all(b"E").await.unwrap();
        socket
            .write_all(&((fields.len() + 4) as u32).to_be_bytes())
            .await
            .unwrap();
        socket.write_all(fields.as_bytes()).await.unwrap();
    });

    let result = pg_open(
        DriverEndpoint::direct(address.ip().to_string(), address.port()),
        "app".to_string(),
        "alice".to_string(),
        SECRET.to_string().into(),
        PostgresTransportMode::InsecurePlaintext,
        Some(PostgresInsecureException::new(
            address.ip().to_string(),
            address.port(),
            "alice",
            "app",
        )),
        false,
    )
    .await;
    server.await.unwrap();

    let failure = match result {
        Err(failure) => failure,
        Ok(_) => panic!("fake authentication rejection must fail"),
    };
    let diagnostic = pg_connect_failure_database_error(&failure);
    assert_eq!(diagnostic.code.as_deref(), Some("28P01"));
    assert_eq!(diagnostic.retryability, Retryability::NotRetryable);
    let serialized = serde_json::to_string(&diagnostic).unwrap();
    assert!(!serialized.contains(SECRET));
    assert!(serialized.contains("&lt;redacted&gt;") || serialized.contains("<redacted>"));
}

#[tokio::test]
async fn pg_tls_rejects_unacked_plaintext_before_network() {
    use std::sync::atomic::{AtomicBool, Ordering};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let accepted = std::sync::Arc::new(AtomicBool::new(false));
    let flag = accepted.clone();
    let server = tokio::spawn(async move {
        if listener.accept().await.is_ok() {
            flag.store(true, Ordering::SeqCst);
        }
    });

    const SECRET: &str = "YUZORA_PG_TLS_PLAINTEXT_SECRET";
    let result = pg_open(
        DriverEndpoint::direct(address.ip().to_string(), address.port()),
        "app".to_string(),
        "alice".to_string(),
        SECRET.to_string().into(),
        PostgresTransportMode::InsecurePlaintext,
        None,
        false,
    )
    .await;
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    server.abort();

    let failure = match result {
        Err(failure) => failure,
        Ok(_) => panic!("unacked plaintext must not open"),
    };
    let diagnostic = pg_connect_failure_database_error(&failure);
    assert_eq!(
        diagnostic.code.as_deref(),
        Some("postgresTransportRejected")
    );
    assert!(!accepted.load(Ordering::SeqCst));
    assert!(!format!("{failure:?}").contains(SECRET));
}

#[tokio::test]
async fn pg_tls_rejects_unacked_trust_server_cert_before_network() {
    use std::sync::atomic::{AtomicBool, Ordering};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let accepted = std::sync::Arc::new(AtomicBool::new(false));
    let flag = accepted.clone();
    let server = tokio::spawn(async move {
        if listener.accept().await.is_ok() {
            flag.store(true, Ordering::SeqCst);
        }
    });

    const SECRET: &str = "YUZORA_PG_TLS_TRUST_SECRET";
    let result = pg_open(
        DriverEndpoint::direct(address.ip().to_string(), address.port()),
        "app".to_string(),
        "alice".to_string(),
        SECRET.to_string().into(),
        PostgresTransportMode::EncryptedTrustServerCert,
        None,
        false,
    )
    .await;
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    server.abort();

    let failure = match result {
        Err(failure) => failure,
        Ok(_) => panic!("unacked trust-server-cert must not open"),
    };
    let diagnostic = pg_connect_failure_database_error(&failure);
    assert_eq!(
        diagnostic.code.as_deref(),
        Some("postgresTransportRejected")
    );
    assert!(!accepted.load(Ordering::SeqCst));
    assert!(!format!("{failure:?}").contains(SECRET));
}

#[tokio::test]
async fn pg_tls_verify_full_does_not_send_password_to_plaintext_server() {
    use tokio::io::AsyncReadExt;

    const SECRET: &str = "YUZORA_PG_TLS_VERIFY_SECRET";
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let received = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let received_for_server = received.clone();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = vec![0_u8; 1024];
        if let Ok(n) = socket.read(&mut buf).await {
            received_for_server
                .lock()
                .await
                .extend_from_slice(&buf[..n]);
        }
    });

    let result = pg_open(
        DriverEndpoint::direct(address.ip().to_string(), address.port()),
        "app".to_string(),
        "alice".to_string(),
        SECRET.to_string().into(),
        PostgresTransportMode::VerifyFull,
        None,
        false,
    )
    .await;
    let _ = server.await;

    if result.is_ok() {
        panic!("plaintext fixture must fail verify-full TLS");
    }
    let bytes = received.lock().await;
    let haystack = String::from_utf8_lossy(&bytes);
    assert!(
        !haystack.contains(SECRET),
        "verify-full must not send the password before TLS succeeds: {haystack:?}"
    );
}

#[tokio::test]
async fn pg_secure_modes_reject_ssl_refusal_before_startup_or_authentication() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    for mode in [
        PostgresTransportMode::VerifyFull,
        PostgresTransportMode::EncryptedTrustServerCert,
    ] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let peer = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 8];
            socket.read_exact(&mut request).await.unwrap();
            assert_eq!(request, [0, 0, 0, 8, 4, 210, 22, 47]);
            // Refuse TLS, then request a cleartext password. No application bytes
            // (not even StartupMessage) may follow a secure-mode SSL refusal.
            socket.write_all(b"NR\0\0\0\x08\0\0\0\x03").await.unwrap();
            let mut remaining = Vec::new();
            let _ =
                tokio::time::timeout(Duration::from_secs(2), socket.read_to_end(&mut remaining))
                    .await;
            remaining
        });
        let result = pg_open_with_timeout(
            DriverEndpoint::direct(address.ip().to_string(), address.port()),
            "app".into(),
            "alice".into(),
            "must-not-leak".to_string().into(),
            mode,
            None,
            true,
            Duration::from_secs(2),
        )
        .await;
        assert!(result.is_err());
        assert!(
            peer.await.unwrap().is_empty(),
            "secure mode sent plaintext after TLS refusal"
        );
    }
}
