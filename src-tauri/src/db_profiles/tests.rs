use super::*;
use crate::db_service::{PostgresInsecureException, PostgresTransportMode};
use secrecy::{ExposeSecret, SecretString};
use std::sync::atomic::{AtomicUsize, Ordering};

const SENTINEL: &str = "YUZORA_PROFILE_SECRET_SENTINEL";

fn runtime_connection(descriptor_id: &str, suffix: &str) -> LiveConnection {
    LiveConnection {
        descriptor_id: DescriptorId(descriptor_id.to_string()),
        connection_id: ConnectionId(format!("connection-{suffix}")),
        connection_generation: ConnectionGeneration(format!("generation-{suffix}")),
        engine: LiveDatabaseEngine::Sqlite,
    }
}

fn termination_finalizer_fixture(
    suffix: &str,
) -> (
    DbState,
    ResultSessionState,
    DatabaseProfileState,
    db_service::ConnectionIdentity,
) {
    let database_state = DbState::default();
    let result_sessions = ResultSessionState::default();
    let runtime = ProfileRuntimeRegistry::default();
    let connection = runtime_connection(&format!("descriptor-{suffix}"), suffix);
    runtime.insert(connection.clone()).unwrap();
    let identity = db_service::ConnectionIdentity {
        descriptor_id: connection.descriptor_id.clone(),
        connection_id: connection.connection_id.clone(),
        connection_generation: connection.connection_generation.clone(),
    };
    db_service::register_actor(
        &database_state,
        Arc::new(crate::db_connection_actor::ProductionConnectionActor::new(
            identity.clone(),
            DbHandle::Sqlite(Mutex::new(rusqlite::Connection::open_in_memory().unwrap())),
        )),
    )
    .unwrap();
    let run_owner = db_service::QueryRunOwner {
        descriptor_id: identity.descriptor_id.clone(),
        connection_id: identity.connection_id.clone(),
        connection_generation: identity.connection_generation.clone(),
        query_run_id: db_service::QueryRunId(format!("run-{suffix}")),
    };
    let session_owner = db_service::ResultSessionOwner {
        descriptor_id: identity.descriptor_id.clone(),
        connection_id: identity.connection_id.clone(),
        connection_generation: identity.connection_generation.clone(),
        query_run_id: run_owner.query_run_id.clone(),
        statement_execution_id: db_service::StatementExecutionId(format!("statement-{suffix}")),
        result_session_id: db_service::ResultSessionId(format!("session-{suffix}")),
    };
    {
        let mut sessions = result_sessions.lock().unwrap();
        sessions.begin_run(&run_owner).unwrap();
        sessions
            .begin_session(session_owner, vec!["value".to_string()])
            .unwrap();
    }
    let profile_state = DatabaseProfileState {
        profiles: Arc::new(Mutex::new(DatabaseProfiles::new(
            Arc::new(FakeProfileRepository::default()),
            Arc::new(TestVault::default()),
            Arc::new(NoLiveProfileCloser),
        ))),
        runtime,
        database_state: database_state.clone(),
        result_sessions: result_sessions.clone(),
        opener: Arc::new(ProductionDatabaseConnectionOpener),
    };
    (database_state, result_sessions, profile_state, identity)
}

fn finalize_as_cancel(
    database_state: &DbState,
    result_sessions: &ResultSessionState,
    profile_state: &DatabaseProfileState,
    identity: &db_service::ConnectionIdentity,
) -> Result<db_service::QueryCancelResult, db_service::DatabaseOperationalError> {
    db_service::finalize_terminated_connection(
        database_state,
        result_sessions,
        profile_state,
        identity,
    )?;
    Ok(db_service::QueryCancelResult {
        outcome: db_service::QueryCancelOutcome::CancelledConnectionTerminated,
    })
}

#[test]
fn termination_finalizer_is_order_independent_when_run_finalizes_first() {
    let (database_state, result_sessions, profile_state, identity) =
        termination_finalizer_fixture("run-first");

    db_service::finalize_terminated_connection(
        &database_state,
        &result_sessions,
        &profile_state,
        &identity,
    )
    .unwrap();
    let cancel =
        finalize_as_cancel(&database_state, &result_sessions, &profile_state, &identity).unwrap();

    assert_eq!(
        cancel.outcome,
        db_service::QueryCancelOutcome::CancelledConnectionTerminated
    );
    assert!(!db_service::has_exact_actor(&database_state, &identity));
    assert!(profile_state.runtime.get(&identity.descriptor_id).is_none());
    assert_eq!(result_sessions.lock().unwrap().session_count(), 0);
}

#[test]
fn termination_finalizer_is_order_independent_when_cancel_finalizes_first() {
    let (database_state, result_sessions, profile_state, identity) =
        termination_finalizer_fixture("cancel-first");

    let cancel =
        finalize_as_cancel(&database_state, &result_sessions, &profile_state, &identity).unwrap();
    db_service::finalize_terminated_connection(
        &database_state,
        &result_sessions,
        &profile_state,
        &identity,
    )
    .unwrap();

    assert_eq!(
        cancel.outcome,
        db_service::QueryCancelOutcome::CancelledConnectionTerminated
    );
    assert!(!db_service::has_exact_actor(&database_state, &identity));
    assert!(profile_state.runtime.get(&identity.descriptor_id).is_none());
    assert_eq!(result_sessions.lock().unwrap().session_count(), 0);
}

#[tokio::test]
async fn app_shutdown_settles_stream_and_resets_actor_profile_and_session_registries() {
    let database_state = DbState::default();
    let result_sessions = ResultSessionState::default();
    let runtime = ProfileRuntimeRegistry::default();
    let live = runtime_connection("descriptor-shutdown-live", "shutdown-live");
    let closing = runtime_connection("descriptor-shutdown-closing", "shutdown-closing");
    runtime.insert(live.clone()).unwrap();
    runtime.insert(closing.clone()).unwrap();
    assert_eq!(
        runtime.begin_close(&closing.descriptor_id).unwrap(),
        Some(closing.clone())
    );
    let opening_completion = match runtime
        .begin_open(&DescriptorId("descriptor-shutdown-opening".to_string()), 1)
        .unwrap()
    {
        OpenDecision::Open(reservation) => reservation.completion,
        _ => panic!("expected deterministic opening reservation"),
    };
    runtime.connections.lock().unwrap().terminated.insert(
        "descriptor-shutdown-tombstone".to_string(),
        db_service::ConnectionIdentity {
            descriptor_id: DescriptorId("descriptor-shutdown-tombstone".to_string()),
            connection_id: ConnectionId("connection-shutdown-tombstone".to_string()),
            connection_generation: ConnectionGeneration(
                "generation-shutdown-tombstone".to_string(),
            ),
        },
    );

    let live_identity = db_service::ConnectionIdentity {
        descriptor_id: live.descriptor_id.clone(),
        connection_id: live.connection_id.clone(),
        connection_generation: live.connection_generation.clone(),
    };
    let live_actor = Arc::new(crate::db_connection_actor::ProductionConnectionActor::new(
        live_identity.clone(),
        DbHandle::Sqlite(Mutex::new(rusqlite::Connection::open_in_memory().unwrap())),
    ));
    db_service::register_actor(&database_state, Arc::clone(&live_actor)).unwrap();
    let closing_identity = db_service::ConnectionIdentity {
        descriptor_id: closing.descriptor_id.clone(),
        connection_id: closing.connection_id.clone(),
        connection_generation: closing.connection_generation.clone(),
    };
    db_service::register_actor(
        &database_state,
        Arc::new(crate::db_connection_actor::ProductionConnectionActor::new(
            closing_identity,
            DbHandle::Sqlite(Mutex::new(rusqlite::Connection::open_in_memory().unwrap())),
        )),
    )
    .unwrap();

    let run_owner = db_service::QueryRunOwner {
        descriptor_id: live_identity.descriptor_id.clone(),
        connection_id: live_identity.connection_id.clone(),
        connection_generation: live_identity.connection_generation.clone(),
        query_run_id: db_service::QueryRunId("run-shutdown-live".to_string()),
    };
    let lease = live_actor
        .acquire_execution(
            run_owner.clone(),
            crate::db_connection_actor::CancelCapability::SqliteInterrupt,
        )
        .unwrap();
    let session_owner = db_service::ResultSessionOwner {
        descriptor_id: run_owner.descriptor_id.clone(),
        connection_id: run_owner.connection_id.clone(),
        connection_generation: run_owner.connection_generation.clone(),
        query_run_id: run_owner.query_run_id.clone(),
        statement_execution_id: db_service::StatementExecutionId(
            "statement-shutdown-live".to_string(),
        ),
        result_session_id: db_service::ResultSessionId("session-shutdown-live".to_string()),
    };
    {
        let mut sessions = result_sessions.lock().unwrap();
        sessions.begin_run(&run_owner).unwrap();
        sessions
            .begin_session(session_owner.clone(), vec!["value".to_string()])
            .unwrap();
        sessions
            .push_row(
                &session_owner,
                vec![db_service::DbValue::Text {
                    value: "cached".to_string(),
                }],
            )
            .unwrap();
    }
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    live_actor
        .install_result_continuation(&lease, session_owner.clone(), sender)
        .unwrap();
    let worker_actor = Arc::clone(&live_actor);
    let worker_lease = lease.clone();
    let worker = tokio::spawn(async move {
        assert!(receiver.recv().await.is_none());
        worker_actor.settle_execution(&worker_lease).unwrap();
    });

    let profiles = DatabaseProfiles::new(
        Arc::new(FakeProfileRepository::default()),
        Arc::new(TestVault::default()),
        Arc::new(NoLiveProfileCloser),
    );
    let state = DatabaseProfileState::deterministic_test(
        profiles,
        runtime.clone(),
        database_state.clone(),
        result_sessions.clone(),
    );
    let report = state
        .shutdown_database_runtime(db_service::DatabaseShutdownTimeouts {
            per_actor: std::time::Duration::from_secs(1),
            overall: std::time::Duration::from_secs(1),
        })
        .await;
    worker.await.unwrap();

    assert!(!report.has_failures(), "{report:?}");
    assert_eq!(report.database.snapshot_count, 2);
    assert_eq!(report.database.registry_remaining, Some(0));
    assert_eq!(report.profiles.opening, 1);
    assert_eq!(report.profiles.live, 1);
    assert_eq!(report.profiles.closing, 1);
    assert_eq!(report.profiles.tombstones, 1);
    assert!(report.profiles.reset);
    assert_eq!(report.result_sessions.sessions_before, 1);
    assert!(report.result_sessions.bytes_before > 0);
    assert_eq!(report.result_sessions.sessions_after, 0);
    assert_eq!(report.result_sessions.bytes_after, 0);
    assert!(database_state.0.lock().unwrap().is_empty());
    let runtime_state = runtime.connections.lock().unwrap();
    assert!(runtime_state.entries.is_empty());
    assert!(runtime_state.terminated.is_empty());
    drop(runtime_state);
    let opening_error = opening_completion.wait().await.unwrap_err();
    assert_eq!(opening_error.code, ProfileErrorCode::StaleConnection);
    let mut sessions = result_sessions.lock().unwrap();
    assert_eq!(sessions.session_count(), 0);
    assert_eq!(sessions.total_bytes(), 0);
    assert_eq!(
        sessions.begin_session(session_owner, vec!["value".to_string()]),
        Err(crate::db_result_session::SessionError::OwnerMismatch),
        "shutdown must clear the old active run as well as its cache"
    );
    drop(sessions);

    let repeated = state
        .shutdown_database_runtime(db_service::DatabaseShutdownTimeouts {
            per_actor: std::time::Duration::from_millis(20),
            overall: std::time::Duration::from_millis(20),
        })
        .await;
    assert!(repeated.database.already_started);
    assert_eq!(repeated.database.snapshot_count, 0);
    assert_eq!(
        repeated.profiles,
        ProfileRuntimeShutdownReport {
            reset: true,
            ..ProfileRuntimeShutdownReport::default()
        }
    );
    assert!(!repeated.has_failures(), "{repeated:?}");
}

struct DeferredProductionPathOpener {
    started: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    release: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    calls: AtomicUsize,
}

impl DeferredProductionPathOpener {
    fn new() -> (
        Arc<Self>,
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Sender<()>,
    ) {
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        (
            Arc::new(Self {
                started: Mutex::new(Some(started_tx)),
                release: Mutex::new(Some(release_rx)),
                calls: AtomicUsize::new(0),
            }),
            started_rx,
            release_tx,
        )
    }
}

impl DatabaseConnectionOpener for DeferredProductionPathOpener {
    fn open(&self, config: DbOpenConfig) -> DatabaseOpenFuture<'_> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let started = self.started.lock().unwrap().take();
        let release = self.release.lock().unwrap().take();
        Box::pin(async move {
            if let Some(started) = started {
                let _ = started.send(());
            }
            if let Some(release) = release {
                let _ = release.await;
            }
            db_service::open_unregistered(config).await
        })
    }
}

fn deferred_sqlite_state(
    path: &Path,
    opener: Arc<dyn DatabaseConnectionOpener>,
) -> (Arc<DatabaseProfileState>, ProfileDescriptor) {
    let repository = Arc::new(FakeProfileRepository::default());
    let vault = Arc::new(TestVault::default());
    let database_state = DbState::default();
    let runtime = ProfileRuntimeRegistry::default();
    let closer: Arc<dyn DatabaseLifecycleCloser> = Arc::new(RegisteredProfileCloser {
        database_state: database_state.clone(),
        runtime: runtime.clone(),
        result_sessions: ResultSessionState::default(),
    });
    let profiles = DatabaseProfiles::new(repository, vault, closer.clone());
    let profile = profiles
        .create(ProfileCreateRequest {
            name: "Deferred SQLite".to_string(),
            target: ProfileTarget::Sqlite {
                workspace: None,
                path: path.to_string_lossy().into_owned(),
            },
            credential: None,
            transport_challenge_id: None,
        })
        .unwrap();
    (
        Arc::new(DatabaseProfileState {
            profiles: Arc::new(Mutex::new(profiles)),
            runtime,
            database_state,
            result_sessions: ResultSessionState::default(),
            opener,
        }),
        profile,
    )
}

