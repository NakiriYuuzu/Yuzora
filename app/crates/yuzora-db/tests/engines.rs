//! The three engines through `Conn`, the way the session uses them.
//!
//! SQLite always runs (it builds its own files). PostgreSQL and SQL Server
//! run when `YUZORA_DATABASE_TEST_ENGINES` names them and the fixture of
//! `tests/database/docker-compose.yml` is up:
//!
//! ```text
//! docker compose -f tests/database/docker-compose.yml --profile mssql up -d
//! YUZORA_DATABASE_TEST_ENGINES=sqlite,postgres,mssql cargo test -p yuzora-db --test engines
//! ```
//!
//! The password is the fixture's (`YUZORA_P8_DATABASE_PASSWORD` overrides it).
//! Tests that write do it in temporary tables or files of their own and leave
//! the fixture's data as it was. The performance runs are `#[ignore]`d:
//! `cargo test … --test engines -- --ignored --nocapture`.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use futures_util::FutureExt;
use yuzora_db::config::{ConnectionConfig, Engine, SslMode};
use yuzora_db::engine::{Conn, ConnectSpec, RowSink};
use yuzora_db::error::DbError;
use yuzora_db::model::{Cell, Column, ObjectKind, ObjectRef, RowEdit};
use yuzora_db::runtime;

mod common;
use common::enabled;

// ---------------------------------------------------------------- helpers

fn run<T>(work: impl Future<Output = T>) -> T {
    runtime::block_on(work)
}

/// Ends the test early, passing, when `engine` is not selected.
macro_rules! only_with {
    ($engine:expr) => {
        if !enabled($engine) {
            eprintln!("skipped: {} is not in YUZORA_DATABASE_TEST_ENGINES", $engine);
            return;
        }
    };
}

fn text(s: &str) -> Cell {
    Cell::text(s)
}

fn texts(row: &[Cell]) -> Vec<String> {
    row.iter().map(|c| if c.is_null() { "NULL".to_owned() } else { c.display().into_owned() }).collect()
}

/// What a statement produced.
#[derive(Debug, Default)]
struct Out {
    columns: Vec<Column>,
    rows: Vec<Vec<Cell>>,
    affected: Option<u64>,
    has_columns: bool,
}

/// A sink that keeps everything, optionally stopping after `stop_after` rows.
#[derive(Default)]
struct Collect {
    results: Vec<Out>,
    open: bool,
    batches: Vec<usize>,
    stop_after: Option<usize>,
}

impl Collect {
    fn stopping_after(rows: usize) -> Self {
        Collect { stop_after: Some(rows), ..Collect::default() }
    }
    fn last(&self) -> &Out {
        self.results.last().expect("a result")
    }
    fn total_rows(&self) -> usize {
        self.results.iter().map(|r| r.rows.len()).sum()
    }
}

impl RowSink for Collect {
    fn columns(&mut self, columns: Vec<Column>) {
        self.results.push(Out { columns, has_columns: true, ..Out::default() });
        self.open = true;
    }

    fn rows(&mut self, rows: Vec<Vec<Cell>>) -> bool {
        self.batches.push(rows.len());
        self.results.last_mut().expect("columns come before rows").rows.extend(rows);
        self.stop_after.is_none_or(|n| self.total_rows() < n)
    }

    fn done(&mut self, affected: Option<u64>) {
        if self.open {
            self.results.last_mut().expect("an open result").affected = affected;
            self.open = false;
        } else {
            self.results.push(Out { affected, ..Out::default() });
        }
    }
}

async fn query(conn: &mut Conn, sql: &str) -> Result<Collect, DbError> {
    let mut sink = Collect::default();
    conn.execute(sql, &mut sink).await?;
    Ok(sink)
}

/// The rows of the last result of `sql`, each cell as displayed.
async fn grid(conn: &mut Conn, sql: &str) -> Vec<Vec<String>> {
    let out = query(conn, sql).await.unwrap_or_else(|e| panic!("{sql}: {e}"));
    out.last().rows.iter().map(|r| texts(r)).collect()
}

/// The one cell of a one-cell result.
async fn scalar(conn: &mut Conn, sql: &str) -> String {
    grid(conn, sql).await.remove(0).remove(0)
}

fn query_error(e: DbError) -> String {
    match e {
        DbError::Query(m) => m,
        other => panic!("expected a query error, got {other:?}"),
    }
}

/// Whether `message` is the write-back refusal for a key found `n` times (in
/// whichever language the tests run in).
fn mismatch(message: &str, n: u64) -> bool {
    yuzora_db::i18n::t("engine.key_mismatch", &[("count", n.to_string())]).split("{key}").all(|part| message.contains(part))
}

/// A sink whose stop was pressed before anything ran (the pane's Stop while
/// its connection was still opening).
struct Stopped;

impl RowSink for Stopped {
    fn columns(&mut self, _: Vec<Column>) {}
    fn rows(&mut self, _: Vec<Vec<Cell>>) -> bool {
        false
    }
    fn done(&mut self, _: Option<u64>) {}
    fn cancelled(&self) -> bool {
        true
    }
}

fn password() -> String {
    std::env::var("YUZORA_P8_DATABASE_PASSWORD").unwrap_or_else(|_| "Yuzora-P8-Only-2026!".to_owned())
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../tests/database/fixtures")
}

// ---------------------------------------------------------------- SQLite

/// A SQLite file made for one test, removed with it.
struct TempDb(PathBuf);

impl TempDb {
    fn new(setup: &str) -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "yuzora-db-engines-{}-{}.sqlite",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let db = rusqlite::Connection::open(&path).expect("create the file");
        db.execute_batch(setup).expect("set the file up");
        TempDb(path)
    }

    fn spec(&self, read_only: bool) -> ConnectSpec {
        let config = ConnectionConfig {
            name: "lite".into(),
            engine: Engine::Sqlite,
            path: self.0.to_string_lossy().into_owned(),
            read_only,
            ..ConnectionConfig::default()
        };
        ConnectSpec::new(config, None)
    }

    async fn open(&self) -> Conn {
        Conn::connect(&self.spec(false)).await.expect("connect")
    }

    /// What the file holds, read behind the connection's back.
    fn peek(&self, sql: &str) -> Vec<Vec<String>> {
        let db = rusqlite::Connection::open(&self.0).unwrap();
        let mut stmt = db.prepare(sql).unwrap();
        let n = stmt.column_count();
        stmt.query_map([], |r| {
            (0..n).map(|i| r.get::<_, rusqlite::types::Value>(i).map(|v| format!("{v:?}"))).collect()
        })
        .unwrap()
        .map(Result::unwrap)
        .collect()
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm", "-journal"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", self.0.display()));
        }
    }
}

const LITE_SCHEMA: &str = "
    CREATE TABLE parent (id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE, note TEXT DEFAULT 'x');
    CREATE TABLE child (id INTEGER, seq INTEGER, parent_id INTEGER REFERENCES parent(id), label TEXT, PRIMARY KEY (id, seq));
    CREATE INDEX child_label ON child(label);
    CREATE VIEW parent_names AS SELECT name FROM parent;
    CREATE TABLE log (msg TEXT);
    CREATE TRIGGER parent_log AFTER INSERT ON parent BEGIN INSERT INTO log VALUES ('added ' || NEW.name); END;
";

#[test]
fn sqlite_connects_and_lists_what_is_there() {
    let db = TempDb::new(LITE_SCHEMA);
    run(async {
        let mut conn = db.open().await;
        assert!(conn.server_version().starts_with("SQLite 3."), "{}", conn.server_version());
        assert!(!conn.is_closed());
        assert_eq!(conn.databases().await.unwrap(), ["main"]);

        let objects = conn.objects().await.unwrap();
        let listed: Vec<_> = objects.iter().map(|o| (o.kind, o.name.as_str(), o.detail.as_str())).collect();
        assert_eq!(
            listed,
            [
                (ObjectKind::Table, "child", ""),
                (ObjectKind::Table, "log", ""),
                (ObjectKind::Table, "parent", ""),
                (ObjectKind::View, "parent_names", ""),
                (ObjectKind::Trigger, "parent_log", "parent"),
                (ObjectKind::Index, "child_label", "child"),
            ]
        );
        assert!(objects.iter().all(|o| o.schema.is_empty() && !o.name.starts_with("sqlite_")));
    });
}

#[test]
fn sqlite_refuses_what_is_not_a_database() {
    run(async {
        let missing = std::env::temp_dir().join("yuzora-db-engines-missing.sqlite");
        let config = ConnectionConfig {
            name: "lite".into(),
            engine: Engine::Sqlite,
            path: missing.to_string_lossy().into_owned(),
            ..ConnectionConfig::default()
        };
        let err = Conn::connect(&ConnectSpec::new(config.clone(), None)).await.err().unwrap();
        assert!(matches!(&err, DbError::Connect(m) if m.contains(&*missing.to_string_lossy())), "{err:?}");
        assert!(!missing.exists(), "connecting must not create the file");

        let junk = std::env::temp_dir().join(format!("yuzora-db-engines-junk-{}.txt", std::process::id()));
        std::fs::write(&junk, "this is not a database, just a few lines of text that are long enough\n".repeat(100))
            .unwrap();
        let config = ConnectionConfig { path: junk.to_string_lossy().into_owned(), ..config };
        let err = Conn::connect(&ConnectSpec::new(config, None)).await.err().unwrap();
        std::fs::remove_file(&junk).ok();
        assert!(matches!(&err, DbError::Connect(m) if m.contains("not a database")), "{err:?}");
    });
}

#[test]
fn sqlite_describes_keys_indexes_foreign_keys_and_ddl() {
    let db = TempDb::new(LITE_SCHEMA);
    run(async {
        let mut conn = db.open().await;
        let child = conn.describe(&ObjectRef { schema: String::new(), name: "child".into() }).await.unwrap();
        let columns: Vec<_> =
            child.columns.iter().map(|c| (c.name.as_str(), c.type_name.as_str(), c.primary_key)).collect();
        assert_eq!(
            columns,
            [
                ("id", "INTEGER", Some(1)),
                ("seq", "INTEGER", Some(2)),
                ("parent_id", "INTEGER", None),
                ("label", "TEXT", None)
            ]
        );
        assert_eq!(child.key_columns(), ["id", "seq"]);
        let indexes: Vec<_> =
            child.indexes.iter().map(|i| (i.name.as_str(), i.columns.clone(), i.unique, i.primary)).collect();
        assert!(indexes.contains(&("child_label", vec!["label".to_owned()], false, false)), "{indexes:?}");
        assert!(
            indexes.iter().any(|(_, cols, unique, primary)| *primary && *unique && cols == &["id", "seq"]),
            "{indexes:?}"
        );
        assert_eq!(child.foreign_keys.len(), 1);
        let fk = &child.foreign_keys[0];
        assert_eq!(
            (fk.columns.clone(), fk.referenced.name.as_str(), fk.referenced_columns.clone()),
            (vec!["parent_id".to_owned()], "parent", vec!["id".to_owned()])
        );
        assert!(
            child.ddl.contains("CREATE TABLE child") && child.ddl.contains("CREATE INDEX child_label"),
            "{}",
            child.ddl
        );

        let parent = conn.describe(&ObjectRef { schema: String::new(), name: "parent".into() }).await.unwrap();
        let note = parent.columns.iter().find(|c| c.name == "note").unwrap();
        assert_eq!((note.default.as_deref(), note.nullable), (Some("'x'"), true));
        let name = parent.columns.iter().find(|c| c.name == "name").unwrap();
        assert!(!name.nullable);
        assert!(parent.indexes.iter().any(|i| i.unique && !i.primary && i.columns == ["name"]));

        let view = conn.describe(&ObjectRef { schema: String::new(), name: "parent_names".into() }).await.unwrap();
        assert_eq!(view.columns.len(), 1);
        assert!(view.ddl.starts_with("CREATE VIEW parent_names"), "{}", view.ddl);

        let err = conn.describe(&ObjectRef { schema: String::new(), name: "nope".into() }).await.err().unwrap();
        assert!(matches!(err, DbError::NotFound(_)), "{err:?}");
    });
}

#[test]
fn sqlite_runs_scripts_statement_by_statement() {
    let db = TempDb::new(LITE_SCHEMA);
    run(async {
        let mut conn = db.open().await;
        let out = query(
            &mut conn,
            "INSERT INTO parent(name) VALUES ('a'), ('b'); -- two rows\n\
             SELECT id, name FROM parent ORDER BY id;\n\
             UPDATE parent SET note = 'y' WHERE id = 1;\n\
             CREATE TABLE z (a);\n\
             INSERT INTO parent(name) VALUES ('c') RETURNING id, name;",
        )
        .await
        .unwrap();
        let shape: Vec<_> = out.results.iter().map(|r| (r.has_columns, r.rows.len(), r.affected)).collect();
        assert_eq!(
            shape,
            [(false, 0, Some(2)), (true, 2, None), (false, 0, Some(1)), (false, 0, None), (true, 1, Some(1))]
        );
        assert_eq!(texts(&out.results[1].rows[0]), ["1", "a"]);
        let types: Vec<_> =
            out.results[1].columns.iter().map(|c| (c.name.as_str(), c.type_name.as_str(), c.numeric)).collect();
        assert_eq!(types, [("id", "INTEGER", true), ("name", "TEXT", false)]);
        // The trigger ran for each insert.
        assert_eq!(scalar(&mut conn, "SELECT count(*) FROM log").await, "3");

        // Values keep their kinds.
        let out = query(&mut conn, "SELECT 1, 1.5, 'x', NULL, x'ff'").await.unwrap();
        assert_eq!(
            out.last().rows[0],
            [Cell::Int(1), Cell::Float(1.5), text("x"), Cell::Null, Cell::Bytes(vec![0xff].into())]
        );
    });
}

