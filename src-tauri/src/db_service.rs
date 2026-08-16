// FEAT-1 Database + F2 network backends: connection registry + query commands.
//
// The registry maps a connId to an exact-identity production connection actor;
// that actor owns one `DbHandle`, one variant per engine:
// SQLite (synchronous rusqlite behind a std Mutex), PostgreSQL (tokio-postgres,
// natively async), and MSSQL (tiberius behind a tokio Mutex, `&mut self` API).
// Query commands are `async fn`: they clone the `Arc` out of the registry lock
// and then run the engine call *outside* the lock — the std Mutex is never held
// across an `.await` (SQLite work is offloaded to `spawn_blocking`). Every engine
// serialises into the same `columns + rows + kind` serde contract so the whole
// front-end (dbStore, DatabasePanel) is engine-agnostic.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::TryStreamExt;
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags};
use secrecy::{ExposeSecret, SecretString};
use tiberius::{AuthMethod, ColumnData, Config as MssqlConfig, FromSql, QueryItem};
use tokio::net::TcpStream;
use zeroize::Zeroize;

use tokio_postgres::types::Type as PgType;
use tokio_postgres::Row as PgRow;
use tokio_util::compat::{Compat, TokioAsyncWriteCompatExt};

use crate::db_connection_actor::{
    ActorError, CancelCapability, ExecutionLease, PostgresCancelResource,
    ProductionConnectionActor, ResultContinuationAck, ResultContinuationCommand,
    ResultContinuationOutcome, TeardownReport,
};
use crate::db_query_worker::{
    cancelled_error, read_request, worker_error, write_frame, NetworkQueryStart,
    NetworkQueryWorker, NetworkRow, WorkerRequest, WorkerResponse,
};
use crate::db_result_session::{
    NextPage, PushRowOutcome, RemainingBudget, ResultLimitKind, ResultSessionState, SessionError,
    DEFAULT_FIELD_BYTES, DEFAULT_ROW_BYTES, RESULT_PAGE_ROWS,
};

/// The MSSQL client type: tiberius over a tokio TcpStream via the compat shim.
pub(crate) type MssqlClient = tiberius::Client<Compat<TcpStream>>;

/// One PostgreSQL client plus the spawned driver task. tokio-postgres splits a
/// connection into a `Client` (issues queries, `&self`) and a `Connection`
/// future that must be polled to drive the socket; we spawn the latter and abort
/// it on close.
pub struct PgConn {
    worker: NetworkQueryWorker,
}

impl PgConn {
    pub(crate) fn abort_driver(&self) {
        self.worker.abort();
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.worker.is_closed()
    }

    pub(crate) fn worker(&self) -> &NetworkQueryWorker {
        &self.worker
    }
}

/// An open database, one variant per engine.
///
/// `Send + Sync`: SQLite's `Connection` is `Send` but `!Sync`, so the inner
/// `Mutex` makes it shareable; tokio-postgres `Client` is already `Send + Sync`;
/// tiberius `Client` is `Send` and wrapped in a tokio `Mutex` (its query API is
/// `&mut self`). The outer `Arc<DbHandle>` is therefore freely cloneable across
/// tasks.
// Stored inside `ProductionConnectionActor`, which itself is shared by the
// registry and one operation future at a time. Boxing a variant would add a
// redundant indirection under the actor Arc.
#[allow(clippy::large_enum_variant)]
pub enum DbHandle {
    Sqlite(Mutex<Connection>),
    Postgres(PgConn),
    Mssql(NetworkQueryWorker),
}

/// connId → production actor. The actor, rather than a public Arc handle, owns
/// the exact identity, driver and teardown state.
#[derive(Clone, Default)]
pub struct DbState(
    pub Arc<Mutex<HashMap<String, Arc<ProductionConnectionActor>>>>,
    Arc<AtomicBool>,
);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DatabaseShutdownTimeouts {
    pub per_actor: Duration,
    pub overall: Duration,
}

