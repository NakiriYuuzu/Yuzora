use super::*;

const UPDATE_CELL: &str =
    "UPDATE \"main\".\"people\" SET \"name\" = 'bob' WHERE \"id\" = 1 AND \"name\" = 'alice' COLLATE BINARY";

struct Fixture {
    file: tempfile::NamedTempFile,
    state: DbState,
    sessions: ResultSessionState,
    actor: Arc<ProductionConnectionActor>,
}

impl Fixture {
    fn new() -> Self {
        let file = tempfile::NamedTempFile::new().unwrap();
        let connection = Connection::open(file.path()).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE people(id INTEGER PRIMARY KEY, name TEXT); \
             INSERT INTO people VALUES(1, 'alice'); CREATE TABLE audit(value INTEGER);",
            )
            .unwrap();
        let actor = Arc::new(ProductionConnectionActor::new(
            ConnectionIdentity {
                descriptor_id: DescriptorId("cell".into()),
                connection_id: ConnectionId("cell-connection".into()),
                connection_generation: ConnectionGeneration("cell-generation".into()),
            },
            DbHandle::Sqlite(Mutex::new(connection)),
        ));
        let state = DbState::default();
        register_actor(&state, actor.clone()).unwrap();
        Self {
            file,
            state,
            sessions: ResultSessionState::default(),
            actor,
        }
    }

    fn request(&self, mode: &str, sql: &str) -> QueryRunRequest {
        // Keep the requested wire contract explicit: old helpers must reject
        // this mode before running any SQL, rather than ignore a guard field.
        let identity = self.actor.identity();
        serde_json::from_value(serde_json::json!({
            "descriptorId": identity.descriptor_id,
            "connectionId": identity.connection_id,
            "connectionGeneration": identity.connection_generation,
            "queryRunId": uuid::Uuid::new_v4().to_string(),
            "mode": mode,
            "statements": [{ "sql": sql, "transactionBoundary": "none" }],
        }))
        .unwrap()
    }

    async fn run(&self, mode: &str, sql: &str) -> Result<QueryRun, DatabaseOperationalError> {
        query_run_in_state(&self.state, &self.sessions, self.request(mode, sql)).await
    }

    fn external(&self, sql: &str) {
        Connection::open(self.file.path())
            .unwrap()
            .execute_batch(sql)
            .unwrap();
    }

    fn persisted(&self) -> (Vec<String>, i64) {
        let connection = Connection::open(self.file.path()).unwrap();
        let names = connection
            .prepare("SELECT name FROM people ORDER BY rowid")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let audit = connection
            .query_row("SELECT count(*) FROM audit", [], |row| row.get(0))
            .unwrap();
        (names, audit)
    }

    fn assert_reusable(&self) {
        let lease = self
            .actor
            .acquire_metadata()
            .expect("mutation must settle its lease");
        let DbHandle::Sqlite(connection) = self.actor.handle() else {
            unreachable!()
        };
        assert!(connection.lock().unwrap().is_autocommit());
        self.actor.settle_metadata(&lease).unwrap();
    }
}

#[tokio::test]
async fn sqlite_cell_mutation_commits_exactly_one_row() {
    let fixture = Fixture::new();
    let run = fixture.run("tableCellMutation", UPDATE_CELL).await.unwrap();
    assert_eq!(fixture.persisted(), (vec!["bob".into()], 0));
    assert_eq!(run.statements[0].effect_outcome, EffectOutcome::Committed);
    assert!(
        matches!(&run.statements[0].result, StatementExecutionResult::Execute { affected_rows } if affected_rows.as_deref() == Some("1"))
    );
    assert!(!run.transaction_may_be_open);
    fixture.assert_reusable();
}