#[test]
fn sqlite_scripts_may_hold_trigger_bodies() {
    let db = TempDb::new("CREATE TABLE a (x); CREATE TABLE b (x);");
    run(async {
        let mut conn = db.open().await;
        let out = query(
            &mut conn,
            "CREATE TRIGGER a_to_b AFTER INSERT ON a BEGIN\n  INSERT INTO b VALUES (NEW.x);\n  INSERT INTO b VALUES (NEW.x * 10);\nEND;\n\
             INSERT INTO a VALUES (1);\nSELECT x FROM b ORDER BY x",
        )
        .await
        .unwrap();
        assert_eq!(out.results.len(), 3);
        assert_eq!(out.last().rows.len(), 2);
    });
}

#[test]
fn sqlite_errors_name_the_cause_and_keep_earlier_results() {
    let db = TempDb::new(LITE_SCHEMA);
    run(async {
        let mut conn = db.open().await;
        let mut sink = Collect::default();
        let err = conn.execute("SELECT 1; SELECT * FROM nope; SELECT 3", &mut sink).await.err().unwrap();
        assert!(query_error(err).contains("no such table: nope"));
        assert_eq!(sink.results.len(), 1, "the statement before the error is kept, the one after is not run");
        let err = conn.execute("SELEC 1", &mut Collect::default()).await.err().unwrap();
        assert!(query_error(err).contains("syntax error"));
    });
}

#[test]
fn sqlite_hands_over_rows_in_batches_and_stops_on_request() {
    let db = TempDb::new("");
    run(async {
        let mut conn = db.open().await;
        let all = "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c WHERE x < 5000) SELECT x FROM c";
        let out = query(&mut conn, all).await.unwrap();
        assert_eq!(out.last().rows.len(), 5000);
        assert_eq!(out.batches.iter().sum::<usize>(), 5000);
        assert!(out.batches.iter().all(|b| *b <= yuzora_db::engine::BATCH), "{:?}", out.batches);

        let mut sink = Collect::stopping_after(1000);
        conn.execute(&format!("{all}; SELECT 99"), &mut sink).await.unwrap();
        assert!((1000..2000).contains(&sink.total_rows()), "{}", sink.total_rows());
        assert_eq!(sink.results.len(), 1, "the script ends where the sink stopped it");
        // And the connection is still good.
        assert_eq!(scalar(&mut conn, "SELECT 42").await, "42");
    });
}

#[test]
fn sqlite_writes_edits_in_one_transaction() {
    let db = TempDb::new(
        "CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT, n INTEGER);\
         INSERT INTO t VALUES (1, 'a', 1), (2, 'b', 2), (3, 'c', 3);\
         CREATE TABLE dup (k TEXT, v TEXT); INSERT INTO dup VALUES ('x', '1'), ('x', '2');\
         CREATE TABLE pair (a INTEGER, b TEXT, v TEXT, PRIMARY KEY (a, b)); INSERT INTO pair VALUES (1, 'p', 'old');",
    );
    run(async {
        let mut conn = db.open().await;
        let t = ObjectRef { schema: String::new(), name: "t".into() };
        let edit = |id: i64, changes: Vec<(&str, Cell)>| RowEdit {
            key: vec![("id".into(), Cell::Int(id))],
            changes: changes.into_iter().map(|(c, v)| (c.to_owned(), v)).collect(),
        };
        let written = conn
            .apply_edits(
                &t,
                &[edit(1, vec![("name", text("A"))]), edit(2, vec![("n", Cell::Null), ("name", text("B"))])],
            )
            .await
            .unwrap();
        assert_eq!(written, 2);
        assert_eq!(
            db.peek("SELECT id, name, n FROM t ORDER BY id"),
            [
                ["Integer(1)", "Text(\"A\")", "Integer(1)"],
                ["Integer(2)", "Text(\"B\")", "Null"],
                ["Integer(3)", "Text(\"c\")", "Integer(3)"]
            ]
        );

        // The second row is not there: nothing at all is written.
        let err = conn
            .apply_edits(&t, &[edit(3, vec![("name", text("Z"))]), edit(99, vec![("name", text("no"))])])
            .await
            .err()
            .unwrap();
        assert!(mismatch(&query_error(err), 0));
        assert_eq!(db.peek("SELECT name FROM t WHERE id = 3"), [["Text(\"c\")"]]);

        // A key that finds two rows is as wrong as one that finds none.
        let dup = ObjectRef { schema: String::new(), name: "dup".into() };
        let two = RowEdit { key: vec![("k".into(), text("x"))], changes: vec![("v".into(), text("9"))] };
        let err = conn.apply_edits(&dup, &[two]).await.err().unwrap();
        assert!(mismatch(&query_error(err), 2));
        assert_eq!(db.peek("SELECT v FROM dup ORDER BY v"), [["Text(\"1\")"], ["Text(\"2\")"]]);

        // A NULL key, an empty key: refused before anything runs.
        let null_key = RowEdit { key: vec![("id".into(), Cell::Null)], changes: vec![("n".into(), Cell::Int(0))] };
        assert!(matches!(conn.apply_edits(&t, &[null_key]).await.err().unwrap(), DbError::NotEditable(_)));
        let no_key = RowEdit { key: vec![], changes: vec![("n".into(), Cell::Int(0))] };
        assert!(matches!(conn.apply_edits(&t, &[no_key]).await.err().unwrap(), DbError::NotEditable(_)));

        // A composite key.
        let pair = ObjectRef { schema: String::new(), name: "pair".into() };
        let one = RowEdit {
            key: vec![("a".into(), Cell::Int(1)), ("b".into(), text("p"))],
            changes: vec![("v".into(), text("new"))],
        };
        assert_eq!(conn.apply_edits(&pair, &[one]).await.unwrap(), 1);
        assert_eq!(db.peek("SELECT v FROM pair"), [["Text(\"new\")"]]);
    });
}

#[test]
fn sqlite_cancel_interrupts_a_running_query() {
    let db = TempDb::new("");
    run(async {
        let mut conn = db.open().await;
        let canceller = conn.canceller();
        runtime::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            canceller.cancel().await;
        });
        let started = Instant::now();
        let endless = "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c) SELECT count(*) FROM c";
        let err = conn.execute(endless, &mut Collect::default()).await.err().unwrap();
        assert_eq!(err, DbError::Cancelled);
        assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
        assert!(!conn.is_closed());
        assert_eq!(scalar(&mut conn, "SELECT 1").await, "1");
    });
}

#[test]
fn sqlite_a_stop_pressed_before_the_statement_keeps_it_from_running() {
    let db = TempDb::new("CREATE TABLE t (v INTEGER); INSERT INTO t VALUES (1);");
    run(async {
        let mut conn = db.open().await;
        // The pane's stop reached the connection's canceller, then the statement came.
        conn.canceller().cancel().await;
        assert_eq!(conn.execute("UPDATE t SET v = 2", &mut Stopped).await, Err(DbError::Cancelled));
        assert_eq!(db.peek("SELECT v FROM t"), [["Integer(1)"]]);
        assert_eq!(scalar(&mut conn, "SELECT v FROM t").await, "1", "the connection goes on");
    });
}

/// The refusal to write back inside a transaction the SQL tab left open.
fn open_transaction() -> DbError {
    DbError::NotEditable(yuzora_db::i18n::t("edit.open_transaction", &[]))
}

fn one_edit(value: &str) -> RowEdit {
    RowEdit { key: vec![("id".into(), Cell::Int(1))], changes: vec![("v".into(), text(value))] }
}

#[test]
fn sqlite_write_back_waits_for_a_transaction_left_open() {
    let db = TempDb::new("CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t VALUES (1, 'a');");
    run(async {
        let mut conn = db.open().await;
        let t = ObjectRef { schema: String::new(), name: "t".into() };
        query(&mut conn, "BEGIN; INSERT INTO t VALUES (2, 'theirs')").await.unwrap();
        assert_eq!(conn.apply_edits(&t, &[one_edit("z")]).await, Err(open_transaction()));
        query(&mut conn, "ROLLBACK").await.unwrap();
        assert_eq!(db.peek("SELECT id, v FROM t"), [["Integer(1)", "Text(\"a\")"]]);
        assert_eq!(conn.apply_edits(&t, &[one_edit("z")]).await, Ok(1), "once it is closed, it writes");
    });
}

#[test]
fn sqlite_describes_generated_columns_as_not_writable() {
    let db = TempDb::new(
        "CREATE TABLE g (id INTEGER PRIMARY KEY, a INTEGER, twice INTEGER GENERATED ALWAYS AS (a * 2) VIRTUAL, \
                         next INTEGER AS (a + 1) STORED);",
    );
    run(async {
        let mut conn = db.open().await;
        let info = conn.describe(&ObjectRef { schema: String::new(), name: "g".into() }).await.unwrap();
        let generated: Vec<_> = info.columns.iter().map(|c| (c.name.as_str(), c.generated)).collect();
        assert_eq!(generated, [("id", false), ("a", false), ("twice", true), ("next", true)]);
    });
}

#[test]
fn sqlite_an_edited_number_keeps_its_type_in_a_column_without_affinity() {
    let db = TempDb::new("CREATE TABLE loose (id INTEGER PRIMARY KEY, v); INSERT INTO loose VALUES (1, 5), (2, 1.5);");
    run(async {
        let mut conn = db.open().await;
        let t = ObjectRef { schema: String::new(), name: "loose".into() };
        let rows = query(&mut conn, "SELECT v FROM loose ORDER BY id").await.unwrap().last().rows.clone();
        // What the grid makes of "7" and "2.5" typed over them.
        let edits = [
            RowEdit { key: vec![("id".into(), Cell::Int(1))], changes: vec![("v".into(), rows[0][0].edited("7".into()))] },
            RowEdit { key: vec![("id".into(), Cell::Int(2))], changes: vec![("v".into(), rows[1][0].edited("2.5".into()))] },
        ];
        assert_eq!(conn.apply_edits(&t, &edits).await, Ok(2));
        assert_eq!(db.peek("SELECT typeof(v), v FROM loose ORDER BY id"), [
            ["Text(\"integer\")", "Integer(7)"],
            ["Text(\"real\")", "Real(2.5)"]
        ]);
    });
}

#[test]
fn sqlite_read_only_refuses_writes() {
    let db = TempDb::new("CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t VALUES (1, 'a');");
    run(async {
        let mut conn = Conn::connect(&db.spec(true)).await.unwrap();
        assert_eq!(scalar(&mut conn, "SELECT v FROM t").await, "a");
        let err = conn.execute("INSERT INTO t VALUES (2, 'b')", &mut Collect::default()).await.err().unwrap();
        assert!(query_error(err).contains("readonly"));
        let t = ObjectRef { schema: String::new(), name: "t".into() };
        let edit = RowEdit { key: vec![("id".into(), Cell::Int(1))], changes: vec![("v".into(), text("z"))] };
        assert!(matches!(conn.apply_edits(&t, &[edit]).await, Err(DbError::Query(_))));
        assert_eq!(db.peek("SELECT v FROM t"), [["Text(\"a\")"]]);
    });
}

// ---------------------------------------------------------------- PostgreSQL

const PG_HOST: &str = "127.0.0.1";
const PG_PORT: u16 = 55432;

fn pg_config(user: &str, ssl: SslMode) -> ConnectionConfig {
    ConnectionConfig {
        name: "pg".into(),
        engine: Engine::Postgres,
        host: PG_HOST.into(),
        port: Some(PG_PORT),
        user: user.into(),
        database: "yuzora_p8".into(),
        ssl,
        ..ConnectionConfig::default()
    }
}

fn pg_spec(user: &str, ssl: SslMode) -> ConnectSpec {
    ConnectSpec::new(pg_config(user, ssl), Some(password()))
}

async fn pg() -> Conn {
    Conn::connect(&pg_spec("yuzora_full", SslMode::Prefer)).await.expect("connect to the PostgreSQL fixture")
}

fn pg_object(schema: &str, name: &str) -> ObjectRef {
    ObjectRef { schema: schema.into(), name: name.into() }
}

/// The schema of this session's temporary tables.
async fn pg_temp_schema(conn: &mut Conn) -> String {
    scalar(conn, "SELECT nspname FROM pg_namespace WHERE oid = pg_my_temp_schema()").await
}