impl Default for DatabaseShutdownTimeouts {
    fn default() -> Self {
        Self {
            per_actor: Duration::from_secs(2),
            overall: Duration::from_secs(3),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DatabaseShutdownTimeoutKind {
    PerActor,
    Overall,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DatabaseActorShutdownOutcome {
    Closed(TeardownReport),
    SignalFailed {
        error: ActorError,
        final_state: TeardownReport,
    },
    SettlementFailed {
        error: ActorError,
        final_state: TeardownReport,
    },
    TimedOut {
        timeout: DatabaseShutdownTimeoutKind,
        final_state: TeardownReport,
    },
    TeardownFailed {
        error: ActorError,
        final_state: TeardownReport,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DatabaseActorShutdownReport {
    pub identity: ConnectionIdentity,
    pub lifecycle: Result<
        crate::db_connection_actor::LifecycleTeardownRequest,
        crate::db_connection_actor::ActorError,
    >,
    pub outcome: DatabaseActorShutdownOutcome,
    pub removed_from_registry: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DatabaseShutdownReport {
    pub already_started: bool,
    pub snapshot_count: usize,
    pub actors: Vec<DatabaseActorShutdownReport>,
    pub registry_remaining: Option<usize>,
    pub registry_error: Option<&'static str>,
}

impl DatabaseShutdownReport {
    pub fn has_failures(&self) -> bool {
        self.registry_error.is_some()
            || self
                .registry_remaining
                .is_some_and(|remaining| remaining != 0)
            || self.actors.iter().any(|actor| {
                !matches!(actor.outcome, DatabaseActorShutdownOutcome::Closed(_))
                    || !actor.removed_from_registry
            })
    }
}

static NEXT_CONN_ID: AtomicU64 = AtomicU64::new(1);

pub(crate) fn next_conn_id() -> String {
    format!("db-{}", NEXT_CONN_ID.fetch_add(1, Ordering::Relaxed))
}

macro_rules! opaque_database_id {
    ($name:ident) => {
        #[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq, Hash)]
        #[serde(transparent)]
        pub struct $name(pub String);
    };
}

opaque_database_id!(DescriptorId);
opaque_database_id!(ConnectionId);
opaque_database_id!(ConnectionGeneration);
opaque_database_id!(QueryRunId);
opaque_database_id!(StatementExecutionId);
opaque_database_id!(ResultSessionId);

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum PostgresTransportMode {
    #[default]
    VerifyFull,
    EncryptedTrustServerCert,
    InsecurePlaintext,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PostgresInsecureException {
    pub host: String,
    #[serde(default)]
    pub port: u16,
    pub user: String,
    pub database: String,
}

impl PostgresInsecureException {
    pub fn new(
        host: impl Into<String>,
        port: u16,
        user: impl Into<String>,
        database: impl Into<String>,
    ) -> Self {
        Self {
            host: host.into(),
            port,
            user: user.into(),
            database: database.into(),
        }
    }

    pub fn matches(&self, host: &str, port: u16, user: &str, database: &str) -> bool {
        self.port != 0
            && self.port == port
            && self.host == host
            && self.user == user
            && self.database == database
    }
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ProfileTarget {
    Sqlite {
        path: String,
    },
    Postgres {
        host: String,
        port: u16,
        database: String,
        user: String,
        #[serde(default)]
        transport_mode: PostgresTransportMode,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        insecure_exception: Option<PostgresInsecureException>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        trust_server_cert_acknowledged: bool,
    },
    Mssql {
        host: String,
        port: u16,
        database: String,
        user: String,
        trust_cert: bool,
    },
}

impl ProfileTarget {
    pub fn postgres(
        host: impl Into<String>,
        port: u16,
        database: impl Into<String>,
        user: impl Into<String>,
        transport_mode: PostgresTransportMode,
    ) -> Self {
        Self::Postgres {
            host: host.into(),
            port,
            database: database.into(),
            user: user.into(),
            transport_mode,
            insecure_exception: None,
            trust_server_cert_acknowledged: false,
        }
    }

    pub fn with_insecure_exception(mut self, exception: PostgresInsecureException) -> Self {
        if let Self::Postgres {
            transport_mode,
            insecure_exception,
            trust_server_cert_acknowledged,
            ..
        } = &mut self
        {
            *transport_mode = PostgresTransportMode::InsecurePlaintext;
            *insecure_exception = Some(exception);
            *trust_server_cert_acknowledged = false;
        }
        self
    }

    pub fn with_trust_server_cert_acknowledged(mut self) -> Self {
        if let Self::Postgres {
            transport_mode,
            insecure_exception,
            trust_server_cert_acknowledged,
            ..
        } = &mut self
        {
            *transport_mode = PostgresTransportMode::EncryptedTrustServerCert;
            *insecure_exception = None;
            *trust_server_cert_acknowledged = true;
        }
        self
    }

    pub fn postgres_transport_authorized(&self) -> bool {
        match self {
            Self::Postgres {
                host,
                port,
                database,
                user,
                transport_mode,
                insecure_exception,
                trust_server_cert_acknowledged,
            } => postgres_transport_is_authorized(
                *transport_mode,
                host,
                *port,
                user,
                database,
                insecure_exception.as_ref(),
                *trust_server_cert_acknowledged,
            ),
            Self::Sqlite { .. } | Self::Mssql { .. } => true,
        }
    }
}

pub fn postgres_transport_is_authorized(
    transport_mode: PostgresTransportMode,
    host: &str,
    port: u16,
    user: &str,
    database: &str,
    insecure_exception: Option<&PostgresInsecureException>,
    trust_server_cert_acknowledged: bool,
) -> bool {
    match transport_mode {
        PostgresTransportMode::VerifyFull => true,
        PostgresTransportMode::EncryptedTrustServerCert => trust_server_cert_acknowledged,
        PostgresTransportMode::InsecurePlaintext => insecure_exception
            .is_some_and(|exception| exception.matches(host, port, user, database)),
    }
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum CredentialState {
    NotRequired,
    Stored,
    Required,
    Unavailable,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProfileDescriptor {
    pub descriptor_id: DescriptorId,
    #[serde(default = "default_profile_config_generation")]
    pub config_generation: u64,
    pub name: String,
    pub target: ProfileTarget,
    pub credential_state: CredentialState,
}

fn default_profile_config_generation() -> u64 {
    1
}

/// Write-only secret input. It intentionally implements neither `Serialize`
/// nor `Debug`, preventing accidental readback or diagnostic formatting.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialInput {
    pub password: secrecy::SecretString,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileCreateRequest {
    pub name: String,
    pub target: ProfileTarget,
    pub credential: Option<CredentialInput>,
    #[serde(default)]
    pub transport_challenge_id: Option<String>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileUpdateRequest {
    pub descriptor_id: DescriptorId,
    pub name: String,
    pub target: ProfileTarget,
    pub replacement_credential: Option<CredentialInput>,
    #[serde(default)]
    pub transport_challenge_id: Option<String>,
}

#[derive(serde::Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum TestConnectionRequest {
    Ephemeral {
        target: ProfileTarget,
        credential: Option<CredentialInput>,
        #[serde(default)]
        transport_challenge_id: Option<String>,
    },
    Saved {
        descriptor_id: DescriptorId,
    },
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TestConnectionResult {
    pub elapsed_ms: u64,
    pub server_version: Option<String>,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionIdentity {
    pub descriptor_id: DescriptorId,
    pub connection_id: ConnectionId,
    pub connection_generation: ConnectionGeneration,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum LiveDatabaseEngine {
    Sqlite,
    Postgres,
    Mssql,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum DatabaseErrorEngine {
    Sqlite,
    Postgres,
    Mssql,
    Yuzora,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LiveConnection {
    pub descriptor_id: DescriptorId,
    pub connection_id: ConnectionId,
    pub connection_generation: ConnectionGeneration,
    pub engine: LiveDatabaseEngine,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct QueryRunOwner {
    pub descriptor_id: DescriptorId,
    pub connection_id: ConnectionId,
    pub connection_generation: ConnectionGeneration,
    pub query_run_id: QueryRunId,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StatementExecutionOwner {
    pub descriptor_id: DescriptorId,
    pub connection_id: ConnectionId,
    pub connection_generation: ConnectionGeneration,
    pub query_run_id: QueryRunId,
    pub statement_execution_id: StatementExecutionId,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ResultSessionOwner {
    pub descriptor_id: DescriptorId,
    pub connection_id: ConnectionId,
    pub connection_generation: ConnectionGeneration,
    pub query_run_id: QueryRunId,
    pub statement_execution_id: StatementExecutionId,
    pub result_session_id: ResultSessionId,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum DatabaseObjectKind {
    Table,
    View,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TableInfo {
    pub catalog: String,
    pub schema: String,
    pub name: String,
    pub kind: DatabaseObjectKind,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct ColumnInfo {
    pub name: String,
    /// Declared column type (may be empty for untyped columns).
    #[serde(rename = "type")]
    pub col_type: String,
    pub notnull: bool,
    pub pk: bool,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum DbValue {
    Null,
    Boolean { value: bool },
    Integer { value: String },
    Decimal { value: String },
    Text { value: String },
    Json { value: String },
    Date { value: String },
    Time { value: String },
    DateTime { value: String },
    Binary { hex: String },
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Retryability {
    Retryable,
    NotRetryable,
    Unknown,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ErrorPosition {
    pub offset: Option<u64>,
    pub line: Option<u64>,
    pub column: Option<u64>,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseError {
    pub engine: DatabaseErrorEngine,
    pub message: String,
    pub code: Option<String>,
    pub position: Option<ErrorPosition>,
    pub detail: Option<String>,
    pub hint: Option<String>,
    pub retryability: Retryability,
}

/// Small, path-safe operational envelope used by P3 orchestration. Detailed
/// vendor diagnostics remain owned by P4; these codes only describe recovery
/// actions the connection state machine can take without exposing raw paths.
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum DatabaseOperationalErrorCode {
    ConnectionFailed,
    ConnectionBusy,
    ServerDisconnected,
    MetadataFailed,
    QueryFailed,
    StaleConnection,
    SqlitePathMissing,
    SqlitePathNotFile,
    SqlitePathUnreadable,
    SqlitePathInvalid,
    SqliteOpenFailed,
    PostgresTransportRejected,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseOperationalError {
    pub code: DatabaseOperationalErrorCode,
    pub message: &'static str,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<DatabaseError>,
}

impl DatabaseOperationalError {
    pub(crate) fn new(code: DatabaseOperationalErrorCode, message: &'static str) -> Self {
        Self {
            code,
            message,
            error: None,
        }
    }

    pub(crate) fn with_database_error(mut self, error: DatabaseError) -> Self {
        self.error = Some(error);
        self
    }

    fn connection_failed() -> Self {
        Self::new(
            DatabaseOperationalErrorCode::ConnectionFailed,
            "database connection failed",
        )
    }

    fn postgres_transport_rejected() -> Self {
        Self::new(
            DatabaseOperationalErrorCode::PostgresTransportRejected,
            "PostgreSQL transport requires an explicit acknowledged exception",
        )
        .with_database_error(DatabaseError {
            engine: DatabaseErrorEngine::Yuzora,
            message: "PostgreSQL transport requires an explicit acknowledged exception".to_string(),
            code: Some("postgresTransportRejected".to_string()),
            position: None,
            detail: None,
            hint: Some(
                "Confirm the per-profile exception for this host, user, and database".to_string(),
            ),
            retryability: Retryability::NotRetryable,
        })
    }
}

impl std::fmt::Display for DatabaseOperationalError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.message)
    }
}

impl std::error::Error for DatabaseOperationalError {}

fn value_decode_error(
    engine: DatabaseErrorEngine,
    context: impl Into<String>,
    detail: impl Into<String>,
) -> DatabaseError {
    DatabaseError {
        engine,
        message: format!("failed to decode {}", context.into()),
        code: Some("valueDecode".to_string()),
        position: None,
        detail: Some(detail.into()),
        hint: None,
        retryability: Retryability::NotRetryable,
    }
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum EffectOutcome {
    None,
    Committed,
    RolledBack,
    TransactionPending,
    #[default]
    Unknown,
}

/// Driver-observed completion evidence. Engines only construct a conclusive
/// variant when their public driver API proves it; absence of evidence stays
/// `Unknown` instead of being inferred from SQL text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EngineCompletion {
    NoEffect,
    Committed,
    RolledBack,
    TransactionPending,
    Unknown,
}

fn effect_outcome_from_completion(completion: EngineCompletion) -> EffectOutcome {
    match completion {
        EngineCompletion::NoEffect => EffectOutcome::None,
        EngineCompletion::Committed => EffectOutcome::Committed,
        EngineCompletion::RolledBack => EffectOutcome::RolledBack,
        EngineCompletion::TransactionPending => EffectOutcome::TransactionPending,
        EngineCompletion::Unknown => EffectOutcome::Unknown,
    }
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ResultSession {
    pub owner: ResultSessionOwner,
    pub columns: Vec<String>,
    pub initial_page: ResultPage,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum StatementExecutionResult {
    Rows {
        result_session: Option<ResultSession>,
        #[serde(default)]
        affected_rows: Option<String>,
    },
    Execute {
        #[serde(default)]
        affected_rows: Option<String>,
    },
    Error {
        error: DatabaseError,
    },
    Cancelled {
        error: DatabaseError,
    },
    ResultLimitReached {
        result_session: ResultSession,
        #[serde(default)]
        affected_rows: Option<String>,
    },
    Skipped,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StatementExecution {
    pub statement_execution_id: StatementExecutionId,
    pub statement_index: usize,
    pub sql: String,
    #[serde(default)]
    pub effect_outcome: EffectOutcome,
    pub result: StatementExecutionResult,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct QueryRun {
    pub descriptor_id: DescriptorId,
    pub connection_id: ConnectionId,
    pub connection_generation: ConnectionGeneration,
    pub query_run_id: QueryRunId,
    pub statements: NonEmptyVec<StatementExecution>,
    pub transaction_may_be_open: bool,
    pub connection_terminated: bool,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum QueryRunMode {
    Primary,
    Script,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TransactionBoundary {
    None,
    Begin,
    Commit,
    Rollback,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct QueryExecutionUnit {
    pub sql: String,
    pub transaction_boundary: TransactionBoundary,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct QueryRunRequest {
    pub descriptor_id: DescriptorId,
    pub connection_id: ConnectionId,
    pub connection_generation: ConnectionGeneration,
    pub query_run_id: QueryRunId,
    pub mode: QueryRunMode,
    pub statements: NonEmptyVec<QueryExecutionUnit>,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum QueryCancelOutcome {
    Cancelled,
    CancelledConnectionTerminated,
    AlreadyRequested,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct QueryCancelResult {
    pub outcome: QueryCancelOutcome,
}

/// Serde-transparent 1..N collection. Its inner `Vec` is private so an empty
/// statement list cannot be constructed at runtime or accepted from IPC JSON.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NonEmptyVec<T>(Vec<T>);

impl<T> NonEmptyVec<T> {
    pub fn as_slice(&self) -> &[T] {
        &self.0
    }

    pub fn iter(&self) -> std::slice::Iter<'_, T> {
        self.0.iter()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn first_mut(&mut self) -> &mut T {
        // Construction and deserialization both enforce the invariant.
        self.0
            .first_mut()
            .expect("NonEmptyVec always contains at least one item")
    }
}

impl<T> TryFrom<Vec<T>> for NonEmptyVec<T> {
    type Error = &'static str;

    fn try_from(values: Vec<T>) -> Result<Self, Self::Error> {
        if values.is_empty() {
            Err("statements must contain at least one item")
        } else {
            Ok(Self(values))
        }
    }
}

impl<T> std::ops::Deref for NonEmptyVec<T> {
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        self.as_slice()
    }
}

impl<T> serde::Serialize for NonEmptyVec<T>
where
    T: serde::Serialize,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serde::Serialize::serialize(&self.0, serializer)
    }
}

impl<'de, T> serde::Deserialize<'de> for NonEmptyVec<T>
where
    T: serde::Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let values = <Vec<T> as serde::Deserialize>::deserialize(deserializer)?;
        Self::try_from(values).map_err(serde::de::Error::custom)
    }
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ResultPageDirection {
    Previous,
    Next,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ResultSessionLifecycle {
    Streaming,
    Complete,
    Released,
    Cancelled,
    Error,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ResultPageRequest {
    pub owner: ResultSessionOwner,
    pub direction: ResultPageDirection,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ResultPage {
    pub owner: ResultSessionOwner,
    pub page_index: usize,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<DbValue>>,
    pub has_previous: bool,
    pub has_next: bool,
    #[serde(default)]
    pub effect_outcome: EffectOutcome,
    pub lifecycle: ResultSessionLifecycle,
    #[serde(default)]
    pub result_limit_reached: bool,
    #[serde(default)]
    pub value_too_large: bool,
}

impl QueryRun {
    /// Runtime validation for the cardinality TypeScript encodes as a non-empty
    /// tuple: one QueryRun owns 1..N uniquely identified statement executions.
    pub fn validate_cardinality(&self) -> Result<(), &'static str> {
        let mut ids = HashSet::with_capacity(self.statements.len());
        for (expected_index, statement) in self.statements.iter().enumerate() {
            if statement.statement_index != expected_index {
                return Err("statement indexes must be contiguous and zero-based");
            }
            if !ids.insert(&statement.statement_execution_id) {
                return Err("statement execution ids must be unique within a query run");
            }
            let result_session = match &statement.result {
                StatementExecutionResult::Rows {
                    result_session: Some(session),
                    ..
                }
                | StatementExecutionResult::ResultLimitReached {
                    result_session: session,
                    ..
                } => Some(session),
                _ => None,
            };
            if let Some(session) = result_session {
                let owner = &session.owner;
                if owner.descriptor_id != self.descriptor_id
                    || owner.connection_id != self.connection_id
                    || owner.connection_generation != self.connection_generation
                    || owner.query_run_id != self.query_run_id
                    || owner.statement_execution_id != statement.statement_execution_id
                {
                    return Err("result session owner must match its statement execution");
                }
                if session.initial_page.owner != *owner
                    || session.initial_page.columns != session.columns
                    || session.initial_page.page_index != 0
                    || session.initial_page.rows.len() > crate::db_result_session::RESULT_PAGE_ROWS
                {
                    return Err("result session initial page must match its owner and columns");
                }
            }
        }
        Ok(())
    }
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
pub enum QueryResult {
    Select {
        columns: Vec<String>,
        rows: Vec<Vec<DbValue>>,
        truncated: bool,
        #[serde(default)]
        affected_rows: Option<String>,
        #[serde(default)]
        effect_outcome: EffectOutcome,
    },
    Execute {
        affected_rows: Option<String>,
        #[serde(default)]
        effect_outcome: EffectOutcome,
    },
}

pub const DEFAULT_MAX_ROWS: usize = 500;

/// Connection descriptor from the front-end. Passwords arrive in-flight only and
/// are never persisted anywhere in this module.
// Deliberately no `Debug`: network variants contain a plaintext password that
// must never enter logs, panic diagnostics, or generic debug output.
#[derive(serde::Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum DbOpenConfig {
    Sqlite {
        path: String,
    },
    Postgres {
        host: String,
        port: u16,
        database: String,
        user: String,
        password: SecretString,
        #[serde(default)]
        transport_mode: PostgresTransportMode,
        #[serde(default)]
        insecure_exception: Option<PostgresInsecureException>,
        #[serde(default)]
        trust_server_cert_acknowledged: bool,
    },
    Mssql {
        host: String,
        port: u16,
        database: String,
        user: String,
        password: SecretString,
        trust_cert: bool,
    },
}

// ---------------------------------------------------------------------------
// SQLite (unchanged MVP behaviour, run on the blocking pool)
// ---------------------------------------------------------------------------

/// Double-quote a SQLite identifier, escaping embedded quotes ("" ) so a table
/// name can never break out of the quotes / inject SQL. `PRAGMA table_info` and
/// the click-to-query builder both need this because they interpolate the name.
fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn sqlite_database_error(error: &rusqlite::Error) -> DatabaseError {
    let (message, code, position, retryability) = match error {
        rusqlite::Error::SqlInputError {
            error, msg, offset, ..
        } => (
            msg.clone(),
            Some(error.extended_code.to_string()),
            usize::try_from(*offset).ok().map(|offset| ErrorPosition {
                offset: Some(offset as u64),
                line: None,
                column: None,
            }),
            Retryability::NotRetryable,
        ),
        rusqlite::Error::SqliteFailure(sqlite, message) => {
            let retryability = if matches!(
                sqlite.code,
                rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
            ) {
                Retryability::Retryable
            } else {
                Retryability::Unknown
            };
            (
                message.clone().unwrap_or_else(|| error.to_string()),
                Some(sqlite.extended_code.to_string()),
                None,
                retryability,
            )
        }
        _ => (error.to_string(), None, None, Retryability::Unknown),
    };
    DatabaseError {
        engine: DatabaseErrorEngine::Sqlite,
        message,
        code,
        position,
        detail: None,
        hint: None,
        retryability,
    }
}

fn sqlite_worker_error(message: &'static str) -> DatabaseError {
    DatabaseError {
        engine: DatabaseErrorEngine::Yuzora,
        message: message.to_string(),
        code: Some("sqliteWorker".to_string()),
        position: None,
        detail: None,
        hint: None,
        retryability: Retryability::Unknown,
    }
}

/// One SQLite value → tagged, lossless wire value. Integers and floating-point
/// values cross the JavaScript boundary as decimal strings; BLOB bytes use hex.
/// SQLite TEXT with invalid UTF-8 is a hard decode error, never a lossy string.
fn sqlite_raw_len(value: ValueRef<'_>) -> usize {
    match value {
        ValueRef::Null | ValueRef::Integer(_) | ValueRef::Real(_) => 0,
        ValueRef::Text(bytes) | ValueRef::Blob(bytes) => bytes.len(),
    }
}

fn sqlite_retained_len(value: ValueRef<'_>) -> usize {
    match value {
        ValueRef::Null => 0,
        ValueRef::Integer(_) | ValueRef::Real(_) => 32,
        ValueRef::Text(bytes) => bytes.len(),
        ValueRef::Blob(bytes) => bytes.len().saturating_mul(2),
    }
}

fn convert_sqlite_row(
    row: &rusqlite::Row<'_>,
    column_count: usize,
    budget: RemainingBudget,
) -> Result<Result<Vec<DbValue>, ResultLimitKind>, DatabaseError> {
    let mut values = Vec::with_capacity(column_count);
    let mut row_used = values
        .capacity()
        .saturating_mul(std::mem::size_of::<DbValue>());
    for index in 0..column_count {
        let value_ref = row
            .get_ref(index)
            .map_err(|error| sqlite_database_error(&error))?;
        let raw = sqlite_raw_len(value_ref);
        if raw > budget.field {
            return Ok(Err(ResultLimitKind::Field));
        }
        let retained = sqlite_retained_len(value_ref);
        if row_used.saturating_add(retained) > budget.row {
            return Ok(Err(ResultLimitKind::Row));
        }
        if row_used.saturating_add(retained) > budget.session {
            return Ok(Err(ResultLimitKind::Session));
        }
        if row_used.saturating_add(retained) > budget.process {
            return Ok(Err(ResultLimitKind::Process));
        }
        values.push(value_to_db_value(value_ref)?);
        row_used = row_used.saturating_add(retained);
    }
    Ok(Ok(values))
}

fn sqlite_push_decoded_row(
    conn: &Connection,
    row: &rusqlite::Row<'_>,
    column_count: usize,
    sessions: &ResultSessionState,
    session_owner: &ResultSessionOwner,
) -> Result<PushRowOutcome, DatabaseError> {
    let budget = sessions
        .lock()
        .map_err(result_session_database_error)?
        .remaining_budget(session_owner)
        .map_err(result_session_database_error)?;
    match convert_sqlite_row(row, column_count, budget)? {
        Ok(values) => sessions
            .lock()
            .map_err(result_session_database_error)?
            .push_row(session_owner, values)
            .map_err(result_session_database_error),
        Err(kind) => apply_sqlite_limit(conn, sessions, session_owner, kind),
    }
}

fn apply_sqlite_limit(
    conn: &Connection,
    sessions: &ResultSessionState,
    session_owner: &ResultSessionOwner,
    kind: ResultLimitKind,
) -> Result<PushRowOutcome, DatabaseError> {
    conn.get_interrupt_handle().interrupt();
    let mut registry = sessions.lock().map_err(result_session_database_error)?;
    match kind {
        ResultLimitKind::Field | ResultLimitKind::Row => {
            registry
                .mark_value_too_large(session_owner)
                .map_err(result_session_database_error)?;
            Ok(PushRowOutcome::ValueTooLarge)
        }
        ResultLimitKind::Session | ResultLimitKind::Process => {
            registry
                .mark_result_limit_reached(session_owner)
                .map_err(result_session_database_error)?;
            Ok(PushRowOutcome::LimitReached)
        }
    }
}

fn value_to_db_value(v: ValueRef<'_>) -> Result<DbValue, DatabaseError> {
    match v {
        ValueRef::Null => Ok(DbValue::Null),
        ValueRef::Integer(n) => Ok(DbValue::Integer {
            value: n.to_string(),
        }),
        ValueRef::Real(f) => Ok(DbValue::Decimal {
            value: f.to_string(),
        }),
        ValueRef::Text(bytes) => std::str::from_utf8(bytes)
            .map(|value| DbValue::Text {
                value: value.to_string(),
            })
            .map_err(|error| {
                value_decode_error(
                    DatabaseErrorEngine::Sqlite,
                    "SQLite text value",
                    error.to_string(),
                )
            }),
        ValueRef::Blob(bytes) => Ok(DbValue::Binary {
            hex: encode_hex(bytes),
        }),
    }
}

/// Enumerate main/temp/attached SQLite namespaces and their user tables/views.
/// Namespace names originate from `database_list` and are still identifier-
/// quoted before selecting that namespace's `sqlite_schema`.
pub fn list_tables(conn: &Connection) -> Result<Vec<TableInfo>, DatabaseError> {
    let mut namespaces_stmt = conn
        .prepare("SELECT name FROM pragma_database_list ORDER BY seq")
        .map_err(|error| sqlite_database_error(&error))?;
    let namespace_rows = namespaces_stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| sqlite_database_error(&error))?;
    let mut namespaces = Vec::new();
    for namespace in namespace_rows {
        namespaces.push(namespace.map_err(|error| sqlite_database_error(&error))?);
    }
    drop(namespaces_stmt);

    let mut out = Vec::new();
    for namespace in namespaces {
        let sql = format!(
            "SELECT name, type FROM {}.sqlite_schema \
             WHERE type IN ('table', 'view') AND name NOT LIKE 'sqlite_%' \
             ORDER BY type, name",
            quote_ident(&namespace)
        );
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|error| sqlite_database_error(&error))?;
        let rows = stmt
            .query_map([], |row| {
                let raw_kind: String = row.get(1)?;
                Ok(TableInfo {
                    catalog: namespace.clone(),
                    schema: namespace.clone(),
                    name: row.get(0)?,
                    kind: if raw_kind.eq_ignore_ascii_case("view") {
                        DatabaseObjectKind::View
                    } else {
                        DatabaseObjectKind::Table
                    },
                })
            })
            .map_err(|error| sqlite_database_error(&error))?;
        for row in rows {
            out.push(row.map_err(|error| sqlite_database_error(&error))?);
        }
    }
    Ok(out)
}

/// Query columns for one exact namespace-qualified object. The table-valued
/// pragma accepts both object and schema as bound parameters, so duplicate
/// object names and hostile identifiers never collapse or interpolate as SQL.
pub fn table_columns(
    conn: &Connection,
    table: &TableInfo,
) -> Result<Vec<ColumnInfo>, DatabaseError> {
    if table.catalog != table.schema {
        return Ok(Vec::new());
    }
    let mut stmt = conn
        .prepare(
            "SELECT name, type, \"notnull\", pk \
             FROM pragma_table_xinfo(?1, ?2) ORDER BY cid",
        )
        .map_err(|error| sqlite_database_error(&error))?;
    let rows = stmt
        .query_map(rusqlite::params![&table.name, &table.schema], |row| {
            Ok(ColumnInfo {
                name: row.get(0)?,
                col_type: row.get(1)?,
                notnull: row.get::<_, i64>(2)? != 0,
                pk: row.get::<_, i64>(3)? != 0,
            })
        })
        .map_err(|error| sqlite_database_error(&error))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|error| sqlite_database_error(&error))?);
    }
    Ok(out)
}

struct SqliteCompletionProbe<'a> {
    connection: &'a Connection,
    committed: Arc<AtomicBool>,
    rolled_back: Arc<AtomicBool>,
    installed: bool,
}

impl<'a> SqliteCompletionProbe<'a> {
    fn install(connection: &'a Connection) -> Option<Self> {
        let committed = Arc::new(AtomicBool::new(false));
        let commit_flag = committed.clone();
        if connection
            .commit_hook(Some(move || {
                commit_flag.store(true, Ordering::SeqCst);
                false
            }))
            .is_err()
        {
            return None;
        }

        let rolled_back = Arc::new(AtomicBool::new(false));
        let rollback_flag = rolled_back.clone();
        if connection
            .rollback_hook(Some(move || rollback_flag.store(true, Ordering::SeqCst)))
            .is_err()
        {
            let _ = connection.commit_hook(None::<fn() -> bool>);
            return None;
        }

        Some(Self {
            connection,
            committed,
            rolled_back,
            installed: true,
        })
    }

    fn finish(mut self, read_only: bool, statement_completed: bool) -> EngineCompletion {
        let transaction_pending = !self.connection.is_autocommit();
        let committed = self.committed.load(Ordering::SeqCst);
        let rolled_back = self.rolled_back.load(Ordering::SeqCst);
        let commit_cleared = self.connection.commit_hook(None::<fn() -> bool>).is_ok();
        let rollback_cleared = self.connection.rollback_hook(None::<fn()>).is_ok();
        self.installed = !(commit_cleared && rollback_cleared);
        if self.installed {
            return EngineCompletion::Unknown;
        }
        if transaction_pending {
            EngineCompletion::TransactionPending
        } else if rolled_back {
            EngineCompletion::RolledBack
        } else if committed && statement_completed {
            EngineCompletion::Committed
        } else if read_only {
            EngineCompletion::NoEffect
        } else {
            EngineCompletion::Unknown
        }
    }
}

impl Drop for SqliteCompletionProbe<'_> {
    fn drop(&mut self) {
        if self.installed {
            let _ = self.connection.commit_hook(None::<fn() -> bool>);
            let _ = self.connection.rollback_hook(None::<fn()>);
        }
    }
}

/// Run one SQL statement. A prepared statement with columns (SELECT / PRAGMA /
/// RETURNING) yields a `Select` result capped at `max_rows` (with `truncated`
/// set when more rows exist); anything else runs via `execute` and reports the
/// affected row count. SQL errors retain structured engine diagnostics.
pub fn run_query(
    conn: &Connection,
    sql: &str,
    max_rows: usize,
) -> Result<QueryResult, DatabaseError> {
    let probe = SqliteCompletionProbe::install(conn);
    let mut stmt = conn
        .prepare(sql)
        .map_err(|error| sqlite_database_error(&error))?;
    let read_only = stmt.readonly();
    let col_count = stmt.column_count();
    if col_count == 0 {
        let affected = stmt
            .execute([])
            .map_err(|error| sqlite_database_error(&error))?;
        let completion = probe.map_or_else(
            || {
                if conn.is_autocommit() && read_only {
                    EngineCompletion::NoEffect
                } else if conn.is_autocommit() {
                    EngineCompletion::Unknown
                } else {
                    EngineCompletion::TransactionPending
                }
            },
            |probe| probe.finish(read_only, true),
        );
        return Ok(QueryResult::Execute {
            affected_rows: Some(affected.to_string()),
            effect_outcome: effect_outcome_from_completion(completion),
        });
    }
    let columns: Vec<String> = stmt
        .column_names()
        .into_iter()
        .map(|s| s.to_string())
        .collect();
    let mut rows = stmt
        .query([])
        .map_err(|error| sqlite_database_error(&error))?;
    let mut out: Vec<Vec<DbValue>> = Vec::new();
    let mut truncated = false;
    while let Some(row) = rows.next().map_err(|error| sqlite_database_error(&error))? {
        if out.len() >= max_rows {
            // One more row exists beyond the cap → mark truncated and stop.
            truncated = true;
            break;
        }
        let mut vals = Vec::with_capacity(col_count);
        for i in 0..col_count {
            vals.push(value_to_db_value(
                row.get_ref(i)
                    .map_err(|error| sqlite_database_error(&error))?,
            )?);
        }
        out.push(vals);
    }
    drop(rows);
    let completion = probe.map_or_else(
        || {
            if !conn.is_autocommit() {
                EngineCompletion::TransactionPending
            } else if read_only {
                EngineCompletion::NoEffect
            } else {
                EngineCompletion::Unknown
            }
        },
        |probe| probe.finish(read_only, !truncated),
    );
    let affected_rows = (!read_only && !truncated).then(|| conn.changes().to_string());
    Ok(QueryResult::Select {
        columns,
        rows: out,
        truncated,
        affected_rows,
        effect_outcome: effect_outcome_from_completion(completion),
    })
}

// ---------------------------------------------------------------------------
// PostgreSQL
// ---------------------------------------------------------------------------

/// How a Postgres column value is decoded into JSON, chosen from its `Type`.
/// Split out as a pure classifier so the type→decode mapping is unit-testable
/// without a live connection.
#[derive(Debug, PartialEq, Eq)]
enum PgColKind {
    Bool,
    I16,
    I32,
    I64,
    F32,
    F64,
    /// Kept as a lossless string (numeric/decimal precision would be lost as f64).
    Numeric,
    Text,
    Uuid,
    Timestamp,
    TimestampTz,
    Date,
    Time,
    Json,
    /// Preserved as exact bytes in the tagged binary/hex wire representation.
    Bytea,
    /// Anything else is rejected unless it is SQL NULL. Binary-protocol bytes
    /// for an unsupported type are never guessed to be text or replaced by a
    /// synthetic marker.
    Fallback,
}

fn classify_pg_type(ty: &PgType) -> PgColKind {
    if *ty == PgType::BOOL {
        PgColKind::Bool
    } else if *ty == PgType::INT2 {
        PgColKind::I16
    } else if *ty == PgType::INT4 {
        PgColKind::I32
    } else if *ty == PgType::INT8 {
        PgColKind::I64
    } else if *ty == PgType::FLOAT4 {
        PgColKind::F32
    } else if *ty == PgType::FLOAT8 {
        PgColKind::F64
    } else if *ty == PgType::NUMERIC {
        PgColKind::Numeric
    } else if *ty == PgType::VARCHAR
        || *ty == PgType::TEXT
        || *ty == PgType::BPCHAR
        || *ty == PgType::NAME
    {
        // NOTE: the internal "char" type (PgType::CHAR, OID 18) is deliberately
        // NOT here — String::accepts rejects it, so it decodes via the Fallback
        // PgText wrapper below (which special-cases its single-byte layout).
        PgColKind::Text
    } else if *ty == PgType::UUID {
        PgColKind::Uuid
    } else if *ty == PgType::TIMESTAMP {
        PgColKind::Timestamp
    } else if *ty == PgType::TIMESTAMPTZ {
        PgColKind::TimestampTz
    } else if *ty == PgType::DATE {
        PgColKind::Date
    } else if *ty == PgType::TIME {
        PgColKind::Time
    } else if *ty == PgType::JSON || *ty == PgType::JSONB {
        PgColKind::Json
    } else if *ty == PgType::BYTEA {
        PgColKind::Bytea
    } else {
        PgColKind::Fallback
    }
}

/// Convert a classified PostgreSQL decode result. Only `Ok(None)` is SQL NULL;
/// a driver/type decode failure crosses the mapper as a structured error.
fn pg_decode_result<T, E, F>(
    idx: usize,
    type_name: &str,
    got: Result<Option<T>, E>,
    f: F,
) -> Result<DbValue, DatabaseError>
where
    E: std::fmt::Display,
    F: FnOnce(T) -> DbValue,
{
    match got {
        Ok(Some(value)) => Ok(f(value)),
        Ok(None) => Ok(DbValue::Null),
        Err(error) => Err(value_decode_error(
            DatabaseErrorEngine::Postgres,
            format!("PostgreSQL column {idx} ({type_name})"),
            error.to_string(),
        )),
    }
}

fn pg_cell<T, F>(row: &PgRow, idx: usize, ty: &PgType, f: F) -> Result<DbValue, DatabaseError>
where
    T: for<'a> tokio_postgres::types::FromSql<'a>,
    F: FnOnce(T) -> DbValue,
{
    let got: Result<Option<T>, _> = row.try_get(idx);
    pg_decode_result(idx, ty.name(), got, f)
}

/// Exact PostgreSQL NUMERIC binary decoder. PostgreSQL sends base-10000 digit
/// groups; decoding them directly avoids the fixed precision ceiling of common
/// decimal crates and preserves the server's declared scale.
fn decode_pg_numeric(raw: &[u8]) -> Result<String, String> {
    const NUMERIC_POS: u16 = 0x0000;
    const NUMERIC_NEG: u16 = 0x4000;
    const NUMERIC_NAN: u16 = 0xC000;
    const NUMERIC_PINF: u16 = 0xD000;
    const NUMERIC_NINF: u16 = 0xF000;

    if raw.len() < 8 {
        return Err("postgres numeric payload is shorter than its header".to_string());
    }
    let read_i16 = |offset: usize| i16::from_be_bytes([raw[offset], raw[offset + 1]]);
    let read_u16 = |offset: usize| u16::from_be_bytes([raw[offset], raw[offset + 1]]);
    let ndigits = read_i16(0);
    if ndigits < 0 {
        return Err("postgres numeric digit count is negative".to_string());
    }
    let ndigits = ndigits as usize;
    let expected = 8usize
        .checked_add(
            ndigits
                .checked_mul(2)
                .ok_or_else(|| "postgres numeric digit count overflows".to_string())?,
        )
        .ok_or_else(|| "postgres numeric payload length overflows".to_string())?;
    if raw.len() != expected {
        return Err(format!(
            "postgres numeric payload has length {}, expected {expected}",
            raw.len()
        ));
    }

    let weight = i32::from(read_i16(2));
    let sign = read_u16(4);
    let scale = usize::from(read_u16(6));
    match sign {
        NUMERIC_NAN => return Ok("NaN".to_string()),
        NUMERIC_PINF => return Ok("Infinity".to_string()),
        NUMERIC_NINF => return Ok("-Infinity".to_string()),
        NUMERIC_POS | NUMERIC_NEG => {}
        _ => return Err(format!("postgres numeric has unknown sign 0x{sign:04x}")),
    }

    let mut digits = Vec::with_capacity(ndigits);
    for index in 0..ndigits {
        let digit = read_u16(8 + index * 2);
        if digit > 9999 {
            return Err(format!("postgres numeric digit {digit} exceeds base 10000"));
        }
        digits.push(digit);
    }

    let is_zero = digits.iter().all(|digit| *digit == 0);
    let mut value = String::new();
    if sign == NUMERIC_NEG && !is_zero {
        value.push('-');
    }

    let integer_groups = weight + 1;
    if integer_groups <= 0 {
        value.push('0');
    } else {
        for group in 0..integer_groups {
            let digit = digits.get(group as usize).copied().unwrap_or(0);
            if group == 0 {
                value.push_str(&digit.to_string());
            } else {
                value.push_str(&format!("{digit:04}"));
            }
        }
    }

    if scale > 0 {
        value.push('.');
        let fractional_groups = scale.div_ceil(4);
        let mut fractional = String::with_capacity(fractional_groups * 4);
        for group in 1..=fractional_groups {
            let digit_index = weight + group as i32;
            let digit = if digit_index >= 0 {
                digits.get(digit_index as usize).copied().unwrap_or(0)
            } else {
                0
            };
            fractional.push_str(&format!("{digit:04}"));
        }
        fractional.truncate(scale);
        value.push_str(&fractional);
    }
    Ok(value)
}

struct PgNumericText(String);

impl<'a> tokio_postgres::types::FromSql<'a> for PgNumericText {
    fn from_sql(
        _ty: &PgType,
        raw: &'a [u8],
    ) -> Result<Self, Box<dyn std::error::Error + Sync + Send>> {
        decode_pg_numeric(raw)
            .map(PgNumericText)
            .map_err(|message| {
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    message,
                )) as Box<dyn std::error::Error + Sync + Send>
            })
    }

    fn accepts(ty: &PgType) -> bool {
        *ty == PgType::NUMERIC
    }
}

/// Exact PostgreSQL JSON wire decoder. JSON arrives as UTF-8 text; JSONB uses
/// one binary-format version byte followed by its server-produced JSON text.
/// Neither path parses through `serde_json::Value`, so numeric tokens retain
/// their complete decimal spelling.
#[derive(Debug)]
struct PgJsonText(String);

impl<'a> tokio_postgres::types::FromSql<'a> for PgJsonText {
    fn from_sql(
        ty: &PgType,
        raw: &'a [u8],
    ) -> Result<Self, Box<dyn std::error::Error + Sync + Send>> {
        let json = if *ty == PgType::JSON {
            raw
        } else if *ty == PgType::JSONB {
            let Some((&version, json)) = raw.split_first() else {
                return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "postgres jsonb payload is missing its version byte",
                )));
            };
            if version != 1 {
                return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("unsupported postgres jsonb version {version}"),
                )));
            }
            json
        } else {
            return Err(Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("unsupported PostgreSQL JSON type {}", ty.name()),
            )));
        };
        Ok(PgJsonText(std::str::from_utf8(json)?.to_string()))
    }

    fn accepts(ty: &PgType) -> bool {
        *ty == PgType::JSON || *ty == PgType::JSONB
    }
}

fn pg_value_to_db_value(row: &PgRow, idx: usize, ty: &PgType) -> Result<DbValue, DatabaseError> {
    match classify_pg_type(ty) {
        PgColKind::Bool => pg_cell::<bool, _>(row, idx, ty, |value| DbValue::Boolean { value }),
        PgColKind::I16 => pg_cell::<i16, _>(row, idx, ty, |value| DbValue::Integer {
            value: value.to_string(),
        }),
        PgColKind::I32 => pg_cell::<i32, _>(row, idx, ty, |value| DbValue::Integer {
            value: value.to_string(),
        }),
        PgColKind::I64 => pg_cell::<i64, _>(row, idx, ty, |value| DbValue::Integer {
            value: value.to_string(),
        }),
        PgColKind::F32 => pg_cell::<f32, _>(row, idx, ty, |value| DbValue::Decimal {
            value: value.to_string(),
        }),
        PgColKind::F64 => pg_cell::<f64, _>(row, idx, ty, |value| DbValue::Decimal {
            value: value.to_string(),
        }),
        PgColKind::Numeric => {
            pg_cell::<PgNumericText, _>(row, idx, ty, |value| DbValue::Decimal { value: value.0 })
        }
        PgColKind::Text => pg_cell::<String, _>(row, idx, ty, |value| DbValue::Text { value }),
        PgColKind::Uuid => pg_cell::<uuid::Uuid, _>(row, idx, ty, |value| DbValue::Text {
            value: value.to_string(),
        }),
        PgColKind::Timestamp => {
            pg_cell::<chrono::NaiveDateTime, _>(row, idx, ty, |value| DbValue::DateTime {
                value: value.to_string(),
            })
        }
        PgColKind::TimestampTz => {
            pg_cell::<chrono::DateTime<chrono::Utc>, _>(row, idx, ty, |value| DbValue::DateTime {
                value: value.to_rfc3339(),
            })
        }
        PgColKind::Date => pg_cell::<chrono::NaiveDate, _>(row, idx, ty, |value| DbValue::Date {
            value: value.to_string(),
        }),
        PgColKind::Time => pg_cell::<chrono::NaiveTime, _>(row, idx, ty, |value| DbValue::Time {
            value: value.to_string(),
        }),
        PgColKind::Json => {
            pg_cell::<PgJsonText, _>(row, idx, ty, |value| DbValue::Json { value: value.0 })
        }
        PgColKind::Bytea => pg_cell::<Vec<u8>, _>(row, idx, ty, |value| DbValue::Binary {
            hex: encode_hex(&value),
        }),
        PgColKind::Fallback => pg_cell::<PgText, _>(row, idx, ty, |value| value.0),
    }
}

/// Strict fallback accepting every type only so an unsupported SQL NULL can
/// still decode as `None`. Non-NULL bytes must either have an explicitly known
/// representation (`char`) or fail as a structured value-decode error.
struct PgText(DbValue);

impl<'a> tokio_postgres::types::FromSql<'a> for PgText {
    fn from_sql(
        ty: &PgType,
        raw: &'a [u8],
    ) -> Result<Self, Box<dyn std::error::Error + Sync + Send>> {
        // The internal "char" type is a single signed byte, not UTF-8 text.
        if *ty == PgType::CHAR {
            let ch = raw
                .first()
                .map(|&b| (b as char).to_string())
                .unwrap_or_default();
            return Ok(PgText(DbValue::Text { value: ch }));
        }
        Err(Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("unsupported PostgreSQL binary type {}", ty.name()),
        )))
    }

    fn accepts(_ty: &PgType) -> bool {
        true
    }
}

/// trustCert 模式的 rustls 憑證驗證器：接受任何伺服器憑證（自簽 Postgres 用）。
/// 簽章驗證仍交給 provider 的演算法，只略過「憑證鏈是否可信」這一關。
#[derive(Debug)]
struct NoCertVerification(Arc<rustls::crypto::CryptoProvider>);

impl rustls::client::danger::ServerCertVerifier for NoCertVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

/// 建 Postgres 的 rustls TLS connector。trust_cert=true 時接受自簽憑證（略過鏈驗證），
/// 否則沿用 webpki 公開 CA 根憑證。固定綁 ring provider。
fn pg_tls(trust_cert: bool) -> Result<tokio_postgres_rustls::MakeRustlsConnect, String> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = if trust_cert {
        rustls::ClientConfig::builder_with_provider(provider.clone())
            .with_safe_default_protocol_versions()
            .map_err(|e| e.to_string())?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoCertVerification(provider)))
            .with_no_client_auth()
    } else {
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|e| e.to_string())?
            .with_root_certificates(roots)
            .with_no_client_auth()
    };
    Ok(tokio_postgres_rustls::MakeRustlsConnect::new(config))
}

/// tokio_postgres::Error 的 Display 只印 kind 的靜態字串（`"db error"`、
/// `"error connecting to server"`…），真因都藏在別處：伺服器錯誤在 `as_db_error()`
/// 的 DbError，傳輸／連線／TLS 錯誤在 `source()` chain。這個 helper 把真因還原成
/// 可診斷字串，供失敗 log 與對話框使用。
/// （DbError 只含伺服器回傳文字，source 只含 io/rustls 訊息——皆不含 client 端密碼。）
fn pg_err_detail(e: &tokio_postgres::Error) -> String {
    if let Some(db) = e.as_db_error() {
        // 例：relation "users" does not exist (42P01)
        return format!("{} ({})", db.message(), db.code().code());
    }
    // Display 只有泛稱；把 source() 逐層接上露出 io/rustls 真因。
    let mut msg = e.to_string();
    let mut src = std::error::Error::source(e);
    while let Some(s) = src {
        msg.push_str(": ");
        msg.push_str(&s.to_string());
        src = s.source();
    }
    msg
}

fn postgres_database_error(error: &tokio_postgres::Error) -> DatabaseError {
    if let Some(db) = error.as_db_error() {
        let code = db.code().code().to_string();
        let position = match db.position() {
            Some(tokio_postgres::error::ErrorPosition::Original(offset)) => Some(ErrorPosition {
                offset: Some(u64::from(*offset)),
                line: None,
                column: None,
            }),
            Some(tokio_postgres::error::ErrorPosition::Internal { .. }) | None => None,
        };
        let retryability =
            if code.starts_with("08") || matches!(code.as_str(), "40001" | "40P01" | "55P03") {
                Retryability::Retryable
            } else {
                Retryability::NotRetryable
            };
        DatabaseError {
            engine: DatabaseErrorEngine::Postgres,
            message: db.message().to_string(),
            code: Some(code),
            position,
            detail: db.detail().map(str::to_string),
            hint: db.hint().map(str::to_string),
            retryability,
        }
    } else {
        DatabaseError {
            engine: DatabaseErrorEngine::Postgres,
            message: pg_err_detail(error),
            code: None,
            position: None,
            detail: None,
            hint: None,
            retryability: Retryability::Unknown,
        }
    }
}

const POSTGRES_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug)]
struct PgConnectFailure {
    error: DatabaseError,
}

fn pg_transport_error_code(message: &str) -> &'static str {
    let lower = message.to_ascii_lowercase();
    if [
        "no such host",
        "host not found",
        "name or service not known",
        "failed to lookup address",
        "nodename nor servname",
        "getaddrinfo",
        "os error 11001",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
    {
        "dnsFailed"
    } else if [
        "tls",
        "certificate",
        "unknownissuer",
        "invalid peer",
        "handshake",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
    {
        "tlsFailed"
    } else if lower.contains("timed out") || lower.contains("timeout") {
        "connectionTimedOut"
    } else {
        "connectionFailed"
    }
}

fn redact_pg_connection_diagnostic(input: &str, secret: &str) -> String {
    let masked = crate::logging::mask_url_userinfo(input);
    if secret.is_empty() {
        masked
    } else {
        masked.replace(secret, "<redacted>")
    }
}

fn redact_postgres_database_error(mut error: DatabaseError, secret: &str) -> DatabaseError {
    error.message = redact_pg_connection_diagnostic(&error.message, secret);
    error.detail = error
        .detail
        .map(|detail| redact_pg_connection_diagnostic(&detail, secret));
    error.hint = error
        .hint
        .map(|hint| redact_pg_connection_diagnostic(&hint, secret));
    error
}

fn pg_driver_connect_failure(error: tokio_postgres::Error, secret: &str) -> PgConnectFailure {
    let mut diagnostic = postgres_database_error(&error);
    if diagnostic.code.is_none() {
        let code = pg_transport_error_code(&diagnostic.message);
        diagnostic.code = Some(code.to_string());
        diagnostic.retryability = match code {
            "dnsFailed" | "connectionTimedOut" | "connectionFailed" => Retryability::Retryable,
            "tlsFailed" => Retryability::NotRetryable,
            _ => Retryability::Unknown,
        };
    }
    PgConnectFailure {
        error: redact_postgres_database_error(diagnostic, secret),
    }
}

fn pg_timeout_failure() -> PgConnectFailure {
    PgConnectFailure {
        error: DatabaseError {
            engine: DatabaseErrorEngine::Postgres,
            message: "PostgreSQL connection timed out".to_string(),
            code: Some("connectionTimedOut".to_string()),
            position: None,
            detail: None,
            hint: Some("Verify the host, port, firewall, and server availability".to_string()),
            retryability: Retryability::Retryable,
        },
    }
}

fn pg_transport_policy_failure() -> PgConnectFailure {
    PgConnectFailure {
        error: DatabaseError {
            engine: DatabaseErrorEngine::Yuzora,
            message: "PostgreSQL transport requires an explicit acknowledged exception".to_string(),
            code: Some("postgresTransportRejected".to_string()),
            position: None,
            detail: None,
            hint: Some(
                "Confirm the per-profile exception for this host, user, and database".to_string(),
            ),
            retryability: Retryability::NotRetryable,
        },
    }
}

fn pg_tls_configuration_failure(message: String, secret: &str) -> PgConnectFailure {
    PgConnectFailure {
        error: DatabaseError {
            engine: DatabaseErrorEngine::Postgres,
            message: redact_pg_connection_diagnostic(&message, secret),
            code: Some("tlsFailed".to_string()),
            position: None,
            detail: None,
            hint: Some("Verify the PostgreSQL TLS and trust-certificate settings".to_string()),
            retryability: Retryability::NotRetryable,
        },
    }
}

fn pg_connect_failure_database_error(failure: &PgConnectFailure) -> DatabaseError {
    failure.error.clone()
}

struct LivePg {
    client: tokio_postgres::Client,
    cancel: PostgresCancelResource,
}

async fn pg_open_with_timeout(
    host: String,
    port: u16,
    database: String,
    user: String,
    password: SecretString,
    transport_mode: PostgresTransportMode,
    insecure_exception: Option<PostgresInsecureException>,
    trust_server_cert_acknowledged: bool,
    connect_timeout: Duration,
) -> Result<LivePg, PgConnectFailure> {
    if !postgres_transport_is_authorized(
        transport_mode,
        &host,
        port,
        &user,
        &database,
        insecure_exception.as_ref(),
        trust_server_cert_acknowledged,
    ) {
        return Err(pg_transport_policy_failure());
    }

    let mut cfg = tokio_postgres::Config::new();
    cfg.host(&host)
        .port(port)
        .dbname(&database)
        .user(&user)
        .password(password.expose_secret());

    // Password is attached only after the transport policy is accepted. The
    // Connection future's concrete type differs per TLS choice, but it is
    // consumed (spawned) inside each branch so both yield the same (Client, task).
    let use_tls = !matches!(transport_mode, PostgresTransportMode::InsecurePlaintext);
    let trust_cert = matches!(
        transport_mode,
        PostgresTransportMode::EncryptedTrustServerCert
    );
    let connected = if use_tls {
        let tls = pg_tls(trust_cert)
            .map_err(|error| pg_tls_configuration_failure(error, password.expose_secret()))?;
        let (client, connection) = tokio::time::timeout(connect_timeout, cfg.connect(tls.clone()))
            .await
            .map_err(|_| pg_timeout_failure())?
            .map_err(|error| pg_driver_connect_failure(error, password.expose_secret()))?;
        let cancel = PostgresCancelResource::rustls(&client, tls);
        tokio::spawn(async move {
            if let Err(e) = connection.await {
                eprintln!("postgres connection error: {e}");
            }
        });
        LivePg { client, cancel }
    } else {
        let (client, connection) =
            tokio::time::timeout(connect_timeout, cfg.connect(tokio_postgres::NoTls))
                .await
                .map_err(|_| pg_timeout_failure())?
                .map_err(|error| pg_driver_connect_failure(error, password.expose_secret()))?;
        let cancel = PostgresCancelResource::no_tls(&client);
        tokio::spawn(async move {
            if let Err(e) = connection.await {
                eprintln!("postgres connection error: {e}");
            }
        });
        LivePg { client, cancel }
    };
    Ok(connected)
}

async fn pg_open(
    host: String,
    port: u16,
    database: String,
    user: String,
    password: SecretString,
    transport_mode: PostgresTransportMode,
    insecure_exception: Option<PostgresInsecureException>,
    trust_server_cert_acknowledged: bool,
) -> Result<LivePg, PgConnectFailure> {
    pg_open_with_timeout(
        host,
        port,
        database,
        user,
        password,
        transport_mode,
        insecure_exception,
        trust_server_cert_acknowledged,
        POSTGRES_CONNECT_TIMEOUT,
    )
    .await
}

const PG_LIST_TABLES_SQL: &str =
    "SELECT table_catalog, table_schema, table_name, table_type FROM information_schema.tables \
     WHERE table_schema NOT IN ('pg_catalog', 'information_schema') \
     ORDER BY table_catalog, table_schema, table_type, table_name";

const PG_TABLE_COLUMNS_SQL: &str = "SELECT c.column_name, c.data_type, c.is_nullable, \
       EXISTS ( \
         SELECT 1 FROM information_schema.table_constraints tc \
         JOIN information_schema.key_column_usage kcu \
           ON kcu.constraint_catalog = tc.constraint_catalog \
          AND kcu.constraint_schema = tc.constraint_schema \
          AND kcu.constraint_name = tc.constraint_name \
         WHERE tc.constraint_type = 'PRIMARY KEY' \
           AND tc.table_catalog = c.table_catalog \
           AND tc.table_schema = c.table_schema \
           AND tc.table_name = c.table_name \
           AND kcu.column_name = c.column_name \
       ) AS is_primary_key \
     FROM information_schema.columns c \
     WHERE c.table_catalog = $1 AND c.table_schema = $2 AND c.table_name = $3 \
     ORDER BY c.ordinal_position";

async fn pg_list_tables(client: &tokio_postgres::Client) -> Result<Vec<TableInfo>, DatabaseError> {
    let rows = client
        .query(PG_LIST_TABLES_SQL, &[])
        .await
        .map_err(|error| postgres_database_error(&error))?;
    Ok(rows
        .iter()
        .map(|r| {
            let catalog: String = r.get(0);
            let schema: String = r.get(1);
            let name: String = r.get(2);
            let table_type: String = r.get(3);
            let kind = if table_type.eq_ignore_ascii_case("VIEW") {
                DatabaseObjectKind::View
            } else {
                DatabaseObjectKind::Table
            };
            TableInfo {
                catalog,
                schema,
                name,
                kind,
            }
        })
        .collect())
}

async fn pg_table_columns(
    client: &tokio_postgres::Client,
    table: &TableInfo,
) -> Result<Vec<ColumnInfo>, DatabaseError> {
    let rows = client
        .query(
            PG_TABLE_COLUMNS_SQL,
            &[&table.catalog, &table.schema, &table.name],
        )
        .await
        .map_err(|error| postgres_database_error(&error))?;
    Ok(rows
        .iter()
        .map(|r| {
            let name: String = r.get(0);
            let data_type: String = r.get(1);
            let is_nullable: String = r.get(2);
            let is_primary_key: bool = r.get(3);
            ColumnInfo {
                name,
                col_type: data_type,
                notnull: is_nullable.eq_ignore_ascii_case("NO"),
                pk: is_primary_key,
            }
        })
        .collect())
}

// ---------------------------------------------------------------------------
// MSSQL
// ---------------------------------------------------------------------------

/// One tiberius `ColumnData` → tagged, lossless value. Pure, so every branch is
/// unit-testable without a live SQL Server.
fn mssql_decode_result<T, E, F>(
    type_name: &str,
    got: Result<Option<T>, E>,
    f: F,
) -> Result<DbValue, DatabaseError>
where
    E: std::fmt::Display,
    F: FnOnce(T) -> DbValue,
{
    match got {
        Ok(Some(value)) => Ok(f(value)),
        Ok(None) => Ok(DbValue::Null),
        Err(error) => Err(value_decode_error(
            DatabaseErrorEngine::Mssql,
            format!("MSSQL {type_name}"),
            error.to_string(),
        )),
    }
}

fn format_mssql_numeric(value: tiberius::numeric::Numeric) -> String {
    let scale = usize::from(value.scale());
    let raw = value.value();
    let negative = raw.is_negative();
    let mut digits = raw.unsigned_abs().to_string();
    if scale > 0 {
        if digits.len() <= scale {
            digits.insert_str(0, &"0".repeat(scale + 1 - digits.len()));
        }
        digits.insert(digits.len() - scale, '.');
    }
    if negative {
        digits.insert(0, '-');
    }
    digits
}

fn mssql_value_to_db_value(data: &ColumnData<'static>) -> Result<DbValue, DatabaseError> {
    match data {
        ColumnData::U8(v) => Ok(v.as_ref().map_or(DbValue::Null, |n| DbValue::Integer {
            value: n.to_string(),
        })),
        ColumnData::I16(v) => Ok(v.as_ref().map_or(DbValue::Null, |n| DbValue::Integer {
            value: n.to_string(),
        })),
        ColumnData::I32(v) => Ok(v.as_ref().map_or(DbValue::Null, |n| DbValue::Integer {
            value: n.to_string(),
        })),
        ColumnData::I64(v) => Ok(v.as_ref().map_or(DbValue::Null, |n| DbValue::Integer {
            value: n.to_string(),
        })),
        ColumnData::F32(v) => Ok(v.as_ref().map_or(DbValue::Null, |n| DbValue::Decimal {
            value: n.to_string(),
        })),
        ColumnData::F64(v) => Ok(v.as_ref().map_or(DbValue::Null, |n| DbValue::Decimal {
            value: n.to_string(),
        })),
        ColumnData::Bit(v) => Ok(v
            .as_ref()
            .map_or(DbValue::Null, |value| DbValue::Boolean { value: *value })),
        ColumnData::String(v) => Ok(v.as_ref().map_or(DbValue::Null, |value| DbValue::Text {
            value: value.to_string(),
        })),
        ColumnData::Guid(v) => Ok(v.as_ref().map_or(DbValue::Null, |value| DbValue::Text {
            value: value.to_string(),
        })),
        ColumnData::Numeric(v) => Ok(v.as_ref().map_or(DbValue::Null, |value| DbValue::Decimal {
            value: format_mssql_numeric(*value),
        })),
        ColumnData::Binary(v) => Ok(v.as_ref().map_or(DbValue::Null, |value| DbValue::Binary {
            hex: encode_hex(value),
        })),
        ColumnData::Xml(v) => Ok(v.as_ref().map_or(DbValue::Null, |value| DbValue::Text {
            value: value.to_string(),
        })),
        // Date/time: reuse tiberius's chrono FromSql conversions for readable
        // strings, but preserve conversion failures as structured errors.
        ColumnData::DateTime(_) | ColumnData::SmallDateTime(_) | ColumnData::DateTime2(_) => {
            mssql_decode_result("dateTime", chrono::NaiveDateTime::from_sql(data), |value| {
                DbValue::DateTime {
                    value: value.to_string(),
                }
            })
        }
        ColumnData::Date(_) => {
            mssql_decode_result("date", chrono::NaiveDate::from_sql(data), |value| {
                DbValue::Date {
                    value: value.to_string(),
                }
            })
        }
        ColumnData::Time(_) => {
            mssql_decode_result("time", chrono::NaiveTime::from_sql(data), |value| {
                DbValue::Time {
                    value: value.to_string(),
                }
            })
        }
        ColumnData::DateTimeOffset(_) => mssql_decode_result(
            "dateTimeOffset",
            chrono::DateTime::<chrono::Utc>::from_sql(data),
            |value| DbValue::DateTime {
                value: value.to_rfc3339(),
            },
        ),
    }
}

async fn mssql_connect(
    host: String,
    port: u16,
    database: String,
    user: String,
    password: SecretString,
    trust_cert: bool,
) -> Result<MssqlClient, MssqlInternalError> {
    let mut config = MssqlConfig::new();
    config.host(&host);
    config.port(port);
    config.database(&database);
    config.authentication(AuthMethod::sql_server(&user, password.expose_secret()));
    if trust_cert {
        config.trust_cert();
    }
    let tcp = TcpStream::connect(config.get_addr())
        .await
        .map_err(|error| MssqlInternalError::Io(error.kind()))?;
    tcp.set_nodelay(true)
        .map_err(|error| MssqlInternalError::Io(error.kind()))?;
    tiberius::Client::connect(config, tcp.compat_write())
        .await
        .map_err(MssqlInternalError::Driver)
}

#[derive(Clone, Debug, PartialEq)]
enum MssqlInternalError {
    Io(std::io::ErrorKind),
    Driver(tiberius::error::Error),
    Value(DatabaseError),
}

fn mssql_database_error(error: &MssqlInternalError) -> DatabaseError {
    match error {
        MssqlInternalError::Value(error) => error.clone(),
        MssqlInternalError::Driver(tiberius::error::Error::Server(server)) => DatabaseError {
            engine: DatabaseErrorEngine::Mssql,
            message: server.message().to_string(),
            code: Some(server.code().to_string()),
            position: (server.line() > 0).then(|| ErrorPosition {
                offset: None,
                line: Some(u64::from(server.line())),
                column: None,
            }),
            detail: None,
            hint: None,
            retryability: if server.code() == 1205 {
                Retryability::Retryable
            } else {
                Retryability::NotRetryable
            },
        },
        MssqlInternalError::Driver(error) => DatabaseError {
            engine: DatabaseErrorEngine::Mssql,
            message: error.to_string(),
            code: error.code().map(|code| code.to_string()),
            position: None,
            detail: None,
            hint: None,
            retryability: Retryability::Unknown,
        },
        MssqlInternalError::Io(kind) => DatabaseError {
            engine: DatabaseErrorEngine::Mssql,
            message: format!("MSSQL transport error: {kind}"),
            code: None,
            position: None,
            detail: None,
            hint: None,
            retryability: Retryability::Retryable,
        },
    }
}

#[cfg(test)]
fn classify_mssql_live_error(
    error: &MssqlInternalError,
    fallback_code: DatabaseOperationalErrorCode,
    fallback_message: &'static str,
) -> DatabaseOperationalError {
    let operational = if matches!(
        error,
        MssqlInternalError::Io(_) | MssqlInternalError::Driver(tiberius::error::Error::Io { .. })
    ) {
        DatabaseOperationalError::new(
            DatabaseOperationalErrorCode::ServerDisconnected,
            "database server disconnected",
        )
    } else {
        DatabaseOperationalError::new(fallback_code, fallback_message)
    };
    operational.with_database_error(mssql_database_error(error))
}

const MSSQL_LIST_TABLES_SQL: &str =
    "SELECT table_catalog, table_schema, table_name, table_type FROM information_schema.tables \
     ORDER BY table_catalog, table_schema, table_type, table_name";

const MSSQL_TABLE_COLUMNS_SQL: &str = "SELECT c.column_name, c.data_type, c.is_nullable, \
       CAST(CASE WHEN EXISTS ( \
         SELECT 1 FROM information_schema.table_constraints tc \
         JOIN information_schema.key_column_usage kcu \
           ON kcu.constraint_catalog = tc.constraint_catalog \
          AND kcu.constraint_schema = tc.constraint_schema \
          AND kcu.constraint_name = tc.constraint_name \
         WHERE tc.constraint_type = 'PRIMARY KEY' \
           AND tc.table_catalog = c.table_catalog \
           AND tc.table_schema = c.table_schema \
           AND tc.table_name = c.table_name \
           AND kcu.column_name = c.column_name \
       ) THEN 1 ELSE 0 END AS bit) AS is_primary_key \
     FROM information_schema.columns c \
     WHERE c.table_catalog = @P1 AND c.table_schema = @P2 AND c.table_name = @P3 \
     ORDER BY c.ordinal_position";

async fn mssql_list_tables(client: &mut MssqlClient) -> Result<Vec<TableInfo>, MssqlInternalError> {
    let stream = client
        .query(MSSQL_LIST_TABLES_SQL, &[])
        .await
        .map_err(MssqlInternalError::Driver)?;
    let rows = stream
        .into_first_result()
        .await
        .map_err(MssqlInternalError::Driver)?;
    Ok(rows
        .iter()
        .map(|r| {
            let catalog: &str = r.get(0).unwrap_or_default();
            let schema: &str = r.get(1).unwrap_or_default();
            let name: &str = r.get(2).unwrap_or_default();
            let table_type: &str = r.get(3).unwrap_or_default();
            let kind = if table_type.eq_ignore_ascii_case("VIEW") {
                DatabaseObjectKind::View
            } else {
                DatabaseObjectKind::Table
            };
            TableInfo {
                catalog: catalog.to_string(),
                schema: schema.to_string(),
                name: name.to_string(),
                kind,
            }
        })
        .collect())
}

async fn mssql_table_columns(
    client: &mut MssqlClient,
    table: &TableInfo,
) -> Result<Vec<ColumnInfo>, MssqlInternalError> {
    let params: &[&dyn tiberius::ToSql] = &[&table.catalog, &table.schema, &table.name];
    let stream = client
        .query(MSSQL_TABLE_COLUMNS_SQL, params)
        .await
        .map_err(MssqlInternalError::Driver)?;
    let rows = stream
        .into_first_result()
        .await
        .map_err(MssqlInternalError::Driver)?;
    Ok(rows
        .iter()
        .map(|r| {
            let name: &str = r.get(0).unwrap_or_default();
            let data_type: &str = r.get(1).unwrap_or_default();
            let is_nullable: &str = r.get(2).unwrap_or_default();
            let is_primary_key: bool = r.get(3).unwrap_or(false);
            ColumnInfo {
                name: name.to_string(),
                col_type: data_type.to_string(),
                notnull: is_nullable.eq_ignore_ascii_case("NO"),
                pk: is_primary_key,
            }
        })
        .collect())
}

/// Checked aggregate of every server-reported DONE-family count in wire order.
/// Procedure/trigger counts may contribute; this is deliberately not labelled
/// as an outer-DML-only count.
fn aggregate_mssql_affected_rows(counts: &[u64]) -> Result<Option<String>, DatabaseError> {
    if counts.is_empty() {
        return Ok(None);
    }
    let total = counts
        .iter()
        .try_fold(0u128, |total, count| total.checked_add(u128::from(*count)));
    total.map(|total| Some(total.to_string())).ok_or_else(|| {
        value_decode_error(
            DatabaseErrorEngine::Mssql,
            "MSSQL affected-row count",
            "server-reported DONE count aggregate overflowed u128",
        )
    })
}

fn mssql_result_shape_error(detail: impl Into<String>) -> DatabaseError {
    DatabaseError {
        engine: DatabaseErrorEngine::Mssql,
        message: "MSSQL query returned a result shape that cannot be represented".to_string(),
        code: Some("resultShape".to_string()),
        position: None,
        detail: Some(detail.into()),
        hint: None,
        retryability: Retryability::NotRetryable,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MssqlRowAction {
    Decode,
    DrainOnly,
}

/// Pure materialization state used by the live MSSQL stream drain. It records
/// the first structured shape/value error without returning it from the item
/// loop, so the caller can continue polling to EOF and collect DONE tokens.
#[derive(Default)]
struct MssqlDrainState {
    primary_result_index: Option<usize>,
    columns: Option<Vec<String>>,
    rows: Vec<Vec<DbValue>>,
    truncated: bool,
    deferred_error: Option<DatabaseError>,
}

impl MssqlDrainState {
    fn defer_error(&mut self, error: DatabaseError) {
        if self.deferred_error.is_none() {
            self.deferred_error = Some(error);
        }
    }

    fn observe_metadata(&mut self, result_index: usize, columns: Vec<String>) {
        match self.primary_result_index {
            None => {
                self.primary_result_index = Some(result_index);
                self.columns = Some(columns);
            }
            Some(primary) if primary != result_index => {
                self.defer_error(mssql_result_shape_error(format!(
                    "the legacy single-result contract cannot represent result set {result_index}; the first result set index is {primary}"
                )));
            }
            Some(_) => {
                let expected = self.columns.as_ref().map_or(0, Vec::len);
                if self.columns.as_ref() != Some(&columns) {
                    self.defer_error(mssql_result_shape_error(format!(
                        "result set {result_index} metadata changed from {expected} columns to {}",
                        columns.len()
                    )));
                }
            }
        }
    }

    fn prepare_row(
        &mut self,
        result_index: usize,
        column_count: usize,
        max_rows: usize,
    ) -> MssqlRowAction {
        if self.deferred_error.is_some() {
            return MssqlRowAction::DrainOnly;
        }
        let Some(primary) = self.primary_result_index else {
            self.defer_error(mssql_result_shape_error(format!(
                "result set {result_index} produced a row before metadata"
            )));
            return MssqlRowAction::DrainOnly;
        };
        if primary != result_index {
            self.defer_error(mssql_result_shape_error(format!(
                "the legacy single-result contract cannot represent row data from result set {result_index}; the first result set index is {primary}"
            )));
            return MssqlRowAction::DrainOnly;
        }
        let expected = self.columns.as_ref().map_or(0, Vec::len);
        if expected != column_count {
            self.defer_error(mssql_result_shape_error(format!(
                "result set {result_index} declares {expected} columns but a row contains {column_count}"
            )));
            return MssqlRowAction::DrainOnly;
        }
        if self.rows.len() >= max_rows {
            self.truncated = true;
            return MssqlRowAction::DrainOnly;
        }
        MssqlRowAction::Decode
    }

    /// Intentionally returns `()` so a value-decode failure is deferred rather
    /// than propagated with `?` from the live stream loop.
    fn record_decoded_row(&mut self, decoded: Result<Vec<DbValue>, DatabaseError>) {
        match decoded {
            Ok(row) => self.rows.push(row),
            Err(error) => self.defer_error(error),
        }
    }

    fn finish(self, counts: &[u64]) -> Result<QueryResult, DatabaseError> {
        if let Some(error) = self.deferred_error {
            return Err(error);
        }
        mssql_result_from_drained(self.columns, self.rows, self.truncated, counts)
    }
}

fn mssql_result_from_drained(
    columns: Option<Vec<String>>,
    rows: Vec<Vec<DbValue>>,
    truncated: bool,
    counts: &[u64],
) -> Result<QueryResult, DatabaseError> {
    let affected_rows = aggregate_mssql_affected_rows(counts)?;
    match columns {
        Some(columns) => Ok(QueryResult::Select {
            columns,
            rows,
            truncated,
            affected_rows,
            effect_outcome: effect_outcome_from_completion(EngineCompletion::Unknown),
        }),
        None => Ok(QueryResult::Execute {
            affected_rows,
            effect_outcome: effect_outcome_from_completion(EngineCompletion::Unknown),
        }),
    }
}

async fn mssql_run_query(
    client: &mut MssqlClient,
    sql: &str,
    max_rows: usize,
) -> Result<QueryResult, MssqlInternalError> {
    let mut stream = client
        .query(sql, &[])
        .await
        .map_err(MssqlInternalError::Driver)?;
    let mut drained = MssqlDrainState::default();
    while let Some(item) = stream
        .try_next()
        .await
        .map_err(MssqlInternalError::Driver)?
    {
        match item {
            QueryItem::Metadata(metadata) => {
                drained.observe_metadata(
                    metadata.result_index(),
                    metadata
                        .columns()
                        .iter()
                        .map(|column| column.name().to_string())
                        .collect(),
                );
            }
            QueryItem::Row(row) => {
                let result_index = row.result_index();
                let column_count = row.columns().len();
                if drained.prepare_row(result_index, column_count, max_rows)
                    == MssqlRowAction::Decode
                {
                    let decoded = row
                        .into_iter()
                        .map(|cell| mssql_value_to_db_value(&cell))
                        .collect::<Result<_, _>>();
                    drained.record_decoded_row(decoded);
                }
            }
        }
    }
    drained
        .finish(stream.rows_affected())
        .map_err(MssqlInternalError::Value)
}

enum LiveNetwork {
    Postgres(LivePg),
    Mssql(MssqlClient),
}

struct PgRawLen(usize);

impl<'a> tokio_postgres::types::FromSql<'a> for PgRawLen {
    fn from_sql(
        _ty: &PgType,
        raw: &'a [u8],
    ) -> Result<Self, Box<dyn std::error::Error + Sync + Send>> {
        Ok(Self(raw.len()))
    }

    fn from_sql_null(_ty: &PgType) -> Result<Self, Box<dyn std::error::Error + Sync + Send>> {
        Ok(Self(0))
    }

    fn accepts(_ty: &PgType) -> bool {
        true
    }
}

fn mssql_raw_len(data: &ColumnData<'static>) -> usize {
    match data {
        ColumnData::String(Some(value)) => value.len(),
        ColumnData::Binary(Some(value)) => value.len(),
        ColumnData::Xml(Some(value)) => value.to_string().len(),
        _ => 0,
    }
}

fn helper_decode_limit(kind: ResultLimitKind) -> WorkerResponse {
    match kind {
        ResultLimitKind::Field | ResultLimitKind::Row => WorkerResponse::ValueTooLarge,
        ResultLimitKind::Session | ResultLimitKind::Process => WorkerResponse::ValueTooLarge,
    }
}

fn classify_helper_raw(raw_len: usize, row_used: usize) -> Option<ResultLimitKind> {
    if raw_len > DEFAULT_FIELD_BYTES {
        Some(ResultLimitKind::Field)
    } else if row_used.saturating_add(raw_len) > DEFAULT_ROW_BYTES {
        Some(ResultLimitKind::Row)
    } else {
        None
    }
}

type WorkerRequestReceiver = tokio::sync::mpsc::Receiver<Result<WorkerRequest, DatabaseError>>;
const WORKER_REQUEST_QUEUE_DEPTH: usize = 8;

// Length-prefixed stdin reads must never be cancelled after consuming a partial
// frame. One dedicated reader owns framing; streaming selects consume the
// bounded channel, whose recv operation is cancellation-safe.
async fn pump_worker_requests<R>(
    mut reader: R,
    sender: tokio::sync::mpsc::Sender<Result<WorkerRequest, DatabaseError>>,
) where
    R: tokio::io::AsyncRead + Unpin,
{
    loop {
        let request = read_request(&mut reader).await;
        let terminal = request.is_err();
        if sender.send(request).await.is_err() || terminal {
            return;
        }
    }
}

fn spawn_worker_request_reader() -> WorkerRequestReceiver {
    let (sender, receiver) = tokio::sync::mpsc::channel(WORKER_REQUEST_QUEUE_DEPTH);
    tokio::spawn(pump_worker_requests(tokio::io::stdin(), sender));
    receiver
}

async fn next_worker_request(
    requests: &mut WorkerRequestReceiver,
) -> Result<WorkerRequest, DatabaseError> {
    match requests.recv().await {
        Some(request) => request,
        None => Err(worker_error(
            "helperIo",
            "database helper request reader stopped",
        )),
    }
}

async fn helper_pg_query(
    live: &LivePg,
    sql: &str,
    stdout: &mut tokio::io::Stdout,
    requests: &mut WorkerRequestReceiver,
) -> Result<(), DatabaseError> {
    let statement = tokio::select! {
        biased;
        request = next_worker_request(requests) => {
            match request? {
                WorkerRequest::CancelQuery | WorkerRequest::Close => {
                    write_frame(stdout, &WorkerResponse::Cancelled).await?;
                    let _ = tokio::time::timeout(
                        std::time::Duration::from_secs(2),
                        live.cancel.cancel(),
                    )
                    .await;
                    return Ok(());
                }
                WorkerRequest::StopStreaming => {
                    write_frame(
                        stdout,
                        &WorkerResponse::End {
                            affected_rows: None,
                        },
                    )
                    .await?;
                    return Ok(());
                }
                _ => {
                    return Err(worker_error(
                        "helperProtocol",
                        "unexpected request before PostgreSQL statement prepare",
                    ));
                }
            }
        }
        statement = live.client.prepare(sql) => {
            statement.map_err(|error| postgres_database_error(&error))?
        }
    };
    if statement.columns().is_empty() {
        let affected = tokio::select! {
            biased;
            request = next_worker_request(requests) => {
                match request? {
                    WorkerRequest::CancelQuery | WorkerRequest::Close => {
                        write_frame(stdout, &WorkerResponse::Cancelled).await?;
                        let _ = tokio::time::timeout(
                            std::time::Duration::from_secs(2),
                            live.cancel.cancel(),
                        )
                        .await;
                        return Ok(());
                    }
                    WorkerRequest::StopStreaming => {
                        write_frame(
                            stdout,
                            &WorkerResponse::End {
                                affected_rows: None,
                            },
                        )
                        .await?;
                        return Ok(());
                    }
                    _ => {
                        return Err(worker_error(
                            "helperProtocol",
                            "unexpected request during PostgreSQL statement execution",
                        ));
                    }
                }
            }
            affected = live.client.execute(&statement, &[]) => {
                affected.map_err(|error| postgres_database_error(&error))?
            }
        };
        return write_frame(
            stdout,
            &WorkerResponse::Execute {
                affected_rows: Some(affected.to_string()),
            },
        )
        .await;
    }
    let columns: Vec<String> = statement
        .columns()
        .iter()
        .map(|column| column.name().to_string())
        .collect();
    let column_types: Vec<PgType> = statement
        .columns()
        .iter()
        .map(|column| column.type_().clone())
        .collect();
    write_frame(stdout, &WorkerResponse::RowMeta { columns }).await?;
    let params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = Vec::new();
    let stream = tokio::select! {
        biased;
        request = next_worker_request(requests) => {
            match request? {
                WorkerRequest::CancelQuery | WorkerRequest::Close => {
                    write_frame(stdout, &WorkerResponse::Cancelled).await?;
                    let _ = tokio::time::timeout(
                        std::time::Duration::from_secs(2),
                        live.cancel.cancel(),
                    )
                    .await;
                    return Ok(());
                }
                WorkerRequest::StopStreaming => {
                    write_frame(
                        stdout,
                        &WorkerResponse::End {
                            affected_rows: None,
                        },
                    )
                    .await?;
                    return Ok(());
                }
                _ => {
                    return Err(worker_error(
                        "helperProtocol",
                        "unexpected request before PostgreSQL query stream started",
                    ));
                }
            }
        }
        stream = live.client.query_raw(&statement, params) => {
            stream.map_err(|error| postgres_database_error(&error))?
        }
    };
    let (row_tx, mut row_rx) = tokio::sync::mpsc::unbounded_channel();
    let reader = tokio::spawn(async move {
        futures_util::pin_mut!(stream);
        loop {
            match stream.try_next().await {
                Ok(Some(row)) => {
                    if row_tx.send(Ok(Some(row))).is_err() {
                        break;
                    }
                }
                Ok(None) => {
                    let _ = row_tx.send(Ok(None));
                    break;
                }
                Err(error) => {
                    let _ = row_tx.send(Err(error));
                    break;
                }
            }
        }
    });
    let mut stop = false;
    loop {
        tokio::select! {
            biased;
            request = next_worker_request(requests) => {
                match request? {
                    WorkerRequest::StopStreaming => {
                        let _ = live.cancel.cancel().await;
                        stop = true;
                    }
                    WorkerRequest::CancelQuery => {
                        write_frame(stdout, &WorkerResponse::Cancelled).await?;
                        let _ = tokio::time::timeout(
                            std::time::Duration::from_secs(2),
                            live.cancel.cancel(),
                        )
                        .await;
                        drop(row_rx);
                        let _ = tokio::time::timeout(
                            std::time::Duration::from_secs(2),
                            reader,
                        )
                        .await;
                        return Ok(());
                    }
                    WorkerRequest::Close => {
                        let _ = live.cancel.cancel().await;
                        drop(row_rx);
                        let _ = reader.await;
                        return write_frame(stdout, &WorkerResponse::Closed).await;
                    }
                    _ => {}
                }
            }
            next = row_rx.recv(), if !stop => {
                match next {
                    None => {
                        return write_frame(
                            stdout,
                            &WorkerResponse::End {
                                affected_rows: None,
                            },
                        )
                        .await;
                    }
                    Some(Err(error)) => {
                        return Err(postgres_database_error(&error));
                    }
                    Some(Ok(None)) => {
                        return write_frame(
                            stdout,
                            &WorkerResponse::End {
                                affected_rows: None,
                            },
                        )
                        .await;
                    }
                    Some(Ok(Some(row))) => {
                        if row.raw_size_bytes() > DEFAULT_ROW_BYTES {
                            let _ = live.cancel.cancel().await;
                            return write_frame(stdout, &WorkerResponse::ValueTooLarge).await;
                        }
                        let mut values = Vec::with_capacity(column_types.len());
                        let mut row_used = 0;
                        for (index, column_type) in column_types.iter().enumerate() {
                            let raw = row
                                .try_get::<_, Option<PgRawLen>>(index)
                                .map_err(|error| {
                                    value_decode_error(
                                        DatabaseErrorEngine::Postgres,
                                        format!("PostgreSQL column {index}"),
                                        error.to_string(),
                                    )
                                })?
                                .map_or(0, |value| value.0);
                            if let Some(kind) = classify_helper_raw(raw, row_used) {
                                let _ = live.cancel.cancel().await;
                                return write_frame(stdout, &helper_decode_limit(kind)).await;
                            }
                            values.push(pg_value_to_db_value(&row, index, column_type)?);
                            row_used = row_used.saturating_add(raw);
                        }
                        write_frame(stdout, &WorkerResponse::Row { values }).await?;
                    }
                }
            }
        }
        if stop {
            while row_rx.recv().await.is_some() {}
            let _ = reader.await;
            return write_frame(
                stdout,
                &WorkerResponse::End {
                    affected_rows: None,
                },
            )
            .await;
        }
    }
}

fn helper_mssql_terminal_response(
    columns_sent: bool,
    affected_rows: Option<String>,
) -> WorkerResponse {
    if columns_sent {
        WorkerResponse::End { affected_rows }
    } else {
        WorkerResponse::Execute { affected_rows }
    }
}

async fn helper_mssql_query(
    client: &mut MssqlClient,
    sql: &str,
    stdout: &mut tokio::io::Stdout,
    requests: &mut WorkerRequestReceiver,
    terminate_on_cancel: &mut bool,
) -> Result<(), DatabaseError> {
    // Tiberius may wait for the first server response before yielding a stream.
    // Race that startup against the control channel so cancellation can still
    // terminate a long-running query that has not produced metadata or rows.
    let mut stream = tokio::select! {
        biased;
        request = next_worker_request(requests) => {
            match request? {
                WorkerRequest::CancelQuery => {
                    *terminate_on_cancel = true;
                    return write_frame(stdout, &WorkerResponse::Cancelled).await;
                }
                WorkerRequest::Close => {
                    *terminate_on_cancel = true;
                    return write_frame(stdout, &WorkerResponse::Closed).await;
                }
                WorkerRequest::StopStreaming => {
                    return write_frame(
                        stdout,
                        &WorkerResponse::End {
                            affected_rows: None,
                        },
                    )
                    .await;
                }
                _ => {
                    return Err(worker_error(
                        "helperProtocol",
                        "unexpected request before MSSQL query stream started",
                    ));
                }
            }
        }
        stream = client.simple_query(sql) => {
            stream.map_err(|error| mssql_database_error(&MssqlInternalError::Driver(error)))?
        }
    };
    let mut columns_sent = false;
    let mut stop = false;
    loop {
        tokio::select! {
            biased;
            request = next_worker_request(requests) => {
                match request? {
                    WorkerRequest::CancelQuery => {
                        *terminate_on_cancel = true;
                        return write_frame(stdout, &WorkerResponse::Cancelled).await;
                    }
                    WorkerRequest::StopStreaming => {
                        stop = true;
                    }
                    WorkerRequest::Close => {
                        *terminate_on_cancel = true;
                        return write_frame(stdout, &WorkerResponse::Closed).await;
                    }
                    _ => {}
                }
            }
            item = stream.try_next() => {
                let item = item.map_err(|error| {
                    mssql_database_error(&MssqlInternalError::Driver(error))
                })?;
                let Some(item) = item else {
                    let affected_rows = aggregate_mssql_affected_rows(stream.rows_affected())?;
                    return write_frame(
                        stdout,
                        &helper_mssql_terminal_response(columns_sent, affected_rows),
                    )
                    .await;
                };
                match item {
                    QueryItem::Metadata(metadata) => {
                        if !columns_sent {
                            write_frame(
                                stdout,
                                &WorkerResponse::RowMeta {
                                    columns: metadata
                                        .columns()
                                        .iter()
                                        .map(|column| column.name().to_string())
                                        .collect(),
                                },
                            )
                            .await?;
                            columns_sent = true;
                        }
                    }
                    QueryItem::Row(row) => {
                        if stop {
                            continue;
                        }
                        let mut values = Vec::new();
                        let mut row_used = 0;
                        for cell in row.into_iter() {
                            let raw = mssql_raw_len(&cell);
                            if let Some(kind) = classify_helper_raw(raw, row_used) {
                                return write_frame(stdout, &helper_decode_limit(kind)).await;
                            }
                            values.push(mssql_value_to_db_value(&cell)?);
                            row_used = row_used.saturating_add(raw);
                        }
                        write_frame(stdout, &WorkerResponse::Row { values }).await?;
                    }
                }
            }
        }
    }
}

pub(crate) async fn query_worker_loop() -> Result<(), DatabaseError> {
    let mut requests = spawn_worker_request_reader();
    let mut stdout = tokio::io::stdout();
    let connect = next_worker_request(&mut requests).await?;
    let mut engine = match connect {
        WorkerRequest::ConnectPostgres {
            host,
            port,
            database,
            user,
            mut password,
            transport_mode,
            insecure_exception,
            trust_server_cert_acknowledged,
        } => {
            let secret = SecretString::from(password.clone());
            password.zeroize();
            match pg_open(
                host,
                port,
                database,
                user,
                secret,
                transport_mode,
                insecure_exception,
                trust_server_cert_acknowledged,
            )
            .await
            {
                Ok(live) => {
                    write_frame(
                        &mut stdout,
                        &WorkerResponse::Ready {
                            engine: "postgres".into(),
                        },
                    )
                    .await?;
                    LiveNetwork::Postgres(live)
                }
                Err(failure) => {
                    write_frame(
                        &mut stdout,
                        &WorkerResponse::Error {
                            error: pg_connect_failure_database_error(&failure),
                        },
                    )
                    .await?;
                    return Ok(());
                }
            }
        }
        WorkerRequest::ConnectMssql {
            host,
            port,
            database,
            user,
            mut password,
            trust_cert,
        } => {
            let secret = SecretString::from(password.clone());
            password.zeroize();
            match mssql_connect(host, port, database, user, secret, trust_cert).await {
                Ok(client) => {
                    write_frame(
                        &mut stdout,
                        &WorkerResponse::Ready {
                            engine: "mssql".into(),
                        },
                    )
                    .await?;
                    LiveNetwork::Mssql(client)
                }
                Err(error) => {
                    write_frame(
                        &mut stdout,
                        &WorkerResponse::Error {
                            error: mssql_database_error(&error),
                        },
                    )
                    .await?;
                    return Ok(());
                }
            }
        }
        _ => {
            write_frame(
                &mut stdout,
                &WorkerResponse::Error {
                    error: worker_error("helperProtocol", "helper expected a connect request"),
                },
            )
            .await?;
            return Ok(());
        }
    };

    let mut pending_cancel = false;
    loop {
        let request = match next_worker_request(&mut requests).await {
            Ok(request) => request,
            Err(_) => break,
        };
        match request {
            WorkerRequest::Probe => {
                let value = match &mut engine {
                    LiveNetwork::Postgres(live) => live
                        .client
                        .query_one("SELECT version()", &[])
                        .await
                        .ok()
                        .map(|row| row.get::<_, String>(0)),
                    LiveNetwork::Mssql(client) => {
                        match mssql_run_query(client, "SELECT @@VERSION", 1).await {
                            Ok(QueryResult::Select { rows, .. }) => rows
                                .first()
                                .and_then(|row| row.first())
                                .and_then(|value| match value {
                                    DbValue::Text { value } => Some(value.clone()),
                                    _ => None,
                                }),
                            _ => None,
                        }
                    }
                };
                write_frame(&mut stdout, &WorkerResponse::Version { value }).await?;
            }
            WorkerRequest::ListTables => {
                let result = match &mut engine {
                    LiveNetwork::Postgres(live) => pg_list_tables(&live.client).await,
                    LiveNetwork::Mssql(client) => mssql_list_tables(client)
                        .await
                        .map_err(|error| mssql_database_error(&error)),
                };
                match result {
                    Ok(tables) => {
                        write_frame(&mut stdout, &WorkerResponse::Tables { tables }).await?;
                    }
                    Err(error) => {
                        write_frame(&mut stdout, &WorkerResponse::Error { error }).await?;
                    }
                }
            }
            WorkerRequest::TableColumns {
                catalog,
                schema,
                name,
                object_kind,
            } => {
                let table = TableInfo {
                    catalog,
                    schema,
                    name,
                    kind: object_kind,
                };
                let result = match &mut engine {
                    LiveNetwork::Postgres(live) => pg_table_columns(&live.client, &table).await,
                    LiveNetwork::Mssql(client) => mssql_table_columns(client, &table)
                        .await
                        .map_err(|error| mssql_database_error(&error)),
                };
                match result {
                    Ok(columns) => {
                        write_frame(&mut stdout, &WorkerResponse::Columns { columns }).await?;
                    }
                    Err(error) => {
                        write_frame(&mut stdout, &WorkerResponse::Error { error }).await?;
                    }
                }
            }
            WorkerRequest::Query { sql } => {
                if pending_cancel {
                    pending_cancel = false;
                    write_frame(&mut stdout, &WorkerResponse::Cancelled).await?;
                    continue;
                }
                let result = match &mut engine {
                    LiveNetwork::Postgres(live) => {
                        helper_pg_query(live, &sql, &mut stdout, &mut requests).await
                    }
                    LiveNetwork::Mssql(client) => {
                        let mut terminate = false;
                        let result = helper_mssql_query(
                            client,
                            &sql,
                            &mut stdout,
                            &mut requests,
                            &mut terminate,
                        )
                        .await;
                        if terminate {
                            return result;
                        }
                        result
                    }
                };
                if let Err(error) = result {
                    write_frame(&mut stdout, &WorkerResponse::Error { error }).await?;
                }
            }
            WorkerRequest::StopStreaming => {}
            WorkerRequest::CancelQuery => {
                pending_cancel = true;
            }
            WorkerRequest::Close => {
                write_frame(&mut stdout, &WorkerResponse::Closed).await?;
                break;
            }
            WorkerRequest::ConnectPostgres { .. } | WorkerRequest::ConnectMssql { .. } => {
                write_frame(
                    &mut stdout,
                    &WorkerResponse::Error {
                        error: worker_error(
                            "helperProtocol",
                            "helper connection is already established",
                        ),
                    },
                )
                .await?;
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Registry + commands
// ---------------------------------------------------------------------------

fn actor_error(error: ActorError) -> DatabaseOperationalError {
    let (code, message) = match error {
        ActorError::ConnectionBusy => (
            DatabaseOperationalErrorCode::ConnectionBusy,
            "database connection is busy",
        ),
        // Closed 表示這個 actor 世代已被 disconnect 或 cancel 終止，identity 永遠
        // 不再可用——對呼叫端而言與拿到舊世代 lease 相同，都是 stale。
        ActorError::OwnerMismatch | ActorError::StaleLease | ActorError::Closed => (
            DatabaseOperationalErrorCode::StaleConnection,
            "database connection identity is stale",
        ),
        ActorError::NoActiveExecution => (
            DatabaseOperationalErrorCode::ServerDisconnected,
            "database connection is disconnected",
        ),
        ActorError::ExecutionIdExhausted => (
            DatabaseOperationalErrorCode::ConnectionFailed,
            "database connection operation identity is exhausted",
        ),
        ActorError::CancelFailed => (
            DatabaseOperationalErrorCode::QueryFailed,
            "database query cancellation failed",
        ),
    };
    DatabaseOperationalError::new(code, message)
}

fn get_actor(
    state: &DbState,
    conn_id: &str,
) -> Result<Arc<ProductionConnectionActor>, DatabaseOperationalError> {
    state
        .0
        .lock()
        .map_err(|_| DatabaseOperationalError::connection_failed())?
        .get(conn_id)
        .cloned()
        .ok_or_else(|| {
            DatabaseOperationalError::new(
                DatabaseOperationalErrorCode::ServerDisconnected,
                "database connection is disconnected",
            )
        })
}

/// Run a blocking SQLite closure against a handle on the blocking pool, so the
/// std Mutex is never locked on an async worker thread.
async fn on_sqlite<T, F>(actor: Arc<ProductionConnectionActor>, f: F) -> Result<T, DatabaseError>
where
    T: Send + 'static,
    F: FnOnce(&Connection) -> Result<T, DatabaseError> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(move || match actor.handle() {
        DbHandle::Sqlite(conn) => {
            let conn = conn
                .lock()
                .map_err(|_| sqlite_worker_error("SQLite connection lock failed"))?;
            f(&conn)
        }
        _ => Err(sqlite_worker_error("database connection engine mismatch")),
    })
    .await
    .map_err(|_| sqlite_worker_error("SQLite worker task failed"))?
}

/// Validate the one production SQLite opening policy shared by Open, Test
/// Connection and Save-and-Connect. The returned canonical path is never
/// serialized; every failure is a fixed domain envelope with no raw path.
pub(crate) fn validate_existing_sqlite_path(
    path: impl AsRef<Path>,
) -> Result<PathBuf, DatabaseOperationalError> {
    let path = path.as_ref();
    if path == Path::new(":memory:") {
        return Err(DatabaseOperationalError::new(
            DatabaseOperationalErrorCode::SqlitePathInvalid,
            "in-memory SQLite is not available for saved connections",
        ));
    }

    let canonical = std::fs::canonicalize(path).map_err(|error| {
        let (code, message) = match error.kind() {
            std::io::ErrorKind::NotFound => (
                DatabaseOperationalErrorCode::SqlitePathMissing,
                "SQLite database file does not exist",
            ),
            std::io::ErrorKind::PermissionDenied => (
                DatabaseOperationalErrorCode::SqlitePathUnreadable,
                "SQLite database file is not readable",
            ),
            _ => (
                DatabaseOperationalErrorCode::SqlitePathInvalid,
                "SQLite database path is invalid",
            ),
        };
        DatabaseOperationalError::new(code, message)
    })?;
    let metadata = std::fs::metadata(&canonical).map_err(|error| {
        let code = if error.kind() == std::io::ErrorKind::PermissionDenied {
            DatabaseOperationalErrorCode::SqlitePathUnreadable
        } else {
            DatabaseOperationalErrorCode::SqlitePathInvalid
        };
        DatabaseOperationalError::new(code, "SQLite database file is unavailable")
    })?;
    if !metadata.is_file() {
        return Err(DatabaseOperationalError::new(
            DatabaseOperationalErrorCode::SqlitePathNotFile,
            "SQLite database path is not a regular file",
        ));
    }
    File::open(&canonical).map_err(|_| {
        DatabaseOperationalError::new(
            DatabaseOperationalErrorCode::SqlitePathUnreadable,
            "SQLite database file is not readable",
        )
    })?;
    Ok(canonical)
}

pub(crate) async fn open_unregistered(
    config: DbOpenConfig,
) -> Result<DbHandle, DatabaseOperationalError> {
    let handle = match config {
        DbOpenConfig::Sqlite { path } => {
            let canonical = validate_existing_sqlite_path(path)?;
            let conn = tauri::async_runtime::spawn_blocking(move || {
                Connection::open_with_flags(
                    canonical,
                    OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
                )
            })
            .await
            .map_err(|_| {
                DatabaseOperationalError::new(
                    DatabaseOperationalErrorCode::SqliteOpenFailed,
                    "SQLite database open task failed",
                )
            })?
            .map_err(|_| {
                DatabaseOperationalError::new(
                    DatabaseOperationalErrorCode::SqliteOpenFailed,
                    "SQLite database could not be opened",
                )
            })?;
            DbHandle::Sqlite(Mutex::new(conn))
        }
        DbOpenConfig::Postgres {
            host,
            port,
            database,
            user,
            password,
            transport_mode,
            insecure_exception,
            trust_server_cert_acknowledged,
        } => {
            let (lh, lu, ld) = (host.clone(), user.clone(), database.clone());
            let worker = NetworkQueryWorker::spawn_postgres(
                host,
                port,
                database,
                user,
                password,
                transport_mode,
                insecure_exception,
                trust_server_cert_acknowledged,
            )
            .await
            .map_err(|error| {
                let diagnostic = error;
                let diagnostic_code = diagnostic.code.as_deref().unwrap_or("connectionFailed");
                crate::logging::write_global(crate::logging::connect_failure_event(
                    "db",
                    &lh,
                    port,
                    &lu,
                    &format!(
                        "database={ld}: code={diagnostic_code}: {}",
                        diagnostic.message
                    ),
                ));
                if diagnostic_code == "postgresTransportRejected" {
                    DatabaseOperationalError::postgres_transport_rejected()
                } else {
                    DatabaseOperationalError::connection_failed().with_database_error(diagnostic)
                }
            })?;
            DbHandle::Postgres(PgConn { worker })
        }
        DbOpenConfig::Mssql {
            host,
            port,
            database,
            user,
            password,
            trust_cert,
        } => {
            let (lh, lu, ld) = (host.clone(), user.clone(), database.clone());
            let worker =
                NetworkQueryWorker::spawn_mssql(host, port, database, user, password, trust_cert)
                    .await
                    .map_err(|error| {
                        crate::logging::write_global(crate::logging::connect_failure_event(
                            "db",
                            &lh,
                            port,
                            &lu,
                            &format!("database={ld}: database connection failed"),
                        ));
                        DatabaseOperationalError::connection_failed().with_database_error(error)
                    })?;
            DbHandle::Mssql(worker)
        }
    };
    Ok(handle)
}

pub(crate) fn register_actor(
    state: &DbState,
    actor: Arc<ProductionConnectionActor>,
) -> Result<(), DatabaseOperationalError> {
    let connection_id = actor.identity().connection_id.0.clone();
    let mut actors = state
        .0
        .lock()
        .map_err(|_| DatabaseOperationalError::connection_failed())?;
    if state.1.load(Ordering::Acquire) {
        return Err(DatabaseOperationalError::new(
            DatabaseOperationalErrorCode::ConnectionFailed,
            "database runtime is shutting down",
        ));
    }
    if actors.contains_key(&connection_id) {
        return Err(DatabaseOperationalError::new(
            DatabaseOperationalErrorCode::StaleConnection,
            "database connection identity already exists",
        ));
    }
    actors.insert(connection_id, actor);
    Ok(())
}

fn get_exact_actor(
    state: &DbState,
    identity: &ConnectionIdentity,
) -> Result<Arc<ProductionConnectionActor>, DatabaseOperationalError> {
    // 呼叫端帶著完整世代 identity：registry 查無此 connection_id 表示該世代
    // 已被 disconnect／cancel 收走，對呼叫端而言是 stale identity，而非
    // transport 層的 ServerDisconnected（raw get_actor 的語義保留給無世代的查詢）。
    let actor = get_actor(state, &identity.connection_id.0).map_err(|error| {
        if error.code == DatabaseOperationalErrorCode::ServerDisconnected {
            DatabaseOperationalError::new(
                DatabaseOperationalErrorCode::StaleConnection,
                "database connection identity is stale",
            )
        } else {
            error
        }
    })?;
    if actor.identity() != identity {
        return Err(DatabaseOperationalError::new(
            DatabaseOperationalErrorCode::StaleConnection,
            "database connection identity is stale",
        ));
    }
    Ok(actor)
}

pub(crate) fn has_exact_actor(state: &DbState, identity: &ConnectionIdentity) -> bool {
    get_exact_actor(state, identity).is_ok()
}

pub(crate) fn exact_actor_is_terminating(state: &DbState, identity: &ConnectionIdentity) -> bool {
    get_exact_actor(state, identity)
        .map(|actor| actor.is_terminating())
        .unwrap_or(false)
}

fn operation_failure(
    actor: &ProductionConnectionActor,
    code: DatabaseOperationalErrorCode,
    message: &'static str,
) -> DatabaseOperationalError {
    if matches!(actor.handle(), DbHandle::Postgres(postgres) if postgres.is_closed())
        || matches!(actor.handle(), DbHandle::Mssql(worker) if worker.is_closed())
    {
        DatabaseOperationalError::new(
            DatabaseOperationalErrorCode::ServerDisconnected,
            "database server disconnected",
        )
    } else {
        DatabaseOperationalError::new(code, message)
    }
}

fn operation_failure_with_database_error(
    actor: &ProductionConnectionActor,
    code: DatabaseOperationalErrorCode,
    message: &'static str,
    error: DatabaseError,
) -> DatabaseOperationalError {
    operation_failure(actor, code, message).with_database_error(error)
}

fn cleanup_server_disconnect(
    state: &DbState,
    identity: &ConnectionIdentity,
    error: DatabaseOperationalError,
) -> DatabaseOperationalError {
    if error.code == DatabaseOperationalErrorCode::ServerDisconnected {
        let _ = close_exact_in_state(state, identity);
    }
    error
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ActorSettlementWait {
    SkippedAfterSignalFailure,
    Settled,
    Failed(ActorError),
    TimedOut(DatabaseShutdownTimeoutKind),
}

fn remove_exact_shutdown_actor(
    state: &DbState,
    identity: &ConnectionIdentity,
    actor: &Arc<ProductionConnectionActor>,
) -> Result<bool, ()> {
    let mut actors = state.0.lock().map_err(|_| ())?;
    match actors.get(&identity.connection_id.0) {
        Some(current) if Arc::ptr_eq(current, actor) && current.identity() == identity => {
            actors.remove(&identity.connection_id.0);
            Ok(true)
        }
        Some(_) => Ok(false),
        None => Ok(true),
    }
}

/// Deterministically shuts down the exact actor snapshot present when the
/// process-exit lifecycle begins.
///
/// The registry mutex is held only while taking the snapshot and while doing
/// each exact removal. Every actor receives its lifecycle signal before any
/// settlement wait starts. Settlement waits run concurrently and are bounded
/// by both the per-actor and overall budgets; a timed-out actor is reported as
/// such and removed from the registry without being misreported as closed.
pub(crate) async fn shutdown_all_connections(
    state: &DbState,
    timeouts: DatabaseShutdownTimeouts,
) -> DatabaseShutdownReport {
    let started = Instant::now();
    let already_started = state.1.swap(true, Ordering::AcqRel);
    let mut registry_error = None;
    let mut snapshot = match state.0.lock() {
        Ok(actors) => actors
            .values()
            .map(|actor| (actor.identity().clone(), Arc::clone(actor)))
            .collect::<Vec<_>>(),
        Err(_) => {
            return DatabaseShutdownReport {
                already_started,
                snapshot_count: 0,
                actors: Vec::new(),
                registry_remaining: None,
                registry_error: Some("database actor registry snapshot failed"),
            };
        }
    };
    snapshot.sort_by(|(left, _), (right, _)| {
        left.connection_id
            .0
            .cmp(&right.connection_id.0)
            .then_with(|| {
                left.connection_generation
                    .0
                    .cmp(&right.connection_generation.0)
            })
    });
    let snapshot_count = snapshot.len();

    let signalled = snapshot
        .into_iter()
        .map(|(identity, actor)| {
            let lifecycle = actor.request_lifecycle_teardown();
            (identity, actor, lifecycle)
        })
        .collect::<Vec<_>>();

    let overall_remaining = timeouts.overall.saturating_sub(started.elapsed());
    let (wait_budget, timeout_kind) = if overall_remaining <= timeouts.per_actor {
        (overall_remaining, DatabaseShutdownTimeoutKind::Overall)
    } else {
        (timeouts.per_actor, DatabaseShutdownTimeoutKind::PerActor)
    };
    let waits = signalled.iter().map(|(_, actor, lifecycle)| {
        let actor = Arc::clone(actor);
        let should_wait = lifecycle.is_ok();
        async move {
            if !should_wait {
                return ActorSettlementWait::SkippedAfterSignalFailure;
            }
            if wait_budget.is_zero() {
                return ActorSettlementWait::TimedOut(timeout_kind);
            }
            match tokio::time::timeout(wait_budget, actor.wait_for_settlement()).await {
                Ok(Ok(())) => ActorSettlementWait::Settled,
                Ok(Err(error)) => ActorSettlementWait::Failed(error),
                Err(_) => ActorSettlementWait::TimedOut(timeout_kind),
            }
        }
    });
    let waits = futures_util::future::join_all(waits).await;

    let mut actor_reports = Vec::with_capacity(signalled.len());
    for ((identity, actor, lifecycle), wait) in signalled.into_iter().zip(waits) {
        let outcome = match wait {
            ActorSettlementWait::Settled => match actor.begin_teardown() {
                Ok(report) => DatabaseActorShutdownOutcome::Closed(report),
                Err(error) => DatabaseActorShutdownOutcome::TeardownFailed {
                    error,
                    final_state: actor.teardown_report(),
                },
            },
            ActorSettlementWait::SkippedAfterSignalFailure => {
                let error = lifecycle
                    .as_ref()
                    .expect_err("signal failure wait must retain its actor error");
                let _ = actor.begin_teardown();
                DatabaseActorShutdownOutcome::SignalFailed {
                    error: *error,
                    final_state: actor.teardown_report(),
                }
            }
            ActorSettlementWait::Failed(error) => {
                let _ = actor.begin_teardown();
                DatabaseActorShutdownOutcome::SettlementFailed {
                    error,
                    final_state: actor.teardown_report(),
                }
            }
            ActorSettlementWait::TimedOut(timeout) => {
                let _ = actor.begin_teardown();
                DatabaseActorShutdownOutcome::TimedOut {
                    timeout,
                    final_state: actor.teardown_report(),
                }
            }
        };
        let removed_from_registry = match remove_exact_shutdown_actor(state, &identity, &actor) {
            Ok(removed) => removed,
            Err(()) => {
                registry_error = Some("database actor registry removal failed");
                false
            }
        };
        actor_reports.push(DatabaseActorShutdownReport {
            identity,
            lifecycle,
            outcome,
            removed_from_registry,
        });
    }

    let registry_remaining = match state.0.lock() {
        Ok(actors) => Some(actors.len()),
        Err(_) => {
            registry_error = Some("database actor registry final count failed");
            None
        }
    };
    DatabaseShutdownReport {
        already_started,
        snapshot_count,
        actors: actor_reports,
        registry_remaining,
        registry_error,
    }
}

pub(crate) fn close_exact_in_state(
    state: &DbState,
    identity: &ConnectionIdentity,
) -> Result<TeardownReport, DatabaseOperationalError> {
    let mut actors = state
        .0
        .lock()
        .map_err(|_| DatabaseOperationalError::connection_failed())?;
    let actor = actors.get(&identity.connection_id.0).ok_or_else(|| {
        DatabaseOperationalError::new(
            DatabaseOperationalErrorCode::StaleConnection,
            "database connection identity is stale",
        )
    })?;
    if actor.identity() != identity {
        return Err(DatabaseOperationalError::new(
            DatabaseOperationalErrorCode::StaleConnection,
            "database connection identity is stale",
        ));
    }
    let lifecycle = actor.request_lifecycle_teardown().map_err(actor_error)?;
    if lifecycle.busy {
        return Err(DatabaseOperationalError::new(
            DatabaseOperationalErrorCode::ConnectionBusy,
            "database connection termination is waiting for execution settlement",
        ));
    }
    let report = actor.begin_teardown().map_err(actor_error)?;
    actors.remove(&identity.connection_id.0);
    Ok(report)
}

/// Completes the cross-registry cleanup for an MSSQL Cancel termination.
///
/// Both the run and Cancel commands can observe driver settlement first. The
/// exact profile tombstone makes either ordering idempotent without allowing a
/// late finalizer to remove a newer connection generation.
pub(crate) fn finalize_terminated_connection(
    state: &DbState,
    sessions: &ResultSessionState,
    profiles: &crate::db_profiles::DatabaseProfileState,
    identity: &ConnectionIdentity,
) -> Result<(), DatabaseOperationalError> {
    match close_exact_in_state(state, identity) {
        Ok(_) => {}
        Err(error)
            if matches!(
                error.code,
                DatabaseOperationalErrorCode::StaleConnection
                    | DatabaseOperationalErrorCode::ServerDisconnected
            ) =>
        {
            let actors = state
                .0
                .lock()
                .map_err(|_| DatabaseOperationalError::connection_failed())?;
            if actors
                .get(&identity.connection_id.0)
                .is_some_and(|actor| actor.identity() != identity)
            {
                return Err(DatabaseOperationalError::new(
                    DatabaseOperationalErrorCode::StaleConnection,
                    "database connection identity is stale",
                ));
            }
        }
        Err(error) => return Err(error),
    }

    profiles
        .mark_exact_connection_offline(identity)
        .map_err(|error| {
            let code = if error.code == crate::db_profiles::ProfileErrorCode::StaleConnection {
                DatabaseOperationalErrorCode::StaleConnection
            } else {
                DatabaseOperationalErrorCode::ConnectionFailed
            };
            DatabaseOperationalError::new(code, "terminated database connection cleanup failed")
        })?;
    sessions
        .lock()
        .map_err(|_| {
            DatabaseOperationalError::new(
                DatabaseOperationalErrorCode::ConnectionFailed,
                "result session registry is unavailable",
            )
        })?
        .release_connection(identity)
        .map_err(|_| {
            DatabaseOperationalError::new(
                DatabaseOperationalErrorCode::ConnectionFailed,
                "result session cleanup failed",
            )
        })?;
    Ok(())
}

/// Opens and probes a database without ever inserting a handle into the live
/// registry. Used exclusively by P2 Test Connection.
pub(crate) async fn test_unregistered(
    config: DbOpenConfig,
) -> Result<Option<String>, DatabaseOperationalError> {
    let handle = open_unregistered(config).await?;
    match handle {
        DbHandle::Sqlite(connection) => tauri::async_runtime::spawn_blocking(move || {
            let connection = connection
                .into_inner()
                .map_err(|_| DatabaseOperationalError::connection_failed())?;
            connection
                .query_row("SELECT sqlite_version()", [], |row| row.get::<_, String>(0))
                .map(Some)
                .map_err(|_| DatabaseOperationalError::connection_failed())
        })
        .await
        .map_err(|_| DatabaseOperationalError::connection_failed())?,
        DbHandle::Postgres(postgres) => postgres
            .worker()
            .probe_version()
            .await
            .map_err(|_| DatabaseOperationalError::connection_failed()),
        DbHandle::Mssql(worker) => worker
            .probe_version()
            .await
            .map_err(|_| DatabaseOperationalError::connection_failed()),
    }
}

#[tauri::command]
pub async fn db_list_tables(
    state: tauri::State<'_, DbState>,
    identity: ConnectionIdentity,
) -> Result<Vec<TableInfo>, DatabaseOperationalError> {
    list_tables_in_state(&state, identity).await
}

pub(crate) async fn list_tables_in_state(
    state: &DbState,
    identity: ConnectionIdentity,
) -> Result<Vec<TableInfo>, DatabaseOperationalError> {
    let actor = get_exact_actor(state, &identity)?;
    let lease = actor.acquire_metadata().map_err(actor_error)?;
    let result: Result<_, DatabaseOperationalError> = match actor.handle() {
        DbHandle::Sqlite(_) => on_sqlite(actor.clone(), list_tables)
            .await
            .map_err(|error| {
                operation_failure_with_database_error(
                    &actor,
                    DatabaseOperationalErrorCode::MetadataFailed,
                    "database metadata request failed",
                    error,
                )
            }),
        DbHandle::Postgres(pg) => pg.worker().list_tables().await.map_err(|error| {
            operation_failure_with_database_error(
                &actor,
                DatabaseOperationalErrorCode::MetadataFailed,
                "database metadata request failed",
                error,
            )
        }),
        DbHandle::Mssql(worker) => worker.list_tables().await.map_err(|error| {
            operation_failure_with_database_error(
                &actor,
                DatabaseOperationalErrorCode::MetadataFailed,
                "database metadata request failed",
                error,
            )
        }),
    };
    actor.settle_metadata(&lease).map_err(actor_error)?;
    result.map_err(|error| cleanup_server_disconnect(state, &identity, error))
}

#[tauri::command]
pub async fn db_table_columns(
    state: tauri::State<'_, DbState>,
    identity: ConnectionIdentity,
    table: TableInfo,
) -> Result<Vec<ColumnInfo>, DatabaseOperationalError> {
    table_columns_in_state(&state, identity, table).await
}

pub(crate) async fn table_columns_in_state(
    state: &DbState,
    identity: ConnectionIdentity,
    table: TableInfo,
) -> Result<Vec<ColumnInfo>, DatabaseOperationalError> {
    let actor = get_exact_actor(state, &identity)?;
    let lease = actor.acquire_metadata().map_err(actor_error)?;
    let result: Result<_, DatabaseOperationalError> = match actor.handle() {
        DbHandle::Sqlite(_) => {
            let object = table.clone();
            on_sqlite(actor.clone(), move |conn| table_columns(conn, &object))
                .await
                .map_err(|error| {
                    operation_failure_with_database_error(
                        &actor,
                        DatabaseOperationalErrorCode::MetadataFailed,
                        "database metadata request failed",
                        error,
                    )
                })
        }
        DbHandle::Postgres(pg) => pg.worker().table_columns(&table).await.map_err(|error| {
            operation_failure_with_database_error(
                &actor,
                DatabaseOperationalErrorCode::MetadataFailed,
                "database metadata request failed",
                error,
            )
        }),
        DbHandle::Mssql(worker) => worker.table_columns(&table).await.map_err(|error| {
            operation_failure_with_database_error(
                &actor,
                DatabaseOperationalErrorCode::MetadataFailed,
                "database metadata request failed",
                error,
            )
        }),
    };
    actor.settle_metadata(&lease).map_err(actor_error)?;
    result.map_err(|error| cleanup_server_disconnect(state, &identity, error))
}

#[cfg(test)]
pub(crate) async fn query_in_state(
    state: &DbState,
    identity: ConnectionIdentity,
    query_run_id: QueryRunId,
    sql: String,
    max_rows: Option<usize>,
) -> Result<QueryResult, DatabaseOperationalError> {
    let cap = max_rows.unwrap_or(DEFAULT_MAX_ROWS);
    let actor = get_exact_actor(state, &identity)?;
    let capability = match actor.handle() {
        DbHandle::Sqlite(_) => CancelCapability::SqliteInterrupt,
        DbHandle::Postgres(_) => CancelCapability::PostgresProtocolCancel,
        DbHandle::Mssql(_) => CancelCapability::MssqlConnectionTermination,
    };
    let lease = actor
        .acquire_execution(
            QueryRunOwner {
                descriptor_id: identity.descriptor_id.clone(),
                connection_id: identity.connection_id.clone(),
                connection_generation: identity.connection_generation.clone(),
                query_run_id,
            },
            capability,
        )
        .map_err(actor_error)?;
    let result: Result<_, DatabaseOperationalError> = match actor.handle() {
        DbHandle::Sqlite(_) => {
            let s = sql.clone();
            on_sqlite(actor.clone(), move |conn| run_query(conn, &s, cap))
                .await
                .map_err(|error| {
                    operation_failure_with_database_error(
                        &actor,
                        DatabaseOperationalErrorCode::QueryFailed,
                        "database query failed",
                        error,
                    )
                })
        }
        DbHandle::Postgres(pg) => network_run_capped_query(pg.worker(), &sql, cap)
            .await
            .map_err(|error| {
                operation_failure_with_database_error(
                    &actor,
                    DatabaseOperationalErrorCode::QueryFailed,
                    "database query failed",
                    error,
                )
            }),
        DbHandle::Mssql(worker) => {
            network_run_capped_query(worker, &sql, cap)
                .await
                .map_err(|error| {
                    operation_failure_with_database_error(
                        &actor,
                        DatabaseOperationalErrorCode::QueryFailed,
                        "database query failed",
                        error,
                    )
                })
        }
    };
    actor.settle_execution(&lease).map_err(actor_error)?;
    result.map_err(|error| cleanup_server_disconnect(state, &identity, error))
}

fn result_session_database_error(error: SessionError) -> DatabaseError {
    DatabaseError {
        engine: DatabaseErrorEngine::Yuzora,
        message: "result session state is unavailable".to_string(),
        code: Some("resultSessionState".to_string()),
        position: None,
        detail: Some(format!("{error:?}")),
        hint: None,
        retryability: Retryability::NotRetryable,
    }
}

struct SessionAbortGuard {
    sessions: ResultSessionState,
    owner: ResultSessionOwner,
    armed: bool,
}

impl SessionAbortGuard {
    fn new(sessions: ResultSessionState, owner: ResultSessionOwner) -> Self {
        Self {
            sessions,
            owner,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for SessionAbortGuard {
    fn drop(&mut self) {
        if self.armed {
            if let Ok(mut sessions) = self.sessions.lock() {
                let _ = sessions.discard(&self.owner);
            }
        }
    }
}

struct ExecutionSettlementGuard {
    actor: Arc<ProductionConnectionActor>,
    lease: Option<crate::db_connection_actor::ExecutionLease>,
}

impl ExecutionSettlementGuard {
    fn new(
        actor: Arc<ProductionConnectionActor>,
        lease: crate::db_connection_actor::ExecutionLease,
    ) -> Self {
        Self {
            actor,
            lease: Some(lease),
        }
    }

    fn settle(mut self) -> Result<crate::db_connection_actor::Settlement, ActorError> {
        self.settle_with_policy(false)
    }

    fn settle_with_policy(
        &mut self,
        terminate_if_cancelled: bool,
    ) -> Result<crate::db_connection_actor::Settlement, ActorError> {
        let lease = self
            .lease
            .take()
            .expect("execution settlement guard is armed");
        self.actor
            .settle_execution_with_policy(&lease, terminate_if_cancelled)
    }
}

impl Drop for ExecutionSettlementGuard {
    fn drop(&mut self) {
        if let Some(lease) = self.lease.take() {
            let _ = self.actor.settle_execution(&lease);
        }
    }
}

fn sqlite_run_materialized_unit(
    conn: &Connection,
    sql: &str,
    sessions: &ResultSessionState,
    session_owner: ResultSessionOwner,
) -> Result<(StatementExecutionResult, EffectOutcome, bool), DatabaseError> {
    let probe = SqliteCompletionProbe::install(conn);
    let mut statement = conn
        .prepare(sql)
        .map_err(|error| sqlite_database_error(&error))?;
    let read_only = statement.readonly();
    let column_count = statement.column_count();
    if column_count == 0 {
        let affected = statement
            .execute([])
            .map_err(|error| sqlite_database_error(&error))?;
        let completion = probe.map_or_else(
            || {
                if !conn.is_autocommit() {
                    EngineCompletion::TransactionPending
                } else if read_only {
                    EngineCompletion::NoEffect
                } else {
                    EngineCompletion::Unknown
                }
            },
            |probe| probe.finish(read_only, true),
        );
        let effect_outcome = effect_outcome_from_completion(completion);
        return Ok((
            StatementExecutionResult::Execute {
                affected_rows: Some(affected.to_string()),
            },
            effect_outcome,
            false,
        ));
    }

    let columns: Vec<String> = statement
        .column_names()
        .into_iter()
        .map(str::to_string)
        .collect();
    sessions
        .lock()
        .map_err(result_session_database_error)?
        .begin_session(session_owner.clone(), columns)
        .map_err(result_session_database_error)?;
    let mut session_guard = SessionAbortGuard::new(sessions.clone(), session_owner.clone());
    let mut rows = statement
        .query([])
        .map_err(|error| sqlite_database_error(&error))?;
    let mut limit_reached = false;
    while let Some(row) = rows.next().map_err(|error| sqlite_database_error(&error))? {
        let outcome = sqlite_push_decoded_row(conn, row, column_count, sessions, &session_owner)?;
        if matches!(
            outcome,
            PushRowOutcome::LimitReached | PushRowOutcome::ValueTooLarge
        ) {
            // SQLite is pull-driven. Interrupting and then leaving this lexical
            // Rows scope stops the current unit without executing another unit.
            conn.get_interrupt_handle().interrupt();
            limit_reached = true;
            break;
        }
    }
    drop(rows);
    let effect_outcome = if limit_reached {
        EffectOutcome::Unknown
    } else {
        let completion = probe.map_or_else(
            || {
                if !conn.is_autocommit() {
                    EngineCompletion::TransactionPending
                } else if read_only {
                    EngineCompletion::NoEffect
                } else {
                    EngineCompletion::Unknown
                }
            },
            |probe| probe.finish(read_only, true),
        );
        effect_outcome_from_completion(completion)
    };
    let affected_rows = (!read_only && !limit_reached).then(|| conn.changes().to_string());
    let result_session = sessions
        .lock()
        .map_err(result_session_database_error)?
        .finish_session(&session_owner, effect_outcome)
        .map_err(result_session_database_error)?;
    session_guard.disarm();
    let result = if limit_reached {
        StatementExecutionResult::ResultLimitReached {
            result_session,
            affected_rows,
        }
    } else {
        StatementExecutionResult::Rows {
            result_session: Some(result_session),
            affected_rows,
        }
    };
    Ok((result, effect_outcome, limit_reached))
}

#[cfg(test)]
async fn network_run_capped_query(
    worker: &NetworkQueryWorker,
    sql: &str,
    max_rows: usize,
) -> Result<QueryResult, DatabaseError> {
    match worker.start_query(sql).await? {
        NetworkQueryStart::Execute { affected_rows } => Ok(QueryResult::Execute {
            affected_rows,
            effect_outcome: EffectOutcome::Unknown,
        }),
        NetworkQueryStart::Rows { columns } => {
            let mut rows = Vec::new();
            let mut truncated = false;
            loop {
                match worker.next_row().await? {
                    NetworkRow::Value(values) => {
                        if rows.len() >= max_rows {
                            truncated = true;
                            let _ = worker.stop_streaming().await;
                            drain_helper_stream(worker).await;
                            break;
                        }
                        rows.push(values);
                    }
                    NetworkRow::End { affected_rows } => {
                        return Ok(QueryResult::Select {
                            columns,
                            rows,
                            truncated,
                            affected_rows,
                            effect_outcome: EffectOutcome::Unknown,
                        });
                    }
                    NetworkRow::ValueTooLarge => {
                        return Err(crate::db_query_worker::value_too_large_error())
                    }
                    NetworkRow::Cancelled => return Err(cancelled_error()),
                }
            }
            Ok(QueryResult::Select {
                columns,
                rows,
                truncated,
                affected_rows: None,
                effect_outcome: EffectOutcome::Unknown,
            })
        }
    }
}

async fn drain_helper_stream(worker: &NetworkQueryWorker) {
    loop {
        match worker.next_row().await {
            Ok(NetworkRow::Value(_)) => {}
            Ok(NetworkRow::End { .. })
            | Ok(NetworkRow::ValueTooLarge)
            | Ok(NetworkRow::Cancelled)
            | Err(_) => break,
        }
    }
}

fn helper_confirmed_cancel(error: &DatabaseError) -> bool {
    error.code.as_deref() == Some("cancelled")
}

fn terminate_network_worker(worker: &NetworkQueryWorker) {
    worker.abort();
}

async fn settle_network_stream_cancel(
    worker: &NetworkQueryWorker,
    terminate_connection: bool,
) -> bool {
    if terminate_connection {
        let _ = worker.cancel_query().await;
        worker.abort();
        return true;
    }
    // PostgreSQL's actor already wrote one helper CancelQuery before it
    // signalled the worker. Reuse is safe only when the helper confirms that
    // it consumed that request for the current stream. End/error means the
    // cancel arrived after completion and may now be queued for the next query.
    loop {
        match worker.next_row().await {
            Ok(NetworkRow::Value(_)) => {}
            Ok(NetworkRow::Cancelled) => return false,
            Ok(NetworkRow::End { .. }) | Ok(NetworkRow::ValueTooLarge) | Err(_) => {
                worker.abort();
                return true;
            }
        }
    }
}

async fn network_run_materialized_unit(
    worker: &NetworkQueryWorker,
    sql: &str,
    sessions: &ResultSessionState,
    session_owner: ResultSessionOwner,
) -> Result<(StatementExecutionResult, EffectOutcome, bool), DatabaseError> {
    match worker.start_query(sql).await? {
        NetworkQueryStart::Execute { affected_rows } => Ok((
            StatementExecutionResult::Execute { affected_rows },
            EffectOutcome::Unknown,
            false,
        )),
        NetworkQueryStart::Rows { columns } => {
            sessions
                .lock()
                .map_err(result_session_database_error)?
                .begin_session(session_owner.clone(), columns)
                .map_err(result_session_database_error)?;
            let mut session_guard = SessionAbortGuard::new(sessions.clone(), session_owner.clone());
            let mut limit_reached = false;
            let mut affected_rows = None;
            loop {
                match worker.next_row().await? {
                    NetworkRow::Value(values) => {
                        let outcome = sessions
                            .lock()
                            .map_err(result_session_database_error)?
                            .push_row(&session_owner, values)
                            .map_err(result_session_database_error)?;
                        if matches!(
                            outcome,
                            PushRowOutcome::LimitReached | PushRowOutcome::ValueTooLarge
                        ) {
                            let _ = worker.stop_streaming().await;
                            drain_helper_stream(worker).await;
                            limit_reached = true;
                            break;
                        }
                    }
                    NetworkRow::End {
                        affected_rows: done,
                    } => {
                        affected_rows = done;
                        break;
                    }
                    NetworkRow::ValueTooLarge => {
                        sessions
                            .lock()
                            .map_err(result_session_database_error)?
                            .mark_value_too_large(&session_owner)
                            .map_err(result_session_database_error)?;
                        limit_reached = true;
                        break;
                    }
                    NetworkRow::Cancelled => {
                        return Ok((
                            StatementExecutionResult::Cancelled {
                                error: cancelled_error(),
                            },
                            EffectOutcome::Unknown,
                            true,
                        ));
                    }
                }
            }
            let effect_outcome = if limit_reached {
                EffectOutcome::Unknown
            } else {
                EffectOutcome::Unknown
            };
            let result_session = sessions
                .lock()
                .map_err(result_session_database_error)?
                .finish_session(&session_owner, effect_outcome)
                .map_err(result_session_database_error)?;
            session_guard.disarm();
            let result = if limit_reached {
                StatementExecutionResult::ResultLimitReached {
                    result_session,
                    affected_rows,
                }
            } else {
                StatementExecutionResult::Rows {
                    result_session: Some(result_session),
                    affected_rows,
                }
            };
            Ok((result, effect_outcome, limit_reached))
        }
    }
}

async fn pg_run_materialized_unit(
    connection: &PgConn,
    sql: &str,
    sessions: &ResultSessionState,
    session_owner: ResultSessionOwner,
) -> Result<(StatementExecutionResult, EffectOutcome, bool), DatabaseError> {
    network_run_materialized_unit(connection.worker(), sql, sessions, session_owner).await
}

struct P6UnitOutcome {
    result: StatementExecutionResult,
    effect_outcome: EffectOutcome,
    stop: bool,
    connection_terminated: bool,
}

fn apply_successful_transaction_boundary(
    transaction_may_be_open: &mut bool,
    boundary: TransactionBoundary,
    result: &StatementExecutionResult,
) {
    if !matches!(
        result,
        StatementExecutionResult::Rows { .. }
            | StatementExecutionResult::Execute { .. }
            | StatementExecutionResult::ResultLimitReached { .. }
    ) {
        return;
    }
    match boundary {
        TransactionBoundary::Begin => *transaction_may_be_open = true,
        TransactionBoundary::Commit | TransactionBoundary::Rollback => {
            *transaction_may_be_open = false
        }
        TransactionBoundary::None => {}
    }
}

fn mssql_cancelled_connection_error() -> DatabaseError {
    DatabaseError {
        engine: DatabaseErrorEngine::Mssql,
        message: "query cancelled by terminating the MSSQL connection".to_string(),
        code: Some("cancelledConnectionTerminated".to_string()),
        position: None,
        detail: None,
        hint: Some("Reconnect the saved connection before running another query".to_string()),
        retryability: Retryability::Retryable,
    }
}

fn network_primary_cancel_error(terminate_connection: bool) -> DatabaseError {
    if terminate_connection {
        mssql_cancelled_connection_error()
    } else {
        cancelled_error()
    }
}

async fn mssql_run_materialized_unit(
    worker: &NetworkQueryWorker,
    sql: &str,
    sessions: &ResultSessionState,
    session_owner: ResultSessionOwner,
    run_owner: &QueryRunOwner,
    cancel_rx: &mut tokio::sync::mpsc::UnboundedReceiver<QueryRunOwner>,
) -> Result<P6UnitOutcome, DatabaseError> {
    let query = network_run_materialized_unit(worker, sql, sessions, session_owner);
    tokio::pin!(query);
    loop {
        tokio::select! {
            request = cancel_rx.recv() => {
                match request {
                    Some(request) if request == *run_owner => {
                        let _ = worker.cancel_query().await;
                        worker.abort();
                        return Ok(P6UnitOutcome {
                            result: StatementExecutionResult::Cancelled {
                                error: mssql_cancelled_connection_error(),
                            },
                            effect_outcome: EffectOutcome::Unknown,
                            stop: true,
                            connection_terminated: true,
                        });
                    }
                    Some(_) => continue,
                    None => {
                        return Err(DatabaseError {
                            engine: DatabaseErrorEngine::Yuzora,
                            message: "MSSQL cancellation channel closed unexpectedly".to_string(),
                            code: Some("cancelChannelClosed".to_string()),
                            position: None,
                            detail: None,
                            hint: None,
                            retryability: Retryability::NotRetryable,
                        });
                    }
                }
            }
            result = &mut query => {
                let (result, effect_outcome, stop) = result?;
                return Ok(P6UnitOutcome {
                    result,
                    effect_outcome,
                    stop,
                    connection_terminated: false,
                });
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PrimaryPageRead {
    Streaming,
    End,
    LimitReached,
    ValueTooLarge,
}

enum CancellablePrimaryPageRead {
    Read(Result<PrimaryPageRead, DatabaseError>),
    Cancelled { connection_terminated: bool },
}

async fn mssql_run_primary_worker(
    actor: Arc<ProductionConnectionActor>,
    sessions: ResultSessionState,
    sql: String,
    run_owner: QueryRunOwner,
    session_owner: ResultSessionOwner,
    lease: ExecutionLease,
    settlement_guard: ExecutionSettlementGuard,
    continuation_sender: tokio::sync::mpsc::UnboundedSender<ResultContinuationCommand>,
    continuation_receiver: tokio::sync::mpsc::UnboundedReceiver<ResultContinuationCommand>,
    cancel_rx: tokio::sync::mpsc::UnboundedReceiver<QueryRunOwner>,
    initial_sender: PrimaryInitialSender,
) {
    let handle_actor = actor.clone();
    let DbHandle::Mssql(worker) = handle_actor.handle() else {
        let _ = initial_sender.send(Err(continuation_database_error(ActorError::OwnerMismatch)));
        return;
    };
    network_run_primary_worker(
        worker,
        actor,
        sessions,
        sql,
        session_owner,
        lease,
        Some(settlement_guard),
        continuation_sender,
        continuation_receiver,
        initial_sender,
        cancel_rx,
        run_owner,
        true,
    )
    .await;
}

type PrimaryInitialSender = tokio::sync::oneshot::Sender<Result<P6UnitOutcome, DatabaseError>>;

fn continuation_database_error(error: ActorError) -> DatabaseError {
    DatabaseError {
        engine: DatabaseErrorEngine::Yuzora,
        message: "result continuation is unavailable".to_string(),
        code: Some("resultContinuation".to_string()),
        position: None,
        detail: Some(format!("{error:?}")),
        hint: None,
        retryability: Retryability::NotRetryable,
    }
}

fn primary_rows_outcome(
    result_session: ResultSession,
    effect_outcome: EffectOutcome,
    result_limit_reached: bool,
    affected_rows: Option<String>,
) -> P6UnitOutcome {
    P6UnitOutcome {
        result: if result_limit_reached {
            StatementExecutionResult::ResultLimitReached {
                result_session,
                affected_rows,
            }
        } else {
            StatementExecutionResult::Rows {
                result_session: Some(result_session),
                affected_rows,
            }
        },
        effect_outcome,
        stop: result_limit_reached,
        connection_terminated: false,
    }
}

fn settle_primary_guard(
    guard: &mut Option<ExecutionSettlementGuard>,
) -> Result<crate::db_connection_actor::Settlement, ActorError> {
    settle_primary_guard_with_policy(guard, false)
}

fn settle_primary_guard_with_policy(
    guard: &mut Option<ExecutionSettlementGuard>,
    terminate_if_cancelled: bool,
) -> Result<crate::db_connection_actor::Settlement, ActorError> {
    guard
        .take()
        .expect("primary worker settlement guard is armed")
        .settle_with_policy(terminate_if_cancelled)
}

fn settle_network_primary_completion(
    guard: &mut Option<ExecutionSettlementGuard>,
    worker: &NetworkQueryWorker,
    terminate_if_cancelled: bool,
) -> Result<crate::db_connection_actor::Settlement, ActorError> {
    let settlement = settle_primary_guard_with_policy(guard, terminate_if_cancelled)?;
    if settlement.connection_termination_required {
        terminate_network_worker(worker);
    }
    Ok(settlement)
}

fn sqlite_run_primary_worker(
    actor: Arc<ProductionConnectionActor>,
    sessions: ResultSessionState,
    sql: String,
    session_owner: ResultSessionOwner,
    lease: ExecutionLease,
    settlement_guard: ExecutionSettlementGuard,
    continuation_sender: tokio::sync::mpsc::UnboundedSender<ResultContinuationCommand>,
    mut continuation_receiver: tokio::sync::mpsc::UnboundedReceiver<ResultContinuationCommand>,
    initial_sender: PrimaryInitialSender,
) {
    let mut settlement_guard = Some(settlement_guard);
    let DbHandle::Sqlite(connection) = actor.handle() else {
        let _ = initial_sender.send(Err(continuation_database_error(ActorError::OwnerMismatch)));
        return;
    };
    let connection = match connection.lock() {
        Ok(connection) => connection,
        Err(_) => {
            let _ = initial_sender.send(Err(sqlite_worker_error("SQLite connection lock failed")));
            return;
        }
    };
    let mut probe = SqliteCompletionProbe::install(&connection);
    let mut statement = match connection.prepare(&sql) {
        Ok(statement) => statement,
        Err(error) => {
            let _ = settle_primary_guard(&mut settlement_guard);
            let _ = initial_sender.send(Err(sqlite_database_error(&error)));
            return;
        }
    };
    let read_only = statement.readonly();
    let column_count = statement.column_count();
    if column_count == 0 {
        let result = statement
            .execute([])
            .map_err(|error| sqlite_database_error(&error));
        let effect_outcome = probe.take().map_or_else(
            || {
                if !connection.is_autocommit() {
                    EffectOutcome::TransactionPending
                } else if read_only {
                    EffectOutcome::None
                } else {
                    EffectOutcome::Unknown
                }
            },
            |probe| effect_outcome_from_completion(probe.finish(read_only, result.is_ok())),
        );
        let _ = settle_primary_guard(&mut settlement_guard);
        let outcome = result.map(|affected| P6UnitOutcome {
            result: StatementExecutionResult::Execute {
                affected_rows: Some(affected.to_string()),
            },
            effect_outcome,
            stop: false,
            connection_terminated: false,
        });
        let _ = initial_sender.send(outcome);
        return;
    }

    let columns = statement
        .column_names()
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
    if let Err(error) = sessions
        .lock()
        .map_err(result_session_database_error)
        .and_then(|mut sessions| {
            sessions
                .begin_session(session_owner.clone(), columns)
                .map_err(result_session_database_error)
        })
    {
        let _ = settle_primary_guard(&mut settlement_guard);
        let _ = initial_sender.send(Err(error));
        return;
    }
    let mut rows = match statement.query([]) {
        Ok(rows) => rows,
        Err(error) => {
            if let Ok(mut sessions) = sessions.lock() {
                let _ = sessions.discard(&session_owner);
            }
            let _ = settle_primary_guard(&mut settlement_guard);
            let _ = initial_sender.send(Err(sqlite_database_error(&error)));
            return;
        }
    };

    let mut read_page = |mut cached_rows: usize| -> Result<PrimaryPageRead, DatabaseError> {
        while cached_rows < RESULT_PAGE_ROWS {
            let Some(row) = rows.next().map_err(|error| sqlite_database_error(&error))? else {
                return Ok(PrimaryPageRead::End);
            };
            if matches!(
                sqlite_push_decoded_row(&connection, row, column_count, &sessions, &session_owner,)?,
                PushRowOutcome::LimitReached | PushRowOutcome::ValueTooLarge
            ) {
                return Ok(PrimaryPageRead::LimitReached);
            }
            cached_rows += 1;
        }

        let Some(row) = rows.next().map_err(|error| sqlite_database_error(&error))? else {
            return Ok(PrimaryPageRead::End);
        };
        if matches!(
            sqlite_push_decoded_row(&connection, row, column_count, &sessions, &session_owner)?,
            PushRowOutcome::LimitReached | PushRowOutcome::ValueTooLarge
        ) {
            Ok(PrimaryPageRead::LimitReached)
        } else {
            Ok(PrimaryPageRead::Streaming)
        }
    };

    let initial_read = read_page(0);
    match initial_read {
        Ok(PrimaryPageRead::Streaming) => {
            let session = sessions
                .lock()
                .map_err(result_session_database_error)
                .and_then(|mut sessions| {
                    sessions
                        .mark_page_ready(&session_owner, 0)
                        .map_err(result_session_database_error)?;
                    sessions
                        .result_session(&session_owner)
                        .map_err(result_session_database_error)
                });
            if let Err(error) = actor.install_result_continuation(
                &lease,
                session_owner.clone(),
                continuation_sender,
            ) {
                if let Ok(mut sessions) = sessions.lock() {
                    let _ = sessions.discard(&session_owner);
                }
                let _ = settle_primary_guard(&mut settlement_guard);
                let _ = initial_sender.send(Err(continuation_database_error(error)));
                return;
            }
            match session {
                Ok(session) => {
                    let initial_effect = if read_only {
                        EffectOutcome::None
                    } else {
                        EffectOutcome::Unknown
                    };
                    if initial_sender
                        .send(Ok(primary_rows_outcome(
                            session,
                            initial_effect,
                            false,
                            None,
                        )))
                        .is_err()
                    {
                        if let Ok(mut sessions) = sessions.lock() {
                            let _ = sessions.discard(&session_owner);
                        }
                        return;
                    }
                }
                Err(error) => {
                    let _ = settle_primary_guard(&mut settlement_guard);
                    let _ = initial_sender.send(Err(error));
                    return;
                }
            }
        }
        Ok(terminal) => {
            drop(read_page);
            drop(rows);
            let completed = terminal == PrimaryPageRead::End;
            let effect_outcome = probe.take().map_or_else(
                || {
                    if !connection.is_autocommit() {
                        EffectOutcome::TransactionPending
                    } else if read_only {
                        EffectOutcome::None
                    } else {
                        EffectOutcome::Unknown
                    }
                },
                |probe| effect_outcome_from_completion(probe.finish(read_only, completed)),
            );
            let result_session = sessions
                .lock()
                .map_err(result_session_database_error)
                .and_then(|mut sessions| {
                    sessions
                        .finish_session(&session_owner, effect_outcome)
                        .map_err(result_session_database_error)
                });
            let affected_rows = (!read_only && completed).then(|| connection.changes().to_string());
            let _ = settle_primary_guard(&mut settlement_guard);
            let outcome = result_session.map(|session| {
                primary_rows_outcome(
                    session,
                    effect_outcome,
                    terminal == PrimaryPageRead::LimitReached,
                    affected_rows,
                )
            });
            let _ = initial_sender.send(outcome);
            return;
        }
        Err(error) => {
            drop(read_page);
            drop(rows);
            let cancelled = actor.cancel_requested(&lease).unwrap_or(false);
            if let Ok(mut sessions) = sessions.lock() {
                let _ = sessions.discard(&session_owner);
            }
            let _ = settle_primary_guard(&mut settlement_guard);
            let outcome = if cancelled {
                Ok(P6UnitOutcome {
                    result: StatementExecutionResult::Cancelled { error },
                    effect_outcome: EffectOutcome::Unknown,
                    stop: true,
                    connection_terminated: false,
                })
            } else {
                Err(error)
            };
            let _ = initial_sender.send(outcome);
            return;
        }
    }

    loop {
        match continuation_receiver.blocking_recv() {
            Some(ResultContinuationCommand::Next { respond_to }) => {
                let page_index = match sessions
                    .lock()
                    .map_err(result_session_database_error)
                    .and_then(|mut sessions| {
                        match sessions
                            .next(&session_owner)
                            .map_err(result_session_database_error)?
                        {
                            NextPage::Continue { page_index } => Ok(page_index),
                            NextPage::Cached(_) => {
                                Err(continuation_database_error(ActorError::ConnectionBusy))
                            }
                        }
                    }) {
                    Ok(page_index) => page_index,
                    Err(_) => {
                        let _ = respond_to.send(ResultContinuationAck {
                            outcome: ResultContinuationOutcome::Error,
                        });
                        continue;
                    }
                };
                match read_page(1) {
                    Ok(PrimaryPageRead::Streaming) => {
                        let ready = sessions
                            .lock()
                            .map_err(result_session_database_error)
                            .and_then(|mut sessions| {
                                sessions
                                    .mark_page_ready(&session_owner, page_index)
                                    .map_err(result_session_database_error)
                            });
                        let outcome = if ready.is_ok() {
                            ResultContinuationOutcome::PageReady
                        } else {
                            ResultContinuationOutcome::Error
                        };
                        let _ = respond_to.send(ResultContinuationAck { outcome });
                    }
                    Ok(terminal) => {
                        drop(read_page);
                        drop(rows);
                        let completed = terminal == PrimaryPageRead::End;
                        let effect_outcome = probe.take().map_or_else(
                            || {
                                if !connection.is_autocommit() {
                                    EffectOutcome::TransactionPending
                                } else if read_only {
                                    EffectOutcome::None
                                } else {
                                    EffectOutcome::Unknown
                                }
                            },
                            |probe| {
                                effect_outcome_from_completion(probe.finish(read_only, completed))
                            },
                        );
                        let _ = sessions.lock().map(|mut sessions| {
                            let _ = sessions.finish_session(&session_owner, effect_outcome);
                        });
                        let _ = settle_primary_guard(&mut settlement_guard);
                        let _ = respond_to.send(ResultContinuationAck {
                            outcome: if terminal == PrimaryPageRead::LimitReached {
                                ResultContinuationOutcome::LimitReached
                            } else {
                                ResultContinuationOutcome::End
                            },
                        });
                        return;
                    }
                    Err(_) => {
                        drop(read_page);
                        drop(rows);
                        let cancelled = actor.cancel_requested(&lease).unwrap_or(false);
                        let lifecycle = if cancelled {
                            ResultSessionLifecycle::Cancelled
                        } else {
                            ResultSessionLifecycle::Error
                        };
                        let _ = sessions.lock().map(|mut sessions| {
                            let _ = sessions.finish_session_with_lifecycle(
                                &session_owner,
                                EffectOutcome::Unknown,
                                lifecycle,
                            );
                        });
                        let _ = settle_primary_guard(&mut settlement_guard);
                        let _ = respond_to.send(ResultContinuationAck {
                            outcome: if cancelled {
                                ResultContinuationOutcome::Cancelled
                            } else {
                                ResultContinuationOutcome::Error
                            },
                        });
                        return;
                    }
                }
            }
            Some(ResultContinuationCommand::Release { respond_to }) => {
                drop(read_page);
                drop(rows);
                let effect_outcome = probe.take().map_or(EffectOutcome::Unknown, |probe| {
                    effect_outcome_from_completion(probe.finish(read_only, false))
                });
                let _ = sessions.lock().map(|mut sessions| {
                    let _ = sessions.release_with_effect(&session_owner, effect_outcome);
                });
                let _ = settle_primary_guard(&mut settlement_guard);
                let _ = respond_to.send(ResultContinuationAck {
                    outcome: ResultContinuationOutcome::Released,
                });
                return;
            }
            Some(ResultContinuationCommand::Cancel) => {
                drop(read_page);
                drop(rows);
                let _ = sessions.lock().map(|mut sessions| {
                    let _ = sessions.finish_session_with_lifecycle(
                        &session_owner,
                        EffectOutcome::Unknown,
                        ResultSessionLifecycle::Cancelled,
                    );
                });
                let _ = settle_primary_guard(&mut settlement_guard);
                return;
            }
            None => {
                drop(read_page);
                drop(rows);
                if let Ok(mut sessions) = sessions.lock() {
                    let _ = sessions.discard(&session_owner);
                }
                return;
            }
        }
    }
}

async fn network_read_primary_page(
    worker: &NetworkQueryWorker,
    sessions: &ResultSessionState,
    session_owner: &ResultSessionOwner,
    mut cached_rows: usize,
) -> Result<PrimaryPageRead, DatabaseError> {
    while cached_rows < RESULT_PAGE_ROWS {
        match worker.next_row().await? {
            NetworkRow::Value(values) => {
                if matches!(
                    sessions
                        .lock()
                        .map_err(result_session_database_error)?
                        .push_row(session_owner, values)
                        .map_err(result_session_database_error)?,
                    PushRowOutcome::LimitReached | PushRowOutcome::ValueTooLarge
                ) {
                    let _ = worker.stop_streaming().await;
                    drain_helper_stream(worker).await;
                    return Ok(PrimaryPageRead::LimitReached);
                }
                cached_rows += 1;
            }
            NetworkRow::End { .. } => return Ok(PrimaryPageRead::End),
            NetworkRow::ValueTooLarge => {
                sessions
                    .lock()
                    .map_err(result_session_database_error)?
                    .mark_value_too_large(session_owner)
                    .map_err(result_session_database_error)?;
                return Ok(PrimaryPageRead::ValueTooLarge);
            }
            NetworkRow::Cancelled => return Err(cancelled_error()),
        }
    }
    match worker.next_row().await? {
        NetworkRow::Value(values) => {
            if matches!(
                sessions
                    .lock()
                    .map_err(result_session_database_error)?
                    .push_row(session_owner, values)
                    .map_err(result_session_database_error)?,
                PushRowOutcome::LimitReached | PushRowOutcome::ValueTooLarge
            ) {
                let _ = worker.stop_streaming().await;
                drain_helper_stream(worker).await;
                Ok(PrimaryPageRead::LimitReached)
            } else {
                Ok(PrimaryPageRead::Streaming)
            }
        }
        NetworkRow::End { .. } => Ok(PrimaryPageRead::End),
        NetworkRow::ValueTooLarge => {
            sessions
                .lock()
                .map_err(result_session_database_error)?
                .mark_value_too_large(session_owner)
                .map_err(result_session_database_error)?;
            Ok(PrimaryPageRead::ValueTooLarge)
        }
        NetworkRow::Cancelled => Err(cancelled_error()),
    }
}

async fn cancellable_network_read_primary_page(
    worker: &NetworkQueryWorker,
    sessions: &ResultSessionState,
    session_owner: &ResultSessionOwner,
    cached_rows: usize,
    cancel_rx: &mut tokio::sync::mpsc::UnboundedReceiver<QueryRunOwner>,
    run_owner: &QueryRunOwner,
    terminate_on_cancel: bool,
) -> CancellablePrimaryPageRead {
    let read_result = {
        let read = network_read_primary_page(worker, sessions, session_owner, cached_rows);
        tokio::pin!(read);
        loop {
            tokio::select! {
                biased;
                request = cancel_rx.recv() => {
                    match request {
                        Some(request) if request == *run_owner => break None,
                        Some(_) => continue,
                        None => break Some(read.as_mut().await),
                    }
                }
                result = &mut read => break Some(result),
            }
        }
    };
    match read_result {
        Some(result) => CancellablePrimaryPageRead::Read(result),
        None => {
            // Drop the pending page read before draining cancellation output so
            // it cannot retain the worker's single stdout lock.
            let connection_terminated =
                settle_network_stream_cancel(worker, terminate_on_cancel).await;
            CancellablePrimaryPageRead::Cancelled {
                connection_terminated,
            }
        }
    }
}

fn mark_next_network_page_ready(
    sessions: &ResultSessionState,
    session_owner: &ResultSessionOwner,
) -> Result<(), DatabaseError> {
    let mut sessions = sessions.lock().map_err(result_session_database_error)?;
    let page_index = sessions
        .next(session_owner)
        .ok()
        .and_then(|next| match next {
            crate::db_result_session::NextPage::Continue { page_index } => Some(page_index),
            _ => None,
        })
        .unwrap_or(1);
    sessions
        .mark_page_ready(session_owner, page_index)
        .map(|_| ())
        .map_err(result_session_database_error)
}

async fn network_run_primary_worker(
    worker: &NetworkQueryWorker,
    actor: Arc<ProductionConnectionActor>,
    sessions: ResultSessionState,
    sql: String,
    session_owner: ResultSessionOwner,
    lease: ExecutionLease,
    mut settlement_guard: Option<ExecutionSettlementGuard>,
    continuation_sender: tokio::sync::mpsc::UnboundedSender<ResultContinuationCommand>,
    mut continuation_receiver: tokio::sync::mpsc::UnboundedReceiver<ResultContinuationCommand>,
    initial_sender: PrimaryInitialSender,
    mut cancel_rx: tokio::sync::mpsc::UnboundedReceiver<QueryRunOwner>,
    run_owner: QueryRunOwner,
    terminate_on_cancel: bool,
) {
    // Keep the start future pinned while racing the exact-owner cancel channel.
    // Recreating it after a cancel wake could send the same SQL more than once.
    let start_query = worker.start_query(&sql);
    tokio::pin!(start_query);
    let start_result = loop {
        tokio::select! {
            biased;
            started = &mut start_query => break started,
            request = cancel_rx.recv() => {
                let Some(request) = request else {
                    break start_query.as_mut().await;
                };
                if request != run_owner {
                    continue;
                }
                let (error, connection_terminated) = if terminate_on_cancel {
                    let _ = worker.cancel_query().await;
                    (network_primary_cancel_error(true), true)
                } else {
                    // ProductionConnectionActor already dispatched PostgreSQL's
                    // protocol CancelToken before notifying this channel. Await
                    // the in-flight start response instead of queuing a second
                    // helper CancelQuery that could cancel the next query.
                    match start_query.as_mut().await {
                        Err(error) => {
                            let terminate = !helper_confirmed_cancel(&error);
                            (error, terminate)
                        }
                        Ok(_) => (cancelled_error(), true),
                    }
                };
                if connection_terminated {
                    terminate_network_worker(worker);
                }
                let _ = settle_network_primary_completion(
                    &mut settlement_guard,
                    worker,
                    connection_terminated,
                );
                let _ = initial_sender.send(Ok(P6UnitOutcome {
                    result: StatementExecutionResult::Cancelled { error },
                    effect_outcome: EffectOutcome::Unknown,
                    stop: true,
                    connection_terminated,
                }));
                return;
            }
        }
    };
    let started = match start_result {
        Ok(started) => started,
        Err(error) => {
            let helper_confirmed = helper_confirmed_cancel(&error);
            let settlement = match settle_network_primary_completion(
                &mut settlement_guard,
                worker,
                terminate_on_cancel || !helper_confirmed,
            ) {
                Ok(settlement) => settlement,
                Err(actor_error) => {
                    terminate_network_worker(worker);
                    let _ = initial_sender.send(Err(continuation_database_error(actor_error)));
                    return;
                }
            };
            let cancelled = settlement.cancel_requested;
            let connection_terminated = settlement.connection_termination_required;
            let result = if cancelled {
                Ok(P6UnitOutcome {
                    result: StatementExecutionResult::Cancelled {
                        error: if terminate_on_cancel {
                            network_primary_cancel_error(true)
                        } else {
                            error
                        },
                    },
                    effect_outcome: EffectOutcome::Unknown,
                    stop: true,
                    connection_terminated,
                })
            } else {
                Err(error)
            };
            let _ = initial_sender.send(result);
            return;
        }
    };
    match started {
        NetworkQueryStart::Execute { affected_rows } => {
            let settlement =
                match settle_network_primary_completion(&mut settlement_guard, worker, true) {
                    Ok(settlement) => settlement,
                    Err(actor_error) => {
                        terminate_network_worker(worker);
                        let _ = initial_sender.send(Err(continuation_database_error(actor_error)));
                        return;
                    }
                };
            let cancelled = settlement.cancel_requested;
            // Completion and cancellation were arbitrated under the actor
            // mutex, so a late helper CancelQuery cannot escape to Run B.
            let result = if cancelled {
                Ok(P6UnitOutcome {
                    result: StatementExecutionResult::Cancelled {
                        error: network_primary_cancel_error(terminate_on_cancel),
                    },
                    effect_outcome: EffectOutcome::Unknown,
                    stop: true,
                    connection_terminated: true,
                })
            } else {
                Ok(P6UnitOutcome {
                    result: StatementExecutionResult::Execute { affected_rows },
                    effect_outcome: EffectOutcome::Unknown,
                    stop: false,
                    connection_terminated: false,
                })
            };
            let _ = initial_sender.send(result);
            return;
        }
        NetworkQueryStart::Rows { columns } => {
            if let Err(error) = sessions
                .lock()
                .map_err(result_session_database_error)
                .and_then(|mut sessions| {
                    sessions
                        .begin_session(session_owner.clone(), columns)
                        .map_err(result_session_database_error)
                })
            {
                let _ = worker.stop_streaming().await;
                drain_helper_stream(worker).await;
                let settlement =
                    settle_network_primary_completion(&mut settlement_guard, worker, true);
                let result = match settlement {
                    Ok(settlement) if settlement.cancel_requested => Ok(P6UnitOutcome {
                        result: StatementExecutionResult::Cancelled {
                            error: network_primary_cancel_error(terminate_on_cancel),
                        },
                        effect_outcome: EffectOutcome::Unknown,
                        stop: true,
                        connection_terminated: settlement.connection_termination_required,
                    }),
                    Ok(_) => Err(error),
                    Err(actor_error) => Err(continuation_database_error(actor_error)),
                };
                let _ = initial_sender.send(result);
                return;
            }
            let initial = cancellable_network_read_primary_page(
                worker,
                &sessions,
                &session_owner,
                0,
                &mut cancel_rx,
                &run_owner,
                terminate_on_cancel,
            )
            .await;
            match initial {
                CancellablePrimaryPageRead::Read(Ok(PrimaryPageRead::Streaming)) => {
                    let session = sessions
                        .lock()
                        .map_err(result_session_database_error)
                        .and_then(|mut sessions| {
                            sessions
                                .mark_page_ready(&session_owner, 0)
                                .map_err(result_session_database_error)?;
                            sessions
                                .result_session(&session_owner)
                                .map_err(result_session_database_error)
                        });
                    if let Err(error) = actor.install_result_continuation(
                        &lease,
                        session_owner.clone(),
                        continuation_sender,
                    ) {
                        if let Ok(mut sessions) = sessions.lock() {
                            let _ = sessions.discard(&session_owner);
                        }
                        let _ = worker.stop_streaming().await;
                        drain_helper_stream(worker).await;
                        let settlement =
                            settle_network_primary_completion(&mut settlement_guard, worker, true);
                        let result = match settlement {
                            Ok(settlement) if settlement.cancel_requested => Ok(P6UnitOutcome {
                                result: StatementExecutionResult::Cancelled {
                                    error: network_primary_cancel_error(terminate_on_cancel),
                                },
                                effect_outcome: EffectOutcome::Unknown,
                                stop: true,
                                connection_terminated: settlement.connection_termination_required,
                            }),
                            Ok(_) => Err(continuation_database_error(error)),
                            Err(actor_error) => Err(continuation_database_error(actor_error)),
                        };
                        let _ = initial_sender.send(result);
                        return;
                    }
                    match session {
                        Ok(session) => {
                            if initial_sender
                                .send(Ok(primary_rows_outcome(
                                    session,
                                    EffectOutcome::Unknown,
                                    false,
                                    None,
                                )))
                                .is_err()
                            {
                                if let Ok(mut sessions) = sessions.lock() {
                                    let _ = sessions.discard(&session_owner);
                                }
                                let _ = worker.stop_streaming().await;
                                drain_helper_stream(worker).await;
                                let _ = settle_network_primary_completion(
                                    &mut settlement_guard,
                                    worker,
                                    true,
                                );
                                return;
                            }
                        }
                        Err(error) => {
                            let _ = worker.stop_streaming().await;
                            drain_helper_stream(worker).await;
                            let settlement = settle_network_primary_completion(
                                &mut settlement_guard,
                                worker,
                                true,
                            );
                            let result = match settlement {
                                Ok(settlement) if settlement.cancel_requested => {
                                    Ok(P6UnitOutcome {
                                        result: StatementExecutionResult::Cancelled {
                                            error: network_primary_cancel_error(
                                                terminate_on_cancel,
                                            ),
                                        },
                                        effect_outcome: EffectOutcome::Unknown,
                                        stop: true,
                                        connection_terminated: settlement
                                            .connection_termination_required,
                                    })
                                }
                                Ok(_) => Err(error),
                                Err(actor_error) => Err(continuation_database_error(actor_error)),
                            };
                            let _ = initial_sender.send(result);
                            return;
                        }
                    }
                }
                CancellablePrimaryPageRead::Read(Ok(terminal)) => {
                    let settlement = match settle_network_primary_completion(
                        &mut settlement_guard,
                        worker,
                        true,
                    ) {
                        Ok(settlement) => settlement,
                        Err(error) => {
                            if let Ok(mut sessions) = sessions.lock() {
                                let _ = sessions.discard(&session_owner);
                            }
                            let _ = initial_sender.send(Err(continuation_database_error(error)));
                            return;
                        }
                    };
                    if settlement.cancel_requested {
                        if let Ok(mut sessions) = sessions.lock() {
                            let _ = sessions.finish_session_with_lifecycle(
                                &session_owner,
                                EffectOutcome::Unknown,
                                ResultSessionLifecycle::Cancelled,
                            );
                        }
                        let _ = initial_sender.send(Ok(P6UnitOutcome {
                            result: StatementExecutionResult::Cancelled {
                                error: network_primary_cancel_error(terminate_on_cancel),
                            },
                            effect_outcome: EffectOutcome::Unknown,
                            stop: true,
                            connection_terminated: settlement.connection_termination_required,
                        }));
                        return;
                    }
                    let result_session = sessions
                        .lock()
                        .map_err(result_session_database_error)
                        .and_then(|mut sessions| {
                            sessions
                                .finish_session(&session_owner, EffectOutcome::Unknown)
                                .map_err(result_session_database_error)
                        });
                    match result_session {
                        Ok(session) => {
                            let _ = initial_sender.send(Ok(primary_rows_outcome(
                                session,
                                EffectOutcome::Unknown,
                                matches!(
                                    terminal,
                                    PrimaryPageRead::LimitReached | PrimaryPageRead::ValueTooLarge
                                ),
                                None,
                            )));
                        }
                        Err(error) => {
                            let _ = initial_sender.send(Err(error));
                        }
                    }
                    return;
                }
                CancellablePrimaryPageRead::Cancelled {
                    connection_terminated,
                } => {
                    let settlement = settle_network_primary_completion(
                        &mut settlement_guard,
                        worker,
                        connection_terminated,
                    )
                    .ok();
                    if let Ok(mut sessions) = sessions.lock() {
                        let _ = sessions.finish_session_with_lifecycle(
                            &session_owner,
                            EffectOutcome::Unknown,
                            ResultSessionLifecycle::Cancelled,
                        );
                    }
                    let _ = initial_sender.send(Ok(P6UnitOutcome {
                        result: StatementExecutionResult::Cancelled {
                            error: network_primary_cancel_error(terminate_on_cancel),
                        },
                        effect_outcome: EffectOutcome::Unknown,
                        stop: true,
                        connection_terminated: settlement
                            .is_none_or(|settlement| settlement.connection_termination_required),
                    }));
                    return;
                }
                CancellablePrimaryPageRead::Read(Err(error)) => {
                    if let Ok(mut sessions) = sessions.lock() {
                        let _ = sessions.discard(&session_owner);
                    }
                    let helper_confirmed = helper_confirmed_cancel(&error);
                    let settlement = match settle_network_primary_completion(
                        &mut settlement_guard,
                        worker,
                        terminate_on_cancel || !helper_confirmed,
                    ) {
                        Ok(settlement) => settlement,
                        Err(actor_error) => {
                            let _ =
                                initial_sender.send(Err(continuation_database_error(actor_error)));
                            return;
                        }
                    };
                    let cancelled = settlement.cancel_requested;
                    let connection_terminated = settlement.connection_termination_required;
                    let result = if cancelled {
                        Ok(P6UnitOutcome {
                            result: StatementExecutionResult::Cancelled {
                                error: if terminate_on_cancel {
                                    network_primary_cancel_error(true)
                                } else {
                                    error
                                },
                            },
                            effect_outcome: EffectOutcome::Unknown,
                            stop: true,
                            connection_terminated,
                        })
                    } else {
                        Err(error)
                    };
                    let _ = initial_sender.send(result);
                    return;
                }
            }
        }
    }

    loop {
        let command = tokio::select! {
            request = cancel_rx.recv() => {
                if request.as_ref() == Some(&run_owner) {
                    let connection_terminated =
                        settle_network_stream_cancel(worker, terminate_on_cancel).await;
                    let _ = settle_network_primary_completion(
                        &mut settlement_guard,
                        worker,
                        connection_terminated,
                    );
                    if let Ok(mut sessions) = sessions.lock() {
                        let _ = sessions.finish_session_with_lifecycle(
                            &session_owner,
                            EffectOutcome::Unknown,
                            ResultSessionLifecycle::Cancelled,
                        );
                    }
                    return;
                }
                continue;
            }
            command = continuation_receiver.recv() => command,
        };
        match command {
            Some(ResultContinuationCommand::Next { respond_to }) => {
                let read = cancellable_network_read_primary_page(
                    worker,
                    &sessions,
                    &session_owner,
                    0,
                    &mut cancel_rx,
                    &run_owner,
                    terminate_on_cancel,
                )
                .await;
                match read {
                    CancellablePrimaryPageRead::Read(Ok(PrimaryPageRead::Streaming)) => {
                        let _ = mark_next_network_page_ready(&sessions, &session_owner);
                        let _ = respond_to.send(ResultContinuationAck {
                            outcome: ResultContinuationOutcome::PageReady,
                        });
                    }
                    CancellablePrimaryPageRead::Read(Ok(PrimaryPageRead::End)) => {
                        let _ = mark_next_network_page_ready(&sessions, &session_owner);
                        let settlement =
                            settle_network_primary_completion(&mut settlement_guard, worker, true);
                        let cancelled = settlement
                            .as_ref()
                            .is_ok_and(|settlement| settlement.cancel_requested);
                        if let Ok(mut sessions) = sessions.lock() {
                            if cancelled {
                                let _ = sessions.finish_session_with_lifecycle(
                                    &session_owner,
                                    EffectOutcome::Unknown,
                                    ResultSessionLifecycle::Cancelled,
                                );
                            } else {
                                let _ =
                                    sessions.finish_session(&session_owner, EffectOutcome::Unknown);
                            }
                        }
                        let _ = respond_to.send(ResultContinuationAck {
                            outcome: if cancelled {
                                ResultContinuationOutcome::Cancelled
                            } else if settlement.is_ok() {
                                ResultContinuationOutcome::End
                            } else {
                                ResultContinuationOutcome::Error
                            },
                        });
                        return;
                    }
                    CancellablePrimaryPageRead::Read(Ok(PrimaryPageRead::LimitReached))
                    | CancellablePrimaryPageRead::Read(Ok(PrimaryPageRead::ValueTooLarge)) => {
                        let settlement =
                            settle_network_primary_completion(&mut settlement_guard, worker, true);
                        let cancelled = settlement
                            .as_ref()
                            .is_ok_and(|settlement| settlement.cancel_requested);
                        if let Ok(mut sessions) = sessions.lock() {
                            if cancelled {
                                let _ = sessions.finish_session_with_lifecycle(
                                    &session_owner,
                                    EffectOutcome::Unknown,
                                    ResultSessionLifecycle::Cancelled,
                                );
                            } else {
                                let _ =
                                    sessions.finish_session(&session_owner, EffectOutcome::Unknown);
                            }
                        }
                        let _ = respond_to.send(ResultContinuationAck {
                            outcome: if cancelled {
                                ResultContinuationOutcome::Cancelled
                            } else if settlement.is_ok() {
                                ResultContinuationOutcome::LimitReached
                            } else {
                                ResultContinuationOutcome::Error
                            },
                        });
                        return;
                    }
                    CancellablePrimaryPageRead::Cancelled {
                        connection_terminated,
                    } => {
                        let _ = settle_network_primary_completion(
                            &mut settlement_guard,
                            worker,
                            connection_terminated,
                        );
                        if let Ok(mut sessions) = sessions.lock() {
                            let _ = sessions.finish_session_with_lifecycle(
                                &session_owner,
                                EffectOutcome::Unknown,
                                ResultSessionLifecycle::Cancelled,
                            );
                        }
                        let _ = respond_to.send(ResultContinuationAck {
                            outcome: ResultContinuationOutcome::Cancelled,
                        });
                        return;
                    }
                    CancellablePrimaryPageRead::Read(Err(error)) => {
                        let helper_confirmed = helper_confirmed_cancel(&error);
                        let settlement = settle_network_primary_completion(
                            &mut settlement_guard,
                            worker,
                            terminate_on_cancel || !helper_confirmed,
                        );
                        let cancelled = settlement
                            .as_ref()
                            .is_ok_and(|settlement| settlement.cancel_requested);
                        if let Ok(mut sessions) = sessions.lock() {
                            let _ = sessions.finish_session_with_lifecycle(
                                &session_owner,
                                EffectOutcome::Unknown,
                                if cancelled {
                                    ResultSessionLifecycle::Cancelled
                                } else {
                                    ResultSessionLifecycle::Error
                                },
                            );
                        }
                        let _ = respond_to.send(ResultContinuationAck {
                            outcome: if cancelled {
                                ResultContinuationOutcome::Cancelled
                            } else {
                                ResultContinuationOutcome::Error
                            },
                        });
                        return;
                    }
                }
            }
            Some(ResultContinuationCommand::Release { respond_to }) => {
                let _ = worker.stop_streaming().await;
                drain_helper_stream(worker).await;
                let settlement =
                    settle_network_primary_completion(&mut settlement_guard, worker, true);
                let cancelled = settlement
                    .as_ref()
                    .is_ok_and(|settlement| settlement.cancel_requested);
                if let Ok(mut sessions) = sessions.lock() {
                    if cancelled {
                        let _ = sessions.finish_session_with_lifecycle(
                            &session_owner,
                            EffectOutcome::Unknown,
                            ResultSessionLifecycle::Cancelled,
                        );
                    } else {
                        let _ =
                            sessions.release_with_effect(&session_owner, EffectOutcome::Unknown);
                    }
                }
                let _ = respond_to.send(ResultContinuationAck {
                    outcome: if cancelled {
                        ResultContinuationOutcome::Cancelled
                    } else if settlement.is_ok() {
                        ResultContinuationOutcome::Released
                    } else {
                        ResultContinuationOutcome::Error
                    },
                });
                return;
            }
            Some(ResultContinuationCommand::Cancel) => {
                let connection_terminated =
                    settle_network_stream_cancel(worker, terminate_on_cancel).await;
                let _ = settle_network_primary_completion(
                    &mut settlement_guard,
                    worker,
                    connection_terminated,
                );
                if let Ok(mut sessions) = sessions.lock() {
                    let _ = sessions.finish_session_with_lifecycle(
                        &session_owner,
                        EffectOutcome::Unknown,
                        ResultSessionLifecycle::Cancelled,
                    );
                }
                return;
            }
            None => {
                let _ = worker.stop_streaming().await;
                drain_helper_stream(worker).await;
                let _ = settle_network_primary_completion(&mut settlement_guard, worker, true);
                if let Ok(mut sessions) = sessions.lock() {
                    let _ = sessions.discard(&session_owner);
                }
                return;
            }
        }
    }
}

async fn pg_run_primary_worker(
    actor: Arc<ProductionConnectionActor>,
    sessions: ResultSessionState,
    sql: String,
    run_owner: QueryRunOwner,
    session_owner: ResultSessionOwner,
    lease: ExecutionLease,
    settlement_guard: ExecutionSettlementGuard,
    continuation_sender: tokio::sync::mpsc::UnboundedSender<ResultContinuationCommand>,
    continuation_receiver: tokio::sync::mpsc::UnboundedReceiver<ResultContinuationCommand>,
    cancel_rx: tokio::sync::mpsc::UnboundedReceiver<QueryRunOwner>,
    initial_sender: PrimaryInitialSender,
) {
    let handle_actor = actor.clone();
    let DbHandle::Postgres(connection) = handle_actor.handle() else {
        let _ = initial_sender.send(Err(continuation_database_error(ActorError::OwnerMismatch)));
        return;
    };
    network_run_primary_worker(
        connection.worker(),
        actor,
        sessions,
        sql,
        session_owner,
        lease,
        Some(settlement_guard),
        continuation_sender,
        continuation_receiver,
        initial_sender,
        cancel_rx,
        run_owner,
        false,
    )
    .await;
}

pub(crate) async fn query_run_in_state(
    state: &DbState,
    sessions: &ResultSessionState,
    request: QueryRunRequest,
) -> Result<QueryRun, DatabaseOperationalError> {
    if request.mode == QueryRunMode::Primary && request.statements.len() != 1 {
        return Err(DatabaseOperationalError::new(
            DatabaseOperationalErrorCode::QueryFailed,
            "primary query must contain exactly one statement",
        ));
    }
    let identity = ConnectionIdentity {
        descriptor_id: request.descriptor_id.clone(),
        connection_id: request.connection_id.clone(),
        connection_generation: request.connection_generation.clone(),
    };
    let owner = QueryRunOwner {
        descriptor_id: request.descriptor_id.clone(),
        connection_id: request.connection_id.clone(),
        connection_generation: request.connection_generation.clone(),
        query_run_id: request.query_run_id.clone(),
    };
    let actor = get_exact_actor(state, &identity)?;
    let capability = match actor.handle() {
        DbHandle::Sqlite(_) => CancelCapability::SqliteInterrupt,
        DbHandle::Postgres(_) => CancelCapability::PostgresProtocolCancel,
        DbHandle::Mssql(_) => CancelCapability::MssqlConnectionTermination,
    };
    let lease = actor
        .acquire_execution(owner.clone(), capability)
        .map_err(actor_error)?;
    let lease_for_status = lease.clone();
    let settlement_guard = ExecutionSettlementGuard::new(actor.clone(), lease);
    let mut mssql_cancel_rx =
        if matches!(actor.handle(), DbHandle::Mssql(_) | DbHandle::Postgres(_),) {
            let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
            actor
                .install_mssql_cancel_channel(&lease_for_status, sender)
                .map_err(actor_error)?;
            Some(receiver)
        } else {
            None
        };
    sessions
        .lock()
        .map_err(|_| {
            DatabaseOperationalError::new(
                DatabaseOperationalErrorCode::QueryFailed,
                "result session state is unavailable",
            )
        })?
        .begin_run(&owner)
        .map_err(|_| {
            DatabaseOperationalError::new(
                DatabaseOperationalErrorCode::StaleConnection,
                "result session owner is stale",
            )
        })?;

    if request.mode == QueryRunMode::Primary {
        let unit = request
            .statements
            .iter()
            .next()
            .expect("primary mode was validated as exactly one statement");
        let statement_execution_id =
            StatementExecutionId(format!("statement-{}", uuid::Uuid::new_v4()));
        let session_owner = ResultSessionOwner {
            descriptor_id: owner.descriptor_id.clone(),
            connection_id: owner.connection_id.clone(),
            connection_generation: owner.connection_generation.clone(),
            query_run_id: owner.query_run_id.clone(),
            statement_execution_id: statement_execution_id.clone(),
            result_session_id: ResultSessionId(format!("result-{}", uuid::Uuid::new_v4())),
        };
        let (continuation_sender, continuation_receiver) = tokio::sync::mpsc::unbounded_channel();
        let (initial_sender, initial_receiver) = tokio::sync::oneshot::channel();
        let worker_actor = actor.clone();
        let worker_sessions = sessions.clone();
        let worker_sql = unit.sql.clone();
        let worker_lease = lease_for_status.clone();
        let worker_owner = owner.clone();
        match actor.handle() {
            DbHandle::Sqlite(_) => {
                tauri::async_runtime::spawn_blocking(move || {
                    sqlite_run_primary_worker(
                        worker_actor,
                        worker_sessions,
                        worker_sql,
                        session_owner,
                        worker_lease,
                        settlement_guard,
                        continuation_sender,
                        continuation_receiver,
                        initial_sender,
                    );
                });
            }
            DbHandle::Postgres(_) => {
                tauri::async_runtime::spawn(pg_run_primary_worker(
                    worker_actor,
                    worker_sessions,
                    worker_sql,
                    worker_owner,
                    session_owner,
                    worker_lease,
                    settlement_guard,
                    continuation_sender,
                    continuation_receiver,
                    mssql_cancel_rx
                        .take()
                        .expect("PostgreSQL execution installs one cancel receiver"),
                    initial_sender,
                ));
            }
            DbHandle::Mssql(_) => {
                tauri::async_runtime::spawn(mssql_run_primary_worker(
                    worker_actor,
                    worker_sessions,
                    worker_sql,
                    worker_owner,
                    session_owner,
                    worker_lease,
                    settlement_guard,
                    continuation_sender,
                    continuation_receiver,
                    mssql_cancel_rx
                        .take()
                        .expect("MSSQL execution installs one cancel receiver"),
                    initial_sender,
                ));
            }
        }
        let outcome = initial_receiver.await.map_err(|_| {
            DatabaseOperationalError::new(
                DatabaseOperationalErrorCode::QueryFailed,
                "primary query worker stopped before returning its initial page",
            )
        })?;
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(error) if error.engine == DatabaseErrorEngine::Yuzora => {
                return Err(DatabaseOperationalError::new(
                    DatabaseOperationalErrorCode::QueryFailed,
                    "database query failed",
                )
                .with_database_error(error));
            }
            Err(error) => P6UnitOutcome {
                result: StatementExecutionResult::Error { error },
                effect_outcome: EffectOutcome::Unknown,
                stop: true,
                connection_terminated: false,
            },
        };
        let mut transaction_may_be_open = false;
        apply_successful_transaction_boundary(
            &mut transaction_may_be_open,
            unit.transaction_boundary,
            &outcome.result,
        );
        let run = QueryRun {
            descriptor_id: owner.descriptor_id,
            connection_id: owner.connection_id,
            connection_generation: owner.connection_generation,
            query_run_id: owner.query_run_id,
            statements: NonEmptyVec::try_from(vec![StatementExecution {
                statement_execution_id,
                statement_index: 0,
                sql: unit.sql.clone(),
                effect_outcome: outcome.effect_outcome,
                result: outcome.result,
            }])
            .expect("primary query always has one statement"),
            transaction_may_be_open,
            connection_terminated: outcome.connection_terminated,
        };
        run.validate_cardinality().map_err(|_| {
            DatabaseOperationalError::new(
                DatabaseOperationalErrorCode::QueryFailed,
                "query run result cardinality is invalid",
            )
        })?;
        return Ok(run);
    }

    let mut statements = Vec::with_capacity(request.statements.len());
    let mut stopped = false;
    let mut transaction_may_be_open = false;
    let mut connection_terminated = false;
    for (statement_index, unit) in request.statements.iter().enumerate() {
        let statement_execution_id =
            StatementExecutionId(format!("statement-{}", uuid::Uuid::new_v4()));
        if stopped {
            statements.push(StatementExecution {
                statement_execution_id,
                statement_index,
                sql: unit.sql.clone(),
                effect_outcome: EffectOutcome::None,
                result: StatementExecutionResult::Skipped,
            });
            continue;
        }
        let session_owner = ResultSessionOwner {
            descriptor_id: owner.descriptor_id.clone(),
            connection_id: owner.connection_id.clone(),
            connection_generation: owner.connection_generation.clone(),
            query_run_id: owner.query_run_id.clone(),
            statement_execution_id: statement_execution_id.clone(),
            result_session_id: ResultSessionId(format!("result-{}", uuid::Uuid::new_v4())),
        };
        let result: Result<P6UnitOutcome, DatabaseError> = match actor.handle() {
            DbHandle::Sqlite(_) => {
                let actor_for_worker = actor.clone();
                let sessions = sessions.clone();
                let sql = unit.sql.clone();
                tauri::async_runtime::spawn_blocking(move || match actor_for_worker.handle() {
                    DbHandle::Sqlite(connection) => {
                        let connection = connection
                            .lock()
                            .map_err(|_| sqlite_worker_error("SQLite connection lock failed"))?;
                        sqlite_run_materialized_unit(&connection, &sql, &sessions, session_owner)
                    }
                    _ => Err(sqlite_worker_error("database connection engine mismatch")),
                })
                .await
                .unwrap_or_else(|_| Err(sqlite_worker_error("SQLite worker task failed")))
                .map(|(result, effect_outcome, stop)| P6UnitOutcome {
                    result,
                    effect_outcome,
                    stop,
                    connection_terminated: false,
                })
            }
            DbHandle::Postgres(connection) => {
                pg_run_materialized_unit(connection, &unit.sql, sessions, session_owner)
                    .await
                    .map(|(result, effect_outcome, stop)| P6UnitOutcome {
                        result,
                        effect_outcome,
                        stop,
                        connection_terminated: false,
                    })
            }
            DbHandle::Mssql(worker) => {
                let outcome = mssql_run_materialized_unit(
                    worker,
                    &unit.sql,
                    sessions,
                    session_owner,
                    &owner,
                    mssql_cancel_rx
                        .as_mut()
                        .expect("MSSQL execution installs one cancel receiver"),
                )
                .await;
                if outcome
                    .as_ref()
                    .is_ok_and(|outcome| outcome.connection_terminated)
                {
                    worker.abort();
                    actor
                        .mark_connection_terminated(&lease_for_status)
                        .map_err(actor_error)?;
                }
                outcome
            }
        };
        match result {
            Ok(outcome) => {
                apply_successful_transaction_boundary(
                    &mut transaction_may_be_open,
                    unit.transaction_boundary,
                    &outcome.result,
                );
                stopped = outcome.stop;
                connection_terminated |= outcome.connection_terminated;
                statements.push(StatementExecution {
                    statement_execution_id,
                    statement_index,
                    sql: unit.sql.clone(),
                    effect_outcome: outcome.effect_outcome,
                    result: outcome.result,
                });
            }
            Err(error) => {
                stopped = true;
                let result = if actor.cancel_requested(&lease_for_status).unwrap_or(false) {
                    StatementExecutionResult::Cancelled { error }
                } else {
                    StatementExecutionResult::Error { error }
                };
                statements.push(StatementExecution {
                    statement_execution_id,
                    statement_index,
                    sql: unit.sql.clone(),
                    effect_outcome: EffectOutcome::Unknown,
                    result,
                });
            }
        }
    }
    settlement_guard.settle().map_err(actor_error)?;
    let run = QueryRun {
        descriptor_id: owner.descriptor_id,
        connection_id: owner.connection_id,
        connection_generation: owner.connection_generation,
        query_run_id: owner.query_run_id,
        statements: NonEmptyVec::try_from(statements).map_err(|_| {
            DatabaseOperationalError::new(
                DatabaseOperationalErrorCode::QueryFailed,
                "query run produced no statement executions",
            )
        })?,
        transaction_may_be_open,
        connection_terminated,
    };
    run.validate_cardinality().map_err(|_| {
        DatabaseOperationalError::new(
            DatabaseOperationalErrorCode::QueryFailed,
            "query run result cardinality is invalid",
        )
    })?;
    Ok(run)
}

#[tauri::command]
pub async fn db_query_run(
    state: tauri::State<'_, DbState>,
    sessions: tauri::State<'_, ResultSessionState>,
    profiles: tauri::State<'_, crate::db_profiles::DatabaseProfileState>,
    request: QueryRunRequest,
) -> Result<QueryRun, DatabaseOperationalError> {
    let identity = ConnectionIdentity {
        descriptor_id: request.descriptor_id.clone(),
        connection_id: request.connection_id.clone(),
        connection_generation: request.connection_generation.clone(),
    };
    let run = query_run_in_state(&state, &sessions, request).await?;
    if run.connection_terminated {
        finalize_terminated_connection(&state, &sessions, &profiles, &identity)?;
    }
    Ok(run)
}

fn classify_cancel_request(
    request: Result<crate::db_connection_actor::CancelRequest, ActorError>,
    connection_terminated: bool,
) -> Result<QueryCancelOutcome, ActorError> {
    match request {
        Err(ActorError::CancelFailed) if connection_terminated => {
            Ok(QueryCancelOutcome::CancelledConnectionTerminated)
        }
        Err(error) => Err(error),
        Ok(crate::db_connection_actor::CancelRequest::AlreadyRequested) => {
            Ok(QueryCancelOutcome::AlreadyRequested)
        }
        Ok(crate::db_connection_actor::CancelRequest::DriverCancellationRequired(_))
            if connection_terminated =>
        {
            Ok(QueryCancelOutcome::CancelledConnectionTerminated)
        }
        Ok(crate::db_connection_actor::CancelRequest::DriverCancellationRequired(_)) => {
            Ok(QueryCancelOutcome::Cancelled)
        }
        Ok(crate::db_connection_actor::CancelRequest::ConnectionTerminationRequired) => {
            Ok(QueryCancelOutcome::CancelledConnectionTerminated)
        }
    }
}

pub(crate) async fn query_cancel_in_state(
    state: &DbState,
    owner: QueryRunOwner,
) -> Result<QueryCancelResult, DatabaseOperationalError> {
    let identity = ConnectionIdentity {
        descriptor_id: owner.descriptor_id.clone(),
        connection_id: owner.connection_id.clone(),
        connection_generation: owner.connection_generation.clone(),
    };
    let actor = get_exact_actor(state, &identity)?;
    let connection_is_terminated = || {
        actor.teardown_report().closed
            || match actor.handle() {
                DbHandle::Postgres(connection) => connection.worker().is_closed(),
                DbHandle::Mssql(worker) => worker.is_closed(),
                DbHandle::Sqlite(_) => false,
            }
    };
    let request = actor.request_cancel(&owner).await;
    let outcome =
        classify_cancel_request(request, connection_is_terminated()).map_err(actor_error)?;
    Ok(QueryCancelResult { outcome })
}

#[tauri::command]
pub async fn db_query_cancel(
    state: tauri::State<'_, DbState>,
    sessions: tauri::State<'_, ResultSessionState>,
    profiles: tauri::State<'_, crate::db_profiles::DatabaseProfileState>,
    owner: QueryRunOwner,
) -> Result<QueryCancelResult, DatabaseOperationalError> {
    let identity = ConnectionIdentity {
        descriptor_id: owner.descriptor_id.clone(),
        connection_id: owner.connection_id.clone(),
        connection_generation: owner.connection_generation.clone(),
    };
    let result = query_cancel_in_state(&state, owner).await?;
    if result.outcome == QueryCancelOutcome::CancelledConnectionTerminated {
        finalize_terminated_connection(&state, &sessions, &profiles, &identity)?;
    }
    Ok(result)
}

fn result_session_operation_error(
    error: SessionError,
    message: &'static str,
) -> DatabaseOperationalError {
    match error {
        SessionError::OwnerMismatch
        | SessionError::SessionNotFound
        | SessionError::PageNotFound => DatabaseOperationalError::new(
            DatabaseOperationalErrorCode::StaleConnection,
            "result session owner is stale",
        ),
        SessionError::SessionAlreadyExists
        | SessionError::BudgetExceeded
        | SessionError::LockUnavailable => {
            DatabaseOperationalError::new(DatabaseOperationalErrorCode::QueryFailed, message)
        }
    }
}

pub(crate) async fn result_page_in_state(
    state: &DbState,
    sessions: &ResultSessionState,
    request: ResultPageRequest,
) -> Result<ResultPage, DatabaseOperationalError> {
    match request.direction {
        ResultPageDirection::Previous => sessions
            .lock()
            .map_err(|error| result_session_operation_error(error, "result page failed"))?
            .previous(&request.owner)
            .map_err(|error| result_session_operation_error(error, "result page failed")),
        ResultPageDirection::Next => {
            let next = sessions
                .lock()
                .map_err(|error| result_session_operation_error(error, "result page failed"))?
                .next(&request.owner)
                .map_err(|error| result_session_operation_error(error, "result page failed"))?;
            match next {
                NextPage::Cached(page) => Ok(page),
                NextPage::Continue { page_index } => {
                    let identity = ConnectionIdentity {
                        descriptor_id: request.owner.descriptor_id.clone(),
                        connection_id: request.owner.connection_id.clone(),
                        connection_generation: request.owner.connection_generation.clone(),
                    };
                    let actor = get_exact_actor(state, &identity)?;
                    let ack = actor
                        .request_result_next(&request.owner)
                        .await
                        .map_err(actor_error)?;
                    if ack.outcome == ResultContinuationOutcome::Released {
                        return Err(DatabaseOperationalError::new(
                            DatabaseOperationalErrorCode::QueryFailed,
                            "released result cannot advance",
                        ));
                    }
                    sessions
                        .lock()
                        .map_err(|error| {
                            result_session_operation_error(error, "result page failed")
                        })?
                        .complete_next(&request.owner, page_index)
                        .map_err(|error| {
                            result_session_operation_error(error, "result page failed")
                        })
                }
            }
        }
    }
}

#[tauri::command]
pub async fn db_result_page(
    state: tauri::State<'_, DbState>,
    sessions: tauri::State<'_, ResultSessionState>,
    request: ResultPageRequest,
) -> Result<ResultPage, DatabaseOperationalError> {
    result_page_in_state(&state, &sessions, request).await
}

pub(crate) async fn result_session_release_in_state(
    state: &DbState,
    sessions: &ResultSessionState,
    owner: ResultSessionOwner,
) -> Result<ResultPage, DatabaseOperationalError> {
    let streaming = sessions
        .lock()
        .map_err(|error| result_session_operation_error(error, "result session release failed"))?
        .is_streaming(&owner)
        .map_err(|error| result_session_operation_error(error, "result session release failed"))?;
    if streaming {
        let identity = ConnectionIdentity {
            descriptor_id: owner.descriptor_id.clone(),
            connection_id: owner.connection_id.clone(),
            connection_generation: owner.connection_generation.clone(),
        };
        let actor = get_exact_actor(state, &identity)?;
        let ack = actor
            .request_result_release(&owner)
            .await
            .map_err(actor_error)?;
        if ack.outcome != ResultContinuationOutcome::Released {
            return Err(DatabaseOperationalError::new(
                DatabaseOperationalErrorCode::QueryFailed,
                "result session release did not settle",
            ));
        }
    }
    let mut sessions = sessions
        .lock()
        .map_err(|error| result_session_operation_error(error, "result session release failed"))?;
    sessions
        .release(&owner)
        .map_err(|error| result_session_operation_error(error, "result session release failed"))?;
    sessions
        .current_page(&owner)
        .map_err(|error| result_session_operation_error(error, "result session release failed"))
}

#[tauri::command]
pub async fn db_result_session_release(
    state: tauri::State<'_, DbState>,
    sessions: tauri::State<'_, ResultSessionState>,
    owner: ResultSessionOwner,
) -> Result<ResultPage, DatabaseOperationalError> {
    result_session_release_in_state(&state, &sessions, owner).await
}

/// Narrow real-driver seam for the ignored database integration matrix.
///
/// This module owns no database behavior. It constructs the same production
/// actor path used by saved profiles and forwards every operation to the
/// production state functions above. Secrets stay write-only inputs to the
/// existing `DbOpenConfig`; that type remains neither `Debug` nor `Serialize`.
#[cfg(debug_assertions)]
#[doc(hidden)]
pub mod integration_harness {
    use super::*;

    fn postgres_open_config_from_legacy_flags(
        host: String,
        port: u16,
        database: String,
        user: String,
        password: String,
        ssl: bool,
        trust_cert: bool,
    ) -> DbOpenConfig {
        let (transport_mode, insecure_exception, trust_server_cert_acknowledged) = if !ssl {
            (
                PostgresTransportMode::InsecurePlaintext,
                Some(PostgresInsecureException::new(
                    host.clone(),
                    port,
                    user.clone(),
                    database.clone(),
                )),
                false,
            )
        } else if trust_cert {
            (PostgresTransportMode::EncryptedTrustServerCert, None, true)
        } else {
            (PostgresTransportMode::VerifyFull, None, false)
        };
        DbOpenConfig::Postgres {
            host,
            port,
            database,
            user,
            password: SecretString::from(password),
            transport_mode,
            insecure_exception,
            trust_server_cert_acknowledged,
        }
    }

    #[derive(Clone, Default)]
    pub struct IntegrationRuntime {
        state: DbState,
        sessions: ResultSessionState,
    }

    #[derive(Clone)]
    pub struct IntegrationConnection {
        runtime: IntegrationRuntime,
        identity: ConnectionIdentity,
    }

    impl IntegrationRuntime {
        async fn open(
            &self,
            descriptor_id: String,
            config: DbOpenConfig,
        ) -> Result<IntegrationConnection, DatabaseOperationalError> {
            let handle = open_unregistered(config).await?;
            let identity = ConnectionIdentity {
                descriptor_id: DescriptorId(descriptor_id),
                connection_id: ConnectionId(next_conn_id()),
                connection_generation: ConnectionGeneration(format!(
                    "integration-{}",
                    uuid::Uuid::new_v4()
                )),
            };
            let actor = Arc::new(ProductionConnectionActor::new(identity.clone(), handle));
            register_actor(&self.state, actor)?;
            Ok(IntegrationConnection {
                runtime: self.clone(),
                identity,
            })
        }

        pub async fn open_sqlite(
            &self,
            descriptor_id: impl Into<String>,
            path: impl Into<String>,
        ) -> Result<IntegrationConnection, DatabaseOperationalError> {
            self.open(
                descriptor_id.into(),
                DbOpenConfig::Sqlite { path: path.into() },
            )
            .await
        }

        #[allow(clippy::too_many_arguments)]
        pub async fn open_postgres(
            &self,
            descriptor_id: impl Into<String>,
            host: impl Into<String>,
            port: u16,
            database: impl Into<String>,
            user: impl Into<String>,
            password: String,
            ssl: bool,
            trust_cert: bool,
        ) -> Result<IntegrationConnection, DatabaseOperationalError> {
            let host = host.into();
            let database = database.into();
            let user = user.into();
            self.open(
                descriptor_id.into(),
                postgres_open_config_from_legacy_flags(
                    host, port, database, user, password, ssl, trust_cert,
                ),
            )
            .await
        }

        #[allow(clippy::too_many_arguments)]
        pub async fn open_mssql(
            &self,
            descriptor_id: impl Into<String>,
            host: impl Into<String>,
            port: u16,
            database: impl Into<String>,
            user: impl Into<String>,
            password: String,
            trust_cert: bool,
        ) -> Result<IntegrationConnection, DatabaseOperationalError> {
            self.open(
                descriptor_id.into(),
                DbOpenConfig::Mssql {
                    host: host.into(),
                    port,
                    database: database.into(),
                    user: user.into(),
                    password: SecretString::from(password),
                    trust_cert,
                },
            )
            .await
        }

        pub async fn test_sqlite(
            &self,
            path: impl Into<String>,
        ) -> Result<Option<String>, DatabaseOperationalError> {
            test_unregistered(DbOpenConfig::Sqlite { path: path.into() }).await
        }

        #[allow(clippy::too_many_arguments)]
        pub async fn test_postgres(
            &self,
            host: impl Into<String>,
            port: u16,
            database: impl Into<String>,
            user: impl Into<String>,
            password: String,
            ssl: bool,
            trust_cert: bool,
        ) -> Result<Option<String>, DatabaseOperationalError> {
            let host = host.into();
            let database = database.into();
            let user = user.into();
            test_unregistered(postgres_open_config_from_legacy_flags(
                host, port, database, user, password, ssl, trust_cert,
            ))
            .await
        }

        #[allow(clippy::too_many_arguments)]
        pub async fn test_mssql(
            &self,
            host: impl Into<String>,
            port: u16,
            database: impl Into<String>,
            user: impl Into<String>,
            password: String,
            trust_cert: bool,
        ) -> Result<Option<String>, DatabaseOperationalError> {
            test_unregistered(DbOpenConfig::Mssql {
                host: host.into(),
                port,
                database: database.into(),
                user: user.into(),
                password: SecretString::from(password),
                trust_cert,
            })
            .await
        }
    }

    impl IntegrationConnection {
        fn finalize_terminated(&self) -> Result<(), DatabaseOperationalError> {
            let removed = match close_exact_in_state(&self.runtime.state, &self.identity) {
                Ok(_) => true,
                Err(error) if error.code == DatabaseOperationalErrorCode::StaleConnection => false,
                Err(error) => return Err(error),
            };
            if removed {
                self.runtime
                    .sessions
                    .lock()
                    .map_err(|error| {
                        result_session_operation_error(error, "result session cleanup failed")
                    })?
                    .release_connection(&self.identity)
                    .map_err(|error| {
                        result_session_operation_error(error, "result session cleanup failed")
                    })?;
            }
            Ok(())
        }

        pub fn identity(&self) -> ConnectionIdentity {
            self.identity.clone()
        }

        pub fn is_registered(&self) -> bool {
            has_exact_actor(&self.runtime.state, &self.identity)
        }

        pub async fn list_tables(&self) -> Result<Vec<TableInfo>, DatabaseOperationalError> {
            list_tables_in_state(&self.runtime.state, self.identity.clone()).await
        }

        pub async fn table_columns(
            &self,
            table: TableInfo,
        ) -> Result<Vec<ColumnInfo>, DatabaseOperationalError> {
            table_columns_in_state(&self.runtime.state, self.identity.clone(), table).await
        }

        pub async fn run_primary(
            &self,
            query_run_id: impl Into<String>,
            sql: impl Into<String>,
        ) -> Result<QueryRun, DatabaseOperationalError> {
            self.run(
                QueryRunId(query_run_id.into()),
                QueryRunMode::Primary,
                vec![QueryExecutionUnit {
                    sql: sql.into(),
                    transaction_boundary: TransactionBoundary::None,
                }],
            )
            .await
        }

        pub async fn run_script(
            &self,
            query_run_id: impl Into<String>,
            statements: Vec<QueryExecutionUnit>,
        ) -> Result<QueryRun, DatabaseOperationalError> {
            self.run(
                QueryRunId(query_run_id.into()),
                QueryRunMode::Script,
                statements,
            )
            .await
        }

        async fn run(
            &self,
            query_run_id: QueryRunId,
            mode: QueryRunMode,
            statements: Vec<QueryExecutionUnit>,
        ) -> Result<QueryRun, DatabaseOperationalError> {
            let statements = NonEmptyVec::try_from(statements).map_err(|_| {
                DatabaseOperationalError::new(
                    DatabaseOperationalErrorCode::QueryFailed,
                    "query run requires at least one statement",
                )
            })?;
            let run = query_run_in_state(
                &self.runtime.state,
                &self.runtime.sessions,
                QueryRunRequest {
                    descriptor_id: self.identity.descriptor_id.clone(),
                    connection_id: self.identity.connection_id.clone(),
                    connection_generation: self.identity.connection_generation.clone(),
                    query_run_id,
                    mode,
                    statements,
                },
            )
            .await?;
            if run.connection_terminated {
                self.finalize_terminated()?;
            }
            Ok(run)
        }

        pub async fn result_page(
            &self,
            owner: ResultSessionOwner,
            direction: ResultPageDirection,
        ) -> Result<ResultPage, DatabaseOperationalError> {
            result_page_in_state(
                &self.runtime.state,
                &self.runtime.sessions,
                ResultPageRequest { owner, direction },
            )
            .await
        }

        pub async fn release_result(
            &self,
            owner: ResultSessionOwner,
        ) -> Result<ResultPage, DatabaseOperationalError> {
            result_session_release_in_state(&self.runtime.state, &self.runtime.sessions, owner)
                .await
        }

        pub async fn cancel(
            &self,
            query_run_id: QueryRunId,
        ) -> Result<QueryCancelResult, DatabaseOperationalError> {
            let result = query_cancel_in_state(
                &self.runtime.state,
                QueryRunOwner {
                    descriptor_id: self.identity.descriptor_id.clone(),
                    connection_id: self.identity.connection_id.clone(),
                    connection_generation: self.identity.connection_generation.clone(),
                    query_run_id,
                },
            )
            .await?;
            if result.outcome == QueryCancelOutcome::CancelledConnectionTerminated {
                self.finalize_terminated()?;
            }
            Ok(result)
        }

        pub fn close(&self) -> Result<TeardownReport, DatabaseOperationalError> {
            let report = close_exact_in_state(&self.runtime.state, &self.identity)?;
            self.runtime
                .sessions
                .lock()
                .map_err(|error| {
                    result_session_operation_error(error, "result session cleanup failed")
                })?
                .release_connection(&self.identity)
                .map_err(|error| {
                    result_session_operation_error(error, "result session cleanup failed")
                })?;
            Ok(report)
        }
    }
}

#[cfg(test)]
mod tests {
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
        let source = include_str!("db_service.rs");
        let production = source
            .split("mod tests {")
            .next()
            .expect("production source before tests");
        assert!(production.contains("network_run_primary_worker("));
        assert!(production.contains("tauri::async_runtime::spawn(pg_run_primary_worker"));
        assert!(production.contains("tauri::async_runtime::spawn(mssql_run_primary_worker"));
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
            .and_then(|source| source.split("pub(crate) async fn query_worker_loop").next())
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
            result_session_id: ResultSessionId(
                "result-00000000-0000-0000-0000-000000000000".into(),
            ),
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
                .execute_batch(
                    "CREATE TABLE hostile(id INTEGER PRIMARY KEY, payload BLOB NOT NULL);",
                )
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
    async fn p6_runner_settles_lease_on_registry_failure_and_aborts_partial_session_on_decode_error(
    ) {
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
        let run = tauri::async_runtime::spawn(async move {
            query_run_in_state(&run_state, &run_sessions, request).await
        });
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
            position: Some(ErrorPosition {
                offset: Some(17),
                line: None,
                column: None,
            }),
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
            position: Some(ErrorPosition {
                offset: Some(9),
                line: None,
                column: None,
            }),
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
                                connection_generation: ConnectionGeneration(
                                    "generation-1".to_string(),
                                ),
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
            session.owner.connection_generation =
                ConnectionGeneration("stale-generation".to_string());
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

        let committed =
            run_query(&conn, "INSERT INTO effects VALUES (1)", DEFAULT_MAX_ROWS).unwrap();
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
        let still_there =
            run_query(&conn, "SELECT count(*) FROM victim", DEFAULT_MAX_ROWS).unwrap();
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
        let invalid_numeric = <PgNumericText as tokio_postgres::types::FromSql>::from_sql(
            &PgType::NUMERIC,
            &[0, 1, 2],
        )
        .map(Some);
        let error = pg_decode_result(3, "numeric", invalid_numeric, |value| DbValue::Decimal {
            value: value.0,
        })
        .expect_err("invalid classified payload must not become DbValue::Null");
        assert_eq!(error.engine, DatabaseErrorEngine::Postgres);
        assert_eq!(error.code.as_deref(), Some("valueDecode"));
        assert!(error.message.contains("column 3"));

        let null =
            pg_decode_result::<i64, &str, _>(4, "int8", Ok(None), |value| DbValue::Integer {
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
        let structured = pg_decode_result(2, "jsonb", bad_version.map(Some), |value| {
            DbValue::Json { value: value.0 }
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

    #[tokio::test]
    async fn pg_open_refused_port_surfaces_os_cause() {
        // 連拒絕的本機埠 → transport 錯誤（as_db_error 為 None，走 source() chain 萃取真因）
        let result = pg_open(
            "127.0.0.1".to_string(),
            1,
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
            address.ip().to_string(),
            address.port(),
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
            address.ip().to_string(),
            address.port(),
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
            address.ip().to_string(),
            address.port(),
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
            address.ip().to_string(),
            address.port(),
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
            address.ip().to_string(),
            address.port(),
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
}