#[tokio::test]
async fn sqlite_cell_mutation_generated_targets_reject_and_base_edits_recalculate() {
    let fixture = Fixture::new();
    fixture.external(
        "ALTER TABLE people ADD COLUMN virtual_name TEXT AS (upper(name)) VIRTUAL;
        CREATE TABLE derived(id INTEGER PRIMARY KEY, name TEXT,
            virtual_name TEXT AS (upper(name)) VIRTUAL,
            stored_name TEXT AS (name || '!') STORED);
        INSERT INTO derived(id, name) VALUES(1, 'alice'),(2, NULL);",
    );
    let read = || {
        let connection = Connection::open(fixture.file.path()).unwrap();
        let mut statement = connection
            .prepare("SELECT name, virtual_name, stored_name FROM derived ORDER BY id")
            .unwrap();
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };
    let before = vec![
        (
            Some("alice".into()),
            Some("ALICE".into()),
            Some("alice!".into()),
        ),
        (None, None, None),
    ];
    assert_eq!(read(), before);
    for column in ["virtual_name", "stored_name"] {
        for id in [1, 2] {
            let run = fixture
                .run(
                    "tableCellMutation",
                    &format!("UPDATE main.derived SET {column} = 'changed' WHERE id = {id}"),
                )
                .await
                .unwrap();
            assert!(
                matches!(&run.statements[0].result, StatementExecutionResult::Error { error } if error.message.contains("generated column"))
            );
            assert_eq!(run.statements[0].effect_outcome, EffectOutcome::RolledBack);
            assert!(!run.transaction_may_be_open);
            assert_eq!(read(), before);
            fixture.assert_reusable();
        }
    }
    for (id, predicate) in [(1, "name = 'alice' COLLATE BINARY"), (2, "name IS NULL")] {
        let run = fixture
            .run(
                "tableCellMutation",
                &format!("UPDATE main.derived SET name = 'bob' WHERE id = {id} AND {predicate}"),
            )
            .await
            .unwrap();
        assert_eq!(run.statements[0].effect_outcome, EffectOutcome::Committed);
        assert!(!run.transaction_may_be_open);
        fixture.assert_reusable();
    }
    assert_eq!(
        read(),
        vec![(Some("bob".into()), Some("BOB".into()), Some("bob!".into())); 2]
    );
}

#[tokio::test]
async fn sqlite_cell_mutation_cardinality_conflict_preserves_all_rows() {
    for duplicate in [false, true] {
        let fixture = Fixture::new();
        if duplicate {
            fixture.external("BEGIN; DROP TABLE people; CREATE TABLE people(id INTEGER, name TEXT); INSERT INTO people VALUES(1,'alice'),(1,'alice'); COMMIT;");
        } else {
            fixture.external("UPDATE people SET name = 'carol'");
        }
        let before = fixture.persisted();
        let run = fixture.run("tableCellMutation", UPDATE_CELL).await.unwrap();
        assert_eq!(fixture.persisted(), before);
        assert_eq!(run.statements[0].effect_outcome, EffectOutcome::RolledBack);
        assert!(
            matches!(&run.statements[0].result, StatementExecutionResult::Error { error } if error.code.as_deref() == Some("cardinalityMismatch"))
        );
        assert!(!run.transaction_may_be_open);
        fixture.assert_reusable();
    }
}

#[tokio::test]
async fn sqlite_cell_mutation_rolls_back_trigger_fail_side_effects() {
    let fixture = Fixture::new();
    fixture.external("CREATE TRIGGER fail_edit BEFORE UPDATE ON people BEGIN INSERT INTO audit VALUES(1); SELECT RAISE(FAIL, 'owned trigger failure'); END;");
    let run = fixture.run("tableCellMutation", UPDATE_CELL).await.unwrap();
    assert_eq!(fixture.persisted(), (vec!["alice".into()], 0));
    assert_eq!(run.statements[0].effect_outcome, EffectOutcome::RolledBack);
    assert!(
        matches!(&run.statements[0].result, StatementExecutionResult::Error { error } if error.code.as_deref() == Some("1811"))
    );
    fixture.assert_reusable();
}

#[tokio::test]
async fn sqlite_cell_mutation_preserves_existing_user_transaction_and_savepoint() {
    let fixture = Fixture::new();
    let DbHandle::Sqlite(connection) = fixture.actor.handle() else {
        unreachable!()
    };
    connection
        .lock()
        .unwrap()
        .execute_batch("BEGIN; INSERT INTO audit VALUES(1); SAVEPOINT user_checkpoint;")
        .unwrap();
    let error = fixture
        .run("tableCellMutation", UPDATE_CELL)
        .await
        .unwrap_err();
    assert_eq!(error.code, DatabaseOperationalErrorCode::ConnectionBusy);
    assert_eq!(fixture.persisted(), (vec!["alice".into()], 0));
    let connection = connection.lock().unwrap();
    assert!(!connection.is_autocommit());
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM audit", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        1
    );
    connection
        .execute_batch("ROLLBACK TO user_checkpoint; RELEASE user_checkpoint; COMMIT;")
        .unwrap();
    drop(connection);
    assert_eq!(fixture.persisted(), (vec!["alice".into()], 1));
    fixture.assert_reusable();
}

