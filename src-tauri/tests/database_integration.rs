use std::collections::HashSet;
use std::path::Path;

use rusqlite::Connection as SqliteFixtureConnection;
use yuzora_lib::db_service::integration_harness::{IntegrationConnection, IntegrationRuntime};
use yuzora_lib::db_service::{
    DatabaseError, DatabaseErrorEngine, DatabaseObjectKind, DatabaseOperationalErrorCode, DbValue,
    EffectOutcome, QueryCancelOutcome, QueryExecutionUnit, QueryRun, QueryRunId,
    ResultPageDirection, ResultSession, ResultSessionLifecycle, StatementExecutionResult,
    TransactionBoundary,
};

const ROW_BOUNDARIES: [usize; 7] = [0, 499, 500, 501, 1000, 1001, 1201];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Engine {
    Sqlite,
    Postgres,
    Mssql,
}

impl Engine {
    const ALL: [Self; 3] = [Self::Sqlite, Self::Postgres, Self::Mssql];

    fn name(self) -> &'static str {
        match self {
            Self::Sqlite => "sqlite",
            Self::Postgres => "postgres",
            Self::Mssql => "mssql",
        }
    }
}

fn selected_engines() -> Result<HashSet<Engine>, &'static str> {
    let raw = match std::env::var("YUZORA_DATABASE_TEST_ENGINES") {
        Ok(raw) => raw,
        Err(std::env::VarError::NotPresent) => "sqlite".to_string(),
        Err(std::env::VarError::NotUnicode(_)) => return Err("engine=selection scenario=invalid"),
    };
    if raw.trim().is_empty() {
        return Err("engine=selection scenario=empty");
    }

    let mut selected = HashSet::new();
    for token in raw.split(',').map(str::trim) {
        let engine = match token {
            "sqlite" => Engine::Sqlite,
            "postgres" => Engine::Postgres,
            "mssql" => Engine::Mssql,
            _ => return Err("engine=selection scenario=unknown"),
        };
        selected.insert(engine);
    }
    Ok(selected)
}

fn scenario(engine: Engine, name: &str) {
    eprintln!("engine={} scenario={name}", engine.name());
}

fn create_sqlite_fixture(path: &Path) {
    let mut fixture = SqliteFixtureConnection::open(path).expect("create SQLite fixture");
    fixture
        .execute_batch(
            "CREATE TABLE boundary_rows (id INTEGER PRIMARY KEY, label TEXT NOT NULL);\
             CREATE TABLE typed_values (\
               id INTEGER PRIMARY KEY,\
               exact_int INTEGER NOT NULL,\
               exact_decimal TEXT NOT NULL,\
               nullable TEXT,\
               payload BLOB NOT NULL\
             );\
             INSERT INTO typed_values\
               (id, exact_int, exact_decimal, nullable, payload)\
             VALUES\
               (1, 9223372036854775807, '-0.123456789012345678901234567890', NULL, x'0001feff');\
             CREATE TABLE transaction_probe (id INTEGER PRIMARY KEY, value INTEGER NOT NULL);\
             INSERT INTO transaction_probe VALUES (1, 0);\
             CREATE TABLE dml_target (\
               id INTEGER PRIMARY KEY,\
               touched INTEGER NOT NULL DEFAULT 0\
             );\
             CREATE TABLE dml_audit (target_id INTEGER NOT NULL, touched INTEGER NOT NULL);\
             CREATE TRIGGER dml_target_audit \
             AFTER UPDATE ON dml_target \
             BEGIN \
               INSERT INTO dml_audit(target_id, touched) VALUES (NEW.id, NEW.touched);\
             END;\
             CREATE VIEW boundary_view AS SELECT id, label FROM boundary_rows;",
        )
        .expect("initialize SQLite fixture schema");

    let transaction = fixture.transaction().expect("start SQLite fixture load");
    {
        let mut insert = transaction
            .prepare("INSERT INTO boundary_rows (id, label) VALUES (?1, ?2)")
            .expect("prepare SQLite fixture load");
        let mut insert_dml = transaction
            .prepare("INSERT INTO dml_target (id) VALUES (?1)")
            .expect("prepare SQLite DML fixture load");
        for id in 1..=1201_i64 {
            insert
                .execute((id, format!("row-{id}")))
                .expect("insert SQLite fixture row");
            insert_dml
                .execute([id])
                .expect("insert SQLite DML fixture row");
        }
    }
    transaction.commit().expect("commit SQLite fixture load");
}

fn first_result_session(run: &QueryRun) -> ResultSession {
    match &run.statements[0].result {
        StatementExecutionResult::Rows {
            result_session: Some(session),
            ..
        }
        | StatementExecutionResult::ResultLimitReached {
            result_session: session,
            ..
        } => session.clone(),
        _ => panic!("expected a row-producing result session"),
    }
}

fn assert_integer_id(value: &DbValue, expected: usize) {
    assert_eq!(
        value,
        &DbValue::Integer {
            value: expected.to_string(),
        }
    );
}

fn integer_value(value: &DbValue) -> i64 {
    match value {
        DbValue::Integer { value } => value.parse().expect("parse database integer"),
        _ => panic!("expected integer database value"),
    }
}

fn statement_result_session(run: &QueryRun, statement_index: usize) -> ResultSession {
    match &run.statements[statement_index].result {
        StatementExecutionResult::Rows {
            result_session: Some(session),
            ..
        }
        | StatementExecutionResult::ResultLimitReached {
            result_session: session,
            ..
        } => session.clone(),
        _ => panic!("expected statement {statement_index} to produce rows"),
    }
}

fn statement_affected_rows(run: &QueryRun, statement_index: usize) -> Option<&str> {
    match &run.statements[statement_index].result {
        StatementExecutionResult::Rows { affected_rows, .. }
        | StatementExecutionResult::Execute { affected_rows }
        | StatementExecutionResult::ResultLimitReached { affected_rows, .. } => {
            affected_rows.as_deref()
        }
        _ => panic!("expected statement {statement_index} to complete successfully"),
    }
}

async fn read_single_integer(
    connection: &IntegrationConnection,
    query_run_id: impl Into<String>,
    sql: impl Into<String>,
) -> i64 {
    let run = connection
        .run_primary(query_run_id, sql)
        .await
        .expect("read integer probe");
    let session = first_result_session(&run);
    assert_eq!(session.initial_page.rows.len(), 1);
    let value = integer_value(&session.initial_page.rows[0][0]);
    connection
        .release_result(session.owner)
        .await
        .expect("release integer probe");
    value
}

async fn assert_select_1201_then_update(
    connection: &IntegrationConnection,
    engine: Engine,
    select_sql: &str,
    update_sql: &str,
    probe_sql: &str,
) {
    let engine_name = engine.name();
    let before = read_single_integer(
        connection,
        format!("{engine_name}-q6-probe-before"),
        probe_sql,
    )
    .await;
    let run = connection
        .run_script(
            format!("{engine_name}-q6-select-update"),
            vec![
                QueryExecutionUnit {
                    sql: select_sql.to_string(),
                    transaction_boundary: TransactionBoundary::None,
                },
                QueryExecutionUnit {
                    sql: update_sql.to_string(),
                    transaction_boundary: TransactionBoundary::None,
                },
            ],
        )
        .await
        .expect("run SELECT 1201 rows then UPDATE script");
    assert_eq!(run.statements.len(), 2);
    assert_eq!(run.statements[0].sql, select_sql);
    assert_eq!(run.statements[1].sql, update_sql);
    assert!(!run.transaction_may_be_open);

    let session = statement_result_session(&run, 0);
    assert_eq!(session.initial_page.page_index, 0);
    assert_eq!(session.initial_page.rows.len(), 500);
    assert!(session.initial_page.has_next);
    for (row_index, row) in session.initial_page.rows.iter().enumerate() {
        assert_integer_id(&row[0], row_index + 1);
    }
    let second = connection
        .result_page(session.owner.clone(), ResultPageDirection::Next)
        .await
        .expect("read materialized script second page");
    assert_eq!(second.page_index, 1);
    assert_eq!(second.rows.len(), 500);
    assert!(second.has_next);
    for (row_index, row) in second.rows.iter().enumerate() {
        assert_integer_id(&row[0], 501 + row_index);
    }
    let third = connection
        .result_page(session.owner.clone(), ResultPageDirection::Next)
        .await
        .expect("read materialized script terminal page");
    assert_eq!(third.page_index, 2);
    assert_eq!(third.rows.len(), 201);
    assert!(!third.has_next);
    assert_eq!(third.lifecycle, ResultSessionLifecycle::Complete);
    for (row_index, row) in third.rows.iter().enumerate() {
        assert_integer_id(&row[0], 1001 + row_index);
    }
    assert_eq!(statement_affected_rows(&run, 1), Some("1"));
    connection
        .release_result(session.owner)
        .await
        .expect("release SELECT 1201 script result");

    let after = read_single_integer(
        connection,
        format!("{engine_name}-q6-probe-after"),
        probe_sql,
    )
    .await;
    assert_eq!(after, before + 1);
}