#[test]
fn postgres_connects_with_every_tls_mode() {
    only_with!("postgres");
    run(async {
        let encrypted = "SELECT ssl FROM pg_stat_ssl WHERE pid = pg_backend_pid()";
        for (ssl, tls) in [(SslMode::Disable, "f"), (SslMode::Prefer, "t"), (SslMode::Require, "t")] {
            let mut conn = Conn::connect(&pg_spec("yuzora_full", ssl)).await.unwrap_or_else(|e| panic!("{ssl:?}: {e}"));
            assert_eq!(scalar(&mut conn, encrypted).await, tls, "{ssl:?}");
            assert!(conn.server_version().starts_with("PostgreSQL 16."), "{}", conn.server_version());
        }

        // The fixture's certificate is self-signed: with no CA given, no verified
        // connection. verify-ca doesn't even try: it trusts only a given CA.
        let err = Conn::connect(&pg_spec("yuzora_full", SslMode::VerifyFull)).await.err().unwrap();
        let DbError::Tls(message) = &err else { panic!("expected a TLS error, got {err:?}") };
        assert!(message.contains("invalid peer certificate"), "{message}");
        let err = Conn::connect(&pg_spec("yuzora_full", SslMode::VerifyCa)).await.err().unwrap();
        let ca_required = yuzora_db::i18n::t("engine.ca_required", &[]);
        assert!(matches!(&err, DbError::Config(m) | DbError::Tls(m) if m == &ca_required), "{err:?}");

        // The fixture's certificate is also a CA (`CA:TRUE`), which rustls refuses
        // as a server certificate even when it is the trusted root: the message
        // says so. The verify modes proper (issuer, name) are tried in memory in
        // engine/tls.rs, against a certificate that is not a CA.
        let ca = fixtures().join("postgres/tls/server.crt");
        for ssl in [SslMode::VerifyCa, SslMode::VerifyFull] {
            let mut spec = pg_spec("yuzora_full", ssl);
            spec.config.ca_file = ca.to_string_lossy().into_owned();
            let err = Conn::connect(&spec).await.err().unwrap();
            assert!(
                matches!(&err, DbError::Tls(m) if m.contains("CaUsedAsEndEntity") && m.contains(&yuzora_db::i18n::t("engine.ca_as_server", &[]))),
                "{ssl:?}: {err:?}"
            );
        }
        let mut spec = pg_spec("yuzora_full", SslMode::VerifyFull);
        spec.config.ca_file = "/no/such/ca.pem".into();
        let err = Conn::connect(&spec).await.err().unwrap();
        assert!(matches!(&err, DbError::Tls(m) if m.contains("/no/such/ca.pem")), "{err:?}");
    });
}

#[test]
fn postgres_connects_through_an_endpoint_and_keeps_the_configured_name() {
    only_with!("postgres");
    run(async {
        // The bytes go to the endpoint (a tunnel's local end), not to the host that is named.
        let mut spec = pg_spec("yuzora_full", SslMode::Require);
        spec.config.host = "db.invalid".into();
        spec.endpoint = Some((PG_HOST.into(), PG_PORT));
        let mut conn = Conn::connect(&spec).await.expect("connect through the endpoint");
        assert_eq!(scalar(&mut conn, "SELECT ssl FROM pg_stat_ssl WHERE pid = pg_backend_pid()").await, "t");
        // Without an endpoint, that name leads nowhere.
        spec.endpoint = None;
        assert!(matches!(Conn::connect(&spec).await.err().unwrap(), DbError::Connect(_)));
    });
}

#[test]
fn postgres_connection_failures_say_why() {
    only_with!("postgres");
    run(async {
        let mut spec = pg_spec("yuzora_full", SslMode::Prefer);
        spec.password = Some("not the password".into());
        let err = Conn::connect(&spec).await.err().unwrap();
        assert!(
            matches!(&err, DbError::Connect(m) if m.contains("password authentication failed") && m.contains("28P01")),
            "{err:?}"
        );

        let mut spec = pg_spec("yuzora_full", SslMode::Prefer);
        spec.config.port = Some(1);
        let err = Conn::connect(&spec).await.err().unwrap();
        assert!(
            matches!(&err, DbError::Connect(m) if m.contains("127.0.0.1:1") && m.to_lowercase().contains("refused")),
            "{err:?}"
        );

        let mut spec = pg_spec("yuzora_full", SslMode::Prefer);
        spec.database = Some("no_such_database".into());
        let err = Conn::connect(&spec).await.err().unwrap();
        assert!(
            matches!(&err, DbError::Connect(m) if m.contains("no_such_database") && m.contains("3D000")),
            "{err:?}"
        );
    });
}

#[test]
fn postgres_without_a_database_finds_one_it_may_open() {
    only_with!("postgres");
    run(async {
        // yuzora_full can open `postgres`.
        let mut spec = pg_spec("yuzora_full", SslMode::Prefer);
        spec.config.database.clear();
        let mut conn = Conn::connect(&spec).await.unwrap();
        assert_eq!(scalar(&mut conn, "SELECT current_database()").await, "postgres");

        // yuzora_readonly may not (CONNECT is revoked there); its same-name database is next.
        let mut spec = pg_spec("yuzora_readonly", SslMode::Prefer);
        spec.config.database.clear();
        let mut conn = Conn::connect(&spec).await.unwrap();
        assert_eq!(scalar(&mut conn, "SELECT current_database()").await, "yuzora_readonly");
    });
}

#[test]
fn postgres_lists_databases_and_the_objects_a_role_may_see() {
    only_with!("postgres");
    run(async {
        let mut conn = pg().await;
        let databases = conn.databases().await.unwrap();
        assert!(
            databases.contains(&"yuzora_p8".to_owned()) && databases.contains(&"postgres".to_owned()),
            "{databases:?}"
        );
        assert!(!databases.iter().any(|d| d.starts_with("template")), "{databases:?}");

        let objects = conn.objects().await.unwrap();
        let find = |schema: &str, name: &str| objects.iter().find(|o| o.schema == schema && o.name == name);
        assert_eq!(find("alpha", "value_extremes").map(|o| o.kind), Some(ObjectKind::Table));
        assert_eq!(find("alpha", "shared_name_view").map(|o| o.kind), Some(ObjectKind::View));
        assert_eq!(
            find("alpha", "long_query").map(|o| (o.kind, o.detail.as_str())),
            Some((ObjectKind::Function, "delay_seconds double precision"))
        );
        assert_eq!(find("audit", "record_dml_target").map(|o| o.kind), Some(ObjectKind::Function));
        assert_eq!(
            find("alpha", "dml_target_audit").map(|o| (o.kind, o.detail.as_str())),
            Some((ObjectKind::Trigger, "dml_target")),
            "a trigger, with its table"
        );
        assert!(find("audit", "dml_log_id_seq").is_some_and(|o| o.kind == ObjectKind::Sequence));
        assert!(
            objects.iter().all(|o| !matches!(o.schema.as_str(), "pg_catalog" | "information_schema")
                && !o.schema.starts_with("pg_toast"))
        );
        // Schemas in order, and within one, tables before views.
        assert!(objects.windows(2).all(|w| w[0].schema <= w[1].schema));
        let alpha: Vec<_> = objects.iter().filter(|o| o.schema == "alpha").map(|o| o.kind).collect();
        assert!(
            alpha.iter().rposition(|k| *k == ObjectKind::Table) < alpha.iter().position(|k| *k == ObjectKind::View)
        );

        // The read-only role has no USAGE on `audit`: it is not in its tree.
        let mut conn = Conn::connect(&pg_spec("yuzora_readonly", SslMode::Prefer)).await.unwrap();
        let objects = conn.objects().await.unwrap();
        assert!(objects.iter().any(|o| o.schema == "alpha"));
        assert!(!objects.iter().any(|o| o.schema == "audit"));
        let denied = conn
            .execute("UPDATE alpha.dml_target SET touched = touched WHERE false", &mut Collect::default())
            .await
            .err()
            .unwrap();
        assert!(query_error(denied).contains("permission denied for table dml_target"));
    });
}

#[test]
fn postgres_reads_every_value_as_the_server_prints_it() {
    only_with!("postgres");
    run(async {
        let mut conn = pg().await;
        let out = query(&mut conn, "SELECT * FROM alpha.value_extremes").await.unwrap();
        let types: Vec<_> =
            out.last().columns.iter().map(|c| (c.name.as_str(), c.type_name.as_str(), c.numeric)).collect();
        assert_eq!(
            types,
            [
                ("id", "int4", true),
                ("big_value", "int8", true),
                ("precise_numeric", "numeric", true),
                ("date_value", "date", false),
                ("time_value", "time", false),
                ("timestamp_value", "timestamp", false),
                ("json_value", "json", false),
                ("binary_value", "bytea", false),
                ("nullable_value", "text", false),
            ]
        );
        assert_eq!(
            texts(&out.last().rows[0]),
            [
                "1",
                "9223372036854775807",
                "12345678901234567890.123456789012345678",
                "2024-02-29",
                "23:59:58.123456",
                "2024-02-29 12:34:56.123456",
                "{\"beyondU64\":18446744073709551616,\"label\":\"fixture\"}",
                "\\x0001ff",
                "NULL",
            ]
        );
        assert!(out.last().rows[0][8].is_null());

        let row = grid(
            &mut conn,
            "SELECT ARRAY[1, 2], '1 day 02:03:04'::interval, '10.0.0.1/24'::inet, 'a'::name, 1e400::numeric, true",
        )
        .await;
        assert_eq!(
            row[0],
            ["{1,2}", "1 day 02:03:04", "10.0.0.1/24", "a", format!("1{}", "0".repeat(400)).as_str(), "t"]
        );
        let out = query(&mut conn, "SELECT ARRAY['a'], 1::numeric(5,2), 'x'::varchar(3)").await.unwrap();
        let types: Vec<_> = out.last().columns.iter().map(|c| c.type_name.as_str()).collect();
        assert_eq!(types, ["text[]", "numeric", "varchar"]);
    });
}

#[test]
fn postgres_describes_tables_views_and_their_keys() {
    only_with!("postgres");
    run(async {
        let mut conn = pg().await;
        let info = conn.describe(&pg_object("alpha", "value_extremes")).await.unwrap();
        let by_name = |n: &str| info.columns.iter().find(|c| c.name == n).unwrap().clone();
        assert_eq!(info.columns.len(), 9);
        assert_eq!((by_name("id").primary_key, by_name("id").nullable), (Some(1), false));
        assert_eq!(by_name("precise_numeric").type_name, "numeric(38,18)");
        assert_eq!(by_name("timestamp_value").type_name, "timestamp(6) without time zone");
        assert!(by_name("nullable_value").nullable && by_name("nullable_value").primary_key.is_none());
        assert_eq!(info.key_columns(), ["id"]);
        assert!(info.indexes.iter().any(|i| i.primary && i.unique && i.columns == ["id"]), "{:?}", info.indexes);
        assert!(info.ddl.starts_with("CREATE TABLE \"alpha\".\"value_extremes\" ("), "{}", info.ddl);
        assert!(
            info.ddl.contains("\"precise_numeric\" numeric(38,18) NOT NULL") && info.ddl.contains("PRIMARY KEY (id)"),
            "{}",
            info.ddl
        );

        let view = conn.describe(&pg_object("alpha", "shared_name_view")).await.unwrap();
        assert_eq!(view.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["id", "source"]);
        assert!(
            view.ddl.starts_with("CREATE VIEW \"alpha\".\"shared_name_view\" AS\n") && view.ddl.contains("shared_name"),
            "{}",
            view.ddl
        );

        let err = conn.describe(&pg_object("alpha", "nope")).await.err().unwrap();
        assert!(matches!(err, DbError::NotFound(_)), "{err:?}");
    });
}

#[test]
fn postgres_describes_foreign_keys_defaults_and_generated_columns() {
    only_with!("postgres");
    run(async {
        let mut conn = pg().await;
        query(
            &mut conn,
            "CREATE TEMP TABLE yz_parent (a int, b text, PRIMARY KEY (a, b));\
             CREATE TEMP TABLE yz_child (\
               id int GENERATED BY DEFAULT AS IDENTITY PRIMARY KEY,\
               pa int NOT NULL, pb text NOT NULL,\
               label text DEFAULT 'none' NOT NULL,\
               doubled int GENERATED ALWAYS AS (pa * 2) STORED,\
               CONSTRAINT yz_child_label_check CHECK (length(label) > 0),\
               CONSTRAINT yz_child_parent FOREIGN KEY (pa, pb) REFERENCES yz_parent (a, b));\
             CREATE INDEX yz_child_label ON yz_child (lower(label), pa);",
        )
        .await
        .unwrap();
        let schema = pg_temp_schema(&mut conn).await;
        let info = conn.describe(&pg_object(&schema, "yz_child")).await.unwrap();
        assert_eq!(info.foreign_keys.len(), 1);
        let fk = &info.foreign_keys[0];
        assert_eq!(fk.name, "yz_child_parent");
        assert_eq!(
            (fk.columns.clone(), fk.referenced.name.as_str(), fk.referenced_columns.clone()),
            (vec!["pa".to_owned(), "pb".to_owned()], "yz_parent", vec!["a".to_owned(), "b".to_owned()])
        );
        assert_eq!(fk.referenced.schema, schema);
        let label = info.columns.iter().find(|c| c.name == "label").unwrap();
        assert_eq!(label.default.as_deref(), Some("'none'::text"));
        assert_eq!(
            info.columns.iter().find(|c| c.name == "doubled").unwrap().default,
            None,
            "a generated column's expression is not a default"
        );
        let generated: Vec<_> = info.columns.iter().map(|c| (c.name.as_str(), c.generated)).collect();
        assert_eq!(
            generated,
            [("id", false), ("pa", false), ("pb", false), ("label", false), ("doubled", true)],
            "an identity BY DEFAULT may be written"
        );
        query(&mut conn, "CREATE TEMP TABLE yz_always (id int GENERATED ALWAYS AS IDENTITY PRIMARY KEY, v text)")
            .await
            .unwrap();
        let always = conn.describe(&pg_object(&schema, "yz_always")).await.unwrap();
        assert_eq!(always.columns.iter().map(|c| c.generated).collect::<Vec<_>>(), [true, false]);
        let index = info.indexes.iter().find(|i| i.name == "yz_child_label").unwrap();
        assert_eq!(index.columns, ["lower(label)", "pa"]);
        for part in [
            "GENERATED BY DEFAULT AS IDENTITY NOT NULL",
            "DEFAULT 'none'::text NOT NULL",
            "GENERATED ALWAYS AS ((pa * 2)) STORED",
            "CONSTRAINT \"yz_child_pkey\" PRIMARY KEY (id)",
            "CONSTRAINT \"yz_child_label_check\" CHECK (length(label) > 0)",
            "CONSTRAINT \"yz_child_parent\" FOREIGN KEY (pa, pb) REFERENCES yz_parent(a, b)",
            "CREATE INDEX yz_child_label ON ",
        ] {
            assert!(info.ddl.contains(part), "{part}\n{}", info.ddl);
        }
        // The primary key's own index is not repeated as a CREATE INDEX.
        assert!(!info.ddl.contains("yz_child_pkey ON"), "{}", info.ddl);
        query(&mut conn, "DROP TABLE yz_child; DROP TABLE yz_parent").await.unwrap();
    });
}