#[tokio::test]
async fn sqlite_cell_mutation_rejects_non_cell_statements_without_writes() {
    for sql in [
        "COMMIT",
        "DROP TABLE people",
        "UPDATE people SET name = 'bob'; DELETE FROM audit",
        "UPDATE people SET name = 'bob' RETURNING name",
    ] {
        let fixture = Fixture::new();
        let result = fixture.run("tableCellMutation", sql).await;
        assert!(
            result.is_err()
                || matches!(
                    &result.as_ref().unwrap().statements[0].result,
                    StatementExecutionResult::Error { .. }
                )
        );
        assert_eq!(fixture.persisted(), (vec!["alice".into()], 0));
        fixture.assert_reusable();
    }
}

#[tokio::test]
async fn sqlite_cell_mutation_leaves_generic_sql_fail_semantics_unchanged() {
    let fixture = Fixture::new();
    fixture.external("CREATE TRIGGER fail_edit BEFORE UPDATE ON people BEGIN INSERT INTO audit VALUES(1); SELECT RAISE(FAIL, 'owned trigger failure'); END;");
    let run = fixture.run("primary", UPDATE_CELL).await.unwrap();
    assert!(matches!(
        run.statements[0].result,
        StatementExecutionResult::Error { .. }
    ));
    assert_eq!(fixture.persisted(), (vec!["alice".into()], 1));
    fixture.assert_reusable();
}

async fn commit_checkpoint(after_commit: bool, abandon: bool) {
    use super::sqlite_cell_mutation::checkpoints::{install, Phase};
    let fixture = Fixture::new();
    fixture.external("CREATE TRIGGER record_edit BEFORE UPDATE ON people BEGIN INSERT INTO audit VALUES(1); END;");
    let request = fixture.request("tableCellMutation", UPDATE_CELL);
    let owner = QueryRunOwner {
        descriptor_id: request.descriptor_id.clone(),
        connection_id: request.connection_id.clone(),
        connection_generation: request.connection_generation.clone(),
        query_run_id: request.query_run_id.clone(),
    };
    let phase = if after_commit {
        Phase::AfterCommit
    } else {
        Phase::BeforeCommit
    };
    let (arrival, resume) = install(owner.query_run_id.clone(), phase);
    let state = fixture.state.clone();
    let sessions = fixture.sessions.clone();
    let pending = tokio::spawn(async move { query_run_in_state(&state, &sessions, request).await });
    tokio::task::spawn_blocking(move || arrival.recv_timeout(Duration::from_secs(3)).unwrap())
        .await
        .unwrap();
    if abandon {
        pending.abort();
        assert!(pending.await.unwrap_err().is_cancelled());
        resume.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                match fixture.actor.acquire_metadata() {
                    Ok(lease) => {
                        fixture.actor.settle_metadata(&lease).unwrap();
                        break;
                    }
                    Err(ActorError::ConnectionBusy) => tokio::task::yield_now().await,
                    other => panic!("mutation actor did not become reusable: {other:?}"),
                }
            }
        })
        .await
        .unwrap();
    } else {
        let cancel = query_cancel_in_state(&fixture.state, owner);
        tokio::pin!(cancel);
        // SQLite dispatch sets the exact cancel flag and interrupt before
        // waiting for settlement. Release the worker only after that point.
        assert!(futures_util::poll!(&mut cancel).is_pending());
        resume.send(()).unwrap();
        cancel.await.unwrap();
        let run = pending.await.unwrap().unwrap();
        if after_commit {
            assert_eq!(run.statements[0].effect_outcome, EffectOutcome::Committed);
            assert!(matches!(
                run.statements[0].result,
                StatementExecutionResult::Execute { .. }
            ));
        } else {
            assert_eq!(run.statements[0].effect_outcome, EffectOutcome::RolledBack);
            assert!(matches!(
                run.statements[0].result,
                StatementExecutionResult::Cancelled { .. }
            ));
        }
    }
    assert_eq!(
        fixture.persisted(),
        if after_commit {
            (vec!["bob".into()], 1)
        } else {
            (vec!["alice".into()], 0)
        }
    );
    fixture.assert_reusable();
    let next = fixture
        .run(
            "tableCellMutation",
            "UPDATE people SET name = 'carol' WHERE id = 1",
        )
        .await
        .unwrap();
    assert_eq!(next.statements[0].effect_outcome, EffectOutcome::Committed);
    assert_eq!(fixture.persisted().0, vec!["carol"]);
}