#[tokio::test]
async fn concurrent_descriptor_reservations_elect_exactly_one_engine_opener() {
    const CALLERS: usize = 12;
    let registry = ProfileRuntimeRegistry::default();
    let barrier = Arc::new(tokio::sync::Barrier::new(CALLERS));
    let opener_count = Arc::new(AtomicUsize::new(0));
    let mut tasks = Vec::new();
    for _ in 0..CALLERS {
        let registry = registry.clone();
        let barrier = Arc::clone(&barrier);
        let opener_count = Arc::clone(&opener_count);
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            let decision = registry
                .begin_open(&DescriptorId("descriptor-single-flight".to_string()), 7)
                .unwrap();
            if matches!(decision, OpenDecision::Open(_)) {
                opener_count.fetch_add(1, Ordering::SeqCst);
            }
            decision
        }));
    }

    let mut reservation = None;
    let mut waiters = Vec::new();
    for task in tasks {
        match task.await.unwrap() {
            OpenDecision::Open(candidate) => reservation = Some(candidate),
            OpenDecision::Wait(completion) => waiters.push(completion),
            OpenDecision::Live(_) => panic!("connection cannot be live before publication"),
            OpenDecision::Unavailable(_) => {
                panic!("opening descriptor cannot already be closing")
            }
        }
    }
    assert_eq!(opener_count.load(Ordering::SeqCst), 1);
    assert_eq!(waiters.len(), CALLERS - 1);

    let reservation = reservation.expect("one caller owns the opening ticket");
    let connection = runtime_connection("descriptor-single-flight", "single");
    assert!(registry
        .publish_open(&reservation, connection.clone())
        .unwrap());
    for waiter in waiters {
        assert_eq!(waiter.wait().await.unwrap(), connection);
    }
    match registry
        .begin_open(&DescriptorId("descriptor-single-flight".to_string()), 7)
        .unwrap()
    {
        OpenDecision::Live(current) => assert_eq!(current, connection),
        _ => panic!("published connection must deduplicate by descriptor"),
    }
}

#[tokio::test]
async fn invalidated_open_ticket_cannot_publish_a_late_connection() {
    let registry = ProfileRuntimeRegistry::default();
    let descriptor_id = DescriptorId("descriptor-invalidated".to_string());
    let reservation = match registry.begin_open(&descriptor_id, 3).unwrap() {
        OpenDecision::Open(reservation) => reservation,
        _ => panic!("first caller must own the opening ticket"),
    };
    let waiter = match registry.begin_open(&descriptor_id, 3).unwrap() {
        OpenDecision::Wait(completion) => completion,
        _ => panic!("second caller must join the opening ticket"),
    };

    assert_eq!(registry.invalidate_open(&descriptor_id).unwrap(), None);
    assert_eq!(
        waiter.wait().await.unwrap_err().code,
        ProfileErrorCode::StaleConnection
    );
    assert!(!registry
        .publish_open(
            &reservation,
            runtime_connection("descriptor-invalidated", "late")
        )
        .unwrap());
    assert!(registry.get(&descriptor_id).is_none());
}

#[test]
fn descriptor_closing_state_blocks_a_reopen_until_exact_teardown_settles() {
    let registry = ProfileRuntimeRegistry::default();
    let connection = runtime_connection("descriptor-closing", "one");
    registry.insert(connection.clone()).unwrap();
    assert_eq!(
        registry.begin_close(&connection.descriptor_id).unwrap(),
        Some(connection.clone())
    );
    match registry.begin_open(&connection.descriptor_id, 1).unwrap() {
        OpenDecision::Unavailable(error) => {
            assert_eq!(error.code, ProfileErrorCode::ConnectionBusy)
        }
        _ => panic!("closing descriptor must not expose or replace its actor"),
    }
    registry.finish_close(&connection, false).unwrap();
    assert_eq!(registry.get(&connection.descriptor_id), Some(connection));
}

#[tokio::test]
async fn connection_affecting_edit_during_engine_open_closes_the_exact_late_handle() {
    let original = tempfile::NamedTempFile::new().unwrap();
    let replacement = tempfile::NamedTempFile::new().unwrap();
    let (opener, started, release) = DeferredProductionPathOpener::new();
    let opener_for_assert = Arc::clone(&opener);
    let (state, profile) = deferred_sqlite_state(original.path(), opener);
    let descriptor_id = profile.descriptor_id.clone();
    let open_state = Arc::clone(&state);
    let open_descriptor = descriptor_id.clone();
    let pending = tokio::spawn(async move { open_state.open_saved(&open_descriptor).await });
    started.await.unwrap();

    state
        .update_profile(ProfileUpdateRequest {
            descriptor_id: descriptor_id.clone(),
            name: "Deferred SQLite moved".to_string(),
            target: ProfileTarget::Sqlite {
                workspace: None,
                path: replacement.path().to_string_lossy().into_owned(),
            },
            replacement_credential: None,
            transport_challenge_id: None,
        })
        .await
        .unwrap();
    release.send(()).unwrap();

    assert_eq!(
        pending.await.unwrap().unwrap_err().code,
        ProfileErrorCode::StaleConnection
    );
    assert_eq!(opener_for_assert.calls.load(Ordering::SeqCst), 1);
    assert!(state.runtime.get(&descriptor_id).is_none());
    assert!(state.database_state.0.lock().unwrap().is_empty());
}

#[tokio::test]
async fn concurrent_saved_open_uses_one_engine_opener_and_registers_the_returned_identity() {
    const CALLERS: usize = 10;
    let sqlite = tempfile::NamedTempFile::new().unwrap();
    let (opener, started, release) = DeferredProductionPathOpener::new();
    let opener_for_assert = Arc::clone(&opener);
    let (state, profile) = deferred_sqlite_state(sqlite.path(), opener);
    let mut tasks = Vec::new();
    for _ in 0..CALLERS {
        let state = Arc::clone(&state);
        let descriptor_id = profile.descriptor_id.clone();
        tasks.push(tokio::spawn(async move {
            state.open_saved(&descriptor_id).await
        }));
    }
    started.await.unwrap();
    tokio::task::yield_now().await;
    release.send(()).unwrap();

    let mut opened = Vec::new();
    for task in tasks {
        opened.push(task.await.unwrap().unwrap());
    }
    assert_eq!(opener_for_assert.calls.load(Ordering::SeqCst), 1);
    assert!(opened.iter().all(|connection| connection == &opened[0]));
    let actors = state.database_state.0.lock().unwrap();
    assert_eq!(actors.len(), 1);
    let actor = actors.get(&opened[0].connection_id.0).unwrap();
    assert_eq!(actor.identity().descriptor_id, opened[0].descriptor_id);
    assert_eq!(actor.identity().connection_id, opened[0].connection_id);
    assert_eq!(
        actor.identity().connection_generation,
        opened[0].connection_generation
    );
}

#[tokio::test]
async fn aborted_open_owner_drops_exact_ticket_and_wakes_joined_waiter() {
    let sqlite = tempfile::NamedTempFile::new().unwrap();
    let (opener, started, release) = DeferredProductionPathOpener::new();
    let (state, profile) = deferred_sqlite_state(sqlite.path(), opener);
    let owner_state = Arc::clone(&state);
    let owner_descriptor = profile.descriptor_id.clone();
    let owner = tokio::spawn(async move { owner_state.open_saved(&owner_descriptor).await });
    started.await.unwrap();

    let waiter_state = Arc::clone(&state);
    let waiter_descriptor = profile.descriptor_id.clone();
    let waiter = tokio::spawn(async move { waiter_state.open_saved(&waiter_descriptor).await });
    for _ in 0..100 {
        if state.runtime.opening_waiter_count(&profile.descriptor_id) >= 1 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(
        state.runtime.opening_waiter_count(&profile.descriptor_id) >= 1,
        "second caller never joined the elected opener"
    );
    owner.abort();
    assert!(owner.await.unwrap_err().is_cancelled());

    let waiter_error = tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
        .await
        .expect("joined opener waiter must be notified when its owner is aborted")
        .unwrap()
        .unwrap_err();
    assert_eq!(waiter_error.code, ProfileErrorCode::StaleConnection);
    assert!(state.runtime.get(&profile.descriptor_id).is_none());
    assert!(state.database_state.0.lock().unwrap().is_empty());
    assert!(
        release.send(()).is_err(),
        "aborted opener must drop its wait"
    );
}

#[tokio::test]
async fn reconnect_discards_a_runtime_identity_whose_exact_actor_is_gone() {
    let sqlite = tempfile::NamedTempFile::new().unwrap();
    let (state, profile) =
        deferred_sqlite_state(sqlite.path(), Arc::new(ProductionDatabaseConnectionOpener));
    let stale = LiveConnection {
        descriptor_id: profile.descriptor_id.clone(),
        connection_id: ConnectionId("connection-gone".to_string()),
        connection_generation: ConnectionGeneration("generation-gone".to_string()),
        engine: LiveDatabaseEngine::Sqlite,
    };
    state.runtime.insert(stale.clone()).unwrap();
    let stale_run = db_service::QueryRunOwner {
        descriptor_id: stale.descriptor_id.clone(),
        connection_id: stale.connection_id.clone(),
        connection_generation: stale.connection_generation.clone(),
        query_run_id: db_service::QueryRunId("run-gone".to_string()),
    };
    let stale_session = db_service::ResultSessionOwner {
        descriptor_id: stale.descriptor_id.clone(),
        connection_id: stale.connection_id.clone(),
        connection_generation: stale.connection_generation.clone(),
        query_run_id: stale_run.query_run_id.clone(),
        statement_execution_id: db_service::StatementExecutionId("statement-gone".to_string()),
        result_session_id: db_service::ResultSessionId("session-gone".to_string()),
    };
    {
        let mut sessions = state.result_sessions.lock().unwrap();
        sessions.begin_run(&stale_run).unwrap();
        sessions
            .begin_session(stale_session, vec!["value".to_string()])
            .unwrap();
    }
    assert_eq!(state.result_sessions.lock().unwrap().session_count(), 1);

    let reconnected = state.open_saved(&profile.descriptor_id).await.unwrap();

    assert_ne!(reconnected.connection_id, stale.connection_id);
    assert_ne!(
        reconnected.connection_generation,
        stale.connection_generation
    );
    assert_eq!(state.runtime.get(&profile.descriptor_id), Some(reconnected));
    assert_eq!(state.database_state.0.lock().unwrap().len(), 1);
    assert_eq!(state.result_sessions.lock().unwrap().session_count(), 0);
}

#[tokio::test]
async fn forget_during_engine_open_closes_the_exact_late_handle_without_a_ghost() {
    let original = tempfile::NamedTempFile::new().unwrap();
    let (opener, started, release) = DeferredProductionPathOpener::new();
    let (state, profile) = deferred_sqlite_state(original.path(), opener);
    let descriptor_id = profile.descriptor_id.clone();
    let open_state = Arc::clone(&state);
    let open_descriptor = descriptor_id.clone();
    let pending = tokio::spawn(async move { open_state.open_saved(&open_descriptor).await });
    started.await.unwrap();

    let forget_descriptor = descriptor_id.clone();
    let snapshot = state
        .with_profiles(move |profiles| profiles.forget(&forget_descriptor))
        .await
        .unwrap();
    assert!(snapshot.profiles.is_empty());
    release.send(()).unwrap();

    assert_eq!(
        pending.await.unwrap().unwrap_err().code,
        ProfileErrorCode::StaleConnection
    );
    assert!(state.runtime.get(&descriptor_id).is_none());
    assert!(state.database_state.0.lock().unwrap().is_empty());
}

#[derive(Clone, Copy)]
enum TestVaultOperation {
    Store,
    Resolve,
    Delete,
}

#[derive(Default)]
struct TestVaultState {
    values: HashMap<(String, String), SecretString>,
    failures: VecDeque<(TestVaultOperation, VaultErrorKind)>,
    store_calls: usize,
    resolve_calls: usize,
    delete_calls: usize,
}

#[derive(Default)]
struct TestVault {
    state: Mutex<TestVaultState>,
}

impl TestVault {
    fn fail_next(&self, operation: TestVaultOperation, kind: VaultErrorKind) {
        self.state
            .lock()
            .unwrap()
            .failures
            .push_back((operation, kind));
    }

    fn counts(&self) -> (usize, usize, usize) {
        let state = self.state.lock().unwrap();
        (state.store_calls, state.resolve_calls, state.delete_calls)
    }

    fn generation_count(&self, descriptor_id: &str) -> usize {
        self.state
            .lock()
            .unwrap()
            .values
            .keys()
            .filter(|(descriptor, _)| descriptor == descriptor_id)
            .count()
    }

    fn take_failure(
        state: &mut TestVaultState,
        operation: TestVaultOperation,
    ) -> Option<VaultError> {
        let matches = state
            .failures
            .front()
            .map(|(queued, _)| std::mem::discriminant(queued) == std::mem::discriminant(&operation))
            .unwrap_or(false);
        matches.then(|| {
            let (_, kind) = state.failures.pop_front().unwrap();
            VaultError::new(kind)
        })
    }
}

impl DatabaseCredentialStore for TestVault {
    fn store(
        &self,
        descriptor_id: &DescriptorId,
        generation: &CredentialGeneration,
        secret: SecretString,
    ) -> Result<(), VaultError> {
        let mut state = self.state.lock().unwrap();
        state.store_calls += 1;
        if let Some(error) = Self::take_failure(&mut state, TestVaultOperation::Store) {
            return Err(error);
        }
        state
            .values
            .insert((descriptor_id.0.clone(), generation.0.clone()), secret);
        Ok(())
    }

    fn resolve(
        &self,
        descriptor_id: &DescriptorId,
        generation: &CredentialGeneration,
    ) -> Result<SecretString, VaultError> {
        let mut state = self.state.lock().unwrap();
        state.resolve_calls += 1;
        if let Some(error) = Self::take_failure(&mut state, TestVaultOperation::Resolve) {
            return Err(error);
        }
        state
            .values
            .get(&(descriptor_id.0.clone(), generation.0.clone()))
            .cloned()
            .ok_or_else(|| VaultError::new(VaultErrorKind::Missing))
    }

    fn delete(
        &self,
        descriptor_id: &DescriptorId,
        generation: &CredentialGeneration,
    ) -> Result<(), VaultError> {
        let mut state = self.state.lock().unwrap();
        state.delete_calls += 1;
        if let Some(error) = Self::take_failure(&mut state, TestVaultOperation::Delete) {
            return Err(error);
        }
        state
            .values
            .remove(&(descriptor_id.0.clone(), generation.0.clone()));
        Ok(())
    }
}

fn harness() -> (
    DatabaseProfiles,
    Arc<FakeProfileRepository>,
    Arc<TestVault>,
    Arc<FakeLifecycleCloser>,
) {
    let repository = Arc::new(FakeProfileRepository::default());
    let vault = Arc::new(TestVault::default());
    let closer = Arc::new(FakeLifecycleCloser::default());
    let profiles = DatabaseProfiles::new(repository.clone(), vault.clone(), closer.clone());
    (profiles, repository, vault, closer)
}

fn postgres_request(secret: &str) -> ProfileCreateRequest {
    ProfileCreateRequest {
        name: "Production".to_string(),
        target: ProfileTarget::postgres(
            "db.internal",
            5432,
            "app",
            "alice",
            PostgresTransportMode::VerifyFull,
        ),
        credential: Some(CredentialInput {
            password: SecretString::from(secret),
        }),
        transport_challenge_id: None,
    }
}

fn sqlite_profile(id: &str) -> StoredProfile {
    StoredProfile {
        descriptor_id: DescriptorId(id.to_string()),
        config_generation: 1,
        name: "Local".to_string(),
        target: ProfileTarget::Sqlite {
            workspace: None,
            path: "/tmp/local.sqlite".to_string(),
        },
        credential_state: CredentialState::NotRequired,
        active_credential_generation: None,
    }
}

fn postgres_profile(
    id: &str,
    credential_state: CredentialState,
    generation: Option<&str>,
) -> StoredProfile {
    StoredProfile {
        descriptor_id: DescriptorId(id.to_string()),
        config_generation: 1,
        name: "Production".to_string(),
        target: ProfileTarget::postgres(
            "db.internal",
            5432,
            "app",
            "alice",
            PostgresTransportMode::VerifyFull,
        ),
        credential_state,
        active_credential_generation: generation
            .map(|generation| CredentialGeneration(generation.to_string())),
    }
}

#[test]
fn file_repository_atomically_reopens_the_last_document() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("profiles.json");
    let repository = FileProfileRepository::new(path.clone());
    assert_eq!(repository.load().unwrap(), ProfileDocument::default());

    let mut document = ProfileDocument::default();
    document.profiles.push(sqlite_profile("descriptor-1"));
    repository.replace(&document).unwrap();

    let reopened = FileProfileRepository::new(path);
    assert_eq!(reopened.load().unwrap(), document);
}