async fn assert_sqlite_boundary(connection: &IntegrationConnection, row_count: usize) {
    let run = connection
        .run_primary(
            format!("sqlite-boundary-{row_count}"),
            format!("SELECT id FROM boundary_rows WHERE id <= {row_count} ORDER BY id"),
        )
        .await
        .expect("run SQLite boundary query");
    let session = first_result_session(&run);
    let expected_page_lengths = if row_count == 0 {
        vec![0]
    } else {
        let mut lengths = vec![500; row_count / 500];
        if !row_count.is_multiple_of(500) {
            lengths.push(row_count % 500);
        }
        lengths
    };

    let mut page = session.initial_page.clone();
    for (page_index, expected_len) in expected_page_lengths.iter().copied().enumerate() {
        if page_index != 0 {
            page = connection
                .result_page(session.owner.clone(), ResultPageDirection::Next)
                .await
                .expect("advance SQLite result page");
        }
        assert_eq!(page.page_index, page_index);
        assert_eq!(page.rows.len(), expected_len);
        assert_eq!(page.has_previous, page_index != 0);
        assert_eq!(page.has_next, page_index + 1 < expected_page_lengths.len());
        for (row_index, row) in page.rows.iter().enumerate() {
            assert_integer_id(&row[0], page_index * 500 + row_index + 1);
        }
    }

    if expected_page_lengths.len() > 1 {
        let previous = connection
            .result_page(session.owner.clone(), ResultPageDirection::Previous)
            .await
            .expect("read cached SQLite previous page");
        assert_eq!(previous.page_index, expected_page_lengths.len() - 2);
        let restored = connection
            .result_page(session.owner.clone(), ResultPageDirection::Next)
            .await
            .expect("restore cached SQLite next page");
        assert_eq!(restored.page_index, expected_page_lengths.len() - 1);
    }

    let released = connection
        .release_result(session.owner)
        .await
        .expect("release SQLite result session");
    assert_eq!(released.lifecycle, ResultSessionLifecycle::Released);
}

async fn run_sqlite() {
    let fixture_dir = tempfile::tempdir().expect("create SQLite fixture directory");
    let fixture_path = fixture_dir.path().join("database-integration.sqlite");
    let missing_path = fixture_dir.path().join("must-not-be-created.sqlite");
    create_sqlite_fixture(&fixture_path);
    let fixture_path = fixture_path.to_string_lossy().into_owned();
    let missing_path_string = missing_path.to_string_lossy().into_owned();
    let runtime = IntegrationRuntime::default();

    scenario(Engine::Sqlite, "missing-path");
    let missing_error = match runtime
        .open_sqlite("sqlite-missing", missing_path_string)
        .await
    {
        Ok(connection) => {
            let _ = connection.close();
            panic!("missing SQLite path unexpectedly opened");
        }
        Err(error) => error,
    };
    assert_eq!(
        missing_error.code,
        DatabaseOperationalErrorCode::SqlitePathMissing
    );
    assert!(
        !missing_path.exists(),
        "missing SQLite path must not be created"
    );

    scenario(Engine::Sqlite, "test-connection");
    let version = runtime
        .test_sqlite(fixture_path.clone())
        .await
        .expect("test existing SQLite fixture");
    assert!(version.is_some_and(|version| !version.is_empty()));

    scenario(Engine::Sqlite, "open-list-columns-query");
    let connection = runtime
        .open_sqlite("sqlite-main", fixture_path.clone())
        .await
        .expect("open existing SQLite fixture");
    assert!(connection.is_registered());
    let tables = connection.list_tables().await.expect("list SQLite objects");
    let boundary_table = tables
        .iter()
        .find(|table| {
            table.schema == "main"
                && table.name == "boundary_rows"
                && table.kind == DatabaseObjectKind::Table
        })
        .cloned()
        .expect("find qualified SQLite table");
    assert!(tables.iter().any(|table| {
        table.schema == "main"
            && table.name == "boundary_view"
            && table.kind == DatabaseObjectKind::View
    }));
    let columns = connection
        .table_columns(boundary_table)
        .await
        .expect("load SQLite columns");
    assert_eq!(
        columns
            .iter()
            .map(|column| column.name.as_str())
            .collect::<Vec<_>>(),
        ["id", "label"]
    );
    assert!(columns[0].pk);
    assert!(columns[1].notnull);

    let values = connection
        .run_primary(
            "sqlite-lossless-values",
            "SELECT exact_int, exact_decimal, nullable, payload FROM typed_values",
        )
        .await
        .expect("query SQLite values");
    let values_session = first_result_session(&values);
    assert_eq!(
        values_session.initial_page.rows,
        vec![vec![
            DbValue::Integer {
                value: "9223372036854775807".to_string(),
            },
            DbValue::Text {
                value: "-0.123456789012345678901234567890".to_string(),
            },
            DbValue::Null,
            DbValue::Binary {
                hex: "0001feff".to_string(),
            },
        ]]
    );
    connection
        .release_result(values_session.owner)
        .await
        .expect("release SQLite value session");

    scenario(Engine::Sqlite, "pagination-boundaries");
    for row_count in ROW_BOUNDARIES {
        assert_sqlite_boundary(&connection, row_count).await;
    }

    scenario(Engine::Sqlite, "busy-release");
    let busy_run = connection
        .run_primary("sqlite-busy", "SELECT id FROM boundary_rows ORDER BY id")
        .await
        .expect("start SQLite lazy result");
    let busy_session = first_result_session(&busy_run);
    assert!(busy_session.initial_page.has_next);
    let busy_error = connection
        .list_tables()
        .await
        .expect_err("metadata must fail while SQLite result owns the lease");
    assert_eq!(
        busy_error.code,
        DatabaseOperationalErrorCode::ConnectionBusy
    );
    let released = connection
        .release_result(busy_session.owner.clone())
        .await
        .expect("release active SQLite result");
    assert_eq!(released.lifecycle, ResultSessionLifecycle::Released);
    let released_next = connection
        .result_page(busy_session.owner, ResultPageDirection::Next)
        .await
        .expect_err("released SQLite result must not advance");
    assert_eq!(
        released_next.code,
        DatabaseOperationalErrorCode::StaleConnection
    );
    assert!(!connection
        .list_tables()
        .await
        .expect("metadata resumes after SQLite release")
        .is_empty());

    scenario(Engine::Sqlite, "row-producing-dml-effect-once");
    let before = connection
        .run_primary(
            "sqlite-dml-before",
            "SELECT (SELECT count(*) FROM dml_audit), min(touched), max(touched) FROM dml_target",
        )
        .await
        .expect("read SQLite DML baseline");
    let before_session = first_result_session(&before);
    let before_audit = integer_value(&before_session.initial_page.rows[0][0]);
    let before_min = integer_value(&before_session.initial_page.rows[0][1]);
    let before_max = integer_value(&before_session.initial_page.rows[0][2]);
    assert_eq!(before_min, before_max);
    connection
        .release_result(before_session.owner)
        .await
        .expect("release SQLite DML baseline");

    let dml = connection
        .run_primary(
            "sqlite-returning-1201",
            "UPDATE dml_target SET touched = touched + 1 RETURNING id, touched",
        )
        .await
        .expect("run SQLite row-producing DML");
    assert_eq!(dml.statements[0].effect_outcome, EffectOutcome::Unknown);
    let dml_session = first_result_session(&dml);
    assert_eq!(dml_session.initial_page.rows.len(), 500);
    assert!(dml_session.initial_page.has_next);
    let dml_second = connection
        .result_page(dml_session.owner.clone(), ResultPageDirection::Next)
        .await
        .expect("read SQLite DML second page");
    let dml_third = connection
        .result_page(dml_session.owner.clone(), ResultPageDirection::Next)
        .await
        .expect("read SQLite DML terminal page");
    assert_eq!(dml_second.rows.len(), 500);
    assert_eq!(dml_third.rows.len(), 201);
    assert_eq!(dml_third.lifecycle, ResultSessionLifecycle::Complete);
    assert_eq!(dml_third.effect_outcome, EffectOutcome::Committed);
    let returned_ids = dml_session
        .initial_page
        .rows
        .iter()
        .chain(dml_second.rows.iter())
        .chain(dml_third.rows.iter())
        .map(|row| integer_value(&row[0]))
        .collect::<HashSet<_>>();
    assert_eq!(returned_ids.len(), 1201);
    assert!(returned_ids.contains(&1));
    assert!(returned_ids.contains(&1201));
    assert!(dml_session
        .initial_page
        .rows
        .iter()
        .chain(dml_second.rows.iter())
        .chain(dml_third.rows.iter())
        .all(|row| integer_value(&row[1]) == before_min + 1));
    let dml_previous = connection
        .result_page(dml_session.owner.clone(), ResultPageDirection::Previous)
        .await
        .expect("read cached SQLite DML previous page");
    assert_eq!(dml_previous.rows, dml_second.rows);
    let dml_released = connection
        .release_result(dml_session.owner)
        .await
        .expect("release SQLite DML result");
    assert_eq!(dml_released.lifecycle, ResultSessionLifecycle::Released);
    assert_eq!(dml_released.effect_outcome, EffectOutcome::Committed);

    let after = connection
        .run_primary(
            "sqlite-dml-after",
            "SELECT (SELECT count(*) FROM dml_audit), min(touched), max(touched) FROM dml_target",
        )
        .await
        .expect("verify SQLite DML effect");
    let after_session = first_result_session(&after);
    assert_eq!(
        integer_value(&after_session.initial_page.rows[0][0]),
        before_audit + 1201
    );
    assert_eq!(
        integer_value(&after_session.initial_page.rows[0][1]),
        before_min + 1
    );
    assert_eq!(
        integer_value(&after_session.initial_page.rows[0][2]),
        before_min + 1
    );
    connection
        .release_result(after_session.owner)
        .await
        .expect("release SQLite DML verification");

    scenario(Engine::Sqlite, "q6-select-1201-then-update");
    assert_select_1201_then_update(
        &connection,
        Engine::Sqlite,
        "SELECT id FROM boundary_rows ORDER BY id",
        "UPDATE transaction_probe SET value = value + 1 WHERE id = 1",
        "SELECT value FROM transaction_probe WHERE id = 1",
    )
    .await;

    scenario(Engine::Sqlite, "cancel-a-run-b");
    let cancel_id = QueryRunId("sqlite-cancel-a".to_string());
    let run_connection = connection.clone();
    let run_id = cancel_id.clone();
    let run_a = tokio::spawn(async move {
        run_connection
            .run_primary(
                run_id.0,
                "WITH RECURSIVE probe(n) AS (VALUES(0) UNION ALL SELECT n + 1 FROM probe WHERE n < 100000000) SELECT sum(n) FROM probe",
            )
            .await
    });
    tokio::task::yield_now().await;
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let early_b = connection
        .run_primary("sqlite-run-b-early", "SELECT 42")
        .await
        .expect_err("SQLite Run B must not start before Run A settles");
    assert_eq!(early_b.code, DatabaseOperationalErrorCode::ConnectionBusy);
    let cancel = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        connection.cancel(cancel_id),
    )
    .await
    .expect("SQLite cancel command timeout")
    .expect("cancel exact SQLite query owner");
    assert_eq!(cancel.outcome, QueryCancelOutcome::Cancelled);
    let cancelled = tokio::time::timeout(std::time::Duration::from_secs(10), run_a)
        .await
        .expect("SQLite cancelled query settlement timeout")
        .expect("SQLite cancelled query task")
        .expect("SQLite cancelled query result");
    assert!(matches!(
        cancelled.statements[0].result,
        StatementExecutionResult::Cancelled { .. }
    ));
    let cancel_error = first_statement_error(&cancelled);
    assert_eq!(cancel_error.engine, DatabaseErrorEngine::Sqlite);
    let run_b = connection
        .run_primary("sqlite-run-b", "SELECT 42")
        .await
        .expect("SQLite Run B starts after Run A settlement");
    let run_b_session = first_result_session(&run_b);
    assert_integer_id(&run_b_session.initial_page.rows[0][0], 42);
    connection
        .release_result(run_b_session.owner)
        .await
        .expect("release SQLite Run B");

    scenario(Engine::Sqlite, "close-reconnect-stale-owner");
    let stale_run = connection
        .run_primary(
            "sqlite-stale-owner",
            "SELECT id FROM boundary_rows ORDER BY id",
        )
        .await
        .expect("start SQLite result before reconnect");
    let stale_owner = first_result_session(&stale_run).owner;
    connection
        .release_result(stale_owner.clone())
        .await
        .expect("release SQLite result before close");
    let old_identity = connection.identity();
    assert!(connection.close().expect("close SQLite fixture").closed);
    assert!(!connection.is_registered());

    let reconnected = runtime
        .open_sqlite("sqlite-main", fixture_path)
        .await
        .expect("reconnect SQLite fixture");
    assert_ne!(
        reconnected.identity().connection_generation,
        old_identity.connection_generation
    );
    let stale_error = reconnected
        .result_page(stale_owner, ResultPageDirection::Next)
        .await
        .expect_err("old SQLite result owner must stay stale after reconnect");
    assert_eq!(
        stale_error.code,
        DatabaseOperationalErrorCode::StaleConnection
    );
    assert!(!reconnected
        .list_tables()
        .await
        .expect("new SQLite actor remains usable")
        .is_empty());
    assert!(
        reconnected
            .close()
            .expect("close reconnected SQLite")
            .closed
    );
}

