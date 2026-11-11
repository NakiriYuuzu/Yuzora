//! SQLite through rusqlite (bundled).
//!
//! rusqlite is not async: the connection sits behind a mutex and every piece
//! of work runs on tokio's blocking pool. A query's rows travel back to the
//! async side over a small channel, so the sink is only ever called from the
//! task that called `execute`, and a slow sink holds the reading thread back
//! instead of piling rows up.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use rusqlite::types::{Value, ValueRef};
use rusqlite::{Connection, ErrorCode, InterruptHandle, OpenFlags, OptionalExtension};
use tokio::sync::mpsc;

use super::{BATCH, CommitSent, ConnectSpec, RowSink};
use crate::config::Engine;
use crate::error::{DbError, DbResult};
use crate::model::{
    Cell, Column, ColumnInfo, DbObject, ForeignKeyInfo, IndexInfo, ObjectKind, ObjectRef, RowEdit, TableInfo,
};
use crate::sql::{self, Statement};

/// How long a statement waits for another connection's lock on the file.
const LOCK_WAIT: Duration = Duration::from_secs(5);

pub struct LiteConn {
    db: Arc<Mutex<Connection>>,
    canceller: LiteCanceller,
    version: String,
}

/// Interrupts whatever the connection is running.
#[derive(Clone)]
pub struct LiteCanceller {
    handle: Arc<InterruptHandle>,
    /// SQLite forgets an interrupt that lands between two statements, so a
    /// script checks this before starting the next one.
    cancelled: Arc<AtomicBool>,
    /// How many stops there have been. A write-back runs on a thread nobody can
    /// take it back from, so it asks whether one came after it was asked for
    /// ([`LiteCanceller::since_now`]) instead of sharing a flag the next piece
    /// of work would reset.
    stops: Arc<AtomicU64>,
    pub(super) commit: CommitSent,
}

impl LiteCanceller {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        self.stops.fetch_add(1, Ordering::SeqCst);
        self.handle.interrupt();
    }

    /// A question to ask later: was there a stop since now?
    fn since_now(&self) -> Stopped {
        Stopped { stops: self.stops.clone(), seen: self.stops.load(Ordering::SeqCst) }
    }
}

struct Stopped {
    stops: Arc<AtomicU64>,
    seen: u64,
}

impl Stopped {
    fn is_set(&self) -> bool {
        self.stops.load(Ordering::SeqCst) != self.seen
    }
}

/// What the reading thread sends to the task that feeds the sink.
enum Msg {
    /// A statement that returns rows began, at this moment (see [`RowSink::started`]).
    Began(Instant),
    Columns(Vec<Column>),
    Rows(Vec<Vec<Cell>>),
    Done(Option<u64>),
}

impl LiteConn {
    pub async fn connect(spec: &ConnectSpec) -> DbResult<Self> {
        let path = spec.config.sqlite_path();
        let read_only = spec.config.read_only;
        blocking(move || {
            if !path.exists() {
                return Err(DbError::Connect(crate::i18n::t("engine.sqlite_missing", &[("path", path.display().to_string())])));
            }
            let access = if read_only { OpenFlags::SQLITE_OPEN_READ_ONLY } else { OpenFlags::SQLITE_OPEN_READ_WRITE };
            let conn = Connection::open_with_flags(&path, access | OpenFlags::SQLITE_OPEN_NO_MUTEX)
                .map_err(|e| DbError::Connect(format!("{}: {}", path.display(), message(&e))))?;
            conn.busy_timeout(LOCK_WAIT).map_err(lite_err)?;
            // Opening is lazy: this reads the header, so a file that is not a
            // database is refused here and not at the first query.
            conn.query_row("SELECT count(*) FROM sqlite_schema", [], |r| r.get::<_, i64>(0))
                .map_err(|e| DbError::Connect(format!("{}: {}", path.display(), message(&e))))?;
            let canceller = LiteCanceller {
                handle: Arc::new(conn.get_interrupt_handle()),
                cancelled: Arc::new(AtomicBool::new(false)),
                stops: Arc::new(AtomicU64::new(0)),
                commit: CommitSent::default(),
            };
            Ok(LiteConn {
                db: Arc::new(Mutex::new(conn)),
                canceller,
                version: format!("SQLite {}", rusqlite::version()),
            })
        })
        .await
    }