#[test]
fn config_generation_defaults_for_p2_documents_and_increments_on_target_edit() {
    let legacy: StoredProfile = serde_json::from_value(serde_json::json!({
        "descriptorId": "descriptor-legacy",
        "name": "Legacy",
        "target": { "kind": "sqlite", "path": "/tmp/legacy.sqlite" },
        "credentialState": "notRequired",
        "activeCredentialGeneration": null
    }))
    .unwrap();
    assert_eq!(legacy.config_generation, 1);

    let (profiles, _, _, _) = harness();
    let created = profiles
        .create(ProfileCreateRequest {
            name: "Local".to_string(),
            target: ProfileTarget::Sqlite {
                workspace: None,
                path: "/tmp/first.sqlite".to_string(),
            },
            credential: None,
            transport_challenge_id: None,
        })
        .unwrap();
    assert_eq!(created.config_generation, 1);
    let updated = profiles
        .update(ProfileUpdateRequest {
            descriptor_id: created.descriptor_id,
            name: "Local moved".to_string(),
            target: ProfileTarget::Sqlite {
                workspace: None,
                path: "/tmp/second.sqlite".to_string(),
            },
            replacement_credential: None,
            transport_challenge_id: None,
        })
        .unwrap();
    assert_eq!(updated.config_generation, 2);
}

#[test]
fn runtime_isolates_same_target_profiles_by_opaque_descriptor() {
    let runtime = ProfileRuntimeRegistry::default();
    let alice = runtime_connection("descriptor-alice", "alice");
    let bob_tls = runtime_connection("descriptor-bob-tls", "bob-tls");

    runtime.insert(alice.clone()).unwrap();
    runtime.insert(bob_tls.clone()).unwrap();

    assert_eq!(runtime.get(&alice.descriptor_id), Some(alice));
    assert_eq!(runtime.get(&bob_tls.descriptor_id), Some(bob_tls));
    assert_eq!(runtime.connections.lock().unwrap().entries.len(), 2);
}

#[test]
fn fake_repository_failures_never_replace_the_durable_snapshot_before_rename() {
    let repository = FakeProfileRepository::default();
    let original = ProfileDocument::default();
    repository.replace(&original).unwrap();
    let mut changed = original.clone();
    changed.profiles.push(sqlite_profile("descriptor-1"));

    let cases = [
        (
            RepositoryFailurePoint::Permission,
            ProfileRepositoryErrorKind::PermissionDenied,
        ),
        (
            RepositoryFailurePoint::Quota,
            ProfileRepositoryErrorKind::QuotaExceeded,
        ),
        (
            RepositoryFailurePoint::TempWrite,
            ProfileRepositoryErrorKind::TempWriteFailed,
        ),
        (
            RepositoryFailurePoint::Sync,
            ProfileRepositoryErrorKind::SyncFailed,
        ),
        (
            RepositoryFailurePoint::Rename,
            ProfileRepositoryErrorKind::RenameFailed,
        ),
    ];
    for (point, expected) in cases {
        repository.fail_next(point);
        assert_eq!(repository.replace(&changed).unwrap_err().kind(), expected);
        assert_eq!(repository.reopen().load().unwrap(), original);
    }
}

#[test]
fn parent_sync_failure_reports_uncertainty_but_reopen_observes_the_atomic_replace() {
    let repository = FakeProfileRepository::default();
    let mut changed = ProfileDocument::default();
    changed.profiles.push(sqlite_profile("descriptor-1"));
    repository.fail_next(RepositoryFailurePoint::ParentSync);

    assert_eq!(
        repository.replace(&changed).unwrap_err().kind(),
        ProfileRepositoryErrorKind::ParentSyncFailed
    );
    assert_eq!(repository.reopen().load().unwrap(), changed);
}

#[test]
fn repository_rejects_duplicate_descriptors_and_pending_rows() {
    let repository = FakeProfileRepository::default();
    let profile = sqlite_profile("descriptor-1");
    let mut duplicate = ProfileDocument::default();
    duplicate.profiles = vec![profile.clone(), profile.clone()];
    assert_eq!(
        repository.replace(&duplicate).unwrap_err().kind(),
        ProfileRepositoryErrorKind::Corrupt
    );

    let generation = CredentialGeneration("credential-1".to_string());
    let operation = PendingOperation::PendingCreate {
        operation_id: "operation-1".to_string(),
        profile,
        credential_generation: generation,
    };
    let mut duplicate_pending = ProfileDocument::default();
    duplicate_pending.pending_operations = vec![operation.clone(), operation];
    assert_eq!(
        repository.replace(&duplicate_pending).unwrap_err().kind(),
        ProfileRepositoryErrorKind::Corrupt
    );
}

#[test]
fn repository_rejects_inconsistent_profile_credential_state() {
    let repository = FakeProfileRepository::default();
    let mut sqlite_required = sqlite_profile("sqlite-required");
    sqlite_required.credential_state = CredentialState::Required;
    let mut sqlite_stored = sqlite_profile("sqlite-stored");
    sqlite_stored.credential_state = CredentialState::Stored;
    sqlite_stored.active_credential_generation =
        Some(CredentialGeneration("credential-sqlite".to_string()));
    let cases = [
        (
            "stored network profile without an active generation",
            postgres_profile("stored-missing", CredentialState::Stored, None),
        ),
        (
            "required network profile with an active generation",
            postgres_profile(
                "required-active",
                CredentialState::Required,
                Some("credential-required"),
            ),
        ),
        (
            "network profile claiming credentials are not required",
            postgres_profile("network-not-required", CredentialState::NotRequired, None),
        ),
        (
            "unavailable network profile without a known generation",
            postgres_profile("unavailable-missing", CredentialState::Unavailable, None),
        ),
        (
            "SQLite profile claiming a credential is required",
            sqlite_required,
        ),
        (
            "SQLite profile carrying a credential generation",
            sqlite_stored,
        ),
    ];

    for (case, profile) in cases {
        let document = ProfileDocument {
            profiles: vec![profile],
            ..ProfileDocument::default()
        };
        assert_eq!(
            repository.replace(&document).unwrap_err().kind(),
            ProfileRepositoryErrorKind::Corrupt,
            "{case}"
        );
    }

    let valid_unavailable = ProfileDocument {
        profiles: vec![postgres_profile(
            "unavailable-known",
            CredentialState::Unavailable,
            Some("credential-known"),
        )],
        ..ProfileDocument::default()
    };
    repository.replace(&valid_unavailable).unwrap();
}

#[test]
fn repository_rejects_referentially_inconsistent_pending_operations() {
    let old_generation = CredentialGeneration("credential-old".to_string());
    let new_generation = CredentialGeneration("credential-new".to_string());
    let current = postgres_profile(
        "descriptor-1",
        CredentialState::Stored,
        Some(&old_generation.0),
    );
    let replacement = postgres_profile(
        "descriptor-1",
        CredentialState::Stored,
        Some(&new_generation.0),
    );
    let pending_create_profile = postgres_profile("descriptor-1", CredentialState::Stored, None);
    let cases = vec![
        (
            "pendingCreate descriptor already exists",
            ProfileDocument {
                profiles: vec![current.clone()],
                pending_operations: vec![PendingOperation::PendingCreate {
                    operation_id: "operation-create".to_string(),
                    profile: pending_create_profile,
                    credential_generation: new_generation.clone(),
                }],
                ..ProfileDocument::default()
            },
        ),
        (
            "pendingReplace current descriptor is missing",
            ProfileDocument {
                pending_operations: vec![PendingOperation::PendingReplace {
                    operation_id: "operation-replace-missing".to_string(),
                    descriptor_id: DescriptorId("descriptor-1".to_string()),
                    replacement: replacement.clone(),
                    old_generation: Some(old_generation.clone()),
                    new_generation: new_generation.clone(),
                }],
                ..ProfileDocument::default()
            },
        ),
        (
            "pendingReplace replacement descriptor does not match",
            ProfileDocument {
                profiles: vec![current.clone()],
                pending_operations: vec![PendingOperation::PendingReplace {
                    operation_id: "operation-replace-descriptor".to_string(),
                    descriptor_id: DescriptorId("descriptor-1".to_string()),
                    replacement: postgres_profile(
                        "descriptor-2",
                        CredentialState::Stored,
                        Some(&new_generation.0),
                    ),
                    old_generation: Some(old_generation.clone()),
                    new_generation: new_generation.clone(),
                }],
                ..ProfileDocument::default()
            },
        ),
        (
            "pendingReplace old generation is not current",
            ProfileDocument {
                profiles: vec![current.clone()],
                pending_operations: vec![PendingOperation::PendingReplace {
                    operation_id: "operation-replace-old".to_string(),
                    descriptor_id: DescriptorId("descriptor-1".to_string()),
                    replacement: replacement.clone(),
                    old_generation: Some(CredentialGeneration("credential-other".to_string())),
                    new_generation: new_generation.clone(),
                }],
                ..ProfileDocument::default()
            },
        ),
        (
            "pendingReplace replacement does not activate new generation",
            ProfileDocument {
                profiles: vec![current.clone()],
                pending_operations: vec![PendingOperation::PendingReplace {
                    operation_id: "operation-replace-new".to_string(),
                    descriptor_id: DescriptorId("descriptor-1".to_string()),
                    replacement: postgres_profile(
                        "descriptor-1",
                        CredentialState::Stored,
                        Some("credential-other"),
                    ),
                    old_generation: Some(old_generation.clone()),
                    new_generation: new_generation.clone(),
                }],
                ..ProfileDocument::default()
            },
        ),
        (
            "cleanupOld active generation is not current",
            ProfileDocument {
                profiles: vec![current.clone()],
                pending_operations: vec![PendingOperation::CleanupOld {
                    operation_id: "operation-cleanup-active".to_string(),
                    descriptor_id: DescriptorId("descriptor-1".to_string()),
                    old_generation: CredentialGeneration("credential-older".to_string()),
                    active_generation: new_generation.clone(),
                }],
                ..ProfileDocument::default()
            },
        ),
        (
            "cleanupOld tries to delete the active generation",
            ProfileDocument {
                profiles: vec![current.clone()],
                pending_operations: vec![PendingOperation::CleanupOld {
                    operation_id: "operation-cleanup-same".to_string(),
                    descriptor_id: DescriptorId("descriptor-1".to_string()),
                    old_generation: old_generation.clone(),
                    active_generation: old_generation.clone(),
                }],
                ..ProfileDocument::default()
            },
        ),
        (
            "pendingForget descriptor is missing",
            ProfileDocument {
                pending_operations: vec![PendingOperation::PendingForget {
                    operation_id: "operation-forget-missing".to_string(),
                    descriptor_id: DescriptorId("descriptor-1".to_string()),
                    generations: vec![old_generation.clone()],
                }],
                ..ProfileDocument::default()
            },
        ),
        (
            "pendingForget repeats a generation",
            ProfileDocument {
                profiles: vec![current.clone()],
                pending_operations: vec![PendingOperation::PendingForget {
                    operation_id: "operation-forget-duplicate".to_string(),
                    descriptor_id: DescriptorId("descriptor-1".to_string()),
                    generations: vec![old_generation.clone(), old_generation.clone()],
                }],
                ..ProfileDocument::default()
            },
        ),
        (
            "pendingRemoveCredential names an unmanaged generation",
            ProfileDocument {
                profiles: vec![current],
                pending_operations: vec![PendingOperation::PendingRemoveCredential {
                    operation_id: "operation-remove-unknown".to_string(),
                    descriptor_id: DescriptorId("descriptor-1".to_string()),
                    generations: vec![CredentialGeneration("credential-other".to_string())],
                }],
                ..ProfileDocument::default()
            },
        ),
    ];

    for (case, document) in cases {
        let repository = FakeProfileRepository::default();
        assert_eq!(
            repository.replace(&document).unwrap_err().kind(),
            ProfileRepositoryErrorKind::Corrupt,
            "{case}"
        );
    }
}

