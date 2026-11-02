//! Live connections and the queries running on them.
//!
//! A connection is opened per (saved connection, database, owner): the
//! sidebar reads the schema on its own (`owner = "meta"`) so a long query in
//! a pane never holds it up, and each pane runs its queries on its own
//! connection, which a cancel may have to end (SQL Server).
//!
//! A query is a [`Job`]: started at once, watched by version (`job(id, seen)`
//! resolves on the next change), producing one [`results`] entry per statement
//! that returns rows (or changes some).

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::config::{self, ConnectionConfig};
use crate::engine::{self, Canceller, Conn, ConnectSpec, RowSink};
use crate::error::{DbError, DbResult};
use crate::model::{Cell, Column, DbObject, ObjectRef, RowEdit, TableInfo};
use crate::results::{self, EditTarget, ResultId, ResultSet};
use crate::runtime;

/// The sidebar's connections (schema, databases).
pub const META: &str = "meta";

/// Rows kept per result before the query is stopped (`db.maxRows` overrides).
pub const DEFAULT_MAX_ROWS: usize = 100_000;

pub fn max_rows() -> usize {
    yuzora_core::settings::section(config::SECTION)
        .get("maxRows")
        .and_then(serde_json::Value::as_u64)
        .map(|n| (n as usize).clamp(100, 5_000_000))
        .unwrap_or(DEFAULT_MAX_ROWS)
}

// ---------------------------------------------------------------- status

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    #[default]
    Idle,
    Connecting,
    Connected,
    Error,
}

/// What the sidebar shows beside a connection.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub state: State,
    /// `PostgreSQL 16.4`, once connected.
    pub version: String,
    pub error: Option<String>,
}

static STATUS: LazyLock<Mutex<HashMap<String, Status>>> = LazyLock::new(Default::default);

fn statuses() -> MutexGuard<'static, HashMap<String, Status>> {
    STATUS.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn status(connection: &str) -> Status {
    statuses().get(connection).cloned().unwrap_or_default()
}

fn set_status(connection: &str, update: impl FnOnce(&mut Status)) {
    let mut all = statuses();
    update(all.entry(connection.to_owned()).or_default());
    drop(all);
    yuzora_core::js::request_refresh();
}

// ---------------------------------------------------------------- pool

type Key = (String, Option<String>, String);

struct Live {
    conn: Arc<tokio::sync::Mutex<Option<Conn>>>,
    canceller: Mutex<Option<Canceller>>,
}

static POOL: LazyLock<Mutex<HashMap<Key, Arc<Live>>>> = LazyLock::new(Default::default);

fn pool() -> MutexGuard<'static, HashMap<Key, Arc<Live>>> {
    POOL.lock().unwrap_or_else(|e| e.into_inner())
}

fn live(key: &Key) -> Arc<Live> {
    let mut pool = pool();
    if key.2 == META && !pool.contains_key(key) {
        // The sidebar browses one database of a connection at a time: the
        // catalog connection to the one it left goes.
        pool.retain(|(c, d, o), _| !(o == META && c == &key.0 && d != &key.1));
    }
    pool.entry(key.clone())
        .or_insert_with(|| Arc::new(Live { conn: Default::default(), canceller: Mutex::new(None) }))
        .clone()
}

/// The spec for a saved connection (its stored password included).
pub fn spec_for(config: ConnectionConfig, database: Option<&str>) -> DbResult<ConnectSpec> {
    refuse_tunnel(&config)?;
    let password = if config.engine == config::Engine::Sqlite { None } else { config::password(&config.id)? };
    let mut spec = ConnectSpec::new(config, password);
    spec.database = database.map(str::to_owned).filter(|d| !d.is_empty());
    Ok(spec)
}