fn database_password(engine: Engine) -> String {
    std::env::var("YUZORA_P8_DATABASE_PASSWORD")
        .unwrap_or_else(|_| panic!("engine={} scenario=missing-password", engine.name()))
}

fn first_statement_error(run: &QueryRun) -> DatabaseError {
    match &run.statements[0].result {
        StatementExecutionResult::Error { error }
        | StatementExecutionResult::Cancelled { error } => error.clone(),
        _ => panic!("expected a database statement error"),
    }
}

async fn assert_postgres_boundary(connection: &IntegrationConnection, row_count: usize) {
    let run = connection
        .run_primary(
            format!("postgres-boundary-{row_count}"),
            format!("SELECT id FROM alpha.rows_{row_count} ORDER BY id"),
        )
        .await
        .expect("run PostgreSQL boundary query");
    let session = first_result_session(&run);
    let expected_page_lengths = if row_count == 0 {
        vec![0]
    } else {
        let mut lengths = vec![500; row_count / 500];
        if !row_count.is_multiple_of(500) {
            lengths.push(row_count % 500);
        }
        lengths
    };
    let mut page = session.initial_page.clone();
    for (page_index, expected_len) in expected_page_lengths.iter().copied().enumerate() {
        if page_index != 0 {
            page = connection
                .result_page(session.owner.clone(), ResultPageDirection::Next)
                .await
                .expect("advance PostgreSQL boundary page");
        }
        assert_eq!(page.page_index, page_index);
        assert_eq!(page.rows.len(), expected_len);
        assert_eq!(page.has_previous, page_index != 0);
        assert_eq!(page.has_next, page_index + 1 < expected_page_lengths.len());
        if row_count != 0 {
            assert!(
                !page.rows.is_empty(),
                "PostgreSQL exposed a blank boundary page"
            );
        }
        for (row_index, row) in page.rows.iter().enumerate() {
            assert_integer_id(&row[0], page_index * 500 + row_index + 1);
        }
    }
    assert_eq!(page.lifecycle, ResultSessionLifecycle::Complete);
    let released = connection
        .release_result(session.owner)
        .await
        .expect("release PostgreSQL boundary result");
    assert_eq!(released.lifecycle, ResultSessionLifecycle::Released);
}

