//! What can go wrong, in words a person can act on.

use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DbError {
    /// The configuration is incomplete or contradictory (no host, bad port…).
    Config(String),
    /// Could not reach or log in to the server.
    Connect(String),
    /// TLS negotiation or certificate verification failed.
    Tls(String),
    /// The server rejected a statement.
    Query(String),
    /// Someone pressed stop, or the work was dropped.
    Cancelled,
    /// A connection, result or editor id that no longer exists.
    NotFound(String),
    /// Editing needs something the result does not have (a primary key…).
    NotEditable(String),
    /// The credential store refused.
    Secret(String),
    /// Reading or writing Yuzora's own files.
    Io(String),
}

impl fmt::Display for DbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DbError::Config(e)
            | DbError::Connect(e)
            | DbError::Tls(e)
            | DbError::Query(e)
            | DbError::NotFound(e)
            | DbError::NotEditable(e)
            | DbError::Secret(e)
            | DbError::Io(e) => f.write_str(e),
            DbError::Cancelled => f.write_str(&crate::i18n::t("error.cancelled", &[])),
        }
    }
}

impl std::error::Error for DbError {}

impl DbError {
    /// A stable name for scripts (`error.kind`).
    pub fn kind(&self) -> &'static str {
        match self {
            DbError::Config(_) => "config",
            DbError::Connect(_) => "connect",
            DbError::Tls(_) => "tls",
            DbError::Query(_) => "query",
            DbError::Cancelled => "cancelled",
            DbError::NotFound(_) => "not_found",
            DbError::NotEditable(_) => "not_editable",
            DbError::Secret(_) => "secret",
            DbError::Io(_) => "io",
        }
    }
}

impl From<yuzora_core::secrets::SecretError> for DbError {
    fn from(e: yuzora_core::secrets::SecretError) -> Self {
        DbError::Secret(e.to_string())
    }
}

impl From<std::io::Error> for DbError {
    fn from(e: std::io::Error) -> Self {
        DbError::Io(e.to_string())
    }
}

pub type DbResult<T> = Result<T, DbError>;
