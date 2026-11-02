//! The session layer end to end on SQLite, the way the panes drive it: a saved
//! connection, a script, a table read with its count and key, cells edited and
//! written back in one transaction (and rolled back when a row is gone), the
//! row cap, a cancelled query (also one stopped before its connection was
//! there), a read-only connection, and releasing a pane. With the SQL Server
//! fixture, also a table pane whose SQL tab `USE`d another database.
//!
//! One test: the app home is process-wide (`YUZORA_APP_HOME`).

use std::path::PathBuf;
use std::time::Duration;

use serde_json::json;
use yuzora_db::config::{self, ConnectionConfig};
use yuzora_db::model::{Cell, ObjectRef};
use yuzora_db::session::{self, JobId, JobState};
use yuzora_db::{results, runtime};

fn wait(job: JobId) -> JobState {
    let mut seen = 0;
    for _ in 0..400 {
        let state = runtime::block_on(session::job_after(job, seen)).expect("job");
        if !state.running {
            return state;
        }
        seen = state.version;
    }
    panic!("job {job} still running");
}

fn run(connection: &str, owner: &str, sql: &str) -> JobState {
    let state = wait(session::start_query(connection, None, owner, sql));
    assert_eq!(state.error, None, "{sql}");
    state
}

/// The rows of a query's last result, as text.
fn rows(connection: &str, sql: &str) -> Vec<Vec<String>> {
    let state = run(connection, "check", sql);
    let set = results::get(state.results.last().expect("a result").id).expect("result");
    let r = results::lock(&set);
    r.rows.iter().map(|row| row.iter().map(Cell::display).map(|s| s.to_string()).collect()).collect()
}

fn table(name: &str) -> ObjectRef {
    serde_json::from_value(json!({"schema": "", "name": name})).unwrap()
}