async fn run_postgres() {
    const HOST: &str = "127.0.0.1";
    const PORT: u16 = 55432;
    const DATABASE: &str = "yuzora_p8";
    const FULL_USER: &str = "yuzora_full";
    const READONLY_USER: &str = "yuzora_readonly";

    let runtime = IntegrationRuntime::default();
    let password = database_password(Engine::Postgres);

    scenario(Engine::Postgres, "tls-strict-rejects-self-signed");
    let strict_error = runtime
        .test_postgres(
            HOST,
            PORT,
            DATABASE,
            FULL_USER,
            password.clone(),
            true,
            false,
        )
        .await
        .expect_err("strict PostgreSQL TLS must reject the fixture certificate");
    assert_eq!(
        strict_error.code,
        DatabaseOperationalErrorCode::ConnectionFailed
    );
    let strict_open_error = match runtime
        .open_postgres(
            "postgres-strict",
            HOST,
            PORT,
            DATABASE,
            FULL_USER,
            password.clone(),
            true,
            false,
        )
        .await
    {
        Ok(connection) => {
            let _ = connection.close();
            panic!("strict PostgreSQL TLS unexpectedly opened");
        }
        Err(error) => error,
    };
    assert_eq!(
        strict_open_error.code,
        DatabaseOperationalErrorCode::ConnectionFailed
    );

    scenario(Engine::Postgres, "tls-trust-test-open");
    let version = runtime
        .test_postgres(
            HOST,
            PORT,
            DATABASE,
            FULL_USER,
            password.clone(),
            true,
            true,
        )
        .await
        .expect("trusted PostgreSQL test connection");
    assert!(version.is_some_and(|version| version.contains("PostgreSQL")));
    let connection = runtime
        .open_postgres(
            "postgres-main",
            HOST,
            PORT,
            DATABASE,
            FULL_USER,
            password.clone(),
            true,
            true,
        )
        .await
        .expect("open trusted PostgreSQL connection");

    scenario(Engine::Postgres, "qualified-objects-columns");
    let objects = connection
        .list_tables()
        .await
        .expect("list PostgreSQL objects");
    assert_eq!(
        objects
            .iter()
            .filter(|object| object.schema == "alpha")
            .count(),
        58
    );
    let alpha_shared = objects
        .iter()
        .find(|object| {
            object.catalog == DATABASE
                && object.schema == "alpha"
                && object.name == "shared_name"
                && object.kind == DatabaseObjectKind::Table
        })
        .cloned()
        .expect("find alpha.shared_name");
    let audit_shared = objects
        .iter()
        .find(|object| {
            object.catalog == DATABASE
                && object.schema == "audit"
                && object.name == "shared_name"
                && object.kind == DatabaseObjectKind::Table
        })
        .cloned()
        .expect("find audit.shared_name");
    let alpha_columns = connection
        .table_columns(alpha_shared)
        .await
        .expect("load alpha.shared_name columns");
    let audit_columns = connection
        .table_columns(audit_shared)
        .await
        .expect("load audit.shared_name columns");
    assert_eq!(
        alpha_columns
            .iter()
            .map(|column| column.name.as_str())
            .collect::<Vec<_>>(),
        ["id", "source"]
    );
    assert_eq!(
        audit_columns
            .iter()
            .map(|column| column.name.as_str())
            .collect::<Vec<_>>(),
        ["id", "source", "audit_only"]
    );

    scenario(Engine::Postgres, "qualified-permissions");
    let readonly = runtime
        .open_postgres(
            "postgres-readonly",
            HOST,
            PORT,
            DATABASE,
            READONLY_USER,
            password.clone(),
            true,
            true,
        )
        .await
        .expect("open readonly PostgreSQL connection");
    assert_ne!(
        readonly.identity().descriptor_id,
        connection.identity().descriptor_id
    );
    assert_ne!(
        readonly.identity().connection_id,
        connection.identity().connection_id
    );
    assert_ne!(
        readonly.identity().connection_generation,
        connection.identity().connection_generation
    );
    let readonly_objects = readonly
        .list_tables()
        .await
        .expect("list readonly PostgreSQL objects");
    assert!(readonly_objects
        .iter()
        .any(|object| object.schema == "alpha" && object.name == "shared_name"));
    assert!(!readonly_objects
        .iter()
        .any(|object| object.schema == "audit"));
    let readonly_alpha = readonly_objects
        .iter()
        .find(|object| {
            object.catalog == DATABASE
                && object.schema == "alpha"
                && object.name == "shared_name"
                && object.kind == DatabaseObjectKind::Table
        })
        .cloned()
        .expect("find readonly alpha.shared_name");
    let readonly_columns = readonly
        .table_columns(readonly_alpha)
        .await
        .expect("load readonly alpha.shared_name columns");
    assert_eq!(
        readonly_columns
            .iter()
            .map(|column| column.name.as_str())
            .collect::<Vec<_>>(),
        ["id", "source"]
    );
    let denied = readonly
        .run_primary(
            "postgres-readonly-denied",
            "SELECT id FROM audit.shared_name",
        )
        .await
        .expect("return structured PostgreSQL permission error");
    let denied_error = first_statement_error(&denied);
    assert_eq!(denied_error.engine, DatabaseErrorEngine::Postgres);
    assert_eq!(denied_error.code.as_deref(), Some("42501"));
    let allowed = readonly
        .run_primary(
            "postgres-readonly-allowed",
            "SELECT id FROM alpha.shared_name ORDER BY id",
        )
        .await
        .expect("query permitted PostgreSQL object");
    let allowed_session = first_result_session(&allowed);
    assert_eq!(allowed_session.initial_page.rows.len(), 1);
    readonly
        .release_result(allowed_session.owner)
        .await
        .expect("release readonly PostgreSQL result");
    assert!(readonly.close().expect("close readonly PostgreSQL").closed);

    scenario(Engine::Postgres, "lossless-values-error-position");
    let values = connection
        .run_primary(
            "postgres-lossless-values",
            "SELECT big_value, precise_numeric, date_value, time_value, timestamp_value, json_value, binary_value, nullable_value FROM alpha.value_extremes WHERE id = 1",
        )
        .await
        .expect("query PostgreSQL values");
    let values_session = first_result_session(&values);
    assert_eq!(
        values_session.initial_page.rows,
        vec![vec![
            DbValue::Integer {
                value: "9223372036854775807".to_string(),
            },
            DbValue::Decimal {
                value: "12345678901234567890.123456789012345678".to_string(),
            },
            DbValue::Date {
                value: "2024-02-29".to_string(),
            },
            DbValue::Time {
                value: "23:59:58.123456".to_string(),
            },
            DbValue::DateTime {
                value: "2024-02-29 12:34:56.123456".to_string(),
            },
            DbValue::Json {
                value: "{\"beyondU64\":18446744073709551616,\"label\":\"fixture\"}".to_string(),
            },
            DbValue::Binary {
                hex: "0001ff".to_string(),
            },
            DbValue::Null,
        ]]
    );
    connection
        .release_result(values_session.owner)
        .await
        .expect("release PostgreSQL value session");
    let syntax = connection
        .run_primary("postgres-syntax-error", "SELECT 1 FROM")
        .await
        .expect("return structured PostgreSQL syntax error");
    let syntax_error = first_statement_error(&syntax);
    assert_eq!(syntax_error.engine, DatabaseErrorEngine::Postgres);
    assert_eq!(syntax_error.code.as_deref(), Some("42601"));
    assert!(syntax_error
        .position
        .is_some_and(|position| position.offset.is_some_and(|offset| offset > 0)));

    scenario(Engine::Postgres, "lazy-1201-busy-previous-next-release");
    let lazy = connection
        .run_primary(
            "postgres-lazy-1201",
            "SELECT id FROM alpha.rows_1201 ORDER BY id",
        )
        .await
        .expect("start PostgreSQL lazy result");
    let lazy_session = first_result_session(&lazy);
    assert_eq!(lazy_session.initial_page.rows.len(), 500);
    assert!(lazy_session.initial_page.has_next);
    let busy = connection
        .list_tables()
        .await
        .expect_err("PostgreSQL metadata must fail while result owns lease");
    assert_eq!(busy.code, DatabaseOperationalErrorCode::ConnectionBusy);
    let second = connection
        .result_page(lazy_session.owner.clone(), ResultPageDirection::Next)
        .await
        .expect("read PostgreSQL second page");
    assert_eq!(second.page_index, 1);
    assert_eq!(second.rows.len(), 500);
    assert!(second.has_next);
    let third = connection
        .result_page(lazy_session.owner.clone(), ResultPageDirection::Next)
        .await
        .expect("read PostgreSQL terminal page");
    assert_eq!(third.page_index, 2);
    assert_eq!(third.rows.len(), 201);
    assert!(!third.has_next);
    assert_eq!(third.lifecycle, ResultSessionLifecycle::Complete);
    let previous = connection
        .result_page(lazy_session.owner.clone(), ResultPageDirection::Previous)
        .await
        .expect("read cached PostgreSQL previous page");
    assert_eq!(previous.page_index, 1);
    assert_eq!(previous.rows, second.rows);
    let released = connection
        .release_result(lazy_session.owner)
        .await
        .expect("release PostgreSQL result");
    assert_eq!(released.lifecycle, ResultSessionLifecycle::Released);

    scenario(Engine::Postgres, "exact-boundaries-all-seven");
    for row_count in ROW_BOUNDARIES {
        assert_postgres_boundary(&connection, row_count).await;
    }

    scenario(Engine::Postgres, "returning-1201-effect-release");
    let before = connection
        .run_primary(
            "postgres-dml-before",
            "SELECT min(touched), max(touched) FROM alpha.dml_target",
        )
        .await
        .expect("read PostgreSQL DML baseline");
    let before_session = first_result_session(&before);
    let before_min = integer_value(&before_session.initial_page.rows[0][0]);
    let before_max = integer_value(&before_session.initial_page.rows[0][1]);
    assert_eq!(before_min, before_max);
    connection
        .release_result(before_session.owner)
        .await
        .expect("release PostgreSQL DML baseline");

    let dml = connection
        .run_primary(
            "postgres-returning-1201",
            "UPDATE alpha.dml_target SET touched = touched + 1 RETURNING id, touched",
        )
        .await
        .expect("run PostgreSQL row-producing DML");
    assert_eq!(dml.statements[0].effect_outcome, EffectOutcome::Unknown);
    let dml_session = first_result_session(&dml);
    assert_eq!(dml_session.initial_page.rows.len(), 500);
    assert_eq!(
        integer_value(&dml_session.initial_page.rows[0][1]),
        before_min + 1
    );
    let dml_second = connection
        .result_page(dml_session.owner.clone(), ResultPageDirection::Next)
        .await
        .expect("read PostgreSQL DML second page");
    assert_eq!(dml_second.rows.len(), 500);
    let dml_third = connection
        .result_page(dml_session.owner.clone(), ResultPageDirection::Next)
        .await
        .expect("read PostgreSQL DML terminal page");
    assert_eq!(dml_third.rows.len(), 201);
    assert_eq!(dml_third.lifecycle, ResultSessionLifecycle::Complete);
    assert_eq!(dml_third.effect_outcome, EffectOutcome::Unknown);
    let returned_ids = dml_session
        .initial_page
        .rows
        .iter()
        .chain(dml_second.rows.iter())
        .chain(dml_third.rows.iter())
        .map(|row| integer_value(&row[0]))
        .collect::<HashSet<_>>();
    assert_eq!(returned_ids.len(), 1201);
    assert!(returned_ids.contains(&1));
    assert!(returned_ids.contains(&1201));
    assert!(dml_session
        .initial_page
        .rows
        .iter()
        .chain(dml_second.rows.iter())
        .chain(dml_third.rows.iter())
        .all(|row| integer_value(&row[1]) == before_min + 1));
    let dml_previous = connection
        .result_page(dml_session.owner.clone(), ResultPageDirection::Previous)
        .await
        .expect("read cached PostgreSQL DML previous page");
    assert_eq!(dml_previous.rows, dml_second.rows);
    let dml_released = connection
        .release_result(dml_session.owner)
        .await
        .expect("release PostgreSQL DML result");
    assert_eq!(dml_released.lifecycle, ResultSessionLifecycle::Released);
    assert_eq!(dml_released.effect_outcome, EffectOutcome::Unknown);

    let after = connection
        .run_primary(
            "postgres-dml-after",
            "SELECT count(*), min(touched), max(touched) FROM alpha.dml_target",
        )
        .await
        .expect("verify PostgreSQL DML effect");
    let after_session = first_result_session(&after);
    assert_eq!(integer_value(&after_session.initial_page.rows[0][0]), 1201);
    assert_eq!(
        integer_value(&after_session.initial_page.rows[0][1]),
        before_min + 1
    );
    assert_eq!(
        integer_value(&after_session.initial_page.rows[0][2]),
        before_min + 1
    );
    connection
        .release_result(after_session.owner)
        .await
        .expect("release PostgreSQL DML verification");

    scenario(Engine::Postgres, "ordered-transaction-script");
    let transaction = connection
        .run_script(
            "postgres-transaction-script",
            vec![
                QueryExecutionUnit {
                    sql: "BEGIN".to_string(),
                    transaction_boundary: TransactionBoundary::Begin,
                },
                QueryExecutionUnit {
                    sql: "SELECT value FROM alpha.transaction_probe WHERE id = 1".to_string(),
                    transaction_boundary: TransactionBoundary::None,
                },
                QueryExecutionUnit {
                    sql: "COMMIT".to_string(),
                    transaction_boundary: TransactionBoundary::Commit,
                },
            ],
        )
        .await
        .expect("run ordered PostgreSQL transaction script");
    assert_eq!(transaction.statements.len(), 3);
    assert_eq!(transaction.statements[0].sql, "BEGIN");
    assert_eq!(
        transaction.statements[1].sql,
        "SELECT value FROM alpha.transaction_probe WHERE id = 1"
    );
    assert_eq!(transaction.statements[2].sql, "COMMIT");
    assert!(!transaction.transaction_may_be_open);
    assert!(matches!(
        &transaction.statements[0].result,
        StatementExecutionResult::Execute { .. }
    ));
    assert!(matches!(
        &transaction.statements[2].result,
        StatementExecutionResult::Execute { .. }
    ));
    let transaction_session = match &transaction.statements[1].result {
        StatementExecutionResult::Rows {
            result_session: Some(session),
            ..
        } => session.clone(),
        _ => panic!("expected transaction SELECT result session"),
    };
    assert_eq!(transaction_session.initial_page.rows.len(), 1);
    assert_integer_id(&transaction_session.initial_page.rows[0][0], 0);
    assert_eq!(
        transaction_session.initial_page.lifecycle,
        ResultSessionLifecycle::Complete
    );
    connection
        .release_result(transaction_session.owner)
        .await
        .expect("release PostgreSQL transaction SELECT result");

    scenario(Engine::Postgres, "q6-select-1201-then-update");
    assert_select_1201_then_update(
        &connection,
        Engine::Postgres,
        "SELECT id FROM alpha.rows_1201 ORDER BY id",
        "UPDATE alpha.transaction_probe SET value = value + 1 WHERE id = 1",
        "SELECT value FROM alpha.transaction_probe WHERE id = 1",
    )
    .await;

    scenario(Engine::Postgres, "cancel-a-run-b");
    let cancel_id = QueryRunId("postgres-cancel-a".to_string());
    let cancel_connection = connection.clone();
    let cancel_id_for_run = cancel_id.clone();
    let run_a = tokio::spawn(async move {
        cancel_connection
            .run_primary(cancel_id_for_run.0, "SELECT alpha.long_query(30)")
            .await
    });
    tokio::task::yield_now().await;
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let cancel_connection = connection.clone();
    let cancel_task = tokio::spawn(async move { cancel_connection.cancel(cancel_id).await });
    tokio::task::yield_now().await;
    let early_b = connection
        .run_primary("postgres-run-b-early", "SELECT 42")
        .await
        .expect_err("Run B must wait for cancelled Run A settlement");
    assert_eq!(early_b.code, DatabaseOperationalErrorCode::ConnectionBusy);
    let cancel = tokio::time::timeout(std::time::Duration::from_secs(10), cancel_task)
        .await
        .expect("PostgreSQL cancel command timeout")
        .expect("PostgreSQL cancel command task")
        .expect("cancel exact PostgreSQL query owner");
    assert_eq!(cancel.outcome, QueryCancelOutcome::Cancelled);
    let cancelled = tokio::time::timeout(std::time::Duration::from_secs(10), run_a)
        .await
        .expect("PostgreSQL cancelled query settlement timeout")
        .expect("PostgreSQL cancelled query task")
        .expect("PostgreSQL cancelled query result");
    assert!(matches!(
        cancelled.statements[0].result,
        StatementExecutionResult::Cancelled { .. }
    ));
    let run_b = connection
        .run_primary("postgres-run-b", "SELECT 42")
        .await
        .expect("Run B starts after PostgreSQL Run A settlement");
    let run_b_session = first_result_session(&run_b);
    assert_integer_id(&run_b_session.initial_page.rows[0][0], 42);
    connection
        .release_result(run_b_session.owner)
        .await
        .expect("release PostgreSQL Run B");

    scenario(Engine::Postgres, "close-reconnect-stale-owner");
    let stale = connection
        .run_primary(
            "postgres-stale-owner",
            "SELECT id FROM alpha.rows_1201 ORDER BY id",
        )
        .await
        .expect("start PostgreSQL stale-owner result");
    let stale_owner = first_result_session(&stale).owner;
    connection
        .release_result(stale_owner.clone())
        .await
        .expect("release PostgreSQL result before close");
    let old_identity = connection.identity();
    assert!(connection.close().expect("close PostgreSQL actor").closed);
    let reconnected = runtime
        .open_postgres(
            "postgres-main",
            HOST,
            PORT,
            DATABASE,
            FULL_USER,
            password,
            true,
            true,
        )
        .await
        .expect("reconnect PostgreSQL actor");
    assert_ne!(
        reconnected.identity().connection_generation,
        old_identity.connection_generation
    );
    let stale_error = reconnected
        .result_page(stale_owner, ResultPageDirection::Next)
        .await
        .expect_err("old PostgreSQL owner must stay stale after reconnect");
    assert_eq!(
        stale_error.code,
        DatabaseOperationalErrorCode::StaleConnection
    );
    let reconnect_query = reconnected
        .run_primary("postgres-reconnect-query", "SELECT 7")
        .await
        .expect("query reconnected PostgreSQL actor");
    let reconnect_session = first_result_session(&reconnect_query);
    assert_integer_id(&reconnect_session.initial_page.rows[0][0], 7);
    reconnected
        .release_result(reconnect_session.owner)
        .await
        .expect("release reconnected PostgreSQL result");
    assert!(
        reconnected
            .close()
            .expect("close reconnected PostgreSQL")
            .closed
    );
}