#[tokio::test]
async fn sqlite_cell_mutation_cancel_between_check_and_commit_rolls_back() {
    commit_checkpoint(false, false).await;
}

#[tokio::test]
async fn sqlite_cell_mutation_abandoned_caller_rolls_back_before_releasing_lease() {
    commit_checkpoint(false, true).await;
}

#[tokio::test]
async fn sqlite_cell_mutation_cancel_after_commit_does_not_relabel_persisted_data() {
    commit_checkpoint(true, false).await;
}

#[test]
fn sqlite_cell_mutation_mode_is_not_ignored_by_legacy_decoders() {
    #[derive(Debug, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    enum LegacyMode {
        Primary,
        Script,
    }
    #[derive(Debug, serde::Deserialize)]
    struct LegacyRequest {
        mode: LegacyMode,
    }
    let fixture = Fixture::new();
    let request = fixture.request("tableCellMutation", UPDATE_CELL);
    let mut wire = serde_json::to_value(request).unwrap();
    assert!(serde_json::from_value::<LegacyRequest>(wire.clone()).is_err());
    wire["mode"] = "primary".into();
    wire["expectedRows"] = "1".into();
    assert!(matches!(
        serde_json::from_value::<LegacyRequest>(wire).unwrap().mode,
        LegacyMode::Primary
    ));
    assert_eq!(fixture.persisted(), (vec!["alice".into()], 0));
}

#[tokio::test]
async fn sqlite_cell_mutation_rejects_extra_units_and_transaction_boundaries() {
    for extra_unit in [false, true] {
        let fixture = Fixture::new();
        let mut request = fixture.request("tableCellMutation", UPDATE_CELL);
        request.statements = NonEmptyVec::try_from(if extra_unit {
            vec![
                QueryExecutionUnit {
                    sql: UPDATE_CELL.into(),
                    transaction_boundary: TransactionBoundary::None,
                },
                QueryExecutionUnit {
                    sql: UPDATE_CELL.into(),
                    transaction_boundary: TransactionBoundary::None,
                },
            ]
        } else {
            vec![QueryExecutionUnit {
                sql: UPDATE_CELL.into(),
                transaction_boundary: TransactionBoundary::Commit,
            }]
        })
        .unwrap();
        assert!(
            query_run_in_state(&fixture.state, &fixture.sessions, request)
                .await
                .is_err()
        );
        assert_eq!(fixture.persisted(), (vec!["alice".into()], 0));
        fixture.assert_reusable();
    }
}

