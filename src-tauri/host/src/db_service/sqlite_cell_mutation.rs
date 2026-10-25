use super::*;

struct WaitingCaller(Arc<AtomicBool>);

impl Drop for WaitingCaller {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

fn mutation_error(code: &str, message: &str) -> DatabaseError {
    let mut error = worker_error(code, message);
    error.retryability = Retryability::NotRetryable;
    error
}

fn failed(error: DatabaseError, effect_outcome: EffectOutcome, cancelled: bool) -> P6UnitOutcome {
    P6UnitOutcome {
        result: if cancelled {
            StatementExecutionResult::Cancelled { error }
        } else {
            StatementExecutionResult::Error { error }
        },
        effect_outcome,
        stop: true,
        connection_terminated: false,
    }
}

fn stopped(actor: &ProductionConnectionActor, lease: &ExecutionLease, caller: &AtomicBool) -> bool {
    caller.load(Ordering::SeqCst)
        || actor.is_terminating()
        || actor.cancel_requested(lease).unwrap_or(true)
}

pub(super) async fn run(
    state: DbState,
    actor: Arc<ProductionConnectionActor>,
    request: QueryRunRequest,
) -> Result<QueryRun, DatabaseOperationalError> {
    let invalid = || {
        DatabaseOperationalError::new(
            DatabaseOperationalErrorCode::QueryFailed,
            "table cell mutation requires one generated SQLite UPDATE",
        )
    };
    if !matches!(actor.handle(), DbHandle::Sqlite(_)) || request.statements.len() != 1 {
        return Err(invalid());
    }
    let unit = &request.statements[0];
    if unit.transaction_boundary != TransactionBoundary::None
        || !unit.sql.trim_start().starts_with("UPDATE ")
    {
        return Err(invalid());
    }
    let sql = unit.sql.clone();
    let owner = QueryRunOwner {
        descriptor_id: request.descriptor_id,
        connection_id: request.connection_id,
        connection_generation: request.connection_generation,
        query_run_id: request.query_run_id,
    };
    let lease = actor
        .acquire_execution(owner.clone(), CancelCapability::SqliteInterrupt)
        .map_err(actor_error)?;
    let mut settlement = ExecutionSettlementGuard::new(actor.clone(), lease.clone());
    let caller = Arc::new(AtomicBool::new(false));
    let _waiting = WaitingCaller(caller.clone());
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let worker_sql = sql.clone();
    tokio::task::spawn_blocking(move || {
        // The worker retains admission even if the async caller disappears.
        let outcome = execute(&actor, &lease, &worker_sql, caller).and_then(|outcome| {
            if outcome.0.connection_terminated {
                // Cleanup has released the connection mutex. Retirement must
                // not depend on a caller surviving to receive the response.
                remove_exact_shutdown_actor(&state, actor.identity(), &actor).map_err(|_| {
                    DatabaseOperationalError::new(
                        DatabaseOperationalErrorCode::ServerDisconnected,
                        "SQLite mutation connection retirement failed",
                    )
                })?;
            }
            Ok(outcome)
        });
        let result = settlement
            .settle_with_policy(false)
            .map_err(actor_error)
            .and(outcome);
        let _ = sender.send(result);
    });
    let (outcome, transaction_may_be_open) = receiver.await.map_err(|_| {
        DatabaseOperationalError::new(
            DatabaseOperationalErrorCode::QueryFailed,
            "table cell mutation worker stopped before returning its outcome",
        )
    })??;
    Ok(QueryRun {
        descriptor_id: owner.descriptor_id,
        connection_id: owner.connection_id,
        connection_generation: owner.connection_generation,
        query_run_id: owner.query_run_id,
        statements: NonEmptyVec::try_from(vec![StatementExecution {
            statement_execution_id: StatementExecutionId(format!(
                "statement-{}",
                uuid::Uuid::new_v4()
            )),
            statement_index: 0,
            sql,
            effect_outcome: outcome.effect_outcome,
            result: outcome.result,
        }])
        .expect("cell mutation has exactly one statement"),
        transaction_may_be_open,
        connection_terminated: outcome.connection_terminated,
    })
}

fn execute(
    actor: &Arc<ProductionConnectionActor>,
    lease: &ExecutionLease,
    sql: &str,
    caller: Arc<AtomicBool>,
) -> Result<(P6UnitOutcome, bool), DatabaseOperationalError> {
    let DbHandle::Sqlite(connection) = actor.handle() else {
        unreachable!("cell mutation engine was checked before acquiring the lease")
    };
    let connection = connection.lock().map_err(|_| {
        DatabaseOperationalError::new(
            DatabaseOperationalErrorCode::QueryFailed,
            "SQLite connection lock failed",
        )
    })?;
    if !connection.is_autocommit() {
        return Err(DatabaseOperationalError::new(
            DatabaseOperationalErrorCode::ConnectionBusy,
            "table cell mutation cannot join an existing transaction",
        ));
    }
    if stopped(actor, lease, &caller) {
        return Ok((failed(cancelled_error(), EffectOutcome::None, true), false));
    }
    let transaction = match rusqlite::Transaction::new_unchecked(
        &connection,
        rusqlite::TransactionBehavior::Immediate,
    ) {
        Ok(transaction) => transaction,
        Err(error) => {
            return Ok((
                failed(
                    sqlite_database_error(&error),
                    EffectOutcome::None,
                    stopped(actor, lease, &caller),
                ),
                !connection.is_autocommit(),
            ))
        }
    };
    // Declared after the transaction so unwinding clears cancellation before
    // the RAII transaction attempts its rollback.
    // A failed hook removal must not create connection -> hook -> actor ->
    // connection cycles. Only this mutation's callbacks include caller loss.
    let progress_actor = Arc::downgrade(actor);
    let progress_lease = lease.clone();
    let progress_caller = caller.clone();
    let cancellation = connection
        .progress_handler(
            1000,
            Some(move || {
                progress_actor
                    .upgrade()
                    .is_none_or(|actor| stopped(&actor, &progress_lease, &progress_caller))
            }),
        )
        .map(|_| SqliteCancellationGuard(&connection))
        .map_err(|error| sqlite_database_error(&error));
    let commit_actor = Arc::downgrade(actor);
    let commit_lease = lease.clone();
    let commit_caller = caller.clone();
    let probe = SqliteCompletionProbe::install_with_veto(&connection, move || {
        commit_actor
            .upgrade()
            .is_none_or(|actor| stopped(&actor, &commit_lease, &commit_caller))
    });
    let update = (|| {
        if let Err(error) = &cancellation {
            return Err(error.clone());
        }
        if probe.is_none() {
            return Err(mutation_error(
                "cellMutationHooks",
                "SQLite completion hooks are unavailable",
            ));
        }
        if stopped(actor, lease, &caller) {
            return Err(cancelled_error());
        }
        // rusqlite rejects a nonempty second statement during prepare.
        let mut statement = transaction
            .prepare(sql)
            .map_err(|error| sqlite_database_error(&error))?;
        if statement.readonly() || statement.column_count() != 0 {
            return Err(mutation_error(
                "cellMutationShape",
                "table cell mutation requires an UPDATE without RETURNING",
            ));
        }
        let affected = statement
            .execute([])
            .map_err(|error| sqlite_database_error(&error))?;
        if affected != 1 {
            return Err(mutation_error(
                "cardinalityMismatch",
                "table cell mutation did not match exactly one row",
            ));
        }
        Ok(())
    })();
    // An accepted cancel must not repeatedly interrupt our own cleanup. The
    // commit hook still checks exact cancellation at the final commit boundary.
    let cancellation_clear = connection
        .progress_handler(0, None::<fn() -> bool>)
        .map_err(|error| sqlite_database_error(&error));
    let cancellation_cleared = cancellation_clear.is_ok();
    drop(cancellation);
    let update = cancellation_clear.and(update);
    #[cfg(test)]
    checkpoints::reach(&lease.owner().query_run_id, checkpoints::Phase::AfterUpdate);
    let cancelled = stopped(actor, lease, &caller);
    let committing = update.is_ok() && !cancelled;
    #[cfg(test)]
    if committing {
        checkpoints::reach(
            &lease.owner().query_run_id,
            checkpoints::Phase::BeforeCommit,
        );
    }
    let mut result = if committing {
        transaction
            .commit()
            .map_err(|error| sqlite_database_error(&error))
    } else {
        let original = update.err().unwrap_or_else(cancelled_error);
        let rollback = if connection.is_autocommit() {
            drop(transaction);
            Ok(())
        } else {
            transaction.rollback()
        };
        Err(rollback
            .err()
            .map(|error| sqlite_database_error(&error))
            .unwrap_or(original))
    };
    #[cfg(test)]
    if committing && result.is_ok() {
        checkpoints::reach(&lease.owner().query_run_id, checkpoints::Phase::AfterCommit);
    }
    // Drop rolls back after a failed commit, but its error is ignored by
    // rusqlite. Confirm cleanup explicitly before allowing another execution.
    if !connection.is_autocommit() {
        if let Err(error) = connection.execute_batch("ROLLBACK") {
            result = Err(sqlite_database_error(&error));
        }
    }
    let transaction_pending = !connection.is_autocommit();
    let effect = probe.map_or(EffectOutcome::Unknown, |probe| {
        effect_outcome_from_completion(probe.finish(false, committing && result.is_ok()))
    });
    let mut outcome = match result {
        Ok(()) => P6UnitOutcome {
            result: StatementExecutionResult::Execute {
                affected_rows: Some("1".into()),
            },
            effect_outcome: effect,
            stop: false,
            connection_terminated: false,
        },
        Err(error) => failed(error, effect, stopped(actor, lease, &caller)),
    };
    // Unknown completion can also mean hook removal failed. Never leave an
    // old owner's commit veto installed on a reusable connection.
    if !cancellation_cleared
        || transaction_pending
        || outcome.effect_outcome == EffectOutcome::Unknown
    {
        let _ = actor.mark_connection_terminated(lease);
        outcome.connection_terminated = true;
        outcome.effect_outcome = EffectOutcome::Unknown;
    }
    Ok((outcome, transaction_pending))
}

#[cfg(test)]
pub(super) mod checkpoints {
    use super::*;
    use std::sync::mpsc::{Receiver, Sender};