async fn assert_mssql_boundary(connection: &IntegrationConnection, row_count: usize) {
    let run = connection
        .run_primary(
            format!("mssql-boundary-{row_count}"),
            format!("SELECT id FROM alpha.rows_{row_count} ORDER BY id"),
        )
        .await
        .expect("run MSSQL boundary query");
    let session = first_result_session(&run);
    let expected_page_lengths = if row_count == 0 {
        vec![0]
    } else {
        let mut lengths = vec![500; row_count / 500];
        if !row_count.is_multiple_of(500) {
            lengths.push(row_count % 500);
        }
        lengths
    };
    let mut page = session.initial_page.clone();
    for (page_index, expected_len) in expected_page_lengths.iter().copied().enumerate() {
        if page_index != 0 {
            page = connection
                .result_page(session.owner.clone(), ResultPageDirection::Next)
                .await
                .expect("advance MSSQL boundary page");
        }
        assert_eq!(page.page_index, page_index);
        assert_eq!(page.rows.len(), expected_len);
        assert_eq!(page.has_previous, page_index != 0);
        assert_eq!(page.has_next, page_index + 1 < expected_page_lengths.len());
        if row_count != 0 {
            assert!(!page.rows.is_empty(), "MSSQL exposed a blank boundary page");
        }
        for (row_index, row) in page.rows.iter().enumerate() {
            assert_integer_id(&row[0], page_index * 500 + row_index + 1);
        }
    }
    assert_eq!(page.lifecycle, ResultSessionLifecycle::Complete);
    let released = connection
        .release_result(session.owner)
        .await
        .expect("release MSSQL boundary result");
    assert_eq!(released.lifecycle, ResultSessionLifecycle::Released);
}

