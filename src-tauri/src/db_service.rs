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
use tokio::sync::Mutex as AsyncMutex;
use tokio_postgres::types::Type as PgType;
use tokio_postgres::Row as PgRow;
use tokio_util::compat::{Compat, TokioAsyncWriteCompatExt};

use crate::db_connection_actor::{
    ActorError, CancelCapability, ExecutionLease, PostgresCancelResource,
    ProductionConnectionActor, ResultContinuationAck, ResultContinuationCommand,
    ResultContinuationOutcome, TeardownReport,
};
use crate::db_result_session::{
    NextPage, PushRowOutcome, ResultSessionState, SessionError, RESULT_PAGE_ROWS,
};

/// The MSSQL client type: tiberius over a tokio TcpStream via the compat shim.
pub(crate) type MssqlClient = tiberius::Client<Compat<TcpStream>>;

/// One PostgreSQL client plus the spawned driver task. tokio-postgres splits a
/// connection into a `Client` (issues queries, `&self`) and a `Connection`
/// future that must be polled to drive the socket; we spawn the latter and abort
/// it on close.
pub struct PgConn {
    client: tokio_postgres::Client,
    conn_task: tauri::async_runtime::JoinHandle<()>,
    cancel: PostgresCancelResource,
}

impl PgConn {
    pub(crate) fn abort_driver(&self) {
        self.conn_task.abort();
    }

