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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::config::{self, ConnectionConfig};
use crate::engine::{self, Canceller, Conn, ConnectSpec, RowSink};
use crate::error::{DbError, DbResult};
use crate::model::{Cell, Column, DbObject, ObjectRef, RowEdit, TableInfo};
use crate::results::{self, EditTarget, Editing, ReadOnly, ResultId, ResultSet, WriteState, WriteStop};
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
    let count = count_owner(owner);
    pool().retain(|(_, _, o), _| o != owner && *o != count);
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
    for owner in [target.2.clone(), count_owner(&target.2)] {
        if let Some(c) = canceller(&target.0, target.1.as_deref(), &owner) {
            runtime::spawn(async move { c.cancel().await });
        }
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
    /// The rows of one table: its results start out `Loading` until the key is known.
    table: bool,
    current: Option<Arc<Mutex<ResultSet>>>,
    /// The newest result this sink made, finished or not.
    last: Option<Arc<Mutex<ResultSet>>>,
}

impl JobSink {
    fn new(job: JobId, owner: &str, statement: &str) -> Self {
        Self {
            job,
            owner: owner.to_owned(),
            statement: statement.to_owned(),
            max_rows: max_rows(),
            table: false,
            current: None,
            last: None,
        }
    }

    fn for_table(job: JobId, owner: &str, statement: &str) -> Self {
        Self { table: true, ..Self::new(job, owner, statement) }
    }