    pub fn server_version(&self) -> String {
        self.version.clone()
    }

    pub fn is_closed(&self) -> bool {
        false
    }

    pub fn canceller(&self) -> LiteCanceller {
        self.canceller.clone()
    }

    pub async fn databases(&mut self) -> DbResult<Vec<String>> {
        self.work(|conn| {
            let mut stmt = conn
                .prepare("SELECT name FROM pragma_database_list WHERE name <> 'temp' ORDER BY seq")
                .map_err(lite_err)?;
            let names = stmt.query_map([], |r| r.get::<_, String>(0)).map_err(lite_err)?;
            names.collect::<Result<_, _>>().map_err(lite_err)
        })
        .await
    }

    pub async fn objects(&mut self) -> DbResult<Vec<DbObject>> {
        self.work(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT type, name, tbl_name FROM sqlite_schema \
                     WHERE type IN ('table', 'view', 'index', 'trigger') AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\'",
                )
                .map_err(lite_err)?;
            let rows = stmt
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))
                .map_err(lite_err)?;
            let mut objects = Vec::new();
            for row in rows {
                let (kind, name, table) = row.map_err(lite_err)?;
                let (kind, detail) = match kind.as_str() {
                    "table" => (ObjectKind::Table, String::new()),
                    "view" => (ObjectKind::View, String::new()),
                    "index" => (ObjectKind::Index, table),
                    _ => (ObjectKind::Trigger, table),
                };
                objects.push(DbObject { schema: String::new(), name, kind, rows: None, detail });
            }
            sql::sort_objects(&mut objects);
            Ok(objects)
        })
        .await
    }

    pub async fn describe(&mut self, object: &ObjectRef) -> DbResult<TableInfo> {
        let name = object.name.clone();
        self.work(move |conn| describe(conn, &name)).await
    }

    pub async fn execute(&mut self, sql: &str, sink: &mut dyn RowSink) -> DbResult<()> {
        self.canceller.cancelled.store(false, Ordering::SeqCst);
        // Armed above: a stop from here on interrupts; one from before is the sink's.
        if sink.cancelled() {
            return Err(DbError::Cancelled);
        }
        let statements = sql::split(sql, Engine::Sqlite);
        let (tx, mut rx) = mpsc::channel::<Msg>(4);
        let stop = Arc::new(AtomicBool::new(false));
        let worker = {
            let (db, stop, cancelled) = (self.db.clone(), stop.clone(), self.canceller.cancelled.clone());
            tokio::task::spawn_blocking(move || run(&lock(&db), &statements, &tx, &stop, &cancelled))
        };
        while let Some(msg) = rx.recv().await {
            match msg {
                Msg::Began(at) => sink.started(at),
                Msg::Columns(columns) => sink.columns(columns),
                // The reading thread notices `stop` after its next batch; the
                // rest of what it sent is dropped here.
                Msg::Rows(rows) if !stop.load(Ordering::Relaxed) => {
                    if !sink.rows(rows) {
                        stop.store(true, Ordering::Relaxed);
                    }
                }
                Msg::Rows(_) => {}
                Msg::Done(affected) => sink.done(affected),
            }
        }
        worker.await.unwrap_or_else(|e| Err(DbError::Query(crate::i18n::t("engine.worker_stopped", &[("error", e.to_string())]))))
    }

    pub async fn apply_edits(&mut self, table: &ObjectRef, edits: &[RowEdit]) -> DbResult<u64> {
        let (table, edits) = (table.clone(), edits.to_vec());
        self.canceller.cancelled.store(false, Ordering::SeqCst);
        let stopped = self.canceller.since_now();
        let commit = self.canceller.commit.clone();
        self.work(move |conn| apply_edits(conn, &table, &edits, &stopped, &commit)).await
    }

    /// Runs `f` on the blocking pool with the connection.
    async fn work<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Connection) -> DbResult<T> + Send + 'static,
    ) -> DbResult<T> {
        let db = self.db.clone();
        blocking(move || f(&mut lock(&db))).await
    }
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> DbResult<T> + Send + 'static) -> DbResult<T> {
    tokio::task::spawn_blocking(f)
        .await
        .unwrap_or_else(|e| Err(DbError::Query(crate::i18n::t("engine.worker_stopped", &[("error", e.to_string())]))))
}