/// SSH tunnel seam: once L4's `yuzora_ssh::tunnel` lands, the tunnel opens
/// here and `ConnectSpec::endpoint` becomes its local end. Until then a
/// connection set to go through one is refused rather than silently made
/// straight to the host.
fn refuse_tunnel(config: &ConnectionConfig) -> DbResult<()> {
    match config.ssh.as_ref().filter(|s| !s.host.is_empty()) {
        Some(ssh) => Err(DbError::Config(crate::i18n::t("error.ssh_pending", &[("host", ssh.host.clone())]))),
        None => Ok(()),
    }
}

/// One connection, held for as long as the lease lives. A connection found
/// closed when the lease ends is dropped, so the next lease opens a new one.
pub struct Lease {
    guard: tokio::sync::OwnedMutexGuard<Option<Conn>>,
}

impl Lease {
    pub fn conn(&mut self) -> &mut Conn {
        self.guard.as_mut().expect("a lease always holds a connection")
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        if self.guard.as_ref().is_some_and(Conn::is_closed) {
            *self.guard = None;
        }
    }
}

/// The connection for (`connection`, `database`, `owner`), opened when needed.
pub async fn lease(connection: &str, database: Option<&str>, owner: &str) -> DbResult<Lease> {
    let key: Key = (connection.to_owned(), database.map(str::to_owned), owner.to_owned());
    let entry = live(&key);
    let mut guard = entry.conn.clone().lock_owned().await;
    if guard.as_ref().is_some_and(Conn::is_closed) {
        *guard = None;
    }
    if guard.is_none() {
        // The credential store may block (a keychain prompt): not on a worker.
        let (id, db) = (connection.to_owned(), database.map(str::to_owned));
        let spec = tokio::task::spawn_blocking(move || config::get(&id).and_then(|c| spec_for(c, db.as_deref())))
            .await
            .unwrap_or_else(|e| Err(DbError::Io(e.to_string())));
        set_status(connection, |s| {
            if s.state != State::Connected {
                s.state = State::Connecting;
            }
        });
        match async { Conn::connect(&spec?).await }.await {
            Ok(conn) => {
                let version = conn.server_version();
                *entry.canceller.lock().unwrap_or_else(|e| e.into_inner()) = Some(conn.canceller());
                *guard = Some(conn);
                set_status(connection, |s| {
                    s.state = State::Connected;
                    s.version = version;
                    s.error = None;
                });
            }
            Err(e) => {
                set_status(connection, |s| {
                    s.state = State::Error;
                    s.error = Some(e.to_string());
                });
                return Err(e);
            }
        }
    }
    Ok(Lease { guard })
}

fn canceller(connection: &str, database: Option<&str>, owner: &str) -> Option<Canceller> {
    let key: Key = (connection.to_owned(), database.map(str::to_owned), owner.to_owned());
    let entry = pool().get(&key).cloned()?;
    let c = entry.canceller.lock().unwrap_or_else(|e| e.into_inner()).clone();
    c
}

/// Ends everything `owner` (a pane) has: its running work is cancelled, its
/// jobs and results forgotten, its connections closed.
pub fn release_owner(owner: &str) {
    supersede(owner);
    pool().retain(|(_, _, o), _| o != owner);
    results::release_owner(owner);
}

/// Closes every connection to `connection` and forgets its status.
pub fn disconnect(connection: &str) {
    pool().retain(|(c, _, _), _| c != connection);
    statuses().remove(connection);
    yuzora_core::js::request_refresh();
}

/// Connects once (a fresh connection, not pooled): the connection form's Test.
pub async fn test(config: ConnectionConfig, password: Option<String>) -> DbResult<(String, u64)> {
    let password = match password {
        Some(p) => Some(p),
        None => config::saved_password_for(&config)?,
    };
    refuse_tunnel(&config)?;
    let mut spec = ConnectSpec::new(config, password);
    spec.connect_timeout = Duration::from_secs(10);
    let started = Instant::now();
    let conn = Conn::connect(&spec).await?;
    Ok((conn.server_version(), started.elapsed().as_millis() as u64))
}