#[test]
fn startup_load_reads_only_non_secret_repository_and_never_touches_vault() {
    let (profiles, repository, vault, _) = harness();
    let mut document = ProfileDocument::default();
    document.profiles.push(sqlite_profile("descriptor-1"));
    repository.replace(&document).unwrap();

    let loaded = profiles.load().unwrap();

    assert_eq!(loaded.profiles.len(), 1);
    assert!(loaded.recovery.is_empty());
    assert_eq!(vault.counts(), (0, 0, 0));
}

#[tokio::test]
async fn test_connection_uses_the_selected_opener_without_registering_an_actor() {
    let sqlite_file = tempfile::NamedTempFile::new().unwrap();
    let (opener, started, release) = DeferredProductionPathOpener::new();
    let (state, profile) = deferred_sqlite_state(sqlite_file.path(), opener.clone());
    let running = state.clone();
    let probe = tokio::spawn(async move {
        running
            .test_connection(TestConnectionRequest::Saved {
                descriptor_id: profile.descriptor_id,
            })
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), started)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(opener.calls.load(Ordering::SeqCst), 1);
    assert!(state.database_state.0.lock().unwrap().is_empty());
    release.send(()).unwrap();
    assert!(probe.await.unwrap().unwrap().server_version.is_some());
    assert!(state.database_state.0.lock().unwrap().is_empty());
}

#[tokio::test]
async fn test_connection_is_ephemeral_and_never_registers_a_live_handle() {
    let repository = Arc::new(FakeProfileRepository::default());
    let vault = Arc::new(TestVault::default());
    let closer: Arc<dyn DatabaseLifecycleCloser> = Arc::new(FakeLifecycleCloser::default());
    let database_state = DbState::default();
    let runtime = ProfileRuntimeRegistry::default();
    let state = DatabaseProfileState {
        profiles: Arc::new(Mutex::new(DatabaseProfiles::new(
            repository,
            vault.clone(),
            closer.clone(),
        ))),
        runtime: runtime.clone(),
        database_state: database_state.clone(),
        result_sessions: ResultSessionState::default(),
        opener: Arc::new(ProductionDatabaseConnectionOpener),
    };
    assert!(database_state.0.lock().unwrap().is_empty());
    assert!(runtime.connections.lock().unwrap().is_empty());
    let sqlite_file = tempfile::NamedTempFile::new().unwrap();

    let result = state
        .test_connection(TestConnectionRequest::Ephemeral {
            target: ProfileTarget::Sqlite {
                workspace: None,
                path: sqlite_file.path().to_string_lossy().into_owned(),
            },
            credential: None,
            transport_challenge_id: None,
        })
        .await
        .unwrap();

    assert!(result.server_version.is_some());
    assert!(database_state.0.lock().unwrap().is_empty());
    assert!(runtime.connections.lock().unwrap().is_empty());
    let loaded = state.profiles.lock().unwrap().load().unwrap();
    assert!(loaded.profiles.is_empty());
    assert!(loaded.recovery.is_empty());
    assert_eq!(vault.counts(), (0, 0, 0));
}

#[test]
fn connection_error_preserves_sanitized_postgres_diagnostics() {
    let diagnostic = db_service::DatabaseError {
        engine: db_service::DatabaseErrorEngine::Postgres,
        message: "password authentication failed".to_string(),
        code: Some("28P01".to_string()),
        position: None,
        detail: None,
        hint: Some("verify the saved credential".to_string()),
        retryability: db_service::Retryability::NotRetryable,
    };
    let profile_error = DatabaseProfileState::connection_error(
        db_service::DatabaseOperationalError::new(
            db_service::DatabaseOperationalErrorCode::ConnectionFailed,
            "database connection failed",
        )
        .with_database_error(diagnostic.clone()),
    );

    assert_eq!(profile_error.code, ProfileErrorCode::ConnectionFailed);
    assert_eq!(profile_error.error.as_deref(), Some(&diagnostic));
    assert_eq!(
        serde_json::to_value(&profile_error).unwrap(),
        serde_json::json!({
            "code": "connectionFailed",
            "message": "database connection failed",
            "error": diagnostic,
        })
    );
    let serialized = serde_json::to_string(&profile_error).unwrap();
    assert!(serialized.contains("28P01"));
    assert!(!serialized.contains("password\":\""));
}

#[tokio::test]
async fn sqlite_missing_path_is_typed_for_open_and_test_without_creating_a_file() {
    let directory = tempfile::tempdir().unwrap();
    let missing = directory.path().join("missing.sqlite");
    let (state, profile) =
        deferred_sqlite_state(&missing, Arc::new(ProductionDatabaseConnectionOpener));

    assert_eq!(
        state
            .open_saved(&profile.descriptor_id)
            .await
            .unwrap_err()
            .code,
        ProfileErrorCode::SqlitePathMissing
    );
    assert_eq!(
        state
            .test_connection(TestConnectionRequest::Ephemeral {
                target: ProfileTarget::Sqlite {
                    workspace: None,
                    path: missing.to_string_lossy().into_owned(),
                },
                credential: None,
                transport_challenge_id: None,
            })
            .await
            .unwrap_err()
            .code,
        ProfileErrorCode::SqlitePathMissing
    );
    assert!(!missing.exists());
    assert!(state.runtime.get(&profile.descriptor_id).is_none());
    assert!(state.database_state.0.lock().unwrap().is_empty());
}

#[tokio::test]
async fn profile_list_runtime_path_reads_ledger_without_resolving_the_vault() {
    let repository = Arc::new(FakeProfileRepository::default());
    let vault = Arc::new(TestVault::default());
    let closer: Arc<dyn DatabaseLifecycleCloser> = Arc::new(FakeLifecycleCloser::default());
    let profiles = DatabaseProfiles::new(repository, vault.clone(), closer.clone());
    let created = profiles.create(postgres_request(SENTINEL)).unwrap();
    let counts_before_list = vault.counts();
    let state = DatabaseProfileState {
        profiles: Arc::new(Mutex::new(profiles)),
        runtime: ProfileRuntimeRegistry::default(),
        database_state: DbState::default(),
        result_sessions: ResultSessionState::default(),
        opener: Arc::new(ProductionDatabaseConnectionOpener),
    };

    let loaded = state.list_profiles().await.unwrap();

    assert_eq!(loaded.profiles[0].descriptor_id, created.descriptor_id);
    assert!(loaded.recovery.is_empty());
    assert_eq!(vault.counts(), counts_before_list);
    assert!(state.database_state.0.lock().unwrap().is_empty());
    assert!(state.runtime.connections.lock().unwrap().is_empty());
}

#[tokio::test]
async fn keep_credential_target_edit_closes_registered_handle_before_repository_update() {
    let repository = Arc::new(FakeProfileRepository::default());
    let vault = Arc::new(TestVault::default());
    let database_state = DbState::default();
    let runtime = ProfileRuntimeRegistry::default();
    let closer: Arc<dyn DatabaseLifecycleCloser> = Arc::new(RegisteredProfileCloser {
        database_state: database_state.clone(),
        runtime: runtime.clone(),
        result_sessions: ResultSessionState::default(),
    });
    let profiles = DatabaseProfiles::new(repository.clone(), vault.clone(), closer.clone());
    let created = profiles.create(postgres_request(SENTINEL)).unwrap();
    let active_generation = repository.reopen().load().unwrap().profiles[0]
        .active_credential_generation
        .clone();
    let counts_before_update = vault.counts();
    let connection = LiveConnection {
        descriptor_id: created.descriptor_id.clone(),
        connection_id: ConnectionId("connection-edit".to_string()),
        connection_generation: ConnectionGeneration("generation-edit".to_string()),
        engine: LiveDatabaseEngine::Postgres,
    };
    runtime.insert(connection.clone()).unwrap();
    db_service::register_actor(
        &database_state,
        Arc::new(crate::db_connection_actor::ProductionConnectionActor::new(
            db_service::ConnectionIdentity {
                descriptor_id: connection.descriptor_id.clone(),
                connection_id: connection.connection_id.clone(),
                connection_generation: connection.connection_generation.clone(),
            },
            DbHandle::Sqlite(Mutex::new(rusqlite::Connection::open_in_memory().unwrap())),
        )),
    )
    .unwrap();
    let state = DatabaseProfileState {
        profiles: Arc::new(Mutex::new(profiles)),
        runtime: runtime.clone(),
        database_state: database_state.clone(),
        result_sessions: ResultSessionState::default(),
        opener: Arc::new(ProductionDatabaseConnectionOpener),
    };

    let updated = state
        .update_profile(ProfileUpdateRequest {
            descriptor_id: created.descriptor_id.clone(),
            name: "Production moved".to_string(),
            target: ProfileTarget::postgres(
                "db-moved.internal",
                5432,
                "app",
                "alice",
                PostgresTransportMode::VerifyFull,
            ),
            replacement_credential: None,
            transport_challenge_id: None,
        })
        .await
        .unwrap();

    assert!(database_state.0.lock().unwrap().is_empty());
    assert!(runtime.get(&created.descriptor_id).is_none());
    assert_eq!(updated.name, "Production moved");
    assert!(matches!(
        updated.target,
        ProfileTarget::Postgres { ref host, .. } if host == "db-moved.internal"
    ));
    let durable = repository.reopen().load().unwrap();
    assert_eq!(
        durable.profiles[0].active_credential_generation,
        active_generation
    );
    assert!(durable.pending_operations.is_empty());
    assert_eq!(vault.counts(), counts_before_update);
}

#[test]
fn keep_credential_target_edit_close_failure_preserves_repository_and_vault() {
    let (profiles, repository, vault, closer) = harness();
    let created = profiles.create(postgres_request("old-secret")).unwrap();
    let durable_before = repository.reopen().load().unwrap();
    let counts_before = vault.counts();
    closer.fail_next(LifecycleCloseErrorKind::CloseFailed);

    let error = profiles
        .update(ProfileUpdateRequest {
            descriptor_id: created.descriptor_id.clone(),
            name: "Production moved".to_string(),
            target: ProfileTarget::postgres(
                "db-moved.internal",
                5432,
                "app",
                "alice",
                PostgresTransportMode::VerifyFull,
            ),
            replacement_credential: None,
            transport_challenge_id: None,
        })
        .unwrap_err();

    assert_eq!(error.code, ProfileErrorCode::LifecycleCloseFailed);
    assert_eq!(closer.call_count(&created.descriptor_id.0), 1);
    assert_eq!(repository.reopen().load().unwrap(), durable_before);
    assert_eq!(vault.counts(), counts_before);
}

#[test]
fn create_persists_pending_before_vault_write_and_keeps_secret_out_of_repository() {
    let (profiles, repository, vault, _) = harness();
    repository.fail_next(RepositoryFailurePoint::Rename);

    let error = profiles.create(postgres_request(SENTINEL)).unwrap_err();

    assert_eq!(error.code, ProfileErrorCode::RepositoryUnavailable);
    assert_eq!(vault.counts(), (0, 0, 0));
    assert!(!String::from_utf8_lossy(&repository.durable_bytes()).contains(SENTINEL));
}

#[test]
fn vault_write_failure_reopens_as_pending_create_and_explicit_resume_is_idempotent() {
    let (profiles, repository, vault, closer) = harness();
    vault.fail_next(TestVaultOperation::Store, VaultErrorKind::WriteFailed);
    assert_eq!(
        profiles
            .create(postgres_request(SENTINEL))
            .unwrap_err()
            .code,
        ProfileErrorCode::VaultWriteFailed
    );
    let reopened = DatabaseProfiles::new(Arc::new(repository.reopen()), vault.clone(), closer);
    let pending = reopened.load().unwrap();
    assert!(pending.profiles.is_empty());
    assert_eq!(
        pending.recovery[0].kind,
        PendingOperationKind::PendingCreate
    );
    assert!(!String::from_utf8_lossy(&repository.durable_bytes()).contains(SENTINEL));

    let completed = reopened
        .recover(ProfileRecoveryRequest {
            operation_id: pending.recovery[0].operation_id.clone(),
            action: RecoveryAction::Resume,
            credential: Some(CredentialInput {
                password: SecretString::from(SENTINEL),
            }),
        })
        .unwrap();
    assert_eq!(
        completed.profiles[0].credential_state,
        CredentialState::Stored
    );
    assert!(completed.recovery.is_empty());
    assert_eq!(
        vault.generation_count(&completed.profiles[0].descriptor_id.0),
        1
    );
    assert_eq!(reopened.load().unwrap(), completed);
}

