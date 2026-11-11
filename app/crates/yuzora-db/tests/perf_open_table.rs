//! How long it takes from opening a table (or running a query) to its first
//! rows, to being editable, to the total, against the real PostgreSQL and SQL
//! Server fixtures of `tests/database/docker-compose.yml`, with a delay line
//! between the app and the server to stand in for a network.
//!
//! Every test here is `#[ignore]`; each sample prints one `PERF` line the
//! moment it is taken. Run:
//!
//! ```text
//! YUZORA_DATABASE_TEST_ENGINES=postgres,mssql \
//!   cargo test --locked -p yuzora-db --test perf_open_table -- --ignored --nocapture --test-threads=1
//! ```
//!
//! Knobs (environment): `YUZORA_PERF_ENGINES` (`postgres,mssql`), `YUZORA_PERF_DELAYS`
//! (one-way milliseconds, `0,25`), `YUZORA_PERF_TABLES` (`small,100k,1m,widecols,widetext`),
//! `YUZORA_PERF_COLD` / `YUZORA_PERF_WARM` (samples each, 5 and 7), `YUZORA_PERF_KEEP=1`
//! (keep the fixture databases `yz_speed_pg` / `yz_speed_ms` for the next run).
//!
//! The delay line sits between the app and the server and holds every chunk
//! for the one-way delay before it passes it on, so pipelined requests stay
//! pipelined and a large result is not slowed by it. It has no bandwidth
//! limit, no loss and no jitter. A zero delay still passes through the relay
//! (two thread hops per direction); the calibration test measures that.

mod common;

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use common::{ConnStat, DelayLine, ms};
use serde_json::json;
use yuzora_db::config::{self, ConnectionConfig};
use yuzora_db::model::ObjectRef;
use yuzora_db::session::{self, JobId, JobState};
use yuzora_db::{engine, results, runtime};

const PG_DB: &str = "yz_speed_pg";
const MS_DB: &str = "yz_speed_ms";

