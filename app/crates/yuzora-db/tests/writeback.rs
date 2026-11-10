//! Write-back through the session, the way the table pane drives it: open a
//! table, change cells of the result it fills, write them back.
//!
//! SQLite always runs. PostgreSQL and SQL Server run when
//! `YUZORA_DATABASE_TEST_ENGINES` names them and the fixture of
//! `tests/database/docker-compose.yml` is up (`YUZORA_P8_DATABASE_PASSWORD`
//! overrides the fixture's password). Each of those tests makes a schema or a
//! database of its own and drops it again.

use std::path::PathBuf;
use std::sync::LazyLock;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::json;
use yuzora_db::config::{self, ConnectionConfig};
use yuzora_db::error::DbError;
use yuzora_db::model::{Cell, ObjectRef};
use yuzora_db::session::{self, JobId, JobState};
use yuzora_db::{results, runtime};

mod common;
use common::enabled;

/// The app home and the row cap are process-wide; every test starts here.
static HOME: LazyLock<PathBuf> = LazyLock::new(|| {
    let home = std::env::temp_dir().join(format!("yuzora-db-writeback-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    // SAFETY: first thing every test does, before it starts a thread of its own.
    unsafe {
        std::env::set_var("YUZORA_APP_HOME", &home);
        std::env::set_var("YUZORA_DB_SECRETS", "memory");
    }
    let mut section = yuzora_core::settings::section(config::SECTION);
    section["maxRows"] = json!(CAP);
    yuzora_core::settings::set_section(config::SECTION, section).unwrap();
    home
});

/// Rows a table read keeps (`db.maxRows`); the tables that must hit it hold far more.
const CAP: usize = 1000;

/// Ends the test early, passing, when `engine` is not selected.
macro_rules! only_with {
    ($engine:expr) => {
        if !enabled($engine) {
            eprintln!("skipped: {} is not in YUZORA_DATABASE_TEST_ENGINES", $engine);
            return;
        }
    };
}

fn password() -> String {
    std::env::var("YUZORA_P8_DATABASE_PASSWORD").unwrap_or_else(|_| "Yuzora-P8-Only-2026!".to_owned())
}

// ---------------------------------------------------------------- helpers

fn wait(job: JobId) -> JobState {
    let started = Instant::now();
    let mut seen = 0;
    while started.elapsed() < Duration::from_secs(60) {
        let state = runtime::block_on(session::job_after(job, seen)).expect("job");
        if !state.running {
            return state;
        }
        seen = state.version;
    }
    panic!("job {job} still running after 60 s");
}

/// Owners are process-wide and starting work for one drops what it had
/// running: tests that run side by side keep apart by connection.
fn owner(connection: &str, name: &str) -> String {
    format!("{name}@{connection}")
}

fn run(connection: &str, name: &str, sql: &str) -> JobState {
    let state = wait(session::start_query(connection, None, &owner(connection, name), sql));
    assert_eq!(state.error, None, "{sql}");
    state
}

/// The rows of a query's last result, as text.
fn rows(connection: &str, sql: &str) -> Vec<Vec<String>> {
    let state = run(connection, "check", sql);
    let set = results::get(state.results.last().expect("a result").id).expect("result");
    let r = results::lock(&set);
    r.rows.iter().map(|row| row.iter().map(|c| c.display().into_owned()).collect()).collect()
}

fn first_row(connection: &str, sql: &str) -> Vec<String> {
    rows(connection, sql).into_iter().next().unwrap_or_default()
}

fn table(schema: &str, name: &str) -> ObjectRef {
    serde_json::from_value(json!({"schema": schema, "name": name})).unwrap()
}

/// Saving a connection reads the list and writes it back: tests that run side
/// by side take turns.
fn save(config: serde_json::Value, password: Option<&str>) -> String {
    static TURN: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LazyLock::force(&HOME);
    let _turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
    config::save(serde_json::from_value::<ConnectionConfig>(config).unwrap(), password).unwrap().id
}

fn sqlite_connection(name: &str) -> (String, PathBuf) {
    LazyLock::force(&HOME);
    let file = HOME.join(format!("{name}.sqlite"));
    let _ = std::fs::remove_file(&file);
    std::fs::File::create(&file).unwrap();
    (save(json!({"name": name, "engine": "sqlite", "path": file}), None), file)
}

fn postgres_connection(name: &str) -> String {
    postgres_in(name, "yuzora_p8")
}

fn postgres_in(name: &str, database: &str) -> String {
    save(
        json!({"name": name, "engine": "postgres", "host": "127.0.0.1", "port": 55432,
               "user": "yuzora_full", "database": database, "ssl": "prefer"}),
        Some(&password()),
    )
}

/// `sa` in `database` (the fixture's login can't create tables of its own).
fn mssql_connection(name: &str, database: &str) -> String {
    save(
        json!({"name": name, "engine": "mssql", "host": "127.0.0.1", "port": 51433,
               "user": "sa", "database": database, "ssl": "prefer"}),
        Some(&password()),
    )
}

/// A SQL Server database of this test's own, dropped when it ends.
struct ScratchDatabase {
    name: String,
    master: String,
}

impl ScratchDatabase {
    fn new(name: &str) -> Self {
        let master = mssql_connection(&format!("ms-master-{name}"), "master");
        let scratch = ScratchDatabase { name: name.to_owned(), master };
        scratch.drop_it();
        run(&scratch.master, "scratch", &format!("CREATE DATABASE {name}"));
        scratch
    }

    fn connection(&self) -> String {
        mssql_connection(&format!("ms-{}", self.name), &self.name)
    }

    /// Drops the database; never panics (this also runs while a test unwinds).
    fn drop_it(&self) {
        let n = &self.name;
        let job = session::start_query(
            &self.master,
            None,
            &owner(&self.master, "scratch"),
            &format!(
                "IF DB_ID(N'{n}') IS NOT NULL BEGIN ALTER DATABASE {n} SET SINGLE_USER WITH ROLLBACK IMMEDIATE; DROP DATABASE {n} END"
            ),
        );
        let mut seen = 0;
        while let Ok(state) = runtime::block_on(session::job_after(job, seen)) {
            if !state.running {
                break;
            }
            seen = state.version;
        }
    }
}

impl Drop for ScratchDatabase {
    fn drop(&mut self) {
        self.drop_it();
    }
}

/// A PostgreSQL database of this test's own, dropped when it ends: a test that
/// terminates backends must not take its neighbours' connections with it.
struct PgScratchDatabase {
    name: String,
    maintenance: String,
}

impl PgScratchDatabase {
    fn new(name: &str) -> Self {
        let maintenance = postgres_in(&format!("pg-maintenance-{name}"), "postgres");
        let scratch = PgScratchDatabase { name: name.to_owned(), maintenance };
        scratch.drop_it();
        run(&scratch.maintenance, "scratch", &format!("CREATE DATABASE {name}"));
        scratch
    }

    fn connection(&self) -> String {
        postgres_in(&format!("pg-{}", self.name), &self.name)
    }

    /// Drops the database; never panics (this also runs while a test unwinds).
    fn drop_it(&self) {
        let job = session::start_query(
            &self.maintenance,
            None,
            &owner(&self.maintenance, "scratch"),
            &format!("DROP DATABASE IF EXISTS {} WITH (FORCE)", self.name),
        );
        let mut seen = 0;
        while let Ok(state) = runtime::block_on(session::job_after(job, seen)) {
            if !state.running {
                break;
            }
            seen = state.version;
        }
    }
}

impl Drop for PgScratchDatabase {
    fn drop(&mut self) {
        self.drop_it();
    }
}

/// A table opened for `owner` and edited like the grid does: the new text
/// becomes a cell of the kind the old one was.
struct Opened {
    owner: String,
    id: u64,
    set: std::sync::Arc<std::sync::Mutex<results::ResultSet>>,
}

fn open(connection: &str, name: &str, t: &ObjectRef) -> Opened {
    let owner = owner(connection, name);
    let state = wait(session::open_table(connection, None, &owner, t, None));
    assert_eq!(state.error, None);
    let summary = state.results.first().expect("a result");
    Opened { owner, id: summary.id, set: results::get(summary.id).expect("result") }
}

impl Opened {
    fn column(&self, name: &str) -> usize {
        results::lock(&self.set).columns.iter().position(|c| c.name == name).unwrap_or_else(|| panic!("no column {name}"))
    }

    /// The row whose `column` shows `value`.
    fn row_where(&self, column: &str, value: &str) -> usize {
        let at = self.column(column);
        let r = results::lock(&self.set);
        r.rows.iter().position(|row| row[at].display() == value).unwrap_or_else(|| panic!("no row with {column} = {value}"))
    }

    fn edit(&self, row: usize, column: &str, text: Option<&str>) {
        let at = self.column(column);
        let mut r = results::lock(&self.set);
        let cell = match text {
            None => Cell::Null,
            Some(text) => r.rows[row][at].edited(text.to_owned()),
        };
        r.set_edit(row, at, cell);
    }

    fn summary(&self) -> results::Summary {
        results::lock(&self.set).summary()
    }
}

/// Runs `work` on a thread; its result, or a panic naming `what` when it is
/// still going after `within`.
fn within<T: Send + 'static>(within: Duration, what: &str, work: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(work());
    });
    rx.recv_timeout(within).unwrap_or_else(|_| panic!("{what}: not done after {within:?}"))
}

fn until(what: &str, mut ready: impl FnMut() -> bool) {
    let started = Instant::now();
    while !ready() {
        assert!(started.elapsed() < Duration::from_secs(10), "{what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

// ---------------------------------------------------------------- a write-back that waits

/// Another connection holds `UPDATE`'s row; the write-back on the pane's
/// connection waits for it. The pane must show that it is waiting, stop on
/// request, and be usable afterwards.
fn write_back_waiting_on_a_lock(connection: &str, t: ObjectRef, begin_and_lock: &str, rollback: &str, sessions_waiting: &str) {
    let pane = open(connection, "lock-pane", &t);
    let row = pane.row_where("id", "1");
    pane.edit(row, "name", Some("mine"));
    // After the read: SQL Server readers wait for a writer's lock too.
    run(connection, "locker", begin_and_lock);
    let rid = pane.id;
    let set = pane.set.clone();
    let writer = std::thread::spawn(move || runtime::block_on(session::write_back_set(&set)));
    let release = || {
        run(connection, "locker", rollback);
    };
    until("the write-back never reported that it is waiting", || results::lock(&pane.set).is_writing());
    std::thread::sleep(Duration::from_millis(400));
    assert!(results::lock(&pane.set).is_writing(), "the lock is still held, the write-back must still be waiting");
    assert_eq!(results::lock(&pane.set).edits.len(), 1);

    session::stop_write(rid);
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(writer.join());
    });
    let stopped = match rx.recv_timeout(Duration::from_secs(5)) {
        Ok(joined) => joined.expect("the writer panicked"),
        Err(_) => {
            release();
            panic!("stopping the write-back did not end the wait for the lock");
        }
    };
    // The server has dropped the statement while the lock is still held: the
    // stop is not just the pane forgetting it.
    let still_waiting = (0..50).any(|_| {
        let waiting = first_row(connection, sessions_waiting) != ["0"];
        if waiting {
            std::thread::sleep(Duration::from_millis(100));
        }
        !waiting
    });
    release();
    assert!(still_waiting, "the server is still waiting for the lock after the stop");
    assert_eq!(stopped, Err(DbError::Cancelled));
    {
        let r = results::lock(&pane.set);
        assert!(!r.is_writing());
        assert_eq!(
            r.write_failure().map(str::to_owned),
            Some(yuzora_db::i18n::t("edit.stopped", &[])),
            "stopped on a lock, so nothing was sent to commit"
        );
        assert_eq!(r.edits.len(), 1, "the edit stays pending");
    }
    assert_eq!(first_row(connection, &format!("SELECT name FROM {} WHERE id = 1", qualified(&t, connection))), ["a"]);

    // The pane's connection is not left stuck behind the stopped statement.
    let written = within(Duration::from_secs(10), "write-back after a stop", move || {
        runtime::block_on(session::write_back(rid))
    });
    assert_eq!(written, Ok(1));
    assert_eq!(first_row(connection, &format!("SELECT name FROM {} WHERE id = 1", qualified(&t, connection))), ["mine"]);
}

fn qualified(t: &ObjectRef, connection: &str) -> String {
    match config::get(connection).unwrap().engine {
        config::Engine::Mssql => format!("[{}].[{}]", t.schema, t.name),
        config::Engine::Postgres => format!("\"{}\".\"{}\"", t.schema, t.name),
        config::Engine::Sqlite => format!("\"{}\"", t.name),
    }
}

#[test]
fn sqlite_a_write_back_queued_behind_the_connection_shows_it_and_can_be_stopped() {
    let (id, _) = sqlite_connection("wb-queued");
    run(&id, "setup", "create table people(id integer primary key, name text); insert into people values (1, 'a'), (2, 'b');");
    let pane = open(&id, "queued-pane", &table("", "people"));
    pane.edit(pane.row_where("id", "1"), "name", Some("mine"));

    let held = runtime::block_on(session::lease(&id, None, &pane.owner)).unwrap();
    let rid = pane.id;
    let set = pane.set.clone();
    let writer = std::thread::spawn(move || runtime::block_on(session::write_back_set(&set)));
    until("the write-back never reported that it is waiting", || results::lock(&pane.set).is_writing());
    assert_eq!(results::lock(&pane.set).edits.len(), 1, "the edits are still there while waiting");

    session::stop_write(rid);
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(writer.join());
    });
    let stopped = match rx.recv_timeout(Duration::from_secs(3)) {
        Ok(joined) => joined.expect("the writer panicked"),
        Err(_) => {
            drop(held);
            panic!("stopping the write-back did not end the wait for the connection");
        }
    };
    assert_eq!(stopped, Err(DbError::Cancelled));
    {
        let r = results::lock(&pane.set);
        assert!(!r.is_writing());
        assert_eq!(
            r.write_failure().map(str::to_owned),
            Some(yuzora_db::i18n::t("edit.stopped", &[])),
            "{:?}",
            r.write
        );
        assert_eq!(r.edits.len(), 1, "the edit stays pending");
    }
    assert_eq!(rows(&id, "select name from people where id = 1"), [["a"]], "nothing was written");
    drop(held);
    assert_eq!(runtime::block_on(session::write_back(rid)), Ok(1));
    assert_eq!(rows(&id, "select name from people where id = 1"), [["mine"]]);
}