#[test]
fn create_final_replace_failure_reopens_pending_and_resume_keeps_single_generation() {
    let (profiles, repository, vault, closer) = harness();
    repository.fail_nth_replace(2, RepositoryFailurePoint::Rename);

    assert_eq!(
        profiles
            .create(postgres_request(SENTINEL))
            .unwrap_err()
            .code,
        ProfileErrorCode::RepositoryUnavailable
    );
    assert_eq!(vault.counts().0, 1);

    let durable_pending = repository.reopen().load().unwrap();
    assert!(durable_pending.profiles.is_empty());
    let (descriptor_id, generation) = match durable_pending.pending_operations.as_slice() {
        [PendingOperation::PendingCreate {
            profile,
            credential_generation,
            ..
        }] => (profile.descriptor_id.clone(), credential_generation.clone()),
        operations => panic!("expected one pendingCreate operation, got {operations:?}"),
    };
    assert_eq!(vault.generation_count(&descriptor_id.0), 1);

    let reopened = DatabaseProfiles::new(Arc::new(repository.reopen()), vault.clone(), closer);
    let pending = reopened.load().unwrap();
    assert!(pending.profiles.is_empty());
    assert_eq!(
        pending.recovery[0].kind,
        PendingOperationKind::PendingCreate
    );
    let completed = reopened
        .recover(ProfileRecoveryRequest {
            operation_id: pending.recovery[0].operation_id.clone(),
            action: RecoveryAction::Resume,
            credential: None,
        })
        .unwrap();

    assert!(completed.recovery.is_empty());
    assert_eq!(completed.profiles.len(), 1);
    assert_eq!(
        completed.profiles[0].credential_state,
        CredentialState::Stored
    );
    assert_eq!(
        vault.counts().0,
        1,
        "resume must not rewrite an existing generation"
    );
    assert_eq!(vault.generation_count(&descriptor_id.0), 1);
    let durable_completed = repository.reopen().load().unwrap();
    assert!(durable_completed.pending_operations.is_empty());
    assert_eq!(
        durable_completed.profiles[0].active_credential_generation,
        Some(generation)
    );
}

#[test]
fn pending_create_abort_delete_failure_reopens_and_retries_idempotently() {
    let (profiles, repository, vault, closer) = harness();
    repository.fail_nth_replace(2, RepositoryFailurePoint::Rename);
    assert_eq!(
        profiles
            .create(postgres_request("abort-create-secret"))
            .unwrap_err()
            .code,
        ProfileErrorCode::RepositoryUnavailable
    );
    let durable_pending = repository.reopen().load().unwrap();
    let (operation_id, descriptor_id, generation) =
        match durable_pending.pending_operations.as_slice() {
            [PendingOperation::PendingCreate {
                operation_id,
                profile,
                credential_generation,
            }] => (
                operation_id.clone(),
                profile.descriptor_id.clone(),
                credential_generation.clone(),
            ),
            operations => panic!("expected one pendingCreate operation, got {operations:?}"),
        };
    assert!(durable_pending.profiles.is_empty());
    assert_eq!(vault.generation_count(&descriptor_id.0), 1);

    vault.fail_next(TestVaultOperation::Delete, VaultErrorKind::DeleteFailed);
    let reopened =
        DatabaseProfiles::new(Arc::new(repository.reopen()), vault.clone(), closer.clone());
    assert_eq!(
        reopened
            .recover(ProfileRecoveryRequest {
                operation_id: operation_id.clone(),
                action: RecoveryAction::Abort,
                credential: None,
            })
            .unwrap_err()
            .code,
        ProfileErrorCode::VaultDeleteFailed
    );
    let after_delete_failure = repository.reopen().load().unwrap();
    assert!(after_delete_failure.profiles.is_empty());
    assert_eq!(after_delete_failure.pending_operations.len(), 1);
    assert_eq!(vault.generation_count(&descriptor_id.0), 1);

    let retried = DatabaseProfiles::new(Arc::new(repository.reopen()), vault.clone(), closer);
    let completed = retried
        .recover(ProfileRecoveryRequest {
            operation_id: operation_id.clone(),
            action: RecoveryAction::Abort,
            credential: None,
        })
        .unwrap();
    assert!(completed.profiles.is_empty());
    assert!(completed.recovery.is_empty());
    assert_eq!(vault.generation_count(&descriptor_id.0), 0);
    let durable_completed = repository.reopen().load().unwrap();
    assert!(durable_completed.profiles.is_empty());
    assert!(durable_completed.pending_operations.is_empty());
    assert_eq!(
        retried
            .recover(ProfileRecoveryRequest {
                operation_id,
                action: RecoveryAction::Abort,
                credential: None,
            })
            .unwrap_err()
            .code,
        ProfileErrorCode::RecoveryNotFound
    );
    assert_eq!(
        vault
            .resolve(&descriptor_id, &generation)
            .unwrap_err()
            .kind(),
        VaultErrorKind::Missing
    );
}

#[test]
fn replace_switch_failure_reopens_old_active_and_resume_keeps_only_new_generation() {
    let (profiles, repository, vault, closer) = harness();
    let created = profiles.create(postgres_request("old-secret")).unwrap();
    let old_generation = repository.reopen().load().unwrap().profiles[0]
        .active_credential_generation
        .clone()
        .unwrap();
    repository.fail_nth_replace(2, RepositoryFailurePoint::Rename);

    let error = profiles
        .update(ProfileUpdateRequest {
            descriptor_id: created.descriptor_id.clone(),
            name: "Production v2".to_string(),
            target: ProfileTarget::postgres(
                "db-v2.internal",
                5432,
                "app",
                "alice",
                PostgresTransportMode::VerifyFull,
            ),
            replacement_credential: Some(CredentialInput {
                password: SecretString::from("new-secret"),
            }),
            transport_challenge_id: None,
        })
        .unwrap_err();
    assert_eq!(error.code, ProfileErrorCode::RepositoryUnavailable);

    let durable_pending = repository.reopen().load().unwrap();
    assert_eq!(durable_pending.profiles[0].name, "Production");
    assert_eq!(
        durable_pending.profiles[0].active_credential_generation,
        Some(old_generation.clone())
    );
    let new_generation = match durable_pending.pending_operations.as_slice() {
        [PendingOperation::PendingReplace {
            descriptor_id,
            replacement,
            old_generation: pending_old,
            new_generation,
            ..
        }] => {
            assert_eq!(descriptor_id, &created.descriptor_id);
            assert_eq!(replacement.descriptor_id, created.descriptor_id);
            assert_eq!(replacement.name, "Production v2");
            assert_eq!(pending_old.as_ref(), Some(&old_generation));
            assert_eq!(
                replacement.active_credential_generation.as_ref(),
                Some(new_generation)
            );
            new_generation.clone()
        }
        operations => panic!("expected one pendingReplace operation, got {operations:?}"),
    };
    assert_eq!(vault.counts().0, 2);
    assert_eq!(vault.generation_count(&created.descriptor_id.0), 2);

    let reopened = DatabaseProfiles::new(Arc::new(repository.reopen()), vault.clone(), closer);
    let pending = reopened.load().unwrap();
    assert_eq!(pending.profiles[0].name, "Production");
    assert_eq!(
        pending.recovery[0].kind,
        PendingOperationKind::PendingReplace
    );
    let completed = reopened
        .recover(ProfileRecoveryRequest {
            operation_id: pending.recovery[0].operation_id.clone(),
            action: RecoveryAction::Resume,
            credential: None,
        })
        .unwrap();

    assert_eq!(completed.profiles[0].name, "Production v2");
    assert!(completed.recovery.is_empty());
    assert_eq!(
        vault.counts().0,
        2,
        "resume must reuse the stored new generation"
    );
    assert_eq!(vault.generation_count(&created.descriptor_id.0), 1);
    assert!(vault
        .resolve(&created.descriptor_id, &new_generation)
        .is_ok());
    assert_eq!(
        vault
            .resolve(&created.descriptor_id, &old_generation)
            .unwrap_err()
            .kind(),
        VaultErrorKind::Missing
    );
    let durable_completed = repository.reopen().load().unwrap();
    assert!(durable_completed.pending_operations.is_empty());
    assert_eq!(
        durable_completed.profiles[0].active_credential_generation,
        Some(new_generation)
    );
}

#[test]
fn pending_replace_abort_finalization_failure_reopens_and_preserves_old_active() {
    let (profiles, repository, vault, closer) = harness();
    let created = profiles.create(postgres_request("old-secret")).unwrap();
    let old_generation = repository.reopen().load().unwrap().profiles[0]
        .active_credential_generation
        .clone()
        .unwrap();
    repository.fail_nth_replace(2, RepositoryFailurePoint::Rename);
    assert_eq!(
        profiles
            .update(ProfileUpdateRequest {
                descriptor_id: created.descriptor_id.clone(),
                name: "Production replacement".to_string(),
                target: ProfileTarget::postgres(
                    "replacement.internal",
                    5432,
                    "app",
                    "alice",
                    PostgresTransportMode::VerifyFull,
                ),
                replacement_credential: Some(CredentialInput {
                    password: SecretString::from("new-secret"),
                }),
                transport_challenge_id: None,
            })
            .unwrap_err()
            .code,
        ProfileErrorCode::RepositoryUnavailable
    );
    let durable_pending = repository.reopen().load().unwrap();
    let (operation_id, new_generation) = match durable_pending.pending_operations.as_slice() {
        [PendingOperation::PendingReplace {
            operation_id,
            descriptor_id,
            old_generation: pending_old,
            new_generation,
            ..
        }] => {
            assert_eq!(descriptor_id, &created.descriptor_id);
            assert_eq!(pending_old.as_ref(), Some(&old_generation));
            (operation_id.clone(), new_generation.clone())
        }
        operations => panic!("expected one pendingReplace operation, got {operations:?}"),
    };
    assert_eq!(
        durable_pending.profiles[0].active_credential_generation,
        Some(old_generation.clone())
    );
    assert_eq!(vault.generation_count(&created.descriptor_id.0), 2);

    repository.fail_next(RepositoryFailurePoint::Rename);
    let reopened =
        DatabaseProfiles::new(Arc::new(repository.reopen()), vault.clone(), closer.clone());
    assert_eq!(
        reopened
            .recover(ProfileRecoveryRequest {
                operation_id: operation_id.clone(),
                action: RecoveryAction::Abort,
                credential: None,
            })
            .unwrap_err()
            .code,
        ProfileErrorCode::RepositoryUnavailable
    );
    let after_finalize_failure = repository.reopen().load().unwrap();
    assert_eq!(after_finalize_failure.pending_operations.len(), 1);
    assert_eq!(
        after_finalize_failure.profiles[0].active_credential_generation,
        Some(old_generation.clone())
    );
    assert_eq!(vault.generation_count(&created.descriptor_id.0), 1);
    assert!(vault
        .resolve(&created.descriptor_id, &old_generation)
        .is_ok());
    assert_eq!(
        vault
            .resolve(&created.descriptor_id, &new_generation)
            .unwrap_err()
            .kind(),
        VaultErrorKind::Missing
    );

    let retried = DatabaseProfiles::new(Arc::new(repository.reopen()), vault.clone(), closer);
    let completed = retried
        .recover(ProfileRecoveryRequest {
            operation_id: operation_id.clone(),
            action: RecoveryAction::Abort,
            credential: None,
        })
        .unwrap();
    assert!(completed.recovery.is_empty());
    assert_eq!(completed.profiles.len(), 1);
    assert_eq!(
        completed.profiles[0].credential_state,
        CredentialState::Stored
    );
    assert_eq!(vault.generation_count(&created.descriptor_id.0), 1);
    let durable_completed = repository.reopen().load().unwrap();
    assert!(durable_completed.pending_operations.is_empty());
    assert_eq!(
        durable_completed.profiles[0].active_credential_generation,
        Some(old_generation)
    );
    assert_eq!(
        retried
            .recover(ProfileRecoveryRequest {
                operation_id,
                action: RecoveryAction::Abort,
                credential: None,
            })
            .unwrap_err()
            .code,
        ProfileErrorCode::RecoveryNotFound
    );
}