static HOME: LazyLock<PathBuf> = LazyLock::new(|| {
    let home = std::env::temp_dir().join(format!("yuzora-db-perf-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    // SAFETY: the first thing every test does, before it starts a thread of its own.
    unsafe {
        std::env::set_var("YUZORA_APP_HOME", &home);
        std::env::set_var("YUZORA_DB_SECRETS", "memory");
    }
    home
});

fn set_max_rows(n: usize) {
    LazyLock::force(&HOME);
    let mut section = yuzora_core::settings::section(config::SECTION);
    section["maxRows"] = json!(n);
    yuzora_core::settings::set_section(config::SECTION, section).unwrap();
}

fn env_list(name: &str, default: &str) -> Vec<String> {
    std::env::var(name)
        .unwrap_or_else(|_| default.to_owned())
        .split(',')
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect()
}

fn env_n(name: &str, default: usize) -> usize {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn enabled(engine: &str) -> bool {
    std::env::var("YUZORA_DATABASE_TEST_ENGINES")
        .is_ok_and(|v| v.split(',').any(|e| e.trim().eq_ignore_ascii_case(engine)))
}

fn password() -> String {
    std::env::var("YUZORA_P8_DATABASE_PASSWORD").unwrap_or_else(|_| "Yuzora-P8-Only-2026!".to_owned())
}

// ---------------------------------------------------------------- statistics

#[derive(Clone, Copy)]
struct Stat {
    median: f64,
    min: f64,
    max: f64,
}

fn stat(values: &[f64]) -> Option<Stat> {
    let mut v: Vec<f64> = values.iter().copied().filter(|x| x.is_finite()).collect();
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mid = v.len() / 2;
    let median = if v.len() % 2 == 1 { v[mid] } else { (v[mid - 1] + v[mid]) / 2.0 };
    Some(Stat { median, min: v[0], max: v[v.len() - 1] })
}

/// Least squares `y = a + n * x`.
fn fit(points: &[(f64, f64)]) -> Option<(f64, f64)> {
    if points.len() < 2 {
        return None;
    }
    let n = points.len() as f64;
    let (sx, sy) = points.iter().fold((0.0, 0.0), |(sx, sy), (x, y)| (sx + x, sy + y));
    let (sxx, sxy) = points.iter().fold((0.0, 0.0), |(sxx, sxy), (x, y)| (sxx + x * x, sxy + x * y));
    let denominator = n * sxx - sx * sx;
    if denominator.abs() < 1e-9 {
        return None;
    }
    let slope = (n * sxy - sx * sy) / denominator;
    Some(((sy - slope * sx) / n, slope))
}

fn load() -> String {
    std::process::Command::new("uptime")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_default()
}

// ---------------------------------------------------------------- session helpers

fn wait(job: JobId) -> JobState {
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(300) {
        let state = session::job_state(job).expect("job");
        if !state.running {
            return state;
        }
        std::thread::sleep(Duration::from_micros(200));
    }
    panic!("job {job} still running after 300 s");
}

fn run(connection: &str, owner: &str, sql: &str) -> JobState {
    let state = wait(session::start_query(connection, None, owner, sql));
    assert_eq!(state.error, None, "{sql}");
    state
}

fn save(config: serde_json::Value) -> String {
    LazyLock::force(&HOME);
    static TURN: Mutex<()> = Mutex::new(());
    let _turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
    config::save(serde_json::from_value::<ConnectionConfig>(config).unwrap(), Some(&password())).unwrap().id
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

    fn engine(self) -> config::Engine {
        match self {
            Kind::Postgres => config::Engine::Postgres,
            Kind::Mssql => config::Engine::Mssql,
        }
    }

    fn server_port(self) -> u16 {
        match self {
            Kind::Postgres => 55432,
            Kind::Mssql => 51433,
        }
    }

    fn database(self) -> &'static str {
        match self {
            Kind::Postgres => PG_DB,
            Kind::Mssql => MS_DB,
        }
    }

    fn table(self, name: &str) -> ObjectRef {
        let schema = if self == Kind::Postgres { "public" } else { "dbo" };
        serde_json::from_value(json!({"schema": schema, "name": name})).unwrap()
    }

    /// A saved connection to this engine's fixture database through `port`.
    fn connection(self, label: &str, port: u16, database: &str) -> String {
        let (user, engine) = match self {
            Kind::Postgres => ("yuzora_full", "postgres"),
            Kind::Mssql => ("sa", "mssql"),
        };
        save(json!({"name": format!("perf-{label}"), "engine": engine, "host": "127.0.0.1", "port": port,
                    "user": user, "database": database, "ssl": "prefer"}))
    }

    fn sleep_statement(self, seconds: f64) -> String {
        match self {
            Kind::Postgres => format!("SELECT pg_sleep({seconds})"),
            Kind::Mssql => format!("WAITFOR DELAY '00:00:{seconds:06.3}'"),
        }
    }
}

// ---------------------------------------------------------------- the fixtures

const TABLES: &[(&str, &str)] =
    &[("small", "t_small"), ("100k", "t_100k"), ("1m", "t_1m"), ("widecols", "t_widecols"), ("widetext", "t_widetext")];

fn table_name(key: &str) -> &'static str {
    TABLES.iter().find(|(k, _)| *k == key).unwrap_or_else(|| panic!("no table {key}")).1
}

fn has_database(admin: &str, kind: Kind) -> bool {
    let sql = match kind {
        Kind::Postgres => format!("SELECT count(*)::text FROM pg_database WHERE datname = '{}'", kind.database()),
        Kind::Mssql => {
            format!("SELECT CAST(count(*) AS varchar(5)) FROM sys.databases WHERE name = N'{}'", kind.database())
        }
    };
    let state = run(admin, "admin", &sql);
    let set = results::get(state.results.last().unwrap().id).unwrap();
    let r = results::lock(&set);
    r.rows[0][0].display() == "1"
}

fn has_table(connection: &str, name: &str) -> bool {
    let state = wait(session::start_query(connection, None, "probe-table", &format!("SELECT count(*) FROM {name}")));
    state.error.is_none()
}

/// The fixture database with its tables, built if it isn't there (and kept when asked to).
fn fixture(kind: Kind) -> bool {
    let (admin_db, database) = match kind {
        Kind::Postgres => ("postgres", PG_DB),
        Kind::Mssql => ("master", MS_DB),
    };
    let admin = kind.connection(&format!("{}-admin", kind.name()), kind.server_port(), admin_db);
    let keep = std::env::var("YUZORA_PERF_KEEP").is_ok_and(|v| v == "1");
    let wanted: Vec<&str> = TABLES.iter().map(|(_, t)| *t).collect();
    if has_database(&admin, kind) {
        let direct = kind.connection(&format!("{}-direct-check", kind.name()), kind.server_port(), database);
        let complete = wanted.iter().all(|t| has_table(&direct, t));
        session::disconnect(&direct);
        if keep && complete {
            eprintln!("fixture {database}: kept from an earlier run");
            session::disconnect(&admin);
            return true;
        }
        drop_database(&admin, kind);
    }
    run(&admin, "admin", &format!("CREATE DATABASE {database}"));
    let direct = kind.connection(&format!("{}-direct", kind.name()), kind.server_port(), database);
    let started = Instant::now();
    match kind {
        Kind::Postgres => fill_postgres(&direct),
        Kind::Mssql => fill_mssql(&direct),
    }
    eprintln!("fixture {database}: built in {:.1} s", started.elapsed().as_secs_f64());
    session::disconnect(&direct);
    session::disconnect(&admin);
    true
}

fn drop_database(admin: &str, kind: Kind) {
    let sql = match kind {
        Kind::Postgres => format!("DROP DATABASE IF EXISTS {} WITH (FORCE)", kind.database()),
        Kind::Mssql => format!(
            "IF DB_ID(N'{0}') IS NOT NULL BEGIN ALTER DATABASE {0} SET SINGLE_USER WITH ROLLBACK IMMEDIATE; DROP DATABASE {0} END",
            kind.database()
        ),
    };
    let state = wait(session::start_query(admin, None, "admin", &sql));
    assert_eq!(state.error, None);
}

fn teardown(kind: Kind) {
    if std::env::var("YUZORA_PERF_KEEP").is_ok_and(|v| v == "1") {
        return;
    }
    let admin_db = if kind == Kind::Postgres { "postgres" } else { "master" };
    let admin = kind.connection(&format!("{}-admin-end", kind.name()), kind.server_port(), admin_db);
    drop_database(&admin, kind);
    session::disconnect(&admin);
}

fn fill_postgres(c: &str) {
    for (name, rows) in [("t_small", 500), ("t_100k", 100_000), ("t_1m", 1_000_000)] {
        run(
            c,
            "fill",
            &format!("CREATE TABLE {name}(id int PRIMARY KEY, a int, b text, c timestamp, d numeric(10,2), e boolean)"),
        );
        run(
            c,
            "fill",
            &format!(
                "INSERT INTO {name} SELECT g, g % 1000, 'row ' || g, timestamp '2020-01-01' + g * interval '1 second', g / 100.0, g % 2 = 0 \
                 FROM generate_series(1, {rows}) g"
            ),
        );
    }
    let columns: Vec<String> = (1..=150).map(|i| format!("c{i} {}", if i % 2 == 0 { "int" } else { "text" })).collect();
    run(c, "fill", &format!("CREATE TABLE t_widecols(id int PRIMARY KEY, {})", columns.join(", ")));
    let values: Vec<String> =
        (1..=150).map(|i| if i % 2 == 0 { "g % 97".to_owned() } else { format!("'v{i}-' || g") }).collect();
    run(c, "fill", &format!("INSERT INTO t_widecols SELECT g, {} FROM generate_series(1, 10000) g", values.join(", ")));
    run(c, "fill", "CREATE TABLE t_widetext(id int PRIMARY KEY, title text, body text, n int, f float8)");
    run(
        c,
        "fill",
        "INSERT INTO t_widetext SELECT g, 'title ' || g, repeat(md5(g::text || random()::text), 128), g, random() FROM generate_series(1, 10000) g",
    );
    for (_, t) in TABLES {
        run(c, "fill", &format!("VACUUM (ANALYZE) {t}"));
    }
}

fn fill_mssql(c: &str) {
    let numbers = |count: usize, from: usize| {
        format!(
            "SELECT TOP ({count}) {from} + ROW_NUMBER() OVER (ORDER BY (SELECT NULL)) AS g FROM sys.all_columns a CROSS JOIN sys.all_columns b"
        )
    };
    for (name, rows) in [("t_small", 500), ("t_100k", 100_000), ("t_1m", 1_000_000)] {
        run(
            c,
            "fill",
            &format!(
                "CREATE TABLE dbo.{name}(id int PRIMARY KEY, a int, b nvarchar(40), c datetime2(3), d decimal(10,2), e bit)"
            ),
        );
        let mut done = 0;
        while done < rows {
            let batch = (rows - done).min(100_000);
            run(
                c,
                "fill",
                &format!(
                    "INSERT INTO dbo.{name} SELECT g, g % 1000, N'row ' + CAST(g AS nvarchar(12)), DATEADD(second, g, '20200101'), \
                     CAST(g / 100.0 AS decimal(10,2)), g % 2 FROM ({}) n",
                    numbers(batch, done)
                ),
            );
            done += batch;
        }
    }
    let columns: Vec<String> =
        (1..=150).map(|i| format!("c{i} {}", if i % 2 == 0 { "int" } else { "nvarchar(30)" })).collect();
    run(c, "fill", &format!("CREATE TABLE dbo.t_widecols(id int PRIMARY KEY, {})", columns.join(", ")));
    let values: Vec<String> = (1..=150)
        .map(|i| if i % 2 == 0 { "g % 97".to_owned() } else { format!("N'v{i}-' + CAST(g AS nvarchar(10))") })
        .collect();
    run(
        c,
        "fill",
        &format!("INSERT INTO dbo.t_widecols SELECT g, {} FROM ({}) n", values.join(", "), numbers(10_000, 0)),
    );
    run(
        c,
        "fill",
        "CREATE TABLE dbo.t_widetext(id int PRIMARY KEY, title nvarchar(40), body nvarchar(max), n int, f float)",
    );
    run(
        c,
        "fill",
        &format!(
            "INSERT INTO dbo.t_widetext SELECT g, N'title ' + CAST(g AS nvarchar(12)), \
             REPLICATE(CAST(CONVERT(nvarchar(36), NEWID()) AS nvarchar(max)), 110), g, RAND(CHECKSUM(NEWID())) FROM ({}) n",
            numbers(10_000, 0)
        ),
    );
    for (_, t) in TABLES {
        run(c, "fill", &format!("UPDATE STATISTICS dbo.{t}"));
    }
}

// ---------------------------------------------------------------- one open

#[derive(Debug, Clone, Default)]
struct Sample {
    first_rows: f64,
    complete: f64,
    editable: f64,
    total: f64,
    done: f64,
    rows: usize,
    truncated: bool,
    total_value: Option<u64>,
    error: Option<String>,
    conns: Vec<ConnStat>,
}

fn poll_until_done(job: JobId, started: Instant) -> Sample {
    let mut s = Sample {
        first_rows: f64::NAN,
        complete: f64::NAN,
        editable: f64::NAN,
        total: f64::NAN,
        done: f64::NAN,
        ..Default::default()
    };
    loop {
        let now = Instant::now();
        let Some(state) = session::job_state(job) else { panic!("job {job} is gone") };
        if let Some(first) = state.results.first() {
            let t = ms(now - started);
            if s.first_rows.is_nan() && first.rows > 0 {
                s.first_rows = t;
            }
            if s.complete.is_nan() && first.complete {
                s.complete = t;
            }
            if s.editable.is_nan() && first.editable {
                s.editable = t;
            }
            if s.total.is_nan() && first.total.is_some() {
                s.total = t;
            }
            s.rows = first.rows;
            s.truncated = first.truncated;
            s.total_value = first.total;
        }
        if !state.running {
            s.done = ms(now - started);
            s.error = state.error.clone();
            return s;
        }
        if now.duration_since(started) > Duration::from_secs(600) {
            panic!("job {job} still running after 600 s");
        }
        std::thread::sleep(Duration::from_micros(200));
    }
}

/// What the table pane does on opening: the table, and (from its own script) a request for its description
/// beside it: the job's own (`tableInfo`), which sends nothing of its own.
fn open_table(line: &DelayLine, connection: &str, owner: &str, table: &ObjectRef, with_pane_describe: bool) -> Sample {
    let started = Instant::now();
    let job = session::open_table(connection, None, owner, table, None);
    if with_pane_describe {
        runtime::spawn(async move {
            let _ = session::table_info(job).await;
        });
    }
    let mut s = poll_until_done(job, started);
    std::thread::sleep(Duration::from_millis(30));
    s.conns = line.window(started);
    s
}

fn fmt(x: f64) -> String {
    if x.is_nan() { "-".to_owned() } else { format!("{x:.1}") }
}

fn print_sample(kind: Kind, table: &str, mode: &str, delay: u64, i: usize, s: &Sample) {
    let conns: Vec<String> = s
        .conns
        .iter()
        .map(|c| {
            format!(
                "[{} {:.0}..{:.0} ms, {} exch, {}B up {}B down]",
                if c.pooled { "pooled" } else { "new" },
                c.first_ms,
                c.last_ms,
                c.exchanges,
                c.up,
                c.down
            )
        })
        .collect();
    println!(
        "PERF sample engine={} table={table} mode={mode} delay_ms={delay} i={i} first_rows={} complete={} editable={} total={} done={} rows={} truncated={} total_value={:?} err={:?} conns={} {}",
        kind.name(),
        fmt(s.first_rows),
        fmt(s.complete),
        fmt(s.editable),
        fmt(s.total),
        fmt(s.done),
        s.rows,
        s.truncated,
        s.total_value,
        s.error,
        s.conns.len(),
        conns.join(" ")
    );
}

/// (metric name, extractor)
type Metric = (&'static str, fn(&Sample) -> f64);

const METRICS: [Metric; 5] = [
    ("first_rows", |s| s.first_rows),
    ("complete", |s| s.complete),
    ("editable", |s| s.editable),
    ("total", |s| s.total),
    ("done", |s| s.done),
];

fn print_summary(kind: Kind, table: &str, mode: &str, delay: u64, samples: &[Sample]) {
    let mut line =
        format!("PERF summary engine={} table={table} mode={mode} delay_ms={delay} n={}", kind.name(), samples.len());
    for (name, get) in METRICS {
        let values: Vec<f64> = samples.iter().map(get).collect();
        if let Some(s) = stat(&values) {
            line.push_str(&format!(" {name}={:.1}[{:.1}..{:.1}]", s.median, s.min, s.max));
        }
    }
    let exch: Vec<f64> = samples.iter().map(|s| s.conns.iter().map(|c| c.exchanges).sum::<usize>() as f64).collect();
    if let Some(s) = stat(&exch) {
        line.push_str(&format!(" exchanges={:.0}", s.median));
    }
    let conns: Vec<f64> = samples.iter().map(|s| s.conns.iter().filter(|c| !c.pooled).count() as f64).collect();
    if let Some(s) = stat(&conns) {
        line.push_str(&format!(" new_connections={:.0}", s.median));
    }
    println!("{line}");
}

// ---------------------------------------------------------------- the tests

fn kinds() -> Vec<Kind> {
    env_list("YUZORA_PERF_ENGINES", "postgres,mssql")
        .into_iter()
        .filter_map(|e| match e.as_str() {
            "postgres" => Some(Kind::Postgres),
            "mssql" => Some(Kind::Mssql),
            _ => None,
        })
        .filter(|k| enabled(k.name()))
        .collect()
}

fn delays() -> Vec<u64> {
    env_list("YUZORA_PERF_DELAYS", "0,25").iter().filter_map(|d| d.parse().ok()).collect()
}

/// The delay line itself and what one statement costs in round trips, before any number is trusted.
#[test]
#[ignore = "performance numbers: run with --ignored --nocapture"]
fn perf_calibrate_the_delay_line() {
    LazyLock::force(&HOME);
    println!("\nPERF load {}", load());
    let echo = TcpListener::bind("127.0.0.1:0").unwrap();
    let echo_addr = echo.local_addr().unwrap();
    std::thread::spawn(move || {
        for mut c in echo.incoming().flatten() {
            let _ = c.set_nodelay(true);
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                while let Ok(n) = c.read(&mut buf) {
                    if n == 0 || c.write_all(&buf[..n]).is_err() {
                        break;
                    }
                }
            });
        }
    });
    let ping = |addr: SocketAddr| {
        let mut c = TcpStream::connect(addr).unwrap();
        c.set_nodelay(true).unwrap();
        let mut times = Vec::new();
        let mut byte = [0u8; 1];
        for _ in 0..30 {
            let t = Instant::now();
            c.write_all(&[7]).unwrap();
            c.read_exact(&mut byte).unwrap();
            times.push(ms(t.elapsed()));
        }
        stat(&times[5..]).unwrap()
    };
    let direct = ping(echo_addr);
    println!("PERF echo delay_ms=direct rtt_median={:.3} min={:.3} max={:.3}", direct.median, direct.min, direct.max);
    for d in [0u64, 10, 25, 50] {
        let line = DelayLine::start(echo_addr, Duration::from_millis(d));
        let s = ping(line.addr);
        println!(
            "PERF echo delay_ms={d} expected_rtt={} rtt_median={:.3} min={:.3} max={:.3} over={:.3}",
            2 * d,
            s.median,
            s.min,
            s.max,
            s.median - (2 * d) as f64
        );
    }

    for kind in kinds() {
        let admin_db = if kind == Kind::Postgres { "postgres" } else { "master" };
        {
            let id = kind.connection(&format!("calib-{}-direct", kind.name()), kind.server_port(), admin_db);
            run(&id, "calib", "SELECT 1");
            let times: Vec<f64> = (0..20)
                .map(|_| {
                    let t = Instant::now();
                    run(&id, "calib", "SELECT 1");
                    ms(t.elapsed())
                })
                .collect();
            let s = stat(&times[3..]).unwrap();
            println!(
                "PERF calibrate engine={} delay_ms=direct(no relay) warm_select1_median={:.2} min={:.2} max={:.2}",
                kind.name(),
                s.median,
                s.min,
                s.max
            );
            session::disconnect(&id);
        }
        for d in [0u64, 25] {
            let line =
                DelayLine::start(SocketAddr::from(([127, 0, 0, 1], kind.server_port())), Duration::from_millis(d));
            let id = kind.connection(&format!("calib-{}-{d}", kind.name()), line.addr.port(), admin_db);
            let owner = "calib";
            run(&id, owner, "SELECT 1");
            let mut times = Vec::new();
            let mut exchanges = 0;
            for _ in 0..15 {
                let t = Instant::now();
                run(&id, owner, "SELECT 1");
                times.push(ms(t.elapsed()));
                std::thread::sleep(Duration::from_millis(5 * d.max(1)));
                exchanges = line.window(t).iter().map(|c| c.exchanges).sum();
            }
            let s = stat(&times[3..]).unwrap();
            println!(
                "PERF calibrate engine={} delay_ms={d} warm_select1_median={:.2} min={:.2} max={:.2} exchanges_in_the_last={exchanges} (one round trip = {} ms)",
                kind.name(),
                s.median,
                s.min,
                s.max,
                2 * d
            );
            session::disconnect(&id);
        }
    }
}