#[test]
fn postgres_a_write_back_waiting_on_a_row_lock_can_be_stopped() {
    only_with!("postgres");
    let id = postgres_connection("pg-lock");
    run(&id, "setup", "DROP SCHEMA IF EXISTS yz_wb_lock CASCADE; CREATE SCHEMA yz_wb_lock; CREATE TABLE yz_wb_lock.t(id int PRIMARY KEY, name text); INSERT INTO yz_wb_lock.t VALUES (1, 'a'), (2, 'b');");
    write_back_waiting_on_a_lock(
        &id,
        table("yz_wb_lock", "t"),
        "BEGIN; UPDATE yz_wb_lock.t SET name = 'locked' WHERE id = 1",
        "ROLLBACK",
        "SELECT count(*)::text FROM pg_stat_activity WHERE wait_event_type = 'Lock' AND datname = 'yuzora_p8'",
    );
    run(&id, "setup", "DROP SCHEMA yz_wb_lock CASCADE");
}

#[test]
fn mssql_a_write_back_waiting_on_a_row_lock_can_be_stopped() {
    only_with!("mssql");
    let scratch = ScratchDatabase::new("yz_wb_lock");
    let id = scratch.connection();
    run(&id, "setup", "CREATE TABLE dbo.t(id int PRIMARY KEY, name nvarchar(20)); INSERT INTO dbo.t VALUES (1, N'a'), (2, N'b');");
    write_back_waiting_on_a_lock(
        &id,
        table("dbo", "t"),
        "BEGIN TRANSACTION; UPDATE dbo.t SET name = N'locked' WHERE id = 1",
        "ROLLBACK TRANSACTION",
        "SELECT CAST(count(*) AS varchar(5)) FROM sys.dm_exec_requests WHERE blocking_session_id <> 0 AND database_id = DB_ID(N'yz_wb_lock')",
    );
}

// ---------------------------------------------------------------- a connection the server dropped

/// The pane's connection was closed by the server while it sat idle (a
/// firewall, a restart, `KILL`): the first write-back after must not fail for
/// that, nothing having been written yet.
fn write_back_after_the_server_dropped_the_connection(connection: &str, t: ObjectRef, kill_the_others: &str, check: &str) {
    let pane = open(connection, "dropped-pane", &t);
    pane.edit(pane.row_where("id", "1"), "name", Some("mine"));
    run(connection, "killer", kill_the_others);
    std::thread::sleep(Duration::from_millis(500));
    let written = within(Duration::from_secs(30), "the write-back after a dropped connection", {
        let set = pane.set.clone();
        move || runtime::block_on(session::write_back_set(&set))
    });
    assert_eq!(written, Ok(1), "{:?}", results::lock(&pane.set).write);
    assert_eq!(first_row(connection, check), ["mine"]);
}

#[test]
fn postgres_a_write_back_after_the_server_dropped_the_connection_still_saves() {
    only_with!("postgres");
    let scratch = PgScratchDatabase::new("yz_wb_dropped");
    let id = scratch.connection();
    run(&id, "setup", "CREATE TABLE t(id int PRIMARY KEY, name text); INSERT INTO t VALUES (1, 'a');");
    write_back_after_the_server_dropped_the_connection(
        &id,
        table("public", "t"),
        "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = 'yz_wb_dropped' AND application_name = 'Yuzora' AND pid <> pg_backend_pid()",
        "SELECT name FROM t WHERE id = 1",
    );
}

#[test]
fn mssql_a_write_back_after_the_server_dropped_the_connection_still_saves() {
    only_with!("mssql");
    let scratch = ScratchDatabase::new("yz_wb_dropped");
    let id = scratch.connection();
    run(&id, "setup", "CREATE TABLE dbo.t(id int PRIMARY KEY, name nvarchar(20)); INSERT INTO dbo.t VALUES (1, N'a');");
    write_back_after_the_server_dropped_the_connection(
        &id,
        table("dbo", "t"),
        "DECLARE @kill nvarchar(max) = N''; SELECT @kill += N'KILL ' + CAST(session_id AS nvarchar(10)) + N';' FROM sys.dm_exec_sessions WHERE is_user_process = 1 AND database_id = DB_ID(N'yz_wb_dropped') AND session_id <> @@SPID AND program_name = N'Yuzora'; EXEC(@kill)",
        "SELECT name FROM dbo.t WHERE id = 1",
    );
}

#[test]
fn mssql_reading_the_schema_after_the_server_dropped_the_catalog_connection_still_works() {
    only_with!("mssql");
    let scratch = ScratchDatabase::new("yz_wb_catalog");
    let id = scratch.connection();
    run(&id, "setup", "CREATE TABLE dbo.t(id int PRIMARY KEY, name nvarchar(20));");
    let t = table("dbo", "t");
    let described = runtime::block_on(session::describe(&id, None, &t)).expect("the first read");
    assert_eq!(described.key_columns(), ["id"]);
    run(
        &id,
        "killer",
        "DECLARE @kill nvarchar(max) = N''; SELECT @kill += N'KILL ' + CAST(session_id AS nvarchar(10)) + N';' FROM sys.dm_exec_sessions WHERE is_user_process = 1 AND database_id = DB_ID(N'yz_wb_catalog') AND session_id <> @@SPID AND program_name = N'Yuzora'; EXEC(@kill)",
    );
    std::thread::sleep(Duration::from_millis(500));
    let again = within(Duration::from_secs(30), "the schema read after a dropped connection", move || {
        runtime::block_on(session::describe(&id, None, &t))
    });
    assert_eq!(again.map(|info| info.key_columns()), Ok(vec!["id".to_owned()]));
}

// ---------------------------------------------------------------- a write-back that fails

#[test]
fn a_result_the_pane_moved_on_from_still_says_why_the_write_back_failed() {
    let (id, _) = sqlite_connection("wb-gone");
    run(&id, "setup", "create table people(id integer primary key, name text); insert into people values (1, 'a');");
    let pane = open(&id, "gone-pane", &table("", "people"));
    pane.edit(0, "name", Some("mine"));
    // The pane starts something else: its old result is dropped from the store.
    run(&id, "gone-pane", "select 1");
    assert!(results::get(pane.id).is_none());

    let out = runtime::block_on(session::write_back_set(&pane.set));
    assert!(out.is_err(), "{out:?}");
    let r = results::lock(&pane.set);
    assert!(
        r.write_failure().is_some_and(|m| !m.is_empty()),
        "the grid that holds the result must be able to show why: {:?}",
        r.write
    );
    assert_eq!(r.edits.len(), 1, "the edit stays pending");
    drop(r);
    assert_eq!(rows(&id, "select name from people"), [["a"]]);

    // An id nobody has any more is an error with words, not an empty one.
    match runtime::block_on(session::write_back(pane.id)) {
        Err(DbError::NotFound(message)) => assert!(!message.is_empty()),
        other => panic!("{other:?}"),
    }
}

#[test]
fn every_result_that_cannot_be_edited_says_why() {
    let (id, file) = sqlite_connection("wb-reasons");
    run(&id, "setup", "create table keyed(id integer primary key, v text); create table loose(v text); insert into keyed values (1, 'a');");
    let said = |summary: &results::Summary| summary.not_editable.clone().filter(|m| !m.is_empty());

    let query = run(&id, "reasons", "select * from keyed");
    assert!(!query.results[0].editable);
    assert!(said(&query.results[0]).is_some(), "a query's rows are read-only for a reason: {:?}", query.results[0]);

    let state = wait(session::open_table(&id, None, &owner(&id, "reasons"), &table("", "loose"), None));
    assert!(said(&state.results[0]).is_some(), "{:?}", state.results[0]);

    let read_only = save(json!({"name": "wb-reasons-ro", "engine": "sqlite", "path": file, "readOnly": true}), None);
    let state = wait(session::open_table(&read_only, None, &owner(&read_only, "reasons-ro"), &table("", "keyed"), None));
    assert!(said(&state.results[0]).is_some(), "{:?}", state.results[0]);

    let state = wait(session::open_table(&id, None, &owner(&id, "reasons"), &table("", "keyed"), None));
    assert!(state.results[0].editable && state.results[0].not_editable.is_none());
}

// ---------------------------------------------------------------- editing before the count

/// When a table's rows were all in (the cap was hit), when it became
/// editable, and when its `count(*)` came in.
struct Timeline {
    editable: Option<Duration>,
    total: Option<Duration>,
    rows: usize,
    truncated: bool,
}

fn observe(job: JobId) -> Timeline {
    let started = Instant::now();
    let mut line = Timeline { editable: None, total: None, rows: 0, truncated: false };
    loop {
        std::thread::sleep(Duration::from_millis(1));
        let at = started.elapsed();
        let state = session::job_state(job).expect("job");
        if let Some(first) = state.results.first() {
            let summary = results::lock(&results::get(first.id).expect("result")).summary();
            (line.rows, line.truncated) = (summary.rows, summary.truncated);
            if summary.editable && line.editable.is_none() {
                line.editable = Some(at);
            }
            if summary.total.is_some() && line.total.is_none() {
                line.total = Some(at);
            }
        }
        if !state.running {
            return line;
        }
        assert!(at < Duration::from_secs(120), "table still loading after 120 s");
    }
}

fn editable_before_the_count(connection: &str, t: ObjectRef) {
    let line = observe(session::open_table(connection, None, &owner(connection, "big-pane"), &t, None));
    assert_eq!((line.rows, line.truncated), (CAP, true), "the table is bigger than the cap");
    let (editable, total) = (line.editable.expect("editable at the end"), line.total.expect("a count"));
    assert!(editable < total, "editable only at {editable:?}, when the count was already in at {total:?}");
}

#[test]
fn sqlite_a_big_table_is_editable_before_its_count_arrives() {
    let (id, _) = sqlite_connection("wb-big");
    run(
        &id,
        "setup",
        "create table big(id integer primary key, v text);
         with recursive c(x) as (select 1 union all select x + 1 from c where x < 3000000) insert into big select x, 'v' from c;",
    );
    editable_before_the_count(&id, table("", "big"));
}

#[test]
fn postgres_a_big_table_is_editable_before_its_count_arrives() {
    only_with!("postgres");
    let id = postgres_connection("pg-big");
    run(&id, "setup", "DROP SCHEMA IF EXISTS yz_wb_big CASCADE; CREATE SCHEMA yz_wb_big; CREATE TABLE yz_wb_big.big AS SELECT g AS id, 'v'::text AS v FROM generate_series(1, 4000000) g; ALTER TABLE yz_wb_big.big ADD PRIMARY KEY (id);");
    editable_before_the_count(&id, table("yz_wb_big", "big"));
    run(&id, "setup", "DROP SCHEMA yz_wb_big CASCADE");
}

#[test]
fn mssql_a_big_table_is_editable_before_its_count_arrives() {
    only_with!("mssql");
    let scratch = ScratchDatabase::new("yz_wb_big");
    let id = scratch.connection();
    run(
        &id,
        "setup",
        "CREATE TABLE dbo.big(id int PRIMARY KEY, v nvarchar(10))\nGO\n\
         INSERT INTO dbo.big SELECT TOP (1500000) ROW_NUMBER() OVER (ORDER BY (SELECT NULL)), N'v' FROM sys.all_columns a CROSS JOIN sys.all_columns b",
    );
    editable_before_the_count(&id, table("dbo", "big"));
}