#[test]
fn postgres_runs_scripts_and_names_the_failing_statement() {
    only_with!("postgres");
    run(async {
        let mut conn = pg().await;
        let out = query(
            &mut conn,
            "CREATE TEMP TABLE yz_t (id int PRIMARY KEY, v text);\n\
             INSERT INTO yz_t VALUES (1, 'a'), (2, 'b'); -- two rows\n\
             UPDATE yz_t SET v = 'c' WHERE id = 1;\n\
             SELECT * FROM yz_t ORDER BY id;\n\
             DELETE FROM yz_t WHERE id = 2 RETURNING id;\n\
             DO $$ BEGIN PERFORM 1; END $$;\n\
             SELECT 'last' AS tail",
        )
        .await
        .unwrap();
        let shape: Vec<_> = out.results.iter().map(|r| (r.has_columns, r.rows.len(), r.affected)).collect();
        assert_eq!(
            shape,
            [
                (false, 0, None),
                (false, 0, Some(2)),
                (false, 0, Some(1)),
                (true, 2, None),
                (true, 1, Some(1)),
                (false, 0, None),
                (true, 1, None)
            ]
        );
        assert_eq!(texts(&out.results[3].rows[0]), ["1", "c"]);
        assert_eq!(texts(&out.results[4].rows[0]), ["2"]);

        // The server's whole answer, not "db error".
        let mut sink = Collect::default();
        let err = conn.execute("SELECT 1; SELECT * FROM nope_not_here", &mut sink).await.err().unwrap();
        let message = query_error(err);
        assert!(
            message.contains("relation \"nope_not_here\" does not exist")
                && message.contains("42P01")
                && message.contains("POSITION: 15"),
            "{message}"
        );
        assert_eq!(sink.results.len(), 1);

        // Inside a transaction, the first failure is the one reported.
        let mut sink = Collect::default();
        let err = conn.execute("BEGIN; SELECT * FROM nope_not_here; SELECT 3", &mut sink).await.err().unwrap();
        assert!(query_error(err).contains("relation \"nope_not_here\" does not exist"));
        query(&mut conn, "ROLLBACK").await.unwrap();
        assert_eq!(scalar(&mut conn, "SELECT 1").await, "1");

        // Detail and hint travel along.
        let err = conn.execute("SELECT 1/0", &mut Collect::default()).await.err().unwrap();
        assert!(query_error(err).contains("division by zero"));
        let err = conn.execute("SELECT '{1,2'::int[]", &mut Collect::default()).await.err().unwrap();
        assert!(query_error(err).contains("DETAIL:"));
    });
}

#[test]
fn postgres_streams_in_batches_and_stops_when_the_sink_has_enough() {
    only_with!("postgres");
    run(async {
        let mut conn = pg().await;
        let out = query(&mut conn, "SELECT g FROM generate_series(1, 3000) g").await.unwrap();
        assert_eq!(out.last().rows.len(), 3000);
        assert!(out.batches.iter().all(|b| *b <= yuzora_db::engine::BATCH), "{:?}", out.batches);

        let mut sink = Collect::stopping_after(1000);
        let started = Instant::now();
        conn.execute("SELECT g, md5(g::text) FROM generate_series(1, 5000000) g; SELECT 99", &mut sink).await.unwrap();
        assert!((1000..20_000).contains(&sink.total_rows()), "{}", sink.total_rows());
        assert_eq!(sink.results.len(), 1, "the script ends where the sink stopped it");
        assert!(started.elapsed() < Duration::from_secs(10), "{:?}", started.elapsed());
        // The connection took the cancel and is as good as before.
        assert!(!conn.is_closed());
        assert_eq!(scalar(&mut conn, "SELECT 42").await, "42");
        assert_eq!(scalar(&mut conn, "SELECT 43").await, "43");
    });
}

#[test]
fn postgres_cancel_stops_a_long_query_and_the_connection_goes_on() {
    only_with!("postgres");
    run(async {
        let mut conn = pg().await;
        for sql in ["SELECT pg_sleep(30)", "SELECT alpha.long_query(30)", "SELECT 1; SELECT pg_sleep(30)"] {
            let canceller = conn.canceller();
            runtime::spawn(async move {
                tokio::time::sleep(Duration::from_millis(300)).await;
                canceller.cancel().await;
            });
            let started = Instant::now();
            let err = conn.execute(sql, &mut Collect::default()).await.err().unwrap();
            assert_eq!(err, DbError::Cancelled, "{sql}");
            assert!(started.elapsed() < Duration::from_secs(5), "{sql}: {:?}", started.elapsed());
            assert!(!conn.is_closed());
            assert_eq!(scalar(&mut conn, "SELECT 'again'").await, "again");
        }
    });
}

#[test]
fn postgres_a_stop_pressed_before_the_statement_keeps_it_from_running() {
    only_with!("postgres");
    run(async {
        let mut conn = pg().await;
        query(&mut conn, "CREATE TEMP TABLE yz_stop (v int); INSERT INTO yz_stop VALUES (1)").await.unwrap();
        conn.canceller().cancel().await;
        assert_eq!(conn.execute("UPDATE yz_stop SET v = 2", &mut Stopped).await, Err(DbError::Cancelled));
        assert_eq!(scalar(&mut conn, "SELECT v FROM yz_stop").await, "1");
    });
}

/// The fixture through a relay that can hold the server back from hearing of
/// a cancel (the relay lives as long as the test).
async fn pg_through(relay: &common::LaggingPostgres, ssl: SslMode) -> Conn {
    let mut spec = pg_spec("yuzora_full", ssl);
    spec.config.port = Some(relay.port());
    Conn::connect(&spec).await.expect("connect to the PostgreSQL fixture through the relay")
}

/// A stop is a request to the server about whatever the connection runs when
/// the request gets there. Once the engine says the stop is done, nothing that
/// the connection runs afterwards may be its target.
#[test]
fn postgres_a_stop_that_the_server_hears_late_is_not_felt_by_what_runs_after_it() {
    only_with!("postgres");
    let relay = common::LaggingPostgres::start();
    for ssl in [SslMode::Prefer, SslMode::Disable] {
        run(async {
            let mut conn = pg_through(&relay, ssl).await;
            query(&mut conn, "CREATE TEMP TABLE yz_late (v int); INSERT INTO yz_late VALUES (1)").await.unwrap();
            relay.hold_connections(Duration::from_millis(250));
            conn.canceller().cancel().await;
            relay.release();
            assert_eq!(conn.execute("UPDATE yz_late SET v = 2", &mut Stopped).await, Err(DbError::Cancelled));
            let after = query(&mut conn, "SELECT v, pg_sleep(1.5) FROM yz_late").await.map(|_| ());
            assert_eq!(after, Ok(()), "{ssl:?}: the stop, still on its way, cancelled the statement that ran after it");
            assert_eq!(scalar(&mut conn, "SELECT v FROM yz_late").await, "1");
        });
    }
}

/// Keeps the runtime's worker threads busy until dropped: a task spawned
/// meanwhile cannot start.
struct BusyWorkers(Arc<AtomicBool>);

impl BusyWorkers {
    fn new() -> Self {
        let (go, busy) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicUsize::new(0)));
        for _ in 0..2 {
            let (go, busy) = (go.clone(), busy.clone());
            runtime::spawn(async move {
                busy.fetch_add(1, Ordering::SeqCst);
                let since = Instant::now();
                while !go.load(Ordering::SeqCst) && since.elapsed() < Duration::from_secs(10) {
                    std::thread::sleep(Duration::from_millis(1));
                }
            });
        }
        while busy.load(Ordering::SeqCst) < 2 {
            std::thread::sleep(Duration::from_millis(1));
        }
        BusyWorkers(go)
    }
}

impl Drop for BusyWorkers {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// The connection is not handed to new work while a stop is on its way, and
/// the stop counts as on its way from the moment it is pressed, not from when
/// the task that sends it first gets to run.
#[test]
fn postgres_a_stop_from_the_background_is_on_its_way_before_it_returns() {
    only_with!("postgres");
    run(async {
        let mut conn = pg().await;
        let busy = BusyWorkers::new();
        conn.canceller().cancel_in_background();
        let counted = conn.settled().now_or_never().is_none();
        drop(busy);
        assert!(counted, "the stop was not counted on its way when it returned");
        conn.settled().await;
        assert!(!conn.is_closed());
        assert_eq!(scalar(&mut conn, "SELECT 'after', pg_sleep(0.3)").await, "after");
    });
}

/// A stop that was not heard to be dealt with may still arrive: the connection
/// it was for is not trusted with another statement.
#[test]
fn postgres_a_stop_the_server_does_not_answer_retires_the_connection() {
    only_with!("postgres");
    let relay = common::LaggingPostgres::start();
    run(async {
        let conn = pg_through(&relay, SslMode::Disable).await;
        relay.hold_cancels(Duration::from_secs(8));
        let started = Instant::now();
        conn.canceller().cancel().await;
        assert!(started.elapsed() < Duration::from_secs(7), "{:?}", started.elapsed());
        assert!(conn.is_closed(), "the connection is kept for statements a stop can still arrive in");
        relay.release();
        assert_eq!(scalar(&mut pg_through(&relay, SslMode::Disable).await, "SELECT 'new'").await, "new");
    });
}

#[test]
fn postgres_a_stop_the_server_hears_late_still_ends_the_statement_it_was_for() {
    only_with!("postgres");
    let relay = common::LaggingPostgres::start();
    run(async {
        let mut conn = pg_through(&relay, SslMode::Prefer).await;
        relay.hold_connections(Duration::from_millis(250));
        let canceller = conn.canceller();
        runtime::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            canceller.cancel().await;
        });
        let started = Instant::now();
        assert_eq!(conn.execute("SELECT pg_sleep(30)", &mut Collect::default()).await, Err(DbError::Cancelled));
        let took = started.elapsed();
        assert!((Duration::from_millis(450)..Duration::from_secs(5)).contains(&took), "{took:?}");
        relay.release();
        assert!(!conn.is_closed());
        assert_eq!(scalar(&mut conn, "SELECT 'again'").await, "again");
    });
}

/// A statement of three seconds with the connection's stop pressed 300 ms into it.
async fn stopped_while_it_sleeps(conn: &mut Conn) -> Result<(), DbError> {
    let canceller = conn.canceller();
    runtime::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        canceller.cancel().await;
    });
    conn.execute("SELECT pg_sleep(3)", &mut Collect::default()).await
}

/// A host can name several addresses, some of which are other servers or none
/// at all. The connection is made to one of them, and a stop is for the server
/// that connection is with: not for the first address the host gives when the
/// stop is pressed, nor for what the configuration points at.
#[test]
fn postgres_a_stop_goes_to_the_address_the_connection_was_made_to() {
    only_with!("postgres");
    let relay = common::LaggingPostgres::start();
    let door = common::ClosedDoor::start();
    for ssl in [SslMode::Disable, SslMode::Prefer] {
        run(async {
            let mut spec = pg_spec("yuzora_full", ssl);
            spec.config.port = Some(door.addr().port());
            spec.addresses = Some(vec![door.addr(), relay.addr()]);
            let mut conn = Conn::connect(&spec).await.expect("connect past the address that closes at once");
            let taken = door.taken();
            let outcome = stopped_while_it_sleeps(&mut conn).await;
            assert_eq!(outcome, Err(DbError::Cancelled), "{ssl:?}: the statement ran to its end, the stop went astray");
            assert_eq!(door.taken(), taken, "{ssl:?}: the stop was sent to an address the connection is not with");
            assert!(!conn.is_closed());
            assert_eq!(scalar(&mut conn, "SELECT 'again'").await, "again");
        });
    }
}