/// How long the connection itself takes, and what each statement of an open costs on a warm one.
#[test]
#[ignore = "performance numbers: run with --ignored --nocapture"]
fn perf_phases_of_an_open() {
    LazyLock::force(&HOME);
    set_max_rows(100_000);
    println!("\nPERF load {}", load());
    for kind in kinds() {
        fixture(kind);
        let home = (kind == Kind::Mssql).then_some(MS_DB);
        let table = kind.table("t_100k");
        for d in delays() {
            let line =
                DelayLine::start(SocketAddr::from(([127, 0, 0, 1], kind.server_port())), Duration::from_millis(d));
            let id = kind.connection(&format!("phases-{}-{d}", kind.name()), line.addr.port(), kind.database());
            // warm the server's cache and the statistics
            run(&id, "warmup", &engine::select_sql(kind.engine(), home, &table, None, 100_001));
            session::disconnect(&id);
            let mut connect = Vec::new();
            let (mut connect_exch, mut connect_bytes) = (0, (0, 0));
            for i in 0..5 {
                session::disconnect(&id);
                let t = Instant::now();
                let lease = runtime::block_on(session::lease(&id, None, &format!("connect-{i}"))).unwrap();
                connect.push(ms(t.elapsed()));
                drop(lease);
                std::thread::sleep(Duration::from_millis(5 * d.max(1)));
                let seen = line.window(t);
                connect_exch = seen.iter().map(|c| c.exchanges).sum::<usize>();
                connect_bytes = (seen.iter().map(|c| c.up).sum(), seen.iter().map(|c| c.down).sum());
            }
            let c = stat(&connect).unwrap();
            println!(
                "PERF phase engine={} delay_ms={d} what=connect(TCP+TLS+login+version) median={:.1} min={:.1} max={:.1} exchanges={connect_exch} up={}B down={}B",
                kind.name(),
                c.median,
                c.min,
                c.max,
                connect_bytes.0,
                connect_bytes.1
            );
            session::disconnect(&id);
            runtime::block_on(session::lease(&id, None, "meta")).unwrap();
            for (what, sql) in [
                ("describe(warm META)", None),
                (
                    "SELECT 100001 rows(warm, SQL job)",
                    Some(engine::select_sql(kind.engine(), home, &table, None, 100_001)),
                ),
                ("count(*) 100k(warm, SQL job)", Some(engine::count_sql(kind.engine(), home, &table))),
                ("SELECT 1(warm, SQL job)", Some("SELECT 1".to_owned())),
            ] {
                let mut times = Vec::new();
                let mut exchanges = 0;
                for i in 0..7 {
                    let t = Instant::now();
                    match &sql {
                        None => {
                            runtime::block_on(session::describe(&id, None, &table)).unwrap();
                        }
                        Some(sql) => {
                            run(&id, "phase", sql);
                        }
                    }
                    times.push(ms(t.elapsed()));
                    if i == 6 {
                        std::thread::sleep(Duration::from_millis(5 * d.max(1)));
                        exchanges = line.window(t).iter().map(|c| c.exchanges).sum();
                    }
                }
                let s = stat(&times[2..]).unwrap();
                println!(
                    "PERF phase engine={} delay_ms={d} what={what} median={:.1} min={:.1} max={:.1} exchanges_in_the_last={exchanges}",
                    kind.name(),
                    s.median,
                    s.min,
                    s.max
                );
            }
            session::disconnect(&id);
        }
        teardown(kind);
    }
}