pub async fn databases(connection: &str) -> DbResult<Vec<String>> {
    lease(connection, None, META).await?.conn().databases().await
}

pub async fn objects(connection: &str, database: Option<&str>) -> DbResult<Vec<DbObject>> {
    lease(connection, database, META).await?.conn().objects().await
}

pub async fn describe(connection: &str, database: Option<&str>, object: &ObjectRef) -> DbResult<TableInfo> {
    lease(connection, database, META).await?.conn().describe(object).await
}

// ---------------------------------------------------------------- jobs

pub type JobId = u64;

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobState {
    pub id: JobId,
    pub running: bool,
    pub version: u64,
    pub elapsed_ms: u64,
    pub error: Option<String>,
    pub cancelled: bool,
    pub results: Vec<results::Summary>,
}

struct Job {
    connection: String,
    database: Option<String>,
    owner: String,
    started: Instant,
    running: bool,
    cancelled: bool,
    error: Option<String>,
    elapsed_ms: u64,
    results: Vec<ResultId>,
    version: u64,
    watchers: Vec<async_channel::Sender<()>>,
}

static JOBS: LazyLock<Mutex<(JobId, HashMap<JobId, Job>)>> = LazyLock::new(|| Mutex::new((1, HashMap::new())));

fn jobs() -> MutexGuard<'static, (JobId, HashMap<JobId, Job>)> {
    JOBS.lock().unwrap_or_else(|e| e.into_inner())
}

fn new_job(connection: &str, database: Option<&str>, owner: &str) -> JobId {
    // A pane shows its newest job only: the ones before it stop and go.
    supersede(owner);
    let mut all = jobs();
    let id = all.0;
    all.0 += 1;
    all.1.insert(
        id,
        Job {
            connection: connection.to_owned(),
            database: database.map(str::to_owned),
            owner: owner.to_owned(),
            started: Instant::now(),
            running: true,
            cancelled: false,
            error: None,
            elapsed_ms: 0,
            results: Vec::new(),
            version: 1,
            watchers: Vec::new(),
        },
    );
    id
}

fn update_job(id: JobId, f: impl FnOnce(&mut Job)) {
    let mut all = jobs();
    if let Some(job) = all.1.get_mut(&id) {
        f(job);
        job.version += 1;
        job.elapsed_ms = job.started.elapsed().as_millis() as u64;
        job.watchers.retain(|w| !matches!(w.try_send(()), Err(async_channel::TrySendError::Closed(()))));
    }
}

fn job_cancelled(id: JobId) -> bool {
    jobs().1.get(&id).is_none_or(|j| j.cancelled)
}

/// The job's state now.
pub fn job_state(id: JobId) -> Option<JobState> {
    let all = jobs();
    let job = all.1.get(&id)?;
    let results = job
        .results
        .iter()
        .filter_map(|r| results::get(*r))
        .map(|set| results::lock(&set).summary())
        .collect();
    Some(JobState {
        id,
        running: job.running,
        version: job.version,
        elapsed_ms: if job.running { job.started.elapsed().as_millis() as u64 } else { job.elapsed_ms },
        error: job.error.clone(),
        cancelled: job.cancelled,
        results,
    })
}

/// The job's state once its version passes `seen` (at once if it already has).
pub async fn job_after(id: JobId, seen: u64) -> DbResult<JobState> {
    let rx = {
        let mut all = jobs();
        let Some(job) = all.1.get_mut(&id) else {
            return Err(DbError::NotFound(crate::i18n::t("error.no_job", &[])));
        };
        if job.version > seen {
            None
        } else {
            let (tx, rx) = async_channel::bounded(1);
            job.watchers.retain(|w| !w.is_closed());
            job.watchers.push(tx);
            Some(rx)
        }
    };
    if let Some(rx) = rx {
        // Progress (rows arriving) only bumps the result, so look again now and then.
        let _ = tokio::time::timeout(Duration::from_millis(250), rx.recv()).await;
    }
    job_state(id).ok_or_else(|| DbError::NotFound(crate::i18n::t("error.no_job", &[])))
}