#[test]
fn a_pane_reads_edits_writes_back_caps_and_cancels() {
    let home = std::env::temp_dir().join(format!("yuzora-db-session-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    // SAFETY: the only test in this binary, before any thread reads them.
    unsafe {
        std::env::set_var("YUZORA_APP_HOME", &home);
        std::env::set_var("YUZORA_DB_SECRETS", "memory");
    }
    let file: PathBuf = home.join("app.sqlite");
    std::fs::File::create(&file).unwrap();
    let saved = config::save(
        serde_json::from_value::<ConnectionConfig>(json!({"name": "lite", "engine": "sqlite", "path": file})).unwrap(),
        None,
    )
    .unwrap();
    let id = saved.id.as_str();

    run(
        id,
        "setup",
        "create table people(id integer primary key, name text not null, note text);\n\
         insert into people(name, note) values ('ada', null), ('linus', 'x');\n\
         create table log(line text);",
    );

    // A table: its rows, its count, and editable because it has a key.
    let state = wait(session::open_table(id, None, "pane", &table("people"), None));
    assert_eq!(state.error, None);
    let summary = &state.results[0];
    assert_eq!((summary.rows, summary.total, summary.editable), (2, Some(2), true), "{summary:?}");
    let set = results::get(summary.id).unwrap();
    let name = results::lock(&set).columns.iter().position(|c| c.name == "name").unwrap();
    let note = results::lock(&set).columns.iter().position(|c| c.name == "note").unwrap();
    {
        let mut r = results::lock(&set);
        r.set_edit(0, name, Cell::Text("grace".into()));
        r.set_edit(1, note, Cell::Null);
    }
    assert_eq!(runtime::block_on(session::write_back(summary.id)).unwrap(), 2);
    {
        let r = results::lock(&set);
        assert!(r.edits.is_empty() && r.write_error.is_none());
        assert_eq!(r.cell(0, name), Some(&Cell::Text("grace".into())), "written values become the result's");
    }
    assert_eq!(rows(id, "select name, note from people order by id"), [["grace", "NULL"], ["linus", "NULL"]]);

    // A row gone under the edit: nothing of the transaction stays.
    let state = wait(session::open_table(id, None, "pane", &table("people"), None));
    let set = results::get(state.results[0].id).unwrap();
    {
        let mut r = results::lock(&set);
        r.set_edit(0, name, Cell::Text("first".into()));
        r.set_edit(1, name, Cell::Text("second".into()));
    }
    run(id, "elsewhere", "delete from people where id = 2");
    assert!(runtime::block_on(session::write_back(state.results[0].id)).is_err());
    assert!(results::lock(&set).write_error.is_some());
    assert_eq!(rows(id, "select name from people"), [["grace"]], "rolled back");

    // No key, no editing, and the reason is given.
    let state = wait(session::open_table(id, None, "pane", &table("log"), None));
    assert!(!state.results[0].editable && state.results[0].not_editable.is_some(), "{:?}", state.results[0]);

    // The row cap: exactly the cap is complete, one more is truncated.
    let mut section = yuzora_core::settings::section(config::SECTION);
    section["maxRows"] = json!(1000);
    yuzora_core::settings::set_section(config::SECTION, section).unwrap();
    let count = |n: u32| format!("with recursive c(x) as (select 1 union all select x + 1 from c where x < {n}) select x from c");
    let exact = run(id, "pane", &count(1000));
    assert_eq!((exact.results[0].rows, exact.results[0].truncated), (1000, false));
    let over = run(id, "pane", &count(5000));
    assert_eq!((over.results[0].rows, over.results[0].truncated), (1000, true));

    // A query that never ends is cancelled, and the pane's connection still works.
    let job = session::start_query(id, None, "pane", "with recursive c(x) as (select 1 union all select x + 1 from c) select count(*) from c");
    std::thread::sleep(Duration::from_millis(300));
    session::cancel(job);
    let state = wait(job);
    assert!(state.cancelled && state.error.is_none(), "{state:?}");
    assert_eq!(rows(id, "select 1 + 1"), [["2"]]);

    // Releasing the pane drops its results.
    let kept = state.results.first().map(|r| r.id);
    session::release_owner("pane");
    if let Some(r) = kept {
        assert!(results::get(r).is_none());
    }

    // History keeps the statements, newest first.
    let history = yuzora_db::history::list(id);
    assert_eq!(history.first().map(|h| h.sql.as_str()), Some("select 1 + 1"));

    // Stop pressed while the pane still waits for its connection: the
    // statement never runs, not even one that returns no rows.
    let held = runtime::block_on(session::lease(id, None, "pane")).unwrap();
    let job = session::start_query(id, None, "pane", "update people set note = 'too late'");
    std::thread::sleep(Duration::from_millis(100));
    session::cancel(job);
    drop(held);
    let stopped = wait(job);
    assert!(stopped.cancelled && stopped.error.is_none(), "{stopped:?}");
    assert_eq!(rows(id, "select note from people"), [["NULL"]]);

    // A read-only connection offers no editing, and says why.
    let ro = config::save(
        serde_json::from_value::<ConnectionConfig>(json!({"name": "lite-ro", "engine": "sqlite", "path": file, "readOnly": true}))
            .unwrap(),
        None,
    )
    .unwrap();
    let state = wait(session::open_table(&ro.id, None, "ro", &table("people"), None));
    assert!(!state.results[0].editable, "{:?}", state.results[0]);
    assert_eq!(state.results[0].not_editable, Some(yuzora_db::i18n::t("edit.read_only", &[])));

    // A generated column is shown but never edited: an edit there would make
    // the whole write-back fail, the other cells with it.
    run(id, "setup", "create table sums(id integer primary key, a integer, twice integer as (a * 2));\ninsert into sums(a) values (4);");
    let state = wait(session::open_table(id, None, "pane", &table("sums"), None));
    let summary = &state.results[0];
    assert!(summary.editable, "{summary:?}");
    let set = results::get(summary.id).unwrap();
    let (a, twice) = {
        let r = results::lock(&set);
        let at = |n: &str| r.columns.iter().position(|c| c.name == n).unwrap();
        (at("a"), at("twice"))
    };
    {
        let mut r = results::lock(&set);
        assert!(!r.column_editable(twice) && r.column_editable(a));
        r.set_edit(0, twice, Cell::Int(99));
        r.set_edit(0, a, Cell::Int(5));
        assert_eq!(r.edits.len(), 1, "{:?}", r.edits);
    }
    assert_eq!(runtime::block_on(session::write_back(summary.id)).unwrap(), 1);
    assert_eq!(rows(id, "select a, twice from sums"), [["5", "10"]]);

    // Write back pressed twice before the pane repaints: one write, the
    // second refused (a trigger counts the updates).
    run(id, "setup", "create table hits(n integer);\ncreate trigger counted after update on sums begin insert into hits values (1); end;");
    let state = wait(session::open_table(id, None, "pane", &table("sums"), None));
    let twice_pressed = state.results[0].id;
    results::lock(&results::get(twice_pressed).unwrap()).set_edit(0, a, Cell::Int(6));
    let (first, second) = runtime::block_on(futures_util::future::join(
        session::write_back(twice_pressed),
        session::write_back(twice_pressed),
    ));
    assert_eq!(first, Ok(1));
    assert!(second.is_err(), "{second:?}");
    assert_eq!(rows(id, "select count(*) from hits"), [["1"]]);

    sql_server_tables_stay_in_the_panes_database();

    let _ = std::fs::remove_dir_all(&home);
}

/// With the fixture (`YUZORA_DATABASE_TEST_ENGINES` naming mssql): the table
/// pane's SQL tab `USE`s another database on the pane's connection, and the
/// table it shows, counts and writes back stays the one it opened.
fn sql_server_tables_stay_in_the_panes_database() {
    if !std::env::var("YUZORA_DATABASE_TEST_ENGINES").is_ok_and(|v| v.split(',').any(|e| e.trim() == "mssql")) {
        eprintln!("skipped: mssql is not in YUZORA_DATABASE_TEST_ENGINES");
        return;
    }
    let password = std::env::var("YUZORA_P8_DATABASE_PASSWORD").unwrap_or_else(|_| "Yuzora-P8-Only-2026!".to_owned());
    let saved = config::save(
        serde_json::from_value::<ConnectionConfig>(json!({
            "name": "ms", "engine": "mssql", "host": "127.0.0.1", "port": 51433,
            "user": "sa", "database": "yuzora_p8", "ssl": "prefer"
        }))
        .unwrap(),
        Some(&password),
    )
    .unwrap();
    let ms = saved.id.as_str();
    let name = format!("yz_pane_ctx_{}", std::process::id());
    let remove_both = format!(
        "IF OBJECT_ID(N'yuzora_p8.dbo.{name}') IS NOT NULL DROP TABLE yuzora_p8.dbo.{name};\n\
         IF OBJECT_ID(N'tempdb.dbo.{name}') IS NOT NULL DROP TABLE tempdb.dbo.{name};"
    );
    run(ms, "setup", &remove_both);
    run(
        ms,
        "setup",
        &format!(
            "CREATE TABLE yuzora_p8.dbo.{name} (id int PRIMARY KEY, v nvarchar(10));\n\
             INSERT INTO yuzora_p8.dbo.{name} VALUES (1, N'home');\n\
             CREATE TABLE tempdb.dbo.{name} (id int PRIMARY KEY, v nvarchar(10));\n\
             INSERT INTO tempdb.dbo.{name} VALUES (1, N'other');"
        ),
    );

    run(ms, "ms-pane", "USE tempdb");
    let t: ObjectRef = serde_json::from_value(json!({"schema": "dbo", "name": name})).unwrap();
    let state = wait(session::open_table(ms, None, "ms-pane", &t, None));
    assert_eq!(state.error, None);
    let summary = &state.results[0];
    assert_eq!((summary.rows, summary.total, summary.editable), (1, Some(1), true), "{summary:?}");
    let set = results::get(summary.id).unwrap();
    let v = results::lock(&set).columns.iter().position(|c| c.name == "v").unwrap();
    assert_eq!(results::lock(&set).cell(0, v), Some(&Cell::Text("home".into())));
    results::lock(&set).set_edit(0, v, Cell::Text("edited".into()));
    assert_eq!(runtime::block_on(session::write_back(summary.id)).unwrap(), 1);
    assert_eq!(rows(ms, &format!("select v from yuzora_p8.dbo.{name}")), [["edited"]]);
    assert_eq!(rows(ms, &format!("select v from tempdb.dbo.{name}")), [["other"]]);

    session::release_owner("ms-pane");
    run(ms, "setup", &remove_both);
}
