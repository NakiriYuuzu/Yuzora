//! The three drivers behind one face, [`Conn`].
//!
//! Everything here runs on the crate's tokio runtime (see `runtime.rs`). A
//! `Conn` is used by one task at a time (`&mut self`); what may happen from
//! elsewhere while it is busy — stopping a query — goes through its
//! [`Canceller`].

pub mod mssql;
pub mod postgres;
pub mod sqlite;
mod tls;

use std::time::Duration;

use crate::config::{ConnectionConfig, Engine};
use crate::error::DbResult;
use crate::model::{Cell, Column, DbObject, ObjectRef, RowEdit, TableInfo};

/// Everything needed to open one connection.
#[derive(Clone, Debug)]
pub struct ConnectSpec {
    pub config: ConnectionConfig,
    pub password: Option<String>,
    /// The database to open instead of `config.database` (the sidebar's picker).
    pub database: Option<String>,
    /// Where to connect instead of `config.host:port`: a tunnel's local end.
    pub endpoint: Option<(String, u16)>,
    pub connect_timeout: Duration,
}

impl ConnectSpec {
    pub fn new(config: ConnectionConfig, password: Option<String>) -> Self {
        Self { config, password, database: None, endpoint: None, connect_timeout: Duration::from_secs(15) }
    }

    pub fn host(&self) -> &str {
        self.endpoint.as_ref().map_or(self.config.host.trim(), |(h, _)| h.as_str())
    }

    pub fn port(&self) -> u16 {
        self.endpoint.as_ref().map_or(self.config.port(), |(_, p)| *p)
    }

    /// The database this connection opens (`None`: the server's default).
    pub fn database(&self) -> Option<&str> {
        self.database.as_deref().or(Some(self.config.database.trim())).filter(|d| !d.is_empty())
    }
}

/// Receives what one `execute` produces, statement by statement.
pub trait RowSink: Send {
    /// A statement that returns rows starts; `columns` describe them.
    fn columns(&mut self, columns: Vec<Column>);
    /// More rows of that statement. `false` asks the engine to stop reading
    /// (the row cap is reached, or the query was cancelled).
    fn rows(&mut self, rows: Vec<Vec<Cell>>) -> bool;
    /// The statement finished; `affected` for one that changed rows.
    fn done(&mut self, affected: Option<u64>);
    /// Stop was pressed. Engines look once their own cancel is armed and
    /// before each statement, so a stop pressed before a statement started
    /// (while the connection was still opening) still holds.
    fn cancelled(&self) -> bool {
        false
    }
}

/// Rows are handed to the sink in batches of this many.
pub const BATCH: usize = 512;

pub enum Conn {
    Postgres(postgres::PgConn),
    Mssql(mssql::MsConn),
    Sqlite(sqlite::LiteConn),
}

/// Stops the query a [`Conn`] is running, from another task.
#[derive(Clone)]
pub enum Canceller {
    Postgres(postgres::PgCanceller),
    Mssql(mssql::MsCanceller),
    Sqlite(sqlite::LiteCanceller),
}

impl Canceller {
    /// Best effort: the running statement ends with an error soon after.
    pub async fn cancel(&self) {
        match self {
            Canceller::Postgres(c) => c.cancel().await,
            Canceller::Mssql(c) => c.cancel().await,
            Canceller::Sqlite(c) => c.cancel(),
        }
    }
}

macro_rules! each {
    ($self:expr, $c:ident => $body:expr) => {
        match $self {
            Conn::Postgres($c) => $body,
            Conn::Mssql($c) => $body,
            Conn::Sqlite($c) => $body,
        }
    };
}

impl Conn {
    pub async fn connect(spec: &ConnectSpec) -> DbResult<Conn> {
        spec.config.validate()?;
        Ok(match spec.config.engine {
            Engine::Postgres => Conn::Postgres(postgres::PgConn::connect(spec).await?),
            Engine::Mssql => Conn::Mssql(mssql::MsConn::connect(spec).await?),
            Engine::Sqlite => Conn::Sqlite(sqlite::LiteConn::connect(spec).await?),
        })
    }

    pub fn engine(&self) -> Engine {
        match self {
            Conn::Postgres(_) => Engine::Postgres,
            Conn::Mssql(_) => Engine::Mssql,
            Conn::Sqlite(_) => Engine::Sqlite,
        }
    }

    /// `PostgreSQL 16.4`, `SQL Server 16.0`, `SQLite 3.46.0`.
    pub fn server_version(&self) -> String {
        each!(self, c => c.server_version())
    }

    /// The connection is gone (server closed it, or a cancel had to end it):
    /// the pool opens a new one next time.
    pub fn is_closed(&self) -> bool {
        each!(self, c => c.is_closed())
    }

    /// The database the connection was opened in (SQL Server, where a
    /// script's `USE` can move the session elsewhere).
    pub fn database(&self) -> Option<&str> {
        match self {
            Conn::Mssql(c) => Some(c.database()),
            Conn::Postgres(_) | Conn::Sqlite(_) => None,
        }
    }

