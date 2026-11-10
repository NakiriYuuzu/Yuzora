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

/// The first row of `sql` run on the connection `owner` holds.
fn first_row_on(connection: &str, owner: &str, sql: &str) -> Vec<String> {
    let state = wait(session::start_query(connection, None, owner, sql));
    assert_eq!(state.error, None, "{sql}");
    let set = results::get(state.results.last().expect("a result").id).expect("result");
    let r = results::lock(&set);
    r.rows[0].iter().map(|c| c.display().into_owned()).collect()
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

    let held = runtime::block_on(session::lease(&id, None, &session::table_owner(&pane.owner))).unwrap();
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

/// The count has its connection held by hand, so it cannot have started: what
/// the rows are does not wait for it, however long it takes. (Which comes
/// first when nothing holds the count back depends on whether the count or
/// the table's description is quicker.)
fn editable_before_the_count(connection: &str, t: ObjectRef) {
    let pane_owner = owner(connection, "big-pane");
    let held = runtime::block_on(session::lease(connection, None, &session::count_owner(&pane_owner))).unwrap();
    let job = session::open_table(connection, None, &pane_owner, &t, None);
    let set = editable_result(job);
    let (rows, truncated, total) = {
        let r = results::lock(&set);
        (r.rows.len(), r.truncated, r.total)
    };
    assert_eq!((rows, truncated), (CAP, true), "the table is bigger than the cap");
    assert_eq!(total, None, "the count is still out");
    assert!(session::job_state(job).unwrap().running, "the job is still counting");
    drop(held);
    let state = within(Duration::from_secs(120), "the count after it was let go", move || wait(job));
    assert_eq!(state.error, None);
    assert!(results::lock(&set).total.is_some_and(|n| n > CAP as u64), "the count came");
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

// ---------------------------------------------------------------- the rows do not wait for the table's description

/// What the table's description (its key, in particular) is waiting for: the
/// sidebar's connection, held here by hand. The rows are read on the pane's
/// own connection and show at once; what they are, editable or not, is
/// settled when the description is in.
#[test]
fn sqlite_the_rows_show_while_the_tables_description_is_still_out_and_are_editable_once_it_is_in() {
    let (id, _) = sqlite_connection("wb-info-late");
    run(
        &id,
        "setup",
        "create table people(id integer primary key, name text); insert into people values (1, 'a'), (2, 'b');",
    );
    let held = runtime::block_on(session::lease(&id, None, session::META)).unwrap();

    let job = session::open_table(&id, None, &owner(&id, "info-late-pane"), &table("", "people"), None);
    let mut found = None;
    until("the rows never showed while the description was out", || {
        found =
            session::job_state(job).and_then(|s| s.results.first().filter(|r| r.rows == 2 && r.complete).map(|r| r.id));
        found.is_some()
    });
    let set = results::get(found.unwrap()).unwrap();
    assert_eq!(results::lock(&set).editing, results::Editing::Loading, "what the rows are is not known yet");
    assert!(session::job_state(job).unwrap().running, "the job is not over before the description is in");

    drop(held);
    let state = wait(job);
    assert_eq!(state.error, None);
    assert!(results::lock(&set).editable(), "{:?}", results::lock(&set).editing);
    assert_eq!(results::lock(&set).total, Some(2));
}

/// A Stop pressed while the description is out ends the job at once: the
/// sidebar's connection is not the pane's to cancel, so the rows stay, read-only,
/// and the reason says why.
#[test]
fn sqlite_a_stop_while_the_tables_description_is_out_ends_the_job_without_waiting_for_it() {
    let (id, _) = sqlite_connection("wb-info-stop");
    run(&id, "setup", "create table people(id integer primary key, name text); insert into people values (1, 'a');");
    let held = runtime::block_on(session::lease(&id, None, session::META)).unwrap();

    let job = session::open_table(&id, None, &owner(&id, "info-stop-pane"), &table("", "people"), None);
    let mut found = None;
    until("the rows never showed while the description was out", || {
        found =
            session::job_state(job).and_then(|s| s.results.first().filter(|r| r.rows == 1 && r.complete).map(|r| r.id));
        found.is_some()
    });
    session::cancel(job);
    let state = within(Duration::from_secs(5), "the Stop ending the job", move || wait(job));
    assert!(state.cancelled && state.error.is_none(), "{state:?}");
    let set = results::get(found.unwrap()).unwrap();
    let reason = results::lock(&set).read_only_reason().expect("read-only, with a reason");
    assert!(reason.contains("stopped"), "{reason}");
    drop(held);
}

/// The pane's Structure tab asks the job that reads the rows, not the sidebar's
/// connection a second time.
#[test]
fn sqlite_the_description_is_the_one_the_job_that_reads_the_rows_got() {
    let (id, _) = sqlite_connection("wb-info-api");
    run(
        &id,
        "setup",
        "create table people(id integer primary key, name text not null); insert into people values (1, 'a');",
    );
    let pane_owner = owner(&id, "info-api-pane");
    let job = session::open_table(&id, None, &pane_owner, &table("", "people"), None);
    let info = runtime::block_on(session::table_info(job)).unwrap();
    assert_eq!(info.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["id", "name"]);
    assert_eq!(info.key_columns(), ["id"]);
    assert!(info.ddl.contains("people"), "{}", info.ddl);
    wait(job);
    assert_eq!(runtime::block_on(session::table_info(job)).unwrap(), info, "asked again once the job is over");

    let missing = session::open_table(&id, None, &pane_owner, &table("", "nobody"), None);
    match runtime::block_on(session::table_info(missing)) {
        Err(DbError::NotFound(why)) => assert!(why.contains("nobody"), "the engine's own reason: {why}"),
        other => panic!("a table that is not there has no description, and says why: {other:?}"),
    }
    wait(missing);

    session::open_table(&id, None, &pane_owner, &table("", "people"), None);
    assert!(
        matches!(runtime::block_on(session::table_info(missing)), Err(DbError::NotFound(_))),
        "a job the pane moved on from is gone"
    );
    assert!(matches!(runtime::block_on(session::table_info(u64::MAX)), Err(DbError::NotFound(_))));
}

/// The rows of a table the engine reads but cannot describe (SQLite's own catalog has no row for
/// itself) show, read-only, and the reason is the engine's own: the one the Structure tab is given.
#[test]
fn sqlite_a_table_that_cannot_be_described_shows_its_rows_read_only_and_says_why_in_both_places() {
    let (id, _) = sqlite_connection("wb-info-failed");
    run(&id, "setup", "create table people(id integer primary key); insert into people values (1);");
    let job = session::open_table(&id, None, &owner(&id, "info-failed-pane"), &table("", "sqlite_master"), None);
    let asked = runtime::block_on(session::table_info(job));
    let state = wait(job);
    assert_eq!(state.error, None);
    let summary = &state.results[0];
    assert!(summary.rows > 0 && summary.complete, "the rows of a table that cannot be described show: {summary:?}");
    assert!(!summary.editable);
    let why = match asked {
        Err(DbError::NotFound(why)) if why.contains("sqlite_master") => why,
        other => panic!("the Structure tab was not told why: {other:?}"),
    };
    let reason = results::lock(&results::get(summary.id).unwrap()).read_only_reason().expect("a reason");
    assert!(reason.contains("could not read the table's key") && reason.contains(&why), "{reason}");
}

/// A job whose connection is gone never gets as far as a description: the Structure tab is told what
/// the job ends with, not that a driver failed.
#[test]
fn a_table_of_a_connection_that_is_gone_ends_with_the_reason_and_tells_the_structure_tab_the_same() {
    LazyLock::force(&HOME);
    let job =
        session::open_table("wb-no-such-connection", None, "gone-pane@wb-no-such-connection", &table("", "t"), None);
    let asked = runtime::block_on(session::table_info(job));
    let ended = wait(job).error.expect("the job says why it could not start");
    assert!(ended.contains("wb-no-such-connection"), "{ended}");
    assert_eq!(asked.map(|_| ()).unwrap_err().to_string(), ended);
}

// ---------------------------------------------------------------- a refresh reads the key the table has now

/// Nothing about a table is kept from one open to the next: a refresh asks
/// again, so edits are matched by the key the table has now, not the one it had.
#[test]
fn sqlite_a_refresh_after_the_primary_key_moved_writes_by_the_new_key_and_by_none_once_it_is_gone() {
    let (id, _) = sqlite_connection("wb-key-moved");
    run(
        &id,
        "setup",
        "create table t(id integer primary key, code text, v text); insert into t values (1, 'x', 'a'), (2, 'x', 'b');",
    );
    let key_of = |pane: &Opened| -> Vec<String> {
        let r = results::lock(&pane.set);
        r.target().map(|t| t.key.iter().map(|k| r.columns[*k].name.clone()).collect()).unwrap_or_default()
    };

    let pane = open(&id, "key-moved-pane", &table("", "t"));
    assert_eq!(key_of(&pane), ["id"]);

    run(
        &id,
        "ddl",
        "drop table t; create table t(id integer, code text, v text primary key); insert into t values (1, 'x', 'a'), (2, 'x', 'b');",
    );
    let pane = open(&id, "key-moved-pane", &table("", "t"));
    assert_eq!(key_of(&pane), ["v"], "the refresh read the key the table has now");
    pane.edit(pane.row_where("v", "b"), "code", Some("changed"));
    assert_eq!(runtime::block_on(session::write_back_set(&pane.set)), Ok(1));
    assert_eq!(rows(&id, "select id, code, v from t order by id"), [["1", "x", "a"], ["2", "changed", "b"]]);

    run(&id, "ddl", "drop table t; create table t(id integer, code text, v text); insert into t values (1, 'x', 'a');");
    let pane = open(&id, "key-moved-pane", &table("", "t"));
    assert!(!pane.summary().editable, "a table without a key is not editable");
    assert!(results::lock(&pane.set).read_only_reason().is_some_and(|r| r.contains("no primary key")));
}

// ---------------------------------------------------------------- the rows and the description are of one table

/// The rows of `table`, as text, in a fixed order, whatever its columns are called by now.
fn stored(connection: &str, table: &str) -> Vec<Vec<String>> {
    let mut all = rows(connection, &format!("select * from {table}"));
    all.sort();
    all
}

/// `t` opened with its description out (the sidebar's connection is held here by hand): once its
/// `rows` rows are read, `ddl` runs, and then the description is let in and the job ends. The rows
/// are older than the description by a change of the table.
fn rows_older_than_their_description(
    connection: &str,
    pane: &str,
    t: &ObjectRef,
    rows: usize,
    ddl: &str,
) -> std::sync::Arc<std::sync::Mutex<results::ResultSet>> {
    let held = runtime::block_on(session::lease(connection, None, session::META)).unwrap();
    let job = session::open_table(connection, None, &owner(connection, pane), t, None);
    let mut found = None;
    until("the rows never showed while the description was out", || {
        found = session::job_state(job)
            .and_then(|s| s.results.first().filter(|r| r.rows == rows && r.complete).map(|r| r.id));
        found.is_some()
    });
    run(connection, "ddl", ddl);
    drop(held);
    assert_eq!(wait(job).error, None);
    results::get(found.unwrap()).unwrap()
}

/// The rows of `t` (id, code, v: `1, 2, x` and `2, 1, y`) are older than its description by a rename
/// of columns: they carry the names they were read under and the description the names the table has
/// now, and a write-back that goes by both finds another row than the one the person edited. It has
/// to write nothing, and the result has to say why.
fn rows_older_than_the_names_in_their_description(connection: &str, pane: &str, t: &ObjectRef, ddl: &str) {
    let name = qualified(t, connection);
    let before = stored(connection, &name);
    assert_eq!(before, [["1", "2", "x"], ["2", "1", "y"]]);

    let set = rows_older_than_their_description(connection, pane, t, 2, ddl);
    let (row, column) = {
        let r = results::lock(&set);
        let v = r.columns.iter().position(|c| c.name == "v").expect("the rows were read under the name v");
        (r.rows.iter().position(|row| row[v].display() == "x").unwrap(), v)
    };
    results::lock(&set).set_edit(row, column, Cell::text("CHANGED"));
    let written = runtime::block_on(session::write_back_set(&set));
    assert_eq!(stored(connection, &name), before, "the write-back changed what the person did not edit ({written:?})");
    assert!(matches!(written, Err(DbError::NotEditable(_))), "{written:?}");
    let r = results::lock(&set);
    assert!(!r.editable(), "{:?}", r.editing);
    let why = r.read_only_reason().expect("read-only, with a reason");
    assert!(why.contains("changed"), "{why}");
}

/// `ALTER TABLE … RENAME COLUMN` for each of `renames`, in order, in the words of `engine`.
fn rename_columns(engine: &str, t: &str, renames: &[(&str, &str)]) -> String {
    renames
        .iter()
        .map(|(from, to)| match engine {
            "mssql" => format!("EXEC sp_rename N'{t}.{from}', N'{to}', N'COLUMN';"),
            _ => format!("ALTER TABLE {t} RENAME COLUMN {from} TO {to};"),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// `id` and `code` exchange their names.
const EXCHANGED: &[(&str, &str)] = &[("id", "tmp"), ("code", "id"), ("tmp", "code")];
/// `id` is called `code` from now on, and `code` is called `k`.
const MOVED: &[(&str, &str)] = &[("code", "k"), ("id", "code")];

fn sqlite_rows_older_than_their_names(name: &str, renames: &[(&str, &str)]) {
    let (id, _) = sqlite_connection(name);
    run(&id, "setup", "create table t(id integer primary key, code text, v text); insert into t values (1, '2', 'x'), (2, '1', 'y');");
    rows_older_than_the_names_in_their_description(&id, "renamed-pane", &table("", "t"), &rename_columns("sqlite", "t", renames));
}

fn postgres_rows_older_than_their_names(name: &str, renames: &[(&str, &str)]) {
    only_with!("postgres");
    let scratch = PgScratchDatabase::new(&format!("yz_wb_{name}_o2"));
    let id = scratch.connection();
    run(
        &id,
        "setup",
        "CREATE TABLE public.t(id int PRIMARY KEY, code text, v text); INSERT INTO public.t VALUES (1, '2', 'x'), (2, '1', 'y');",
    );
    rows_older_than_the_names_in_their_description(&id, "renamed-pane", &table("public", "t"), &rename_columns("postgres", "public.t", renames));
}

fn mssql_rows_older_than_their_names(name: &str, renames: &[(&str, &str)]) {
    only_with!("mssql");
    let scratch = ScratchDatabase::new(&format!("yz_wb_{name}_o2"));
    let id = scratch.connection();
    run(
        &id,
        "setup",
        "CREATE TABLE dbo.t(id int PRIMARY KEY, code nvarchar(10), v nvarchar(10)); INSERT INTO dbo.t VALUES (1, N'2', N'x'), (2, N'1', N'y');",
    );
    rows_older_than_the_names_in_their_description(&id, "renamed-pane", &table("dbo", "t"), &rename_columns("mssql", "dbo.t", renames));
}

#[test]
fn sqlite_rows_read_before_two_columns_exchanged_names_are_not_written_back() {
    sqlite_rows_older_than_their_names("wb-exchanged", EXCHANGED);
}

#[test]
fn sqlite_rows_read_before_the_key_column_took_another_name_are_not_written_back() {
    sqlite_rows_older_than_their_names("wb-moved", MOVED);
}

#[test]
fn postgres_rows_read_before_two_columns_exchanged_names_are_not_written_back() {
    postgres_rows_older_than_their_names("exchanged", EXCHANGED);
}

#[test]
fn postgres_rows_read_before_the_key_column_took_another_name_are_not_written_back() {
    postgres_rows_older_than_their_names("moved", MOVED);
}

#[test]
fn mssql_rows_read_before_two_columns_exchanged_names_are_not_written_back() {
    mssql_rows_older_than_their_names("exchanged", EXCHANGED);
}

#[test]
fn mssql_rows_read_before_the_key_column_took_another_name_are_not_written_back() {
    mssql_rows_older_than_their_names("moved", MOVED);
}

/// Rows that name a column the table no longer has are not the table's: whatever else they agree on,
/// nothing can be said about which row or column an edit of them means, so they are read-only.
#[test]
fn sqlite_rows_read_before_a_column_was_dropped_are_read_only() {
    let (id, _) = sqlite_connection("wb-dropped-under");
    run(&id, "setup", "create table t(id integer primary key, x text, y text, z text); insert into t values (1, 'x1', 'y1', 'z1');");
    let set = rows_older_than_their_description(&id, "dropped-under-pane", &table("", "t"), 1, "alter table t drop column x");
    let r = results::lock(&set);
    assert_eq!(r.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["id", "x", "y", "z"], "the rows are the old ones");
    assert!(!r.editable(), "{:?}", r.editing);
    let why = r.read_only_reason().expect("read-only, with a reason");
    assert!(why.contains("changed"), "{why}");
}

/// Tables a person edits every day whose `SELECT *` and description do not list the same columns in
/// the same way (a dropped column, a generated one, a hidden one, one folded into a column set, names
/// with spaces and capitals). The rows are of the table they were read from: the check that the
/// rows and the description are of one table must not take any of them for another.
/// Each is (table, a text column to edit).
fn tables_with_columns_of_their_own_stay_editable(connection: &str, schema: &str, tables: &[(&str, &str)]) {
    let mut failed = Vec::new();
    for (n, (name, column)) in tables.iter().enumerate() {
        let pane_owner = owner(connection, &format!("ordinary-pane-{n}"));
        let state = wait(session::open_table(connection, None, &pane_owner, &table(schema, name), None));
        if let Some(error) = state.error {
            failed.push(format!("{name}: did not open ({error})"));
            continue;
        }
        let summary = &state.results[0];
        if !summary.editable {
            failed.push(format!("{name}: not editable ({:?})", summary.not_editable));
            continue;
        }
        let pane = Opened { owner: pane_owner, id: summary.id, set: results::get(summary.id).expect("result") };
        pane.edit(0, column, Some("changed"));
        match runtime::block_on(session::write_back_set(&pane.set)) {
            Ok(1) => {}
            other => failed.push(format!("{name}: the write-back came to {other:?}")),
        }
        let again = open(connection, &format!("ordinary-pane-{n}"), &table(schema, name));
        let at = again.column(column);
        let shown = results::lock(&again.set).rows[0][at].display().into_owned();
        if shown != "changed" {
            failed.push(format!("{name}: {column} reads {shown:?} after the write-back"));
        }
        session::release_owner(&pane.owner);
    }
    assert!(failed.is_empty(), "{} of {} tables:\n{}", failed.len(), tables.len(), failed.join("\n"));
}

/// A view has no primary key; that is what it says, not that the table changed.
fn views_say_they_have_no_key(connection: &str, schema: &str, views: &[&str]) {
    for (n, name) in views.iter().enumerate() {
        let state = wait(session::open_table(connection, None, &owner(connection, &format!("view-pane-{n}")), &table(schema, name), None));
        assert_eq!(state.error, None, "{name}");
        let summary = &state.results[0];
        assert!(!summary.editable, "{name}");
        let why = summary.not_editable.clone().unwrap_or_default();
        assert!(why.contains("no primary key"), "{name}: {why}");
    }
}

#[test]
fn sqlite_tables_with_columns_of_their_own_stay_editable() {
    let (id, _) = sqlite_connection("wb-own-columns");
    run(
        &id,
        "setup",
        "create table dropped(id integer primary key, gone text, v text); insert into dropped values (1, 'g', 'a');
         alter table dropped drop column gone;
         create table gen(id integer primary key, a int, b int generated always as (a * 2) stored, c int as (a + 1), v text);
         insert into gen (id, a, v) values (1, 2, 'a');
         create table \"Mixed Case\"(\"Id\" integer primary key, \"Some Col\" text, \"ünï\" text);
         insert into \"Mixed Case\" values (1, 'a', 'b');
         create table plain(a int, v text, b int, primary key (b, a)) without rowid; insert into plain values (1, 'a', 2);
         create view seen as select id, v || '!' as w, id from dropped;",
    );
    tables_with_columns_of_their_own_stay_editable(
        &id,
        "",
        &[("dropped", "v"), ("gen", "v"), ("Mixed Case", "Some Col"), ("Mixed Case", "ünï"), ("plain", "v")],
    );
    views_say_they_have_no_key(&id, "", &["seen"]);
}

#[test]
fn postgres_tables_with_columns_of_their_own_stay_editable() {
    only_with!("postgres");
    let scratch = PgScratchDatabase::new("yz_wb_own_o2");
    let id = scratch.connection();
    run(
        &id,
        "setup",
        "CREATE TABLE dropped(id int PRIMARY KEY, gone text, v text); INSERT INTO dropped VALUES (1, 'g', 'a');
         ALTER TABLE dropped DROP COLUMN gone;
         CREATE TABLE gen(id int PRIMARY KEY, a int, b int GENERATED ALWAYS AS (a * 2) STORED, v text);
         INSERT INTO gen (id, a, v) VALUES (1, 2, 'a');
         CREATE TABLE ident(id int GENERATED ALWAYS AS IDENTITY PRIMARY KEY, v text); INSERT INTO ident (v) VALUES ('a');
         CREATE TABLE \"Mixed Case\"(\"Id\" int PRIMARY KEY, \"Some Col\" text, \"ünï\" text); INSERT INTO \"Mixed Case\" VALUES (1, 'a', 'b');
         CREATE TABLE plain(a int, v text, b int, PRIMARY KEY (b, a)); INSERT INTO plain VALUES (1, 'a', 2);
         CREATE VIEW seen AS SELECT id, v || '!' AS w FROM dropped;",
    );
    tables_with_columns_of_their_own_stay_editable(
        &id,
        "public",
        &[("dropped", "v"), ("gen", "v"), ("ident", "v"), ("Mixed Case", "Some Col"), ("Mixed Case", "ünï"), ("plain", "v")],
    );
    views_say_they_have_no_key(&id, "public", &["seen"]);
}

#[test]
fn mssql_tables_with_columns_of_their_own_stay_editable() {
    only_with!("mssql");
    let scratch = ScratchDatabase::new("yz_wb_own_o2");
    let id = scratch.connection();
    run(
        &id,
        "setup",
        "CREATE TABLE dbo.computed(id int PRIMARY KEY, a int, b AS (a * 2) PERSISTED, v nvarchar(10)); INSERT INTO dbo.computed (id, a, v) VALUES (1, 2, N'a');
         CREATE TABLE dbo.ident(id int IDENTITY PRIMARY KEY, v nvarchar(10)); INSERT INTO dbo.ident (v) VALUES (N'a');
         CREATE TABLE dbo.[Mixed Case]([Id] int PRIMARY KEY, [Some Col] nvarchar(10), [ünï] nvarchar(10)); INSERT INTO dbo.[Mixed Case] VALUES (1, N'a', N'b');
         CREATE TABLE dbo.plain(a int, v nvarchar(10), b int, PRIMARY KEY (b, a)); INSERT INTO dbo.plain VALUES (1, N'a', 2);
         CREATE TABLE dbo.temporal(id int NOT NULL PRIMARY KEY CLUSTERED, v nvarchar(10),
             ValidFrom datetime2 GENERATED ALWAYS AS ROW START HIDDEN NOT NULL, ValidTo datetime2 GENERATED ALWAYS AS ROW END HIDDEN NOT NULL,
             PERIOD FOR SYSTEM_TIME (ValidFrom, ValidTo)) WITH (SYSTEM_VERSIONING = ON (HISTORY_TABLE = dbo.temporal_history));
         INSERT INTO dbo.temporal (id, v) VALUES (1, N'a');
         CREATE TABLE dbo.sparse(id int PRIMARY KEY, s1 int SPARSE NULL, s2 nvarchar(10) SPARSE NULL, cs xml COLUMN_SET FOR ALL_SPARSE_COLUMNS, v nvarchar(10));
         INSERT INTO dbo.sparse (id, v) VALUES (1, N'a');\nGO\n\
         CREATE VIEW dbo.seen AS SELECT id, v + N'!' AS w FROM dbo.ident",
    );
    tables_with_columns_of_their_own_stay_editable(
        &id,
        "dbo",
        &[
            ("computed", "v"),
            ("ident", "v"),
            ("Mixed Case", "Some Col"),
            ("Mixed Case", "ünï"),
            ("plain", "v"),
            ("temporal", "v"),
            ("sparse", "v"),
        ],
    );
    views_say_they_have_no_key(&id, "dbo", &["seen"]);
}

// ---------------------------------------------------------------- a SQLite statement names the columns it returns

/// SQLite compiles a statement against the schema this connection last read; another connection that
/// changed the table is noticed when the statement first runs, and the statement is compiled again
/// there. The names of the columns have to be those of the statement that ran, or the values of one
/// table sit under the headings of another.
fn a_table_another_connection_dropped_a_column_of(file_name: &str, pane: &str) -> (String, Opened) {
    let (id, _) = sqlite_connection(file_name);
    run(&id, "setup", "create table t(id integer primary key, x text, y text, z text); insert into t values (1, 'x1', 'y1', 'z1');");
    let first = open(&id, pane, &table("", "t"));
    assert_eq!(results::lock(&first.set).columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["id", "x", "y", "z"]);
    run(&id, "ddl", "alter table t drop column x");
    let refreshed = open(&id, pane, &table("", "t"));
    (id, refreshed)
}

#[test]
fn sqlite_a_refresh_after_another_connection_dropped_a_column_shows_the_columns_the_table_has_now() {
    let (_, pane) = a_table_another_connection_dropped_a_column_of("wb-dropped-names", "dropped-names-pane");
    let r = results::lock(&pane.set);
    assert_eq!(r.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["id", "y", "z"]);
    let shown: Vec<_> = r.rows[0].iter().map(|c| c.display().into_owned()).collect();
    assert_eq!(shown, ["1", "y1", "z1"]);
}

/// Whichever way a table the columns of which changed under the pane ends up, editable or not, an edit
/// is saved into the column it was made in, or not at all.
#[test]
fn sqlite_a_refresh_after_another_connection_dropped_a_column_never_writes_into_a_column_it_did_not_show() {
    let (id, pane) = a_table_another_connection_dropped_a_column_of("wb-dropped-writes", "dropped-writes-pane");
    let shown = {
        let r = results::lock(&pane.set);
        let y = r.columns.iter().position(|c| c.name == "y").expect("a column y");
        r.rows[0][y].display().into_owned()
    };
    pane.edit(0, "y", Some("CHANGED"));
    let written = runtime::block_on(session::write_back_set(&pane.set));
    let stored = first_row(&id, "select y, z from t");
    match written {
        Ok(_) => {
            assert_eq!(shown, "y1", "the cell that was edited showed what another column holds");
            assert_eq!(stored, ["CHANGED", "z1"]);
        }
        Err(_) => assert_eq!(stored, ["y1", "z1"], "refused, so nothing was written"),
    }
}

#[test]
fn sqlite_a_query_after_another_connection_dropped_a_column_names_the_columns_it_returns() {
    let (id, _) = sqlite_connection("wb-dropped-query");
    run(&id, "setup", "create table t(id integer primary key, x text, y text, z text); insert into t values (1, 'x1', 'y1', 'z1');");
    let pane = owner(&id, "dropped-query-pane");
    let names = |state: &JobState| {
        let set = results::get(state.results.last().expect("a result").id).expect("result");
        let r = results::lock(&set);
        (r.columns.iter().map(|c| c.name.clone()).collect::<Vec<_>>(), r.rows[0].iter().map(|c| c.display().into_owned()).collect::<Vec<_>>())
    };
    let first = wait(session::start_query(&id, None, &pane, "select * from t"));
    assert_eq!(names(&first).0, ["id", "x", "y", "z"]);
    run(&id, "ddl", "alter table t drop column x");
    let again = wait(session::start_query(&id, None, &pane, "select * from t"));
    assert_eq!(again.error, None);
    assert_eq!(names(&again), (vec!["id".to_owned(), "y".into(), "z".into()], vec!["1".to_owned(), "y1".into(), "z1".into()]));
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
    drop(runtime::block_on(session::lease(&id, None, &session::table_owner(&pane_owner))).unwrap());
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

/// A pane with a table open and a SQL tab is two connections where it used to be one. On a
/// login that may have two (the sidebar's and the table's) the SQL tab is the one that finds
/// no room: it says why, and the table goes on being read and saved.
#[test]
fn postgres_a_sql_tab_beside_a_table_with_no_connection_to_spare_says_why_and_leaves_the_table_alone() {
    only_with!("postgres");
    let maintenance = postgres_in("pg-maintenance-r2lim", "postgres");
    run(&maintenance, "role", "DROP ROLE IF EXISTS yz_r2_lim");
    run(&maintenance, "role", "CREATE ROLE yz_r2_lim LOGIN PASSWORD 'yz-r2-lim-pw' CONNECTION LIMIT 2");
    let _role = PgRole { maintenance: maintenance.clone(), name: "yz_r2_lim" };
    let scratch = PgScratchDatabase::new("yz_r2_limit");
    let setup = scratch.connection();
    run(&setup, "setup", "CREATE TABLE t(id int PRIMARY KEY, name text); INSERT INTO t VALUES (1, 'a'), (2, 'b'); GRANT SELECT, UPDATE ON t TO yz_r2_lim;");
    let limited = save(
        json!({"name": "pg-r2-limited", "engine": "postgres", "host": "127.0.0.1", "port": 55432,
               "user": "yz_r2_lim", "database": "yz_r2_limit", "ssl": "prefer"}),
        Some("yz-r2-lim-pw"),
    );
    let pane_owner = owner(&limited, "lim-pane");
    let opened = wait(session::open_table(&limited, None, &pane_owner, &table("public", "t"), None));
    assert_eq!(opened.error, None);
    assert_eq!(session::status(&limited).state, session::State::Connected);

    let tab = wait(session::start_query(&limited, None, &pane_owner, "SELECT 1"));
    let said = tab.error.expect("a third connection is not allowed");
    assert!(said.contains("too many connections"), "the SQL tab says why it has no connection: {said}");

    // The SQL tab replaced the table's result (that is what running anything there does):
    // the table is opened again, on the connection it already has, and saved.
    let opened = wait(session::open_table(&limited, None, &pane_owner, &table("public", "t"), None));
    assert_eq!(opened.error, None);
    let set = results::get(opened.results[0].id).unwrap();
    results::lock(&set).set_edit(0, 1, Cell::text("mine"));
    assert_eq!(runtime::block_on(session::write_back_set(&set)), Ok(1), "the table is saved though the SQL tab found no room");
    assert_eq!(first_row(&setup, "SELECT name FROM t WHERE id = 1"), ["mine"]);
    session::release_owner(&pane_owner);
    session::disconnect(&limited);
}

/// A login that may have two connections, the table's still in the pool and the sidebar's gone (the
/// server dropped it, or the sidebar moved to another database): the description's connection and
/// the count's both want the one place left, and the description's has to be the one that gets it,
/// as it was when the description came first. Whether it is depends on which gets there first, so
/// this repeats (60 opens); without the count waiting for the description's connection, one open
/// in ten to twenty-five ended read-only with `too many connections`.
#[test]
fn postgres_a_login_limited_to_two_connections_keeps_the_table_editable_when_the_sidebars_connection_is_gone() {
    only_with!("postgres");
    let _role = limited_role("yz_wb_order");
    let scratch = PgScratchDatabase::new("yz_wb_order");
    let setup = scratch.connection();
    run(
        &setup,
        "setup",
        "CREATE TABLE t(id int PRIMARY KEY, name text); INSERT INTO t SELECT g, 'a' FROM generate_series(1, 50) g; GRANT SELECT, UPDATE ON t TO yz_wb_order;",
    );
    let limited = save(
        json!({"name": "pg-order", "engine": "postgres", "host": "127.0.0.1", "port": 55432,
               "user": "yz_wb_order", "database": "yz_wb_order", "ssl": "prefer"}),
        Some("yz_wb_order-pw"),
    );
    let pane = owner(&limited, "order-pane");
    let opened = wait(session::open_table(&limited, None, &pane, &table("public", "t"), None));
    assert!(opened.results[0].editable, "{:?}", opened.results[0]);

    let mut refused = Vec::new();
    for round in 0..60 {
        // The count borrowed the table's connection, so its last statement is the count's: the other
        // backend of this login is the sidebar's.
        let killed = rows(
            &setup,
            "SELECT pid::text, pg_terminate_backend(pid)::text FROM pg_stat_activity WHERE usename = 'yz_wb_order' AND query NOT ILIKE '%count(%'",
        );
        // The server is done with them when they are gone from its list; an open that reached the
        // client before it saw the connection closed would be the drop itself (a dead connection handed
        // out), not the order of the connections.
        let pids: Vec<_> = killed.iter().map(|row| row[0].as_str()).collect();
        if !pids.is_empty() {
            let count_left = format!("SELECT count(*)::text FROM pg_stat_activity WHERE pid IN ({})", pids.join(", "));
            until("the server still lists the backends it was told to end", || first_row(&setup, &count_left) == ["0"]);
        }
        let state = wait(session::open_table(&limited, None, &pane, &table("public", "t"), None));
        let summary = &state.results[0];
        if !summary.editable || state.error.is_some() {
            refused.push((round, summary.not_editable.clone(), state.error.clone()));
        }
    }
    session::release_owner(&pane);
    session::disconnect(&limited);
    assert!(refused.is_empty(), "opens that did not end editable: {refused:?}");
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

    let next = session::start_query(&through, None, &session::table_owner(&pane.owner), "SELECT pg_sleep(2.5)");
    results::lock(&pane.set).stop_write();
    // The lock goes only after the write-back has sent its Stop: otherwise a write-back that has not
    // looked at the Stop yet is stopped by the commit step instead, and no Stop is ever sent to be late.
    until("the write-back never sent its Stop", || relay.stops_arrived() == 1);
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
        relay.heard_until(2),
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

/// The same, with the table opening next (its read is what the request finds):
/// the stop is for a statement on the table's own connection, which is the one
/// the table is read on. A SQL tab's stop is for another connection, and no
/// request of it can find the table (see the test below).
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
    let tables_connection = session::table_owner(&pane);
    assert_eq!(wait(session::start_query(&id, None, &tables_connection, "SELECT 1")).error, None);
    relay.hold_cancels(Duration::from_millis(700));

    let first = session::start_query(&id, None, &tables_connection, "DO $$ BEGIN PERFORM pg_sleep(0.3); END $$");
    std::thread::sleep(Duration::from_millis(150));
    session::cancel(first);
    assert_eq!(within(Duration::from_secs(10), "the query that ended by itself", move || wait(first)).error, None);

    let opened = session::open_table(&id, None, &pane, &table("public", "slow"), None);
    let state = within(Duration::from_secs(30), "the table", move || wait(opened));
    assert_eq!(state.error, None, "the Stop reached the table the pane opened after the query: {state:?}");
    assert_eq!(state.results.first().map(|r| (r.rows, r.error.clone())), Some((1, None)), "{state:?}");
}

/// A stop is for one connection. With a request for `stopped` held back from
/// the server (the relay has not let it through, however long this takes), the
/// pane's `other` connection is leased and used at once, and `stopped` is not
/// handed to new work before the server has dealt with the request.
fn a_stop_on_the_way_holds_back_only_its_own_connection(
    relay: &common::LaggingPostgres,
    id: &str,
    stopped: &str,
    other: &str,
) {
    relay.hold_cancels_until_released();
    let before = relay.heard().len();
    let mut lease = runtime::block_on(session::lease(id, None, stopped)).unwrap();
    lease.conn().canceller().cancel_in_background();
    drop(lease);
    // A stop the server does not deal with in 5 s is given up on, and the connection with it
    // (`CANCEL_WAIT`): from here to the release below has to take less than that.
    let pressed = Instant::now();

    let (connection, alone) = (id.to_owned(), other.to_owned());
    let used =
        within(Duration::from_secs(4), "the other connection, while a stop is on its way for this one", move || {
            wait(session::start_query(&connection, None, &alone, "SELECT 1"))
        });
    assert_eq!(used.error, None, "{used:?}");
    assert_eq!(
        relay.heard().len(),
        before,
        "the request got to the server through the relay that holds it: the premise is gone"
    );

    let (heard, outcome) = mpsc::channel();
    std::thread::scope(|scope| {
        scope.spawn(|| {
            let leased = runtime::block_on(session::lease(id, None, stopped)).map(|_| ());
            let _ = heard.send((leased, relay.heard().split_off(before)));
        });
        let early = outcome.recv_timeout(Duration::from_millis(500));
        relay.release();
        assert!(
            early.is_err(),
            "the connection was handed to new work with a stop still on its way to the server ({:?} after the stop; \
             the server is given 5 s): {early:?}",
            pressed.elapsed()
        );
        let (leased, heard) = outcome
            .recv_timeout(Duration::from_secs(10))
            .expect("the connection, once the server has dealt with the stop");
        assert_eq!(leased, Ok(()));
        assert_eq!(
            heard,
            [common::Heard::StopDealtWith],
            "the connection was handed on before the server had dealt with the stop"
        );
    });
    let after = wait(session::start_query(id, None, stopped, "SELECT 1"));
    assert_eq!(after.error, None, "the connection the stop was dealt with on is still whole: {after:?}");
}

/// The pane's SQL tab and its table are two connections and keep their stops
/// apart: a stop on the way for the table is waited for by the table alone,
/// and the same for the tab. (`open_lease` is where it is waited for, and a
/// lease is per connection.)
#[test]
fn postgres_a_stop_on_the_tables_connection_is_settled_there_and_holds_back_only_the_table() {
    only_with!("postgres");
    let relay = common::LaggingPostgres::start();
    let id = postgres_behind_slow_cancels("pg-own-stop", "yuzora_p8", &relay);
    let pane = owner(&id, "own-stop");
    let (tab, table_connection) = (pane.clone(), session::table_owner(&pane));
    for owner in [&tab, &table_connection] {
        assert_eq!(wait(session::start_query(&id, None, owner, "SELECT 1")).error, None);
    }
    a_stop_on_the_way_holds_back_only_its_own_connection(&relay, &id, &table_connection, &tab);
    a_stop_on_the_way_holds_back_only_its_own_connection(&relay, &id, &tab, &table_connection);
}

/// The same on SQL Server, where a stop ends the connection it is for: the
/// write-back's ends the table's socket and the SQL tab's statement goes on.
#[test]
fn mssql_a_sql_tab_statement_and_a_waiting_write_back_are_stopped_apart() {
    only_with!("mssql");
    let scratch = ScratchDatabase::new("yz_wb_twostops");
    let id = scratch.connection();
    run(&id, "setup", "CREATE TABLE dbo.t(id int PRIMARY KEY, name nvarchar(20)); INSERT INTO dbo.t VALUES (1, N'a');");
    let pane = open(&id, "two-stops", &table("dbo", "t"));
    pane.edit(pane.row_where("id", "1"), "name", Some("mine"));
    run(&id, "locker", "BEGIN TRANSACTION; UPDATE dbo.t SET name = N'locked' WHERE id = 1");
    let writer = {
        let set = pane.set.clone();
        std::thread::spawn(move || runtime::block_on(session::write_back_set(&set)))
    };
    let count = |what: &str| {
        first_row(
            &id,
            &format!(
                "SELECT CAST(count(*) AS varchar(5)) FROM sys.dm_exec_requests WHERE {what} AND database_id = DB_ID(N'yz_wb_twostops')"
            ),
        )
    };
    until("the server never saw the write-back wait for the lock", || count("blocking_session_id <> 0") == ["1"]);
    let waiting = || count("command = N'WAITFOR'") == ["1"];

    let query = session::start_query(&id, None, &pane.owner, "WAITFOR DELAY '00:01:00'");
    until("the SQL tab's statement never ran beside the write-back that waits", waiting);
    session::cancel(query);
    assert!(within(Duration::from_secs(10), "the SQL tab's Stop", move || wait(query)).cancelled);
    assert!(results::lock(&pane.set).is_writing(), "the SQL tab's Stop ended the write-back");
    assert_eq!(count("blocking_session_id <> 0"), ["1"]);

    let query = session::start_query(&id, None, &pane.owner, "WAITFOR DELAY '00:01:00'");
    until("the SQL tab's second statement never ran", waiting);
    results::lock(&pane.set).stop_write();
    assert_eq!(
        within(Duration::from_secs(10), "the stopped write-back", move || writer.join().unwrap()),
        Err(DbError::Cancelled)
    );
    assert!(waiting(), "the write-back's Stop ended the SQL tab's statement");
    assert!(session::job_state(query).unwrap().running);
    session::cancel(query);
    assert!(within(Duration::from_secs(10), "the SQL tab's Stop", move || wait(query)).cancelled);
    run(&id, "locker", "ROLLBACK TRANSACTION");
    assert_eq!(first_row(&id, "SELECT name FROM dbo.t WHERE id = 1"), ["a"], "the stopped write-back wrote");
}

/// What a value is shown and written by is set where the table's connection is opened, and on
/// the one that takes the place of it when it is retired, whatever the database says as its own.
/// The SQL tab and the count keep the database's: a person's script reads dates and floats the
/// way the server prints them, as `psql` does.
#[test]
fn postgres_only_the_connection_a_table_is_read_and_written_on_is_opened_with_the_settings_a_value_is_read_and_written_by()
{
    only_with!("postgres");
    let scratch = PgScratchDatabase::new("yz_r3_printread");
    let db = scratch.name.clone();
    run(
        &scratch.maintenance,
        "style",
        &format!("ALTER DATABASE {db} SET datestyle = 'SQL, DMY'; ALTER DATABASE {db} SET extra_float_digits = 0"),
    );
    let id = scratch.connection();
    let pane = owner(&id, "print-read");
    let says = |on: &str| -> Vec<String> {
        let sql = "SELECT current_setting('datestyle'), current_setting('extra_float_digits'), pg_backend_pid()::text, \
                   TIMESTAMP '2024-01-06 10:00:00'::text, (0.1::float8 + 0.2::float8)::text";
        let state = wait(session::start_query(&id, None, on, sql));
        assert_eq!(state.error, None);
        let set = results::get(state.results.last().expect("a result").id).expect("result");
        let r = results::lock(&set);
        r.rows[0].iter().map(|c| c.display().into_owned()).collect()
    };
    let as_psql = ["SQL, DMY", "0", "06/01/2024 10:00:00", "0.3"];
    let as_read = ["ISO, DMY", "3", "2024-01-06 10:00:00", "0.30000000000000004"];
    let shown = |a: &[String]| [a[0].as_str(), a[1].as_str(), a[3].as_str(), a[4].as_str()].map(str::to_owned);

    let tab = says(&pane);
    assert_eq!(shown(&tab), as_psql, "the SQL tab prints what the database says");
    let count = says(&session::count_owner(&pane));
    assert_eq!(shown(&count), as_psql, "the count's connection is the database's too");

    let table_connection = session::table_owner(&pane);
    let table = says(&table_connection);
    assert_eq!(shown(&table), as_read, "the table's connection");
    let mut lease = runtime::block_on(session::lease(&id, None, &table_connection)).unwrap();
    runtime::block_on(lease.conn().canceller().abandon());
    drop(lease);
    let again = says(&table_connection);
    assert_eq!(shown(&again), as_read, "the connection that took the place of the retired one");
    assert_ne!(again[2], table[2], "the retired connection was used again");
    assert_eq!(shown(&says(&pane)), as_psql, "the SQL tab is untouched by the table's");
}

/// Closing a pane closes every connection it holds (the SQL tab's, the table's, the count's) and
/// only those: it is what ends a transaction a SQL tab left open, and frees a login's connections.
#[test]
fn postgres_closing_a_pane_closes_the_connections_it_holds_and_no_other() {
    only_with!("postgres");
    let scratch = PgScratchDatabase::new("yz_r3_closepane");
    let id = scratch.connection();
    let (pane, neighbour) = (owner(&id, "closing"), owner(&id, "staying"));
    let pid_on = |on: &str| -> String { first_row_on(&id, on, "SELECT pg_backend_pid()::text").remove(0) };
    let watcher = raw_postgres(&scratch.name);
    let alive = |pid: &str| -> bool {
        let found = runtime::block_on(watcher.query_one(
            &format!("SELECT count(*)::int FROM pg_stat_activity WHERE pid = {pid}"),
            &[],
        ))
        .unwrap();
        found.get::<_, i32>(0) == 1
    };
    let held: Vec<(String, String)> = [pane.clone(), session::table_owner(&pane), session::count_owner(&pane)]
        .into_iter()
        .map(|on| (pid_on(&on), on))
        .collect();
    let kept = pid_on(&neighbour);
    for (pid, on) in &held {
        assert!(alive(pid), "{on} should be open before the pane closes");
    }

    session::release_owner(&pane);
    for (pid, on) in &held {
        until(&format!("{on}: its connection stayed open after the pane closed"), || !alive(pid));
    }
    assert!(alive(&kept), "another pane's connection was closed with it");
    session::release_owner(&neighbour);
    session::disconnect(&id);
}

/// The query in the SQL tab and the write-back of the table run side by side,
/// each on its connection: neither waits behind the other, and the Stop of one
/// does not end the other.
#[test]
fn postgres_a_sql_tab_statement_and_a_waiting_write_back_are_stopped_apart() {
    only_with!("postgres");
    let scratch = PgScratchDatabase::new("yz_wb_twostops");
    let direct = scratch.connection();
    run(&direct, "setup", "CREATE TABLE t(id int PRIMARY KEY, name text); INSERT INTO t VALUES (1, 'a');");
    let locker = raw_postgres("yz_wb_twostops");
    let pane = open(&direct, "two-stops", &table("public", "t"));
    pane.edit(pane.row_where("id", "1"), "name", Some("mine"));
    runtime::block_on(locker.batch_execute("BEGIN; UPDATE t SET name = 'holder' WHERE id = 1")).unwrap();
    let writer = writing_on_a_row_lock(&pane, &locker, "yz_wb_twostops");
    // Not the locker: inside its transaction the server shows it the activity as it was at the first look.
    let watcher = raw_postgres("yz_wb_twostops");
    let sleeping = || {
        let found = runtime::block_on(watcher.query_one(
            "SELECT count(*)::int FROM pg_stat_activity WHERE datname = 'yz_wb_twostops' AND state = 'active' \
             AND query LIKE '%pg_sleep(60)%' AND pid <> pg_backend_pid()",
            &[],
        ))
        .unwrap();
        found.get::<_, i32>(0) == 1
    };

    let query = session::start_query(&direct, None, &pane.owner, "SELECT pg_sleep(60)");
    until("the SQL tab's statement never ran beside the write-back that waits", sleeping);
    session::cancel(query);
    assert!(within(Duration::from_secs(10), "the SQL tab's Stop", move || wait(query)).cancelled);
    assert!(results::lock(&pane.set).is_writing(), "the SQL tab's Stop ended the write-back");

    let query = session::start_query(&direct, None, &pane.owner, "SELECT pg_sleep(60)");
    until("the SQL tab's second statement never ran", sleeping);
    results::lock(&pane.set).stop_write();
    assert_eq!(
        within(Duration::from_secs(10), "the stopped write-back", move || writer.join().unwrap()),
        Err(DbError::Cancelled)
    );
    assert!(sleeping(), "the write-back's Stop ended the SQL tab's statement");
    assert!(session::job_state(query).unwrap().running);
    session::cancel(query);
    assert!(within(Duration::from_secs(10), "the SQL tab's Stop", move || wait(query)).cancelled);
    runtime::block_on(locker.batch_execute("ROLLBACK")).unwrap();
    assert_eq!(first_row(&direct, "SELECT name FROM t WHERE id = 1"), ["a"], "the stopped write-back wrote");
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
        // Microseconds between the Stop and the lock being let go; negative: the Stop first, which
        // is the order they are called in, whatever the write-back's task got to see of them when.
        // The cancel lands late when the two are within a millisecond or two of each other.
        let offset = (seed % 1601) as i64 - 1500;
        let pane = open(&id, "sweep-pane", &t);
        pane.edit(pane.row_where("id", "1"), "name", Some(&format!("mine-{round}")));
        runtime::block_on(locker.batch_execute("BEGIN; UPDATE t SET name = 'holder' WHERE id = 1")).unwrap();
        let writer = writing_on_a_row_lock(&pane, &locker, "yz_wb_sweep");
        let next = session::start_query(&id, None, &session::table_owner(&pane.owner), "SELECT pg_sleep(0.08)");
        std::thread::sleep(Duration::from_millis(10));

        let spin = |micros: i64| {
            let until = Instant::now() + Duration::from_micros(micros.unsigned_abs());
            while Instant::now() < until {
                std::hint::spin_loop();
            }
        };
        let stop_first = offset < 0;
        if stop_first {
            results::lock(&pane.set).stop_write();
            spin(offset);
            runtime::block_on(locker.batch_execute("COMMIT")).unwrap();
        } else {
            runtime::block_on(locker.batch_execute("COMMIT")).unwrap();
            spin(offset);
            results::lock(&pane.set).stop_write();
        }
        let written = writer.join().unwrap();
        let nothing_written = results::lock(&pane.set).write_failure().map(str::to_owned)
            == Some(yuzora_db::i18n::t("edit.stopped", &[]));

        let state = wait(next);
        assert_eq!(
            state.error, None,
            "round {round} (offset {offset} µs): the Stop reached what ran after the write-back"
        );
        let alive = wait(session::start_query(&id, None, &session::table_owner(&pane.owner), "SELECT 1"));
        assert_eq!(alive.error, None, "round {round} (offset {offset} µs): the pane's connection is left broken");
        let name: String = runtime::block_on(locker.query_one("SELECT name FROM t WHERE id = 1", &[])).unwrap().get(0);
        if stop_first {
            stop_came_first += 1;
            assert_eq!(written, Err(DbError::Cancelled), "round {round} (offset {offset} µs)");
            assert_eq!(
                name, "holder",
                "round {round} (offset {offset} µs): a write-back stopped before the lock let go wrote anyway"
            );
            assert!(nothing_written, "round {round} (offset {offset} µs): it did not say that nothing was written");
        } else {
            let (mine, holder) = (name == format!("mine-{round}"), name == "holder");
            assert!(mine || holder, "round {round} (offset {offset} µs): {name}");
            assert!(
                written != Ok(1) || mine,
                "round {round} (offset {offset} µs): it said it wrote, and {name} is there"
            );
            assert!(
                !nothing_written || holder,
                "round {round} (offset {offset} µs): it said nothing was written, and {name} is there"
            );
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
    assert_eq!(wait(session::start_query(&direct, None, &session::table_owner(&pane.owner), "SELECT 1")).error, None);
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
    let owner = session::table_owner(&pane.owner);
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

/// Whatever else asks for the table's connection — a refresh, a save — gets it
/// at once when a count has borrowed it. (A SQL tab's query has a connection of
/// its own: `a_count_on_the_tables_connection_steps_aside_for_the_tables_work_and_not_for_the_sql_tabs`.)
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
    drop(runtime::block_on(session::lease(&id, None, &session::table_owner(&pane_owner))).unwrap());
    let moved = file.with_extension("moved");
    std::fs::rename(&file, &moved).unwrap();
    let job = session::open_table(&id, None, &pane_owner, &t, None);
    until("the rows never arrived", || {
        session::job_state(job).is_some_and(|s| s.results.first().is_some_and(|r| r.rows > 0 && r.complete))
    });
    std::thread::sleep(Duration::from_millis(300));
    assert!(session::job_state(job).unwrap().running, "the count is running");

    let (connection, pane) = (id.clone(), session::table_owner(&pane_owner));
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

// ---------------------------------------------------------------- dates, whatever the session reads them as

const LOGIN_PASSWORD: &str = "Yz-Dt-Only-2026!";

/// Runs `sql` and waits for it; whatever it says, it says nothing (this runs while a test unwinds).
fn quietly(connection: &str, sql: &str) {
    let job = session::start_query(connection, None, &owner(connection, "quietly"), sql);
    let mut seen = 0;
    while let Ok(state) = runtime::block_on(session::job_after(job, seen)) {
        if !state.running {
            break;
        }
        seen = state.version;
    }
}

/// A SQL Server login whose default language is `language`, owner of a database
/// of its own. The database goes first, then the login.
struct LanguageLogin {
    scratch: ScratchDatabase,
    login: String,
}

impl LanguageLogin {
    fn new(database: &str, language: &str) -> Self {
        let scratch = ScratchDatabase::new(database);
        let login = LanguageLogin { login: format!("{database}_login"), scratch };
        login.drop_login();
        let name = &login.login;
        run(
            &login.scratch.master,
            "login",
            &format!("CREATE LOGIN {name} WITH PASSWORD = N'{LOGIN_PASSWORD}', DEFAULT_LANGUAGE = {language}, CHECK_POLICY = OFF"),
        );
        run(&login.scratch.connection(), "login", &format!("CREATE USER {name} FOR LOGIN {name}; ALTER ROLE db_owner ADD MEMBER {name}"));
        login
    }

    fn connection(&self) -> String {
        save(
            json!({"name": format!("ms-login-{}", self.login), "engine": "mssql", "host": "127.0.0.1", "port": 51433,
                   "user": self.login, "database": self.scratch.name, "ssl": "prefer"}),
            Some(LOGIN_PASSWORD),
        )
    }

    fn drop_login(&self) {
        let name = &self.login;
        quietly(
            &self.scratch.master,
            &format!(
                "DECLARE @k nvarchar(max) = N''; \
                 SELECT @k += N'KILL ' + CAST(session_id AS nvarchar(10)) + N'; ' FROM sys.dm_exec_sessions WHERE login_name = N'{name}'; \
                 EXEC (@k); \
                 IF SUSER_ID(N'{name}') IS NOT NULL DROP LOGIN {name}"
            ),
        );
    }
}

impl Drop for LanguageLogin {
    fn drop(&mut self) {
        self.scratch.drop_it();
        self.drop_login();
    }
}

/// What a person types into a date cell, and what the table must hold afterwards.
fn day_first_cases() -> Vec<Case> {
    let s = "dbo";
    vec![
        Case { name: "datetime and smalldatetime cells, a day up to 12", setup: "CREATE TABLE dbo.c1(id int PRIMARY KEY, a datetime, b smalldatetime); INSERT INTO dbo.c1 VALUES (1, '20240105 10:00:00', '20240105 10:00:00');", schema: s, table: "c1", edits: vec![(0, "a", Some("2024-01-06 10:00:00")), (0, "b", Some("2024-01-07 10:01:00"))], check: "SELECT CONVERT(varchar(30), a, 121), CONVERT(varchar(30), b, 121) FROM dbo.c1", expect: Expect::Written(&["2024-01-06 10:00:00.000", "2024-01-07 10:01:00.000"]) },
        Case { name: "datetime and smalldatetime cells, a day above 12", setup: "CREATE TABLE dbo.c2(id int PRIMARY KEY, a datetime, b smalldatetime); INSERT INTO dbo.c2 VALUES (1, '20240105 10:00:00', '20240105 10:00:00');", schema: s, table: "c2", edits: vec![(0, "a", Some("2024-01-13 10:00:00")), (0, "b", Some("2024-01-14 10:02:00"))], check: "SELECT CONVERT(varchar(30), a, 121), CONVERT(varchar(30), b, 121) FROM dbo.c2", expect: Expect::Written(&["2024-01-13 10:00:00.000", "2024-01-14 10:02:00.000"]) },
        Case { name: "a datetime with milliseconds and a date without a time", setup: "CREATE TABLE dbo.c3(id int PRIMARY KEY, a datetime, b smalldatetime); INSERT INTO dbo.c3 VALUES (1, '20240105 10:00:00', '20240105 10:00:00');", schema: s, table: "c3", edits: vec![(0, "a", Some("2024-02-03 04:05:06.997")), (0, "b", Some("2024-02-04"))], check: "SELECT CONVERT(varchar(30), a, 121), CONVERT(varchar(30), b, 121) FROM dbo.c3", expect: Expect::Written(&["2024-02-03 04:05:06.997", "2024-02-04 00:00:00.000"]) },
        Case { name: "a year-first date with slashes", setup: "CREATE TABLE dbo.c4(id int PRIMARY KEY, a datetime); INSERT INTO dbo.c4 VALUES (1, '20240105 10:00:00');", schema: s, table: "c4", edits: vec![(0, "a", Some("2024/01/13"))], check: "SELECT CONVERT(varchar(30), a, 121) FROM dbo.c4", expect: Expect::Written(&["2024-01-13 00:00:00.000"]) },
        Case { name: "a datetime key, days up to 12 and above", setup: "CREATE TABLE dbo.k1(k datetime PRIMARY KEY, v nvarchar(10)); INSERT INTO dbo.k1 VALUES ('20240105 10:00:00', N'a'), ('20240113 10:00:00.997', N'b'), ('20240512 08:30:00', N'c');", schema: s, table: "k1", edits: vec![(0, "v", Some("zz")), (1, "v", Some("yy")), (2, "v", Some("xx"))], check: "SELECT STRING_AGG(v, N',') WITHIN GROUP (ORDER BY k) FROM dbo.k1", expect: Expect::Written(&["zz,yy,xx"]) },
        Case { name: "a smalldatetime key, days up to 12 and above", setup: "CREATE TABLE dbo.k2(k smalldatetime PRIMARY KEY, v nvarchar(10)); INSERT INTO dbo.k2 VALUES ('20240105 10:00', N'a'), ('20240113 10:01', N'b');", schema: s, table: "k2", edits: vec![(0, "v", Some("zz")), (1, "v", Some("yy"))], check: "SELECT STRING_AGG(v, N',') WITHIN GROUP (ORDER BY k) FROM dbo.k2", expect: Expect::Written(&["zz,yy"]) },
        Case { name: "a datetime key that is itself edited", setup: "CREATE TABLE dbo.k3(k datetime PRIMARY KEY, v nvarchar(10)); INSERT INTO dbo.k3 VALUES ('20240105 10:00:00', N'a');", schema: s, table: "k3", edits: vec![(0, "k", Some("2024-02-13 04:05:06"))], check: "SELECT CONVERT(varchar(30), k, 121) FROM dbo.k3", expect: Expect::Written(&["2024-02-13 04:05:06.000"]) },
        Case { name: "a date typed the way the login's language writes it", setup: "CREATE TABLE dbo.c5(id int PRIMARY KEY, a datetime); INSERT INTO dbo.c5 VALUES (1, '20240105 10:00:00');", schema: s, table: "c5", edits: vec![(0, "a", Some("13/01/2024 10:00:00"))], check: "SELECT CONVERT(varchar(30), a, 121) FROM dbo.c5", expect: Expect::Written(&["2024-01-13 10:00:00.000"]) },
        Case { name: "the other date and time types, and numbers", setup: "CREATE TABLE dbo.o1(id int PRIMARY KEY, b datetime2(7), d date, e time(7), f datetimeoffset(7), m money, n decimal(12,2), x float, r real); INSERT INTO dbo.o1 VALUES (1, SYSDATETIME(), GETDATE(), SYSDATETIME(), SYSDATETIMEOFFSET(), 1, 1, 1, 1);", schema: s, table: "o1", edits: vec![(0, "b", Some("2024-01-13 10:00:00.1234567")), (0, "d", Some("2024-01-13")), (0, "e", Some("11:22:33.1234567")), (0, "f", Some("2024-01-13 10:00:00.1234567 +08:00")), (0, "m", Some("1234.5678")), (0, "n", Some("19.99")), (0, "x", Some("0.1")), (0, "r", Some("0.25"))], check: "SELECT CONVERT(varchar(30), b, 121), CONVERT(varchar(30), d, 121), CONVERT(varchar(30), e, 121), CONVERT(varchar(40), f, 121), CONVERT(varchar(30), m, 2), CAST(n AS varchar(20)), CAST(x AS varchar(20)), CAST(r AS varchar(20)) FROM dbo.o1", expect: Expect::Written(&["2024-01-13 10:00:00.1234567", "2024-01-13", "11:22:33.1234567", "2024-01-13 10:00:00.1234567 +08:00", "1234.5678", "19.99", "0.1", "0.25"]) },
        Case { name: "text that looks like a date stays text, in a cell and in a key", setup: "CREATE TABLE dbo.t1(k nvarchar(30) PRIMARY KEY, v nvarchar(30), w varchar(30)); INSERT INTO dbo.t1 VALUES (N'2024-01-06 10:00:00', N'a', 'a'), (N'2024-01-13 11:00:00', N'b', 'b');", schema: s, table: "t1", edits: vec![(0, "v", Some("2024-01-06 10:00:00")), (0, "w", Some("2024/01/13")), (1, "v", Some("zz"))], check: "SELECT STRING_AGG(v + N'|' + w, N',') WITHIN GROUP (ORDER BY k) FROM dbo.t1", expect: Expect::Written(&["2024-01-06 10:00:00|2024/01/13,zz|b"]) },
    ]
}

#[test]
fn mssql_a_login_that_reads_dates_day_first_writes_the_dates_it_is_given() {
    only_with!("mssql");
    for (n, language) in ["British", "German"].into_iter().enumerate() {
        let login = LanguageLogin::new(&format!("yz_dt_lang{n}"), language);
        let id = login.connection();
        assert_eq!(
            first_row(&id, "SELECT CONVERT(varchar(10), CAST(N'2024-01-06' AS datetime), 120)"),
            ["2024-06-01"],
            "{language} reads 2024-01-06 as the 1st of June, or this test would prove nothing"
        );
        go(&id, day_first_cases());
    }
}

fn first_row_as(connection: &str, owner_name: &str, sql: &str) -> Vec<String> {
    let state = run(connection, owner_name, sql);
    let set = results::get(state.results.last().expect("a result").id).expect("result");
    let r = results::lock(&set);
    r.rows.first().map(|row| row.iter().map(|c| c.display().into_owned()).collect()).unwrap_or_default()
}

#[test]
fn mssql_a_sql_tab_that_reads_dates_day_first_does_not_change_the_dates_the_table_writes() {
    only_with!("mssql");
    let scratch = ScratchDatabase::new("yz_dt_pane");
    let id = scratch.connection();
    run(&id, "setup", "CREATE TABLE dbo.p(k datetime PRIMARY KEY, a datetime, b smalldatetime, v nvarchar(10)); INSERT INTO dbo.p VALUES ('20240105 10:00:00', '20240105 10:00:00', '20240105 10:00', N'a'), ('20240113 10:00:00', '20240105 10:00:00', '20240105 10:00', N'b');");
    for (n, format) in ["dmy", "ydm", "dym"].into_iter().enumerate() {
        let pane_name = format!("df-{format}");
        run(&id, &pane_name, &format!("SET DATEFORMAT {format}"));
        let pane = open(&id, &pane_name, &table("dbo", "p"));
        let day = 10 + n;
        for row in 0..2 {
            pane.edit(row, "a", Some(&format!("2024-02-{day} 11:00:00")));
            pane.edit(row, "b", Some(&format!("2024-03-{day} 12:01:00")));
            pane.edit(row, "v", Some(format));
        }
        runtime::block_on(session::write_back_set(&pane.set)).unwrap_or_else(|e| panic!("{format}: {e}"));
        assert_eq!(
            rows(&id, "SELECT CONVERT(varchar(30), a, 121), CONVERT(varchar(30), b, 121), v FROM dbo.p ORDER BY k"),
            vec![vec![format!("2024-02-{day} 11:00:00.000"), format!("2024-03-{day} 12:01:00.000"), format.to_owned()]; 2],
            "{format}"
        );
        assert_eq!(
            first_row_as(&id, &pane_name, "SELECT CONVERT(varchar(10), CAST(N'2024-01-06' AS datetime), 120)"),
            ["2024-06-01"],
            "{format}: the write-back left the pane's DATEFORMAT as it found it"
        );
        session::release_owner(&pane.owner);
    }
}

/// A table per key type, each holding two rows: one with a day up to 12, one above.
fn key_cases(types: &[(&'static str, &'static str, &'static str)]) -> Vec<Case> {
    types
        .iter()
        .map(|(ty, one, two)| {
            let name: &'static str = Box::leak(format!("{ty} key").into_boxed_str());
            let table: &'static str = Box::leak(format!("pk_{}", ty.replace(' ', "_")).into_boxed_str());
            let setup: &'static str = Box::leak(
                format!("DROP TABLE IF EXISTS {table}; CREATE TABLE {table}(k {ty} PRIMARY KEY, v text); INSERT INTO {table} VALUES ({one}, 'a'), ({two}, 'b');")
                    .into_boxed_str(),
            );
            let check: &'static str = Box::leak(format!("SELECT count(*)::text FROM {table} WHERE v IN ('zz', 'yy')").into_boxed_str());
            Case { name, setup, schema: "public", table, edits: vec![(0, "v", Some("zz")), (1, "v", Some("yy"))], check, expect: Expect::Written(&["2"]) }
        })
        .collect()
}

#[test]
fn postgres_dates_are_written_back_in_whatever_style_the_database_prints_them() {
    only_with!("postgres");
    let scratch = PgScratchDatabase::new("yz_dt_pg");
    let db = scratch.name.clone();
    for (n, (datestyle, intervalstyle)) in
        [("ISO, MDY", "postgres"), ("SQL, DMY", "sql_standard"), ("German, DMY", "iso_8601"), ("Postgres, DMY", "postgres_verbose")]
            .into_iter()
            .enumerate()
    {
        run(
            &scratch.maintenance,
            "style",
            &format!(
                "ALTER DATABASE {db} SET datestyle = '{datestyle}'; ALTER DATABASE {db} SET intervalstyle = '{intervalstyle}'; \
                 ALTER DATABASE {db} SET timezone = 'Asia/Taipei'"
            ),
        );
        let id = postgres_in(&format!("pg-dt-{n}"), &db);
        run(&id, "setup", "DROP TABLE IF EXISTS pc; CREATE TABLE pc(id int PRIMARY KEY, tz timestamptz, ts timestamp, d date, tm time, tmz timetz, iv interval); INSERT INTO pc VALUES (1, '2024-01-06 10:00:00+08', '2024-01-06 10:00:00', '2024-01-06', '10:00:00', '10:00:00+08', '1 day');");
        let pane = open(&id, &format!("dt-pane-{n}"), &table("public", "pc"));
        let shown = |column: &str| {
            let at = pane.column(column);
            results::lock(&pane.set).rows[0][at].display().into_owned()
        };
        // The person changes what the grid printed, the way it printed it.
        let printed = (shown("tz"), shown("ts"), shown("d"));
        pane.edit(0, "tz", Some(&shown("tz").replace("10:00:00", "11:00:00")));
        pane.edit(0, "ts", Some(&shown("ts").replace("10:00:00", "11:00:00")));
        pane.edit(0, "d", Some(&shown("d").replace("06", "07")));
        pane.edit(0, "tm", Some(&shown("tm").replace("10:00:00", "11:00:00")));
        pane.edit(0, "tmz", Some(&shown("tmz").replace("10:00:00", "11:00:00")));
        pane.edit(0, "iv", Some("5 days 06:07:08"));
        runtime::block_on(session::write_back_set(&pane.set)).unwrap_or_else(|e| panic!("{datestyle}: {e}"));
        assert_eq!(
            first_row(
                &id,
                "SELECT to_char(tz AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS'), to_char(ts, 'YYYY-MM-DD HH24:MI:SS'), to_char(d, 'YYYY-MM-DD'), \
                        to_char(tm, 'HH24:MI:SS'), (tmz AT TIME ZONE 'UTC')::time::text, floor(extract(epoch FROM iv))::bigint::text FROM pc"
            ),
            ["2024-01-06 03:00:00", "2024-01-06 11:00:00", "2024-01-07", "11:00:00", "03:00:00", "454028"],
            "{datestyle} / {intervalstyle}: the grid printed {printed:?}"
        );
        session::release_owner(&pane.owner);
        go(
            &id,
            key_cases(&[
                ("timestamptz", "'2024-01-06 10:00:00+08'", "'2024-01-13 10:00:00.5+08'"),
                ("timestamp", "'2024-01-06 10:00:00'", "'2024-01-13 10:00:00.123456'"),
                ("date", "'2024-01-06'", "'2024-01-13'"),
                ("time", "'10:00:00'", "'13:14:15.5'"),
                ("timetz", "'10:00:00+08'", "'13:14:15.5-05'"),
                ("interval", "'1 day 02:03:04'", "'3 mons 13 days 04:05:06.5'"),
            ]),
        );
    }
}

#[test]
fn postgres_float_keys_are_found_whatever_extra_float_digits_the_database_sets() {
    only_with!("postgres");
    let scratch = PgScratchDatabase::new("yz_dt_float");
    run(&scratch.maintenance, "style", &format!("ALTER DATABASE {} SET extra_float_digits = 0", scratch.name));
    let id = scratch.connection();
    go(
        &id,
        key_cases(&[
            ("float8", "0.1::float8 + 0.2::float8", "1.0::float8 / 3"),
            ("real", "0.1::real", "(1.0 / 3)::real"),
        ]),
    );
}

// ---------------------------------------------------------------- a date typed year first, whatever the login reads

/// Dates and times as a person might write them with the year first.
fn year_first_inputs() -> Vec<String> {
    let dates = [
        "2024-01-06",
        "2024/1/6",
        "2024.01.06",
        "20240106",
        "2024-1-6",
        "2024-06-01",
        "2024-12-11",
        "2024-01-13",
        "2024-02-29",
        "2023-02-29",
        "2024-13-06",
        "2024-01-32",
        "0001-01-01",
        "1752-12-31",
        "1753-01-01",
        "9999-12-31",
        "2079-06-06",
        "2079-06-07",
        "2024 Jan 6",
        "2024-Jan-06",
        "2024-01",
        "2024",
    ];
    let times = [
        "",
        " 10:00",
        " 10:00:00",
        " 10:00:00.5",
        " 10:00:00.997",
        " 10:00:00.9999",
        " 2:30 PM",
        " 2:30PM",
        " 02:30 pm",
        " 12:00 AM",
        " 12:30 AM",
        " 12:00 PM",
        " 12:30 PM",
        " 0:30 AM",
        " 13:00 PM",
        " 10AM",
        " 10 PM",
        " 10",
        " 9:5",
        " 10:00:5",
        " 1:2:3",
        " 10:00:00:000",
        " 10:00:00:5",
        " 24:00",
        " 23:59:59",
        " 23:59:59.999",
        " 23:59:30",
        " 10:00:29.999",
        " 10:00:00+08:00",
        " 10:00:00 +08:00",
        "T10:00",
        "T10:00:00",
        " T10:00",
        "T 10:00",
        " 10:00 :00",
        " 10 : 00",
        "  10:00",
        " 09:05",
        " 9:05",
        " 12:00:00.5 PM",
        " 10:60",
        " 10:00:60",
    ];
    let mut inputs = Vec::new();
    for date in dates {
        for time in ["", " 10:00", " 2:30 PM", " 10:00:00.5"] {
            inputs.push(format!("{date}{time}"));
        }
    }
    for date in ["2024-01-06", "2024/1/6", "20240106"] {
        for time in times {
            inputs.push(format!("{date}{time}"));
        }
    }
    for date in ["2024-01-06", "2024/1/6", "2024.01.06", "20240106", "2024-02-03"] {
        for time in ["10:00", "10:00:00", "10:00:00.5", "2:30 PM", "12:00 AM", "9:5", "10:00:00+08:00", "13:00 PM"] {
            inputs.push(format!("{time} {date}"));
        }
        // ISO 8601 as the server itself reads it; the other writings with a Z are in the engine tests.
        if date.starts_with("2024-") {
            for time in ["10:00:00Z", "10:00:00.000Z", "10:00:00.250Z"] {
                inputs.push(format!("{date}T{time}"));
            }
        }
    }
    inputs.sort();
    inputs.dedup();
    inputs
}

/// Typed the way the grid prints them, or a plain 12-hour clock: the write-back must take these.
const YEAR_FIRST_STORED: &[&str] = &[
    "2024-01-06",
    "2024-01-06 10:00",
    "2024-01-06 10:00:00",
    "2024-01-06 10:00:00.5",
    "2024-01-06 10:00:00.997",
    "2024-01-06T10:00:00",
    "2024/1/6 9:05",
    "2024.01.06 10:00:00",
    "2024-01-06 2:30 PM",
    "2024-01-06 2:30PM",
    "2024-01-06 12:00 AM",
    "2024-01-06 12:30 PM",
    "2024-01-06 10AM",
    "20240106",
    "20240106 10:00",
    "2024-02-29 23:59:30",
    "10:00 2024-01-06",
    "2:30 PM 2024/1/6",
    "10:00:00 2024.01.06",
    "2024-01-06T10:00:00Z",
    "2024-01-06T10:00:00.000Z",
];

/// The shapes a day-first login used to read as another day, or another time of day.
const YEAR_FIRST_REFUSED: &[&str] = &[
    "2024-01-06 9:5",
    "2024-01-06 10:00:5",
    "2024-01-06 1:2:3",
    "2024-01-06 10:00:00:000",
    "2024-01-06 10:00:00:5",
    "2024-01-06 10:00:00+08:00",
    "2024-13-06",
    "2024-01-32",
    "2023-02-29",
];

/// What a login that reads dates `language`-first stores for each text typed
/// into a `datetime` and a `smalldatetime` cell: what a session that reads
/// the same text year first (`sa`) reads from it, or nothing at all.
fn year_first_sweep(language: &str, database: &str, inputs: &[String]) {
    let login = LanguageLogin::new(database, language);
    let id = login.connection();
    let reference = login.scratch.connection();
    assert_eq!(
        first_row(&reference, "SELECT CONVERT(varchar(10), CAST(N'2024-01-06' AS datetime), 120)"),
        ["2024-01-06"],
        "the reference session reads year first"
    );
    assert_eq!(
        first_row(&id, "SELECT CONVERT(varchar(10), CAST(N'2024-01-06' AS datetime), 120)"),
        ["2024-06-01"],
        "{language} reads 2024-01-06 as the 1st of June, or this test would prove nothing"
    );
    run(&reference, "setup", "CREATE TABLE dbo.s(id int PRIMARY KEY, a datetime, b smalldatetime); INSERT INTO dbo.s VALUES (1, '19990101', '19990101');");
    let sentinel = ["1999-01-01 00:00:00.000".to_owned(), "1999-01-01 00:00:00.000".to_owned()];
    let pane = open(&id, "sweep", &table("dbo", "s"));
    let mut wrong = Vec::new();
    let mut written = std::collections::BTreeSet::new();
    for typed in inputs {
        run(&reference, "reset", "UPDATE dbo.s SET a = '19990101', b = '19990101' WHERE id = 1");
        pane.edit(0, "a", Some(typed));
        pane.edit(0, "b", Some(typed));
        let outcome = runtime::block_on(session::write_back_set(&pane.set));
        let stored = first_row(&reference, "SELECT CONVERT(varchar(30), a, 121), CONVERT(varchar(30), b, 121) FROM dbo.s WHERE id = 1");
        match outcome {
            Ok(_) => {
                // A `T` between the date and the time is only a separator; the server wants seconds after it, which a person need not type.
                let spaced = typed.replacen('T', " ", 1);
                let read_year_first = first_row(
                    &reference,
                    &format!(
                        "SELECT CONVERT(varchar(30), COALESCE(TRY_CAST(N'{typed}' AS datetime), TRY_CAST(N'{spaced}' AS datetime)), 121), \
                                CONVERT(varchar(30), COALESCE(TRY_CAST(N'{typed}' AS smalldatetime), TRY_CAST(N'{spaced}' AS smalldatetime)), 121)"
                    ),
                );
                if stored == read_year_first {
                    written.insert(typed.clone());
                } else {
                    wrong.push(format!("{typed:?}: stored {stored:?}, read year first it is {read_year_first:?}"));
                }
            }
            Err(error) => {
                if stored != sentinel {
                    wrong.push(format!("{typed:?}: refused ({error}) but stored {stored:?}"));
                }
                results::lock(&pane.set).discard_edits();
            }
        }
    }
    for typed in YEAR_FIRST_STORED {
        if inputs.iter().any(|i| i == typed) && !written.contains(*typed) {
            wrong.push(format!("{typed:?}: the grid's own way of writing it was not saved"));
        }
    }
    for typed in YEAR_FIRST_REFUSED {
        if written.contains(*typed) {
            wrong.push(format!("{typed:?}: saved, but no reading of it is certain"));
        }
    }
    assert!(wrong.is_empty(), "{language}: {} of {} inputs went wrong:\n{}", wrong.len(), inputs.len(), wrong.join("\n"));
}

#[test]
fn mssql_a_day_first_login_never_stores_a_year_first_date_as_another_day() {
    only_with!("mssql");
    year_first_sweep("British", "yz_r1_brit", &year_first_inputs());
}

#[test]
fn mssql_a_german_login_never_stores_a_year_first_date_as_another_day() {
    only_with!("mssql");
    let mut inputs: Vec<String> = YEAR_FIRST_STORED.iter().chain(YEAR_FIRST_REFUSED).map(|s| (*s).to_owned()).collect();
    inputs.extend(year_first_inputs().into_iter().filter(|i| i.starts_with("2024-01-06")));
    inputs.sort();
    inputs.dedup();
    year_first_sweep("German", "yz_r1_ger", &inputs);
}

#[test]
fn mssql_a_day_first_login_saves_dates_of_an_aliased_type_and_under_a_computed_key() {
    only_with!("mssql");
    let login = LanguageLogin::new("yz_r1_alias", "British");
    let id = login.connection();
    run(&login.scratch.connection(), "setup", "CREATE TYPE dbo.yz_when FROM datetime;");
    let s = "dbo";
    go(
        &id,
        vec![
            Case { name: "a column of a type made from datetime", setup: "CREATE TABLE dbo.al(id int PRIMARY KEY, w dbo.yz_when NULL); INSERT INTO dbo.al VALUES (1, '20240105');", schema: s, table: "al", edits: vec![(0, "w", Some("2024-01-06 2:30 PM"))], check: "SELECT CONVERT(varchar(30), w, 121) FROM dbo.al", expect: Expect::Written(&["2024-01-06 14:30:00.000"]) },
            Case { name: "the same, typed in a way nobody can read one way", setup: "CREATE TABLE dbo.al2(id int PRIMARY KEY, w dbo.yz_when NULL); INSERT INTO dbo.al2 VALUES (1, '20240105');", schema: s, table: "al2", edits: vec![(0, "w", Some("2024-01-06 9:5"))], check: "SELECT CONVERT(varchar(30), w, 121) FROM dbo.al2", expect: Expect::Refused("can't be saved as a date and time") },
            Case { name: "a persisted computed column as the key", setup: "CREATE TABLE dbo.cp(a datetime NOT NULL, k AS (a) PERSISTED NOT NULL PRIMARY KEY, v nvarchar(10)); INSERT INTO dbo.cp(a, v) VALUES ('20240105 10:00:00', N'x'), ('20240113 11:00:00.997', N'y');", schema: s, table: "cp", edits: vec![(0, "v", Some("p")), (1, "v", Some("q"))], check: "SELECT STRING_AGG(v, N',') WITHIN GROUP (ORDER BY a) FROM dbo.cp", expect: Expect::Written(&["p,q"]) },
            Case { name: "a date in a text column that the same table holds a datetime beside", setup: "CREATE TABLE dbo.mx(id int PRIMARY KEY, w datetime NULL, t nvarchar(30) NULL); INSERT INTO dbo.mx VALUES (1, '20240105', N'a');", schema: s, table: "mx", edits: vec![(0, "w", Some("2024-01-06 2:30 PM")), (0, "t", Some("2024-01-06 2:30 PM"))], check: "SELECT CONVERT(varchar(30), w, 121), t FROM dbo.mx", expect: Expect::Written(&["2024-01-06 14:30:00.000", "2024-01-06 2:30 PM"]) },
        ],
    );
}

#[test]
fn mssql_a_date_the_column_rounds_is_found_again_by_the_next_write_back() {
    only_with!("mssql");
    let login = LanguageLogin::new("yz_r1_round", "British");
    let id = login.connection();
    let reference = login.scratch.connection();
    run(
        &reference,
        "setup",
        "CREATE TABLE dbo.r1(k datetime PRIMARY KEY, v nvarchar(10)); \
         CREATE TABLE dbo.r2(k smalldatetime PRIMARY KEY, v nvarchar(10));",
    );
    for (table_name, typed, stored) in [
        ("r1", "2024-02-13 11:00:00.001", "2024-02-13 11:00:00.000"),
        ("r1", "2024-02-13 11:00:00.002", "2024-02-13 11:00:00.003"),
        ("r1", "2024-02-13 11:00:59.999", "2024-02-13 11:01:00.000"),
        ("r2", "2024-02-13 11:00:31", "2024-02-13 11:01:00.000"),
        ("r2", "2024-02-13 11:00:29", "2024-02-13 11:00:00.000"),
    ] {
        run(&reference, "reset", &format!("DELETE dbo.{table_name}; INSERT INTO dbo.{table_name} VALUES ('20240106 10:00:00', N'a')"));
        let pane = open(&id, &format!("round-{table_name}"), &table("dbo", table_name));
        pane.edit(0, "k", Some(typed));
        runtime::block_on(session::write_back_set(&pane.set)).unwrap_or_else(|e| panic!("{typed}: {e}"));
        assert_eq!(first_row(&reference, &format!("SELECT CONVERT(varchar(30), k, 121) FROM dbo.{table_name}")), [stored], "{typed}");
        // The grid now holds what was typed; the database holds what the column made of it.
        pane.edit(0, "v", Some("next"));
        runtime::block_on(session::write_back_set(&pane.set)).unwrap_or_else(|e| panic!("{typed}: the second write-back: {e}"));
        assert_eq!(first_row(&reference, &format!("SELECT v FROM dbo.{table_name}")), ["next"], "{typed}");
        session::release_owner(&pane.owner);
    }
}

// ---------------------------------------------------------------- a connection that opens a transaction by itself

/// The first cell of what `sql` reads on the connection the table pane `pane` reads and writes by.
fn pane_value(connection: &str, pane: &str, sql: &str) -> String {
    let state = wait(session::start_query(connection, None, &session::table_owner(&owner(connection, pane)), sql));
    assert_eq!(state.error, None, "{sql}");
    let set = results::get(state.results.last().expect("a result").id).expect("result");
    let r = results::lock(&set);
    r.rows.first().map(|row| row[0].display().into_owned()).unwrap_or_default()
}

// ---------------------------------------------------------------- PostgreSQL: what the SQL tab does to the session

/// What a person's SQL tab can do to the settings a value is printed and read by.
const SESSION_RESETS: &[&str] = &[
    "RESET ALL",
    "DISCARD ALL",
    "RESET datestyle",
    "SET datestyle = 'SQL, DMY'",
    "SET extra_float_digits = 0",
    "SELECT set_config('datestyle', 'SQL, DMY', false), set_config('extra_float_digits', '0', false)",
];

#[test]
fn postgres_what_the_sql_tab_does_to_the_session_does_not_change_what_a_write_back_saves() {
    only_with!("postgres");
    let scratch = PgScratchDatabase::new("yz_r1_pgset");
    let db = scratch.name.clone();
    run(
        &scratch.maintenance,
        "style",
        &format!(
            "ALTER DATABASE {db} SET datestyle = 'SQL, DMY'; ALTER DATABASE {db} SET timezone = 'Asia/Taipei'; \
             ALTER DATABASE {db} SET extra_float_digits = 0"
        ),
    );
    let id = scratch.connection();
    let mut wrong = Vec::new();
    for (n, statement) in SESSION_RESETS.iter().enumerate() {
        run(
            &id,
            "setup",
            "DROP TABLE IF EXISTS pz; CREATE TABLE pz(k timestamptz PRIMARY KEY, t timestamptz, v text); \
             INSERT INTO pz VALUES ('2024-01-06 10:00:00+08', '2024-01-06 10:00:00+08', 'a'); \
             DROP TABLE IF EXISTS pf; CREATE TABLE pf(k float8 PRIMARY KEY, v text); \
             INSERT INTO pf VALUES (0.1::float8 + 0.2::float8, 'a');",
        );

        // The SQL tab runs it; the table is read again afterwards, and edited as the grid prints it.
        let name = format!("settings-pz-{n}");
        run(&id, &name, statement);
        let pane = open(&id, &name, &table("public", "pz"));
        let at = pane.column("t");
        let printed = results::lock(&pane.set).rows[0][at].display().into_owned();
        pane.edit(0, "t", Some(&printed.replace("10:00:00", "11:00:00")));
        pane.edit(0, "v", Some("b"));
        match runtime::block_on(session::write_back_set(&pane.set)) {
            Err(e) => wrong.push(format!("{statement}: a timestamptz key printed as {printed:?}: {e}")),
            Ok(_) => {
                let stored = first_row(&id, "SELECT v, to_char(t AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS') FROM pz");
                if stored != ["b", "2024-01-06 03:00:00"] {
                    wrong.push(format!("{statement}: the grid printed {printed:?}, the table holds {stored:?}"));
                }
            }
        }
        session::release_owner(&pane.owner);

        let name = format!("settings-pf-{n}");
        run(&id, &name, statement);
        let pane = open(&id, &name, &table("public", "pf"));
        let at = pane.column("k");
        let printed = results::lock(&pane.set).rows[0][at].display().into_owned();
        pane.edit(0, "v", Some("b"));
        match runtime::block_on(session::write_back_set(&pane.set)) {
            Err(e) => wrong.push(format!("{statement}: a float key printed as {printed:?}: {e}")),
            Ok(_) => assert_eq!(first_row(&id, "SELECT v FROM pf"), ["b"], "{statement}"),
        }
        session::release_owner(&pane.owner);
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

// ---------------------------------------------------------------- written means committed, on every engine

#[test]
fn sqlite_a_write_back_leaves_no_transaction_open_and_another_connection_reads_it() {
    let (id, _file) = sqlite_connection("r1-committed");
    run(&id, "setup", "CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t VALUES (1, 'a');");
    let pane = open(&id, "committed-pane", &table("", "t"));
    pane.edit(0, "v", Some("z"));
    runtime::block_on(session::write_back_set(&pane.set)).unwrap();
    // A second BEGIN is an error while one is open.
    pane_value(&id, "committed-pane", "BEGIN; ROLLBACK");
    assert_eq!(first_row(&id, "SELECT v FROM t"), ["z"]);
}

#[test]
fn postgres_a_write_back_leaves_no_transaction_open_and_another_connection_reads_it() {
    only_with!("postgres");
    let scratch = PgScratchDatabase::new("yz_r1_pgtx");
    let id = scratch.connection();
    run(&id, "setup", "CREATE TABLE t(id int PRIMARY KEY, v text); INSERT INTO t VALUES (1, 'a');");
    let pane = open(&id, "committed-pane", &table("public", "t"));
    pane.edit(0, "v", Some("z"));
    runtime::block_on(session::write_back_set(&pane.set)).unwrap();
    assert_eq!(first_row(&id, "SELECT v FROM t"), ["z"], "another connection reads it");
    assert_eq!(pane_value(&id, "committed-pane", "SELECT (now() = statement_timestamp())::text"), "true", "the pane's connection is not inside a transaction");
}

// ---------------------------------------------------------------- a cleared cell

impl Opened {
    /// What the grid keeps of text typed into a cell and ended with Enter.
    fn type_into(&self, row: usize, column: &str, text: &str) {
        let at = self.column(column);
        let mut r = results::lock(&self.set);
        let original = r.cell(row, at).cloned().unwrap_or(Cell::Null);
        let initial = if original.is_null() { String::new() } else { original.display().into_owned() };
        r.type_into(row, at, &original, &initial, text.to_owned(), true);
    }

    /// A cell as the grid shows it now, a pending edit included.
    fn shown(&self, row: usize, column: &str) -> String {
        let at = self.column(column);
        results::lock(&self.set).cell(row, at).map_or_else(|| "NULL".to_owned(), |c| c.display().into_owned())
    }
}

/// A column type, a value of it, and whether an empty string is a value of the type.
type Kind = (&'static str, &'static str, bool);

const MSSQL_KINDS: &[Kind] = &[
    ("datetime", "'20240105 10:00:00'", false),
    ("smalldatetime", "'20240105 10:00'", false),
    ("date", "'20240105'", false),
    ("time", "'10:00:00'", false),
    ("datetime2", "'20240105 10:00:00'", false),
    ("datetimeoffset", "'2024-01-05 10:00:00 +08:00'", false),
    ("int", "5", false),
    ("bigint", "5", false),
    ("smallint", "5", false),
    ("tinyint", "5", false),
    ("bit", "1", false),
    ("float", "1.5", false),
    ("real", "1.5", false),
    ("decimal(10,2)", "1.5", false),
    ("money", "1.5", false),
    ("smallmoney", "1.5", false),
    ("uniqueidentifier", "'6F9619FF-8B86-D011-B42D-00C04FC964FF'", false),
    ("varchar(10)", "'a'", true),
    ("nvarchar(10)", "N'a'", true),
    ("char(5)", "'a'", true),
    ("nchar(5)", "N'a'", true),
    ("varchar(max)", "'a'", true),
    ("nvarchar(max)", "N'a'", true),
    ("text", "'a'", true),
];

const POSTGRES_KINDS: &[Kind] = &[
    ("timestamp", "'2024-01-05 10:00:00'", false),
    ("timestamptz", "'2024-01-05 10:00:00+08'", false),
    ("date", "'2024-01-05'", false),
    ("time", "'10:00:00'", false),
    ("timetz", "'10:00:00+08'", false),
    ("interval", "'1 day'", false),
    ("int2", "5", false),
    ("int4", "5", false),
    ("int8", "5", false),
    ("float4", "1.5", false),
    ("float8", "1.5", false),
    ("numeric(10,2)", "1.5", false),
    ("money", "1.5", false),
    ("bool", "true", false),
    ("uuid", "'6f9619ff-8b86-d011-b42d-00c04fc964ff'", false),
    ("inet", "'10.0.0.1'", false),
    ("cidr", "'10.0.0.0/8'", false),
    ("macaddr", "'08:00:2b:01:02:03'", false),
    ("json", "'{}'", false),
    ("jsonb", "'{}'", false),
    ("text", "'a'", true),
    ("varchar(10)", "'a'", true),
    ("char(5)", "'a'", true),
];

/// Clears a cell of each type the way a person does (Backspace, Enter) and writes back, one type
/// at a time; says which types do not show NULL before the write, or do not hold what a cleared
/// cell means after it.
fn clearing_every_kind(id: &str, kinds: &[Kind], schema: &str, as_text: impl Fn(&str) -> String) -> Vec<String> {
    let columns: Vec<String> = kinds.iter().enumerate().map(|(i, (ty, _, _))| format!("c{i} {ty} NULL")).collect();
    run(id, "setup", &format!("CREATE TABLE {schema}.e1(id int PRIMARY KEY, {})", columns.join(", ")));
    for (i, (_, value, _)) in kinds.iter().enumerate() {
        run(id, "setup", &format!("INSERT INTO {schema}.e1(id, c{i}) VALUES ({i}, {value})"));
    }
    let pane = open(id, "clear-pane", &table(schema, "e1"));
    let mut wrong = Vec::new();
    for (i, (ty, _, empty_is_a_value)) in kinds.iter().enumerate() {
        let (row, column) = (pane.row_where("id", &i.to_string()), format!("c{i}"));
        pane.type_into(row, &column, "");
        let shown = pane.shown(row, &column);
        let stored = match runtime::block_on(session::write_back_set(&pane.set)) {
            Ok(_) => first_row(id, &format!("SELECT {} FROM {schema}.e1 WHERE id = {i}", as_text(&column))).remove(0),
            Err(e) => {
                results::lock(&pane.set).discard_edits();
                format!("refused: {e}")
            }
        };
        let want = if *empty_is_a_value { "" } else { "NULL" };
        eprintln!("CLEARED {ty:<16} shown={shown:<6} stored={stored}");
        if shown != want || stored.trim_end() != want {
            wrong.push(format!("{ty}: the cleared cell shows {shown:?} and the table holds {stored:?}, wanted {want:?} in both"));
        }
    }
    session::release_owner(&pane.owner);
    wrong
}

#[test]
fn mssql_a_cleared_cell_is_null_unless_the_type_holds_an_empty_string() {
    only_with!("mssql");
    let scratch = ScratchDatabase::new("yz_e1_ms");
    let wrong = clearing_every_kind(&scratch.connection(), MSSQL_KINDS, "dbo", |c| format!("ISNULL(CAST({c} AS nvarchar(40)), N'NULL')"));
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn postgres_a_cleared_cell_is_null_unless_the_type_holds_an_empty_string() {
    only_with!("postgres");
    let scratch = PgScratchDatabase::new("yz_e1_pg");
    let wrong = clearing_every_kind(&scratch.connection(), POSTGRES_KINDS, "public", |c| format!("COALESCE({c}::text, 'NULL')"));
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

fn a_cleared_cell_of_a_column_that_cannot_be_null_is_refused(id: &str, schema: &str, number: &str, instant: &str) {
    run(id, "setup", &format!("CREATE TABLE {schema}.nn(id int PRIMARY KEY, n {number} NOT NULL, d {instant} NOT NULL); INSERT INTO {schema}.nn VALUES (1, 5, '2024-01-05 10:00:00')"));
    for column in ["n", "d"] {
        let pane = open(id, &format!("nn-{column}"), &table(schema, "nn"));
        pane.type_into(0, column, "");
        assert_eq!(pane.shown(0, column), "NULL", "{schema}.{column}: the cell shows what it will write");
        let refused = runtime::block_on(session::write_back_set(&pane.set)).expect_err("the server refuses a NULL it cannot store").to_string();
        assert!(refused.to_lowercase().contains("null"), "{schema}.{column}: the server says why: {refused}");
        let held = first_row(id, &format!("SELECT CAST(n AS varchar(10)), CAST(d AS varchar(30)) FROM {schema}.nn WHERE id = 1"));
        assert_eq!(held[0], "5", "{schema}.{column}: nothing was written");
        assert!(held[1].contains("10:00"), "{schema}.{column}: nothing was written ({held:?})");
        session::release_owner(&pane.owner);
    }
}

#[test]
fn mssql_a_cleared_cell_of_a_column_that_cannot_be_null_is_refused_by_the_server() {
    only_with!("mssql");
    let scratch = ScratchDatabase::new("yz_e1_nn");
    a_cleared_cell_of_a_column_that_cannot_be_null_is_refused(&scratch.connection(), "dbo", "int", "datetime");
}

#[test]
fn postgres_a_cleared_cell_of_a_column_that_cannot_be_null_is_refused_by_the_server() {
    only_with!("postgres");
    let scratch = PgScratchDatabase::new("yz_e1_pgnn");
    a_cleared_cell_of_a_column_that_cannot_be_null_is_refused(&scratch.connection(), "public", "numeric", "timestamp");
}

// ---------------------------------------------------------------- what the SQL tab leaves on its connection

/// A statement the SQL tab runs, how to ask the tab afterwards whether it still holds, and what it answers.
struct TabState {
    run: &'static str,
    /// Fails: the state it leaves is what it did before it failed.
    fails: bool,
    ask: &'static str,
    answer: &'static str,
}

fn tab_state(run: &'static str, ask: &'static str, answer: &'static str) -> TabState {
    TabState { run, fails: run.ends_with("1/0"), ask, answer }
}

/// Runs `sql` as the SQL tab of `pane` would, and waits; a script that fails is no surprise to the caller.
fn run_in_the_tab(connection: &str, pane: &str, sql: &str) -> JobState {
    wait(session::start_query(connection, None, &owner(connection, pane), sql))
}

/// The first column of what the SQL tab of `pane` reads.
fn tab_says(connection: &str, pane: &str, sql: &str) -> Vec<String> {
    let state = run(connection, pane, sql);
    let set = results::get(state.results.last().expect("a result").id).expect("result");
    let r = results::lock(&set);
    r.rows.iter().map(|row| row[0].display().into_owned()).collect()
}

/// What the server called the failure: `Msg 2628`, or the first words.
fn refusal(error: &DbError) -> String {
    let text = error.to_string();
    text.split("(Msg ").nth(1).map_or_else(|| text.chars().take(40).collect(), |rest| format!("Msg {}", rest.split(',').next().unwrap_or("")))
}

/// What the table pane read and saved, to be the same whatever the SQL tab had done on its connection.
#[derive(Debug, PartialEq)]
struct Pane {
    rows_read: usize,
    long_cell: usize,
    too_long: String,
    too_big: String,
    saved: String,
    held: Vec<String>,
}

fn mssql_pane(id: &str, name: &str, tab: Option<&TabState>) -> Pane {
    run(
        id,
        "reset",
        "DELETE FROM dbo.s; INSERT INTO dbo.s VALUES (1, N'ab', 1, REPLICATE(CAST('x' AS varchar(max)), 5000), N'a'), (2, N'cd', 2, 'y', N'b'), (3, N'ef', 3, 'z', N'c')",
    );
    if let Some(tab) = tab {
        let state = run_in_the_tab(id, name, tab.run);
        assert_eq!(state.error.is_some(), tab.fails, "{}: {:?}", tab.run, state.error);
    }
    let pane = open(id, name, &table("dbo", "s"));
    let rows_read = results::lock(&pane.set).rows.len();
    let first = pane.row_where("id", "1");
    let at = pane.column("big");
    let long_cell = results::lock(&pane.set).rows[first][at].display().len();
    let attempt = || match runtime::block_on(session::write_back_set(&pane.set)) {
        Ok(_) => "saved".to_owned(),
        Err(e) => {
            results::lock(&pane.set).discard_edits();
            format!("refused: {}", refusal(&e))
        }
    };
    pane.edit(first, "short", Some("abcdefgh"));
    let too_long = attempt();
    pane.edit(first, "tiny", Some("300"));
    let too_big = attempt();
    pane.edit(first, "note", Some("ok"));
    let saved = attempt();
    let held = first_row(id, "SELECT short, CAST(tiny AS varchar(5)), CAST(LEN(big) AS varchar(10)), note FROM dbo.s WHERE id = 1");
    Pane { rows_read, long_cell, too_long, too_big, saved, held }
}

/// A server whose own default opens a transaction at the first statement that touches a table
/// (`sp_configure 'user options'` with 2): the connection the table is read and saved on starts
/// out that way, and there is no SQL tab on it to end the transaction its read opened. The
/// table must still save, without a person having to do anything; the setting a person's SQL
/// tab has is theirs and is not asked about.
#[test]
fn mssql_a_table_connection_that_opens_transactions_by_itself_still_saves() {
    only_with!("mssql");
    let scratch = ScratchDatabase::new("yz_r2_impl");
    let id = scratch.connection();
    for (n, setting) in ["SET IMPLICIT_TRANSACTIONS ON", "SET ANSI_DEFAULTS ON"].into_iter().enumerate() {
        run(&id, "setup", "DROP TABLE IF EXISTS dbo.i; CREATE TABLE dbo.i(id int PRIMARY KEY, v nvarchar(10)); INSERT INTO dbo.i VALUES (1, N'a'), (2, N'b');");
        let name = format!("impl-{n}");
        // What the server's default does to a connection that was just opened: the table's.
        pane_value(&id, &name, setting);
        assert_eq!(pane_value(&id, &name, "SELECT CAST(@@OPTIONS & 2 AS varchar(5))"), "2", "{setting}: the connection opens transactions by itself");
        let pane = open(&id, &name, &table("dbo", "i"));
        pane.edit(0, "v", Some("z"));
        runtime::block_on(session::write_back_set(&pane.set)).unwrap_or_else(|e| panic!("{setting}: {e}"));
        assert_eq!(first_row(&id, "SELECT v FROM dbo.i WHERE id = 1"), ["z"], "{setting}: another connection reads what was written");
        assert_eq!(pane_value(&id, &name, "SELECT CAST(@@TRANCOUNT AS varchar(2))"), "0", "{setting}: nothing is left open on the table's connection");
        session::release_owner(&pane.owner);
    }
}

#[test]
fn mssql_what_the_sql_tab_sets_does_not_change_what_the_table_reads_and_saves() {
    only_with!("mssql");
    let scratch = ScratchDatabase::new("yz_e2_ms");
    let id = scratch.connection();
    run(&id, "setup", "CREATE TABLE dbo.s(id int PRIMARY KEY, short nvarchar(5), tiny tinyint, big varchar(max), note nvarchar(20))");
    let baseline = mssql_pane(&id, "e2-plain", None);
    session::release_owner(&owner(&id, "e2-plain"));
    assert_eq!(baseline.rows_read, 3);
    assert_eq!(baseline.long_cell, 5000);
    assert_eq!(baseline.saved, "saved");
    assert!(baseline.too_long.starts_with("refused: Msg 2628") && baseline.too_big.starts_with("refused: Msg"), "{baseline:?}");
    assert_eq!(baseline.held, ["ab", "1", "5000", "ok"]);

    let tabs = [
        tab_state("SET ANSI_WARNINGS OFF", "SELECT CAST(@@OPTIONS & 8 AS varchar(5))", "0"),
        tab_state("SET ANSI_WARNINGS OFF; SET ARITHABORT OFF", "SELECT CAST(@@OPTIONS & 72 AS varchar(5))", "0"),
        tab_state("SET ROWCOUNT 1", "SELECT CAST(id AS varchar(5)) FROM dbo.s", "1"),
        tab_state("SET TEXTSIZE 8", "SELECT CAST(@@TEXTSIZE AS varchar(10))", "8"),
        tab_state("SET LANGUAGE British", "SELECT @@LANGUAGE", "British"),
        tab_state("SET LANGUAGE German", "SELECT @@LANGUAGE", "Deutsch"),
        tab_state("SET DATEFORMAT dmy", "SELECT CONVERT(varchar(10), CAST(N'2024-01-06' AS datetime), 120)", "2024-06-01"),
        tab_state("SET IMPLICIT_TRANSACTIONS ON", "SELECT CAST(@@OPTIONS & 2 AS varchar(5))", "2"),
        tab_state("SET ANSI_DEFAULTS ON", "SELECT CAST(@@OPTIONS & 2 AS varchar(5))", "2"),
        tab_state("SET NUMERIC_ROUNDABORT ON", "SELECT CAST(@@OPTIONS & 8192 AS varchar(10))", "8192"),
        tab_state("SET XACT_ABORT ON", "SELECT CAST(@@OPTIONS & 16384 AS varchar(10))", "16384"),
        tab_state("SET TRANSACTION ISOLATION LEVEL READ UNCOMMITTED", "SELECT CAST(transaction_isolation_level AS varchar(5)) FROM sys.dm_exec_sessions WHERE session_id = @@SPID", "1"),
        tab_state("SET DATEFIRST 3", "SELECT CAST(@@DATEFIRST AS varchar(5))", "3"),
        tab_state("SET CONCAT_NULL_YIELDS_NULL OFF", "SELECT CAST(@@OPTIONS & 4096 AS varchar(10))", "0"),
        tab_state("SET ANSI_NULLS OFF", "SELECT CAST(@@OPTIONS & 32 AS varchar(10))", "0"),
        tab_state("SET QUOTED_IDENTIFIER OFF", "SELECT CAST(@@OPTIONS & 256 AS varchar(10))", "0"),
        tab_state("SET LOCK_TIMEOUT 1", "SELECT CAST(@@LOCK_TIMEOUT AS varchar(10))", "1"),
        tab_state("USE tempdb", "SELECT DB_NAME()", "tempdb"),
        tab_state("CREATE TABLE #mine(x int); INSERT INTO #mine VALUES (7)", "SELECT CAST(x AS varchar(5)) FROM #mine", "7"),
        tab_state("BEGIN TRANSACTION; CREATE TABLE #mine2(x int)", "SELECT CAST(@@TRANCOUNT AS varchar(5))", "1"),
    ];
    let mut wrong = Vec::new();
    for (n, tab) in tabs.iter().enumerate() {
        let name = format!("e2-{n}");
        let pane = mssql_pane(&id, &name, Some(tab));
        if pane != baseline {
            wrong.push(format!("{}: the table pane did {pane:?}, without it {baseline:?}", tab.run));
        }
        let kept = tab_says(&id, &name, tab.ask);
        if kept.first().map(String::as_str) != Some(tab.answer) {
            wrong.push(format!("{}: the SQL tab answers {kept:?} afterwards, wanted {:?}: what it set was taken away", tab.run, tab.answer));
        }
        session::release_owner(&owner(&id, &name));
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

fn postgres_pane(id: &str, name: &str, tab: Option<&TabState>) -> Vec<String> {
    run(
        id,
        "reset",
        "DELETE FROM pg_s; INSERT INTO pg_s VALUES (1, 'a', '{x,y}', '1 hour', '2024-01-06 10:00:00+08', 0.1::float8 + 0.2::float8, '2024-01-05', 1.5)",
    );
    if let Some(tab) = tab {
        let state = run_in_the_tab(id, name, tab.run);
        assert_eq!(state.error.is_some(), tab.fails, "{}: {:?}", tab.run, state.error);
    }
    let pane = open(id, name, &table("public", "pg_s"));
    let shown = |column: &str| pane.shown(0, column);
    let printed = vec![shown("ts"), shown("f"), shown("iv")];
    pane.edit(0, "t", Some("中é"));
    pane.edit(0, "arr", Some("{a,NULL}"));
    pane.edit(0, "iv", Some("1 day 02:03:04"));
    pane.edit(0, "ts", Some(&shown("ts").replace("10:00:00", "11:00:00")));
    pane.edit(0, "d", Some("2024-02-03"));
    pane.edit(0, "n", Some("2.5"));
    let written = match runtime::block_on(session::write_back_set(&pane.set)) {
        Ok(_) => "saved".to_owned(),
        Err(e) => format!("refused: {e}"),
    };
    let mut outcome = printed;
    outcome.push(written);
    outcome.extend(first_row(
        id,
        "SELECT octet_length(t)::text, (arr[2] IS NULL)::text, EXTRACT(epoch FROM iv)::bigint::text, to_char(ts AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS'), to_char(d, 'YYYY-MM-DD'), n::text FROM pg_s WHERE id = 1",
    ));
    outcome
}

#[test]
fn postgres_what_the_sql_tab_sets_does_not_change_what_the_table_reads_and_saves() {
    only_with!("postgres");
    let scratch = PgScratchDatabase::new("yz_e2_pg");
    let db = scratch.name.clone();
    run(
        &scratch.maintenance,
        "style",
        &format!("ALTER DATABASE {db} SET datestyle = 'SQL, DMY'; ALTER DATABASE {db} SET timezone = 'Asia/Taipei'; ALTER DATABASE {db} SET extra_float_digits = 0"),
    );
    let id = scratch.connection();
    run(&id, "setup", "CREATE TABLE pg_s(id int PRIMARY KEY, t text, arr text[], iv interval, ts timestamptz, f float8, d date, n numeric(10,2))");
    let baseline = postgres_pane(&id, "e2-plain", None);
    session::release_owner(&owner(&id, "e2-plain"));
    assert_eq!(baseline[3], "saved", "{baseline:?}");
    assert_eq!(baseline[4..], ["5", "true", "93784", "2024-01-06 03:00:00", "2024-02-03", "2.50"], "{baseline:?}");

    let tabs = [
        tab_state("SET client_encoding = 'LATIN1'", "SHOW client_encoding", "LATIN1"),
        tab_state("SET datestyle = 'German, DMY'", "SHOW datestyle", "German, DMY"),
        tab_state("SET datestyle = 'SQL, DMY'; SELECT 1/0", "SHOW datestyle", "SQL, DMY"),
        tab_state("SET extra_float_digits = 0", "SHOW extra_float_digits", "0"),
        tab_state("SET IntervalStyle = 'sql_standard'", "SHOW intervalstyle", "sql_standard"),
        tab_state("SET TimeZone = 'Asia/Kolkata'", "SHOW timezone", "Asia/Kolkata"),
        tab_state("SET bytea_output = 'escape'", "SHOW bytea_output", "escape"),
        tab_state("SET array_nulls = off", "SHOW array_nulls", "off"),
        tab_state("SET search_path = pg_catalog", "SHOW search_path", "pg_catalog"),
        tab_state("SET standard_conforming_strings = off", "SHOW standard_conforming_strings", "off"),
        tab_state("SET default_transaction_read_only = on", "SHOW default_transaction_read_only", "on"),
        tab_state("SELECT set_config('datestyle', 'SQL, DMY', false), set_config('extra_float_digits', '0', false)", "SHOW extra_float_digits", "0"),
        tab_state("RESET ALL", "SHOW datestyle", "SQL, DMY"),
        tab_state("DISCARD ALL", "SHOW datestyle", "SQL, DMY"),
        tab_state("CREATE TEMP TABLE mine(x int); INSERT INTO mine VALUES (7)", "SELECT x::text FROM mine", "7"),
        tab_state("BEGIN", "SELECT (now() = statement_timestamp())::text", "false"),
    ];
    let mut wrong = Vec::new();
    for (n, tab) in tabs.iter().enumerate() {
        let name = format!("e2-{n}");
        let pane = postgres_pane(&id, &name, Some(tab));
        if pane != baseline {
            wrong.push(format!("{}: the table pane did {pane:?}, without it {baseline:?}", tab.run));
        }
        let kept = tab_says(&id, &name, tab.ask);
        if kept.first().map(String::as_str) != Some(tab.answer) {
            wrong.push(format!("{}: the SQL tab answers {kept:?} afterwards, wanted {:?}: what it set was taken away", tab.run, tab.answer));
        }
        session::release_owner(&owner(&id, &name));
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