fn lock(db: &Mutex<Connection>) -> MutexGuard<'_, Connection> {
    db.lock().unwrap_or_else(|e| e.into_inner())
}

// ---------------------------------------------------------------- errors

/// SQLite's own words for what went wrong.
fn message(e: &rusqlite::Error) -> String {
    match e {
        rusqlite::Error::SqliteFailure(f, msg) => msg.clone().unwrap_or_else(|| f.to_string()),
        rusqlite::Error::SqlInputError { msg, offset, .. } => format!("{msg} (at offset {offset})"),
        other => other.to_string(),
    }
}

fn lite_err(e: rusqlite::Error) -> DbError {
    let interrupted = match &e {
        rusqlite::Error::SqliteFailure(f, _) => f.code == ErrorCode::OperationInterrupted,
        rusqlite::Error::SqlInputError { error, .. } => error.code == ErrorCode::OperationInterrupted,
        _ => false,
    };
    if interrupted { DbError::Cancelled } else { DbError::Query(message(&e)) }
}

// ---------------------------------------------------------------- execute

/// Runs the statements, sending what they produce; ends early when `stop` is
/// set or nobody listens any more.
fn run(
    conn: &Connection,
    statements: &[Statement],
    tx: &mpsc::Sender<Msg>,
    stop: &AtomicBool,
    cancelled: &AtomicBool,
) -> DbResult<()> {
    let send = |msg| tx.blocking_send(msg).is_ok();
    for statement in statements {
        if cancelled.load(Ordering::SeqCst) {
            return Err(DbError::Cancelled);
        }
        let began = Instant::now();
        let mut stmt = conn.prepare(&statement.text).map_err(lite_err)?;
        if stmt.column_count() == 0 {
            let changed = stmt.execute([]).map_err(lite_err)? as u64;
            if !send(Msg::Done(sql::changes_rows(&statement.text).then_some(changed))) {
                return Ok(());
            }
            continue;
        }
        if !send(Msg::Began(began)) {
            return Ok(());
        }
        let mut rows = stmt.query([]).map_err(lite_err)?;
        let mut batch = Vec::with_capacity(BATCH);
        // What the statement returns is known once it has run: if another connection changed the schema
        // since this one last read it, SQLite notices at the first step and compiles the statement
        // again, with other columns than it was prepared with.
        let mut width = 0;
        let mut columns_sent = false;
        while let Some(row) = rows.next().map_err(lite_err)? {
            if !columns_sent {
                let ran: &rusqlite::Statement = row.as_ref();
                width = ran.column_count();
                if !send(Msg::Columns(columns_of(ran))) {
                    return Ok(());
                }
                columns_sent = true;
            }
            batch.push((0..width).map(|i| cell(row.get_ref(i).unwrap_or(ValueRef::Null))).collect());
            if batch.len() == BATCH {
                if !send(Msg::Rows(std::mem::replace(&mut batch, Vec::with_capacity(BATCH))))
                    || stop.load(Ordering::Relaxed)
                {
                    return Ok(());
                }
            }
        }
        if !batch.is_empty() && (!send(Msg::Rows(batch)) || stop.load(Ordering::Relaxed)) {
            return Ok(());
        }
        drop(rows);
        if !columns_sent && !send(Msg::Columns(columns_of(&stmt))) {
            return Ok(());
        }
        if !send(Msg::Done(sql::changes_rows(&statement.text).then(|| conn.changes()))) {
            return Ok(());
        }
    }
    Ok(())
}

