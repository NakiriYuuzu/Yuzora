//! The three drivers behind one face, [`Conn`].
//!
//! Everything here runs on the crate's tokio runtime (see `runtime.rs`). A
//! `Conn` is used by one task at a time (`&mut self`); what may happen from
//! elsewhere while it is busy — stopping a query — goes through its
//! [`Canceller`].

mod date_text;
pub mod mssql;
pub mod postgres;
pub mod sqlite;
mod tls;

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::config::{ConnectionConfig, Engine};
use crate::error::DbResult;
use crate::model::{Cell, Column, DbObject, ObjectRef, RowEdit, TableInfo};
use crate::results::WriteStop;

/// Everything needed to open one connection.
#[derive(Clone, Debug)]
pub struct ConnectSpec {
    pub config: ConnectionConfig,
    pub password: Option<String>,
    /// The database to open instead of `config.database` (the sidebar's picker).
    pub database: Option<String>,
    /// Where to connect instead of `config.host:port`: a tunnel's local end.
    pub endpoint: Option<(String, u16)>,
    /// PostgreSQL: the addresses to try, in this order, in place of looking the
    /// host up. The host still names the server for TLS.
    pub addresses: Option<Vec<SocketAddr>>,
    pub connect_timeout: Duration,
    /// The connection a table is read and written back on, which only this
    /// crate's statements run on (`session::table_owner`). PostgreSQL prints
    /// values the way they are read back there; any other connection keeps the
    /// server's settings.
    pub for_table: bool,
}

impl ConnectSpec {
    pub fn new(config: ConnectionConfig, password: Option<String>) -> Self {
        Self {
            config,
            password,
            database: None,
            endpoint: None,
            addresses: None,
            connect_timeout: Duration::from_secs(15),
            for_table: false,
        }
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

/// Whether a write-back has sent COMMIT. Before that its transaction was never
/// ended, so a stop leaves nothing in the database (the server rolls it back);
/// after it, the server alone decides, and a stop cannot say which way.
///
/// It also holds the write-back's own stop, which is pressed from another
/// thread and noticed by the write-back's task only when that gets to run: the
/// commit asks it directly ([`CommitSent::mark`]), so a Stop pressed before
/// COMMIT is sent is never overtaken by a task that has not looked yet.
#[derive(Clone, Debug, Default)]
pub struct CommitSent(Arc<CommitState>);

#[derive(Debug, Default)]
struct CommitState {
    sent: AtomicBool,
    stop: Mutex<Option<WriteStop>>,
}

impl CommitSent {
    /// A write-back starts: COMMIT has not been sent, and `stop` is the one
    /// that ends it.
    fn arm(&self, stop: &WriteStop) {
        *self.0.stop.lock().unwrap_or_else(|e| e.into_inner()) = Some(stop.clone());
        self.reset();
    }

    /// COMMIT has not been sent (a write-back starts, or it took its mark back).
    fn reset(&self) {
        self.0.sent.store(false, Ordering::SeqCst);
    }

    /// Called right before the COMMIT is sent, never after: a stop that
    /// reads this as unset is one that COMMIT cannot follow. `false` means a
    /// Stop was pressed first: the mark is taken back and COMMIT is not to be
    /// sent. The mark goes up before the Stop is looked for, so a Stop pressed
    /// after the look finds it set.
    fn mark(&self) -> bool {
        self.0.sent.store(true, Ordering::SeqCst);
        let pressed = self.0.stop.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(WriteStop::is_stopped);
        if pressed {
            self.reset();
        }
        !pressed
    }

    pub fn is_sent(&self) -> bool {
        self.0.sent.load(Ordering::SeqCst)
    }
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

    /// [`cancel`](Self::cancel) from where nothing can wait for it. What has to
    /// happen before this returns happens: SQL Server's socket is told to go and
    /// SQLite is interrupted at once, and PostgreSQL's request is counted on its
    /// way ([`Conn::settled`]).
    pub fn cancel_in_background(&self) {
        match self {
            Canceller::Postgres(c) => c.cancel_in_background(),
            Canceller::Mssql(c) => c.cancel_now(),
            Canceller::Sqlite(c) => c.cancel(),
        }
    }

    /// [`cancel`](Self::cancel) for work that is dropped where it stands (a
    /// write-back): PostgreSQL's connection is let go of afterwards rather than
    /// trusted to be clean. SQL Server's socket is already gone, and a SQLite
    /// interrupt cannot outlive the statement it was made for.
    pub async fn abandon(&self) {
        match self {
            Canceller::Postgres(c) => c.abandon().await,
            Canceller::Mssql(c) => c.cancel().await,
            Canceller::Sqlite(c) => c.cancel(),
        }
    }

    /// A write-back is about to run on the connection: COMMIT has not been
    /// sent yet, and `stop` is what ends it. Asked before anything of it can be
    /// stopped, so a stop never reads what the write-back before it left.
    pub fn write_back_starts(&self, stop: &WriteStop) {
        match self {
            Canceller::Postgres(c) => c.commit.arm(stop),
            Canceller::Mssql(c) => c.commit.arm(stop),
            Canceller::Sqlite(c) => c.commit.arm(stop),
        }
    }

    /// The write-back on this connection has sent COMMIT.
    pub fn commit_sent(&self) -> bool {
        match self {
            Canceller::Postgres(c) => c.commit.is_sent(),
            Canceller::Mssql(c) => c.commit.is_sent(),
            Canceller::Sqlite(c) => c.commit.is_sent(),
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

    /// The connection is gone (the server closed it, a cancel had to end it, or
    /// it was told to let go after a stop it could not see the end of): the pool
    /// opens a new one next time.
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

    /// Waits until no stop is on its way to this connection's server. PostgreSQL
    /// acts on a stop when the request gets there, and a statement started
    /// before that is the one it finds. SQL Server's stop is the socket itself
    /// and SQLite's an interrupt, both made where the stop is pressed.
    pub async fn settled(&self) {
        if let Conn::Postgres(c) = self {
            c.settled().await;
        }
    }

    /// Whether the stop last sent to this connection reached its statement. A
    /// stop that did not found the statement over; the server has dealt with it
    /// by then, but a server that answers before it acts (a pooler may) would
    /// leave a request still on its way, which may find a later statement: the
    /// connection is not safe to give to other work. SQL Server's stop is the
    /// socket itself, SQLite's an interrupt that only acts on a statement
    /// running when it is made.
    pub fn cancel_landed(&self) -> bool {
        match self {
            Conn::Postgres(c) => c.cancel_landed(),
            Conn::Mssql(_) | Conn::Sqlite(_) => true,
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

/// Said ahead of a table's SELECT, in the same batch (a round trip of its own is not spent).
/// A SQL Server login or server whose default opens a transaction at the first statement that
/// touches a table would leave one open after the read, and a write-back does not run beside
/// one. The connection a table is read and saved on is the app's alone (`session::table_owner`),
/// no SQL tab on it can end that transaction, so the app says it does not want one.
pub fn table_prelude(engine: Engine) -> &'static str {
    match engine {
        Engine::Mssql => "SET IMPLICIT_TRANSACTIONS OFF; ",
        _ => "",
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