#[test]
fn cleanup_clear_failure_reopens_cleanup_and_retry_keeps_only_active_generation() {
    let (profiles, repository, vault, closer) = harness();
    let created = profiles.create(postgres_request("old-secret")).unwrap();
    let old_generation = repository.reopen().load().unwrap().profiles[0]
        .active_credential_generation
        .clone()
        .unwrap();
    repository.fail_nth_replace(3, RepositoryFailurePoint::Rename);

    let error = profiles
        .update(ProfileUpdateRequest {
            descriptor_id: created.descriptor_id.clone(),
            name: "Production v2".to_string(),
            target: ProfileTarget::postgres(
                "db-v2.internal",
                5432,
                "app",
                "alice",
                PostgresTransportMode::VerifyFull,
            ),
            replacement_credential: Some(CredentialInput {
                password: SecretString::from("new-secret"),
            }),
            transport_challenge_id: None,
        })
        .unwrap_err();
    assert_eq!(error.code, ProfileErrorCode::RepositoryUnavailable);

    let durable_pending = repository.reopen().load().unwrap();
    assert_eq!(durable_pending.profiles[0].name, "Production v2");
    let active_generation = match durable_pending.pending_operations.as_slice() {
        [PendingOperation::CleanupOld {
            descriptor_id,
            old_generation: pending_old,
            active_generation,
            ..
        }] => {
            assert_eq!(descriptor_id, &created.descriptor_id);
            assert_eq!(pending_old, &old_generation);
            active_generation.clone()
        }
        operations => panic!("expected one cleanupOld operation, got {operations:?}"),
    };
    assert_eq!(
        durable_pending.profiles[0]
            .active_credential_generation
            .as_ref(),
        Some(&active_generation)
    );
    assert_eq!(vault.generation_count(&created.descriptor_id.0), 1);
    assert_eq!(
        vault
            .resolve(&created.descriptor_id, &old_generation)
            .unwrap_err()
            .kind(),
        VaultErrorKind::Missing
    );

    let deletes_before_retry = vault.counts().2;
    let reopened = DatabaseProfiles::new(Arc::new(repository.reopen()), vault.clone(), closer);
    let pending = reopened.load().unwrap();
    assert_eq!(pending.recovery[0].kind, PendingOperationKind::CleanupOld);
    let completed = reopened
        .recover(ProfileRecoveryRequest {
            operation_id: pending.recovery[0].operation_id.clone(),
            action: RecoveryAction::RetryCleanup,
            credential: None,
        })
        .unwrap();

    assert!(completed.recovery.is_empty());
    assert_eq!(completed.profiles[0].name, "Production v2");
    assert_eq!(vault.counts().2, deletes_before_retry + 1);
    assert_eq!(vault.generation_count(&created.descriptor_id.0), 1);
    assert!(vault
        .resolve(&created.descriptor_id, &active_generation)
        .is_ok());
    let durable_completed = repository.reopen().load().unwrap();
    assert!(durable_completed.pending_operations.is_empty());
    assert_eq!(
        durable_completed.profiles[0].active_credential_generation,
        Some(active_generation)
    );
}

#[test]
fn replace_switches_active_generation_before_delete_and_retries_cleanup_after_reopen() {
    let (profiles, repository, vault, closer) = harness();
    let created = profiles.create(postgres_request("old-secret")).unwrap();
    vault.fail_next(TestVaultOperation::Delete, VaultErrorKind::DeleteFailed);

    let error = profiles
        .update(ProfileUpdateRequest {
            descriptor_id: created.descriptor_id.clone(),
            name: "Production v2".to_string(),
            target: ProfileTarget::postgres(
                "db-v2.internal",
                5432,
                "app",
                "alice",
                PostgresTransportMode::VerifyFull,
            ),
            replacement_credential: Some(CredentialInput {
                password: SecretString::from("new-secret"),
            }),
            transport_challenge_id: None,
        })
        .unwrap_err();
    assert_eq!(error.code, ProfileErrorCode::VaultDeleteFailed);
    let reopened = DatabaseProfiles::new(Arc::new(repository.reopen()), vault.clone(), closer);
    let pending = reopened.load().unwrap();
    assert_eq!(pending.profiles[0].name, "Production v2");
    assert_eq!(pending.recovery[0].kind, PendingOperationKind::CleanupOld);
    assert_eq!(vault.generation_count(&created.descriptor_id.0), 2);

    let completed = reopened
        .recover(ProfileRecoveryRequest {
            operation_id: pending.recovery[0].operation_id.clone(),
            action: RecoveryAction::RetryCleanup,
            credential: None,
        })
        .unwrap();
    assert!(completed.recovery.is_empty());
    assert_eq!(vault.generation_count(&created.descriptor_id.0), 1);
}

#[test]
fn forget_final_replace_failure_reopens_pending_and_retry_removes_profile_and_ledger() {
    let (profiles, repository, vault, closer) = harness();
    let created = profiles.create(postgres_request("secret")).unwrap();
    let generation = repository.reopen().load().unwrap().profiles[0]
        .active_credential_generation
        .clone()
        .unwrap();
    repository.fail_nth_replace(2, RepositoryFailurePoint::Rename);

    assert_eq!(
        profiles.forget(&created.descriptor_id).unwrap_err().code,
        ProfileErrorCode::RepositoryUnavailable
    );

    let durable_pending = repository.reopen().load().unwrap();
    assert_eq!(durable_pending.profiles.len(), 1);
    match durable_pending.pending_operations.as_slice() {
        [PendingOperation::PendingForget {
            descriptor_id,
            generations,
            ..
        }] => {
            assert_eq!(descriptor_id, &created.descriptor_id);
            assert_eq!(generations, std::slice::from_ref(&generation));
        }
        operations => panic!("expected one pendingForget operation, got {operations:?}"),
    }
    assert_eq!(vault.generation_count(&created.descriptor_id.0), 0);
    assert_eq!(closer.call_count(&created.descriptor_id.0), 1);

    let reopened =
        DatabaseProfiles::new(Arc::new(repository.reopen()), vault.clone(), closer.clone());
    let pending = reopened.load().unwrap();
    assert_eq!(pending.profiles.len(), 1);
    assert_eq!(
        pending.recovery[0].kind,
        PendingOperationKind::PendingForget
    );
    let completed = reopened
        .recover(ProfileRecoveryRequest {
            operation_id: pending.recovery[0].operation_id.clone(),
            action: RecoveryAction::RetryCleanup,
            credential: None,
        })
        .unwrap();

    assert!(completed.profiles.is_empty());
    assert!(completed.recovery.is_empty());
    assert_eq!(closer.call_count(&created.descriptor_id.0), 2);
    assert_eq!(vault.generation_count(&created.descriptor_id.0), 0);
    let durable_completed = repository.reopen().load().unwrap();
    assert!(durable_completed.profiles.is_empty());
    assert!(durable_completed.pending_operations.is_empty());
}

#[test]
fn forget_failure_retains_profile_and_retry_row_until_delete_succeeds() {
    let (profiles, repository, vault, closer) = harness();
    let created = profiles.create(postgres_request("secret")).unwrap();
    vault.fail_next(TestVaultOperation::Delete, VaultErrorKind::DeleteFailed);

    assert_eq!(
        profiles.forget(&created.descriptor_id).unwrap_err().code,
        ProfileErrorCode::VaultDeleteFailed
    );
    let reopened = DatabaseProfiles::new(Arc::new(repository.reopen()), vault.clone(), closer);
    let pending = reopened.load().unwrap();
    assert_eq!(pending.profiles.len(), 1);
    assert_eq!(
        pending.recovery[0].kind,
        PendingOperationKind::PendingForget
    );
    let completed = reopened
        .recover(ProfileRecoveryRequest {
            operation_id: pending.recovery[0].operation_id.clone(),
            action: RecoveryAction::RetryCleanup,
            credential: None,
        })
        .unwrap();
    assert!(completed.profiles.is_empty());
    assert!(completed.recovery.is_empty());
}

#[test]
fn remove_credential_final_replace_failure_reopens_pending_and_retry_keeps_required_profile() {
    let (profiles, repository, vault, closer) = harness();
    let created = profiles.create(postgres_request("secret")).unwrap();
    let generation = repository.reopen().load().unwrap().profiles[0]
        .active_credential_generation
        .clone()
        .unwrap();
    repository.fail_nth_replace(2, RepositoryFailurePoint::Rename);

    assert_eq!(
        profiles
            .remove_credential(&created.descriptor_id)
            .unwrap_err()
            .code,
        ProfileErrorCode::RepositoryUnavailable
    );

    let durable_pending = repository.reopen().load().unwrap();
    assert_eq!(durable_pending.profiles.len(), 1);
    assert_eq!(
        durable_pending.profiles[0].active_credential_generation,
        Some(generation.clone())
    );
    match durable_pending.pending_operations.as_slice() {
        [PendingOperation::PendingRemoveCredential {
            descriptor_id,
            generations,
            ..
        }] => {
            assert_eq!(descriptor_id, &created.descriptor_id);
            assert_eq!(generations, &[generation]);
        }
        operations => {
            panic!("expected one pendingRemoveCredential operation, got {operations:?}")
        }
    }
    assert_eq!(vault.generation_count(&created.descriptor_id.0), 0);
    assert_eq!(closer.call_count(&created.descriptor_id.0), 1);

    let reopened =
        DatabaseProfiles::new(Arc::new(repository.reopen()), vault.clone(), closer.clone());
    let pending = reopened.load().unwrap();
    assert_eq!(
        pending.recovery[0].kind,
        PendingOperationKind::PendingRemoveCredential
    );
    let completed = reopened
        .recover(ProfileRecoveryRequest {
            operation_id: pending.recovery[0].operation_id.clone(),
            action: RecoveryAction::RetryCleanup,
            credential: None,
        })
        .unwrap();

    assert_eq!(completed.profiles.len(), 1);
    assert_eq!(
        completed.profiles[0].credential_state,
        CredentialState::Required
    );
    assert!(completed.recovery.is_empty());
    assert_eq!(closer.call_count(&created.descriptor_id.0), 2);
    assert_eq!(vault.generation_count(&created.descriptor_id.0), 0);
    let durable_completed = repository.reopen().load().unwrap();
    assert!(durable_completed.pending_operations.is_empty());
    assert_eq!(
        durable_completed.profiles[0].credential_state,
        CredentialState::Required
    );
    assert_eq!(
        durable_completed.profiles[0].active_credential_generation,
        None
    );
}

#[test]
fn remove_credential_failure_retains_profile_then_marks_it_required_on_retry() {
    let (profiles, repository, vault, closer) = harness();
    let created = profiles.create(postgres_request("secret")).unwrap();
    vault.fail_next(TestVaultOperation::Delete, VaultErrorKind::DeleteFailed);

    assert_eq!(
        profiles
            .remove_credential(&created.descriptor_id)
            .unwrap_err()
            .code,
        ProfileErrorCode::VaultDeleteFailed
    );
    let reopened = DatabaseProfiles::new(Arc::new(repository.reopen()), vault, closer);
    let pending = reopened.load().unwrap();
    assert_eq!(
        pending.recovery[0].kind,
        PendingOperationKind::PendingRemoveCredential
    );
    let completed = reopened
        .recover(ProfileRecoveryRequest {
            operation_id: pending.recovery[0].operation_id.clone(),
            action: RecoveryAction::RetryCleanup,
            credential: None,
        })
        .unwrap();
    assert_eq!(
        completed.profiles[0].credential_state,
        CredentialState::Required
    );
    assert!(completed.recovery.is_empty());
}

#[test]
fn lifecycle_close_failure_stops_before_vault_delete_and_persists_retry_row() {
    let (profiles, repository, vault, closer) = harness();
    let created = profiles.create(postgres_request("secret")).unwrap();
    closer.fail_next(LifecycleCloseErrorKind::CancelFailed);
    let before = vault.counts();

    assert_eq!(
        profiles.forget(&created.descriptor_id).unwrap_err().code,
        ProfileErrorCode::LifecycleCancelFailed
    );
    assert_eq!(vault.counts().2, before.2);
    let pending = DatabaseProfiles::new(Arc::new(repository.reopen()), vault, closer)
        .load()
        .unwrap();
    assert_eq!(
        pending.recovery[0].kind,
        PendingOperationKind::PendingForget
    );
}

#[test]
fn production_closer_requests_lifecycle_termination_before_recovery_retry() {
    let repository = Arc::new(FakeProfileRepository::default());
    let vault = Arc::new(TestVault::default());
    let database_state = DbState::default();
    let runtime = ProfileRuntimeRegistry::default();
    let closer = Arc::new(RegisteredProfileCloser {
        database_state: database_state.clone(),
        runtime: runtime.clone(),
        result_sessions: ResultSessionState::default(),
    });
    let profiles = DatabaseProfiles::new(repository.clone(), vault.clone(), closer.clone());
    let created = profiles.create(postgres_request("secret")).unwrap();
    let connection = LiveConnection {
        descriptor_id: created.descriptor_id.clone(),
        connection_id: ConnectionId("connection-1".to_string()),
        connection_generation: ConnectionGeneration("generation-1".to_string()),
        engine: LiveDatabaseEngine::Sqlite,
    };
    runtime.insert(connection.clone()).unwrap();
    let identity = db_service::ConnectionIdentity {
        descriptor_id: connection.descriptor_id.clone(),
        connection_id: connection.connection_id.clone(),
        connection_generation: connection.connection_generation.clone(),
    };
    let actor = Arc::new(crate::db_connection_actor::ProductionConnectionActor::new(
        identity.clone(),
        DbHandle::Sqlite(Mutex::new(rusqlite::Connection::open_in_memory().unwrap())),
    ));
    db_service::register_actor(&database_state, actor.clone()).unwrap();
    let execution = actor
        .acquire_execution(
            db_service::QueryRunOwner {
                descriptor_id: identity.descriptor_id.clone(),
                connection_id: identity.connection_id.clone(),
                connection_generation: identity.connection_generation.clone(),
                query_run_id: db_service::QueryRunId("query-busy".to_string()),
            },
            crate::db_connection_actor::CancelCapability::SqliteInterrupt,
        )
        .unwrap();
    let deletes_before = vault.counts().2;

    assert_eq!(
        profiles.forget(&created.descriptor_id).unwrap_err().code,
        ProfileErrorCode::LifecycleCloseFailed
    );
    assert_eq!(vault.counts().2, deletes_before);
    assert!(runtime.get(&created.descriptor_id).is_some());
    assert_eq!(database_state.0.lock().unwrap().len(), 1);
    assert!(actor.is_terminating());
    assert_eq!(
        actor.acquire_metadata(),
        Err(crate::db_connection_actor::ActorError::Closed)
    );

    let state = DatabaseProfileState {
        profiles: Arc::new(Mutex::new(profiles)),
        runtime: runtime.clone(),
        database_state: database_state.clone(),
        result_sessions: ResultSessionState::default(),
        opener: Arc::new(ProductionDatabaseConnectionOpener),
    };
    let open_error =
        tauri::async_runtime::block_on(state.open_saved(&created.descriptor_id)).unwrap_err();
    assert_eq!(open_error.code, ProfileErrorCode::PendingOperationConflict);

    assert_eq!(
        actor.settle_execution(&execution).unwrap(),
        crate::db_connection_actor::Settlement {
            cancel_requested: true,
            release_requested: true,
            connection_termination_required: false,
        }
    );
    let pending = state.profiles.lock().unwrap().load().unwrap();
    let completed = state
        .profiles
        .lock()
        .unwrap()
        .recover(ProfileRecoveryRequest {
            operation_id: pending.recovery[0].operation_id.clone(),
            action: RecoveryAction::RetryCleanup,
            credential: None,
        })
        .unwrap();
    assert!(completed.profiles.is_empty());
    assert!(runtime.get(&created.descriptor_id).is_none());
    assert_eq!(vault.counts().2, deletes_before + 1);
}