/// The count has a connection of its own, so a slow one (here: one that is
/// still being waited for) holds back neither editing nor a write-back, and
/// Stop reaches it.
#[test]
fn sqlite_editing_and_writing_back_do_not_wait_for_the_count() {
    let (id, _) = sqlite_connection("wb-count");
    run(&id, "setup", "create table people(id integer primary key, name text); insert into people values (1, 'a');");
    let pane_owner = owner(&id, "count-pane");
    let held = runtime::block_on(session::lease(&id, None, &session::count_owner(&pane_owner))).unwrap();

    let job = session::open_table(&id, None, &pane_owner, &table("", "people"), None);
    let mut found = None;
    until("the table never became editable while its count was waiting", || {
        let state = session::job_state(job).expect("job");
        found = state.results.first().map(|r| r.id).filter(|rid| results::get(*rid).is_some_and(|s| results::lock(&s).editable()));
        found.is_some()
    });
    let set = results::get(found.unwrap()).unwrap();
    let state = session::job_state(job).unwrap();
    assert!(state.running, "the job is still counting");
    assert_eq!(results::lock(&set).total, None);

    results::lock(&set).set_edit(0, 1, Cell::text("mine"));
    let written = within(Duration::from_secs(5), "a write-back while the count waits", {
        let set = set.clone();
        move || runtime::block_on(session::write_back_set(&set))
    });
    assert_eq!(written, Ok(1));
    assert_eq!(results::lock(&set).total, None, "the count is still out");

    drop(held);
    let state = wait(job);
    assert_eq!(state.error, None);
    assert_eq!(results::lock(&set).total, Some(1));
    assert_eq!(rows(&id, "select name from people"), [["mine"]]);
}

#[test]
fn sqlite_a_count_that_was_stopped_never_runs() {
    let (id, _) = sqlite_connection("wb-count-stop");
    run(&id, "setup", "create table people(id integer primary key, name text); insert into people values (1, 'a');");
    let pane_owner = owner(&id, "count-stop-pane");
    let held = runtime::block_on(session::lease(&id, None, &session::count_owner(&pane_owner))).unwrap();
    let job = session::open_table(&id, None, &pane_owner, &table("", "people"), None);
    let mut found = None;
    until("the table never became editable", || {
        found = session::job_state(job).and_then(|s| s.results.first().filter(|r| r.editable).map(|r| r.id));
        found.is_some()
    });
    session::cancel(job);
    drop(held);
    let state = wait(job);
    assert!(state.cancelled, "{state:?}");
    assert_eq!(results::lock(&results::get(found.unwrap()).unwrap()).total, None, "a stopped job does not count");
}

// ---------------------------------------------------------------- what the server returns around the UPDATE

#[test]
fn mssql_a_trigger_that_returns_rows_does_not_hide_the_row_count() {
    only_with!("mssql");
    let scratch = ScratchDatabase::new("yz_wb_trigger");
    let id = scratch.connection();
    run(
        &id,
        "setup",
        "CREATE TABLE dbo.t(id int PRIMARY KEY, v nvarchar(10))\nGO\n\
         CREATE TRIGGER dbo.t_noisy ON dbo.t AFTER UPDATE AS BEGIN SELECT 1 AS noisy END\nGO\n\
         INSERT INTO dbo.t VALUES (1, N'a')",
    );
    let pane = open(&id, "trigger-pane", &table("dbo", "t"));
    pane.edit(0, "v", Some("zz"));
    assert_eq!(runtime::block_on(session::write_back_set(&pane.set)), Ok(1));
    assert_eq!(first_row(&id, "SELECT v FROM dbo.t"), ["zz"]);
}

/// The driver can't decode some column types (CLR types, sql_variant). Opening
/// such a table must end with that said, not leave the pane loading for ever.
#[test]
fn mssql_a_table_with_a_column_the_driver_cannot_read_ends_with_an_error() {
    only_with!("mssql");
    let scratch = ScratchDatabase::new("yz_wb_types");
    let id = scratch.connection();
    run(
        &id,
        "setup",
        "CREATE TABLE dbo.geo(id int PRIMARY KEY, g geography, v nvarchar(10)); INSERT INTO dbo.geo VALUES (1, geography::Point(1, 1, 4326), N'a');
         CREATE TABLE dbo.tree(id int PRIMARY KEY, h hierarchyid); INSERT INTO dbo.tree VALUES (1, hierarchyid::Parse('/1/'));
         CREATE TABLE dbo.sv(id int PRIMARY KEY, x sql_variant); INSERT INTO dbo.sv VALUES (1, CAST(5 AS int));",
    );
    for (name, what) in [("geo", "geography"), ("tree", "hierarchyid"), ("sv", "sql_variant")] {
        let connection = id.clone();
        let t = table("dbo", name);
        let state = within(Duration::from_secs(20), &format!("opening the table with {what}"), move || {
            wait(session::open_table(&connection, None, &owner(&connection, &format!("types-{name}")), &t, None))
        });
        let error = state.error.unwrap_or_else(|| panic!("{what}: the job ended without saying it can't read the table"));
        assert!(error.contains("unsupported column type"), "{what}: {error}");
    }
    // The connection is still good for what the driver can read.
    assert_eq!(first_row(&id, "SELECT CAST(id AS varchar(5)) FROM dbo.geo"), ["1"]);
}

// ---------------------------------------------------------------- the column types, through the session

/// What a case expects of its write-back.
enum Expect {
    /// Written; `check` then reads back these values.
    Written(&'static [&'static str]),
    /// Refused by the server with a message holding this.
    Refused(&'static str),
}

struct Case {
    name: &'static str,
    setup: &'static str,
    schema: &'static str,
    table: &'static str,
    /// (row, column, new text; `None` is NULL)
    edits: Vec<(usize, &'static str, Option<&'static str>)>,
    check: &'static str,
    expect: Expect,
}

fn go(connection: &str, cases: Vec<Case>) {
    let mut failed = Vec::new();
    for (n, case) in cases.iter().enumerate() {
        run(connection, "setup", case.setup);
        let pane = open(connection, &format!("matrix-pane-{n}"), &table(case.schema, case.table));
        if !pane.summary().editable {
            failed.push(format!("{}: not editable ({:?})", case.name, pane.summary().not_editable));
            continue;
        }
        for (row, column, text) in &case.edits {
            pane.edit(*row, column, *text);
        }
        let wanted = case.edits.len();
        let kept = results::lock(&pane.set).edits.len();
        if kept != wanted {
            failed.push(format!("{}: {kept} of {wanted} edits kept", case.name));
            continue;
        }
        let written = runtime::block_on(session::write_back_set(&pane.set));
        match (&case.expect, written) {
            (Expect::Written(values), Ok(_)) => {
                let got = first_row(connection, case.check);
                if got != *values {
                    failed.push(format!("{}: read back {got:?}, wanted {values:?}", case.name));
                }
            }
            (Expect::Written(_), Err(e)) => failed.push(format!("{}: refused: {e}", case.name)),
            (Expect::Refused(part), Err(e)) => {
                if !e.to_string().contains(part) {
                    failed.push(format!("{}: refused with {e}, wanted {part:?}", case.name));
                }
                if results::lock(&pane.set).write_failure().is_none() {
                    failed.push(format!("{}: refused, but the result shows no reason", case.name));
                }
            }
            (Expect::Refused(part), Ok(_)) => failed.push(format!("{}: written, wanted a refusal with {part:?}", case.name)),
        }
        session::release_owner(&pane.owner);
    }
    assert!(failed.is_empty(), "{} of {} cases failed:\n{}", failed.len(), cases.len(), failed.join("\n"));
}

