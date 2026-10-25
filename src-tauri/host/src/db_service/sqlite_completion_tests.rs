use super::*;

fn changed_schema(row_count: i64) -> (tempfile::NamedTempFile, Connection) {
    let file = tempfile::NamedTempFile::new().unwrap();
    let connection = Connection::open(file.path()).unwrap();
    connection
        .execute_batch("CREATE TABLE people(id INTEGER PRIMARY KEY, name TEXT);")
        .unwrap();
    // Keep this connection's loaded schema stale, as when a second editor
    // rebuilds the table after the user loaded its columns and rows.
    let external = Connection::open(file.path()).unwrap();
    external
        .execute_batch(
            "BEGIN; DROP TABLE people; \
             CREATE TABLE people(id INTEGER PRIMARY KEY, name TEXT, extra TEXT); \
             INSERT INTO people(id, name) VALUES(1, 'alice'); COMMIT;",
        )
        .unwrap();
    if row_count > 1 {
        external
            .execute(
                "WITH RECURSIVE ids(id) AS (VALUES(2) UNION ALL \
                 SELECT id + 1 FROM ids WHERE id < ?1) \
                 INSERT INTO people(id, name) SELECT id, 'alice' FROM ids",
                [row_count],
            )
            .unwrap();
    }
    (file, connection)
}

fn persisted_name(file: &tempfile::NamedTempFile) -> String {
    Connection::open(file.path())
        .unwrap()
        .query_row("SELECT name FROM people WHERE id = 1", [], |row| row.get(0))
        .unwrap()
}

async fn schema_retry(mode: QueryRunMode, returning: bool, row_count: i64) {
    let (file, connection) = changed_schema(row_count);
    let identity = ConnectionIdentity {
        descriptor_id: DescriptorId("schema-retry".into()),
        connection_id: ConnectionId("schema-retry-connection".into()),
        connection_generation: ConnectionGeneration("schema-retry-generation".into()),
    };
    let state = DbState::default();
    register_actor(
        &state,
        Arc::new(ProductionConnectionActor::new(
            identity.clone(),
            DbHandle::Sqlite(Mutex::new(connection)),
        )),
    )
    .unwrap();
    let sessions = ResultSessionState::default();
    let mut sql = "UPDATE people SET name = 'bob'".to_string();
    if returning {
        sql.push_str(" RETURNING name");
    }
    let result = query_run_in_state(
        &state,
        &sessions,
        QueryRunRequest {
            descriptor_id: identity.descriptor_id,
            connection_id: identity.connection_id,
            connection_generation: identity.connection_generation,
            query_run_id: QueryRunId("schema-retry-run".into()),
            mode,
            statements: NonEmptyVec::try_from(vec![QueryExecutionUnit {
                sql,
                transaction_boundary: TransactionBoundary::None,
            }])
            .unwrap(),
        },
    )
    .await
    .unwrap();

    assert!(!result.transaction_may_be_open);
    assert!(!result.connection_terminated);
    let statement = &result.statements[0];
    let completed_outcome = match &statement.result {
        StatementExecutionResult::Execute { affected_rows } if !returning => {
            assert_eq!(
                affected_rows.as_deref(),
                Some(row_count.to_string().as_str())
            );
            statement.effect_outcome
        }
        StatementExecutionResult::Rows {
            result_session: Some(session),
            ..
        } if returning => {
            let mut page = session.initial_page.clone();
            if page.has_next {
                assert_eq!(statement.effect_outcome, EffectOutcome::Unknown);
            }
            let mut seen = page.rows.len();
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
                seen += page.rows.len();
            }
            assert_eq!(seen as i64, row_count);
            page.effect_outcome
        }
        other => panic!("unexpected schema retry result: {other:?}"),
    };
    assert_eq!(persisted_name(&file), "bob");
    let persisted: i64 = Connection::open(file.path())
        .unwrap()
        .query_row(
            "SELECT count(*) FROM people WHERE name = 'bob'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(persisted, row_count);
    assert_eq!(completed_outcome, EffectOutcome::Committed);
    if row_count == 1 {
        assert_eq!(statement.effect_outcome, EffectOutcome::Committed);
    }
}