/// Stops a running job: the engine is asked to cancel, the sink stops reading.
pub fn cancel(id: JobId) {
    let target = {
        let mut all = jobs();
        let Some(job) = all.1.get_mut(&id) else { return };
        if !job.running {
            return;
        }
        job.cancelled = true;
        (job.connection.clone(), job.database.clone(), job.owner.clone())
    };
    update_job(id, |_| {});
    if let Some(c) = canceller(&target.0, target.1.as_deref(), &target.2) {
        runtime::spawn(async move { c.cancel().await });
    }
}

/// Cancels `owner`'s running jobs and forgets all of them with their results.
/// A job still winding down finds itself gone: it stops reading and keeps
/// nothing it makes after this (see [`JobSink::start`]).
fn supersede(owner: &str) {
    let ids: Vec<JobId> = jobs().1.iter().filter(|(_, j)| j.owner == owner).map(|(id, _)| *id).collect();
    for id in &ids {
        cancel(*id);
    }
    let gone: Vec<Job> = {
        let mut all = jobs();
        ids.iter().filter_map(|id| all.1.remove(id)).collect()
    };
    for job in gone {
        for r in job.results {
            results::release(r);
        }
    }
}

/// Feeds one job's statements into results.
struct JobSink {
    job: JobId,
    owner: String,
    statement: String,
    max_rows: usize,
    current: Option<Arc<Mutex<ResultSet>>>,
}

impl JobSink {
    fn new(job: JobId, owner: &str, statement: &str) -> Self {
        Self { job, owner: owner.to_owned(), statement: statement.to_owned(), max_rows: max_rows(), current: None }
    }

    fn start(&mut self) -> Arc<Mutex<ResultSet>> {
        let set = results::create(&self.owner, &self.statement);
        let id = results::lock(&set).id;
        if !jobs().1.contains_key(&self.job) {
            // Superseded or released: nobody will look at this one.
            results::release(id);
        }
        update_job(self.job, |j| j.results.push(id));
        self.current = Some(set.clone());
        set
    }

    fn finish(&mut self, error: Option<String>) {
        if let Some(set) = self.current.take() {
            let id = {
                let mut r = results::lock(&set);
                r.complete = true;
                r.elapsed_ms = r.started.elapsed().as_millis() as u64;
                if r.error.is_none() {
                    r.error = error;
                }
                r.version += 1;
                r.id
            };
            results::changed(id);
            update_job(self.job, |_| {});
        }
    }
}

impl RowSink for JobSink {
    fn columns(&mut self, columns: Vec<Column>) {
        self.finish(None);
        let set = self.start();
        results::lock(&set).columns = columns;
    }

    fn rows(&mut self, rows: Vec<Vec<Cell>>) -> bool {
        if job_cancelled(self.job) {
            return false;
        }
        let Some(set) = self.current.clone() else { return true };
        let (id, more) = {
            let mut r = results::lock(&set);
            let room = self.max_rows.saturating_sub(r.rows.len());
            let offered = rows.len();
            let take = offered.min(room);
            r.rows.extend(rows.into_iter().take(take));
            // Truncated only once a row doesn't fit: at exactly the cap it
            // keeps reading to find out whether there is one.
            let more = take == offered;
            if !more {
                r.truncated = true;
            }
            r.elapsed_ms = r.started.elapsed().as_millis() as u64;
            r.version += 1;
            (r.id, more)
        };
        results::changed(id);
        more
    }

    fn done(&mut self, affected: Option<u64>) {
        if self.current.is_none() {
            let set = self.start();
            results::lock(&set).affected = affected;
        } else if let Some(set) = self.current.as_ref() {
            results::lock(set).affected = affected;
        }
        self.finish(None);
    }
}