#[test]
fn postgres_writes_every_kind_of_column_back_through_the_session() {
    only_with!("postgres");
    let id = postgres_connection("pg-matrix");
    run(&id, "setup", "DROP SCHEMA IF EXISTS yz_wb_matrix CASCADE; DROP SCHEMA IF EXISTS yz_wb_matrix_b CASCADE; CREATE SCHEMA yz_wb_matrix; CREATE TYPE yz_wb_matrix.mood AS ENUM ('sad', 'happy');");
    let s = "yz_wb_matrix";
    let big = Box::leak("x".repeat(200_000).into_boxed_str());
    let cases = vec![
        Case { name: "uuid key", setup: "CREATE TABLE yz_wb_matrix.u(id uuid PRIMARY KEY DEFAULT gen_random_uuid(), name text); INSERT INTO yz_wb_matrix.u(name) VALUES ('a'), ('b');", schema: s, table: "u", edits: vec![(0, "name", Some("zz"))], check: "SELECT count(*)::text FROM yz_wb_matrix.u WHERE name = 'zz'", expect: Expect::Written(&["1"]) },
        Case { name: "numeric and numeric key", setup: "CREATE TABLE yz_wb_matrix.n(id numeric(12,3) PRIMARY KEY, price numeric(12,2)); INSERT INTO yz_wb_matrix.n VALUES (1.5, 1), (2, 2);", schema: s, table: "n", edits: vec![(0, "price", Some("19.99"))], check: "SELECT price::text FROM yz_wb_matrix.n WHERE id = 1.5", expect: Expect::Written(&["19.99"]) },
        Case { name: "date and time types", setup: "CREATE TABLE yz_wb_matrix.ts(id int PRIMARY KEY, t timestamptz, t2 timestamp, d date, tm time, iv interval); INSERT INTO yz_wb_matrix.ts VALUES (1, now(), now(), now(), now(), '1 day');", schema: s, table: "ts", edits: vec![(0, "t", Some("2024-03-01 10:00:00+08")), (0, "t2", Some("2024-03-01 10:00:00.123456")), (0, "d", Some("2024-03-02")), (0, "tm", Some("11:22:33")), (0, "iv", Some("2 days 03:00:00"))], check: "SELECT (t AT TIME ZONE 'UTC')::text, t2::text, d::text, tm::text, iv::text FROM yz_wb_matrix.ts", expect: Expect::Written(&["2024-03-01 02:00:00", "2024-03-01 10:00:00.123456", "2024-03-02", "11:22:33", "2 days 03:00:00"]) },
        Case { name: "timestamp keys", setup: "CREATE TABLE yz_wb_matrix.tk(id timestamptz PRIMARY KEY, v text); INSERT INTO yz_wb_matrix.tk VALUES ('2024-02-29 12:34:56.123456+00', 'a');", schema: s, table: "tk", edits: vec![(0, "v", Some("zz"))], check: "SELECT v FROM yz_wb_matrix.tk", expect: Expect::Written(&["zz"]) },
        Case { name: "json, bytea, arrays, enum, bool", setup: "CREATE TABLE yz_wb_matrix.m(id int PRIMARY KEY, j jsonb, b bytea, ia int[], ta text[], m yz_wb_matrix.mood, f bool); INSERT INTO yz_wb_matrix.m VALUES (1, '{}', '\\x00', '{1}', '{a}', 'sad', false);", schema: s, table: "m", edits: vec![(0, "j", Some("[1,2]")), (0, "b", Some("\\xdeadbeef")), (0, "ia", Some("{1,2,3}")), (0, "ta", Some("{\"x y\",z}")), (0, "m", Some("happy")), (0, "f", Some("1"))], check: "SELECT j::text, encode(b, 'hex'), ia::text, ta::text, m::text, f::text FROM yz_wb_matrix.m", expect: Expect::Written(&["[1, 2]", "deadbeef", "{1,2,3}", "{\"x y\",z}", "happy", "true"]) },
        Case { name: "reserved and mixed-case names", setup: "CREATE TABLE yz_wb_matrix.\"Order\"(\"Id\" int PRIMARY KEY, \"Group\" text, \"select\" text, \"a\" int, \"A\" int); INSERT INTO yz_wb_matrix.\"Order\" VALUES (1, 'g', 's', 1, 2);", schema: s, table: "Order", edits: vec![(0, "Group", Some("G2")), (0, "select", Some("S2")), (0, "A", Some("9"))], check: "SELECT \"Group\", \"select\", \"a\"::text, \"A\"::text FROM yz_wb_matrix.\"Order\"", expect: Expect::Written(&["G2", "S2", "1", "9"]) },
        Case { name: "spaces, Chinese, quotes, emoji", setup: "CREATE TABLE yz_wb_matrix.\"訂單 明細\"(\"編 號\" int PRIMARY KEY, \"名稱 欄\" text, \"q\"\"uote\" text); INSERT INTO yz_wb_matrix.\"訂單 明細\" VALUES (1, '甲', 'x');", schema: s, table: "訂單 明細", edits: vec![(0, "名稱 欄", Some("乙 😀")), (0, "q\"uote", Some("y"))], check: "SELECT \"名稱 欄\", \"q\"\"uote\" FROM yz_wb_matrix.\"訂單 明細\"", expect: Expect::Written(&["乙 😀", "y"]) },
        Case { name: "composite key", setup: "CREATE TABLE yz_wb_matrix.cp(a int, b text, v text, PRIMARY KEY (a, b)); INSERT INTO yz_wb_matrix.cp VALUES (1, 'x', 'v'), (1, 'y', 'w');", schema: s, table: "cp", edits: vec![(1, "v", Some("zz"))], check: "SELECT count(*)::text FROM yz_wb_matrix.cp WHERE v = 'zz'", expect: Expect::Written(&["1"]) },
        Case { name: "identity and generated columns", setup: "CREATE TABLE yz_wb_matrix.ig(id int GENERATED ALWAYS AS IDENTITY PRIMARY KEY, a int, tw int GENERATED ALWAYS AS (a * 2) STORED); INSERT INTO yz_wb_matrix.ig(a) VALUES (4);", schema: s, table: "ig", edits: vec![(0, "a", Some("5"))], check: "SELECT tw::text FROM yz_wb_matrix.ig", expect: Expect::Written(&["10"]) },
        Case { name: "editing the key itself", setup: "CREATE TABLE yz_wb_matrix.sp(id serial PRIMARY KEY, v text); INSERT INTO yz_wb_matrix.sp(v) VALUES ('a'), ('b');", schema: s, table: "sp", edits: vec![(0, "id", Some("100"))], check: "SELECT count(*)::text FROM yz_wb_matrix.sp WHERE id = 100", expect: Expect::Written(&["1"]) },
        Case { name: "NULL and the empty string", setup: "CREATE TABLE yz_wb_matrix.ne(id int PRIMARY KEY, a text, b text); INSERT INTO yz_wb_matrix.ne VALUES (1, 'x', NULL);", schema: s, table: "ne", edits: vec![(0, "a", None), (0, "b", Some(""))], check: "SELECT coalesce(a, '<null>'), coalesce(b, '<null>') || '|' FROM yz_wb_matrix.ne", expect: Expect::Written(&["<null>", "|"]) },
        Case { name: "text keys with quotes and backslashes", setup: "CREATE TABLE yz_wb_matrix.tp(k text PRIMARY KEY, v text); INSERT INTO yz_wb_matrix.tp VALUES ('it''s \"q\" \\ back  ', 'a'), ('中文 😀', 'b');", schema: s, table: "tp", edits: vec![(0, "v", Some("zz")), (1, "v", Some("yy"))], check: "SELECT count(*)::text FROM yz_wb_matrix.tp WHERE v IN ('zz', 'yy')", expect: Expect::Written(&["2"]) },
        Case { name: "float keys", setup: "CREATE TABLE yz_wb_matrix.fp(k real PRIMARY KEY, v text); INSERT INTO yz_wb_matrix.fp VALUES (0.1, 'a'), (1.0/3, 'b');", schema: s, table: "fp", edits: vec![(0, "v", Some("zz")), (1, "v", Some("yy"))], check: "SELECT count(*)::text FROM yz_wb_matrix.fp WHERE v IN ('zz', 'yy')", expect: Expect::Written(&["2"]) },
        Case { name: "other types as text", setup: "CREATE TABLE yz_wb_matrix.mi(id int PRIMARY KEY, m money, i inet, x xml, b bit(3), r int4range, pt point); INSERT INTO yz_wb_matrix.mi VALUES (1, 1, '10.0.0.1', '<a/>', '101', '[1,5)', '(1,2)');", schema: s, table: "mi", edits: vec![(0, "m", Some("$12.50")), (0, "i", Some("192.168.1.1")), (0, "x", Some("<b/>")), (0, "b", Some("011")), (0, "r", Some("[2,7)")), (0, "pt", Some("(3,4)"))], check: "SELECT i::text, x::text, b::text, r::text, pt::text FROM yz_wb_matrix.mi", expect: Expect::Written(&["192.168.1.1/32", "<b/>", "011", "[2,7)", "(3,4)"]) },
        Case { name: "partitioned table", setup: "CREATE TABLE yz_wb_matrix.pt(id int, v text, PRIMARY KEY (id)) PARTITION BY RANGE (id); CREATE TABLE yz_wb_matrix.pt_1 PARTITION OF yz_wb_matrix.pt FOR VALUES FROM (0) TO (100); INSERT INTO yz_wb_matrix.pt VALUES (1, 'a');", schema: s, table: "pt", edits: vec![(0, "v", Some("zz"))], check: "SELECT v FROM yz_wb_matrix.pt", expect: Expect::Written(&["zz"]) },
        Case { name: "BEFORE UPDATE trigger", setup: "CREATE TABLE yz_wb_matrix.bt(id int PRIMARY KEY, v text, upd int DEFAULT 0); CREATE FUNCTION yz_wb_matrix.bt_f() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN NEW.upd := OLD.upd + 1; RETURN NEW; END $$; CREATE TRIGGER bt BEFORE UPDATE ON yz_wb_matrix.bt FOR EACH ROW EXECUTE FUNCTION yz_wb_matrix.bt_f(); INSERT INTO yz_wb_matrix.bt(id, v) VALUES (1, 'a');", schema: s, table: "bt", edits: vec![(0, "v", Some("zz"))], check: "SELECT v, upd::text FROM yz_wb_matrix.bt", expect: Expect::Written(&["zz", "1"]) },
        Case { name: "a 200 000 character value", setup: "CREATE TABLE yz_wb_matrix.lv(id int PRIMARY KEY, v text); INSERT INTO yz_wb_matrix.lv VALUES (1, 'a');", schema: s, table: "lv", edits: vec![(0, "v", Some(big))], check: "SELECT length(v)::text FROM yz_wb_matrix.lv", expect: Expect::Written(&["200000"]) },
        Case { name: "several rows and cells", setup: "CREATE TABLE yz_wb_matrix.mm(id int PRIMARY KEY, a text, b int); INSERT INTO yz_wb_matrix.mm VALUES (1, 'a', 1), (2, 'b', 2), (3, 'c', 3);", schema: s, table: "mm", edits: vec![(0, "a", Some("A")), (1, "b", Some("20")), (2, "a", None), (2, "b", Some("30"))], check: "SELECT string_agg(coalesce(a, 'N') || b, ',' ORDER BY id) FROM yz_wb_matrix.mm", expect: Expect::Written(&["A1,b20,N30"]) },
        Case { name: "a value the column refuses", setup: "CREATE TABLE yz_wb_matrix.vl(id int PRIMARY KEY, v varchar(3)); INSERT INTO yz_wb_matrix.vl VALUES (1, 'a');", schema: s, table: "vl", edits: vec![(0, "v", Some("toolong"))], check: "SELECT v FROM yz_wb_matrix.vl", expect: Expect::Refused("value too long") },
        Case { name: "a table that is in two schemas", setup: "CREATE SCHEMA yz_wb_matrix_b; CREATE TABLE yz_wb_matrix_b.t(id int PRIMARY KEY, v text); INSERT INTO yz_wb_matrix_b.t VALUES (1, 'b'); CREATE TABLE yz_wb_matrix.t(id int PRIMARY KEY, v text); INSERT INTO yz_wb_matrix.t VALUES (1, 'a');", schema: "yz_wb_matrix_b", table: "t", edits: vec![(0, "v", Some("zz"))], check: "SELECT v FROM yz_wb_matrix.t", expect: Expect::Written(&["a"]) },
    ];
    go(&id, cases);
    run(&id, "setup", "DROP SCHEMA yz_wb_matrix CASCADE; DROP SCHEMA yz_wb_matrix_b CASCADE");
}