#[tokio::test]
async fn sqlite_completion_primary_schema_retry_matches_persisted_commit() {
    schema_retry(QueryRunMode::Primary, false, 1).await;
}

#[tokio::test]
async fn sqlite_completion_script_schema_retry_matches_persisted_commit() {
    schema_retry(QueryRunMode::Script, false, 1).await;
}

#[tokio::test]
async fn sqlite_completion_primary_returning_schema_retry_matches_persisted_commit() {
    schema_retry(QueryRunMode::Primary, true, 1).await;
}

#[tokio::test]
async fn sqlite_completion_script_returning_schema_retry_matches_persisted_commit() {
    schema_retry(QueryRunMode::Script, true, 1).await;
}

#[tokio::test]
async fn sqlite_completion_primary_returning_continuation_matches_persisted_commit() {
    schema_retry(QueryRunMode::Primary, true, 1201).await;
}

#[test]
fn sqlite_completion_legacy_schema_retry_matches_persisted_commit() {
    for returning in [false, true] {
        let (file, connection) = changed_schema(1);
        let mut sql = "UPDATE people SET name = 'bob' WHERE id = 1".to_string();
        if returning {
            sql.push_str(" RETURNING name");
        }
        let result = run_query(&connection, &sql, DEFAULT_MAX_ROWS).unwrap();
        assert_eq!(persisted_name(&file), "bob");
        let outcome = match result {
            QueryResult::Execute { effect_outcome, .. }
            | QueryResult::Select { effect_outcome, .. } => effect_outcome,
        };
        assert_eq!(outcome, EffectOutcome::Committed);
    }
}

// Renaming directories containing open journals is a Unix filesystem fixture.
#[cfg(unix)]
#[test]
fn sqlite_completion_failed_commit_after_hook_still_reports_rollback() {
    let fixture = tempfile::tempdir().unwrap();
    let main_dir = fixture.path().join("main");
    let moved_dir = fixture.path().join("moved");
    std::fs::create_dir(&main_dir).unwrap();
    let main_path = main_dir.join("main.sqlite");
    let aux_path = fixture.path().join("aux.sqlite");
    let connection = Connection::open(&main_path).unwrap();
    connection
        .execute("ATTACH DATABASE ?1 AS aux", [&aux_path.to_str().unwrap()])
        .unwrap();
    connection
        .execute_batch(
            "PRAGMA main.journal_mode = DELETE; PRAGMA aux.journal_mode = DELETE; \
             PRAGMA main.synchronous = FULL; PRAGMA aux.synchronous = FULL; \
             CREATE TABLE main.effects(id INTEGER); CREATE TABLE aux.effects(id INTEGER); \
             BEGIN; INSERT INTO main.effects VALUES(1); INSERT INTO aux.effects VALUES(1);",
        )
        .unwrap();
    let probe = SqliteCompletionProbe::install(&connection).unwrap();
    // Both rollback journals are already open. The multi-database commit must
    // open its super-journal after the commit hook; make only that open fail.
    std::fs::rename(&main_dir, &moved_dir).unwrap();
    std::fs::write(&main_dir, b"owned fixture blocks super-journal creation").unwrap();
    let commit = connection.execute_batch("COMMIT");
    let commit_hook_seen = probe.committed.load(Ordering::SeqCst);
    let rollback_hook_seen = probe.rolled_back.load(Ordering::SeqCst);
    let outcome = probe.finish(false, commit.is_ok());
    std::fs::remove_file(&main_dir).unwrap();
    std::fs::rename(&moved_dir, &main_dir).unwrap();

    assert!(commit.is_err(), "the owned super-journal open must fail");
    assert!(commit_hook_seen, "failure must occur after the commit hook");
    assert!(rollback_hook_seen, "the failed commit must roll back");
    assert_eq!(outcome, EngineCompletion::RolledBack);
    assert!(connection.is_autocommit());
    drop(connection);
    for path in [&main_path, &aux_path] {
        let persisted = Connection::open(path).unwrap();
        let count: i64 = persisted
            .query_row("SELECT count(*) FROM effects", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0, "failed commit changed {}", path.display());
    }
}
