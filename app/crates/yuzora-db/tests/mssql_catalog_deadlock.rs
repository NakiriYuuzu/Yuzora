//! SQL Server may pick a read of its catalog as the victim of a deadlock with
//! someone else's DDL (Msg 1205) and tells whoever asked to run it again. A
//! person browsing a database's objects while a migration or another session
//! creates and removes tables would otherwise see that message in place of the
//! list.
//!
//! Runs when `YUZORA_DATABASE_TEST_ENGINES` names `mssql` and the fixture of
//! `tests/database/docker-compose.yml` is up. The DDL happens in a database of
//! this test's own, which it drops again: DDL in the fixture's database would
//! deadlock with the catalog reads of every other test.

use std::time::{Duration, Instant};

use yuzora_db::config::{ConnectionConfig, Engine, SslMode};
use yuzora_db::engine::{Conn, ConnectSpec, RowSink};
use yuzora_db::model::{Cell, Column, ObjectKind};
use yuzora_db::runtime;

mod common;
use common::enabled;

fn password() -> String {
    std::env::var("YUZORA_P8_DATABASE_PASSWORD").unwrap_or_else(|_| "Yuzora-P8-Only-2026!".to_owned())
}

async fn sa(database: &str) -> Conn {
    let config = ConnectionConfig {
        name: "ms".into(),
        engine: Engine::Mssql,
        host: "127.0.0.1".into(),
        port: Some(51433),
        user: "sa".into(),
        database: database.into(),
        ssl: SslMode::Prefer,
        ..ConnectionConfig::default()
    };
    Conn::connect(&ConnectSpec::new(config, Some(password()))).await.expect("connect to the SQL Server fixture")
}

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

async fn run(conn: &mut Conn, sql: &str) -> String {
    let mut first = First::default();
    conn.execute(sql, &mut first).await.unwrap_or_else(|e| panic!("{sql}: {e}"));
    first.0.unwrap_or_default()
}

/// A database of this test's own, gone when it ends.
struct Scratch(String);

impl Scratch {
    fn new() -> Self {
        let scratch = Scratch(format!("yz_catalog_deadlock_{}", std::process::id()));
        scratch.remove();
        runtime::block_on(async {
            run(&mut sa("master").await, &format!("CREATE DATABASE {}", scratch.0)).await;
        });
        scratch
    }

    fn remove(&self) {
        let n = &self.0;
        let sql = format!(
            "IF DB_ID(N'{n}') IS NOT NULL BEGIN ALTER DATABASE {n} SET SINGLE_USER WITH ROLLBACK IMMEDIATE; DROP DATABASE {n} END"
        );
        runtime::block_on(async {
            let _ = tokio::time::timeout(Duration::from_secs(30), async {
                let _ = sa("master").await.execute(&sql, &mut First::default()).await;
            })
            .await;
        });
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        self.remove();
    }
}

/// The deadlock of SQL Server's own `system_health` record, made on cue: a DDL
/// transaction has created an object (its row in the catalog is locked) and then
/// removes an older one that a catalog read has already passed. The read is the
/// cheaper to roll back, so it is the victim; the DDL is made the one that
/// cannot be. The DDL transaction ends only after that, so the second run of
/// the read has to wait for it and then sees what it left.
#[test]
fn mssql_a_catalog_read_chosen_as_a_deadlock_victim_is_run_again() {
    if !enabled("mssql") {
        eprintln!("skipped: mssql is not in YUZORA_DATABASE_TEST_ENGINES");
        return;
    }
    let scratch = Scratch::new();
    let database = scratch.0.clone();
    runtime::block_on(async {
        let mut ddl = sa(&database).await;
        let mut watcher = sa(&database).await;
        run(&mut ddl, "SET DEADLOCK_PRIORITY HIGH").await;
        run(&mut ddl, "CREATE TABLE dbo.old_one (id int)").await;
        let spid = run(&mut ddl, "SELECT CAST(@@SPID AS varchar(10))").await;
        run(&mut ddl, "BEGIN TRANSACTION").await;
        run(&mut ddl, "CREATE TABLE dbo.new_one (id int)").await;

        let reading = database.clone();
        let reader = tokio::spawn(async move { sa(&reading).await.objects().await });
        let blocked = format!(
            "SELECT CAST(COUNT(*) AS varchar(10)) FROM sys.dm_exec_requests WHERE blocking_session_id = {spid}"
        );
        let started = Instant::now();
        while run(&mut watcher, &blocked).await == "0" {
            assert!(
                started.elapsed() < Duration::from_secs(20),
                "the catalog read never waited for the DDL transaction"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        run(&mut ddl, "DROP TABLE dbo.old_one").await;
        run(&mut ddl, "COMMIT").await;

        let objects = reader.await.unwrap().expect("the catalog read that was the deadlock victim runs again");
        let names: Vec<_> = objects.iter().filter(|o| o.kind == ObjectKind::Table).map(|o| o.name.as_str()).collect();
        assert_eq!(names, ["new_one"], "the second run reads what the DDL transaction left");
    });
}
