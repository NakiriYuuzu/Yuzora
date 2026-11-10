//! What opening a table costs in round trips, counted through a delay line
//! (`common`) that sits between the app and the PostgreSQL and SQL Server
//! fixtures of `tests/database/docker-compose.yml`.
//!
//! A round trip is what the line sees: a flight of server data that follows a
//! flight of client data, so these tests do not depend on how fast the machine
//! is. They run when `YUZORA_DATABASE_TEST_ENGINES` names the engine (and
//! fail, rather than skip, for one `YUZORA_DATABASE_REQUIRE_ENGINES` names but
//! the selection leaves out; `YUZORA_P8_DATABASE_PASSWORD` overrides the
//! fixture's password), make a database of their own and drop it again, and
//! take turns: what they time must not share the runtime with each other.

mod common;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use common::{ConnStat, DelayLine, enabled, ms};
use serde_json::json;
use yuzora_db::config::{self, ConnectionConfig};
use yuzora_db::model::ObjectRef;
use yuzora_db::runtime;
use yuzora_db::session::{self, JobId, JobState};

/// Rows a table read keeps (`db.maxRows`). `t_over` holds more, `t_small` fewer.
const CAP: usize = 100;
const SMALL_ROWS: usize = 80;
const OVER_ROWS: usize = 150;
/// One way. A round trip is twice this.
const ONE_WAY: Duration = Duration::from_millis(15);
const ROUND_TRIP_MS: f64 = 30.0;

static HOME: LazyLock<PathBuf> = LazyLock::new(|| {
    let home = std::env::temp_dir().join(format!("yuzora-db-open-rounds-{}", std::process::id()));
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

fn password() -> String {
    std::env::var("YUZORA_P8_DATABASE_PASSWORD").unwrap_or_else(|_| "Yuzora-P8-Only-2026!".to_owned())
}

/// What is timed here must not share the runtime or the machine with another test.
fn one_at_a_time() -> MutexGuard<'static, ()> {
    static TURN: Mutex<()> = Mutex::new(());
    LazyLock::force(&HOME);
    TURN.lock().unwrap_or_else(|e| e.into_inner())
}

fn save(config: serde_json::Value) -> String {
    config::save(serde_json::from_value::<ConnectionConfig>(config).unwrap(), Some(&password())).unwrap().id
}

fn wait(job: JobId) -> JobState {
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(120) {
        let state = session::job_state(job).expect("job");
        if !state.running {
            return state;
        }
        std::thread::sleep(Duration::from_micros(200));
    }
    panic!("job {job} still running after 120 s");
}

fn run(connection: &str, sql: &str) -> JobState {
    let state = wait(session::start_query(connection, None, &format!("setup@{connection}"), sql));
    assert_eq!(state.error, None, "{sql}");
    state
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Postgres,
    Mssql,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Postgres => "postgres",
            Kind::Mssql => "mssql",
        }
    }

    fn server_port(self) -> u16 {
        match self {
            Kind::Postgres => 55432,
            Kind::Mssql => 51433,
        }
    }

    fn table(self, name: &str) -> ObjectRef {
        let schema = if self == Kind::Postgres { "public" } else { "dbo" };
        serde_json::from_value(json!({"schema": schema, "name": name})).unwrap()
    }

    /// A saved connection to this engine's server at `port`, in `database`.
    fn connection(self, label: &str, port: u16, database: &str) -> String {
        let (user, engine) = match self {
            Kind::Postgres => ("yuzora_full", "postgres"),
            Kind::Mssql => ("sa", "mssql"),
        };
        save(json!({"name": format!("rounds-{label}"), "engine": engine, "host": "127.0.0.1", "port": port,
                    "user": user, "database": database, "ssl": "prefer"}))
    }
}

/// The engines this run covers.
fn kinds() -> Vec<Kind> {
    [Kind::Postgres, Kind::Mssql].into_iter().filter(|k| enabled(k.name())).collect()
}

/// A database of this test's own, with a small table and one over the row cap
/// (rows padded, so the pane's connection is the one that carries the bytes),
/// dropped when the test ends.
struct Fixture {
    kind: Kind,
    database: String,
    admin: String,
}

impl Fixture {
    fn new(kind: Kind, tag: &str) -> Self {
        LazyLock::force(&HOME);
        let (admin_db, prefix) = match kind {
            Kind::Postgres => ("postgres", "yz_open_pg"),
            Kind::Mssql => ("master", "yz_open_ms"),
        };
        let database = format!("{prefix}_{tag}");
        let admin = kind.connection(&format!("{}-admin-{tag}", kind.name()), kind.server_port(), admin_db);
        let fixture = Fixture { kind, database, admin };
        fixture.drop_it();
        run(&fixture.admin, &format!("CREATE DATABASE {}", fixture.database));
        let direct = fixture.connect(kind.server_port(), "direct");
        match kind {
            Kind::Postgres => {
                for (name, rows) in [("t_small", SMALL_ROWS), ("t_over", OVER_ROWS)] {
                    run(&direct, &format!("CREATE TABLE {name}(id int PRIMARY KEY, pad text)"));
                    run(
                        &direct,
                        &format!("INSERT INTO {name} SELECT g, repeat('x', 400) FROM generate_series(1, {rows}) g"),
                    );
                }
            }
            Kind::Mssql => {
                for (name, rows) in [("t_small", SMALL_ROWS), ("t_over", OVER_ROWS)] {
                    run(&direct, &format!("CREATE TABLE dbo.{name}(id int PRIMARY KEY, pad varchar(500))"));
                    run(
                        &direct,
                        &format!(
                            "INSERT INTO dbo.{name} SELECT TOP ({rows}) ROW_NUMBER() OVER (ORDER BY (SELECT NULL)), REPLICATE('x', 400) FROM sys.all_columns"
                        ),
                    );
                }
            }
        }
        session::disconnect(&direct);
        fixture
    }