fn first_affected_rows(run: &QueryRun) -> Option<&str> {
    match &run.statements[0].result {
        StatementExecutionResult::Execute { affected_rows } => affected_rows.as_deref(),
        _ => panic!("expected a non-row-producing execution"),
    }
}

async fn run_mssql() {
    if !cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        panic!("engine=mssql scenario=requires-linux-x86_64");
    }

    const HOST: &str = "127.0.0.1";
    const PORT: u16 = 51433;
    const DATABASE: &str = "yuzora_p8";
    const FULL_USER: &str = "yuzora_full";
    const READONLY_USER: &str = "yuzora_readonly";

    let runtime = IntegrationRuntime::default();
    let password = database_password(Engine::Mssql);

    scenario(Engine::Mssql, "tls-strict-rejects-self-signed");
    let strict_error = runtime
        .test_mssql(HOST, PORT, DATABASE, FULL_USER, password.clone(), false)
        .await
        .expect_err("strict MSSQL trust must reject the fixture certificate");
    assert_eq!(
        strict_error.code,
        DatabaseOperationalErrorCode::ConnectionFailed
    );
    let strict_open_error = match runtime
        .open_mssql(
            "mssql-strict",
            HOST,
            PORT,
            DATABASE,
            FULL_USER,
            password.clone(),
            false,
        )
        .await
    {
        Ok(connection) => {
            let _ = connection.close();
            panic!("strict MSSQL trust unexpectedly opened");
        }
        Err(error) => error,
    };
    assert_eq!(
        strict_open_error.code,
        DatabaseOperationalErrorCode::ConnectionFailed
    );

    scenario(Engine::Mssql, "tls-trust-test-open");
    let version = runtime
        .test_mssql(HOST, PORT, DATABASE, FULL_USER, password.clone(), true)
        .await
        .expect("test MSSQL fixture connection");
    assert!(version.is_some_and(|version| version.contains("Microsoft SQL Server")));
    let connection = runtime
        .open_mssql(
            "mssql-main",
            HOST,
            PORT,
            DATABASE,
            FULL_USER,
            password.clone(),
            true,
        )
        .await
        .expect("open MSSQL fixture connection");

    scenario(Engine::Mssql, "qualified-objects-columns");
    let objects = connection.list_tables().await.expect("list MSSQL objects");
    assert_eq!(
        objects
            .iter()
            .filter(|object| {
                object.schema == "alpha"
                    && object.name.starts_with("object_")
                    && object.kind == DatabaseObjectKind::Table
            })
            .count(),
        45
    );
    let alpha_shared = objects
        .iter()
        .find(|object| {
            object.catalog == DATABASE
                && object.schema == "alpha"
                && object.name == "shared_name"
                && object.kind == DatabaseObjectKind::Table
        })
        .cloned()
        .expect("find alpha.shared_name in MSSQL");
    let audit_shared = objects
        .iter()
        .find(|object| {
            object.catalog == DATABASE
                && object.schema == "audit"
                && object.name == "shared_name"
                && object.kind == DatabaseObjectKind::Table
        })
        .cloned()
        .expect("find audit.shared_name in MSSQL");
    let alpha_columns = connection
        .table_columns(alpha_shared)
        .await
        .expect("load MSSQL alpha.shared_name columns");
    let audit_columns = connection
        .table_columns(audit_shared)
        .await
        .expect("load MSSQL audit.shared_name columns");
    assert_eq!(
        alpha_columns
            .iter()
            .map(|column| column.name.as_str())
            .collect::<Vec<_>>(),
        ["id", "source"]
    );
    assert_eq!(
        audit_columns
            .iter()
            .map(|column| column.name.as_str())
            .collect::<Vec<_>>(),
        ["id", "source", "audit_only"]
    );

    scenario(Engine::Mssql, "qualified-permissions");
    let readonly = runtime
        .open_mssql(
            "mssql-readonly",
            HOST,
            PORT,
            DATABASE,
            READONLY_USER,
            password.clone(),
            true,
        )
        .await
        .expect("open readonly MSSQL connection");
    assert_ne!(
        readonly.identity().descriptor_id,
        connection.identity().descriptor_id
    );
    assert_ne!(
        readonly.identity().connection_id,
        connection.identity().connection_id
    );
    assert_ne!(
        readonly.identity().connection_generation,
        connection.identity().connection_generation
    );
    let readonly_objects = readonly
        .list_tables()
        .await
        .expect("list readonly MSSQL objects");
    assert!(readonly_objects
        .iter()
        .any(|object| object.schema == "alpha" && object.name == "shared_name"));
    assert!(!readonly_objects
        .iter()
        .any(|object| object.schema == "audit"));
    let readonly_alpha = readonly_objects
        .iter()
        .find(|object| {
            object.catalog == DATABASE
                && object.schema == "alpha"
                && object.name == "shared_name"
                && object.kind == DatabaseObjectKind::Table
        })
        .cloned()
        .expect("find readonly MSSQL alpha.shared_name");
    let readonly_columns = readonly
        .table_columns(readonly_alpha)
        .await
        .expect("load readonly MSSQL columns");
    assert_eq!(
        readonly_columns
            .iter()
            .map(|column| column.name.as_str())
            .collect::<Vec<_>>(),
        ["id", "source"]
    );
    let allowed = readonly
        .run_primary(
            "mssql-readonly-allowed",
            "SELECT id FROM alpha.shared_name ORDER BY id",
        )
        .await
        .expect("query permitted MSSQL object");
    let allowed_session = first_result_session(&allowed);
    assert_eq!(allowed_session.initial_page.rows.len(), 1);
    readonly
        .release_result(allowed_session.owner)
        .await
        .expect("release readonly MSSQL result");
    let denied = readonly
        .run_primary("mssql-readonly-denied", "SELECT id FROM audit.shared_name")
        .await
        .expect("return structured MSSQL permission error");
    let denied_error = first_statement_error(&denied);
    assert_eq!(denied_error.engine, DatabaseErrorEngine::Mssql);
    assert_eq!(denied_error.code.as_deref(), Some("229"));
    if readonly.is_registered() {
        assert!(readonly.close().expect("close readonly MSSQL").closed);
    }

    scenario(Engine::Mssql, "lossless-values-structured-error");
    let values = connection
        .run_primary(
            "mssql-lossless-values",
            "SELECT big_value, precise_decimal, date_value, time_value, timestamp_value, json_value, binary_value, nullable_value FROM alpha.value_extremes WHERE id = 1",
        )
        .await
        .expect("query MSSQL lossless values");
    let values_session = first_result_session(&values);
    assert_eq!(
        values_session.initial_page.rows,
        vec![vec![
            DbValue::Integer {
                value: "9223372036854775807".to_string(),
            },
            DbValue::Decimal {
                value: "12345678901234567890.123456789012345678".to_string(),
            },
            DbValue::Date {
                value: "2024-02-29".to_string(),
            },
            DbValue::Time {
                value: "23:59:58.123456".to_string(),
            },
            DbValue::DateTime {
                value: "2024-02-29 12:34:56.123456".to_string(),
            },
            DbValue::Text {
                value: "{\"beyondU64\":18446744073709551616,\"label\":\"fixture\"}".to_string(),
            },
            DbValue::Binary {
                hex: "0001ff".to_string(),
            },
            DbValue::Null,
        ]]
    );
    connection
        .release_result(values_session.owner)
        .await
        .expect("release MSSQL value result");
    let syntax_connection = runtime
        .open_mssql(
            "mssql-syntax",
            HOST,
            PORT,
            DATABASE,
            FULL_USER,
            password.clone(),
            true,
        )
        .await
        .expect("open MSSQL syntax-error connection");
    let syntax = syntax_connection
        .run_primary("mssql-syntax-error", "SELECT 1 FROM")
        .await
        .expect("return structured MSSQL syntax error");
    let syntax_error = first_statement_error(&syntax);
    assert_eq!(syntax_error.engine, DatabaseErrorEngine::Mssql);
    assert!(syntax_error.code.is_some());
    assert!(syntax_error
        .position
        .is_some_and(|position| position.line.is_some_and(|line| line > 0)));
    if syntax_connection.is_registered() {
        assert!(
            syntax_connection
                .close()
                .expect("close MSSQL syntax-error connection")
                .closed
        );
    }

    scenario(Engine::Mssql, "exact-boundaries-all-seven");
    for row_count in ROW_BOUNDARIES {
        assert_mssql_boundary(&connection, row_count).await;
    }

    scenario(Engine::Mssql, "lazy-1201-busy-previous-next-release");
    let lazy = connection
        .run_primary(
            "mssql-lazy-1201",
            "SELECT id FROM alpha.rows_1201 ORDER BY id",
        )
        .await
        .expect("start MSSQL lazy result");
    let lazy_session = first_result_session(&lazy);
    assert_eq!(lazy_session.initial_page.rows.len(), 500);
    assert!(lazy_session.initial_page.has_next);
    let busy = connection
        .list_tables()
        .await
        .expect_err("MSSQL metadata must fail while result owns lease");
    assert_eq!(busy.code, DatabaseOperationalErrorCode::ConnectionBusy);
    let second = connection
        .result_page(lazy_session.owner.clone(), ResultPageDirection::Next)
        .await
        .expect("read MSSQL second page");
    assert_eq!(second.rows.len(), 500);
    let third = connection
        .result_page(lazy_session.owner.clone(), ResultPageDirection::Next)
        .await
        .expect("read MSSQL terminal page");
    assert_eq!(third.rows.len(), 201);
    assert_eq!(third.lifecycle, ResultSessionLifecycle::Complete);
    let previous = connection
        .result_page(lazy_session.owner.clone(), ResultPageDirection::Previous)
        .await
        .expect("read cached MSSQL previous page");
    assert_eq!(previous.page_index, 1);
    assert_eq!(previous.rows, second.rows);
    let cached_third = connection
        .result_page(lazy_session.owner.clone(), ResultPageDirection::Next)
        .await
        .expect("restore cached MSSQL terminal page");
    assert_eq!(cached_third.rows, third.rows);
    let released = connection
        .release_result(lazy_session.owner)
        .await
        .expect("release MSSQL result");
    assert_eq!(released.lifecycle, ResultSessionLifecycle::Released);

    let release_early = connection
        .run_primary(
            "mssql-release-drain",
            "SELECT id FROM alpha.rows_1201 ORDER BY id",
        )
        .await
        .expect("start MSSQL result for release drain");
    let release_early_session = first_result_session(&release_early);
    let release_early_page = connection
        .release_result(release_early_session.owner)
        .await
        .expect("drain and release active MSSQL stream");
    assert_eq!(
        release_early_page.lifecycle,
        ResultSessionLifecycle::Released
    );
    let reused = connection
        .run_primary("mssql-release-reuse", "SELECT CAST(7 AS INT)")
        .await
        .expect("reuse MSSQL actor after release drain");
    let reused_session = first_result_session(&reused);
    assert_integer_id(&reused_session.initial_page.rows[0][0], 7);
    connection
        .release_result(reused_session.owner)
        .await
        .expect("release MSSQL reuse result");

    scenario(Engine::Mssql, "procedure-done-counts-trigger-effect-once");
    let before = connection
        .run_primary(
            "mssql-affected-before",
            "SELECT (SELECT COUNT_BIG(*) FROM audit.dml_log), MIN(touched), MAX(touched) FROM alpha.dml_target",
        )
        .await
        .expect("read MSSQL affected-rows procedure baseline");
    let before_session = first_result_session(&before);
    let before_audit = integer_value(&before_session.initial_page.rows[0][0]);
    let before_min = integer_value(&before_session.initial_page.rows[0][1]);
    let before_max = integer_value(&before_session.initial_page.rows[0][2]);
    assert_eq!(before_min, before_max);
    connection
        .release_result(before_session.owner)
        .await
        .expect("release MSSQL affected-rows baseline");

    let affected = connection
        .run_primary(
            "mssql-affected-procedure-1201",
            "EXEC alpha.affected_rows_update @max_id = 1201",
        )
        .await
        .expect("run MSSQL counted DML procedure");
    assert_eq!(first_affected_rows(&affected), Some("1201"));
    let after = connection
        .run_primary(
            "mssql-affected-after",
            "SELECT (SELECT COUNT_BIG(*) FROM audit.dml_log), MIN(touched), MAX(touched) FROM alpha.dml_target",
        )
        .await
        .expect("verify MSSQL affected-rows procedure effect");
    let after_session = first_result_session(&after);
    assert_eq!(
        integer_value(&after_session.initial_page.rows[0][0]),
        before_audit + 1201
    );
    assert_eq!(
        integer_value(&after_session.initial_page.rows[0][1]),
        before_min + 1
    );
    assert_eq!(
        integer_value(&after_session.initial_page.rows[0][2]),
        before_min + 1
    );
    connection
        .release_result(after_session.owner)
        .await
        .expect("release MSSQL affected-rows verification");

    let zero = connection
        .run_primary(
            "mssql-affected-procedure-zero",
            "EXEC alpha.affected_rows_update @max_id = 0",
        )
        .await
        .expect("run MSSQL zero-row DML procedure");
    assert_eq!(first_affected_rows(&zero), Some("0"));
    let zero_after = read_single_integer(
        &connection,
        "mssql-affected-zero-after",
        "SELECT COUNT_BIG(*) FROM audit.dml_log",
    )
    .await;
    assert_eq!(zero_after, before_audit + 1201);

    let nocount = connection
        .run_primary("mssql-nocount", "EXEC alpha.nocount_update")
        .await
        .expect("run MSSQL NOCOUNT DML");
    assert_eq!(first_affected_rows(&nocount), None);

    scenario(Engine::Mssql, "output-1201-effect-trigger");
    let before = connection
        .run_primary(
            "mssql-output-before",
            "SELECT (SELECT COUNT_BIG(*) FROM audit.dml_log), MIN(touched), MAX(touched) FROM alpha.dml_target",
        )
        .await
        .expect("read MSSQL OUTPUT baseline");
    let before_session = first_result_session(&before);
    let before_audit = integer_value(&before_session.initial_page.rows[0][0]);
    let before_min = integer_value(&before_session.initial_page.rows[0][1]);
    let before_max = integer_value(&before_session.initial_page.rows[0][2]);
    assert_eq!(before_min, before_max);
    connection
        .release_result(before_session.owner)
        .await
        .expect("release MSSQL OUTPUT baseline");

    let output = connection
        .run_primary(
            "mssql-output-1201",
            "EXEC alpha.output_update @max_id = 1201",
        )
        .await
        .expect("run MSSQL OUTPUT procedure");
    assert_eq!(output.statements[0].effect_outcome, EffectOutcome::Unknown);
    assert_eq!(statement_affected_rows(&output, 0), None);
    let output_session = first_result_session(&output);
    assert_eq!(output_session.initial_page.rows.len(), 500);
    let output_second = connection
        .result_page(output_session.owner.clone(), ResultPageDirection::Next)
        .await
        .expect("read MSSQL OUTPUT second page");
    let output_third = connection
        .result_page(output_session.owner.clone(), ResultPageDirection::Next)
        .await
        .expect("read MSSQL OUTPUT terminal page");
    assert_eq!(output_second.rows.len(), 500);
    assert_eq!(output_third.rows.len(), 201);
    assert_eq!(output_third.lifecycle, ResultSessionLifecycle::Complete);
    assert_eq!(output_third.effect_outcome, EffectOutcome::Unknown);
    let output_ids = output_session
        .initial_page
        .rows
        .iter()
        .chain(output_second.rows.iter())
        .chain(output_third.rows.iter())
        .map(|row| integer_value(&row[0]))
        .collect::<HashSet<_>>();
    assert_eq!(output_ids.len(), 1201);
    assert!(output_ids.contains(&1));
    assert!(output_ids.contains(&1201));
    assert!(output_session
        .initial_page
        .rows
        .iter()
        .chain(output_second.rows.iter())
        .chain(output_third.rows.iter())
        .all(|row| integer_value(&row[1]) == before_min + 1));
    let output_previous = connection
        .result_page(output_session.owner.clone(), ResultPageDirection::Previous)
        .await
        .expect("read cached MSSQL OUTPUT page");
    assert_eq!(output_previous.rows, output_second.rows);
    let output_released = connection
        .release_result(output_session.owner)
        .await
        .expect("release MSSQL OUTPUT result");
    assert_eq!(output_released.lifecycle, ResultSessionLifecycle::Released);
    assert_eq!(output_released.effect_outcome, EffectOutcome::Unknown);

    let after = connection
        .run_primary(
            "mssql-output-after",
            "SELECT (SELECT COUNT_BIG(*) FROM audit.dml_log), MIN(touched), MAX(touched) FROM alpha.dml_target",
        )
        .await
        .expect("verify MSSQL OUTPUT effect");
    let after_session = first_result_session(&after);
    assert_eq!(
        integer_value(&after_session.initial_page.rows[0][0]),
        before_audit + 1201
    );
    assert_eq!(
        integer_value(&after_session.initial_page.rows[0][1]),
        before_min + 1
    );
    assert_eq!(
        integer_value(&after_session.initial_page.rows[0][2]),
        before_min + 1
    );
    connection
        .release_result(after_session.owner)
        .await
        .expect("release MSSQL OUTPUT verification");

    scenario(Engine::Mssql, "ordered-transaction-script");
    let transaction = connection
        .run_script(
            "mssql-transaction-script",
            vec![
                QueryExecutionUnit {
                    sql: "BEGIN TRANSACTION".to_string(),
                    transaction_boundary: TransactionBoundary::Begin,
                },
                QueryExecutionUnit {
                    sql: "SELECT value FROM alpha.transaction_probe WHERE id = 1".to_string(),
                    transaction_boundary: TransactionBoundary::None,
                },
                QueryExecutionUnit {
                    sql: "COMMIT".to_string(),
                    transaction_boundary: TransactionBoundary::Commit,
                },
            ],
        )
        .await
        .expect("run ordered MSSQL transaction script");
    assert_eq!(transaction.statements.len(), 3);
    assert_eq!(transaction.statements[0].sql, "BEGIN TRANSACTION");
    assert_eq!(
        transaction.statements[1].sql,
        "SELECT value FROM alpha.transaction_probe WHERE id = 1"
    );
    assert_eq!(transaction.statements[2].sql, "COMMIT");
    assert!(!transaction.transaction_may_be_open);
    assert!(matches!(
        &transaction.statements[0].result,
        StatementExecutionResult::Execute { .. }
    ));
    assert!(matches!(
        &transaction.statements[2].result,
        StatementExecutionResult::Execute { .. }
    ));
    let transaction_session = match &transaction.statements[1].result {
        StatementExecutionResult::Rows {
            result_session: Some(session),
            ..
        } => session.clone(),
        _ => panic!("expected MSSQL transaction SELECT result session"),
    };
    assert_eq!(transaction_session.initial_page.rows.len(), 1);
    connection
        .release_result(transaction_session.owner)
        .await
        .expect("release MSSQL transaction result");

    scenario(Engine::Mssql, "q6-select-1201-then-update");
    assert_select_1201_then_update(
        &connection,
        Engine::Mssql,
        "SELECT id FROM alpha.rows_1201 ORDER BY id",
        "UPDATE alpha.transaction_probe SET value = value + 1 WHERE id = 1",
        "SELECT value FROM alpha.transaction_probe WHERE id = 1",
    )
    .await;

    scenario(Engine::Mssql, "close-reconnect-stale-owner");
    let stale = connection
        .run_primary(
            "mssql-stale-owner",
            "SELECT id FROM alpha.rows_1201 ORDER BY id",
        )
        .await
        .expect("start MSSQL stale-owner result");
    let stale_owner = first_result_session(&stale).owner;
    connection
        .release_result(stale_owner.clone())
        .await
        .expect("release MSSQL result before close");
    let closed_identity = connection.identity();
    assert!(connection.close().expect("close MSSQL actor").closed);
    let connection = runtime
        .open_mssql(
            "mssql-main",
            HOST,
            PORT,
            DATABASE,
            FULL_USER,
            password.clone(),
            true,
        )
        .await
        .expect("reconnect MSSQL actor");
    assert_ne!(
        connection.identity().connection_generation,
        closed_identity.connection_generation
    );
    let stale_error = connection
        .result_page(stale_owner, ResultPageDirection::Next)
        .await
        .expect_err("old MSSQL result owner must stay stale after reconnect");
    assert_eq!(
        stale_error.code,
        DatabaseOperationalErrorCode::StaleConnection
    );

    scenario(Engine::Mssql, "cancel-a-terminate-reconnect-b");
    let cancel_identity = connection.identity();
    let cancel_id = QueryRunId("mssql-cancel-a".to_string());
    let run_connection = connection.clone();
    let run_id = cancel_id.clone();
    let run_a = tokio::spawn(async move {
        run_connection
            .run_primary(run_id.0, "EXEC alpha.long_query")
            .await
    });
    tokio::task::yield_now().await;
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let cancel_connection = connection.clone();
    let cancel_task = tokio::spawn(async move { cancel_connection.cancel(cancel_id).await });
    let early_b = connection
        .run_primary("mssql-run-b-early", "SELECT CAST(1 AS INT)")
        .await
        .expect_err("MSSQL Run B must wait for cancelled Run A settlement");
    assert_eq!(early_b.code, DatabaseOperationalErrorCode::ConnectionBusy);
    let cancel = tokio::time::timeout(std::time::Duration::from_secs(10), cancel_task)
        .await
        .expect("MSSQL cancel command timeout")
        .expect("MSSQL cancel command task")
        .expect("cancel exact MSSQL query owner");
    assert_eq!(
        cancel.outcome,
        QueryCancelOutcome::CancelledConnectionTerminated
    );
    let cancelled = tokio::time::timeout(std::time::Duration::from_secs(10), run_a)
        .await
        .expect("MSSQL cancelled query settlement timeout")
        .expect("MSSQL cancelled query task")
        .expect("MSSQL cancelled query result");
    assert!(cancelled.connection_terminated);
    let cancel_error = first_statement_error(&cancelled);
    assert_eq!(cancel_error.engine, DatabaseErrorEngine::Mssql);
    assert_eq!(
        cancel_error.code.as_deref(),
        Some("cancelledConnectionTerminated")
    );
    assert!(!connection.is_registered());
    let old_actor_error = connection
        .run_primary("mssql-old-actor", "SELECT CAST(1 AS INT)")
        .await
        .expect_err("terminated MSSQL actor must stay unusable");
    assert_eq!(
        old_actor_error.code,
        DatabaseOperationalErrorCode::StaleConnection
    );

    let reconnected = runtime
        .open_mssql(
            "mssql-main",
            HOST,
            PORT,
            DATABASE,
            FULL_USER,
            password,
            true,
        )
        .await
        .expect("reconnect MSSQL after cancellation");
    assert_ne!(
        reconnected.identity().connection_generation,
        cancel_identity.connection_generation
    );
    let run_b = reconnected
        .run_primary("mssql-run-b", "SELECT CAST(1 AS INT)")
        .await
        .expect("run MSSQL B on reconnected actor");
    let run_b_session = first_result_session(&run_b);
    assert_integer_id(&run_b_session.initial_page.rows[0][0], 1);
    reconnected
        .release_result(run_b_session.owner)
        .await
        .expect("release MSSQL Run B");
    assert!(reconnected.close().expect("close reconnected MSSQL").closed);
}

#[tokio::test]
#[ignore = "requires explicit real database integration selection"]
async fn database_integration_matrix() {
    let selected = selected_engines().unwrap_or_else(|scenario| panic!("{scenario}"));
    for engine in Engine::ALL {
        if !selected.contains(&engine) {
            scenario(engine, "not-selected");
            continue;
        }
        match engine {
            Engine::Sqlite => run_sqlite().await,
            Engine::Postgres => run_postgres().await,
            Engine::Mssql => run_mssql().await,
        }
    }
}