fn finish_job(id: JobId, outcome: DbResult<()>, sink: &mut JobSink) {
    let error = match &outcome {
        Err(DbError::Cancelled) => None,
        Err(e) => Some(e.to_string()),
        Ok(()) => None,
    };
    sink.finish(error.clone());
    update_job(id, |j| {
        j.running = false;
        j.error = error;
        if matches!(outcome, Err(DbError::Cancelled)) {
            j.cancelled = true;
        }
        j.elapsed_ms = j.started.elapsed().as_millis() as u64;
    });
}

/// Starts running `sql` (one statement or a script) for `owner`.
pub fn start_query(connection: &str, database: Option<&str>, owner: &str, sql: &str) -> JobId {
    let id = new_job(connection, database, owner);
    let (connection, database, owner, sql) =
        (connection.to_owned(), database.map(str::to_owned), owner.to_owned(), sql.to_owned());
    runtime::spawn(async move {
        let mut sink = JobSink::new(id, &owner, &sql);
        let started = Instant::now();
        let outcome = match lease(&connection, database.as_deref(), &owner).await {
            Ok(mut l) => l.conn().execute(&sql, &mut sink).await,
            Err(e) => Err(e),
        };
        // In the history before the job reads as finished.
        crate::history::record(&connection, &sql, started.elapsed().as_millis() as u64, outcome.is_ok());
        finish_job(id, outcome, &mut sink);
    });
    id
}

/// Starts reading a table's rows for `owner`: `SELECT *` (capped, optionally
/// ordered), its `count(*)`, and its key so the result can be edited.
pub fn open_table(
    connection: &str,
    database: Option<&str>,
    owner: &str,
    table: &ObjectRef,
    order: Option<(String, bool)>,
) -> JobId {
    let id = new_job(connection, database, owner);
    let (connection, database, owner, table) =
        (connection.to_owned(), database.map(str::to_owned), owner.to_owned(), table.clone());
    runtime::spawn(async move {
        let engine = match config::get(&connection) {
            Ok(c) => c.engine,
            Err(e) => {
                let mut sink = JobSink::new(id, &owner, "");
                finish_job(id, Err(e), &mut sink);
                return;
            }
        };
        // One row past the cap, so a capped table reads as truncated.
        let limit = max_rows() as u64 + 1;
        let sql = engine::select_sql(engine, &table, order.as_ref().map(|(c, d)| (c.as_str(), *d)), limit);
        let info = describe(&connection, database.as_deref(), &table).await;
        let mut sink = JobSink::new(id, &owner, &sql);
        let mut l = match lease(&connection, database.as_deref(), &owner).await {
            Ok(l) => l,
            Err(e) => return finish_job(id, Err(e), &mut sink),
        };
        let outcome = l.conn().execute(&sql, &mut sink).await;
        // The key as column positions; no key (or a key column not in the
        // result) means read-only, with the reason shown.
        if let Some(set) = sink.current.clone().or_else(|| last_result(id)) {
            let mut r = results::lock(&set);
            let names: Vec<String> = info.as_ref().map(TableInfo::key_columns).unwrap_or_default();
            let positions: Option<Vec<usize>> =
                names.iter().map(|n| r.columns.iter().position(|c| &c.name == n)).collect();
            match (info.as_ref(), positions) {
                (Err(e), _) => r.not_editable = Some(crate::i18n::t("edit.no_info", &[("error", e.to_string())])),
                (Ok(_), Some(key)) if !key.is_empty() => {
                    r.edit = Some(EditTarget {
                        connection: connection.clone(),
                        database: database.clone(),
                        owner: owner.clone(),
                        table: table.clone(),
                        key,
                    })
                }
                _ => r.not_editable = Some(crate::i18n::t("edit.no_key", &[])),
            }
            r.version += 1;
        }
        // The row count, after the rows (a slow count never delays them) and
        // while the job still runs, so Stop reaches it too.
        if outcome.is_ok() && !job_cancelled(id) {
            let mut one = FirstCell(None);
            let counted = l.conn().execute(&engine::count_sql(engine, &table), &mut one).await;
            if let (Ok(()), Some(n), Some(set)) = (counted, one.0, last_result(id)) {
                let mut r = results::lock(&set);
                r.total = n.display().trim().parse().ok();
                r.version += 1;
            }
        }
        drop(l);
        finish_job(id, outcome, &mut sink);
    });
    id
}