    /// A saved connection to the fixture's database through `port`.
    fn connect(&self, port: u16, label: &str) -> String {
        self.kind.connection(&format!("{}-{label}", self.database), port, &self.database)
    }

    /// Through a delay line of its own.
    fn through_the_line(&self) -> (DelayLine, String) {
        let line = DelayLine::start(SocketAddr::from(([127, 0, 0, 1], self.kind.server_port())), ONE_WAY);
        let id = self.connect(line.addr.port(), "line");
        (line, id)
    }

    fn drop_it(&self) {
        let sql = match self.kind {
            Kind::Postgres => format!("DROP DATABASE IF EXISTS {} WITH (FORCE)", self.database),
            Kind::Mssql => format!(
                "IF DB_ID(N'{0}') IS NOT NULL BEGIN ALTER DATABASE {0} SET SINGLE_USER WITH ROLLBACK IMMEDIATE; DROP DATABASE {0} END",
                self.database
            ),
        };
        let job = session::start_query(&self.admin, None, &format!("drop@{}", self.admin), &sql);
        let started = Instant::now();
        while session::job_state(job).is_some_and(|s| s.running) && started.elapsed() < Duration::from_secs(60) {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.drop_it();
    }
}

/// One open of a table, from the moment the job started.
struct Opened {
    job: JobId,
    first_rows_ms: f64,
    done_ms: f64,
    rows: usize,
    truncated: bool,
    total: Option<u64>,
    editable: bool,
    error: Option<String>,
    conns: Vec<ConnStat>,
}

impl Opened {
    /// Connections this open made (the ones already in the pool are not).
    fn made(&self) -> usize {
        self.conns.iter().filter(|c| !c.pooled).count()
    }

    /// The connection the table's rows came over: the only one that carries many bytes.
    fn pane(&self) -> &ConnStat {
        self.conns.iter().filter(|c| c.down >= 20_000).max_by_key(|c| c.down).expect("the rows came over a connection")
    }
}

fn open(line: &DelayLine, connection: &str, owner: &str, table: &ObjectRef) -> Opened {
    let started = Instant::now();
    let job = session::open_table(connection, None, owner, table, None);
    let mut opened = Opened {
        job,
        first_rows_ms: f64::NAN,
        done_ms: f64::NAN,
        rows: 0,
        truncated: false,
        total: None,
        editable: false,
        error: None,
        conns: Vec::new(),
    };
    loop {
        let now = Instant::now();
        let state = session::job_state(job).expect("job");
        if let Some(first) = state.results.first() {
            if opened.first_rows_ms.is_nan() && first.rows > 0 {
                opened.first_rows_ms = ms(now - started);
            }
            (opened.rows, opened.truncated, opened.total, opened.editable) =
                (first.rows, first.truncated, first.total, first.editable);
        }
        if !state.running {
            opened.done_ms = ms(now - started);
            opened.error = state.error;
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(120), "table still loading after 120 s");
        std::thread::sleep(Duration::from_micros(200));
    }
    std::thread::sleep(Duration::from_millis(40));
    opened.conns = line.window(started);
    opened
}

// ---------------------------------------------------------------- the rows do not wait for the description

/// The rows are asked for at once: not after the table's description (columns,
/// key, indexes, DDL) has come back, which takes four to six round trips on a
/// connection of its own. The connection the rows come over starts within two
/// round trips of the open, whether it was in the pool or had to be made.
#[test]
fn the_rows_are_asked_for_before_the_description_is_back() {
    let _turn = one_at_a_time();
    for kind in kinds() {
        let fixture = Fixture::new(kind, "first");
        let (line, id) = fixture.through_the_line();
        let t = kind.table("t_small");
        let owner = format!("first-pane@{id}");

        session::disconnect(&id);
        let cold = open(&line, &id, &owner, &t);
        assert_eq!(cold.error, None, "{}", kind.name());
        assert!(
            cold.pane().first_ms < 2.0 * ROUND_TRIP_MS,
            "{}: cold, the rows' connection was first used at {:.0} ms; it waited for something",
            kind.name(),
            cold.pane().first_ms
        );
        assert_eq!(
            cold.made(),
            3,
            "{}: cold, an open makes the sidebar's, the table's and the count's connection and no other",
            kind.name()
        );

        let warm = open(&line, &id, &owner, &t);
        assert_eq!(warm.error, None, "{}", kind.name());
        assert!(
            warm.editable && warm.rows == SMALL_ROWS,
            "{}: {} rows, editable {}",
            kind.name(),
            warm.rows,
            warm.editable
        );
        assert!(
            warm.pane().first_ms < 2.0 * ROUND_TRIP_MS,
            "{}: warm, the rows' connection was first used at {:.0} ms; it waited for the description",
            kind.name(),
            warm.pane().first_ms
        );
        assert_eq!(warm.made(), 1, "{}: warm, only the count makes a connection", kind.name());

        // connections that were closed meanwhile are still saying goodbye
        std::thread::sleep(Duration::from_millis(300));
        let asked = Instant::now();
        let described = runtime::block_on(session::table_info(warm.job)).expect("the open's description");
        assert_eq!(described.key_columns(), ["id"], "{}", kind.name());
        std::thread::sleep(Duration::from_millis(40));
        assert!(
            line.window(asked).is_empty(),
            "{}: asking the job for the description went to the server: {:?}",
            kind.name(),
            line.window(asked)
        );
        session::release_owner(&owner);
        session::disconnect(&id);
    }
}