    pub(crate) fn cancel_resource(&self) -> &PostgresCancelResource {
        &self.cancel
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
    Mssql(AsyncMutex<Option<MssqlClient>>),
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
        ssl: bool,
        trust_cert: bool,
    },
    Mssql {
        host: String,
        port: u16,
        database: String,
        user: String,
        trust_cert: bool,
    },
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
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileUpdateRequest {
    pub descriptor_id: DescriptorId,
    pub name: String,
    pub target: ProfileTarget,
    pub replacement_credential: Option<CredentialInput>,
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

#[derive(serde::Serialize, Clone, Debug)]
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

    fn with_database_error(mut self, error: DatabaseError) -> Self {
        self.error = Some(error);
        self
    }

    fn connection_failed() -> Self {
        Self::new(
            DatabaseOperationalErrorCode::ConnectionFailed,
            "database connection failed",
        )
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
        ssl: bool,
        #[serde(default)]
        trust_cert: bool,
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

/// 連線失敗的訊息（帶 `cannot connect to postgres:` 前綴，供 log／對話框）。
fn pg_err(e: tokio_postgres::Error) -> String {
    format!("cannot connect to postgres: {}", pg_err_detail(&e))
}

async fn pg_open(
    host: String,
    port: u16,
    database: String,
    user: String,
    password: SecretString,
    ssl: bool,
    trust_cert: bool,
) -> Result<DbHandle, String> {
    let mut cfg = tokio_postgres::Config::new();
    cfg.host(&host)
        .port(port)
        .dbname(&database)
        .user(&user)
        .password(password.expose_secret());

    // The Connection future's concrete type differs per TLS choice, but it is
    // consumed (spawned) inside each branch so both yield the same (Client, task).
    let (client, conn_task, cancel) = if ssl {
        let tls = pg_tls(trust_cert)?;
        let (client, connection) = cfg.connect(tls.clone()).await.map_err(pg_err)?;
        let cancel = PostgresCancelResource::rustls(&client, tls);
        let task = tauri::async_runtime::spawn(async move {
            if let Err(e) = connection.await {
                eprintln!("postgres connection error: {e}");
            }
        });
        (client, task, cancel)
    } else {
        let (client, connection) = cfg.connect(tokio_postgres::NoTls).await.map_err(pg_err)?;
        let cancel = PostgresCancelResource::no_tls(&client);
        let task = tauri::async_runtime::spawn(async move {
            if let Err(e) = connection.await {
                eprintln!("postgres connection error: {e}");
            }
        });
        (client, task, cancel)
    };
    Ok(DbHandle::Postgres(PgConn {
        client,
        conn_task,
        cancel,
    }))
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

#[cfg(test)]
async fn pg_run_query(
    client: &tokio_postgres::Client,
    sql: &str,
    max_rows: usize,
) -> Result<QueryResult, DatabaseError> {
    let stmt = client
        .prepare(sql)
        .await
        .map_err(|error| postgres_database_error(&error))?;
    // A statement with no result columns (INSERT/UPDATE/DDL) reports affected rows.
    if stmt.columns().is_empty() {
        let affected = client
            .execute(&stmt, &[])
            .await
            .map_err(|error| postgres_database_error(&error))?;
        return Ok(QueryResult::Execute {
            affected_rows: Some(affected.to_string()),
            effect_outcome: effect_outcome_from_completion(EngineCompletion::Unknown),
        });
    }
    let columns: Vec<String> = stmt
        .columns()
        .iter()
        .map(|c| c.name().to_string())
        .collect();
    let col_types: Vec<PgType> = stmt.columns().iter().map(|c| c.type_().clone()).collect();

    // Stream rows so an unbounded SELECT stops at the cap instead of buffering the
    // whole table (mirrors the SQLite path). An empty typed params list satisfies
    // query_raw's BorrowToSql/ExactSizeIterator bounds.
    let params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = Vec::new();
    let row_stream = client
        .query_raw(&stmt, params)
        .await
        .map_err(|error| postgres_database_error(&error))?;
    futures_util::pin_mut!(row_stream);
    let mut out: Vec<Vec<DbValue>> = Vec::new();
    let mut truncated = false;
    while let Some(row) = row_stream
        .try_next()
        .await
        .map_err(|error| postgres_database_error(&error))?
    {
        if out.len() >= max_rows {
            truncated = true;
            continue;
        }
        let mut vals = Vec::with_capacity(columns.len());
        for (i, ty) in col_types.iter().enumerate() {
            vals.push(pg_value_to_db_value(&row, i, ty)?);
        }
        out.push(vals);
    }
    let affected_rows = row_stream.rows_affected().map(|rows| rows.to_string());
    Ok(QueryResult::Select {
        columns,
        rows: out,
        truncated,
        affected_rows,
        effect_outcome: effect_outcome_from_completion(EngineCompletion::Unknown),
    })
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
            ssl,
            trust_cert,
        } => {
            let (lh, lu, ld) = (host.clone(), user.clone(), database.clone());
            pg_open(host, port, database, user, password, ssl, trust_cert)
                .await
                .map_err(|e| {
                    crate::logging::write_global(crate::logging::connect_failure_event(
                        "db",
                        &lh,
                        port,
                        &lu,
                        &format!("database={ld}: {e}"),
                    ));
                    DatabaseOperationalError::connection_failed()
                })?
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
            let client = mssql_connect(host, port, database, user, password, trust_cert)
                .await
                .map_err(|_| {
                    crate::logging::write_global(crate::logging::connect_failure_event(
                        "db",
                        &lh,
                        port,
                        &lu,
                        &format!("database={ld}: database connection failed"),
                    ));
                    DatabaseOperationalError::connection_failed()
                })?;
            DbHandle::Mssql(AsyncMutex::new(Some(client)))
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
    if matches!(actor.handle(), DbHandle::Postgres(postgres) if postgres.client.is_closed()) {
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
        DbHandle::Postgres(postgres) => {
            let result = postgres
                .client
                .query_one("SELECT version()", &[])
                .await
                .map(|row| row.get::<_, String>(0))
                .map(Some)
                .map_err(|_| DatabaseOperationalError::connection_failed());
            postgres.conn_task.abort();
            result
        }
        DbHandle::Mssql(client) => {
            let mut client = client.lock().await;
            let client = client
                .as_mut()
                .ok_or_else(DatabaseOperationalError::connection_failed)?;
            match mssql_run_query(client, "SELECT @@VERSION", 1)
                .await
                .map_err(|_| DatabaseOperationalError::connection_failed())?
            {
                QueryResult::Select { rows, .. } => Ok(rows
                    .first()
                    .and_then(|row| row.first())
                    .and_then(|value| match value {
                        DbValue::Text { value } => Some(value.clone()),
                        _ => None,
                    })),
                QueryResult::Execute { .. } => Ok(None),
            }
        }
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
        DbHandle::Postgres(pg) => pg_list_tables(&pg.client).await.map_err(|error| {
            operation_failure_with_database_error(
                &actor,
                DatabaseOperationalErrorCode::MetadataFailed,
                "database metadata request failed",
                error,
            )
        }),
        DbHandle::Mssql(m) => {
            let mut client = m.lock().await;
            match client.as_mut() {
                Some(client) => mssql_list_tables(client).await.map_err(|error| {
                    classify_mssql_live_error(
                        &error,
                        DatabaseOperationalErrorCode::MetadataFailed,
                        "database metadata request failed",
                    )
                }),
                None => Err(DatabaseOperationalError::new(
                    DatabaseOperationalErrorCode::ServerDisconnected,
                    "database server disconnected",
                )),
            }
        }
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
        DbHandle::Postgres(pg) => pg_table_columns(&pg.client, &table).await.map_err(|error| {
            operation_failure_with_database_error(
                &actor,
                DatabaseOperationalErrorCode::MetadataFailed,
                "database metadata request failed",
                error,
            )
        }),
        DbHandle::Mssql(m) => {
            let mut client = m.lock().await;
            match client.as_mut() {
                Some(client) => mssql_table_columns(client, &table).await.map_err(|error| {
                    classify_mssql_live_error(
                        &error,
                        DatabaseOperationalErrorCode::MetadataFailed,
                        "database metadata request failed",
                    )
                }),
                None => Err(DatabaseOperationalError::new(
                    DatabaseOperationalErrorCode::ServerDisconnected,
                    "database server disconnected",
                )),
            }
        }
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
        DbHandle::Postgres(pg) => pg_run_query(&pg.client, &sql, cap).await.map_err(|error| {
            operation_failure_with_database_error(
                &actor,
                DatabaseOperationalErrorCode::QueryFailed,
                "database query failed",
                error,
            )
        }),
        DbHandle::Mssql(m) => {
            let mut client = m.lock().await;
            match client.as_mut() {
                Some(client) => mssql_run_query(client, &sql, cap).await.map_err(|error| {
                    classify_mssql_live_error(
                        &error,
                        DatabaseOperationalErrorCode::QueryFailed,
                        "database query failed",
                    )
                }),
                None => Err(DatabaseOperationalError::new(
                    DatabaseOperationalErrorCode::ServerDisconnected,
                    "database server disconnected",
                )),
            }
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
        let lease = self
            .lease
            .take()
            .expect("execution settlement guard is armed");
        self.actor.settle_execution(&lease)
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
        let values = (0..column_count)
            .map(|index| {
                row.get_ref(index)
                    .map_err(|error| sqlite_database_error(&error))
                    .and_then(value_to_db_value)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let outcome = sessions
            .lock()
            .map_err(result_session_database_error)?
            .push_row(&session_owner, values)
            .map_err(result_session_database_error)?;
        if outcome == PushRowOutcome::LimitReached {
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

async fn pg_run_materialized_unit(
    connection: &PgConn,
    sql: &str,
    sessions: &ResultSessionState,
    session_owner: ResultSessionOwner,
) -> Result<(StatementExecutionResult, EffectOutcome, bool), DatabaseError> {
    let statement = connection
        .client
        .prepare(sql)
        .await
        .map_err(|error| postgres_database_error(&error))?;
    if statement.columns().is_empty() {
        let affected = connection
            .client
            .execute(&statement, &[])
            .await
            .map_err(|error| postgres_database_error(&error))?;
        return Ok((
            StatementExecutionResult::Execute {
                affected_rows: Some(affected.to_string()),
            },
            EffectOutcome::Unknown,
            false,
        ));
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
    sessions
        .lock()
        .map_err(result_session_database_error)?
        .begin_session(session_owner.clone(), columns)
        .map_err(result_session_database_error)?;
    let mut session_guard = SessionAbortGuard::new(sessions.clone(), session_owner.clone());
    let params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = Vec::new();
    let stream = connection
        .client
        .query_raw(&statement, params)
        .await
        .map_err(|error| postgres_database_error(&error))?;
    futures_util::pin_mut!(stream);
    let mut limit_reached = false;
    loop {
        let next = stream.try_next().await;
        let row = match next {
            Ok(Some(row)) => row,
            Ok(None) => break,
            Err(_) if limit_reached => break,
            Err(error) => return Err(postgres_database_error(&error)),
        };
        if limit_reached {
            continue;
        }
        let values = column_types
            .iter()
            .enumerate()
            .map(|(index, column_type)| pg_value_to_db_value(&row, index, column_type))
            .collect::<Result<Vec<_>, _>>()?;
        if sessions
            .lock()
            .map_err(result_session_database_error)?
            .push_row(&session_owner, values)
            .map_err(result_session_database_error)?
            == PushRowOutcome::LimitReached
        {
            limit_reached = true;
            // A cache-limit cancel is internal, not a user Cancel flag. If the
            // protocol request cannot be dispatched, keep draining/discarding
            // this same stream to EOF so the connection still settles while
            // preserving the already cached pages and limit outcome.
            let _ = connection.cancel_resource().cancel().await;
        }
    }
    let affected_rows = stream.rows_affected().map(|rows| rows.to_string());
    let effect_outcome = EffectOutcome::Unknown;
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

async fn mssql_run_materialized_unit(
    client: &mut MssqlClient,
    sql: &str,
    sessions: &ResultSessionState,
    session_owner: ResultSessionOwner,
    run_owner: &QueryRunOwner,
    cancel_rx: &mut tokio::sync::mpsc::UnboundedReceiver<QueryRunOwner>,
) -> Result<P6UnitOutcome, DatabaseError> {
    // ad-hoc batch（ExecuteSqlBatch）而非 client.query 的 sp_executesql RPC：
    // P6 unit 不帶參數，且 BEGIN/COMMIT 若在 sp_executesql 內執行，離開 sp 時
    // trancount 改變會觸發 Msg 266，交易 script 無法跨語句成立。
    let query = client.simple_query(sql);
    tokio::pin!(query);
    let mut stream = loop {
        tokio::select! {
            request = cancel_rx.recv() => {
                match request {
                    Some(request) if request == *run_owner => {
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
                break result.map_err(|error| {
                    mssql_database_error(&MssqlInternalError::Driver(error))
                })?;
            }
        }
    };

    let mut primary_result_index = None;
    let mut columns: Option<Vec<String>> = None;
    let mut session_guard: Option<SessionAbortGuard> = None;
    let mut deferred_error: Option<DatabaseError> = None;
    let mut limit_reached = false;
    loop {
        let item = tokio::select! {
            request = cancel_rx.recv() => {
                match request {
                    Some(request) if request == *run_owner => {
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
            item = stream.try_next() => item.map_err(|error| {
                mssql_database_error(&MssqlInternalError::Driver(error))
            })?,
        };
        let Some(item) = item else { break };
        match item {
            QueryItem::Metadata(metadata) => {
                let result_index = metadata.result_index();
                if primary_result_index.is_some_and(|primary| primary != result_index) {
                    deferred_error.get_or_insert_with(|| {
                        mssql_result_shape_error(
                            "P6 execution units must produce at most one result set",
                        )
                    });
                    continue;
                }
                if primary_result_index.is_none() {
                    primary_result_index = Some(result_index);
                    let result_columns = metadata
                        .columns()
                        .iter()
                        .map(|column| column.name().to_string())
                        .collect::<Vec<_>>();
                    let begin_result = sessions
                        .lock()
                        .map_err(result_session_database_error)?
                        .begin_session(session_owner.clone(), result_columns.clone())
                        .map_err(result_session_database_error);
                    match begin_result {
                        Ok(()) => {
                            session_guard = Some(SessionAbortGuard::new(
                                sessions.clone(),
                                session_owner.clone(),
                            ));
                            columns = Some(result_columns);
                        }
                        Err(error) => {
                            deferred_error.get_or_insert(error);
                        }
                    }
                }
            }
            QueryItem::Row(row) => {
                let Some(primary) = primary_result_index else {
                    deferred_error.get_or_insert_with(|| {
                        mssql_result_shape_error("MSSQL returned a row before result metadata")
                    });
                    continue;
                };
                if row.result_index() != primary {
                    deferred_error.get_or_insert_with(|| {
                        mssql_result_shape_error(
                            "P6 execution units must not mix MSSQL result sets",
                        )
                    });
                    continue;
                }
                if deferred_error.is_some() || limit_reached {
                    continue;
                }
                let values = match row
                    .into_iter()
                    .map(|cell| mssql_value_to_db_value(&cell))
                    .collect::<Result<Vec<_>, _>>()
                {
                    Ok(values) => values,
                    Err(error) => {
                        deferred_error.get_or_insert(error);
                        continue;
                    }
                };
                if sessions
                    .lock()
                    .map_err(result_session_database_error)?
                    .push_row(&session_owner, values)
                    .map_err(result_session_database_error)?
                    == PushRowOutcome::LimitReached
                {
                    // Cache exhaustion is not a user cancellation. Keep this
                    // borrowed stream alive and discard the remaining rows so
                    // the MSSQL connection settles normally at EOF.
                    limit_reached = true;
                }
            }
        }
    }

    if let Some(error) = deferred_error {
        return Err(error);
    }
    let affected_rows = aggregate_mssql_affected_rows(stream.rows_affected())?;
    if columns.is_some() {
        let result_session = sessions
            .lock()
            .map_err(result_session_database_error)?
            .finish_session(&session_owner, EffectOutcome::Unknown)
            .map_err(result_session_database_error)?;
        if let Some(guard) = session_guard.as_mut() {
            guard.disarm();
        }
        Ok(P6UnitOutcome {
            result: if limit_reached {
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
            effect_outcome: EffectOutcome::Unknown,
            stop: limit_reached,
            connection_terminated: false,
        })
    } else {
        Ok(P6UnitOutcome {
            result: StatementExecutionResult::Execute { affected_rows },
            effect_outcome: EffectOutcome::Unknown,
            stop: false,
            connection_terminated: false,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PrimaryPageRead {
    Streaming,
    End,
    LimitReached,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MssqlPrimaryClientDisposition {
    Reuse,
    CloseNoReuse,
}

#[derive(Debug, PartialEq, Eq)]
struct MssqlPrimaryClientFinish<C> {
    disposition: MssqlPrimaryClientDisposition,
    client_to_close: Option<C>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MssqlPrimaryExitPolicy {
    drain_required: bool,
    disposition: MssqlPrimaryClientDisposition,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MssqlPrimaryExit {
    NormalEof,
    Release,
    Limit,
    DeferredError,
    UserCancel,
    LifecycleTermination,
    ChannelClosedWhileTerminating,
    ChannelClosedUnexpectedly,
    DriverFailure,
}

impl MssqlPrimaryExit {
    fn policy(self) -> MssqlPrimaryExitPolicy {
        match self {
            Self::NormalEof | Self::Release | Self::Limit | Self::DeferredError => {
                MssqlPrimaryExitPolicy {
                    drain_required: true,
                    disposition: MssqlPrimaryClientDisposition::Reuse,
                }
            }
            Self::UserCancel
            | Self::LifecycleTermination
            | Self::ChannelClosedWhileTerminating
            | Self::ChannelClosedUnexpectedly
            | Self::DriverFailure => MssqlPrimaryExitPolicy {
                drain_required: false,
                disposition: MssqlPrimaryClientDisposition::CloseNoReuse,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MssqlPrimaryStateError {
    ClientUnavailable,
    ExitNotRequested,
    DrainRequired,
    ClientSlotOccupied,
}

/// Owns the client taken from `AsyncMutex<Option<MssqlClient>>` while a
/// tiberius `QueryStream` borrows it in the worker's lexical scope. The generic
/// parameter keeps the ownership policy deterministic and testable without a
/// network connection.
struct MssqlPrimaryClientState<C> {
    client: Option<C>,
    exit: Option<MssqlPrimaryExit>,
    drained: bool,
}

impl<C> MssqlPrimaryClientState<C> {
    fn take(slot: &mut Option<C>) -> Result<Self, MssqlPrimaryStateError> {
        let client = slot
            .take()
            .ok_or(MssqlPrimaryStateError::ClientUnavailable)?;
        Ok(Self {
            client: Some(client),
            exit: None,
            drained: false,
        })
    }

    fn request_exit(&mut self, exit: MssqlPrimaryExit) {
        self.exit = Some(exit);
    }

    fn client_mut(&mut self) -> Result<&mut C, MssqlPrimaryStateError> {
        self.client
            .as_mut()
            .ok_or(MssqlPrimaryStateError::ClientUnavailable)
    }

    fn mark_drained(&mut self) {
        self.drained = true;
    }

    fn finish(
        &mut self,
        slot: &mut Option<C>,
    ) -> Result<MssqlPrimaryClientFinish<C>, MssqlPrimaryStateError> {
        let policy = self
            .exit
            .ok_or(MssqlPrimaryStateError::ExitNotRequested)?
            .policy();
        if policy.drain_required && !self.drained {
            return Err(MssqlPrimaryStateError::DrainRequired);
        }
        let client = self
            .client
            .take()
            .ok_or(MssqlPrimaryStateError::ClientUnavailable)?;
        let client_to_close = match policy.disposition {
            MssqlPrimaryClientDisposition::Reuse => {
                if slot.is_some() {
                    self.client = Some(client);
                    return Err(MssqlPrimaryStateError::ClientSlotOccupied);
                }
                *slot = Some(client);
                None
            }
            MssqlPrimaryClientDisposition::CloseNoReuse => Some(client),
        };
        Ok(MssqlPrimaryClientFinish {
            disposition: policy.disposition,
            client_to_close,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MssqlPrimaryPageProgress {
    Continue,
    Streaming,
    Draining,
}

#[derive(Debug, PartialEq, Eq)]
struct MssqlPrimaryStreamTerminal {
    exit: MssqlPrimaryExit,
    affected_rows: Option<String>,
    effect_outcome: EffectOutcome,
    deferred_error: Option<DatabaseError>,
    drained_items: usize,
}

/// Deterministic item-level policy shared by the real tiberius loop and the
/// network-free fixture tests. It owns shape/error precedence and the
/// 500-row-plus-one-lookahead boundary, but never owns a driver stream.
struct MssqlPrimaryStreamState {
    shape: MssqlDrainState,
    page_cached_rows: usize,
    exit: Option<MssqlPrimaryExit>,
    drained_items: usize,
    session_started: bool,
}

impl MssqlPrimaryStreamState {
    fn new(cached_lookahead_rows: usize) -> Self {
        Self {
            shape: MssqlDrainState::default(),
            page_cached_rows: cached_lookahead_rows,
            exit: None,
            drained_items: 0,
            session_started: false,
        }
    }

    fn begin_page(&mut self, cached_lookahead_rows: usize) {
        self.page_cached_rows = cached_lookahead_rows;
    }

    fn defer_shape_error_if_present(&mut self) {
        if self.shape.deferred_error.is_some() {
            self.exit = Some(MssqlPrimaryExit::DeferredError);
        }
    }

    fn defer_error(&mut self, error: DatabaseError) {
        self.shape.defer_error(error);
        self.exit = Some(MssqlPrimaryExit::DeferredError);
    }

    fn mark_session_started(&mut self) {
        self.session_started = true;
    }

    fn observe_metadata(
        &mut self,
        result_index: usize,
        columns: Vec<String>,
    ) -> Option<Vec<String>> {
        let first_result = self.shape.primary_result_index.is_none();
        let session_columns = first_result.then(|| columns.clone());
        self.shape.observe_metadata(result_index, columns);
        self.defer_shape_error_if_present();
        if self.exit == Some(MssqlPrimaryExit::DeferredError) {
            None
        } else {
            session_columns
        }
    }

    fn prepare_row(&mut self, result_index: usize, column_count: usize) -> MssqlRowAction {
        let action = self
            .shape
            .prepare_row(result_index, column_count, usize::MAX);
        self.defer_shape_error_if_present();
        if self.exit.is_some() {
            MssqlRowAction::DrainOnly
        } else {
            action
        }
    }

    fn record_decoded_row(
        &mut self,
        decoded: Result<Vec<DbValue>, DatabaseError>,
    ) -> Option<Vec<DbValue>> {
        if self.exit.is_some() {
            return None;
        }
        match decoded {
            Ok(row) => Some(row),
            Err(error) => {
                self.shape.defer_error(error);
                self.exit = Some(MssqlPrimaryExit::DeferredError);
                None
            }
        }
    }

    fn record_push(&mut self, outcome: PushRowOutcome) -> MssqlPrimaryPageProgress {
        match outcome {
            PushRowOutcome::Stored if self.exit.is_none() => {
                self.page_cached_rows += 1;
            }
            PushRowOutcome::LimitReached => {
                if self.exit.is_none() {
                    self.exit = Some(MssqlPrimaryExit::Limit);
                }
            }
            PushRowOutcome::Stored => {}
        }
        self.page_progress()
    }

    fn page_progress(&self) -> MssqlPrimaryPageProgress {
        if self.exit.is_some() {
            MssqlPrimaryPageProgress::Draining
        } else if self.page_cached_rows > RESULT_PAGE_ROWS {
            MssqlPrimaryPageProgress::Streaming
        } else {
            MssqlPrimaryPageProgress::Continue
        }
    }

    fn request_release(&mut self) {
        if self.exit.is_none() {
            self.exit = Some(MssqlPrimaryExit::Release);
        }
    }

    fn record_drained_item(&mut self) {
        self.drained_items += 1;
    }

    fn finish_eof(&mut self, counts: &[u64]) -> MssqlPrimaryStreamTerminal {
        let affected_rows = match aggregate_mssql_affected_rows(counts) {
            Ok(rows) => rows,
            Err(error) => {
                self.shape.defer_error(error);
                None
            }
        };
        self.defer_shape_error_if_present();
        let deferred_error = self.shape.deferred_error.take();
        let exit = if deferred_error.is_some() {
            MssqlPrimaryExit::DeferredError
        } else {
            self.exit.unwrap_or(MssqlPrimaryExit::NormalEof)
        };
        MssqlPrimaryStreamTerminal {
            exit,
            affected_rows,
            effect_outcome: EffectOutcome::Unknown,
            deferred_error,
            drained_items: self.drained_items,
        }
    }
}

/// Real-driver compile seam: the stream lifetime is tied to the mutable
/// `MssqlClient` borrow and therefore cannot be stored beside that client.
#[allow(dead_code)]
async fn mssql_primary_query_stream_compile_seam<'a>(
    state: &'a mut MssqlPrimaryClientState<MssqlClient>,
    sql: &'a str,
) -> Result<tiberius::QueryStream<'a>, tiberius::error::Error> {
    state
        .client_mut()
        .expect("MSSQL primary worker owns its taken client")
        .query(sql, &[])
        .await
}

enum MssqlPrimaryCompletion {
    None,
    Initial(PrimaryInitialSender, Result<P6UnitOutcome, DatabaseError>),
    Continuation(
        tokio::sync::oneshot::Sender<ResultContinuationAck>,
        ResultContinuationOutcome,
    ),
}

struct MssqlPrimaryDriveResult {
    exit: MssqlPrimaryExit,
    drained: bool,
    completion: MssqlPrimaryCompletion,
}

async fn mssql_drive_primary_stream(
    client: &mut MssqlClient,
    actor: &ProductionConnectionActor,
    sessions: &ResultSessionState,
    sql: &str,
    run_owner: &QueryRunOwner,
    session_owner: &ResultSessionOwner,
    lease: &ExecutionLease,
    continuation_sender: tokio::sync::mpsc::UnboundedSender<ResultContinuationCommand>,
    continuation_receiver: tokio::sync::mpsc::UnboundedReceiver<ResultContinuationCommand>,
    cancel_rx: tokio::sync::mpsc::UnboundedReceiver<QueryRunOwner>,
    initial_sender: PrimaryInitialSender,
) -> MssqlPrimaryDriveResult {
    let mut continuation_sender = Some(continuation_sender);
    let mut continuation_receiver = continuation_receiver;
    let mut cancel_rx = cancel_rx;
    let mut initial_sender = Some(initial_sender);
    let mut initial_sent = false;
    let mut pending_next: Option<tokio::sync::oneshot::Sender<ResultContinuationAck>> = None;
    let mut pending_release: Option<tokio::sync::oneshot::Sender<ResultContinuationAck>> = None;
    let mut next_page_index = 0usize;
    let mut state = MssqlPrimaryStreamState::new(0);

    let cancelled_completion = |sender: Option<PrimaryInitialSender>| match sender {
        Some(sender) => MssqlPrimaryCompletion::Initial(
            sender,
            Ok(P6UnitOutcome {
                result: StatementExecutionResult::Cancelled {
                    error: mssql_cancelled_connection_error(),
                },
                effect_outcome: EffectOutcome::Unknown,
                stop: true,
                connection_terminated: true,
            }),
        ),
        None => MssqlPrimaryCompletion::None,
    };

    let query = client.query(sql, &[]);
    tokio::pin!(query);
    let mut stream = loop {
        tokio::select! {
            request = cancel_rx.recv() => {
                match request {
                    Some(request) if request == *run_owner => {
                        return MssqlPrimaryDriveResult {
                            exit: MssqlPrimaryExit::UserCancel,
                            drained: false,
                            completion: cancelled_completion(initial_sender.take()),
                        };
                    }
                    Some(_) => continue,
                    None => {
                        let terminating = actor.is_terminating();
                        let error = DatabaseError {
                            engine: DatabaseErrorEngine::Yuzora,
                            message: "MSSQL cancellation channel closed unexpectedly".to_string(),
                            code: Some("cancelChannelClosed".to_string()),
                            position: None,
                            detail: None,
                            hint: None,
                            retryability: Retryability::NotRetryable,
                        };
                        return MssqlPrimaryDriveResult {
                            exit: if terminating {
                                MssqlPrimaryExit::LifecycleTermination
                            } else {
                                MssqlPrimaryExit::ChannelClosedUnexpectedly
                            },
                            drained: false,
                            completion: MssqlPrimaryCompletion::Initial(
                                initial_sender.take().expect("initial completion is pending"),
                                Err(error),
                            ),
                        };
                    }
                }
            }
            result = &mut query => {
                match result {
                    Ok(stream) => break stream,
                    Err(error) => {
                        let error = mssql_database_error(&MssqlInternalError::Driver(error));
                        return MssqlPrimaryDriveResult {
                            exit: MssqlPrimaryExit::DriverFailure,
                            drained: false,
                            completion: MssqlPrimaryCompletion::Initial(
                                initial_sender.take().expect("initial completion is pending"),
                                Ok(P6UnitOutcome {
                                    result: StatementExecutionResult::Error { error },
                                    effect_outcome: EffectOutcome::Unknown,
                                    stop: true,
                                    connection_terminated: true,
                                }),
                            ),
                        };
                    }
                }
            }
        }
    };

    loop {
        let item = tokio::select! {
            request = cancel_rx.recv() => {
                match request {
                    Some(request) if request == *run_owner => {
                        if state.session_started {
                            let _ = sessions.lock().map(|mut sessions| {
                                let _ = sessions.finish_session_with_lifecycle(
                                    session_owner,
                                    EffectOutcome::Unknown,
                                    ResultSessionLifecycle::Cancelled,
                                );
                            });
                        }
                        if let Some(respond_to) = pending_next.take() {
                            let _ = respond_to.send(ResultContinuationAck {
                                outcome: ResultContinuationOutcome::Cancelled,
                            });
                        }
                        if let Some(respond_to) = pending_release.take() {
                            let _ = respond_to.send(ResultContinuationAck {
                                outcome: ResultContinuationOutcome::Cancelled,
                            });
                        }
                        return MssqlPrimaryDriveResult {
                            exit: MssqlPrimaryExit::UserCancel,
                            drained: false,
                            completion: cancelled_completion(initial_sender.take()),
                        };
                    }
                    Some(_) => continue,
                    None => {
                        let terminating = actor.is_terminating();
                        if state.session_started {
                            let _ = sessions.lock().map(|mut sessions| {
                                let _ = sessions.finish_session_with_lifecycle(
                                    session_owner,
                                    EffectOutcome::Unknown,
                                    ResultSessionLifecycle::Error,
                                );
                            });
                        }
                        return MssqlPrimaryDriveResult {
                            exit: if terminating {
                                MssqlPrimaryExit::LifecycleTermination
                            } else {
                                MssqlPrimaryExit::ChannelClosedUnexpectedly
                            },
                            drained: false,
                            completion: MssqlPrimaryCompletion::None,
                        };
                    }
                }
            }
            command = continuation_receiver.recv(), if initial_sent => {
                match command {
                    Some(ResultContinuationCommand::Cancel) => {
                        if state.session_started {
                            let _ = sessions.lock().map(|mut sessions| {
                                let _ = sessions.finish_session_with_lifecycle(
                                    session_owner,
                                    EffectOutcome::Unknown,
                                    ResultSessionLifecycle::Cancelled,
                                );
                            });
                        }
                        return MssqlPrimaryDriveResult {
                            exit: MssqlPrimaryExit::UserCancel,
                            drained: false,
                            completion: MssqlPrimaryCompletion::None,
                        };
                    }
                    Some(ResultContinuationCommand::Release { respond_to }) => {
                        if let Some(pending) = pending_next.take() {
                            let _ = pending.send(ResultContinuationAck {
                                outcome: ResultContinuationOutcome::Error,
                            });
                        }
                        pending_release = Some(respond_to);
                        state.request_release();
                        continue;
                    }
                    Some(ResultContinuationCommand::Next { respond_to }) => {
                        let _ = respond_to.send(ResultContinuationAck {
                            outcome: ResultContinuationOutcome::Error,
                        });
                        continue;
                    }
                    None => {
                        let terminating = actor.is_terminating();
                        if state.session_started {
                            let _ = sessions.lock().map(|mut sessions| {
                                let _ = sessions.finish_session_with_lifecycle(
                                    session_owner,
                                    EffectOutcome::Unknown,
                                    ResultSessionLifecycle::Error,
                                );
                            });
                        }
                        return MssqlPrimaryDriveResult {
                            exit: if terminating {
                                MssqlPrimaryExit::ChannelClosedWhileTerminating
                            } else {
                                MssqlPrimaryExit::ChannelClosedUnexpectedly
                            },
                            drained: false,
                            completion: MssqlPrimaryCompletion::None,
                        };
                    }
                }
            }
            item = stream.try_next() => {
                match item {
                    Ok(item) => item,
                    Err(error) => {
                        let cancelled = actor.cancel_requested(lease).unwrap_or(false);
                        let error = mssql_database_error(&MssqlInternalError::Driver(error));
                        if state.session_started {
                            let _ = sessions.lock().map(|mut sessions| {
                                let _ = sessions.finish_session_with_lifecycle(
                                    session_owner,
                                    EffectOutcome::Unknown,
                                    if cancelled {
                                        ResultSessionLifecycle::Cancelled
                                    } else {
                                        ResultSessionLifecycle::Error
                                    },
                                );
                            });
                        }
                        let completion = if let Some(sender) = initial_sender.take() {
                            MssqlPrimaryCompletion::Initial(
                                sender,
                                Ok(P6UnitOutcome {
                                    result: if cancelled {
                                        StatementExecutionResult::Cancelled { error }
                                    } else {
                                        StatementExecutionResult::Error { error }
                                    },
                                    effect_outcome: EffectOutcome::Unknown,
                                    stop: true,
                                    connection_terminated: true,
                                }),
                            )
                        } else if let Some(sender) = pending_next.take().or_else(|| pending_release.take()) {
                            MssqlPrimaryCompletion::Continuation(
                                sender,
                                if cancelled {
                                    ResultContinuationOutcome::Cancelled
                                } else {
                                    ResultContinuationOutcome::Error
                                },
                            )
                        } else {
                            MssqlPrimaryCompletion::None
                        };
                        return MssqlPrimaryDriveResult {
                            exit: if cancelled {
                                MssqlPrimaryExit::UserCancel
                            } else {
                                MssqlPrimaryExit::DriverFailure
                            },
                            drained: false,
                            completion,
                        };
                    }
                }
            }
        };

        let Some(item) = item else {
            let terminal = state.finish_eof(stream.rows_affected());
            let completion = if !state.session_started {
                let result = match terminal.deferred_error {
                    Some(error) => Err(error),
                    None => Ok(P6UnitOutcome {
                        result: StatementExecutionResult::Execute {
                            affected_rows: terminal.affected_rows,
                        },
                        effect_outcome: terminal.effect_outcome,
                        stop: false,
                        connection_terminated: false,
                    }),
                };
                MssqlPrimaryCompletion::Initial(
                    initial_sender
                        .take()
                        .expect("initial completion is pending"),
                    result,
                )
            } else {
                match terminal.exit {
                    MssqlPrimaryExit::Release => {
                        let _ = sessions.lock().map(|mut sessions| {
                            let _ = sessions
                                .release_with_effect(session_owner, terminal.effect_outcome);
                        });
                    }
                    MssqlPrimaryExit::DeferredError => {
                        if initial_sent {
                            let _ = sessions.lock().map(|mut sessions| {
                                let _ = sessions.finish_session_with_lifecycle(
                                    session_owner,
                                    terminal.effect_outcome,
                                    ResultSessionLifecycle::Error,
                                );
                            });
                        } else if let Ok(mut sessions) = sessions.lock() {
                            let _ = sessions.discard(session_owner);
                        }
                    }
                    _ => {
                        let _ = sessions.lock().map(|mut sessions| {
                            let _ = sessions.finish_session(session_owner, terminal.effect_outcome);
                        });
                    }
                }

                if !initial_sent {
                    match terminal.deferred_error {
                        Some(error) => MssqlPrimaryCompletion::Initial(
                            initial_sender
                                .take()
                                .expect("initial completion is pending"),
                            Err(error),
                        ),
                        None => {
                            let result_session = sessions
                                .lock()
                                .map_err(result_session_database_error)
                                .and_then(|sessions| {
                                    sessions
                                        .result_session(session_owner)
                                        .map_err(result_session_database_error)
                                });
                            MssqlPrimaryCompletion::Initial(
                                initial_sender
                                    .take()
                                    .expect("initial completion is pending"),
                                result_session.map(|session| {
                                    primary_rows_outcome(
                                        session,
                                        terminal.effect_outcome,
                                        terminal.exit == MssqlPrimaryExit::Limit,
                                        terminal.affected_rows,
                                    )
                                }),
                            )
                        }
                    }
                } else if let Some(sender) = pending_release.take() {
                    MssqlPrimaryCompletion::Continuation(
                        sender,
                        if terminal.exit == MssqlPrimaryExit::Release {
                            ResultContinuationOutcome::Released
                        } else {
                            ResultContinuationOutcome::Error
                        },
                    )
                } else if let Some(sender) = pending_next.take() {
                    MssqlPrimaryCompletion::Continuation(
                        sender,
                        match terminal.exit {
                            MssqlPrimaryExit::Limit => ResultContinuationOutcome::LimitReached,
                            MssqlPrimaryExit::DeferredError => ResultContinuationOutcome::Error,
                            _ => ResultContinuationOutcome::End,
                        },
                    )
                } else {
                    MssqlPrimaryCompletion::None
                }
            };
            return MssqlPrimaryDriveResult {
                exit: terminal.exit,
                drained: true,
                completion,
            };
        };

        if state.exit.is_some() {
            state.record_drained_item();
            continue;
        }
        match item {
            QueryItem::Metadata(metadata) => {
                if let Some(columns) = state.observe_metadata(
                    metadata.result_index(),
                    metadata
                        .columns()
                        .iter()
                        .map(|column| column.name().to_string())
                        .collect(),
                ) {
                    match sessions
                        .lock()
                        .map_err(result_session_database_error)
                        .and_then(|mut sessions| {
                            sessions
                                .begin_session(session_owner.clone(), columns)
                                .map_err(result_session_database_error)
                        }) {
                        Ok(()) => state.mark_session_started(),
                        Err(error) => state.defer_error(error),
                    }
                }
            }
            QueryItem::Row(row) => {
                if state.prepare_row(row.result_index(), row.columns().len())
                    == MssqlRowAction::Decode
                {
                    let decoded = row
                        .into_iter()
                        .map(|cell| mssql_value_to_db_value(&cell))
                        .collect::<Result<Vec<_>, _>>();
                    if let Some(values) = state.record_decoded_row(decoded) {
                        let outcome = sessions
                            .lock()
                            .map_err(result_session_database_error)
                            .and_then(|mut sessions| {
                                sessions
                                    .push_row(session_owner, values)
                                    .map_err(result_session_database_error)
                            });
                        match outcome {
                            Ok(outcome) => {
                                state.record_push(outcome);
                            }
                            Err(error) => state.defer_error(error),
                        }
                    }
                } else {
                    state.record_drained_item();
                }
            }
        }

        if state.page_progress() != MssqlPrimaryPageProgress::Streaming {
            continue;
        }
        let ready = sessions
            .lock()
            .map_err(result_session_database_error)
            .and_then(|mut sessions| {
                sessions
                    .mark_page_ready(session_owner, next_page_index)
                    .map_err(result_session_database_error)?;
                sessions
                    .result_session(session_owner)
                    .map_err(result_session_database_error)
            });
        if !initial_sent {
            if let Err(error) = actor.install_result_continuation(
                lease,
                session_owner.clone(),
                continuation_sender
                    .take()
                    .expect("MSSQL primary installs one continuation sender"),
            ) {
                if let Ok(mut sessions) = sessions.lock() {
                    let _ = sessions.discard(session_owner);
                }
                return MssqlPrimaryDriveResult {
                    exit: MssqlPrimaryExit::ChannelClosedUnexpectedly,
                    drained: false,
                    completion: MssqlPrimaryCompletion::Initial(
                        initial_sender
                            .take()
                            .expect("initial completion is pending"),
                        Err(continuation_database_error(error)),
                    ),
                };
            }
            match ready {
                Ok(session) => {
                    let send_result = initial_sender
                        .take()
                        .expect("initial completion is pending")
                        .send(Ok(primary_rows_outcome(
                            session,
                            EffectOutcome::Unknown,
                            false,
                            None,
                        )));
                    if send_result.is_err() {
                        if let Ok(mut sessions) = sessions.lock() {
                            let _ = sessions.discard(session_owner);
                        }
                        return MssqlPrimaryDriveResult {
                            exit: MssqlPrimaryExit::ChannelClosedUnexpectedly,
                            drained: false,
                            completion: MssqlPrimaryCompletion::None,
                        };
                    }
                    initial_sent = true;
                }
                Err(error) => {
                    return MssqlPrimaryDriveResult {
                        exit: MssqlPrimaryExit::ChannelClosedUnexpectedly,
                        drained: false,
                        completion: MssqlPrimaryCompletion::Initial(
                            initial_sender
                                .take()
                                .expect("initial completion is pending"),
                            Err(error),
                        ),
                    };
                }
            }
        } else if let Some(respond_to) = pending_next.take() {
            let _ = respond_to.send(ResultContinuationAck {
                outcome: if ready.is_ok() {
                    ResultContinuationOutcome::PageReady
                } else {
                    ResultContinuationOutcome::Error
                },
            });
        }
        next_page_index += 1;

        loop {
            tokio::select! {
                request = cancel_rx.recv() => {
                    match request {
                        Some(request) if request == *run_owner => {
                            let _ = sessions.lock().map(|mut sessions| {
                                let _ = sessions.finish_session_with_lifecycle(
                                    session_owner,
                                    EffectOutcome::Unknown,
                                    ResultSessionLifecycle::Cancelled,
                                );
                            });
                            return MssqlPrimaryDriveResult {
                                exit: MssqlPrimaryExit::UserCancel,
                                drained: false,
                                completion: MssqlPrimaryCompletion::None,
                            };
                        }
                        Some(_) => continue,
                        None => {
                            return MssqlPrimaryDriveResult {
                                exit: if actor.is_terminating() {
                                    MssqlPrimaryExit::LifecycleTermination
                                } else {
                                    MssqlPrimaryExit::ChannelClosedUnexpectedly
                                },
                                drained: false,
                                completion: MssqlPrimaryCompletion::None,
                            };
                        }
                    }
                }
                command = continuation_receiver.recv() => {
                    match command {
                        Some(ResultContinuationCommand::Next { respond_to }) => {
                            pending_next = Some(respond_to);
                            state.begin_page(1);
                            break;
                        }
                        Some(ResultContinuationCommand::Release { respond_to }) => {
                            pending_release = Some(respond_to);
                            state.request_release();
                            break;
                        }
                        Some(ResultContinuationCommand::Cancel) => {
                            let _ = sessions.lock().map(|mut sessions| {
                                let _ = sessions.finish_session_with_lifecycle(
                                    session_owner,
                                    EffectOutcome::Unknown,
                                    ResultSessionLifecycle::Cancelled,
                                );
                            });
                            return MssqlPrimaryDriveResult {
                                exit: MssqlPrimaryExit::UserCancel,
                                drained: false,
                                completion: MssqlPrimaryCompletion::None,
                            };
                        }
                        None => {
                            return MssqlPrimaryDriveResult {
                                exit: if actor.is_terminating() {
                                    MssqlPrimaryExit::ChannelClosedWhileTerminating
                                } else {
                                    MssqlPrimaryExit::ChannelClosedUnexpectedly
                                },
                                drained: false,
                                completion: MssqlPrimaryCompletion::None,
                            };
                        }
                    }
                }
            }
        }
    }
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
    let mut settlement_guard = Some(settlement_guard);
    let DbHandle::Mssql(connection) = actor.handle() else {
        let _ = initial_sender.send(Err(continuation_database_error(ActorError::OwnerMismatch)));
        return;
    };
    let mut client_state = {
        let mut slot = connection.lock().await;
        match MssqlPrimaryClientState::take(&mut slot) {
            Ok(state) => state,
            Err(_) => {
                let _ = settle_primary_guard(&mut settlement_guard);
                let _ = initial_sender.send(Ok(P6UnitOutcome {
                    result: StatementExecutionResult::Error {
                        error: DatabaseError {
                            engine: DatabaseErrorEngine::Mssql,
                            message: "database server disconnected".to_string(),
                            code: Some("serverDisconnected".to_string()),
                            position: None,
                            detail: None,
                            hint: None,
                            retryability: Retryability::Retryable,
                        },
                    },
                    effect_outcome: EffectOutcome::Unknown,
                    stop: true,
                    connection_terminated: true,
                }));
                return;
            }
        }
    };

    let drive = {
        let client = client_state
            .client_mut()
            .expect("MSSQL primary worker owns its taken client");
        mssql_drive_primary_stream(
            client,
            &actor,
            &sessions,
            &sql,
            &run_owner,
            &session_owner,
            &lease,
            continuation_sender,
            continuation_receiver,
            cancel_rx,
            initial_sender,
        )
        .await
    };

    client_state.request_exit(drive.exit);
    if drive.drained {
        client_state.mark_drained();
    }
    let finish = {
        let mut slot = connection.lock().await;
        client_state.finish(&mut slot)
    };
    let mut terminated = false;
    if let Ok(finish) = finish {
        if finish.disposition == MssqlPrimaryClientDisposition::CloseNoReuse {
            terminated = true;
            if let Some(client) = finish.client_to_close {
                let _ = client.close().await;
            }
        }
    } else {
        terminated = true;
    }
    if terminated {
        let _ = actor.mark_connection_terminated(&lease);
    }
    let _ = settle_primary_guard(&mut settlement_guard);

    match drive.completion {
        MssqlPrimaryCompletion::None => {}
        MssqlPrimaryCompletion::Initial(sender, result) => {
            let _ = sender.send(result);
        }
        MssqlPrimaryCompletion::Continuation(sender, outcome) => {
            let _ = sender.send(ResultContinuationAck { outcome });
        }
    }
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
    guard
        .take()
        .expect("primary worker settlement guard is armed")
        .settle()
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
            let values = (0..column_count)
                .map(|index| {
                    row.get_ref(index)
                        .map_err(|error| sqlite_database_error(&error))
                        .and_then(value_to_db_value)
                })
                .collect::<Result<Vec<_>, _>>()?;
            if sessions
                .lock()
                .map_err(result_session_database_error)?
                .push_row(&session_owner, values)
                .map_err(result_session_database_error)?
                == PushRowOutcome::LimitReached
            {
                return Ok(PrimaryPageRead::LimitReached);
            }
            cached_rows += 1;
        }

        let Some(row) = rows.next().map_err(|error| sqlite_database_error(&error))? else {
            return Ok(PrimaryPageRead::End);
        };
        let values = (0..column_count)
            .map(|index| {
                row.get_ref(index)
                    .map_err(|error| sqlite_database_error(&error))
                    .and_then(value_to_db_value)
            })
            .collect::<Result<Vec<_>, _>>()?;
        if sessions
            .lock()
            .map_err(result_session_database_error)?
            .push_row(&session_owner, values)
            .map_err(result_session_database_error)?
            == PushRowOutcome::LimitReached
        {
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

async fn pg_read_primary_page(
    stream: &mut std::pin::Pin<Box<tokio_postgres::RowStream>>,
    column_types: &[PgType],
    sessions: &ResultSessionState,
    session_owner: &ResultSessionOwner,
    mut cached_rows: usize,
) -> Result<PrimaryPageRead, DatabaseError> {
    while cached_rows < RESULT_PAGE_ROWS {
        let Some(row) = stream
            .as_mut()
            .try_next()
            .await
            .map_err(|error| postgres_database_error(&error))?
        else {
            return Ok(PrimaryPageRead::End);
        };
        let values = column_types
            .iter()
            .enumerate()
            .map(|(index, column_type)| pg_value_to_db_value(&row, index, column_type))
            .collect::<Result<Vec<_>, _>>()?;
        if sessions
            .lock()
            .map_err(result_session_database_error)?
            .push_row(session_owner, values)
            .map_err(result_session_database_error)?
            == PushRowOutcome::LimitReached
        {
            return Ok(PrimaryPageRead::LimitReached);
        }
        cached_rows += 1;
    }

    let Some(row) = stream
        .as_mut()
        .try_next()
        .await
        .map_err(|error| postgres_database_error(&error))?
    else {
        return Ok(PrimaryPageRead::End);
    };
    let values = column_types
        .iter()
        .enumerate()
        .map(|(index, column_type)| pg_value_to_db_value(&row, index, column_type))
        .collect::<Result<Vec<_>, _>>()?;
    if sessions
        .lock()
        .map_err(result_session_database_error)?
        .push_row(session_owner, values)
        .map_err(result_session_database_error)?
        == PushRowOutcome::LimitReached
    {
        Ok(PrimaryPageRead::LimitReached)
    } else {
        Ok(PrimaryPageRead::Streaming)
    }
}

async fn pg_cancel_and_drain(
    connection: &PgConn,
    stream: &mut std::pin::Pin<Box<tokio_postgres::RowStream>>,
) {
    let _ = connection.cancel_resource().cancel().await;
    loop {
        match stream.as_mut().try_next().await {
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => break,
        }
    }
}

async fn pg_run_primary_worker(
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
    let DbHandle::Postgres(connection) = actor.handle() else {
        let _ = initial_sender.send(Err(continuation_database_error(ActorError::OwnerMismatch)));
        return;
    };
    let statement = match connection.client.prepare(&sql).await {
        Ok(statement) => statement,
        Err(error) => {
            let cancelled = actor.cancel_requested(&lease).unwrap_or(false);
            let _ = settle_primary_guard(&mut settlement_guard);
            let error = postgres_database_error(&error);
            let result = if cancelled {
                Ok(P6UnitOutcome {
                    result: StatementExecutionResult::Cancelled { error },
                    effect_outcome: EffectOutcome::Unknown,
                    stop: true,
                    connection_terminated: false,
                })
            } else {
                Err(error)
            };
            let _ = initial_sender.send(result);
            return;
        }
    };
    if statement.columns().is_empty() {
        let result = connection
            .client
            .execute(&statement, &[])
            .await
            .map_err(|error| postgres_database_error(&error));
        let cancelled = actor.cancel_requested(&lease).unwrap_or(false);
        let _ = settle_primary_guard(&mut settlement_guard);
        let result = match result {
            Ok(affected) => Ok(P6UnitOutcome {
                result: StatementExecutionResult::Execute {
                    affected_rows: Some(affected.to_string()),
                },
                effect_outcome: EffectOutcome::Unknown,
                stop: false,
                connection_terminated: false,
            }),
            Err(error) if cancelled => Ok(P6UnitOutcome {
                result: StatementExecutionResult::Cancelled { error },
                effect_outcome: EffectOutcome::Unknown,
                stop: true,
                connection_terminated: false,
            }),
            Err(error) => Err(error),
        };
        let _ = initial_sender.send(result);
        return;
    }

    let columns = statement
        .columns()
        .iter()
        .map(|column| column.name().to_string())
        .collect::<Vec<_>>();
    let column_types = statement
        .columns()
        .iter()
        .map(|column| column.type_().clone())
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
    let params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = Vec::new();
    let stream = match connection.client.query_raw(&statement, params).await {
        Ok(stream) => stream,
        Err(error) => {
            if let Ok(mut sessions) = sessions.lock() {
                let _ = sessions.discard(&session_owner);
            }
            let cancelled = actor.cancel_requested(&lease).unwrap_or(false);
            let _ = settle_primary_guard(&mut settlement_guard);
            let error = postgres_database_error(&error);
            let result = if cancelled {
                Ok(P6UnitOutcome {
                    result: StatementExecutionResult::Cancelled { error },
                    effect_outcome: EffectOutcome::Unknown,
                    stop: true,
                    connection_terminated: false,
                })
            } else {
                Err(error)
            };
            let _ = initial_sender.send(result);
            return;
        }
    };
    let mut stream = Box::pin(stream);
    match pg_read_primary_page(&mut stream, &column_types, &sessions, &session_owner, 0).await {
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
                pg_cancel_and_drain(connection, &mut stream).await;
                let _ = settle_primary_guard(&mut settlement_guard);
                let _ = initial_sender.send(Err(continuation_database_error(error)));
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
                        pg_cancel_and_drain(connection, &mut stream).await;
                        return;
                    }
                }
                Err(error) => {
                    pg_cancel_and_drain(connection, &mut stream).await;
                    let _ = settle_primary_guard(&mut settlement_guard);
                    let _ = initial_sender.send(Err(error));
                    return;
                }
            }
        }
        Ok(terminal) => {
            if terminal == PrimaryPageRead::LimitReached {
                pg_cancel_and_drain(connection, &mut stream).await;
            }
            let affected_rows = stream
                .as_ref()
                .get_ref()
                .rows_affected()
                .map(|rows| rows.to_string());
            drop(stream);
            let result_session = sessions
                .lock()
                .map_err(result_session_database_error)
                .and_then(|mut sessions| {
                    sessions
                        .finish_session(&session_owner, EffectOutcome::Unknown)
                        .map_err(result_session_database_error)
                });
            let _ = settle_primary_guard(&mut settlement_guard);
            let outcome = result_session.map(|session| {
                primary_rows_outcome(
                    session,
                    EffectOutcome::Unknown,
                    terminal == PrimaryPageRead::LimitReached,
                    affected_rows,
                )
            });
            let _ = initial_sender.send(outcome);
            return;
        }
        Err(error) => {
            let cancelled = actor.cancel_requested(&lease).unwrap_or(false);
            pg_cancel_and_drain(connection, &mut stream).await;
            drop(stream);
            if let Ok(mut sessions) = sessions.lock() {
                let _ = sessions.discard(&session_owner);
            }
            let _ = settle_primary_guard(&mut settlement_guard);
            let result = if cancelled {
                Ok(P6UnitOutcome {
                    result: StatementExecutionResult::Cancelled { error },
                    effect_outcome: EffectOutcome::Unknown,
                    stop: true,
                    connection_terminated: false,
                })
            } else {
                Err(error)
            };
            let _ = initial_sender.send(result);
            return;
        }
    }

    loop {
        match continuation_receiver.recv().await {
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
                match pg_read_primary_page(&mut stream, &column_types, &sessions, &session_owner, 1)
                    .await
                {
                    Ok(PrimaryPageRead::Streaming) => {
                        let ready = sessions
                            .lock()
                            .map_err(result_session_database_error)
                            .and_then(|mut sessions| {
                                sessions
                                    .mark_page_ready(&session_owner, page_index)
                                    .map_err(result_session_database_error)
                            });
                        if ready.is_ok() {
                            let _ = respond_to.send(ResultContinuationAck {
                                outcome: ResultContinuationOutcome::PageReady,
                            });
                        } else {
                            pg_cancel_and_drain(connection, &mut stream).await;
                            let _ = sessions.lock().map(|mut sessions| {
                                let _ = sessions.finish_session_with_lifecycle(
                                    &session_owner,
                                    EffectOutcome::Unknown,
                                    ResultSessionLifecycle::Error,
                                );
                            });
                            let _ = settle_primary_guard(&mut settlement_guard);
                            let _ = respond_to.send(ResultContinuationAck {
                                outcome: ResultContinuationOutcome::Error,
                            });
                            return;
                        }
                    }
                    Ok(terminal) => {
                        if terminal == PrimaryPageRead::LimitReached {
                            pg_cancel_and_drain(connection, &mut stream).await;
                        }
                        drop(stream);
                        let _ = sessions.lock().map(|mut sessions| {
                            let _ = sessions.finish_session(&session_owner, EffectOutcome::Unknown);
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
                        let cancelled = actor.cancel_requested(&lease).unwrap_or(false);
                        pg_cancel_and_drain(connection, &mut stream).await;
                        drop(stream);
                        let _ = sessions.lock().map(|mut sessions| {
                            let _ = sessions.finish_session_with_lifecycle(
                                &session_owner,
                                EffectOutcome::Unknown,
                                if cancelled {
                                    ResultSessionLifecycle::Cancelled
                                } else {
                                    ResultSessionLifecycle::Error
                                },
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
                pg_cancel_and_drain(connection, &mut stream).await;
                drop(stream);
                let _ = sessions.lock().map(|mut sessions| {
                    let _ = sessions.release_with_effect(&session_owner, EffectOutcome::Unknown);
                });
                let _ = settle_primary_guard(&mut settlement_guard);
                let _ = respond_to.send(ResultContinuationAck {
                    outcome: ResultContinuationOutcome::Released,
                });
                return;
            }
            Some(ResultContinuationCommand::Cancel) => {
                pg_cancel_and_drain(connection, &mut stream).await;
                drop(stream);
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
                pg_cancel_and_drain(connection, &mut stream).await;
                drop(stream);
                if let Ok(mut sessions) = sessions.lock() {
                    let _ = sessions.discard(&session_owner);
                }
                return;
            }
        }
    }
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
    let mut mssql_cancel_rx = if matches!(actor.handle(), DbHandle::Mssql(_)) {
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
                    session_owner,
                    worker_lease,
                    settlement_guard,
                    continuation_sender,
                    continuation_receiver,
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
            DbHandle::Mssql(connection) => {
                let mut guard = connection.lock().await;
                let Some(client) = guard.as_mut() else {
                    return Err(DatabaseOperationalError::new(
                        DatabaseOperationalErrorCode::ServerDisconnected,
                        "database server disconnected",
                    ));
                };
                let outcome = mssql_run_materialized_unit(
                    client,
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
                    let client = guard.take().expect("MSSQL client was borrowed above");
                    drop(guard);
                    let _ = client.close().await;
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
    let outcome = match actor.request_cancel(&owner).await.map_err(actor_error)? {
        crate::db_connection_actor::CancelRequest::AlreadyRequested => {
            QueryCancelOutcome::AlreadyRequested
        }
        crate::db_connection_actor::CancelRequest::DriverCancellationRequired(_) => {
            QueryCancelOutcome::Cancelled
        }
        crate::db_connection_actor::CancelRequest::ConnectionTerminationRequired => {
            QueryCancelOutcome::CancelledConnectionTerminated
        }
    };
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
            self.open(
                descriptor_id.into(),
                DbOpenConfig::Postgres {
                    host: host.into(),
                    port,
                    database: database.into(),
                    user: user.into(),
                    password: SecretString::from(password),
                    ssl,
                    trust_cert,
                },
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
            test_unregistered(DbOpenConfig::Postgres {
                host: host.into(),
                port,
                database: database.into(),
                user: user.into(),
                password: SecretString::from(password),
                ssl,
                trust_cert,
            })
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
        };
        let json = serde_json::to_value(page).unwrap();
        assert_eq!(json["lifecycle"], "released");
        assert_eq!(json["resultLimitReached"], true);
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
    fn p7_postgres_primary_worker_owns_rowstream_and_drains_release_before_settlement() {
        fn assert_send<T: Send>() {}
        assert_send::<tokio_postgres::RowStream>();

        let source = include_str!("db_service.rs");
        let worker = source
            .split("async fn pg_run_primary_worker")
            .nth(1)
            .and_then(|source| {
                source
                    .split("pub(crate) async fn query_run_in_state")
                    .next()
            })
            .expect("PostgreSQL primary worker source boundary");
        let release = worker
            .split("Some(ResultContinuationCommand::Release")
            .nth(1)
            .expect("PostgreSQL Release branch");
        let cancel = release
            .find("pg_cancel_and_drain(connection, &mut stream).await")
            .expect("Release must cancel and drain the owned RowStream");
        let settle = release
            .find("settle_primary_guard")
            .expect("Release must settle its exact lease");
        assert!(cancel < settle);
        assert!(source.contains("tauri::async_runtime::spawn(pg_run_primary_worker"));
    }

    #[derive(Debug, PartialEq, Eq)]
    struct FakeMssqlPrimaryClient(u8);

    #[test]
    fn p7_mssql_primary_client_is_taken_once_and_normal_eof_restores_it() {
        let mut slot = Some(FakeMssqlPrimaryClient(7));
        let mut worker = MssqlPrimaryClientState::take(&mut slot).unwrap();

        assert!(slot.is_none());
        assert!(matches!(
            MssqlPrimaryClientState::take(&mut slot),
            Err(MssqlPrimaryStateError::ClientUnavailable)
        ));

        worker.request_exit(MssqlPrimaryExit::NormalEof);
        assert_eq!(
            worker.finish(&mut slot),
            Err(MssqlPrimaryStateError::DrainRequired)
        );
        worker.mark_drained();
        let finished = worker.finish(&mut slot).unwrap();
        assert_eq!(finished.disposition, MssqlPrimaryClientDisposition::Reuse);
        assert!(finished.client_to_close.is_none());
        assert_eq!(slot, Some(FakeMssqlPrimaryClient(7)));
    }

    #[test]
    fn p7_mssql_primary_cancel_and_teardown_never_restore_the_client() {
        for reason in [
            MssqlPrimaryExit::UserCancel,
            MssqlPrimaryExit::LifecycleTermination,
            MssqlPrimaryExit::ChannelClosedWhileTerminating,
        ] {
            let mut slot = Some(FakeMssqlPrimaryClient(9));
            let mut worker = MssqlPrimaryClientState::take(&mut slot).unwrap();
            worker.request_exit(reason);

            let finished = worker.finish(&mut slot).unwrap();
            assert_eq!(
                finished.disposition,
                MssqlPrimaryClientDisposition::CloseNoReuse
            );
            assert_eq!(finished.client_to_close, Some(FakeMssqlPrimaryClient(9)));
            assert!(slot.is_none(), "{reason:?} restored a terminated client");
        }
    }

    #[test]
    fn p7_mssql_primary_release_limit_and_error_require_eof_before_reuse() {
        for reason in [
            MssqlPrimaryExit::Release,
            MssqlPrimaryExit::Limit,
            MssqlPrimaryExit::DeferredError,
        ] {
            let policy = reason.policy();
            assert!(policy.drain_required, "{reason:?} skipped EOF drain");
            assert_eq!(policy.disposition, MssqlPrimaryClientDisposition::Reuse);

            let mut slot = Some(FakeMssqlPrimaryClient(11));
            let mut worker = MssqlPrimaryClientState::take(&mut slot).unwrap();
            worker.request_exit(reason);
            assert_eq!(
                worker.finish(&mut slot),
                Err(MssqlPrimaryStateError::DrainRequired)
            );
            assert!(slot.is_none());
            worker.mark_drained();
            let finished = worker.finish(&mut slot).unwrap();
            assert_eq!(finished.disposition, MssqlPrimaryClientDisposition::Reuse);
            assert!(finished.client_to_close.is_none());
            assert_eq!(slot, Some(FakeMssqlPrimaryClient(11)));
        }
    }

    #[test]
    fn p7_mssql_primary_real_query_stream_lifetime_seam_compiles() {
        let _ = mssql_primary_query_stream_compile_seam;
        let source = include_str!("db_service.rs");
        assert!(source.contains("tauri::async_runtime::spawn(mssql_run_primary_worker"));
        assert!(source.contains("async fn mssql_drive_primary_stream"));
        assert!(!source.contains("todo!(\"P7 MSSQL primary"));
    }

    fn drive_mssql_primary_fixture_rows(
        state: &mut MssqlPrimaryStreamState,
        row_count: usize,
    ) -> usize {
        let mut observed = 0;
        for value in 0..row_count {
            if state.prepare_row(0, 1) == MssqlRowAction::DrainOnly {
                state.record_drained_item();
                continue;
            }
            let row = state
                .record_decoded_row(Ok(vec![DbValue::Integer {
                    value: value.to_string(),
                }]))
                .expect("fixture row should decode");
            assert_eq!(row.len(), 1);
            observed += 1;
            if state.record_push(PushRowOutcome::Stored) == MssqlPrimaryPageProgress::Streaming {
                break;
            }
        }
        observed
    }

    #[test]
    fn p7_mssql_primary_fixture_proves_500_and_501_lookahead_boundaries() {
        for (rows, expected_progress, expected_observed) in [
            (500, MssqlPrimaryPageProgress::Continue, 500),
            (501, MssqlPrimaryPageProgress::Streaming, 501),
        ] {
            let mut state = MssqlPrimaryStreamState::new(0);
            assert_eq!(
                state.observe_metadata(0, vec!["value".to_string()]),
                Some(vec!["value".to_string()])
            );
            assert_eq!(
                drive_mssql_primary_fixture_rows(&mut state, rows),
                expected_observed
            );
            assert_eq!(state.page_progress(), expected_progress);
        }
    }

    #[test]
    fn p7_mssql_primary_fixture_release_drains_later_items_and_keeps_effect() {
        let mut state = MssqlPrimaryStreamState::new(0);
        state.observe_metadata(0, vec!["value".to_string()]);
        assert_eq!(drive_mssql_primary_fixture_rows(&mut state, 2), 2);
        state.request_release();
        for _ in 0..3 {
            assert_eq!(state.prepare_row(0, 1), MssqlRowAction::DrainOnly);
            state.record_drained_item();
        }

        let terminal = state.finish_eof(&[5]);
        assert_eq!(terminal.exit, MssqlPrimaryExit::Release);
        assert_eq!(terminal.affected_rows.as_deref(), Some("5"));
        assert_eq!(terminal.effect_outcome, EffectOutcome::Unknown);
        assert_eq!(terminal.drained_items, 3);
        assert!(terminal.deferred_error.is_none());
    }

    #[test]
    fn p7_mssql_primary_fixture_defers_first_decode_and_shape_errors_until_eof() {
        let first_decode_error = value_decode_error(
            DatabaseErrorEngine::Mssql,
            "fixture value",
            "first decode failure",
        );
        let mut decode = MssqlPrimaryStreamState::new(0);
        decode.observe_metadata(0, vec!["value".to_string()]);
        assert!(decode
            .record_decoded_row(Err(first_decode_error.clone()))
            .is_none());
        decode.observe_metadata(1, vec!["later".to_string()]);
        decode.record_drained_item();
        let terminal = decode.finish_eof(&[7]);
        assert_eq!(terminal.exit, MssqlPrimaryExit::DeferredError);
        assert_eq!(terminal.deferred_error, Some(first_decode_error));
        assert_eq!(terminal.affected_rows.as_deref(), Some("7"));

        let mut shape = MssqlPrimaryStreamState::new(0);
        shape.observe_metadata(0, vec!["first".to_string()]);
        shape.observe_metadata(1, vec!["second".to_string()]);
        assert_eq!(shape.prepare_row(1, 1), MssqlRowAction::DrainOnly);
        shape.record_drained_item();
        let terminal = shape.finish_eof(&[]);
        assert_eq!(terminal.exit, MssqlPrimaryExit::DeferredError);
        assert_eq!(
            terminal
                .deferred_error
                .as_ref()
                .and_then(|error| error.code.as_deref()),
            Some("resultShape")
        );
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
            target: ProfileTarget::Postgres {
                host: "db.internal".to_string(),
                port: 5432,
                database: "app".to_string(),
                user: "alice".to_string(),
                ssl: true,
                trust_cert: false,
            },
            credential_state: CredentialState::Stored,
        };
        let profile_json = serde_json::to_value(profile).unwrap();
        assert_eq!(profile_json["descriptorId"], "descriptor-1");
        assert_eq!(profile_json["target"]["trustCert"], false);
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
            "ssl": false,
            "trustCert": false
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
            false,
            false,
        )
        .await;
        let err = match result {
            Err(e) => e,
            Ok(_) => panic!("expected connection to refused port to fail"),
        };
        assert!(err.starts_with("cannot connect to postgres:"), "got: {err}");
        // 真因（io 層）必須由 source() chain 帶出，而非只到泛稱 "error connecting to server"。
        assert!(
            err.to_lowercase().contains("refused"),
            "expected the OS-level cause to be surfaced, got: {err}"
        );
    }
}
