//! A Stop pressed before a write-back sends COMMIT keeps COMMIT from being sent,
//! in each engine's own commit step. The write-back's task learns of a Stop only
//! when it runs, and what runs the UPDATEs and the COMMIT may get to the commit
//! step first: so the commit step asks for the Stop itself.
//!
//! SQLite always runs; PostgreSQL and SQL Server when
//! `YUZORA_DATABASE_TEST_ENGINES` names them (see `engines.rs`). Each test
//! writes to a temporary table or file of its own.

use std::sync::atomic::{AtomicUsize, Ordering};

use yuzora_db::config::{ConnectionConfig, Engine, SslMode};
use yuzora_db::engine::{Conn, ConnectSpec, RowSink};
use yuzora_db::error::DbError;
use yuzora_db::model::{Cell, Column, ObjectRef, RowEdit};
use yuzora_db::results::WriteStop;
use yuzora_db::runtime;

mod common;
use common::enabled;

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

/// The first cell of the first row.
#[derive(Default)]
struct First(Option<String>);

impl RowSink for First {
    fn columns(&mut self, _: Vec<Column>) {}
    fn rows(&mut self, rows: Vec<Vec<Cell>>) -> bool {
        if self.0.is_none() {
            self.0 = rows.first().and_then(|r| r.first()).map(|c| c.display().into_owned());
        }
        true
    }
    fn done(&mut self, _: Option<u64>) {}
}

async fn scalar(conn: &mut Conn, sql: &str) -> String {
    let mut first = First::default();
    conn.execute(sql, &mut first).await.unwrap_or_else(|e| panic!("{sql}: {e}"));
    first.0.unwrap_or_else(|| panic!("{sql}: no row"))
}

async fn run(conn: &mut Conn, sql: &str) {
    conn.execute(sql, &mut First::default()).await.unwrap_or_else(|e| panic!("{sql}: {e}"));
}

/// The table holds `(1, 'a')`; `read` shows its value.
async fn a_stop_pressed_before_commit_is_sent_keeps_it_from_being_sent(mut conn: Conn, table: ObjectRef, read: &str) {
    let edit = RowEdit { key: vec![("id".into(), Cell::Int(1))], changes: vec![("v".into(), Cell::text("z"))] };

    let stop = WriteStop::new();
    conn.canceller().write_back_starts(&stop);
    stop.stop();
    assert_eq!(conn.apply_edits(&table, std::slice::from_ref(&edit)).await, Err(DbError::Cancelled));
    assert!(!conn.canceller().commit_sent(), "a COMMIT that was never sent is not one the server may have run");
    assert_eq!(scalar(&mut conn, read).await, "a", "a Stop before COMMIT wrote anyway");

    conn.canceller().write_back_starts(&WriteStop::new());
    assert_eq!(conn.apply_edits(&table, std::slice::from_ref(&edit)).await, Ok(1), "no Stop, so it writes");
    assert!(conn.canceller().commit_sent());
    assert_eq!(scalar(&mut conn, read).await, "z");
}

#[test]
fn sqlite_a_stop_pressed_before_commit_is_sent_keeps_it_from_being_sent() {
    static N: AtomicUsize = AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!(
        "yuzora-db-stop-before-commit-{}-{}.sqlite",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch("CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t VALUES (1, 'a');")
        .unwrap();
    let config = ConnectionConfig {
        name: "lite".into(),
        engine: Engine::Sqlite,
        path: path.to_string_lossy().into_owned(),
        ..ConnectionConfig::default()
    };
    runtime::block_on(async {
        let conn = Conn::connect(&ConnectSpec::new(config, None)).await.unwrap();
        let table = ObjectRef { schema: String::new(), name: "t".into() };
        a_stop_pressed_before_commit_is_sent_keeps_it_from_being_sent(conn, table, "SELECT v FROM t").await;
    });
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
}

#[test]
fn postgres_a_stop_pressed_before_commit_is_sent_keeps_it_from_being_sent() {
    only_with!("postgres");
    let config = ConnectionConfig {
        name: "pg".into(),
        engine: Engine::Postgres,
        host: "127.0.0.1".into(),
        port: Some(55432),
        user: "yuzora_full".into(),
        database: "yuzora_p8".into(),
        ssl: SslMode::Prefer,
        ..ConnectionConfig::default()
    };
    runtime::block_on(async {
        let mut conn = Conn::connect(&ConnectSpec::new(config, Some(password()))).await.unwrap();
        run(&mut conn, "CREATE TEMP TABLE yz_stop (id int PRIMARY KEY, v text); INSERT INTO yz_stop VALUES (1, 'a')")
            .await;
        let schema = scalar(&mut conn, "SELECT nspname FROM pg_namespace WHERE oid = pg_my_temp_schema()").await;
        let table = ObjectRef { schema, name: "yz_stop".into() };
        a_stop_pressed_before_commit_is_sent_keeps_it_from_being_sent(conn, table, "SELECT v FROM yz_stop").await;
    });
}

#[test]
fn mssql_a_stop_pressed_before_commit_is_sent_keeps_it_from_being_sent() {
    only_with!("mssql");
    let config = ConnectionConfig {
        name: "ms".into(),
        engine: Engine::Mssql,
        host: "127.0.0.1".into(),
        port: Some(51433),
        user: "yuzora_full".into(),
        database: "yuzora_p8".into(),
        ssl: SslMode::Prefer,
        ..ConnectionConfig::default()
    };
    runtime::block_on(async {
        let mut conn = Conn::connect(&ConnectSpec::new(config, Some(password()))).await.unwrap();
        run(&mut conn, "CREATE TABLE #stop (id int PRIMARY KEY, v nvarchar(10)); INSERT INTO #stop VALUES (1, N'a')")
            .await;
        let table = ObjectRef { schema: String::new(), name: "#stop".into() };
        a_stop_pressed_before_commit_is_sent_keeps_it_from_being_sent(conn, table, "SELECT v FROM #stop").await;
    });
}