#[test]
fn mssql_writes_every_kind_of_column_back_through_the_session() {
    only_with!("mssql");
    let scratch = ScratchDatabase::new("yz_wb_matrix");
    let id = scratch.connection();
    let s = "dbo";
    let big = Box::leak("x".repeat(200_000).into_boxed_str());
    let cases = vec![
        Case { name: "identity key", setup: "CREATE TABLE dbo.idt(id int IDENTITY(1,1) PRIMARY KEY, v nvarchar(20)); INSERT INTO dbo.idt(v) VALUES (N'a'), (N'b');", schema: s, table: "idt", edits: vec![(0, "v", Some("zz"))], check: "SELECT v FROM dbo.idt WHERE id = 1", expect: Expect::Written(&["zz"]) },
        Case { name: "uniqueidentifier key", setup: "CREATE TABLE dbo.g(id uniqueidentifier PRIMARY KEY DEFAULT NEWID(), v nvarchar(20)); INSERT INTO dbo.g(v) VALUES (N'a'), (N'b');", schema: s, table: "g", edits: vec![(0, "v", Some("zz")), (1, "v", Some("yy"))], check: "SELECT CAST(count(*) AS varchar(5)) FROM dbo.g WHERE v IN (N'zz', N'yy')", expect: Expect::Written(&["2"]) },
        Case { name: "decimal and money columns", setup: "CREATE TABLE dbo.dm(id int PRIMARY KEY, d decimal(12,2), n numeric(10,4), m money, sm smallmoney, f float, r real); INSERT INTO dbo.dm VALUES (1, 1.5, 1.5, 1.5, 1.5, 1.5, 1.5);", schema: s, table: "dm", edits: vec![(0, "d", Some("19.99")), (0, "n", Some("0.1234")), (0, "m", Some("1234.5678")), (0, "sm", Some("12.5")), (0, "f", Some("2.5")), (0, "r", Some("0.1"))], check: "SELECT CAST(d AS varchar(20)), CAST(n AS varchar(20)), CONVERT(varchar(20), m, 2), CAST(sm AS varchar(20)), CAST(f AS varchar(20)), CAST(r AS varchar(20)) FROM dbo.dm", expect: Expect::Written(&["19.99", "0.1234", "1234.5678", "12.50", "2.5", "0.1"]) },
        Case { name: "decimal key", setup: "CREATE TABLE dbo.dpk(id decimal(12,3) PRIMARY KEY, v nvarchar(10)); INSERT INTO dbo.dpk VALUES (1.5, N'a'), (2, N'b');", schema: s, table: "dpk", edits: vec![(0, "v", Some("zz"))], check: "SELECT v FROM dbo.dpk WHERE id = 1.5", expect: Expect::Written(&["zz"]) },
        Case { name: "date and time columns", setup: "CREATE TABLE dbo.dt(id int PRIMARY KEY, a datetime, b datetime2(7), c smalldatetime, d date, e time(7), f datetimeoffset(7)); INSERT INTO dbo.dt VALUES (1, GETDATE(), SYSDATETIME(), GETDATE(), GETDATE(), SYSDATETIME(), SYSDATETIMEOFFSET());", schema: s, table: "dt", edits: vec![(0, "a", Some("2024-03-01 10:00:00.123")), (0, "b", Some("2024-03-01 10:00:00.1234567")), (0, "c", Some("2024-03-01 10:01:00")), (0, "d", Some("2024-03-02")), (0, "e", Some("11:22:33.1234567")), (0, "f", Some("2024-03-01 10:00:00.1234567 +08:00"))], check: "SELECT CONVERT(varchar(30), a, 121), CONVERT(varchar(30), b, 121), CONVERT(varchar(30), c, 121), CONVERT(varchar(30), d, 121), CONVERT(varchar(30), e, 121), CONVERT(varchar(40), f, 121) FROM dbo.dt", expect: Expect::Written(&["2024-03-01 10:00:00.123", "2024-03-01 10:00:00.1234567", "2024-03-01 10:01:00.000", "2024-03-02", "11:22:33.1234567", "2024-03-01 10:00:00.1234567 +08:00"]) },
        Case { name: "date and time keys", setup: "CREATE TABLE dbo.dk1(id datetime PRIMARY KEY, v nvarchar(10)); INSERT INTO dbo.dk1 VALUES ('2024-02-29 12:34:56.123', N'a'), ('2024-02-29 12:34:56.127', N'b'); CREATE TABLE dbo.dk2(id datetime2(7) PRIMARY KEY, v nvarchar(10)); INSERT INTO dbo.dk2 VALUES ('2024-02-29 12:34:56.1234567', N'a'); CREATE TABLE dbo.dk3(id datetimeoffset(3) PRIMARY KEY, v nvarchar(10)); INSERT INTO dbo.dk3 VALUES ('2024-02-29 12:34:56.123 +08:00', N'a');", schema: s, table: "dk1", edits: vec![(0, "v", Some("zz")), (1, "v", Some("yy"))], check: "SELECT CAST(count(*) AS varchar(5)) FROM dbo.dk1 WHERE v IN (N'zz', N'yy')", expect: Expect::Written(&["2"]) },
        Case { name: "datetime2 key", setup: "SELECT 1", schema: s, table: "dk2", edits: vec![(0, "v", Some("zz"))], check: "SELECT v FROM dbo.dk2", expect: Expect::Written(&["zz"]) },
        Case { name: "datetimeoffset key", setup: "SELECT 1", schema: s, table: "dk3", edits: vec![(0, "v", Some("zz"))], check: "SELECT v FROM dbo.dk3", expect: Expect::Written(&["zz"]) },
        Case { name: "bit", setup: "CREATE TABLE dbo.bt(id int PRIMARY KEY, f bit, g bit); INSERT INTO dbo.bt VALUES (1, 0, 0);", schema: s, table: "bt", edits: vec![(0, "f", Some("1")), (0, "g", Some("true"))], check: "SELECT CAST(f AS varchar(1)) + CAST(g AS varchar(1)) FROM dbo.bt", expect: Expect::Written(&["11"]) },
        Case { name: "varchar and nvarchar, Chinese and emoji", setup: "CREATE TABLE dbo.vc(id int PRIMARY KEY, v varchar(20) COLLATE Chinese_Taiwan_Stroke_CI_AS, w nvarchar(20)); INSERT INTO dbo.vc VALUES (1, N'a', N'a');", schema: s, table: "vc", edits: vec![(0, "v", Some("乙")), (0, "w", Some("乙 😀"))], check: "SELECT v, w FROM dbo.vc", expect: Expect::Written(&["乙", "乙 😀"]) },
        Case { name: "text keys", setup: "CREATE TABLE dbo.vpk(k nvarchar(20) PRIMARY KEY, v nvarchar(10)); INSERT INTO dbo.vpk VALUES (N'中文 😀', N'a'), (N'it''s [x]', N'b'); CREATE TABLE dbo.cpk(k char(6) PRIMARY KEY, v nvarchar(10)); INSERT INTO dbo.cpk VALUES ('ab', N'a');", schema: s, table: "vpk", edits: vec![(0, "v", Some("zz")), (1, "v", Some("yy"))], check: "SELECT CAST(count(*) AS varchar(5)) FROM dbo.vpk WHERE v IN (N'zz', N'yy')", expect: Expect::Written(&["2"]) },
        Case { name: "a padded char key", setup: "SELECT 1", schema: s, table: "cpk", edits: vec![(0, "v", Some("zz"))], check: "SELECT v FROM dbo.cpk", expect: Expect::Written(&["zz"]) },
        Case { name: "computed column", setup: "CREATE TABLE dbo.cc(id int PRIMARY KEY, a int, tw AS (a * 2)); INSERT INTO dbo.cc VALUES (1, 4);", schema: s, table: "cc", edits: vec![(0, "a", Some("5"))], check: "SELECT CAST(tw AS varchar(10)) FROM dbo.cc", expect: Expect::Written(&["10"]) },
        Case { name: "rowversion column", setup: "CREATE TABLE dbo.rv(id int PRIMARY KEY, v nvarchar(10), rv rowversion); INSERT INTO dbo.rv(id, v) VALUES (1, N'a');", schema: s, table: "rv", edits: vec![(0, "v", Some("zz"))], check: "SELECT v FROM dbo.rv", expect: Expect::Written(&["zz"]) },
        Case { name: "text, ntext, xml, nvarchar(max)", setup: "CREATE TABLE dbo.tx(id int PRIMARY KEY, x xml, t text, nt ntext, nm nvarchar(max)); INSERT INTO dbo.tx VALUES (1, '<a/>', 't', N'nt', N'm');", schema: s, table: "tx", edits: vec![(0, "x", Some("<b/>")), (0, "t", Some("tt")), (0, "nt", Some("ntt")), (0, "nm", Some("mm"))], check: "SELECT CAST(x AS nvarchar(20)), CAST(t AS varchar(20)), CAST(nt AS nvarchar(20)), nm FROM dbo.tx", expect: Expect::Written(&["<b/>", "tt", "ntt", "mm"]) },
        Case { name: "brackets, spaces and Chinese in names", setup: "CREATE TABLE dbo.[訂單 明細]([編 號] int PRIMARY KEY, [名稱 ]] 欄] nvarchar(20)); INSERT INTO dbo.[訂單 明細] VALUES (1, N'甲');", schema: s, table: "訂單 明細", edits: vec![(0, "名稱 ] 欄", Some("乙 😀"))], check: "SELECT [名稱 ]] 欄] FROM dbo.[訂單 明細]", expect: Expect::Written(&["乙 😀"]) },
        Case { name: "reserved words", setup: "CREATE TABLE dbo.[order]([select] int PRIMARY KEY, [group] nvarchar(10)); INSERT INTO dbo.[order] VALUES (1, N'g');", schema: s, table: "order", edits: vec![(0, "group", Some("G2"))], check: "SELECT [group] FROM dbo.[order]", expect: Expect::Written(&["G2"]) },
        Case { name: "a table that is in two schemas", setup: "EXEC('CREATE SCHEMA yz'); CREATE TABLE yz.t(id int PRIMARY KEY, v nvarchar(10)); INSERT INTO yz.t VALUES (1, N'a'); CREATE TABLE dbo.t(id int PRIMARY KEY, v nvarchar(10)); INSERT INTO dbo.t VALUES (1, N'dbo');", schema: "yz", table: "t", edits: vec![(0, "v", Some("zz"))], check: "SELECT v FROM dbo.t", expect: Expect::Written(&["dbo"]) },
        Case { name: "composite key", setup: "CREATE TABLE dbo.cp(a int, b nvarchar(5), v nvarchar(5), PRIMARY KEY (a, b)); INSERT INTO dbo.cp VALUES (1, N'x', N'v'), (1, N'y', N'w');", schema: s, table: "cp", edits: vec![(1, "v", Some("zz"))], check: "SELECT CAST(count(*) AS varchar(5)) FROM dbo.cp WHERE v = N'zz'", expect: Expect::Written(&["1"]) },
        Case { name: "NULL and the empty string", setup: "CREATE TABLE dbo.ne(id int PRIMARY KEY, a nvarchar(10), b nvarchar(10)); INSERT INTO dbo.ne VALUES (1, N'x', NULL);", schema: s, table: "ne", edits: vec![(0, "a", None), (0, "b", Some(""))], check: "SELECT ISNULL(a, N'<null>'), ISNULL(b, N'<null>') + N'|' FROM dbo.ne", expect: Expect::Written(&["<null>", "|"]) },
        Case { name: "integer widths", setup: "CREATE TABLE dbo.it(id tinyint PRIMARY KEY, a smallint, b int, c bigint); INSERT INTO dbo.it VALUES (1, 1, 1, 1);", schema: s, table: "it", edits: vec![(0, "a", Some("300")), (0, "b", Some("70000")), (0, "c", Some("5000000000"))], check: "SELECT CAST(a AS varchar(20)), CAST(b AS varchar(20)), CAST(c AS varchar(20)) FROM dbo.it", expect: Expect::Written(&["300", "70000", "5000000000"]) },
        Case { name: "a float typed into an int column", setup: "CREATE TABLE dbo.it2(id int PRIMARY KEY, b int); INSERT INTO dbo.it2 VALUES (1, 1);", schema: s, table: "it2", edits: vec![(0, "b", Some("7.5"))], check: "SELECT CAST(b AS varchar(20)) FROM dbo.it2", expect: Expect::Refused("Msg 245") },
        Case { name: "a value that is too long", setup: "CREATE TABLE dbo.tl(id int PRIMARY KEY, v nvarchar(3)); INSERT INTO dbo.tl VALUES (1, N'a');", schema: s, table: "tl", edits: vec![(0, "v", Some("toolong"))], check: "SELECT v FROM dbo.tl", expect: Expect::Refused("truncated") },
        Case { name: "AFTER and INSTEAD OF triggers", setup: "CREATE TABLE dbo.tr(id int PRIMARY KEY, v nvarchar(10), upd int DEFAULT 0)\nGO\nCREATE TRIGGER dbo.tr_u ON dbo.tr AFTER UPDATE AS BEGIN UPDATE t SET upd = t.upd + 1 FROM dbo.tr t JOIN inserted i ON i.id = t.id END\nGO\nINSERT INTO dbo.tr(id, v) VALUES (1, N'a')\nGO\nCREATE TABLE dbo.tio(id int PRIMARY KEY, v nvarchar(10))\nGO\nCREATE TRIGGER dbo.tio_u ON dbo.tio INSTEAD OF UPDATE AS BEGIN UPDATE t SET v = i.v FROM dbo.tio t JOIN inserted i ON i.id = t.id END\nGO\nINSERT INTO dbo.tio(id, v) VALUES (1, N'a')", schema: s, table: "tr", edits: vec![(0, "v", Some("zz"))], check: "SELECT v, CAST(upd AS varchar(5)) FROM dbo.tr", expect: Expect::Written(&["zz", "1"]) },
        Case { name: "an INSTEAD OF trigger", setup: "SELECT 1", schema: s, table: "tio", edits: vec![(0, "v", Some("zz"))], check: "SELECT v FROM dbo.tio", expect: Expect::Written(&["zz"]) },
        Case { name: "a temporal table", setup: "CREATE TABLE dbo.tm(id int PRIMARY KEY, v nvarchar(10), s datetime2 GENERATED ALWAYS AS ROW START NOT NULL, e datetime2 GENERATED ALWAYS AS ROW END NOT NULL, PERIOD FOR SYSTEM_TIME (s, e)) WITH (SYSTEM_VERSIONING = ON (HISTORY_TABLE = dbo.tm_hist)); INSERT INTO dbo.tm(id, v) VALUES (1, N'a');", schema: s, table: "tm", edits: vec![(0, "v", Some("zz"))], check: "SELECT v FROM dbo.tm", expect: Expect::Written(&["zz"]) },
        Case { name: "a 200 000 character value", setup: "CREATE TABLE dbo.lv(id int PRIMARY KEY, v nvarchar(max)); INSERT INTO dbo.lv VALUES (1, N'a');", schema: s, table: "lv", edits: vec![(0, "v", Some(big))], check: "SELECT CAST(LEN(v) AS varchar(10)) FROM dbo.lv", expect: Expect::Written(&["200000"]) },
        Case { name: "several rows and cells", setup: "CREATE TABLE dbo.mm(id int PRIMARY KEY, a nvarchar(10), b int); INSERT INTO dbo.mm VALUES (1, N'a', 1), (2, N'b', 2), (3, N'c', 3);", schema: s, table: "mm", edits: vec![(0, "a", Some("A")), (1, "b", Some("20")), (2, "a", None), (2, "b", Some("30"))], check: "SELECT STRING_AGG(ISNULL(a, N'N') + CAST(b AS nvarchar(5)), N',') WITHIN GROUP (ORDER BY id) FROM dbo.mm", expect: Expect::Written(&["A1,b20,N30"]) },
    ];
    go(&id, cases);
}

// ---------------------------------------------------------------- each Stop ends what it stands beside

/// The result of a table job once it can be edited (its count may still be out).
fn editable_result(job: JobId) -> std::sync::Arc<std::sync::Mutex<results::ResultSet>> {
    let mut found = None;
    until("the table never became editable", || {
        found = session::job_state(job).and_then(|s| s.results.first().filter(|r| r.editable).map(|r| r.id));
        found.is_some()
    });
    results::get(found.unwrap()).unwrap()
}

/// Another connection to the file, holding its write lock.
fn another_writer(file: &std::path::Path) -> rusqlite::Connection {
    let other = rusqlite::Connection::open(file).unwrap();
    other.execute_batch("BEGIN IMMEDIATE").unwrap();
    other
}

/// A table of one person read for a pane whose count is still out (its
/// connection is not to be had), with one cell edited.
struct SlowCount {
    id: String,
    file: PathBuf,
    job: JobId,
    set: std::sync::Arc<std::sync::Mutex<results::ResultSet>>,
    held: Option<session::Lease>,
}

fn slow_count(name: &str, setup: &str) -> SlowCount {
    let (id, file) = sqlite_connection(name);
    run(&id, "setup", &format!("create table people(id integer primary key, name text); insert into people values (1, 'a'); {setup}"));
    let pane_owner = owner(&id, &format!("{name}-pane"));
    let held = runtime::block_on(session::lease(&id, None, &session::count_owner(&pane_owner))).unwrap();
    let job = session::open_table(&id, None, &pane_owner, &table("", "people"), None);
    let set = editable_result(job);
    assert!(session::job_state(job).unwrap().running, "the count is still out");
    results::lock(&set).set_edit(0, 1, Cell::text("mine"));
    SlowCount { id, file, job, set, held: Some(held) }
}