#[test]
#[ignore = "performance numbers: run with --ignored --nocapture"]
fn perf_open_table() {
    LazyLock::force(&HOME);
    println!("\nPERF load {}", load());
    let tables = env_list("YUZORA_PERF_TABLES", "small,100k,1m,widecols,widetext");
    let (cold_n, warm_n) = (env_n("YUZORA_PERF_COLD", 5), env_n("YUZORA_PERF_WARM", 7));
    let delays = delays();
    // `*-alone`: without the pane's own request for the description beside the table (since the job carries the
    // description it makes no difference to the traffic; before that, the pane sent a second `describe`).
    let modes = env_list("YUZORA_PERF_MODES", "cold,warm");
    for kind in kinds() {
        fixture(kind);
        let mut summaries: Vec<(String, String, u64, Vec<Sample>)> = Vec::new();
        for &d in &delays {
            let line =
                DelayLine::start(SocketAddr::from(([127, 0, 0, 1], kind.server_port())), Duration::from_millis(d));
            let id = kind.connection(&format!("open-{}-{d}", kind.name()), line.addr.port(), kind.database());
            for key in &tables {
                let name = table_name(key);
                let table = kind.table(name);
                set_max_rows(100_000);
                // the server's cache and statistics, and the first read's hint bits, are not what is measured
                for _ in 0..2 {
                    open_table(&line, &id, "warmup", &table, false);
                }
                session::disconnect(&id);
                for mode in &modes {
                    let cold = mode.starts_with("cold");
                    let pane_describe = !mode.ends_with("alone");
                    let n = if cold { cold_n } else { warm_n };
                    let owner = format!("pane-{name}-{mode}");
                    let mut samples = Vec::new();
                    if !cold {
                        open_table(&line, &id, &owner, &table, pane_describe);
                    }
                    for i in 0..n {
                        if cold {
                            session::disconnect(&id);
                        }
                        let s = open_table(&line, &id, &owner, &table, pane_describe);
                        print_sample(kind, key, mode, d, i, &s);
                        samples.push(s);
                    }
                    print_summary(kind, key, mode, d, &samples);
                    summaries.push((key.clone(), mode.clone(), d, samples));
                    session::release_owner(&owner);
                }
                if key == "1m" {
                    // the cap raised to the whole table: how long reading everything takes
                    set_max_rows(1_100_000);
                    let owner = "pane-1m-all".to_owned();
                    let mut samples = Vec::new();
                    open_table(&line, &id, &owner, &table, true);
                    for i in 0..3 {
                        let s = open_table(&line, &id, &owner, &table, true);
                        print_sample(kind, "1m-all", "warm", d, i, &s);
                        samples.push(s);
                    }
                    print_summary(kind, "1m-all", "warm", d, &samples);
                    session::release_owner(&owner);
                    set_max_rows(100_000);
                }
            }
            session::disconnect(&id);
        }
        // T = a + n * RTT over the delays measured, per table and mode
        for key in &tables {
            for mode in &modes {
                for (metric, get) in METRICS {
                    let points: Vec<(f64, f64)> = summaries
                        .iter()
                        .filter(|(k, m, _, _)| k == key && m == mode)
                        .filter_map(|(_, _, d, samples)| {
                            let values: Vec<f64> = samples.iter().map(get).collect();
                            stat(&values).map(|s| ((2 * d) as f64, s.median))
                        })
                        .collect();
                    if let Some((a, n)) = fit(&points) {
                        println!(
                            "PERF fit engine={} table={key} mode={mode} metric={metric} points={} a_ms={a:.1} round_trips={n:.2}",
                            kind.name(),
                            points.len()
                        );
                    }
                }
            }
        }
        teardown(kind);
    }
    println!("\nPERF load {}", load());
}