fn columns_of(stmt: &rusqlite::Statement<'_>) -> Vec<Column> {
    stmt.columns().map(|c| Column::new(c.name(), c.decl_type().unwrap_or(""))).collect()
}

fn cell(value: ValueRef<'_>) -> Cell {
    match value {
        ValueRef::Null => Cell::Null,
        ValueRef::Integer(i) => Cell::Int(i),
        ValueRef::Real(f) => Cell::Float(f),
        ValueRef::Text(t) => Cell::text(String::from_utf8_lossy(t)),
        ValueRef::Blob(b) => Cell::Bytes(b.into()),
    }
}

// ---------------------------------------------------------------- describe

fn describe(conn: &Connection, name: &str) -> DbResult<TableInfo> {
    let ddl: Option<String> = conn
        .query_row("SELECT sql FROM sqlite_schema WHERE name = ?1 AND type IN ('table', 'view')", [name], |r| r.get(0))
        .optional()
        .map_err(lite_err)?;
    let Some(mut ddl) = ddl else {
        return Err(DbError::NotFound(crate::i18n::t("engine.no_object", &[("name", name.to_string())])));
    };

    let mut stmt = conn
        .prepare(
            // hidden: 0 plain, 1 a virtual table's own, 2 and 3 generated.
            "SELECT name, type, \"notnull\", dflt_value, pk, hidden FROM pragma_table_xinfo(?1) \
             WHERE hidden IN (0, 2, 3) ORDER BY cid",
        )
        .map_err(lite_err)?;
    let columns = stmt
        .query_map([name], |r| {
            let pk: i64 = r.get(4)?;
            Ok(ColumnInfo {
                name: r.get(0)?,
                type_name: r.get(1)?,
                nullable: r.get::<_, i64>(2)? == 0,
                default: r.get(3)?,
                primary_key: u32::try_from(pk).ok().filter(|p| *p > 0),
                generated: r.get::<_, i64>(5)? >= 2,
            })
        })
        .map_err(lite_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(lite_err)?;

    let mut indexes = Vec::new();
    let mut stmt =
        conn.prepare("SELECT name, \"unique\", origin FROM pragma_index_list(?1) ORDER BY seq").map_err(lite_err)?;
    let listed = stmt
        .query_map([name], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? != 0, r.get::<_, String>(2)?)))
        .map_err(lite_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(lite_err)?;
    let mut stmt = conn.prepare("SELECT name FROM pragma_index_info(?1) ORDER BY seqno").map_err(lite_err)?;
    for (index, unique, origin) in listed {
        let columns = stmt
            .query_map([&index], |r| r.get::<_, Option<String>>(0))
            .map_err(lite_err)?
            .map(|c| c.map(|c| c.unwrap_or_else(|| "(expression)".to_owned())))
            .collect::<Result<Vec<_>, _>>()
            .map_err(lite_err)?;
        indexes.push(IndexInfo { name: index, columns, unique, primary: origin == "pk" });
    }

    let mut stmt = conn
        .prepare("SELECT id, \"table\", \"from\", \"to\" FROM pragma_foreign_key_list(?1) ORDER BY id, seq")
        .map_err(lite_err)?;
    let listed = stmt
        .query_map([name], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, Option<String>>(3)?))
        })
        .map_err(lite_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(lite_err)?;
    let mut foreign_keys: Vec<(i64, ForeignKeyInfo)> = Vec::new();
    for (id, table, from, to) in listed {
        if foreign_keys.last().is_none_or(|(last, _)| *last != id) {
            let info = ForeignKeyInfo {
                name: format!("fk_{id}"),
                columns: Vec::new(),
                referenced: ObjectRef { schema: String::new(), name: table },
                referenced_columns: Vec::new(),
            };
            foreign_keys.push((id, info));
        }
        let info = &mut foreign_keys.last_mut().expect("just pushed").1;
        info.columns.push(from);
        // `REFERENCES t` without columns means t's primary key.
        let position = info.referenced_columns.len();
        let to = match to {
            Some(to) => Some(to),
            None => primary_key_of(conn, &info.referenced.name)?.into_iter().nth(position),
        };
        info.referenced_columns.extend(to);
    }

    let mut stmt = conn
        .prepare(
            "SELECT sql FROM sqlite_schema WHERE type = 'index' AND tbl_name = ?1 AND sql IS NOT NULL ORDER BY name",
        )
        .map_err(lite_err)?;
    let index_sql = stmt
        .query_map([name], |r| r.get::<_, String>(0))
        .map_err(lite_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(lite_err)?;
    ddl.push(';');
    for sql in index_sql {
        ddl.push_str("\n\n");
        ddl.push_str(&sql);
        ddl.push(';');
    }

    Ok(TableInfo { columns, indexes, foreign_keys: foreign_keys.into_iter().map(|(_, f)| f).collect(), ddl })
}

fn primary_key_of(conn: &Connection, table: &str) -> DbResult<Vec<String>> {
    let mut stmt =
        conn.prepare("SELECT name FROM pragma_table_xinfo(?1) WHERE pk > 0 ORDER BY pk").map_err(lite_err)?;
    stmt.query_map([table], |r| r.get::<_, String>(0)).map_err(lite_err)?.collect::<Result<_, _>>().map_err(lite_err)
}

// ---------------------------------------------------------------- edits

fn apply_edits(
    conn: &mut Connection,
    table: &ObjectRef,
    edits: &[RowEdit],
    stopped: &Stopped,
    commit: &CommitSent,
) -> DbResult<u64> {
    if !conn.is_autocommit() {
        return Err(sql::open_transaction());
    }
    let tx = WriteTransaction::begin(conn, stopped)?;
    let mut written = 0;
    for edit in edits.iter().filter(|e| !e.changes.is_empty()) {
        if edit.key.is_empty() {
            return Err(DbError::NotEditable(crate::i18n::t("edit.no_key", &[])));
        }
        if edit.key.iter().any(|(_, v)| v.is_null()) {
            return Err(DbError::NotEditable(crate::i18n::t("edit.null_key", &[])));
        }
        if stopped.is_set() {
            return Err(DbError::Cancelled);
        }
        let sql = update_sql(table, edit);
        let params = edit.changes.iter().chain(&edit.key).map(|(_, c)| value(c));
        let changed = conn.execute(&sql, rusqlite::params_from_iter(params)).map_err(lite_err)?;
        if changed != 1 {
            // Dropping the transaction rolls it back.
            return Err(DbError::Query(sql::key_mismatch(&edit.key, changed as u64)));
        }
        written += 1;
    }
    // Marked before the last look for a stop, so a stop that finds the mark
    // unset is one this thread has not yet looked for, and gives way to.
    if !commit.mark() {
        return Err(DbError::Cancelled);
    }
    if stopped.is_set() {
        commit.reset();
        return Err(DbError::Cancelled);
    }
    tx.commit().map_err(lite_err)?;
    Ok(written)
}

/// A write transaction (`BEGIN IMMEDIATE`) that is rolled back unless committed.
struct WriteTransaction<'c> {
    conn: &'c Connection,
    open: bool,
}