/// Through a tunnel the bytes go to its local end and the configured host is
/// only the name the certificate is for: it is not looked up, by the
/// connection or by a stop.
#[test]
fn postgres_a_stop_goes_to_the_tunnels_end_and_not_to_the_configured_host() {
    only_with!("postgres");
    let relay = common::LaggingPostgres::start();
    for ssl in [SslMode::Disable, SslMode::Prefer] {
        run(async {
            let mut spec = pg_spec("yuzora_full", ssl);
            spec.config.host = "no-such-host.invalid".into();
            spec.endpoint = Some(("127.0.0.1".into(), relay.port()));
            let mut conn = Conn::connect(&spec).await.expect("connect through the tunnel's end");
            assert_eq!(stopped_while_it_sleeps(&mut conn).await, Err(DbError::Cancelled), "{ssl:?}");
            assert!(!conn.is_closed());
        });
    }
}

/// A host name as the machine resolves it (`localhost` is `::1` and
/// `127.0.0.1` where there is IPv6): the connection and its stop are with the
/// same server.
#[test]
fn postgres_a_host_name_is_looked_up_and_its_stop_ends_the_statement() {
    only_with!("postgres");
    run(async {
        let mut spec = pg_spec("yuzora_full", SslMode::Prefer);
        spec.config.host = "localhost".into();
        let mut conn = Conn::connect(&spec).await.expect("connect to localhost");
        assert_eq!(stopped_while_it_sleeps(&mut conn).await, Err(DbError::Cancelled));
        assert!(!conn.is_closed());
        assert_eq!(scalar(&mut conn, "SELECT 'again'").await, "again");
    });
}

/// The write-back is a run of statements of its own, each of them a target of
/// a stop that is late: the UPDATE, and the COMMIT, which a deferred trigger
/// keeps busy for a while.
fn a_write_back_after_a_stop_the_server_hears_late(ddl: &str) {
    let relay = common::LaggingPostgres::start();
    run(async {
        let mut conn = pg_through(&relay, SslMode::Prefer).await;
        query(&mut conn, "CREATE TEMP TABLE yz_wb (id int PRIMARY KEY, v text); INSERT INTO yz_wb VALUES (1, 'a')")
            .await
            .unwrap();
        query(&mut conn, ddl).await.unwrap();
        let t = pg_object(&pg_temp_schema(&mut conn).await, "yz_wb");
        relay.hold_connections(Duration::from_millis(250));
        conn.canceller().cancel().await;
        relay.release();
        assert_eq!(
            conn.apply_edits(&t, &[one_edit("z")]).await,
            Ok(1),
            "the stop, still on its way, cut the write-back short"
        );
        assert_eq!(scalar(&mut conn, "SELECT v FROM yz_wb").await, "z");
        assert_eq!(scalar(&mut conn, "SELECT 'again'").await, "again");
    });
}

#[test]
fn postgres_a_write_back_is_not_cut_short_in_its_update_by_a_stop_the_server_hears_late() {
    only_with!("postgres");
    a_write_back_after_a_stop_the_server_hears_late(
        "CREATE FUNCTION pg_temp.slow_update() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_sleep(1.5); RETURN NEW; END $$;
         CREATE TRIGGER slow_update BEFORE UPDATE ON yz_wb FOR EACH ROW EXECUTE FUNCTION pg_temp.slow_update();",
    );
}

#[test]
fn postgres_a_write_back_is_not_cut_short_in_its_commit_by_a_stop_the_server_hears_late() {
    only_with!("postgres");
    a_write_back_after_a_stop_the_server_hears_late(
        "CREATE FUNCTION pg_temp.slow_commit() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_sleep(1.5); RETURN NULL; END $$;
         CREATE CONSTRAINT TRIGGER slow_commit AFTER UPDATE ON yz_wb DEFERRABLE INITIALLY DEFERRED
           FOR EACH ROW EXECUTE FUNCTION pg_temp.slow_commit();",
    );
}

#[test]
fn postgres_write_back_waits_for_a_transaction_left_open() {
    only_with!("postgres");
    run(async {
        let mut conn = pg().await;
        query(&mut conn, "CREATE TEMP TABLE yz_tx (id int PRIMARY KEY, v text); INSERT INTO yz_tx VALUES (1, 'a')")
            .await
            .unwrap();
        let t = pg_object(&pg_temp_schema(&mut conn).await, "yz_tx");
        // Write-back would have committed the SQL tab's own work with it.
        query(&mut conn, "BEGIN; INSERT INTO yz_tx VALUES (2, 'theirs')").await.unwrap();
        assert_eq!(conn.apply_edits(&t, &[one_edit("z")]).await, Err(open_transaction()));
        query(&mut conn, "ROLLBACK").await.unwrap();
        assert_eq!(grid(&mut conn, "SELECT id, v FROM yz_tx").await, [["1", "a"]]);
        // A transaction already failed is still open.
        let _ = query(&mut conn, "BEGIN; SELECT 1 / 0").await;
        assert_eq!(conn.apply_edits(&t, &[one_edit("z")]).await, Err(open_transaction()));
        query(&mut conn, "ROLLBACK").await.unwrap();
        assert_eq!(conn.apply_edits(&t, &[one_edit("z")]).await, Ok(1));
        assert_eq!(scalar(&mut conn, "SELECT v FROM yz_tx").await, "z");
    });
}

#[test]
fn postgres_splits_scripts_the_way_the_sessions_string_mode_reads_them() {
    only_with!("postgres");
    run(async {
        let mut conn = pg().await;
        query(&mut conn, "CREATE TEMP TABLE yz_keep (v int); INSERT INTO yz_keep VALUES (1)").await.unwrap();
        // With standard_conforming_strings off, `\'` is a quote inside the string:
        // the DELETE is text, not a statement.
        query(&mut conn, "SET standard_conforming_strings = off").await.unwrap();
        let out = query(&mut conn, "SELECT 'it\\'; DELETE FROM yz_keep; SELECT 2'; SELECT 3").await.unwrap();
        assert_eq!(texts(&out.results[0].rows[0]), ["it'; DELETE FROM yz_keep; SELECT 2"]);
        assert_eq!(scalar(&mut conn, "SELECT count(*) FROM yz_keep").await, "1");
        // Turned off by the script itself, it holds for the statements after.
        query(&mut conn, "SET standard_conforming_strings = on").await.unwrap();
        query(&mut conn, "SET standard_conforming_strings = off; SELECT 'a\\'; DELETE FROM yz_keep; SELECT 1'")
            .await
            .unwrap();
        assert_eq!(scalar(&mut conn, "SELECT count(*) FROM yz_keep").await, "1");
        query(&mut conn, "SET standard_conforming_strings = on").await.unwrap();
    });
}

#[test]
fn postgres_writes_text_back_as_the_columns_type() {
    only_with!("postgres");
    run(async {
        let mut conn = pg().await;
        query(
            &mut conn,
            "CREATE TEMP TABLE yz_e (id int PRIMARY KEY, n numeric(10,2), j jsonb, b bytea, ts timestamptz, flag bool, tag text);\
             INSERT INTO yz_e VALUES (1, 1, '{}', '\\x00', '2024-01-01 00:00:00+00', false, 'a'), (2, 2, '{}', '\\x00', '2024-01-01 00:00:00+00', false, 'b'), (3, 3, '{}', '\\x00', '2024-01-01 00:00:00+00', false, 'c');\
             CREATE TEMP TABLE yz_dup (k text, v text);\
             INSERT INTO yz_dup VALUES ('x', '1'), ('x', '2');",
        )
        .await
        .unwrap();
        let schema = pg_temp_schema(&mut conn).await;
        let table = pg_object(&schema, "yz_e");
        let key = |id: &str| vec![("id".to_owned(), text(id))];
        let set = |changes: Vec<(&str, Cell)>| changes.into_iter().map(|(c, v)| (c.to_owned(), v)).collect::<Vec<_>>();

        let edits = [
            RowEdit {
                key: key("1"),
                changes: set(vec![
                    ("n", text("12.50")),
                    ("j", text("{\"a\": 18446744073709551616}")),
                    ("b", text("\\xdeadbeef")),
                    ("ts", text("2024-02-29 12:34:56.123456+02")),
                    ("flag", text("t")),
                    ("tag", Cell::Null),
                ]),
            },
            RowEdit { key: key("2"), changes: set(vec![("tag", text("it's \"quoted\""))]) },
        ];
        assert_eq!(conn.apply_edits(&table, &edits).await.unwrap(), 2);
        let row =
            grid(&mut conn, "SELECT n, j, b, ts AT TIME ZONE 'UTC', flag, tag FROM yz_e WHERE id = 1").await.remove(0);
        assert_eq!(
            row,
            ["12.50", "{\"a\": 18446744073709551616}", "\\xdeadbeef", "2024-02-29 10:34:56.123456", "t", "NULL"]
        );
        assert_eq!(scalar(&mut conn, "SELECT tag FROM yz_e WHERE id = 2").await, "it's \"quoted\"");

        // One of the rows is not there: the first is not written either.
        let edits = [
            RowEdit { key: key("3"), changes: set(vec![("tag", text("changed"))]) },
            RowEdit { key: key("99"), changes: set(vec![("tag", text("nobody"))]) },
        ];
        let err = conn.apply_edits(&table, &edits).await.err().unwrap();
        assert!(mismatch(&query_error(err), 0));
        assert_eq!(scalar(&mut conn, "SELECT tag FROM yz_e WHERE id = 3").await, "c");

        // A value the column can't hold: the server says so, and nothing is written.
        let bad = [RowEdit { key: key("3"), changes: set(vec![("n", text("not a number"))]) }];
        assert!(query_error(conn.apply_edits(&table, &bad).await.err().unwrap()).contains("invalid input syntax"));

        // Two rows under one key.
        let dup = pg_object(&schema, "yz_dup");
        let two = RowEdit { key: vec![("k".into(), text("x"))], changes: set(vec![("v", text("9"))]) };
        assert!(mismatch(&query_error(conn.apply_edits(&dup, &[two]).await.err().unwrap()), 2));
        assert_eq!(grid(&mut conn, "SELECT v FROM yz_dup ORDER BY v").await, [["1"], ["2"]]);

        let null_key = RowEdit { key: vec![("id".into(), Cell::Null)], changes: set(vec![("tag", text("x"))]) };
        assert!(matches!(conn.apply_edits(&table, &[null_key]).await.err().unwrap(), DbError::NotEditable(_)));
        // The session is not left inside a failed transaction.
        assert_eq!(scalar(&mut conn, "SELECT count(*) FROM yz_e").await, "3");
    });
}

#[test]
fn postgres_edits_are_counted_per_row_even_with_triggers() {
    only_with!("postgres");
    run(async {
        let mut conn = pg().await;
        // A trigger that writes other rows on every update must not confuse the count.
        query(
            &mut conn,
            "CREATE TEMP TABLE yz_log (id int);\
             CREATE TEMP TABLE yz_tr (id int PRIMARY KEY, touched int NOT NULL DEFAULT 0);\
             INSERT INTO yz_tr(id) VALUES (1), (2);\
             CREATE FUNCTION pg_temp.yz_note() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN INSERT INTO pg_temp.yz_log VALUES (NEW.id), (NEW.id); RETURN NEW; END $$;\
             CREATE TRIGGER yz_tr_audit AFTER UPDATE ON yz_tr FOR EACH ROW EXECUTE FUNCTION pg_temp.yz_note();",
        )
        .await
        .unwrap();
        let schema = pg_temp_schema(&mut conn).await;
        let edits: Vec<_> = (1..=2)
            .map(|id| RowEdit {
                key: vec![("id".into(), text(&id.to_string()))],
                changes: vec![("touched".into(), text("5"))],
            })
            .collect();
        assert_eq!(conn.apply_edits(&pg_object(&schema, "yz_tr"), &edits).await.unwrap(), 2);
        assert_eq!(scalar(&mut conn, "SELECT count(*) FROM yz_log").await, "4");
        assert_eq!(scalar(&mut conn, "SELECT sum(touched) FROM yz_tr").await, "10");
    });
}

#[test]
fn postgres_reports_a_lost_connection() {
    only_with!("postgres");
    run(async {
        let mut conn = pg().await;
        let pid = scalar(&mut conn, "SELECT pg_backend_pid()").await;
        let mut other = pg().await;
        assert_eq!(scalar(&mut other, &format!("SELECT pg_terminate_backend({pid})")).await, "t");
        let err = conn.execute("SELECT 1", &mut Collect::default()).await.err().unwrap();
        let message = query_error(err);
        assert!(
            message.contains("terminating connection") || message.contains("lost") || message.contains("closed"),
            "{message}"
        );
        // Give the driver a moment to notice, then the pool would open a new one.
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(conn.is_closed());
    });
}

// ---------------------------------------------------------------- SQL Server

const MS_HOST: &str = "127.0.0.1";
const MS_PORT: u16 = 51433;

fn ms_config(user: &str, ssl: SslMode, database: &str) -> ConnectionConfig {
    ConnectionConfig {
        name: "ms".into(),
        engine: Engine::Mssql,
        host: MS_HOST.into(),
        port: Some(MS_PORT),
        user: user.into(),
        database: database.into(),
        ssl,
        ..ConnectionConfig::default()
    }
}

fn ms_spec(user: &str, ssl: SslMode, database: &str) -> ConnectSpec {
    ConnectSpec::new(ms_config(user, ssl, database), Some(password()))
}