fn last_result(job: JobId) -> Option<Arc<Mutex<ResultSet>>> {
    let id = *jobs().1.get(&job)?.results.last()?;
    results::get(id)
}

/// Keeps the first cell of the first row.
struct FirstCell(Option<Cell>);

impl RowSink for FirstCell {
    fn columns(&mut self, _: Vec<Column>) {}
    fn rows(&mut self, rows: Vec<Vec<Cell>>) -> bool {
        if self.0.is_none() {
            self.0 = rows.into_iter().next().and_then(|r| r.into_iter().next());
        }
        // Not `false`: stopping early would send a cancel that could land on
        // the next statement. A count is one row anyway.
        true
    }
    fn done(&mut self, _: Option<u64>) {}
}

// ---------------------------------------------------------------- write-back

/// The row edits of a result, keyed by its primary key.
pub fn row_edits(set: &ResultSet) -> DbResult<(EditTarget, Vec<RowEdit>)> {
    let target = set.edit.clone().ok_or_else(|| {
        DbError::NotEditable(set.not_editable.clone().unwrap_or_else(|| crate::i18n::t("edit.no_key", &[])))
    })?;
    let mut by_row: std::collections::BTreeMap<u32, Vec<(String, Cell)>> = Default::default();
    for ((row, column), value) in &set.edits {
        let name = set.columns.get(*column as usize).map(|c| c.name.clone()).unwrap_or_default();
        by_row.entry(*row).or_default().push((name, value.clone()));
    }
    let mut edits = Vec::with_capacity(by_row.len());
    for (row, changes) in by_row {
        let original = set.rows.get(row as usize).ok_or_else(|| DbError::NotFound(String::new()))?;
        let key: Vec<(String, Cell)> = target
            .key
            .iter()
            .map(|k| (set.columns[*k].name.clone(), original.get(*k).cloned().unwrap_or(Cell::Null)))
            .collect();
        if key.iter().any(|(_, v)| v.is_null()) {
            return Err(DbError::NotEditable(crate::i18n::t("edit.null_key", &[])));
        }
        edits.push(RowEdit { key, changes });
    }
    Ok((target, edits))
}

/// Writes a result's pending edits in one transaction; on success they become
/// its values.
pub async fn write_back(result: ResultId) -> DbResult<u64> {
    let set = results::get(result).ok_or_else(|| DbError::NotFound(String::new()))?;
    let prepared = {
        let mut r = results::lock(&set);
        let out = row_edits(&r).map(|(target, edits)| (target, edits, r.edits.clone()));
        match &out {
            Ok(_) => {
                r.writing = true;
                r.write_error = None;
            }
            Err(e) => r.write_error = Some(e.to_string()),
        }
        r.version += 1;
        out
    };
    results::changed(result);
    let (target, edits, written) = prepared?;
    let outcome = match lease(&target.connection, target.database.as_deref(), &target.owner).await {
        Ok(mut l) => l.conn().apply_edits(&target.table, &edits).await,
        Err(e) => Err(e),
    };
    {
        let mut r = results::lock(&set);
        r.writing = false;
        match &outcome {
            Ok(_) => r.commit_edits(&written),
            Err(e) => r.write_error = Some(e.to_string()),
        }
        r.version += 1;
    }
    results::changed(result);
    outcome
}