/// Running the person's own SQL: what the first rows cost, and how quickly a Stop is answered.
#[test]
#[ignore = "performance numbers: run with --ignored --nocapture"]
fn perf_sql_tab() {
    LazyLock::force(&HOME);
    set_max_rows(100_000);
    println!("\nPERF load {}", load());
    let (warm_n, delays) = (env_n("YUZORA_PERF_WARM", 5), delays());
    for kind in kinds() {
        fixture(kind);
        let t = |name: &str| match kind {
            Kind::Postgres => format!("public.{name}"),
            Kind::Mssql => format!("dbo.{name}"),
        };
        let slow_first = kind.sleep_statement(2.0);
        let first_then_slow = format!("SELECT 1; {}", kind.sleep_statement(8.0));
        for &d in &delays {
            let line =
                DelayLine::start(SocketAddr::from(([127, 0, 0, 1], kind.server_port())), Duration::from_millis(d));
            let id = kind.connection(&format!("sql-{}-{d}", kind.name()), line.addr.port(), kind.database());
            let cases: Vec<(&str, String)> = vec![
                ("small result", "SELECT 1 AS x".to_owned()),
                ("100k rows", format!("SELECT * FROM {}", t("t_100k"))),
                ("1m-row table, capped at 100k", format!("SELECT * FROM {}", t("t_1m"))),
                ("wide table, 10k rows x 150 columns", format!("SELECT * FROM {}", t("t_widecols"))),
            ];
            let owner = "sql-pane";
            run(&id, owner, "SELECT 1");
            for (label, sql) in &cases {
                run(&id, owner, sql);
                let mut samples = Vec::new();
                for i in 0..warm_n {
                    let started = Instant::now();
                    let job = session::start_query(&id, None, owner, sql);
                    let s = poll_until_done(job, started);
                    print_sample(kind, label, "sql-warm", d, i, &s);
                    samples.push(s);
                }
                print_summary(kind, label, "sql-warm", d, &samples);
            }
            // a slow first row, and a first batch then a long wait: Stop after 500 ms
            for (label, sql, stop_after) in [
                ("slow first row (2 s), stop at 0.5 s", &slow_first, 0.5),
                ("first row then 8 s wait, stop at 0.5 s", &first_then_slow, 0.5),
            ] {
                let mut rows_at = Vec::new();
                let (mut stopped, mut next_ok) = (Vec::new(), Vec::new());
                for i in 0..warm_n {
                    let started = Instant::now();
                    let job = session::start_query(&id, None, owner, sql);
                    let mut first = f64::NAN;
                    while started.elapsed() < Duration::from_secs_f64(stop_after) {
                        if let Some(state) = session::job_state(job)
                            && first.is_nan()
                            && state.results.first().is_some_and(|r| r.rows > 0)
                        {
                            first = ms(started.elapsed());
                        }
                        std::thread::sleep(Duration::from_micros(200));
                    }
                    let pressed = Instant::now();
                    session::cancel(job);
                    let state = loop {
                        let state = session::job_state(job).unwrap();
                        if !state.running {
                            break state;
                        }
                        std::thread::sleep(Duration::from_micros(200));
                    };
                    let stopped_ms = ms(pressed.elapsed());
                    let again = Instant::now();
                    let next = wait(session::start_query(&id, None, owner, "SELECT 1"));
                    let next_ms = ms(again.elapsed());
                    println!(
                        "PERF stop engine={} case=\"{label}\" delay_ms={d} i={i} first_rows_at={} stopped_after={stopped_ms:.1} cancelled={} next_statement_ok={} next_statement_took={next_ms:.1}",
                        kind.name(),
                        fmt(first),
                        state.cancelled,
                        next.error.is_none()
                    );
                    rows_at.push(first);
                    stopped.push(stopped_ms);
                    next_ok.push(next_ms);
                }
                let (a, b) = (stat(&stopped).unwrap(), stat(&next_ok).unwrap());
                let first = stat(&rows_at);
                println!(
                    "PERF stop summary engine={} case=\"{label}\" delay_ms={d} first_rows_at_median={} stopped_after_median={:.1}[{:.1}..{:.1}] next_statement_median={:.1}[{:.1}..{:.1}]",
                    kind.name(),
                    first.map_or("-".to_owned(), |s| format!("{:.1}", s.median)),
                    a.median,
                    a.min,
                    a.max,
                    b.median,
                    b.min,
                    b.max
                );
            }
            // the same slow first row, left to finish: when its row shows
            {
                let mut times = Vec::new();
                for i in 0..3 {
                    let started = Instant::now();
                    let job = session::start_query(&id, None, owner, &format!("{}; SELECT 1", slow_first));
                    let s = poll_until_done(job, started);
                    print_sample(kind, "2 s wait then one row", "sql-warm", d, i, &s);
                    times.push(s);
                }
                print_summary(kind, "2 s wait then one row", "sql-warm", d, &times);
            }
            session::disconnect(&id);
        }
        teardown(kind);
    }
}