impl<'c> WriteTransaction<'c> {
    /// SQLite's own wait for the write lock (`busy_timeout`) cannot be
    /// interrupted, and a write-back stopped in it would write the moment the
    /// lock is let go. So the wait is ours: a stop is looked for between tries.
    fn begin(conn: &'c Connection, stopped: &Stopped) -> DbResult<Self> {
        conn.busy_timeout(Duration::ZERO).map_err(lite_err)?;
        let waiting = Instant::now();
        let began = loop {
            match conn.execute_batch("BEGIN IMMEDIATE") {
                Ok(()) => break Ok(()),
                Err(e) if is_busy(&e) && !stopped.is_set() && waiting.elapsed() < LOCK_WAIT => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(e) => break Err(if stopped.is_set() { DbError::Cancelled } else { lite_err(e) }),
            }
        };
        conn.busy_timeout(LOCK_WAIT).map_err(lite_err)?;
        began.map(|()| Self { conn, open: true })
    }

    fn commit(mut self) -> rusqlite::Result<()> {
        self.conn.execute_batch("COMMIT")?;
        self.open = false;
        Ok(())
    }
}

impl Drop for WriteTransaction<'_> {
    fn drop(&mut self) {
        if self.open {
            // Not open any more when an interrupt already rolled it back.
            let _ = self.conn.execute_batch("ROLLBACK");
        }
    }
}