/// Starts the write-back and waits until it sits on the database's lock.
fn write_back_on_a_lock(set: &std::sync::Arc<std::sync::Mutex<results::ResultSet>>) -> std::thread::JoinHandle<Result<u64, DbError>> {
    let set = set.clone();
    let writer = {
        let set = set.clone();
        std::thread::spawn(move || runtime::block_on(session::write_back_set(&set)))
    };
    until("the write-back never reported that it is waiting", || results::lock(&set).is_writing());
    std::thread::sleep(Duration::from_millis(400));
    writer
}

/// An UPDATE that takes a few seconds (its trigger writes a few million rows).
const SLOW_UPDATE: &str = "create table scratch(n integer);
    create trigger slow after update on people begin
      insert into scratch select x from (with recursive c(x) as (select 1 union all select x + 1 from c where x < 3000000) select x from c);
    end;";

#[test]
fn sqlite_the_tables_stop_leaves_a_write_back_that_is_running() {
    let mut pane = slow_count("wb-stop-table", SLOW_UPDATE);
    let writer = {
        let set = pane.set.clone();
        std::thread::spawn(move || runtime::block_on(session::write_back_set(&set)))
    };
    until("the write-back never started", || results::lock(&pane.set).is_writing());
    std::thread::sleep(Duration::from_millis(500));

    session::cancel(pane.job);
    let written = within(Duration::from_secs(60), "the write-back after the table's Stop", move || writer.join().unwrap());
    assert_eq!(written, Ok(1), "the table's Stop is not the write-back's: {:?}", results::lock(&pane.set).write);
    assert_eq!(rows(&pane.id, "select name from people"), [["mine"]]);

    drop(pane.held.take());
    let state = wait(pane.job);
    assert!(state.cancelled, "the Stop did reach the count: {state:?}");
    assert_eq!(results::lock(&pane.set).total, None);
}

#[test]
fn sqlite_stopping_a_write_back_leaves_the_count_running() {
    let mut pane = slow_count("wb-stop-write", "");
    let other = another_writer(&pane.file);
    let writer = write_back_on_a_lock(&pane.set);

    let rid = results::lock(&pane.set).id;
    session::stop_write(rid);
    let stopped = within(Duration::from_secs(5), "the stopped write-back", move || writer.join().unwrap());
    assert_eq!(stopped, Err(DbError::Cancelled));
    let state = session::job_state(pane.job).unwrap();
    assert!(state.running && !state.cancelled, "the write-back's Stop is not the table's: {state:?}");
    {
        let r = results::lock(&pane.set);
        assert_eq!(r.write_failure().map(str::to_owned), Some(yuzora_db::i18n::t("edit.stopped", &[])));
    }
    other.execute_batch("ROLLBACK").unwrap();

    drop(pane.held.take());
    let state = wait(pane.job);
    assert!(!state.cancelled && state.error.is_none(), "{state:?}");
    assert_eq!(results::lock(&pane.set).total, Some(1), "the count still ran");
}

/// A count that really runs (a view that has to count a few hundred million
/// rows, one the read never gets to): the table's Stop ends it.
#[test]
fn sqlite_the_tables_stop_ends_a_count_that_is_running() {
    let (id, _) = sqlite_connection("wb-stop-count");
    run(
        &id,
        "setup",
        "create view slow(id) as with recursive c(x) as (select 1 union all select x + 1 from c where x < 400000000) select x from c;",
    );
    let job = session::open_table(&id, None, &owner(&id, "stop-count-pane"), &table("", "slow"), None);
    until("the rows never arrived", || {
        session::job_state(job).is_some_and(|s| s.results.first().is_some_and(|r| r.rows > 0 && r.complete))
    });
    std::thread::sleep(Duration::from_millis(300));
    assert!(session::job_state(job).unwrap().running, "the count is running");
    session::cancel(job);
    let state = within(Duration::from_secs(5), "the table's Stop ending the count", move || wait(job));
    assert!(state.cancelled, "{state:?}");
    assert_eq!(state.results[0].total, None);
}

#[test]
fn postgres_the_tables_stop_leaves_a_write_back_that_is_waiting_on_a_row_lock() {
    only_with!("postgres");
    let scratch = PgScratchDatabase::new("yz_wb_stoptable");
    let id = scratch.connection();
    run(&id, "setup", "CREATE TABLE t(id int PRIMARY KEY, name text); INSERT INTO t VALUES (1, 'a'), (2, 'b');");
    let pane_owner = owner(&id, "stop-table-pane");
    let held = runtime::block_on(session::lease(&id, None, &session::count_owner(&pane_owner))).unwrap();
    let job = session::open_table(&id, None, &pane_owner, &table("public", "t"), None);
    let set = editable_result(job);
    assert!(session::job_state(job).unwrap().running, "the count is still out");
    let row = {
        let r = results::lock(&set);
        r.rows.iter().position(|row| row[0].display() == "2").unwrap()
    };
    run(&id, "locker", "BEGIN; UPDATE t SET name = 'locked' WHERE id = 2");
    results::lock(&set).set_edit(row, 1, Cell::text("mine"));
    let writer = write_back_on_a_lock(&set);

    session::cancel(job);
    std::thread::sleep(Duration::from_millis(600));
    assert!(
        results::lock(&set).is_writing(),
        "the table's Stop is not the write-back's: {:?}",
        results::lock(&set).write
    );
    run(&id, "locker", "ROLLBACK");
    let written = within(Duration::from_secs(10), "the write-back after the table's Stop", move || writer.join().unwrap());
    assert_eq!(written, Ok(1), "{:?}", results::lock(&set).write);
    assert_eq!(first_row(&id, "SELECT name FROM t WHERE id = 2"), ["mine"]);
    drop(held);
    assert!(wait(job).cancelled);
}

#[test]
fn postgres_stopping_a_write_back_says_so_in_words_and_leaves_the_count_running() {
    only_with!("postgres");
    let scratch = PgScratchDatabase::new("yz_wb_stopwrite");
    let id = scratch.connection();
    run(&id, "setup", "CREATE TABLE t(id int PRIMARY KEY, name text); INSERT INTO t VALUES (1, 'a');");
    let pane_owner = owner(&id, "stop-write-pane");
    let held = runtime::block_on(session::lease(&id, None, &session::count_owner(&pane_owner))).unwrap();
    let job = session::open_table(&id, None, &pane_owner, &table("public", "t"), None);
    let set = editable_result(job);
    run(&id, "locker", "BEGIN; UPDATE t SET name = 'locked' WHERE id = 1");
    results::lock(&set).set_edit(0, 1, Cell::text("mine"));
    let writer = write_back_on_a_lock(&set);

    let rid = results::lock(&set).id;
    session::stop_write(rid);
    let stopped = within(Duration::from_secs(5), "the stopped write-back", move || writer.join().unwrap());
    assert_eq!(stopped, Err(DbError::Cancelled));
    let words = results::lock(&set).write_failure().map(str::to_owned).unwrap_or_default();
    assert_eq!(words, yuzora_db::i18n::t("edit.stopped", &[]), "stopped on a lock, nothing was committed: {words}");
    let state = session::job_state(job).unwrap();
    assert!(state.running && !state.cancelled, "{state:?}");
    run(&id, "locker", "ROLLBACK");
    drop(held);
    let state = wait(job);
    assert!(!state.cancelled);
    assert_eq!(results::lock(&set).total, Some(1));
    assert_eq!(first_row(&id, "SELECT name FROM t WHERE id = 1"), ["a"]);
}

/// Only a Stop that lands once COMMIT has been sent leaves it open whether the
/// database kept the changes: before that the transaction was never ended, and
/// the server rolls it back.
#[test]
fn postgres_a_write_back_stopped_while_the_server_commits_says_the_database_may_have_saved_it() {
    only_with!("postgres");
    let scratch = PgScratchDatabase::new("yz_wb_stopcommit");
    let id = scratch.connection();
    let admin = raw_postgres("yz_wb_stopcommit");
    runtime::block_on(admin.batch_execute(
        "CREATE TABLE t(id int PRIMARY KEY, name text); INSERT INTO t VALUES (1, 'a');
         CREATE FUNCTION slow_commit() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_sleep(3); RETURN NULL; END $$;
         CREATE CONSTRAINT TRIGGER slow_commit AFTER UPDATE ON t DEFERRABLE INITIALLY DEFERRED
           FOR EACH ROW EXECUTE FUNCTION slow_commit();",
    ))
    .unwrap();
    let pane = open(&id, "commit-pane", &table("public", "t"));
    pane.edit(pane.row_where("id", "1"), "name", Some("mine"));
    let writer = {
        let set = pane.set.clone();
        std::thread::spawn(move || runtime::block_on(session::write_back_set(&set)))
    };
    until("the server never started to commit", || {
        let committing = runtime::block_on(admin.query_one(
            "SELECT count(*)::int FROM pg_stat_activity WHERE datname = 'yz_wb_stopcommit' AND state = 'active' AND query = 'COMMIT'",
            &[],
        ))
        .unwrap();
        committing.get::<_, i32>(0) == 1
    });

    session::stop_write(pane.id);
    assert_eq!(
        within(Duration::from_secs(10), "the stopped write-back", move || writer.join().unwrap()),
        Err(DbError::Cancelled)
    );
    let words = results::lock(&pane.set).write_failure().map(str::to_owned).unwrap_or_default();
    assert_eq!(words, yuzora_db::i18n::t("edit.stopped_maybe", &[]), "COMMIT was on its way: {words}");
}

#[test]
fn postgres_the_tables_stop_ends_a_count_that_is_running() {
    only_with!("postgres");
    let scratch = PgScratchDatabase::new("yz_wb_stopcount");
    let id = scratch.connection();
    run(&id, "setup", "CREATE VIEW slow AS WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c WHERE x < 2000000000) SELECT x AS id FROM c;");
    let job = session::open_table(&id, None, &owner(&id, "stop-count-pane"), &table("public", "slow"), None);
    until("the rows never arrived", || {
        session::job_state(job).is_some_and(|s| s.results.first().is_some_and(|r| r.rows > 0 && r.complete))
    });
    std::thread::sleep(Duration::from_millis(500));
    assert!(session::job_state(job).unwrap().running, "the count is running");
    session::cancel(job);
    let state = within(Duration::from_secs(10), "the table's Stop ending the count", move || wait(job));
    assert!(state.cancelled, "{state:?}");
    assert_eq!(state.results[0].total, None);
}

// ---------------------------------------------------------------- a count that has no connection of its own

/// A pane's table is read while the connection a count would use cannot be
/// opened: the connection stays what it was, and the total still comes.
#[cfg(unix)]
#[test]
fn sqlite_a_count_without_a_connection_of_its_own_runs_on_the_panes_and_does_not_fail_the_connection() {
    let (id, file) = sqlite_connection("wb-count-fallback");
    run(&id, "setup", "create table people(id integer primary key, name text); insert into people values (1, 'a'), (2, 'b');");
    let pane_owner = owner(&id, "fallback-pane");
    let t = table("", "people");
    // The connections the read opens first are there: the count's is the only new one.
    runtime::block_on(session::describe(&id, None, &t)).unwrap();
    drop(runtime::block_on(session::lease(&id, None, &pane_owner)).unwrap());
    let moved = file.with_extension("moved");
    std::fs::rename(&file, &moved).unwrap();

    let state = wait(session::open_table(&id, None, &pane_owner, &t, None));
    std::fs::rename(&moved, &file).unwrap();
    assert_eq!(state.error, None);
    assert_eq!(state.results[0].rows, 2);
    assert_eq!(state.results[0].total, Some(2), "the count came, on the pane's connection");
    let status = session::status(&id);
    assert_eq!(status.state, session::State::Connected, "{status:?}");
}

/// A role that may have two connections: the sidebar's and the pane's.
#[test]
fn postgres_a_connection_limit_does_not_fail_the_connection_or_hide_the_total() {
    only_with!("postgres");
    let maintenance = postgres_in("pg-maintenance-limit", "postgres");
    run(&maintenance, "role", "DROP ROLE IF EXISTS yz_wb_lim");
    run(&maintenance, "role", "CREATE ROLE yz_wb_lim LOGIN PASSWORD 'yz-wb-lim-pw' CONNECTION LIMIT 2");
    let _role = PgRole { maintenance: maintenance.clone(), name: "yz_wb_lim" };
    let scratch = PgScratchDatabase::new("yz_wb_limit");
    let setup = scratch.connection();
    run(&setup, "setup", "CREATE TABLE t(id int PRIMARY KEY, name text); INSERT INTO t VALUES (1, 'a'), (2, 'b'), (3, 'c'); GRANT SELECT, UPDATE ON t TO yz_wb_lim;");
    let limited = save(
        json!({"name": "pg-limited", "engine": "postgres", "host": "127.0.0.1", "port": 55432,
               "user": "yz_wb_lim", "database": "yz_wb_limit", "ssl": "prefer"}),
        Some("yz-wb-lim-pw"),
    );

    let state = wait(session::open_table(&limited, None, &owner(&limited, "limit-pane"), &table("public", "t"), None));
    assert_eq!(state.error, None);
    let summary = &state.results[0];
    assert!(summary.editable, "{summary:?}");
    assert_eq!(summary.total, Some(3), "the count came, though a third connection is not allowed");
    let status = session::status(&limited);
    assert_eq!(status.state, session::State::Connected, "{status:?}");

    let set = results::get(summary.id).unwrap();
    results::lock(&set).set_edit(0, 1, Cell::text("mine"));
    assert_eq!(runtime::block_on(session::write_back_set(&set)), Ok(1));
    assert_eq!(first_row(&setup, "SELECT name FROM t WHERE id = 1"), ["mine"]);
    session::release_owner(&owner(&limited, "limit-pane"));
    session::disconnect(&limited);
}