    fn start(&mut self) -> Arc<Mutex<ResultSet>> {
        let set = results::create(&self.owner, &self.statement);
        if self.table {
            results::lock(&set).editing = Editing::Loading;
        }
        let id = results::lock(&set).id;
        if !jobs().1.contains_key(&self.job) {
            // Superseded or released: nobody will look at this one.
            results::release(id);
        }
        update_job(self.job, |j| j.results.push(id));
        self.current = Some(set.clone());
        self.last = Some(set.clone());
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
            r.push_rows(rows.into_iter().take(take));
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

    fn cancelled(&self) -> bool {
        job_cancelled(self.job)
    }
}

/// Runs a job's work. A panic in a driver ends the job with an error instead
/// of leaving it running (and its pane loading) for ever.
fn spawn_job(id: JobId, work: impl std::future::Future<Output = ()> + Send + 'static) {
    runtime::spawn(async move {
        if futures_util::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(work)).await.is_err() {
            update_job(id, |j| {
                j.running = false;
                j.error = Some(crate::i18n::t("error.job_panicked", &[]));
            });
        }
    });
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
    spawn_job(id, async move {
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
    spawn_job(id, async move {
        let opened_with = match config::get(&connection) {
            Ok(c) => c,
            Err(e) => {
                let mut sink = JobSink::new(id, &owner, "");
                finish_job(id, Err(e), &mut sink);
                return;
            }
        };
        let (engine, read_only) = (opened_with.engine, opened_with.read_only);
        let info = describe(&connection, database.as_deref(), &table).await;
        let mut l = match lease(&connection, database.as_deref(), &owner).await {
            Ok(l) => l,
            Err(e) => return finish_job(id, Err(e), &mut JobSink::new(id, &owner, "")),
        };
        // In the database the pane's connection was opened in, whatever its
        // SQL tab has `USE`d since (SQL Server).
        let home = l.conn().database().map(str::to_owned);
        // One row past the cap, so a capped table reads as truncated.
        let limit = max_rows() as u64 + 1;
        let order = order.as_ref().map(|(c, d)| (c.as_str(), *d));
        let sql = engine::select_sql(engine, home.as_deref(), &table, order, limit);
        let mut sink = JobSink::for_table(id, &owner, &sql);
        let outcome = l.conn().execute(&sql, &mut sink).await;
        // The pane's connection is free from here: the count and any write-back
        // do not wait for each other.
        drop(l);
        // The rows are in, whether or not the engine said so (a read that hit
        // the cap ends mid-stream): the result is complete, and it is now
        // known what it is. The key as column positions; no key (or a key
        // column not in the result) means read-only, with the reason shown.
        let read_failed = match &outcome {
            Ok(()) | Err(DbError::Cancelled) => None,
            Err(e) => Some(e.to_string()),
        };
        sink.finish(read_failed.clone());
        if let Some(set) = sink.last.clone() {
            let mut r = results::lock(&set);
            let names: Vec<String> = info.as_ref().map(TableInfo::key_columns).unwrap_or_default();
            let positions: Option<Vec<usize>> =
                names.iter().map(|n| r.columns.iter().position(|c| &c.name == n)).collect();
            r.editing = match (info.as_ref(), positions) {
                _ if read_failed.is_some() => Editing::ReadOnly(ReadOnly::ReadFailed),
                _ if read_only => Editing::ReadOnly(ReadOnly::Connection),
                (Err(e), _) => Editing::ReadOnly(ReadOnly::NoInfo(e.to_string())),
                (Ok(_), Some(key)) if !key.is_empty() => Editing::Editable(Box::new(EditTarget {
                    connection: connection.clone(),
                    config: opened_with.clone(),
                    database: database.clone(),
                    owner: owner.clone(),
                    table: table.clone(),
                    key,
                    read_only: info
                        .iter()
                        .flat_map(|i| &i.columns)
                        .filter(|c| c.generated)
                        .filter_map(|c| r.columns.iter().position(|rc| rc.name == c.name))
                        .collect(),
                })),
                _ => Editing::ReadOnly(ReadOnly::NoKey),
            };
            r.version += 1;
            let result = r.id;
            drop(r);
            results::changed(result);
            update_job(id, |_| {});
        }
        // The row count, after the rows and while the job still runs, so Stop
        // reaches it too. It has a connection of its own: a slow `count(*)`
        // on a big table neither holds back editing nor queues a write-back.
        if outcome.is_ok() && !job_cancelled(id) {
            let total = count_rows(&connection, database.as_deref(), &owner, engine, &table, id).await;
            if let (Some(total), Some(set)) = (total, sink.last.clone()) {
                let result = {
                    let mut r = results::lock(&set);
                    r.total = Some(total);
                    r.version += 1;
                    r.id
                };
                results::changed(result);
            }
        }
        finish_job(id, outcome, &mut sink);
    });
    id
}

/// `count(*)` of `table`, on the connection leased under [`count_owner`]
/// (closed again right after: only the count needs it).
async fn count_rows(
    connection: &str,
    database: Option<&str>,
    owner: &str,
    engine: config::Engine,
    table: &ObjectRef,
    job: JobId,
) -> Option<u64> {
    let counting = count_owner(owner);
    let mut l = lease(connection, database, &counting).await.ok()?;
    let home = l.conn().database().map(str::to_owned);
    let mut one = FirstCell { job, cell: None };
    let counted = l.conn().execute(&engine::count_sql(engine, home.as_deref(), table), &mut one).await;
    drop(l);
    pool().remove(&(connection.to_owned(), database.map(str::to_owned), counting));
    counted.ok()?;
    one.cell?.display().trim().parse().ok()
}

/// Keeps the first cell of the first row, for `job`.
struct FirstCell {
    job: JobId,
    cell: Option<Cell>,
}

impl RowSink for FirstCell {
    fn columns(&mut self, _: Vec<Column>) {}
    fn rows(&mut self, rows: Vec<Vec<Cell>>) -> bool {
        if self.cell.is_none() {
            self.cell = rows.into_iter().next().and_then(|r| r.into_iter().next());
        }
        // Not `false`: stopping early would send a cancel that could land on
        // the next statement. A count is one row anyway.
        true
    }
    fn done(&mut self, _: Option<u64>) {}
    fn cancelled(&self) -> bool {
        job_cancelled(self.job)
    }
}

// ---------------------------------------------------------------- write-back

/// The row edits of a result, keyed by its primary key.
pub fn row_edits(set: &ResultSet) -> DbResult<(EditTarget, Vec<RowEdit>)> {
    let target = set
        .target()
        .cloned()
        .ok_or_else(|| DbError::NotEditable(set.editing.blocked().unwrap_or_else(|| crate::i18n::t("edit.no_key", &[]))))?;
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

/// Stops `result`'s write-back, whether it still waits for the pane's
/// connection or is already on the server.
pub fn stop_write(result: ResultId) {
    if let Some(set) = results::get(result) {
        results::lock(&set).stop_write();
    }
}

/// The owner the connection of a table's `count(*)` is leased under.
pub fn count_owner(owner: &str) -> String {
    format!("{owner}:count")
}

/// Writes a result's pending edits in one transaction; on success they become
/// its values.
pub async fn write_back(result: ResultId) -> DbResult<u64> {
    match results::get(result) {
        Some(set) => write_back_set(&set).await,
        None => Err(DbError::NotFound(crate::i18n::t("edit.result_gone", &[]))),
    }
}

/// [`write_back`] for a result its grid holds, whether or not the store still
/// has it. Every failure after the first press is recorded on the result
/// (`write`), where the grid shows it.
pub async fn write_back_set(set: &Arc<Mutex<ResultSet>>) -> DbResult<u64> {
    let result = results::lock(set).id;
    let stop = WriteStop::new();
    let prepared = {
        let mut r = results::lock(set);
        // Pressed again before the pane shows the first one: the same edits
        // would be written twice.
        if r.is_writing() {
            return Err(DbError::NotEditable(crate::i18n::t("edit.writing", &[])));
        }
        let still_there = results::get(result).is_some_and(|stored| Arc::ptr_eq(&stored, set));
        let out = if still_there {
            row_edits(&r).map(|(target, edits)| (target, edits, r.edits.clone()))
        } else {
            Err(DbError::NotFound(crate::i18n::t("edit.result_gone", &[])))
        };
        r.write = match &out {
            Ok(_) => WriteState::Writing(stop.clone()),
            Err(e) => WriteState::failed(e.to_string()),
        };
        r.version += 1;
        out
    };
    results::changed(result);
    let (target, edits, written) = prepared?;
    // Saved since to connect elsewhere (another server, database or file):
    // these keys and values are not for it. Asked again once connected, in
    // case it was saved meanwhile.
    let unchanged = || config::get(&target.connection).is_ok_and(|c| c.connects_like(&target.config));
    let changed = || DbError::NotEditable(crate::i18n::t("edit.connection_changed", &[]));
    // What a stop needs to reach the server, once there is a connection; and
    // whether the statements had started by then.
    let engine_stop: Mutex<Option<Canceller>> = Mutex::new(None);
    let began = AtomicBool::new(false);
    let work = Box::pin(async {
        if !unchanged() {
            return Err(changed());
        }
        let mut l = lease(&target.connection, target.database.as_deref(), &target.owner).await?;
        if !unchanged() {
            return Err(changed());
        }
        *engine_stop.lock().unwrap_or_else(|e| e.into_inner()) = Some(l.conn().canceller());
        began.store(true, Ordering::SeqCst);
        l.conn().apply_edits(&target.table, &edits).await
    });
    let stopped = Box::pin(stop.stopped());
    let outcome = match futures_util::future::select(work, stopped).await {
        futures_util::future::Either::Left((outcome, _)) => outcome,
        futures_util::future::Either::Right((_, work)) => {
            // Waiting for the connection ends with the future; on the server
            // the statement has to be told (PostgreSQL, SQLite), or the socket
            // dropped with it (SQL Server).
            drop(work);
            let canceller = engine_stop.lock().unwrap_or_else(|e| e.into_inner()).take();
            if let Some(canceller) = canceller {
                canceller.cancel().await;
            }
            Err(DbError::Cancelled)
        }
    };
    {
        let mut r = results::lock(set);
        match &outcome {
            Ok(_) => r.commit_edits(&written),
            Err(DbError::Cancelled) => {
                let words = if began.load(Ordering::SeqCst) { "edit.stopped_maybe" } else { "edit.stopped" };
                r.write = WriteState::failed(crate::i18n::t(words, &[]));
            }
            Err(e) => r.write = WriteState::failed(e.to_string()),
        }
        r.version += 1;
    }
    results::changed(result);
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_job_whose_work_panics_ends_with_an_error() {
        let id = new_job("c", None, "panic-test");
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        spawn_job(id, async move {
            let _tx = tx;
            panic!("a driver bug");
        });
        // The sender drops when the panicking task unwinds.
        assert!(rx.recv().is_err());
        let state = loop {
            let state = job_state(id).expect("the job");
            if !state.running {
                break state;
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        assert!(state.error.is_some_and(|e| !e.is_empty()));
        supersede("panic-test");
    }
}