/// The fixture's login, in the fixture's database.
async fn ms() -> Conn {
    Conn::connect(&ms_spec("yuzora_full", SslMode::Prefer, "yuzora_p8"))
        .await
        .expect("connect to the SQL Server fixture")
}

/// `sa` in `database`: for tests that make tables of their own (in `tempdb`).
async fn ms_sa(database: &str) -> Conn {
    Conn::connect(&ms_spec("sa", SslMode::Prefer, database)).await.expect("connect to the SQL Server fixture as sa")
}

fn ms_object(schema: &str, name: &str) -> ObjectRef {
    ObjectRef { schema: schema.into(), name: name.into() }
}

#[test]
fn mssql_connects_with_every_tls_mode() {
    only_with!("mssql");
    run(async {
        let encrypted = "SELECT encrypt_option FROM sys.dm_exec_connections WHERE session_id = @@SPID";
        let ca = |name: &str| fixtures().join("mssql/tls").join(name).to_string_lossy().into_owned();
        // `prefer` encrypts the login and leaves the rest to the server, which here does not insist.
        for (ssl, ca_file, expected) in [
            (SslMode::Disable, String::new(), "FALSE"),
            (SslMode::Prefer, String::new(), "FALSE"),
            (SslMode::Require, String::new(), "TRUE"),
            (SslMode::VerifyCa, ca("ca.pem"), "TRUE"),
            (SslMode::VerifyFull, ca("ca.pem"), "TRUE"),
        ] {
            let mut spec = ms_spec("sa", ssl, "");
            spec.config.ca_file = ca_file;
            let mut conn = Conn::connect(&spec).await.unwrap_or_else(|e| panic!("{ssl:?}: {e}"));
            assert_eq!(scalar(&mut conn, encrypted).await, expected, "{ssl:?}");
            assert!(conn.server_version().starts_with("SQL Server 16."), "{}", conn.server_version());
        }

        // The fixture's certificate is signed by its own CA: without it, or with another, nothing verifies.
        for (ssl, ca_file) in [(SslMode::VerifyFull, String::new()), (SslMode::VerifyCa, ca("other-ca.pem"))] {
            let mut spec = ms_spec("sa", ssl, "");
            spec.config.ca_file = ca_file;
            let err = Conn::connect(&spec).await.err().unwrap();
            assert!(matches!(&err, DbError::Tls(m) if m.contains("127.0.0.1:51433")), "{ssl:?}: {err:?}");
        }
        let mut spec = ms_spec("sa", SslMode::VerifyFull, "");
        spec.config.ca_file = ca("ca.pem");
        spec.config.host = "localhost".into();
        let mut conn = Conn::connect(&spec).await.expect("the certificate names localhost too");
        assert_eq!(scalar(&mut conn, "SELECT 1").await, "1");
    });
}

#[test]
fn mssql_connection_failures_say_why() {
    only_with!("mssql");
    run(async {
        let mut spec = ms_spec("sa", SslMode::Prefer, "");
        spec.password = Some("not the password".into());
        let err = Conn::connect(&spec).await.err().unwrap();
        assert!(
            matches!(&err, DbError::Connect(m) if m.contains("Login failed for user") && m.contains("Msg 18456")),
            "{err:?}"
        );

        let mut spec = ms_spec("sa", SslMode::Prefer, "");
        spec.config.port = Some(1);
        let err = Conn::connect(&spec).await.err().unwrap();
        assert!(matches!(&err, DbError::Connect(m) if m.contains("127.0.0.1:1")), "{err:?}");

        let err = Conn::connect(&ms_spec("sa", SslMode::Prefer, "no_such_database")).await.err().unwrap();
        assert!(
            matches!(&err, DbError::Connect(m) if m.contains("no_such_database") && m.contains("Msg 4060")),
            "{err:?}"
        );

        // A named instance can only be reached by its port.
        let mut spec = ms_spec("sa", SslMode::Prefer, "");
        spec.config.instance = "SQLEXPRESS".into();
        spec.config.port = None;
        let err = Conn::connect(&spec).await.err().unwrap();
        assert!(matches!(&err, DbError::Config(m) if m.contains("SQLEXPRESS") && m.contains("port")), "{err:?}");
    });
}

#[test]
fn mssql_lists_databases_and_objects() {
    only_with!("mssql");
    run(async {
        let mut conn = ms().await;
        let databases = conn.databases().await.unwrap();
        assert!(
            databases.contains(&"yuzora_p8".to_owned()) && databases.contains(&"master".to_owned()),
            "{databases:?}"
        );

        let objects = conn.objects().await.unwrap();
        let find = |schema: &str, name: &str| objects.iter().find(|o| o.schema == schema && o.name == name);
        assert_eq!(find("alpha", "value_extremes").map(|o| o.kind), Some(ObjectKind::Table));
        assert_eq!(find("alpha", "rows_1201").map(|o| (o.kind, o.rows)), Some((ObjectKind::Table, Some(1201))));
        assert_eq!(find("alpha", "shared_name_view").map(|o| o.kind), Some(ObjectKind::View));
        assert_eq!(find("alpha", "long_query").map(|o| o.kind), Some(ObjectKind::Procedure));
        assert_eq!(find("audit", "dml_log").map(|o| o.kind), Some(ObjectKind::Table));
        assert_eq!(
            find("alpha", "dml_target_audit").map(|o| (o.kind, o.detail.as_str())),
            Some((ObjectKind::Trigger, "dml_target")),
            "a trigger, with its table"
        );
        assert!(objects.iter().all(|o| o.schema != "sys" && o.schema != "INFORMATION_SCHEMA"));
        assert!(objects.windows(2).all(|w| w[0].schema <= w[1].schema));
        let alpha: Vec<_> = objects.iter().filter(|o| o.schema == "alpha").map(|o| o.kind).collect();
        assert!(
            alpha.iter().rposition(|k| *k == ObjectKind::Table) < alpha.iter().position(|k| *k == ObjectKind::View)
        );

        // Another database, through a connection of its own.
        let mut master = Conn::connect(&ms_spec("sa", SslMode::Prefer, "master")).await.unwrap();
        assert_eq!(scalar(&mut master, "SELECT DB_NAME()").await, "master");
        assert!(!master.objects().await.unwrap().iter().any(|o| o.name == "value_extremes"));
    });
}

#[test]
fn mssql_reads_values_as_text_or_their_own_kind() {
    only_with!("mssql");
    run(async {
        let mut conn = ms().await;
        let out = query(&mut conn, "SELECT * FROM alpha.value_extremes").await.unwrap();
        let types: Vec<_> =
            out.last().columns.iter().map(|c| (c.name.as_str(), c.type_name.as_str(), c.numeric)).collect();
        assert_eq!(
            types,
            [
                ("id", "int", true),
                ("big_value", "bigint", true),
                ("precise_decimal", "decimal", true),
                ("date_value", "date", false),
                ("time_value", "time", false),
                ("timestamp_value", "datetime2", false),
                ("json_value", "nvarchar", false),
                ("binary_value", "varbinary", false),
                ("nullable_value", "nvarchar", false),
            ]
        );
        let row = &out.last().rows[0];
        assert_eq!(row[0], Cell::Int(1));
        assert_eq!(row[1], Cell::Int(i64::MAX));
        assert_eq!(
            texts(&row[2..7]),
            [
                "12345678901234567890.123456789012345678",
                "2024-02-29",
                "23:59:58.123456",
                "2024-02-29 12:34:56.123456",
                "{\"beyondU64\":18446744073709551616,\"label\":\"fixture\"}",
            ]
        );
        assert_eq!(row[7], Cell::Bytes(vec![0x00, 0x01, 0xff].into()));
        assert_eq!(row[8], Cell::Null);

        let out = query(
            &mut conn,
            "SELECT CAST(-0.5 AS money), CAST(1 AS bit), CAST(0.1 AS real), CAST(1.5 AS float), CAST(N'<a>1</a>' AS xml), \
                    CAST('6f9619ff-8b86-d011-b42d-00c04fc964ff' AS uniqueidentifier), CAST(-5.25 AS decimal(5,2)), \
                    CAST('2024-02-29 12:34:56.123456 +08:00' AS datetimeoffset(6)), CAST('2024-02-29 12:34:56.123' AS datetime), \
                    CAST('2024-02-29 12:34:00' AS smalldatetime), CAST(255 AS tinyint), CAST(-7 AS smallint), CAST(N'é' AS nchar(2)), \
                    CAST(NULL AS int), CAST(0x as varbinary(1))",
        )
        .await
        .unwrap();
        let row = &out.last().rows[0];
        assert_eq!(row[0].display(), "-0.5000");
        assert_eq!(row[1], Cell::Bool(true));
        assert_eq!(row[2], Cell::Float(0.1));
        assert_eq!(row[3], Cell::Float(1.5));
        assert_eq!(
            texts(&row[4..14]),
            [
                "<a>1</a>",
                "6F9619FF-8B86-D011-B42D-00C04FC964FF",
                "-5.25",
                "2024-02-29 12:34:56.123456 +08:00",
                "2024-02-29 12:34:56.123",
                "2024-02-29 12:34:00",
                "255",
                "-7",
                "é ",
                "NULL"
            ]
        );
        assert_eq!(row[14], Cell::Bytes(Vec::new().into()));
        // The driver does not tell a nullable smalldatetime from a datetime.
        let kinds: Vec<_> = out.last().columns.iter().map(|c| c.type_name.as_str()).collect();
        assert_eq!(
            kinds,
            [
                "money",
                "bit",
                "real",
                "float",
                "xml",
                "uniqueidentifier",
                "decimal",
                "datetimeoffset",
                "datetime",
                "datetime",
                "tinyint",
                "smallint",
                "nchar",
                "int",
                "varbinary"
            ]
        );
    });
}

#[test]
fn mssql_runs_go_batches_and_several_result_sets() {
    only_with!("mssql");
    run(async {
        let mut conn = ms().await;
        let out = query(
            &mut conn,
            "CREATE TABLE #t (id int PRIMARY KEY, v nvarchar(10))\n\
             GO\n\
             INSERT INTO #t VALUES (1, N'a'), (2, N'b'); INSERT INTO #t VALUES (3, N'c')\n\
             go -- the third batch\n\
             UPDATE #t SET v = N'z' WHERE id < 3; SELECT * FROM #t ORDER BY id; SELECT count(*) AS n FROM #t\n\
             GO\n\
             SELECT 1 AS one, 'x' AS two; SELECT N'only' AS three",
        )
        .await
        .unwrap();
        let shape: Vec<_> = out.results.iter().map(|r| (r.has_columns, r.rows.len(), r.affected)).collect();
        assert_eq!(
            shape,
            [
                (false, 0, None),
                (false, 0, Some(3)),
                (false, 0, Some(2)),
                (true, 3, None),
                (true, 1, None),
                (true, 1, None),
                (true, 1, None)
            ]
        );
        assert_eq!(texts(&out.results[3].rows[0]), ["1", "z"]);
        assert_eq!(out.results[4].rows[0], [Cell::Int(3)]);
        assert_eq!(out.results[5].columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["one", "two"]);
        assert_eq!(out.results[6].columns[0].name, "three");

        // A statement that reports no count is "done", not "0 rows".
        let out = query(&mut conn, "SET NOCOUNT ON; UPDATE #t SET v = v; SET NOCOUNT OFF").await.unwrap();
        assert_eq!(out.results.len(), 1);
        assert_eq!(out.results[0].affected, None);
    });
}

#[test]
fn mssql_counts_rows_changed_through_procedures() {
    only_with!("mssql");
    run(async {
        let mut conn = ms_sa("tempdb").await;
        query(
            &mut conn,
            "CREATE TABLE #c (id int PRIMARY KEY, n int NOT NULL DEFAULT 0)\n\
             GO\n\
             INSERT INTO #c(id) VALUES (1), (2), (3)\n\
             GO\n\
             CREATE PROCEDURE #bump @max int AS UPDATE #c SET n = n + 1 WHERE id <= @max\n\
             GO\n\
             CREATE PROCEDURE #bump_quiet @max int AS BEGIN SET NOCOUNT ON; UPDATE #c SET n = n + 1 WHERE id <= @max END",
        )
        .await
        .unwrap();
        let out = query(&mut conn, "EXEC #bump 2").await.unwrap();
        let affected: Vec<_> = out.results.iter().map(|r| r.affected).collect();
        assert_eq!(affected, [Some(2)]);
        assert!(out.results.iter().all(|r| !r.has_columns));
        let out = query(&mut conn, "EXEC #bump_quiet 2").await.unwrap();
        assert_eq!(out.results.iter().map(|r| r.affected).collect::<Vec<_>>(), [None]);
        assert_eq!(scalar(&mut conn, "SELECT SUM(n) FROM #c").await, "4");
    });
}

