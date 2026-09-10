//! Typed SQLite lane. It is separate from the primary host control pipe.
use crate::db_service::*;
use crate::protocol::{ConnectionOwner, MAX_FRAME_BYTES, PROTOCOL_VERSION};
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::pin::Pin;

pub const MAX_DATABASE_REQUESTS: usize = 4;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SqliteWorkspace {
    pub host_id: String,
    pub canonical_path: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SqliteOpen {
    pub version: u32,
    pub owner: ConnectionOwner,
    pub workspace_path: String,
    pub database_path: String,
    pub identity: ConnectionIdentity,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SqliteRequest {
    pub version: u32,
    pub owner: ConnectionOwner,
    pub id: u64,
    pub call: SqliteCommand,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(
    tag = "method",
    content = "params",
    rename_all = "camelCase",
    deny_unknown_fields
)]
pub enum SqliteCommand {
    Probe,
    ListTables {
        identity: ConnectionIdentity,
    },
    TableColumns {
        identity: ConnectionIdentity,
        table: TableInfo,
    },
    QueryRun(QueryRunRequest),
    Cancel(QueryRunOwner),
    Page(ResultPageRequest),
    Release(ResultSessionOwner),
}

impl SqliteCommand {
    pub fn accepts(&self, result: &SqliteResult) -> bool {
        match (self, result) {
            (Self::Probe, SqliteResult::Version(_))
            | (Self::ListTables { .. }, SqliteResult::Tables(_))
            | (Self::TableColumns { .. }, SqliteResult::Columns(_))
            | (Self::Cancel(_), SqliteResult::Cancelled(_)) => true,
            (Self::Page(request), SqliteResult::Page(page)) => {
                request.owner == page.owner
                    && page.rows.len() <= crate::db_result_session::RESULT_PAGE_ROWS
            }
            (Self::Release(owner), SqliteResult::Page(page)) => {
                *owner == page.owner
                    && page.rows.len() <= crate::db_result_session::RESULT_PAGE_ROWS
            }
            (Self::QueryRun(request), SqliteResult::Run(run)) => {
                request.descriptor_id == run.descriptor_id
                    && request.connection_id == run.connection_id
                    && request.connection_generation == run.connection_generation
                    && request.query_run_id == run.query_run_id
                    && request.statements.len() == run.statements.len()
                    && run.validate_cardinality().is_ok()
                    && request
                        .statements
                        .iter()
                        .zip(run.statements.iter())
                        .all(|(request, result)| request.sql == result.sql)
            }
            _ => false,
        }
    }
}

impl SqliteRequest {
    pub fn validate(&self, owner: &ConnectionOwner, last_id: u64) -> Result<(), String> {
        if self.version != PROTOCOL_VERSION || &self.owner != owner {
            return Err("sqlite-connection-owner-mismatch".into());
        }
        if self.id <= last_id {
            return Err("sqlite-request-replayed-or-out-of-order".into());
        }
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "camelCase",
    deny_unknown_fields
)]
pub enum SqliteResult {
    Opened(ConnectionIdentity),
    Version(String),
    Tables(Vec<TableInfo>),
    Columns(Vec<ColumnInfo>),
    Run(QueryRun),
    Cancelled(QueryCancelResult),
    Page(ResultPage),
}

/// The local error envelope has static, redacted messages. Decode source-host
/// diagnostics as owned data and retain them in its structured error field.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SqliteError {
    pub code: DatabaseOperationalErrorCode,
    pub message: String,
    pub error: Option<DatabaseError>,
}
impl From<DatabaseOperationalError> for SqliteError {
    fn from(error: DatabaseOperationalError) -> Self {
        Self {
            code: error.code,
            message: error.message.into(),
            error: error.error.map(|error| *error),
        }
    }
}
impl From<SqliteError> for DatabaseOperationalError {
    fn from(error: SqliteError) -> Self {
        Self::new(error.code, "remote SQLite operation failed").with_database_error(
            error.error.unwrap_or(DatabaseError {
                engine: DatabaseErrorEngine::Yuzora,
                code: None,
                message: error.message,
                detail: None,
                hint: None,
                position: None,
                retryability: Retryability::Unknown,
            }),
        )
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SqliteReply {
    pub version: u32,
    pub owner: ConnectionOwner,
    pub id: u64,
    pub result: Result<SqliteResult, SqliteError>,
}

pub type SqliteFuture<'a> =
    Pin<Box<dyn Future<Output = Result<SqliteResult, DatabaseOperationalError>> + Send + 'a>>;

/// A desktop proxy has no SQLite handle. All actor/cursor ownership stays on
/// the source host; abort releases only this connection's helper lane.
pub trait RemoteSqlite: Send + Sync {
    fn request(&self, call: SqliteCommand) -> SqliteFuture<'_>;
    fn is_closed(&self) -> bool;
    fn abort(&self);
}

pub fn disconnected() -> DatabaseOperationalError {
    DatabaseOperationalError::new(
        DatabaseOperationalErrorCode::ServerDisconnected,
        "remote SQLite disconnected; the operation was not replayed",
    )
}
pub fn unexpected_reply() -> DatabaseOperationalError {
    DatabaseOperationalError::new(
        DatabaseOperationalErrorCode::QueryFailed,
        "invalid remote SQLite response",
    )
}

/// Bound serialization while writing, including JSON escaping, before a large
/// Vec can be allocated. Oversized replies are reported without replaying SQL.
pub fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, String> {
    struct Buffer(Vec<u8>);
    impl std::io::Write for Buffer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if self.0.len().saturating_add(bytes.len()) >= MAX_FRAME_BYTES {
                return Err(std::io::Error::other("sqlite-frame-too-large"));
            }
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut output = Buffer(Vec::new());
    serde_json::to_writer(&mut output, value).map_err(|e| e.to_string())?;
    output.0.push(b'\n');
    Ok(output.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn request_identity_replay_and_serialized_byte_limits_are_enforced() {
        let owner = ConnectionOwner {
            host_id: "host-a".into(),
            generation: 3,
        };
        let mut request = SqliteRequest {
            version: PROTOCOL_VERSION,
            owner: owner.clone(),
            id: 1,
            call: SqliteCommand::Probe,
        };
        assert!(request.validate(&owner, 0).is_ok());
        assert!(request.validate(&owner, 1).is_err());
        request.owner.host_id = "host-b".into();
        assert!(request.validate(&owner, 0).is_err());
        request.owner = owner.clone();
        request.owner.generation -= 1;
        assert!(request.validate(&owner, 0).is_err());
        request.owner = owner.clone();
        request.version += 1;
        assert!(request.validate(&owner, 0).is_err());
        // Escaping expands this string sixfold; the limit is on wire bytes.
        assert!(encode(&"\u{0001}".repeat(MAX_FRAME_BYTES / 6 + 1)).is_err());
    }
}