/// A login dropped when the test ends, whatever happened (declared before the
/// database it was granted on, so it goes after it).
struct PgRole {
    maintenance: String,
    name: &'static str,
}

impl Drop for PgRole {
    fn drop(&mut self) {
        let job = session::start_query(
            &self.maintenance,
            None,
            &owner(&self.maintenance, "role"),
            &format!("DROP ROLE IF EXISTS {}", self.name),
        );
        let mut seen = 0;
        while let Ok(state) = runtime::block_on(session::job_after(job, seen)) {
            if !state.running {
                break;
            }
            seen = state.version;
        }
    }
}

// ---------------------------------------------------------------- a stop reaches nothing but the write-back

fn raw_postgres(database: &str) -> tokio_postgres::Client {
    runtime::block_on(async {
        let (client, connection) = tokio_postgres::connect(
            &format!("host=127.0.0.1 port=55432 user=yuzora_full password={} dbname={database}", password()),
            tokio_postgres::NoTls,
        )
        .await
        .unwrap();
        tokio::spawn(connection);
        client
    })
}

/// The write-back of `pane` on its way to the server, waiting on a row lock another connection holds.
fn writing_on_a_row_lock(
    pane: &Opened,
    locker: &tokio_postgres::Client,
    database: &str,
) -> std::thread::JoinHandle<Result<u64, DbError>> {
    let writer = {
        let set = pane.set.clone();
        std::thread::spawn(move || runtime::block_on(session::write_back_set(&set)))
    };
    until("the write-back never reported that it is waiting", || results::lock(&pane.set).is_writing());
    until("the server never saw the write-back wait for the lock", || {
        let waiting = runtime::block_on(locker.query_one(
            &format!(
                "SELECT count(*)::int FROM pg_stat_activity WHERE datname = '{database}' AND wait_event_type = 'Lock'"
            ),
            &[],
        ))
        .unwrap();
        waiting.get::<_, i32>(0) == 1
    });
    writer
}

/// However late the server hears of a Stop, it is for the write-back: what
/// the pane runs after it neither fails nor is cut short.
#[test]
fn postgres_a_stopped_write_back_cannot_be_felt_by_what_the_pane_runs_next() {
    only_with!("postgres");
    let scratch = PgScratchDatabase::new("yz_wb_latecancel");
    let direct = scratch.connection();
    run(&direct, "setup", "CREATE TABLE t(id int PRIMARY KEY, name text); INSERT INTO t VALUES (1, 'a');");
    let relay = common::LaggingPostgres::start();
    relay.hold_cancels(Duration::from_millis(900));
    let through = save(
        json!({"name": "pg-late-cancel", "engine": "postgres", "host": "127.0.0.1", "port": relay.port(),
               "user": "yuzora_full", "database": "yz_wb_latecancel", "ssl": "disable"}),
        Some(&password()),
    );
    let locker = raw_postgres("yz_wb_latecancel");
    let pane = open(&through, "late-pane", &table("public", "t"));
    pane.edit(pane.row_where("id", "1"), "name", Some("mine"));
    runtime::block_on(locker.batch_execute("BEGIN; UPDATE t SET name = 'holder' WHERE id = 1")).unwrap();
    let writer = writing_on_a_row_lock(&pane, &locker, "yz_wb_latecancel");

    let next = session::start_query(&through, None, &pane.owner, "SELECT pg_sleep(2.5)");
    results::lock(&pane.set).stop_write();
    runtime::block_on(locker.batch_execute("COMMIT")).unwrap();
    assert_eq!(
        within(Duration::from_secs(10), "the stopped write-back", move || writer.join().unwrap()),
        Err(DbError::Cancelled)
    );

    let state = within(Duration::from_secs(20), "the query after the write-back", move || wait(next));
    assert_eq!(state.error, None, "the Stop reached the query that ran after the write-back: {state:?}");
    assert!(!state.cancelled);
    assert!(state.elapsed_ms >= 2400, "the query was cut short: {state:?}");
    assert_eq!(
        first_row(&direct, "SELECT name FROM t WHERE id = 1"),
        ["holder"],
        "nothing of the stopped write-back is written"
    );

    let again = open(&through, "late-pane", &table("public", "t"));
    again.edit(again.row_where("id", "1"), "name", Some("mine"));
    assert_eq!(runtime::block_on(session::write_back_set(&again.set)), Ok(1), "the pane can write again");
    assert_eq!(first_row(&direct, "SELECT name FROM t WHERE id = 1"), ["mine"]);
}

/// A stopped write-back is dropped where it stands, and dropping its
/// transaction sends ROLLBACK. That must be the first the server hears after
/// the stop, so the stop cannot land on it: the connection is let go of after
/// the server has dealt with the stop, not before. The relay holds the stop
/// back and sees the order the server does.
#[test]
fn postgres_a_stopped_write_back_is_rolled_back_only_after_the_server_has_dealt_with_the_stop() {
    only_with!("postgres");
    let scratch = PgScratchDatabase::new("yz_wb_stoporder");
    let direct = scratch.connection();
    run(&direct, "setup", "CREATE TABLE t(id int PRIMARY KEY, name text); INSERT INTO t VALUES (1, 'a');");
    let relay = common::LaggingPostgres::start();
    relay.hold_cancels(Duration::from_millis(600));
    let through = postgres_behind_slow_cancels("pg-stop-order", "yz_wb_stoporder", &relay);
    let locker = raw_postgres("yz_wb_stoporder");
    let pane = open(&through, "order-pane", &table("public", "t"));
    pane.edit(pane.row_where("id", "1"), "name", Some("mine"));
    runtime::block_on(locker.batch_execute("BEGIN; UPDATE t SET name = 'holder' WHERE id = 1")).unwrap();
    let writer = writing_on_a_row_lock(&pane, &locker, "yz_wb_stoporder");

    results::lock(&pane.set).stop_write();
    assert_eq!(
        within(Duration::from_secs(10), "the stopped write-back", move || writer.join().unwrap()),
        Err(DbError::Cancelled)
    );
    runtime::block_on(locker.batch_execute("ROLLBACK")).unwrap();
    assert_eq!(
        relay.heard(),
        [common::Heard::StopDealtWith, common::Heard::RollBack],
        "the write-back was rolled back before the server had dealt with the stop, which could land on the ROLLBACK"
    );
}

/// A pane's connection through a relay of its own that is holding cancels back
/// (`ssl` is off: the relay tells a cancel request by its first bytes).
fn postgres_behind_slow_cancels(name: &str, database: &str, relay: &common::LaggingPostgres) -> String {
    save(
        json!({"name": name, "engine": "postgres", "host": "127.0.0.1", "port": relay.port(),
               "user": "yuzora_full", "database": database, "ssl": "disable"}),
        Some(&password()),
    )
}

/// A Stop pressed as a query ends has nothing left to stop, but the request is
/// still on its way to the server, and finds what the connection runs when it
/// gets there: whatever the pane does next must not be that.
#[test]
fn postgres_a_stop_pressed_as_a_query_ends_is_not_felt_by_the_next_run() {
    only_with!("postgres");
    let relay = common::LaggingPostgres::start();
    let id = postgres_behind_slow_cancels("pg-late-run", "yuzora_p8", &relay);
    let pane = owner(&id, "late-run");
    assert_eq!(wait(session::start_query(&id, None, &pane, "SELECT 1")).error, None);
    relay.hold_cancels(Duration::from_millis(900));

    let first = session::start_query(&id, None, &pane, "DO $$ BEGIN PERFORM pg_sleep(0.3); END $$");
    std::thread::sleep(Duration::from_millis(150));
    session::cancel(first);
    assert_eq!(within(Duration::from_secs(10), "the query that ended by itself", move || wait(first)).error, None);

    let next = session::start_query(&id, None, &pane, "SELECT pg_sleep(2)");
    let state = within(Duration::from_secs(30), "the next run", move || wait(next));
    assert_eq!(state.error, None, "the Stop reached the run after the query it was for: {state:?}");
    assert!(!state.cancelled);
    assert!(state.elapsed_ms >= 2000, "the next run was cut short: {state:?}");
}

/// The same, with the pane opening a table next (its read is what the request finds).
#[test]
fn postgres_a_stop_pressed_as_a_query_ends_is_not_felt_by_the_table_the_pane_opens_next() {
    only_with!("postgres");
    let scratch = PgScratchDatabase::new("yz_wb_lateopen");
    let direct = scratch.connection();
    run(
        &direct,
        "setup",
        "CREATE TABLE t(id int PRIMARY KEY, name text); INSERT INTO t VALUES (1, 'a');
         CREATE VIEW slow AS SELECT id, pg_sleep(0.8)::text AS waited FROM t;",
    );
    let relay = common::LaggingPostgres::start();
    let id = postgres_behind_slow_cancels("pg-late-open", "yz_wb_lateopen", &relay);
    let pane = owner(&id, "late-open");
    assert_eq!(wait(session::start_query(&id, None, &pane, "SELECT 1")).error, None);
    relay.hold_cancels(Duration::from_millis(700));

    let first = session::start_query(&id, None, &pane, "DO $$ BEGIN PERFORM pg_sleep(0.3); END $$");
    std::thread::sleep(Duration::from_millis(150));
    session::cancel(first);
    assert_eq!(within(Duration::from_secs(10), "the query that ended by itself", move || wait(first)).error, None);

    let opened = session::open_table(&id, None, &pane, &table("public", "slow"), None);
    let state = within(Duration::from_secs(30), "the table", move || wait(opened));
    assert_eq!(state.error, None, "the Stop reached the table the pane opened after the query: {state:?}");
    assert_eq!(state.results.first().map(|r| (r.rows, r.error.clone())), Some((1, None)), "{state:?}");
}

/// The Stop and the end of the query a few hundred microseconds apart either
/// way, over and over, with no relay: the next run is never the one it stops.
#[test]
fn postgres_a_stop_timed_with_the_end_of_a_query_is_never_felt_by_the_next_run() {
    only_with!("postgres");
    let rounds: usize = std::env::var("YUZORA_STOP_SWEEP_ROUNDS").ok().and_then(|v| v.parse().ok()).unwrap_or(150);
    let id = postgres_connection("pg-stop-sweep-run");
    let pane = owner(&id, "sweep-run");
    assert_eq!(wait(session::start_query(&id, None, &pane, "SELECT 1")).error, None);
    let mut seed: u64 = 0x2545_F491_4F6C_DD1D;
    for round in 0..rounds {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let offset = (seed % 4001) as i64 - 2000;
        let first = session::start_query(&id, None, &pane, "DO $$ BEGIN PERFORM pg_sleep(0.03); END $$");
        let stop_at = Instant::now() + Duration::from_micros((30_000 + offset + 1_500) as u64);
        while Instant::now() < stop_at {
            std::hint::spin_loop();
        }
        session::cancel(first);
        let next = session::start_query(&id, None, &pane, "SELECT pg_sleep(0.05)");
        let state = wait(next);
        assert_eq!(state.error, None, "round {round} (offset {offset} µs): the Stop reached the next run");
    }
}