    #[derive(Clone, Copy, Hash, PartialEq, Eq)]
    pub(in super::super) enum Phase {
        AfterUpdate,
        BeforeCommit,
        AfterCommit,
    }

    type Latch = (Sender<()>, Receiver<()>);
    type Latches = Mutex<HashMap<(QueryRunId, Phase), Latch>>;
    fn latches() -> &'static Latches {
        static LATCHES: std::sync::OnceLock<Latches> = std::sync::OnceLock::new();
        LATCHES.get_or_init(Mutex::default)
    }

    pub(in super::super) fn install(owner: QueryRunId, phase: Phase) -> (Receiver<()>, Sender<()>) {
        let (arrived, arrival) = std::sync::mpsc::channel();
        let (resume, resumed) = std::sync::mpsc::channel();
        assert!(latches()
            .lock()
            .unwrap()
            .insert((owner, phase), (arrived, resumed))
            .is_none());
        (arrival, resume)
    }

    pub(super) fn reach(owner: &QueryRunId, phase: Phase) {
        let latch = latches().lock().unwrap().remove(&(owner.clone(), phase));
        if let Some((arrived, resumed)) = latch {
            arrived.send(()).unwrap();
            resumed
                .recv_timeout(Duration::from_secs(3))
                .expect("cell mutation test checkpoint was not released");
        }
    }
}