    pub fn canceller(&self) -> Canceller {
        match self {
            Conn::Postgres(c) => Canceller::Postgres(c.canceller()),
            Conn::Mssql(c) => Canceller::Mssql(c.canceller()),
            Conn::Sqlite(c) => Canceller::Sqlite(c.canceller()),
        }
    }

    /// Databases on the server (SQLite: `main` and attached ones).
    pub async fn databases(&mut self) -> DbResult<Vec<String>> {
        each!(self, c => c.databases().await)
    }

    /// Tables, views and other objects of the open database, system schemas left out.
    pub async fn objects(&mut self) -> DbResult<Vec<DbObject>> {
        each!(self, c => c.objects().await)
    }

    pub async fn describe(&mut self, object: &ObjectRef) -> DbResult<TableInfo> {
        each!(self, c => c.describe(object).await)
    }

    /// Runs `sql` (one statement or several), feeding every result to `sink`.
    pub async fn execute(&mut self, sql: &str, sink: &mut dyn RowSink) -> DbResult<()> {
        each!(self, c => c.execute(sql, sink).await)
    }

    /// Writes `edits` to `table` in one transaction; every row must match
    /// exactly one row by its key, or nothing is written. Returns rows changed.
    pub async fn apply_edits(&mut self, table: &ObjectRef, edits: &[RowEdit]) -> DbResult<u64> {
        each!(self, c => c.apply_edits(table, edits).await)
    }
}

/// An identifier quoted for `engine`.
pub fn quote_ident(engine: Engine, ident: &str) -> String {
    match engine {
        Engine::Mssql => format!("[{}]", ident.replace(']', "]]")),
        Engine::Postgres | Engine::Sqlite => format!("\"{}\"", ident.replace('"', "\"\"")),
    }
}

/// `schema.name` quoted for `engine` (no schema part when it is empty).
pub fn qualified(engine: Engine, object: &ObjectRef) -> String {
    if object.schema.is_empty() {
        quote_ident(engine, &object.name)
    } else {
        format!("{}.{}", quote_ident(engine, &object.schema), quote_ident(engine, &object.name))
    }
}

/// `qualified`, in `database` too when it is given (SQL Server's
/// `[db].[schema].[name]`). Temporary tables (no schema) stay as they are.
pub fn qualified_in(engine: Engine, database: Option<&str>, object: &ObjectRef) -> String {
    match database.filter(|_| !object.schema.is_empty()) {
        Some(db) => format!("{}.{}", quote_ident(engine, db), qualified(engine, object)),
        None => qualified(engine, object),
    }
}

/// A table's rows; `database` from [`Conn::database`].
pub fn select_sql(
    engine: Engine,
    database: Option<&str>,
    table: &ObjectRef,
    order: Option<(&str, bool)>,
    limit: u64,
) -> String {
    let order = order
        .map(|(c, desc)| format!(" ORDER BY {}{}", quote_ident(engine, c), if desc { " DESC" } else { "" }))
        .unwrap_or_default();
    let table = qualified_in(engine, database, table);
    match engine {
        Engine::Mssql => format!("SELECT TOP ({limit}) * FROM {table}{order};"),
        _ => format!("SELECT * FROM {table}{order} LIMIT {limit};"),
    }
}

/// `SELECT count(*)` of a table in `engine`'s quoting; `database` as for [`select_sql`].
pub fn count_sql(engine: Engine, database: Option<&str>, table: &ObjectRef) -> String {
    let table = qualified_in(engine, database, table);
    match engine {
        Engine::Mssql => format!("SELECT COUNT_BIG(*) FROM {table};"),
        _ => format!("SELECT count(*) FROM {table};"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(schema: &str, name: &str) -> ObjectRef {
        ObjectRef { schema: schema.into(), name: name.into() }
    }

    #[test]
    fn identifiers_are_quoted_per_engine() {
        assert_eq!(quote_ident(Engine::Postgres, "a\"b"), "\"a\"\"b\"");
        assert_eq!(quote_ident(Engine::Mssql, "a]b"), "[a]]b]");
        assert_eq!(qualified(Engine::Sqlite, &t("", "x")), "\"x\"");
        assert_eq!(
            select_sql(Engine::Postgres, None, &t("public", "agent_sessions"), Some(("updated_at", true)), 500),
            "SELECT * FROM \"public\".\"agent_sessions\" ORDER BY \"updated_at\" DESC LIMIT 500;"
        );
        assert_eq!(select_sql(Engine::Mssql, None, &t("dbo", "t"), None, 10), "SELECT TOP (10) * FROM [dbo].[t];");
        assert_eq!(
            select_sql(Engine::Mssql, Some("app]db"), &t("dbo", "t"), None, 10),
            "SELECT TOP (10) * FROM [app]]db].[dbo].[t];"
        );
        assert_eq!(count_sql(Engine::Mssql, Some("app"), &t("", "#t")), "SELECT COUNT_BIG(*) FROM [#t];");
    }
}