async fn failed_rollback_quarantine(abandon: bool, replace_generation: bool) {
    use super::sqlite_cell_mutation::checkpoints::{install, Phase};
    use rusqlite::hooks::{AuthAction, AuthContext, Authorization, TransactionOperation};
    let fixture = Fixture::new();
    let denials = Arc::new(AtomicU64::new(0));
    let denied = denials.clone();
    let DbHandle::Sqlite(connection) = fixture.actor.handle() else {
        unreachable!()
    };
    connection
        .lock()
        .unwrap()
        .authorizer(Some(move |context: AuthContext<'_>| {
            if matches!(
                context.action,
                AuthAction::Transaction {
                    operation: TransactionOperation::Rollback
                }
            ) {
                denied.fetch_add(1, Ordering::SeqCst);
                Authorization::Deny
            } else {
                Authorization::Allow
            }
        }))
        .unwrap();
    let request = fixture.request("tableCellMutation", UPDATE_CELL);
    let owner = QueryRunOwner {
        descriptor_id: request.descriptor_id.clone(),
        connection_id: request.connection_id.clone(),
        connection_generation: request.connection_generation.clone(),
        query_run_id: request.query_run_id.clone(),
    };
    let (arrival, resume) = install(owner.query_run_id.clone(), Phase::AfterUpdate);
    let state = fixture.state.clone();
    let sessions = fixture.sessions.clone();
    let pending = tokio::spawn(async move { query_run_in_state(&state, &sessions, request).await });
    tokio::task::spawn_blocking(move || arrival.recv_timeout(Duration::from_secs(3)).unwrap())
        .await
        .unwrap();
    let replacement = replace_generation.then(|| {
        let mut identity = fixture.actor.identity().clone();
        identity.connection_generation = ConnectionGeneration("replacement-generation".into());
        let replacement = Arc::new(ProductionConnectionActor::new(
            identity,
            DbHandle::Sqlite(Mutex::new(Connection::open_in_memory().unwrap())),
        ));
        fixture
            .state
            .0
            .lock()
            .unwrap()
            .insert(owner.connection_id.0.clone(), replacement.clone());
        replacement
    });
    if abandon {
        pending.abort();
        assert!(pending.await.unwrap_err().is_cancelled());
        resume.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            while fixture.actor.teardown_report().unreleased_execution {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    } else {
        let cancel = query_cancel_in_state(&fixture.state, owner.clone());
        tokio::pin!(cancel);
        assert!(futures_util::poll!(&mut cancel).is_pending());
        resume.send(()).unwrap();
        cancel.await.unwrap();
        let run = pending.await.unwrap().unwrap();
        assert!(run.connection_terminated);
        assert!(run.transaction_may_be_open);
        assert_eq!(run.statements[0].effect_outcome, EffectOutcome::Unknown);
    }
    assert!(
        denials.load(Ordering::SeqCst) > 0,
        "SQLite must actually reject rollback"
    );
    assert!(!connection.lock().unwrap().is_autocommit());
    assert!(matches!(
        fixture.actor.acquire_metadata(),
        Err(ActorError::Closed)
    ));
    let retained = fixture
        .state
        .0
        .lock()
        .unwrap()
        .get(&owner.connection_id.0)
        .is_some_and(|actor| Arc::ptr_eq(actor, &fixture.actor));
    let weak = Arc::downgrade(&fixture.actor);
    let Fixture {
        file, state, actor, ..
    } = fixture;
    drop(actor);
    if !retained {
        tokio::time::timeout(Duration::from_secs(3), async {
            while weak.strong_count() != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("retired actor callbacks must not retain their own connection");
    }
    let independent = Connection::open(file.path()).unwrap();
    independent.busy_timeout(Duration::ZERO).unwrap();
    let write = independent.execute("INSERT INTO audit VALUES(99)", []);
    assert!(
        write.is_ok(),
        "quarantined SQLite retained its write lock: {write:?}, registry retained={retained}"
    );
    assert!(!retained);
    assert_eq!(weak.strong_count(), 0);
    assert_eq!(
        independent
            .query_row("SELECT name FROM people WHERE id=1", [], |row| row
                .get::<_, String>(0))
            .unwrap(),
        "alice"
    );
    match replacement {
        Some(replacement) => {
            assert!(state
                .0
                .lock()
                .unwrap()
                .get(&owner.connection_id.0)
                .is_some_and(|current| Arc::ptr_eq(current, &replacement)));
            let lease = replacement.acquire_metadata().unwrap();
            replacement.settle_metadata(&lease).unwrap();
        }
        None => assert!(!state.0.lock().unwrap().contains_key(&owner.connection_id.0)),
    }
}

#[tokio::test]
async fn sqlite_cell_mutation_quarantine_retires_failed_rollback_without_a_caller() {
    failed_rollback_quarantine(true, false).await;
}

#[tokio::test]
async fn sqlite_cell_mutation_quarantine_retires_failed_rollback_after_cancel() {
    failed_rollback_quarantine(false, false).await;
}

#[tokio::test]
async fn sqlite_cell_mutation_quarantine_preserves_replacement_actor() {
    failed_rollback_quarantine(true, true).await;
}