#[test]
fn actor_missing_close_removes_only_the_stale_runtime_identity() {
    let database_state = DbState::default();
    let runtime = ProfileRuntimeRegistry::default();
    let closer = RegisteredProfileCloser {
        database_state: database_state.clone(),
        runtime: runtime.clone(),
        result_sessions: ResultSessionState::default(),
    };
    let stale = runtime_connection("descriptor-a", "stale");
    runtime.insert(stale.clone()).unwrap();

    assert_eq!(
        closer.cancel_and_close(&stale.descriptor_id).unwrap(),
        LifecycleCloseEvidence::HandleClosedAndSettled
    );
    assert!(runtime.get(&stale.descriptor_id).is_none());

    let old_runtime = LiveConnection {
        descriptor_id: DescriptorId("descriptor-a".to_string()),
        connection_id: ConnectionId("connection-reused".to_string()),
        connection_generation: ConnectionGeneration("generation-1".to_string()),
        engine: LiveDatabaseEngine::Sqlite,
    };
    let new_identity = db_service::ConnectionIdentity {
        descriptor_id: old_runtime.descriptor_id.clone(),
        connection_id: old_runtime.connection_id.clone(),
        connection_generation: ConnectionGeneration("generation-2".to_string()),
    };
    let generation_two_actor =
        Arc::new(crate::db_connection_actor::ProductionConnectionActor::new(
            new_identity.clone(),
            DbHandle::Sqlite(Mutex::new(rusqlite::Connection::open_in_memory().unwrap())),
        ));
    db_service::register_actor(&database_state, generation_two_actor.clone()).unwrap();
    runtime.insert(old_runtime.clone()).unwrap();

    assert_eq!(
        closer.cancel_and_close(&old_runtime.descriptor_id).unwrap(),
        LifecycleCloseEvidence::HandleClosedAndSettled
    );
    assert!(runtime.get(&old_runtime.descriptor_id).is_none());
    assert_eq!(database_state.0.lock().unwrap().len(), 1);
    assert_eq!(generation_two_actor.identity(), &new_identity);
    assert!(!generation_two_actor.teardown_report().closed);
}

#[test]
fn explicit_resume_handles_corrupt_or_denied_generations_without_startup_access() {
    for failure in [VaultErrorKind::Corrupt, VaultErrorKind::Denied] {
        let (profiles, _, vault, _) = harness();
        vault.fail_next(TestVaultOperation::Store, VaultErrorKind::WriteFailed);
        assert!(profiles.create(postgres_request("first")).is_err());
        let pending = profiles.load().unwrap();
        assert_eq!(vault.counts().1, 0, "startup load must not resolve vault");
        vault.fail_next(TestVaultOperation::Resolve, failure);

        let completed = profiles
            .recover(ProfileRecoveryRequest {
                operation_id: pending.recovery[0].operation_id.clone(),
                action: RecoveryAction::Resume,
                credential: Some(CredentialInput {
                    password: SecretString::from("replacement"),
                }),
            })
            .unwrap();
        assert!(completed.recovery.is_empty());
        assert_eq!(
            completed.profiles[0].credential_state,
            CredentialState::Stored
        );
    }
}

#[test]
fn legacy_import_is_one_atomic_non_secret_merge_and_never_claims_a_vault_secret() {
    let (profiles, repository, vault, _) = harness();
    let imported = profiles
        .import_legacy(LegacyProfileImportRequest {
            profiles: vec![ProfileDescriptor {
                descriptor_id: DescriptorId("legacy-1".to_string()),
                config_generation: 1,
                name: "Legacy".to_string(),
                target: postgres_request("unused").target,
                credential_state: CredentialState::Stored,
            }],
        })
        .unwrap();
    assert_eq!(
        imported.profiles[0].credential_state,
        CredentialState::Required
    );
    assert_eq!(vault.counts(), (0, 0, 0));
    let persisted = String::from_utf8(repository.durable_bytes()).unwrap();
    assert!(persisted.contains("legacy-1"));
    assert!(!persisted.contains(SENTINEL));
}

#[test]
fn legacy_import_strips_renderer_postgres_attestation_and_requires_fresh_challenge() {
    let (profiles, _, _, _) = harness();
    let target = ProfileTarget::postgres(
        "db.example",
        5432,
        "app",
        "alice",
        PostgresTransportMode::InsecurePlaintext,
    )
    .with_insecure_exception(PostgresInsecureException::new(
        "db.example",
        5432,
        "alice",
        "app",
    ));
    let imported = profiles
        .import_legacy(LegacyProfileImportRequest {
            profiles: vec![ProfileDescriptor {
                descriptor_id: DescriptorId("legacy-attested".to_string()),
                config_generation: 1,
                name: "Legacy attested".to_string(),
                target,
                credential_state: CredentialState::Stored,
            }],
        })
        .unwrap();
    match &imported.profiles[0].target {
        ProfileTarget::Postgres {
            transport_mode,
            insecure_exception,
            trust_server_cert_acknowledged,
            ..
        } => {
            assert_eq!(*transport_mode, PostgresTransportMode::InsecurePlaintext);
            assert!(insecure_exception.is_none());
            assert!(!*trust_server_cert_acknowledged);
        }
        other => panic!("unexpected {other:?}"),
    }
    assert_eq!(
        profiles
            .update(ProfileUpdateRequest {
                descriptor_id: DescriptorId("legacy-attested".to_string()),
                name: "Legacy attested".to_string(),
                target: ProfileTarget::postgres(
                    "db.example",
                    5432,
                    "app",
                    "alice",
                    PostgresTransportMode::InsecurePlaintext,
                ),
                replacement_credential: None,
                transport_challenge_id: None,
            })
            .unwrap_err()
            .code,
        ProfileErrorCode::PostgresTransportRejected
    );
}

#[test]
fn errors_debug_and_repository_bytes_never_contain_secret_sentinel() {
    let (profiles, repository, vault, _) = harness();
    vault.fail_next(TestVaultOperation::Store, VaultErrorKind::WriteFailed);
    let error = profiles.create(postgres_request(SENTINEL)).unwrap_err();
    assert!(!format!("{error:?}").contains(SENTINEL));
    assert!(!serde_json::to_string(&error).unwrap().contains(SENTINEL));
    assert!(!String::from_utf8_lossy(&repository.durable_bytes()).contains(SENTINEL));

    // The sentinel exists only inside the secret type when a write succeeds;
    // neither its Debug representation nor repository state exposes it.
    let secret = SecretString::from(SENTINEL);
    assert_eq!(secret.expose_secret(), SENTINEL);
    assert!(!format!("{secret:?}").contains(SENTINEL));
}

#[test]
fn new_postgres_profile_serializes_verify_full_by_default() {
    let (profiles, repository, vault, _) = harness();
    let created = profiles.create(postgres_request(SENTINEL)).unwrap();
    match created.target {
        ProfileTarget::Postgres {
            transport_mode,
            insecure_exception,
            trust_server_cert_acknowledged,
            ..
        } => {
            assert_eq!(transport_mode, PostgresTransportMode::VerifyFull);
            assert!(insecure_exception.is_none());
            assert!(!trust_server_cert_acknowledged);
        }
        _ => panic!("expected postgres target"),
    }
    let persisted =
        serde_json::from_slice::<serde_json::Value>(&repository.durable_bytes()).unwrap();
    let target = &persisted["profiles"][0]["target"];
    assert_eq!(target["transportMode"], "verifyFull");
    assert!(target.get("ssl").is_none());
    assert!(target.get("trustCert").is_none());
    assert!(target.get("insecureException").is_none());
    assert!(!String::from_utf8_lossy(&repository.durable_bytes()).contains(SENTINEL));
    assert_eq!(vault.counts().0, 1);
}

#[test]
fn migrates_legacy_postgres_booleans_deterministically() {
    let mut document = serde_json::json!({
        "version": 1,
        "profiles": [
            {
                "descriptorId": "pg-verify",
                "configGeneration": 1,
                "name": "Verify",
                "target": {
                    "kind": "postgres",
                    "host": "db.example",
                    "port": 5432,
                    "database": "app",
                    "user": "alice",
                    "ssl": true,
                    "trustCert": false
                },
                "credentialState": "required"
            },
            {
                "descriptorId": "pg-trust",
                "configGeneration": 1,
                "name": "Trust",
                "target": {
                    "kind": "postgres",
                    "host": "db.example",
                    "port": 5432,
                    "database": "app",
                    "user": "bob",
                    "ssl": true,
                    "trustCert": true
                },
                "credentialState": "required"
            },
            {
                "descriptorId": "pg-plain",
                "configGeneration": 1,
                "name": "Plain",
                "target": {
                    "kind": "postgres",
                    "host": "db.example",
                    "port": 5432,
                    "database": "app",
                    "user": "carol",
                    "ssl": false,
                    "trustCert": false
                },
                "credentialState": "required"
            },
            {
                "descriptorId": "pg-missing",
                "configGeneration": 1,
                "name": "Missing",
                "target": {
                    "kind": "postgres",
                    "host": "db.example",
                    "port": 5432,
                    "database": "app",
                    "user": "dave"
                },
                "credentialState": "required"
            },
            {
                "descriptorId": "mssql-1",
                "configGeneration": 1,
                "name": "Mssql",
                "target": {
                    "kind": "mssql",
                    "host": "sql.example",
                    "port": 1433,
                    "database": "app",
                    "user": "sa",
                    "trustCert": true
                },
                "credentialState": "required"
            },
            {
                "descriptorId": "sqlite-1",
                "configGeneration": 1,
                "name": "Local",
                "target": { "kind": "sqlite", "path": "/tmp/local.sqlite" },
                "credentialState": "notRequired"
            }
        ],
        "pendingOperations": []
    });
    migrate_legacy_postgres_targets(&mut document);
    let parsed: ProfileDocument = serde_json::from_value(document).unwrap();
    parsed.validate().unwrap();

    let by_id = |id: &str| {
        parsed
            .profiles
            .iter()
            .find(|profile| profile.descriptor_id.0 == id)
            .unwrap()
    };
    match &by_id("pg-verify").target {
        ProfileTarget::Postgres {
            transport_mode,
            insecure_exception,
            trust_server_cert_acknowledged,
            ..
        } => {
            assert_eq!(*transport_mode, PostgresTransportMode::VerifyFull);
            assert!(insecure_exception.is_none());
            assert!(!*trust_server_cert_acknowledged);
        }
        other => panic!("unexpected {other:?}"),
    }
    match &by_id("pg-trust").target {
        ProfileTarget::Postgres {
            transport_mode,
            insecure_exception,
            trust_server_cert_acknowledged,
            ..
        } => {
            assert_eq!(
                *transport_mode,
                PostgresTransportMode::EncryptedTrustServerCert
            );
            assert!(insecure_exception.is_none());
            assert!(*trust_server_cert_acknowledged);
        }
        other => panic!("unexpected {other:?}"),
    }
    match &by_id("pg-plain").target {
        ProfileTarget::Postgres {
            transport_mode,
            insecure_exception,
            trust_server_cert_acknowledged,
            ..
        } => {
            assert_eq!(*transport_mode, PostgresTransportMode::InsecurePlaintext);
            assert!(insecure_exception.is_none());
            assert!(!*trust_server_cert_acknowledged);
        }
        other => panic!("unexpected {other:?}"),
    }
    match &by_id("pg-missing").target {
        ProfileTarget::Postgres { transport_mode, .. } => {
            assert_eq!(*transport_mode, PostgresTransportMode::VerifyFull);
        }
        other => panic!("unexpected {other:?}"),
    }
    match &by_id("mssql-1").target {
        ProfileTarget::Mssql { trust_cert, .. } => assert!(*trust_cert),
        other => panic!("unexpected {other:?}"),
    }
    assert!(matches!(
        by_id("sqlite-1").target,
        ProfileTarget::Sqlite { .. }
    ));
}