#[test]
fn mssql_errors_name_the_cause_and_the_connection_goes_on() {
    only_with!("mssql");
    run(async {
        let mut conn = ms().await;
        let mut sink = Collect::default();
        let err = conn.execute("SELECT 1 AS a; SELECT * FROM nope_not_here", &mut sink).await.err().unwrap();
        let message = query_error(err);
        assert!(
            message.contains("Invalid object name 'nope_not_here'")
                && message.contains("Msg 208")
                && message.contains("Level 16"),
            "{message}"
        );
        let err = conn.execute("SELECT 1 FROM", &mut Collect::default()).await.err().unwrap();
        assert!(query_error(err).contains("Incorrect syntax"));
        // A later batch of the same script is not run once one fails.
        let mut sink = Collect::default();
        let err =
            conn.execute("SELECT 1\nGO\nSELECT * FROM nope_not_here\nGO\nSELECT 3", &mut sink).await.err().unwrap();
        assert!(query_error(err).contains("Msg 208"));
        assert_eq!(sink.results.len(), 1);
        assert!(!conn.is_closed());
        assert_eq!(scalar(&mut conn, "SELECT 'still here'").await, "still here");
        // A procedure's error says where it came from.
        query(&mut conn, "CREATE PROCEDURE #boom AS RAISERROR('boom from inside', 16, 1)").await.unwrap();
        let err = conn.execute("EXEC #boom", &mut Collect::default()).await.err().unwrap();
        let message = query_error(err);
        assert!(message.contains("boom from inside") && message.contains("Procedure #boom"), "{message}");
    });
}

#[test]
fn mssql_batches_rows_and_drops_the_socket_when_the_sink_stops_early() {
    only_with!("mssql");
    run(async {
        let mut conn = ms().await;
        let big = "SELECT TOP (300000) a.object_id, b.name FROM sys.all_objects a CROSS JOIN sys.all_objects b";
        let out = query(&mut conn, "SELECT TOP (3000) a.object_id FROM sys.all_objects a CROSS JOIN sys.all_objects b")
            .await
            .unwrap();
        assert_eq!(out.last().rows.len(), 3000);
        assert!(out.batches.iter().all(|b| *b <= yuzora_db::engine::BATCH), "{:?}", out.batches);

        let mut sink = Collect::stopping_after(1000);
        let started = Instant::now();
        conn.execute(&format!("{big}; SELECT 99"), &mut sink).await.unwrap();
        assert!((1000..20_000).contains(&sink.total_rows()), "{}", sink.total_rows());
        assert_eq!(sink.results.len(), 1, "the script ends where the sink stopped it");
        assert!(started.elapsed() < Duration::from_secs(10), "{:?}", started.elapsed());
        // The rest of that result could only be dropped with the socket; the next call opens a new one.
        assert!(conn.is_closed());
        assert_eq!(scalar(&mut conn, "SELECT 42").await, "42");
        assert!(!conn.is_closed());
    });
}

#[test]
fn mssql_a_stop_pressed_before_the_statement_keeps_it_from_running() {
    only_with!("mssql");
    run(async {
        let mut conn = ms().await;
        query(&mut conn, "CREATE TABLE #stop (v int); INSERT INTO #stop VALUES (1)").await.unwrap();
        conn.canceller().cancel().await;
        assert_eq!(conn.execute("UPDATE #stop SET v = 2", &mut Stopped).await, Err(DbError::Cancelled));
        assert_eq!(scalar(&mut conn, "SELECT v FROM #stop").await, "1");
    });
}

#[test]
fn mssql_go_lines_may_carry_block_comments() {
    only_with!("mssql");
    run(async {
        let mut conn = ms().await;
        let out = query(&mut conn, "SELECT 1 AS a\nGO /* next batch */\nSELECT 2 AS b").await.unwrap();
        assert_eq!(out.results.iter().map(|r| texts(&r.rows[0])).collect::<Vec<_>>(), [["1"], ["2"]]);
    });
}

#[test]
fn mssql_write_back_waits_for_a_transaction_left_open() {
    only_with!("mssql");
    run(async {
        let mut conn = ms().await;
        query(&mut conn, "CREATE TABLE #tx (id int PRIMARY KEY, v nvarchar(10)); INSERT INTO #tx VALUES (1, N'a')")
            .await
            .unwrap();
        let t = ms_object("", "#tx");
        // Nested, the write-back's COMMIT would only count down: the SQL tab's ROLLBACK would undo it.
        query(&mut conn, "BEGIN TRANSACTION; INSERT INTO #tx VALUES (2, N'theirs')").await.unwrap();
        assert_eq!(conn.apply_edits(&t, &[one_edit("z")]).await, Err(open_transaction()));
        query(&mut conn, "ROLLBACK").await.unwrap();
        assert_eq!(grid(&mut conn, "SELECT id, v FROM #tx").await, [["1", "a"]]);
        assert_eq!(conn.apply_edits(&t, &[one_edit("z")]).await, Ok(1));
        assert_eq!(scalar(&mut conn, "SELECT v FROM #tx").await, "z");
    });
}

#[test]
fn mssql_table_sql_stays_in_the_connections_database_after_a_use() {
    only_with!("mssql");
    run(async {
        // The same table in the connection's database and in tempdb.
        let mut conn = ms_sa("yuzora_p8").await;
        let name = format!("yz_ctx_{}", std::process::id());
        let remove_both = format!(
            "IF OBJECT_ID(N'yuzora_p8.dbo.{name}') IS NOT NULL DROP TABLE yuzora_p8.dbo.{name};\n\
             IF OBJECT_ID(N'tempdb.dbo.{name}') IS NOT NULL DROP TABLE tempdb.dbo.{name};"
        );
        query(&mut conn, &remove_both).await.unwrap();
        query(
            &mut conn,
            &format!(
                "CREATE TABLE yuzora_p8.dbo.{name} (id int PRIMARY KEY, v nvarchar(10));\n\
                 INSERT INTO yuzora_p8.dbo.{name} VALUES (1, N'home');\n\
                 CREATE TABLE tempdb.dbo.{name} (id int PRIMARY KEY, v nvarchar(10));\n\
                 INSERT INTO tempdb.dbo.{name} VALUES (1, N'other');"
            ),
        )
        .await
        .unwrap();
        assert_eq!(conn.database(), Some("yuzora_p8"));

        // A script moves the session; the table's SQL names where it was opened.
        query(&mut conn, "USE tempdb").await.unwrap();
        assert_eq!(scalar(&mut conn, "SELECT DB_NAME()").await, "tempdb");
        let table = ms_object("dbo", &name);
        let db = conn.database().map(str::to_owned);
        let select = yuzora_db::engine::select_sql(Engine::Mssql, db.as_deref(), &table, None, 10);
        assert_eq!(grid(&mut conn, &select).await, [["1", "home"]]);
        let count = yuzora_db::engine::count_sql(Engine::Mssql, db.as_deref(), &table);
        assert_eq!(scalar(&mut conn, &count).await, "1");
        let edit = RowEdit { key: vec![("id".into(), Cell::Int(1))], changes: vec![("v".into(), text("edited"))] };
        assert_eq!(conn.apply_edits(&table, &[edit]).await.unwrap(), 1);
        assert_eq!(scalar(&mut conn, &format!("SELECT v FROM yuzora_p8.dbo.{name}")).await, "edited");
        assert_eq!(scalar(&mut conn, &format!("SELECT v FROM tempdb.dbo.{name}")).await, "other");

        query(&mut conn, &remove_both).await.unwrap();
    });
}