/// The same stop, timed against the moment the lock is let go, over and over:
/// whichever of them comes first, the pane's connection is whole afterwards
/// and nothing the stopped write-back held is written.
#[test]
fn postgres_a_stop_leaves_the_panes_connection_whole_whichever_comes_first_the_stop_or_the_lock() {
    only_with!("postgres");
    let rounds: usize = std::env::var("YUZORA_STOP_SWEEP_ROUNDS").ok().and_then(|v| v.parse().ok()).unwrap_or(200);
    let scratch = PgScratchDatabase::new("yz_wb_sweep");
    let id = scratch.connection();
    run(&id, "setup", "CREATE TABLE t(id int PRIMARY KEY, name text); INSERT INTO t VALUES (1, 'a');");
    let locker = raw_postgres("yz_wb_sweep");
    let t = table("public", "t");
    let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut stop_came_first = 0;
    for round in 0..rounds {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        // Microseconds between the Stop and the lock being let go; negative: the Stop first. The
        // cancel lands late when the two are within a millisecond or two of each other.
        let offset = (seed % 1601) as i64 - 1500;
        let pane = open(&id, "sweep-pane", &t);
        pane.edit(pane.row_where("id", "1"), "name", Some(&format!("mine-{round}")));
        runtime::block_on(locker.batch_execute("BEGIN; UPDATE t SET name = 'holder' WHERE id = 1")).unwrap();
        let writer = writing_on_a_row_lock(&pane, &locker, "yz_wb_sweep");
        let next = session::start_query(&id, None, &pane.owner, "SELECT pg_sleep(0.08)");
        std::thread::sleep(Duration::from_millis(10));

        let spin = |micros: i64| {
            let until = Instant::now() + Duration::from_micros(micros.unsigned_abs());
            while Instant::now() < until {
                std::hint::spin_loop();
            }
        };
        let (stopped_at, released_at);
        if offset >= 0 {
            released_at = Instant::now();
            runtime::block_on(locker.batch_execute("COMMIT")).unwrap();
            spin(offset);
            stopped_at = Instant::now();
            results::lock(&pane.set).stop_write();
        } else {
            stopped_at = Instant::now();
            results::lock(&pane.set).stop_write();
            spin(offset);
            released_at = Instant::now();
            runtime::block_on(locker.batch_execute("COMMIT")).unwrap();
        }
        let written = writer.join().unwrap();
        let stop_first = released_at > stopped_at + Duration::from_micros(300);

        let state = wait(next);
        assert_eq!(
            state.error, None,
            "round {round} (offset {offset} µs): the Stop reached what ran after the write-back"
        );
        let alive = wait(session::start_query(&id, None, &pane.owner, "SELECT 1"));
        assert_eq!(alive.error, None, "round {round} (offset {offset} µs): the pane's connection is left broken");
        let name: String = runtime::block_on(locker.query_one("SELECT name FROM t WHERE id = 1", &[])).unwrap().get(0);
        if stop_first {
            stop_came_first += 1;
            assert_eq!(written, Err(DbError::Cancelled), "round {round} (offset {offset} µs)");
            assert_eq!(
                name, "holder",
                "round {round} (offset {offset} µs): a write-back stopped before the lock let go wrote anyway"
            );
        } else {
            assert!(name == "holder" || name == format!("mine-{round}"), "round {round} (offset {offset} µs): {name}");
        }
        runtime::block_on(locker.batch_execute("UPDATE t SET name = 'a' WHERE id = 1")).unwrap();
        session::release_owner(&pane.owner);
    }
    assert!(stop_came_first >= rounds / 4, "only {stop_came_first} of {rounds} rounds had the Stop first");
}

/// What the write-back is doing is not up to when it gets to look at the Stop:
/// a Stop pressed before COMMIT is sent means no COMMIT, whether or not the
/// write-back has noticed it yet. The write-back is polled by hand and left
/// alone from the moment it waits on the lock until its UPDATE has gone
/// through on the server, so the Stop is pressed while it cannot see anything.
#[test]
fn postgres_a_stop_pressed_before_the_commit_is_sent_writes_nothing_though_the_write_back_has_not_seen_it() {
    use std::future::Future;
    use std::task::Poll;
    only_with!("postgres");
    let scratch = PgScratchDatabase::new("yz_wb_unseen");
    let direct = scratch.connection();
    run(&direct, "setup", "CREATE TABLE t(id int PRIMARY KEY, name text); INSERT INTO t VALUES (1, 'a');");
    let locker = raw_postgres("yz_wb_unseen");
    let pane = open(&direct, "unseen-pane", &table("public", "t"));
    pane.edit(pane.row_where("id", "1"), "name", Some("mine"));
    runtime::block_on(locker.batch_execute("BEGIN; UPDATE t SET name = 'holder' WHERE id = 1")).unwrap();
    let count = |state: &'static str| {
        let locker = &locker;
        async move {
            let sql = format!("SELECT count(*)::int FROM pg_stat_activity WHERE datname = 'yz_wb_unseen' AND {state}");
            locker.query_one(sql.as_str(), &[]).await.unwrap().get::<_, i32>(0)
        }
    };

    let set = pane.set.clone();
    let written = runtime::block_on(async {
        let mut write = Box::pin(session::write_back_set(&set));
        let mut polls = 0;
        while count("wait_event_type = 'Lock'").await == 0 {
            assert!(std::future::poll_fn(|cx| Poll::Ready(write.as_mut().poll(cx))).await.is_pending());
            polls += 1;
            assert!(polls < 2000, "the write-back never got to wait on the lock");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        results::lock(&pane.set).stop_write();
        locker.batch_execute("COMMIT").await.unwrap();
        let mut waited = 0;
        while count("state = 'idle in transaction'").await == 0 {
            waited += 1;
            assert!(waited < 2000, "the UPDATE never went through once the lock was let go");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        tokio::time::timeout(Duration::from_secs(10), write).await.expect("the write-back ends once it is polled again")
    });

    assert_eq!(written, Err(DbError::Cancelled));
    assert_eq!(
        results::lock(&pane.set).write_failure().map(str::to_owned),
        Some(yuzora_db::i18n::t("edit.stopped", &[])),
        "COMMIT had not been sent when the Stop was pressed, so the message must not say the database may have saved it"
    );
    assert_eq!(first_row(&direct, "SELECT name FROM t WHERE id = 1"), ["holder"], "a Stop before COMMIT wrote anyway");
    assert_eq!(wait(session::start_query(&direct, None, &pane.owner, "SELECT 1")).error, None);
}

/// SQLite waits for another writer's lock inside the file, where an interrupt
/// does not reach: a stopped write-back must still write nothing once the lock
/// is let go, and must not keep the pane's connection until then.
#[test]
fn sqlite_a_write_back_stopped_while_it_waits_for_the_file_lock_writes_nothing_and_frees_the_connection() {
    let (id, file) = sqlite_connection("wb-stop-filelock");
    run(&id, "setup", "create table people(id integer primary key, name text); insert into people values (1, 'a');");
    let pane = open(&id, "filelock-pane", &table("", "people"));
    pane.edit(pane.row_where("id", "1"), "name", Some("mine"));
    let other = another_writer(&file);
    let writer = write_back_on_a_lock(&pane.set);

    results::lock(&pane.set).stop_write();
    assert_eq!(
        within(Duration::from_secs(5), "the stopped write-back", move || writer.join().unwrap()),
        Err(DbError::Cancelled)
    );
    let owner = pane.owner.clone();
    let connection = id.clone();
    let state = within(Duration::from_secs(2), "the pane's connection after the stop", move || {
        wait(session::start_query(&connection, None, &owner, "select name from people"))
    });
    assert_eq!(state.error, None);
    other.execute_batch("ROLLBACK").unwrap();
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(rows(&id, "select name from people"), [["a"]], "the stopped write-back wrote once the lock was let go");
}

// ---------------------------------------------------------------- a person's work goes before a count that borrowed the connection

/// A login that may have two connections (the sidebar's and the pane's).
fn limited_role(role: &'static str) -> PgRole {
    let maintenance = postgres_in(&format!("pg-maintenance-{role}"), "postgres");
    run(&maintenance, "role", &format!("DROP ROLE IF EXISTS {role}"));
    run(&maintenance, "role", &format!("CREATE ROLE {role} LOGIN PASSWORD '{role}-pw' CONNECTION LIMIT 2"));
    PgRole { maintenance, name: role }
}

/// A table whose rows read at once and whose count takes about four seconds,
/// and the saved connection of `role` to it; with the connection of the setup.
fn slow_count_for(role: &'static str, database: &str) -> (String, String) {
    let setup = postgres_in(&format!("pg-setup-{role}"), database);
    run(
        &setup,
        "setup",
        &format!(
            "CREATE TABLE t(id int PRIMARY KEY, name text);
             INSERT INTO t SELECT g, 'a' FROM generate_series(1, 101) g;
             INSERT INTO t SELECT g, 'slow' FROM generate_series(102, 105) g;
             CREATE FUNCTION gate(i int) RETURNS bool LANGUAGE plpgsql VOLATILE AS $$
               BEGIN IF i > 101 AND current_query() ILIKE '%count(%' THEN PERFORM pg_sleep(1); END IF; RETURN true; END $$;
             ALTER TABLE t ENABLE ROW LEVEL SECURITY;
             CREATE POLICY p ON t USING (gate(id));
             GRANT SELECT, UPDATE ON t TO {role};"
        ),
    );
    let limited = save(
        json!({"name": format!("pg-{role}"), "engine": "postgres", "host": "127.0.0.1", "port": 55432,
               "user": role, "database": database, "ssl": "prefer"}),
        Some(&format!("{role}-pw")),
    );
    (setup, limited)
}

fn counting_on(setup: &str, role: &str) -> bool {
    first_row(
        setup,
        &format!(
            "SELECT count(*)::text FROM pg_stat_activity WHERE usename = '{role}' AND state = 'active' \
             AND query ILIKE '%count(%' AND pid <> pg_backend_pid()"
        ),
    ) == ["1"]
}

/// A save pressed while the count runs on the pane's own connection (the
/// login has no connection to spare) does not wait for it: the count steps
/// aside, and runs again once the write-back is done.
#[test]
fn postgres_a_write_back_does_not_wait_for_a_count_that_borrowed_the_panes_connection() {
    only_with!("postgres");
    let _role = limited_role("yz_wb_borrow");
    let _scratch = PgScratchDatabase::new("yz_wb_borrow");
    let (setup, limited) = slow_count_for("yz_wb_borrow", "yz_wb_borrow");
    let pane_owner = owner(&limited, "borrow-pane");
    let job = session::open_table(&limited, None, &pane_owner, &table("public", "t"), None);
    let set = editable_result(job);
    until("the count never started on the pane's connection", || counting_on(&setup, "yz_wb_borrow"));
    let row = results::lock(&set).rows.iter().position(|r| r[0].display() == "1").unwrap();
    results::lock(&set).set_edit(row, 1, Cell::text("mine"));

    let started = Instant::now();
    let written = {
        let set = set.clone();
        within(Duration::from_secs(3), "the write-back, with a count on the pane's connection", move || {
            runtime::block_on(session::write_back_set(&set))
        })
    };
    assert_eq!(written, Ok(1), "{:?}", results::lock(&set).write);
    assert!(started.elapsed() < Duration::from_secs(3), "the write-back waited for the count");
    assert_eq!(first_row(&setup, "SELECT name FROM t WHERE id = 1"), ["mine"]);

    let state = within(Duration::from_secs(40), "the count after the write-back", move || wait(job));
    assert!(!state.cancelled && state.error.is_none(), "{state:?}");
    assert_eq!(results::lock(&set).total, Some(105), "the count ran again");
    assert_eq!(session::status(&limited).state, session::State::Connected);
}

/// Whatever else asks for the pane's connection — the SQL tab, a refresh, a
/// save — gets it at once when a count has borrowed it.
#[cfg(unix)]
#[test]
fn sqlite_a_count_that_borrowed_the_panes_connection_steps_aside_for_whoever_asks() {
    let (id, file) = sqlite_connection("wb-count-aside");
    run(
        &id,
        "setup",
        "create view slow(id) as with recursive c(x) as (select 1 union all select x + 1 from c where x < 400000000) select x from c;",
    );
    let pane_owner = owner(&id, "aside-pane");
    let t = table("", "slow");
    runtime::block_on(session::describe(&id, None, &t)).unwrap();
    drop(runtime::block_on(session::lease(&id, None, &pane_owner)).unwrap());
    let moved = file.with_extension("moved");
    std::fs::rename(&file, &moved).unwrap();
    let job = session::open_table(&id, None, &pane_owner, &t, None);
    until("the rows never arrived", || {
        session::job_state(job).is_some_and(|s| s.results.first().is_some_and(|r| r.rows > 0 && r.complete))
    });
    std::thread::sleep(Duration::from_millis(300));
    assert!(session::job_state(job).unwrap().running, "the count is running");

    let (connection, pane) = (id.clone(), pane_owner.clone());
    let asked = Instant::now();
    let lease = within(Duration::from_secs(5), "the pane's connection, while a count has it", move || {
        runtime::block_on(session::lease(&connection, None, &pane))
    });
    assert!(lease.is_ok());
    assert!(asked.elapsed() < Duration::from_secs(3), "asking for the pane's connection waited for the count");
    drop(lease);
    let state = session::job_state(job).unwrap();
    assert!(state.running && !state.cancelled, "stepping aside is not the Stop: {state:?}");

    std::fs::rename(&moved, &file).unwrap();
    session::cancel(job);
    assert!(within(Duration::from_secs(10), "the table's Stop", move || wait(job)).cancelled);
}