fn is_busy(e: &rusqlite::Error) -> bool {
    matches!(e, rusqlite::Error::SqliteFailure(f, _) if matches!(f.code, ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked))
}

/// `UPDATE "t" SET "a" = ?1 WHERE "k" = ?2`: changes first, then the key.
fn update_sql(table: &ObjectRef, edit: &RowEdit) -> String {
    let quote = |c: &str| super::quote_ident(Engine::Sqlite, c);
    let mut n = 0;
    let mut assign = |(column, _): &(String, Cell)| {
        n += 1;
        format!("{} = ?{n}", quote(column))
    };
    let set: Vec<_> = edit.changes.iter().map(&mut assign).collect();
    let keys: Vec<_> = edit.key.iter().map(&mut assign).collect();
    format!("UPDATE {} SET {} WHERE {}", super::qualified(Engine::Sqlite, table), set.join(", "), keys.join(" AND "))
}

fn value(cell: &Cell) -> Value {
    match cell {
        Cell::Null => Value::Null,
        Cell::Bool(b) => Value::Integer(i64::from(*b)),
        Cell::Int(i) => Value::Integer(*i),
        Cell::Float(f) => Value::Real(*f),
        Cell::Text(s) => Value::Text(s.to_string()),
        Cell::Bytes(b) => Value::Blob(b.to_vec()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn updates_set_the_changes_then_match_the_key() {
        let edit = RowEdit {
            key: vec![("id".into(), Cell::Int(1)), ("a\"b".into(), Cell::text("x"))],
            changes: vec![("name".into(), Cell::text("n")), ("age".into(), Cell::Null)],
        };
        let table = ObjectRef { schema: String::new(), name: "people".into() };
        assert_eq!(
            update_sql(&table, &edit),
            "UPDATE \"people\" SET \"name\" = ?1, \"age\" = ?2 WHERE \"id\" = ?3 AND \"a\"\"b\" = ?4"
        );
    }

    #[test]
    fn cells_become_sqlite_values() {
        assert_eq!(value(&Cell::Null), Value::Null);
        assert_eq!(value(&Cell::Bool(true)), Value::Integer(1));
        assert_eq!(value(&Cell::Float(1.5)), Value::Real(1.5));
        assert_eq!(value(&Cell::text("a")), Value::Text("a".into()));
        assert_eq!(value(&Cell::Bytes(vec![1, 2].into())), Value::Blob(vec![1, 2]));
        assert_eq!(cell(ValueRef::Text(b"hi")), Cell::text("hi"));
        assert_eq!(cell(ValueRef::Blob(&[0xff])), Cell::Bytes(vec![0xff].into()));
        assert_eq!(cell(ValueRef::Integer(7)), Cell::Int(7));
    }

    #[test]
    fn an_interrupt_is_a_cancel_and_the_rest_are_queries() {
        let interrupted = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error { code: ErrorCode::OperationInterrupted, extended_code: 9 },
            Some("interrupted".into()),
        );
        assert_eq!(lite_err(interrupted), DbError::Cancelled);
        let other = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error { code: ErrorCode::Unknown, extended_code: 1 },
            Some("no such table: x".into()),
        );
        assert_eq!(lite_err(other), DbError::Query("no such table: x".into()));
    }
}