#[test]
fn migrated_plaintext_cannot_save_test_or_connect_until_acknowledged() {
    let repository = Arc::new(FakeProfileRepository::default());
    let document = serde_json::json!({
        "version": 1,
        "profiles": [{
            "descriptorId": "pg-plain",
            "configGeneration": 1,
            "name": "Plain",
            "target": {
                "kind": "postgres",
                "host": "db.example",
                "port": 5432,
                "database": "app",
                "user": "alice",
                "ssl": false,
                "trustCert": false
            },
            "credentialState": "required"
        }],
        "pendingOperations": []
    });
    repository.seed_durable_bytes(serde_json::to_vec(&document).unwrap());

    let vault = Arc::new(TestVault::default());
    let closer = Arc::new(FakeLifecycleCloser::default());
    let profiles = DatabaseProfiles::new(repository.clone(), vault, closer);
    let loaded = profiles.load().unwrap();
    match &loaded.profiles[0].target {
        ProfileTarget::Postgres {
            transport_mode,
            insecure_exception,
            ..
        } => {
            assert_eq!(*transport_mode, PostgresTransportMode::InsecurePlaintext);
            assert!(insecure_exception.is_none());
        }
        other => panic!("unexpected {other:?}"),
    }

    let descriptor_id = DescriptorId("pg-plain".to_string());
    let unacked = ProfileTarget::postgres(
        "db.example",
        5432,
        "app",
        "alice",
        PostgresTransportMode::InsecurePlaintext,
    );
    assert_eq!(
        profiles
            .update(ProfileUpdateRequest {
                descriptor_id: descriptor_id.clone(),
                name: "Plain".to_string(),
                target: unacked.clone(),
                replacement_credential: None,
                transport_challenge_id: None,
            })
            .unwrap_err()
            .code,
        ProfileErrorCode::PostgresTransportRejected
    );
    assert_eq!(
        profiles
            .create(ProfileCreateRequest {
                name: "New plaintext".to_string(),
                target: unacked.clone(),
                credential: Some(CredentialInput {
                    password: SecretString::from(SENTINEL),
                }),
                transport_challenge_id: None,
            })
            .unwrap_err()
            .code,
        ProfileErrorCode::PostgresTransportRejected
    );

    let challenge_id = profiles
        .issue_transport_challenge(PostgresTransportChallengeRequest {
            via_host: None,
            transport_mode: PostgresTransportMode::InsecurePlaintext,
            host: "db.example".into(),
            port: 5432,
            user: "alice".into(),
            database: "app".into(),
        })
        .unwrap()
        .challenge_id;
    let updated = profiles
        .update(ProfileUpdateRequest {
            descriptor_id: descriptor_id.clone(),
            name: "Plain".to_string(),
            target: unacked.clone(),
            replacement_credential: None,
            transport_challenge_id: Some(challenge_id),
        })
        .unwrap();
    match updated.target {
        ProfileTarget::Postgres {
            insecure_exception,
            transport_mode,
            ..
        } => {
            assert_eq!(transport_mode, PostgresTransportMode::InsecurePlaintext);
            assert_eq!(
                insecure_exception,
                Some(PostgresInsecureException::new(
                    "db.example",
                    5432,
                    "alice",
                    "app",
                ))
            );
        }
        other => panic!("unexpected {other:?}"),
    }

    let revoked = ProfileTarget::postgres(
        "db.example",
        5432,
        "app",
        "alice",
        PostgresTransportMode::VerifyFull,
    );
    let after_revoke = profiles
        .update(ProfileUpdateRequest {
            descriptor_id,
            name: "Plain".to_string(),
            target: revoked,
            replacement_credential: None,
            transport_challenge_id: None,
        })
        .unwrap();
    match after_revoke.target {
        ProfileTarget::Postgres {
            transport_mode,
            insecure_exception,
            ..
        } => {
            assert_eq!(transport_mode, PostgresTransportMode::VerifyFull);
            assert!(insecure_exception.is_none());
        }
        other => panic!("unexpected {other:?}"),
    }
    let persisted = String::from_utf8(repository.durable_bytes()).unwrap();
    assert!(!persisted.contains(SENTINEL));
    assert!(persisted.contains("verifyFull"));
}

#[test]
fn save_and_test_reject_mismatched_plaintext_exception_and_unacked_trust_cert() {
    let (profiles, repository, vault, _) = harness();
    let mismatched = ProfileTarget::postgres(
        "db.example",
        5432,
        "app",
        "alice",
        PostgresTransportMode::InsecurePlaintext,
    )
    .with_insecure_exception(PostgresInsecureException::new(
        "other.example",
        5432,
        "alice",
        "app",
    ));
    assert_eq!(
        profiles
            .create(ProfileCreateRequest {
                name: "Mismatch".to_string(),
                target: mismatched,
                credential: Some(CredentialInput {
                    password: SecretString::from(SENTINEL),
                }),
                transport_challenge_id: None,
            })
            .unwrap_err()
            .code,
        ProfileErrorCode::PostgresTransportRejected
    );
    assert_eq!(
        profiles
            .create(ProfileCreateRequest {
                name: "Trust".to_string(),
                target: ProfileTarget::postgres(
                    "db.example",
                    5432,
                    "app",
                    "alice",
                    PostgresTransportMode::EncryptedTrustServerCert,
                ),
                credential: Some(CredentialInput {
                    password: SecretString::from(SENTINEL),
                }),
                transport_challenge_id: None,
            })
            .unwrap_err()
            .code,
        ProfileErrorCode::PostgresTransportRejected
    );
    let challenge_id = profiles
        .issue_transport_challenge(PostgresTransportChallengeRequest {
            via_host: None,
            transport_mode: PostgresTransportMode::EncryptedTrustServerCert,
            host: "db.example".into(),
            port: 5432,
            user: "alice".into(),
            database: "app".into(),
        })
        .unwrap()
        .challenge_id;
    let trusted = profiles
        .create(ProfileCreateRequest {
            name: "Trust".to_string(),
            target: ProfileTarget::postgres(
                "db.example",
                5432,
                "app",
                "alice",
                PostgresTransportMode::EncryptedTrustServerCert,
            ),
            credential: Some(CredentialInput {
                password: SecretString::from(SENTINEL),
            }),
            transport_challenge_id: Some(challenge_id),
        })
        .unwrap();
    match trusted.target {
        ProfileTarget::Postgres {
            transport_mode,
            trust_server_cert_acknowledged,
            ..
        } => {
            assert_eq!(
                transport_mode,
                PostgresTransportMode::EncryptedTrustServerCert
            );
            assert!(trust_server_cert_acknowledged);
        }
        other => panic!("unexpected {other:?}"),
    }
    assert!(!String::from_utf8_lossy(&repository.durable_bytes()).contains(SENTINEL));
    assert_eq!(vault.counts().0, 1);
}

fn plaintext_target() -> ProfileTarget {
    ProfileTarget::postgres(
        "db.example",
        5432,
        "app",
        "alice",
        PostgresTransportMode::InsecurePlaintext,
    )
}

fn issue_plain(profiles: &DatabaseProfiles) -> String {
    profiles
        .issue_transport_challenge(PostgresTransportChallengeRequest {
            via_host: None,
            transport_mode: PostgresTransportMode::InsecurePlaintext,
            host: "db.example".into(),
            port: 5432,
            user: "alice".into(),
            database: "app".into(),
        })
        .unwrap()
        .challenge_id
}

#[test]
fn postgres_transport_bypass_without_challenge_is_rejected() {
    let (profiles, _, _, _) = harness();
    let attested = plaintext_target().with_insecure_exception(PostgresInsecureException::new(
        "db.example",
        5432,
        "alice",
        "app",
    ));
    assert_eq!(
        profiles
            .create(ProfileCreateRequest {
                name: "Bypass".to_string(),
                target: attested,
                credential: Some(CredentialInput {
                    password: SecretString::from(SENTINEL),
                }),
                transport_challenge_id: None,
            })
            .unwrap_err()
            .code,
        ProfileErrorCode::PostgresTransportRejected
    );
}

#[test]
fn postgres_transport_stale_challenge_is_rejected() {
    let (profiles, _, _, _) = harness();
    let challenge_id = issue_plain(&profiles);
    profiles.expire_transport_challenges();
    assert_eq!(
        profiles
            .create(ProfileCreateRequest {
                name: "Stale".to_string(),
                target: plaintext_target(),
                credential: Some(CredentialInput {
                    password: SecretString::from(SENTINEL),
                }),
                transport_challenge_id: Some(challenge_id),
            })
            .unwrap_err()
            .code,
        ProfileErrorCode::PostgresTransportChallengeExpired
    );
}

#[test]
fn postgres_transport_mismatched_challenge_is_rejected() {
    let (profiles, _, _, _) = harness();
    let challenge_id = issue_plain(&profiles);
    let other = ProfileTarget::postgres(
        "other.example",
        5432,
        "app",
        "alice",
        PostgresTransportMode::InsecurePlaintext,
    );
    assert_eq!(
        profiles
            .create(ProfileCreateRequest {
                name: "Mismatch host".to_string(),
                target: other,
                credential: Some(CredentialInput {
                    password: SecretString::from(SENTINEL),
                }),
                transport_challenge_id: Some(challenge_id),
            })
            .unwrap_err()
            .code,
        ProfileErrorCode::PostgresTransportChallengeMismatch
    );
}

#[test]
fn postgres_transport_challenge_and_saved_attestation_do_not_cross_tunnel_hosts() {
    let (profiles, _, _, _) = harness();
    let challenge_id = issue_plain(&profiles);
    let mut tunneled = plaintext_target();
    if let ProfileTarget::Postgres { via_host, .. } = &mut tunneled {
        *via_host = Some("host-a".into());
    }
    assert_eq!(
        profiles
            .create(ProfileCreateRequest {
                name: "Tunneled".into(),
                target: tunneled.clone(),
                credential: Some(CredentialInput {
                    password: SecretString::from(SENTINEL)
                }),
                transport_challenge_id: Some(challenge_id),
            })
            .unwrap_err()
            .code,
        ProfileErrorCode::PostgresTransportChallengeMismatch
    );
    let mut other = tunneled.clone();
    if let ProfileTarget::Postgres { via_host, .. } = &mut other {
        *via_host = Some("host-b".into());
    }
    assert!(!same_postgres_identity(&tunneled, &other));
    let normalized = apply_backend_postgres_authorization(tunneled.clone());
    assert!(same_postgres_identity(&normalized, &tunneled));
    assert!(normalized.postgres_transport_authorized());
}

#[test]
fn postgres_transport_challenge_port_mismatch_is_rejected() {
    let (profiles, _, _, _) = harness();
    let challenge_id = issue_plain(&profiles);
    let other_port = ProfileTarget::postgres(
        "db.example",
        5433,
        "app",
        "alice",
        PostgresTransportMode::InsecurePlaintext,
    );
    assert_eq!(
        profiles
            .create(ProfileCreateRequest {
                name: "Mismatch port".to_string(),
                target: other_port,
                credential: Some(CredentialInput {
                    password: SecretString::from(SENTINEL),
                }),
                transport_challenge_id: Some(challenge_id),
            })
            .unwrap_err()
            .code,
        ProfileErrorCode::PostgresTransportChallengeMismatch
    );
}

#[test]
fn persisted_plaintext_authorization_is_bound_to_exact_port() {
    let (profiles, _, _, _) = harness();
    let created = profiles
        .create(ProfileCreateRequest {
            name: "Plain".to_string(),
            target: plaintext_target(),
            credential: Some(CredentialInput {
                password: SecretString::from(SENTINEL),
            }),
            transport_challenge_id: Some(issue_plain(&profiles)),
        })
        .unwrap();
    assert_eq!(
        profiles
            .update(ProfileUpdateRequest {
                descriptor_id: created.descriptor_id,
                name: "Plain new port".to_string(),
                target: ProfileTarget::postgres(
                    "db.example",
                    5433,
                    "app",
                    "alice",
                    PostgresTransportMode::InsecurePlaintext,
                ),
                replacement_credential: None,
                transport_challenge_id: None,
            })
            .unwrap_err()
            .code,
        ProfileErrorCode::PostgresTransportRejected
    );
}

#[test]
fn postgres_transport_challenge_cannot_be_replayed() {
    let (profiles, _, _, _) = harness();
    let challenge_id = issue_plain(&profiles);
    profiles
        .create(ProfileCreateRequest {
            name: "Once".to_string(),
            target: plaintext_target(),
            credential: Some(CredentialInput {
                password: SecretString::from(SENTINEL),
            }),
            transport_challenge_id: Some(challenge_id.clone()),
        })
        .unwrap();
    assert_eq!(
        profiles
            .create(ProfileCreateRequest {
                name: "Replay".to_string(),
                target: plaintext_target(),
                credential: Some(CredentialInput {
                    password: SecretString::from(SENTINEL),
                }),
                transport_challenge_id: Some(challenge_id),
            })
            .unwrap_err()
            .code,
        ProfileErrorCode::PostgresTransportChallengeReplay
    );
}

#[test]
fn persisted_backend_exception_can_be_reused_for_the_same_target() {
    let (profiles, _, _, _) = harness();
    let challenge_id = issue_plain(&profiles);
    let created = profiles
        .create(ProfileCreateRequest {
            name: "Saved".to_string(),
            target: plaintext_target(),
            credential: Some(CredentialInput {
                password: SecretString::from(SENTINEL),
            }),
            transport_challenge_id: Some(challenge_id),
        })
        .unwrap();
    let updated = profiles
        .update(ProfileUpdateRequest {
            descriptor_id: created.descriptor_id,
            name: "Saved again".to_string(),
            target: plaintext_target(),
            replacement_credential: None,
            transport_challenge_id: None,
        })
        .unwrap();
    match updated.target {
        ProfileTarget::Postgres {
            insecure_exception,
            transport_mode,
            ..
        } => {
            assert_eq!(transport_mode, PostgresTransportMode::InsecurePlaintext);
            assert_eq!(
                insecure_exception,
                Some(PostgresInsecureException::new(
                    "db.example",
                    5432,
                    "alice",
                    "app",
                ))
            );
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn profile_error_keeps_diagnostics_out_of_the_inline_result() {
    assert!(std::mem::size_of::<ProfileError>() <= 128);
}