#[test]
fn mssql_cancel_ends_the_query_and_the_server_session() {
    only_with!("mssql");
    run(async {
        // sa may look at the server's connections; each has an id of its own (a session number is reused).
        let which = "SELECT CAST(connection_id AS varchar(40)) FROM sys.dm_exec_connections WHERE session_id = @@SPID";
        let mut conn = ms_sa("yuzora_p8").await;
        let mut other = ms_sa("master").await;
        for sql in ["WAITFOR DELAY '00:00:30'", "EXEC alpha.long_query"] {
            let before = scalar(&mut conn, which).await;
            let canceller = conn.canceller();
            runtime::spawn(async move {
                tokio::time::sleep(Duration::from_millis(300)).await;
                canceller.cancel().await;
            });
            let started = Instant::now();
            let err = conn.execute(sql, &mut Collect::default()).await.err().unwrap();
            assert_eq!(err, DbError::Cancelled, "{sql}");
            assert!(started.elapsed() < Duration::from_secs(5), "{sql}: {:?}", started.elapsed());
            assert!(conn.is_closed(), "{sql}");
            // The next call on the connection opens a new socket.
            assert_ne!(scalar(&mut conn, which).await, before);
            assert!(!conn.is_closed());
            // The server let the old connection go.
            let mut gone = false;
            for _ in 0..50 {
                let count = format!("SELECT count(*) FROM sys.dm_exec_connections WHERE connection_id = '{before}'");
                if scalar(&mut other, &count).await == "0" {
                    gone = true;
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            assert!(gone, "connection {before} still on the server after {sql}");
        }
    });
}

#[test]
fn mssql_describes_tables_views_and_procedures() {
    only_with!("mssql");
    run(async {
        let mut conn = ms().await;
        let info = conn.describe(&ms_object("alpha", "value_extremes")).await.unwrap();
        let columns: Vec<_> =
            info.columns.iter().map(|c| (c.name.as_str(), c.type_name.as_str(), c.nullable, c.primary_key)).collect();
        assert_eq!(
            columns,
            [
                ("id", "int", false, Some(1)),
                ("big_value", "bigint", false, None),
                ("precise_decimal", "decimal(38,18)", false, None),
                ("date_value", "date", false, None),
                ("time_value", "time(6)", false, None),
                ("timestamp_value", "datetime2(6)", false, None),
                ("json_value", "nvarchar(max)", false, None),
                ("binary_value", "varbinary(32)", false, None),
                ("nullable_value", "nvarchar(64)", true, None),
            ]
        );
        assert_eq!(info.key_columns(), ["id"]);
        assert!(info.indexes.iter().any(|i| i.primary && i.unique && i.columns == ["id"]), "{:?}", info.indexes);
        for part in [
            "CREATE TABLE [alpha].[value_extremes] (",
            "[id] int NOT NULL",
            "[nullable_value] nvarchar(64) NULL",
            "PRIMARY KEY CLUSTERED ([id])",
            "CHECK (isjson([json_value])=(1))",
        ] {
            assert!(info.ddl.contains(part), "{part}\n{}", info.ddl);
        }

        let view = conn.describe(&ms_object("alpha", "shared_name_view")).await.unwrap();
        assert_eq!(view.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["id", "source"]);
        assert!(view.ddl.starts_with("CREATE VIEW alpha.shared_name_view AS"), "{}", view.ddl);

        let procedure = conn.describe(&ms_object("alpha", "long_query")).await.unwrap();
        assert!(procedure.columns.is_empty() && procedure.ddl.contains("WAITFOR DELAY"), "{}", procedure.ddl);

        // Without a schema, the default one is meant.
        assert!(conn.describe(&ms_object("", "rows_0")).await.is_err(), "dbo has no rows_0");
        let err = conn.describe(&ms_object("alpha", "nope")).await.err().unwrap();
        assert!(matches!(err, DbError::NotFound(_)), "{err:?}");
    });
}

#[test]
fn mssql_describes_keys_identities_computed_columns_and_foreign_keys() {
    only_with!("mssql");
    run(async {
        let mut conn = ms_sa("tempdb").await;
        let suffix = std::process::id();
        let (parent, child) = (format!("yz_parent_{suffix}"), format!("yz_child_{suffix}"));
        query(
            &mut conn,
            &format!(
                "IF OBJECT_ID(N'dbo.{child}') IS NOT NULL DROP TABLE dbo.{child}\nGO\n\
                 IF OBJECT_ID(N'dbo.{parent}') IS NOT NULL DROP TABLE dbo.{parent}\nGO\n\
                 CREATE TABLE dbo.{parent} (a int NOT NULL, b nvarchar(10) NOT NULL, CONSTRAINT PK_{parent} PRIMARY KEY (a, b))\nGO\n\
                 CREATE TABLE dbo.{child} (\
                   id int IDENTITY(10, 5) NOT NULL PRIMARY KEY,\
                   pa int NOT NULL, pb nvarchar(10) NOT NULL,\
                   label nvarchar(32) NOT NULL CONSTRAINT DF_{child}_label DEFAULT N'none',\
                   doubled AS (pa * 2),\
                   code varchar(8) NULL, amount decimal(9,3) NULL,\
                   CONSTRAINT UQ_{child}_code UNIQUE (code),\
                   CONSTRAINT CK_{child}_label CHECK (len(label) > 0),\
                   CONSTRAINT FK_{child}_parent FOREIGN KEY (pa, pb) REFERENCES dbo.{parent} (a, b))\nGO\n\
                 CREATE NONCLUSTERED INDEX IX_{child}_label ON dbo.{child} (label DESC, pa)"
            ),
        )
        .await
        .unwrap();
        let outcome = conn.describe(&ms_object("dbo", &child)).await;
        query(&mut conn, &format!("DROP TABLE dbo.{child}\nGO\nDROP TABLE dbo.{parent}")).await.unwrap();
        let info = outcome.unwrap();

        let by_name = |n: &str| info.columns.iter().find(|c| c.name == n).unwrap();
        assert_eq!((by_name("id").primary_key, by_name("id").nullable), (Some(1), false));
        let generated: Vec<_> = info.columns.iter().filter(|c| c.generated).map(|c| c.name.as_str()).collect();
        assert_eq!(generated, ["id", "doubled"], "an identity and a computed column are the server's to fill");
        assert_eq!(by_name("label").default.as_deref(), Some("(N'none')"));
        assert_eq!((by_name("code").type_name.as_str(), by_name("code").nullable), ("varchar(8)", true));
        assert_eq!(by_name("amount").type_name, "decimal(9,3)");
        assert_eq!(info.foreign_keys.len(), 1);
        let fk = &info.foreign_keys[0];
        assert_eq!(
            (fk.name.clone(), fk.columns.clone(), fk.referenced.clone(), fk.referenced_columns.clone()),
            (
                format!("FK_{child}_parent"),
                vec!["pa".to_owned(), "pb".to_owned()],
                ms_object("dbo", &parent),
                vec!["a".to_owned(), "b".to_owned()]
            )
        );
        let index = info.indexes.iter().find(|i| i.name == format!("IX_{child}_label")).unwrap();
        assert_eq!(
            (index.columns.clone(), index.unique, index.primary),
            (vec!["label".to_owned(), "pa".to_owned()], false, false)
        );
        assert!(info.indexes.iter().any(|i| i.name == format!("UQ_{child}_code") && i.unique && !i.primary));
        let parts = [
            "[id] int IDENTITY(10, 5) NOT NULL".to_owned(),
            "[label] nvarchar(32) DEFAULT (N'none') NOT NULL".to_owned(),
            "[doubled] AS ([pa]*(2))".to_owned(),
            format!("CONSTRAINT [UQ_{child}_code] UNIQUE NONCLUSTERED ([code])"),
            format!("CONSTRAINT [FK_{child}_parent] FOREIGN KEY ([pa], [pb]) REFERENCES [dbo].[{parent}] ([a], [b])"),
            format!("CREATE NONCLUSTERED INDEX [IX_{child}_label] ON [dbo].[{child}] ([label] DESC, [pa]);"),
        ];
        for part in &parts {
            assert!(info.ddl.contains(part.as_str()), "{part}\n{}", info.ddl);
        }
        assert!(info.ddl.contains(&format!("CONSTRAINT [CK_{child}_label] CHECK")), "{}", info.ddl);
    });
}

#[test]
fn mssql_writes_values_back_in_the_columns_own_type() {
    only_with!("mssql");
    run(async {
        let mut conn = ms().await;
        query(
            &mut conn,
            "CREATE TABLE #e (id int PRIMARY KEY, n decimal(10,2), name nvarchar(20), stamp datetime2(3), b varbinary(8), flag bit, big bigint, f float)\n\
             GO\n\
             INSERT INTO #e(id, name) VALUES (1, N'a'), (2, N'b'), (3, N'c')\n\
             GO\n\
             CREATE TABLE #dup (k nvarchar(5), v nvarchar(5))\n\
             GO\n\
             INSERT INTO #dup VALUES (N'x', N'1'), (N'x', N'2')",
        )
        .await
        .unwrap();
        let table = ms_object("", "#e");
        let key = |id: i64| vec![("id".to_owned(), Cell::Int(id))];
        let set = |changes: Vec<(&str, Cell)>| changes.into_iter().map(|(c, v)| (c.to_owned(), v)).collect::<Vec<_>>();

        let edits = [
            RowEdit {
                key: key(1),
                changes: set(vec![
                    ("n", text("12.50")),
                    ("name", text("it's [quoted]")),
                    ("stamp", text("2024-02-29 12:34:56.123")),
                    ("b", Cell::Bytes(vec![0xde, 0xad].into())),
                    ("flag", Cell::Bool(true)),
                    ("big", Cell::Int(i64::MAX)),
                    ("f", Cell::Float(1.5)),
                ]),
            },
            RowEdit { key: key(2), changes: set(vec![("name", Cell::Null)]) },
        ];
        assert_eq!(conn.apply_edits(&table, &edits).await.unwrap(), 2);
        let row =
            grid(&mut conn, "SELECT n, name, stamp, CONVERT(varchar(16), b, 2), flag, big, f FROM #e WHERE id = 1")
                .await
                .remove(0);
        assert_eq!(
            row,
            ["12.50", "it's [quoted]", "2024-02-29 12:34:56.123", "DEAD", "true", "9223372036854775807", "1.5"]
        );
        assert_eq!(scalar(&mut conn, "SELECT ISNULL(name, N'<null>') FROM #e WHERE id = 2").await, "<null>");

        // One of the rows is not there: the first is not written either.
        let edits = [
            RowEdit { key: key(3), changes: set(vec![("name", text("changed"))]) },
            RowEdit { key: key(99), changes: set(vec![("name", text("nobody"))]) },
        ];
        assert!(mismatch(&query_error(conn.apply_edits(&table, &edits).await.err().unwrap()), 0));
        assert_eq!(scalar(&mut conn, "SELECT name FROM #e WHERE id = 3").await, "c");
        assert_eq!(scalar(&mut conn, "SELECT @@TRANCOUNT").await, "0", "no transaction is left open");

        // A value the column can't hold: the server says so, and nothing is written.
        let bad =
            [RowEdit { key: key(3), changes: set(vec![("name", text("a name that is far too long for twenty"))]) }];
        assert!(query_error(conn.apply_edits(&table, &bad).await.err().unwrap()).contains("truncated"));
        let bad = [RowEdit { key: key(3), changes: set(vec![("n", text("not a number"))]) }];
        assert!(query_error(conn.apply_edits(&table, &bad).await.err().unwrap()).contains("Msg 8114"));
        assert_eq!(scalar(&mut conn, "SELECT @@TRANCOUNT").await, "0");
        assert_eq!(scalar(&mut conn, "SELECT name FROM #e WHERE id = 3").await, "c");

        // Two rows under one key.
        let dup = ms_object("", "#dup");
        let two = RowEdit { key: vec![("k".into(), text("x"))], changes: set(vec![("v", text("9"))]) };
        assert!(mismatch(&query_error(conn.apply_edits(&dup, &[two]).await.err().unwrap()), 2));
        assert_eq!(grid(&mut conn, "SELECT v FROM #dup ORDER BY v").await, [["1"], ["2"]]);

        let null_key = RowEdit { key: vec![("id".into(), Cell::Null)], changes: set(vec![("name", text("x"))]) };
        assert!(matches!(conn.apply_edits(&table, &[null_key]).await.err().unwrap(), DbError::NotEditable(_)));
    });
}

#[test]
fn mssql_edits_are_counted_per_row_even_with_triggers() {
    only_with!("mssql");
    run(async {
        // DML triggers can't sit on a temp table: a table of this test's own in tempdb.
        let mut conn = ms_sa("tempdb").await;
        let (table, log) = (format!("yz_tr_{}", std::process::id()), format!("yz_tr_log_{}", std::process::id()));
        query(
            &mut conn,
            &format!(
                "IF OBJECT_ID(N'dbo.{table}') IS NOT NULL DROP TABLE dbo.{table}\nGO\n\
                 IF OBJECT_ID(N'dbo.{log}') IS NOT NULL DROP TABLE dbo.{log}\nGO\n\
                 CREATE TABLE dbo.{log} (id int)\nGO\n\
                 CREATE TABLE dbo.{table} (id int PRIMARY KEY, touched int NOT NULL DEFAULT 0)\nGO\n\
                 INSERT INTO dbo.{table}(id) VALUES (1), (2)\nGO\n\
                 CREATE TRIGGER dbo.{table}_audit ON dbo.{table} AFTER UPDATE AS BEGIN \
                   INSERT INTO dbo.{log} SELECT id FROM inserted; INSERT INTO dbo.{log} SELECT id FROM inserted; INSERT INTO dbo.{log} SELECT id FROM inserted END"
            ),
        )
        .await
        .unwrap();
        let edits: Vec<_> = (1..=2)
            .map(|id| RowEdit {
                key: vec![("id".into(), Cell::Int(id))],
                changes: vec![("touched".into(), Cell::Int(5))],
            })
            .collect();
        let outcome = conn.apply_edits(&ms_object("dbo", &table), &edits).await;
        let logged = scalar(&mut conn, &format!("SELECT count(*) FROM dbo.{log}")).await;
        let touched = scalar(&mut conn, &format!("SELECT SUM(touched) FROM dbo.{table}")).await;
        query(&mut conn, &format!("DROP TABLE dbo.{table}\nGO\nDROP TABLE dbo.{log}")).await.unwrap();
        assert_eq!(outcome.unwrap(), 2);
        assert_eq!((logged.as_str(), touched.as_str()), ("6", "10"));
    });
}

#[test]
fn mssql_reports_a_lost_connection_and_opens_a_new_one() {
    only_with!("mssql");
    run(async {
        let mut conn = ms().await;
        let spid = scalar(&mut conn, "SELECT @@SPID").await;
        let mut other = ms_sa("master").await;
        query(&mut other, &format!("KILL {spid}")).await.unwrap();
        let err = conn.execute("SELECT 1", &mut Collect::default()).await.err().unwrap();
        let message = query_error(err);
        assert!(!message.is_empty());
        assert!(conn.is_closed(), "a connection that failed like that is let go: {message}");
        assert_eq!(scalar(&mut conn, "SELECT 'back'").await, "back");
    });
}

// ---------------------------------------------------------------- performance

/// The memory the rows themselves take, from their shape: the cell array,
/// the strings behind it, the vector of each row.
fn rows_bytes(rows: &[Vec<Cell>]) -> usize {
    rows.iter()
        .map(|r| {
            std::mem::size_of::<Vec<Cell>>()
                + r.len() * std::mem::size_of::<Cell>()
                + r.iter()
                    .map(|c| match c {
                        Cell::Text(s) => s.len(),
                        Cell::Bytes(b) => b.len(),
                        _ => 0,
                    })
                    .sum::<usize>()
        })
        .sum()
}

/// The resident set of this process in KiB (`ps`), 0 if it can't be read.
fn rss_kib() -> usize {
    std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

/// Keeps rows up to `limit`, then says stop; notes when the first batch came.
struct Timed {
    started: Instant,
    limit: usize,
    first: Option<Duration>,
    rows: Vec<Vec<Cell>>,
    finished: Option<Duration>,
}

impl Timed {
    fn new(limit: usize) -> Self {
        Timed { started: Instant::now(), limit, first: None, rows: Vec::new(), finished: None }
    }
}

impl RowSink for Timed {
    fn columns(&mut self, _: Vec<Column>) {}
    fn rows(&mut self, rows: Vec<Vec<Cell>>) -> bool {
        self.first.get_or_insert_with(|| self.started.elapsed());
        self.rows.extend(rows);
        if self.rows.len() >= self.limit {
            self.finished = Some(self.started.elapsed());
            return false;
        }
        true
    }
    fn done(&mut self, _: Option<u64>) {
        self.finished.get_or_insert_with(|| self.started.elapsed());
    }
}

async fn measure(label: &str, conn: &mut Conn, sql: &str, limit: usize) {
    let before = rss_kib();
    let mut sink = Timed::new(limit);
    conn.execute(sql, &mut sink).await.unwrap();
    let total = sink.started.elapsed();
    let (first, finished) = (sink.first.unwrap(), sink.finished.unwrap_or(total));
    let (n, bytes) = (sink.rows.len(), rows_bytes(&sink.rows));
    let after = rss_kib();
    eprintln!(
        "{label}: {n} rows | first batch {:.1} ms | {n} rows after {:.0} ms ({:.0} rows/s) | execute returned after {:.0} ms | rows hold ~{:.1} MiB ({} B/row), process RSS {} -> {} MiB",
        first.as_secs_f64() * 1000.0,
        finished.as_secs_f64() * 1000.0,
        n as f64 / finished.as_secs_f64(),
        total.as_secs_f64() * 1000.0,
        bytes as f64 / 1_048_576.0,
        bytes / n.max(1),
        before / 1024,
        after / 1024,
    );
    // And the connection is as good as before.
    assert_eq!(scalar(conn, "SELECT 1").await, "1");
}

#[test]
#[ignore = "performance numbers: run with --ignored --nocapture"]
fn perf_postgres_reads_100k_of_a_million_rows_then_stops() {
    only_with!("postgres");
    run(async {
        let mut conn = pg().await;
        for round in 1..=3 {
            measure(
                &format!("postgres #{round}"),
                &mut conn,
                "SELECT g, md5(g::text), now() FROM generate_series(1, 1000000) g",
                100_000,
            )
            .await;
        }
    });
}

#[test]
#[ignore = "performance numbers: run with --ignored --nocapture"]
fn perf_sqlite_reads_a_million_rows() {
    let db = TempDb::new("");
    run(async {
        let mut conn = db.open().await;
        for round in 1..=3 {
            measure(
                &format!("sqlite #{round}"),
                &mut conn,
                "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c WHERE x < 1000000) SELECT x, 'row ' || x, x * 1.5 FROM c",
                1_000_000,
            )
            .await;
        }
    });
}

#[test]
#[ignore = "performance numbers: run with --ignored --nocapture"]
fn perf_mssql_reads_100k_rows_then_stops() {
    only_with!("mssql");
    run(async {
        let mut conn = ms().await;
        for round in 1..=3 {
            measure(
                &format!("mssql #{round}"),
                &mut conn,
                "SELECT TOP (1000000) a.object_id, a.name, SYSDATETIME() FROM sys.all_objects a CROSS JOIN sys.all_objects b",
                100_000,
            )
            .await;
        }
    });
}