/// What the relay itself costs on a big result, so its numbers can be read against a direct connection.
#[test]
#[ignore = "performance numbers: run with --ignored --nocapture"]
fn perf_relay_overhead_on_a_big_result() {
    LazyLock::force(&HOME);
    set_max_rows(100_000);
    println!("\nPERF load {}", load());
    for kind in kinds() {
        fixture(kind);
        let home = (kind == Kind::Mssql).then_some(MS_DB);
        let sql = engine::select_sql(kind.engine(), home, &kind.table("t_100k"), None, 100_001);
        let line = DelayLine::start(SocketAddr::from(([127, 0, 0, 1], kind.server_port())), Duration::ZERO);
        for (label, port) in [("direct", kind.server_port()), ("relay-0", line.addr.port())] {
            let id = kind.connection(&format!("overhead-{}-{label}", kind.name()), port, kind.database());
            run(&id, "o", &sql);
            let times: Vec<f64> = (0..9)
                .map(|_| {
                    let started = Instant::now();
                    run(&id, "o", &sql);
                    ms(started.elapsed())
                })
                .collect();
            let s = stat(&times).unwrap();
            println!(
                "PERF overhead engine={} via={label} select_100001_rows median={:.1} min={:.1} max={:.1}",
                kind.name(),
                s.median,
                s.min,
                s.max
            );
            session::disconnect(&id);
        }
    }
}
