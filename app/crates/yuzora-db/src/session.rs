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

use futures_util::future::{Either, select};

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
    turn: Mutex<Turn>,
}

/// Who wants a connection while a count has borrowed it. A person's work
/// (a save, a query, a read) goes before a count, which is only there to make
/// the total known: it steps aside and runs again once the work is done.
#[derive(Default)]
struct Turn {
    /// Work asking for the connection, or about to hold it.
    waiting: usize,
    /// The count running on the connection now.
    borrower: Option<Borrower>,
}

struct Borrower {
    stand_down: StandDown,
    stepped_aside: Arc<AtomicBool>,
}

/// Asks the count that borrowed a connection to give it up.
#[derive(Clone)]
struct StandDown(Arc<tokio::sync::watch::Sender<bool>>);

impl StandDown {
    fn new() -> Self {
        Self(Arc::new(tokio::sync::watch::channel(false).0))
    }

    fn call(&self) {
        self.0.send_replace(true);
    }

    async fn called(&self) {
        let _ = self.0.subscribe().wait_for(|called| *called).await;
    }
}

impl Live {
    /// Work asks for the connection: a count on it steps aside now, not when it is done.
    fn want(self: &Arc<Self>) -> Wanting {
        let mut turn = self.turn.lock().unwrap_or_else(|e| e.into_inner());
        turn.waiting += 1;
        if let Some(borrower) = &turn.borrower {
            borrower.stepped_aside.store(true, Ordering::SeqCst);
            borrower.stand_down.call();
        }
        Wanting(self.clone())
    }

    /// A count that holds the connection takes its turn, unless work is
    /// already asking for it (`None`: let it go).
    fn borrow(self: &Arc<Self>, stand_down: StandDown) -> Option<Borrowing> {
        let mut turn = self.turn.lock().unwrap_or_else(|e| e.into_inner());
        if turn.waiting > 0 {
            return None;
        }
        let stepped_aside = Arc::new(AtomicBool::new(false));
        turn.borrower = Some(Borrower { stand_down, stepped_aside: stepped_aside.clone() });
        Some(Borrowing { live: self.clone(), stepped_aside })
    }
}

/// Work waiting for a connection, until it holds it.
struct Wanting(Arc<Live>);

impl Drop for Wanting {
    fn drop(&mut self) {
        self.0.turn.lock().unwrap_or_else(|e| e.into_inner()).waiting -= 1;
    }
}

struct Borrowing {
    live: Arc<Live>,
    stepped_aside: Arc<AtomicBool>,
}

impl Borrowing {
    fn stepped_aside(&self) -> bool {
        self.stepped_aside.load(Ordering::SeqCst)
    }
}

impl Drop for Borrowing {
    fn drop(&mut self) {
        self.live.turn.lock().unwrap_or_else(|e| e.into_inner()).borrower = None;
    }
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
        .or_insert_with(|| Arc::new(Live { conn: Default::default(), turn: Default::default() }))
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

    /// Ends the connection instead of giving it to the next user: a stop sent
    /// to it may still be on its way, and would find that user's statement.
    fn retire(&mut self) {
        *self.guard = None;
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
/// A count that borrowed it steps aside for the caller.
pub async fn lease(connection: &str, database: Option<&str>, owner: &str) -> DbResult<Lease> {
    open_lease(connection, database, owner, Use::Work).await
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Use {
    /// A person's work. Opening its connection is what the sidebar shows of the saved connection.
    Work,
    /// A count. A connection opened for it that can't be is its loss, not the connection's.
    Count,
}

async fn open_lease(connection: &str, database: Option<&str>, owner: &str, usage: Use) -> DbResult<Lease> {
    let report = usage == Use::Work;
    let key: Key = (connection.to_owned(), database.map(str::to_owned), owner.to_owned());
    let entry = live(&key);
    let wanting = report.then(|| entry.want());
    let mut guard = entry.conn.clone().lock_owned().await;
    drop(wanting);
    // A Stop pressed for what ran here before may still be on its way to the
    // server, and would be felt by whatever starts now.
    if let Some(conn) = guard.as_ref() {
        conn.settled().await;
    }
    if guard.as_ref().is_some_and(Conn::is_closed) {
        *guard = None;
    }
    if guard.is_none() {
        // The credential store may block (a keychain prompt): not on a worker.
        let (id, db) = (connection.to_owned(), database.map(str::to_owned));
        let spec = tokio::task::spawn_blocking(move || config::get(&id).and_then(|c| spec_for(c, db.as_deref())))
            .await
            .unwrap_or_else(|e| Err(DbError::Io(e.to_string())));
        if report {
            set_status(connection, |s| {
                if s.state != State::Connected {
                    s.state = State::Connecting;
                }
            });
        }
        match async {
            let mut spec = spec?;
            spec.for_table = is_table_owner(owner);
            Conn::connect(&spec).await
        }
        .await
        {
            Ok(conn) => {
                let version = conn.server_version();
                *guard = Some(conn);
                if report {
                    set_status(connection, |s| {
                        s.state = State::Connected;
                        s.version = version;
                        s.error = None;
                    });
                }
            }
            Err(e) => {
                if report {
                    set_status(connection, |s| {
                        s.state = State::Error;
                        s.error = Some(e.to_string());
                    });
                }
                return Err(e);
            }
        }
    }
    Ok(Lease { guard })
}

/// Ends everything `owner` (a pane) has: its running work is cancelled, its
/// jobs and results forgotten, its connections closed.
pub fn release_owner(owner: &str) {
    supersede(owner);
    let (count, table) = (count_owner(owner), table_owner(owner));
    pool().retain(|(_, _, o), _| o != owner && *o != count && *o != table);
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
    owner: String,
    started: Instant,
    running: bool,
    cancelled: bool,
    error: Option<String>,
    elapsed_ms: u64,
    results: Vec<ResultId>,
    /// Stops what the job is running (see [`serving`]); `None` while the job
    /// waits or does something else.
    serving: Option<Stop>,
    /// Called when the job is stopped, whatever it is doing: what only waits
    /// (for a table's description) ends on it.
    halt: StandDown,
    /// The description of the table the job opens (see [`open_table`]).
    info: Option<InfoRx>,
    version: u64,
    watchers: Vec<async_channel::Sender<()>>,
}

static JOBS: LazyLock<Mutex<(JobId, HashMap<JobId, Job>)>> = LazyLock::new(|| Mutex::new((1, HashMap::new())));

fn jobs() -> MutexGuard<'static, (JobId, HashMap<JobId, Job>)> {
    JOBS.lock().unwrap_or_else(|e| e.into_inner())
}

fn new_job(owner: &str) -> JobId {
    // A pane shows its newest job only: the ones before it stop and go.
    supersede(owner);
    let mut all = jobs();
    let id = all.0;
    all.0 += 1;
    all.1.insert(
        id,
        Job {
            owner: owner.to_owned(),
            started: Instant::now(),
            running: true,
            cancelled: false,
            error: None,
            elapsed_ms: 0,
            results: Vec::new(),
            serving: None,
            halt: StandDown::new(),
            info: None,
            version: 1,
            watchers: Vec::new(),
        },
    );
    id
}

/// How a job's Stop ends what it is running.
#[derive(Clone)]
enum Stop {
    /// The statement the job runs on the connection it holds.
    Statement(Canceller),
    /// A count on a connection that is the pane's: the count ends its own
    /// statement and gives the connection back (see [`count_between`]), because
    /// the stop is sent from nowhere else than the task that holds the lease.
    Count(StandDown),
}

impl Stop {
    /// Presses the stop, with the jobs lock held. `Serving` lets go of its
    /// stop under that lock, and does so before the job lets go of the
    /// connection, so a stop that was pressed is counted on its way before
    /// any other work can have the connection and not wait for it.
    fn press(&self, _jobs: &MutexGuard<'static, (JobId, HashMap<JobId, Job>)>) {
        #[cfg(test)]
        PRESSED_WITH_THE_LOCK_HELD
            .with(|held| held.set(Some(matches!(JOBS.try_lock(), Err(std::sync::TryLockError::WouldBlock)))));
        match self {
            Stop::Statement(c) => c.cancel_in_background(),
            Stop::Count(count) => count.call(),
        }
    }
}

#[cfg(test)]
thread_local! {
    /// Whether the last stop pressed on this thread was pressed with the jobs lock held.
    static PRESSED_WITH_THE_LOCK_HELD: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// While the guard lives, `job`'s Stop reaches what `stop` belongs to: the
/// statement the job runs on the connection it holds. Take it once that lease
/// is held and let it go before the lease is: a table's connection also carries
/// its write-back, which only its own Stop may end.
struct Serving(JobId);

fn serving(job: JobId, stop: Stop) -> Serving {
    if let Some(j) = jobs().1.get_mut(&job) {
        j.serving = Some(stop);
    }
    Serving(job)
}

impl Drop for Serving {
    fn drop(&mut self) {
        if let Some(j) = jobs().1.get_mut(&self.0) {
            j.serving = None;
        }
    }
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

/// Stops a running job: the engine is asked to cancel the statement it is
/// running, the sink stops reading. Nothing else on the pane's connections is
/// touched: a write-back is not the job's.
pub fn cancel(id: JobId) {
    {
        let mut all = jobs();
        let Some(job) = all.1.get_mut(&id) else { return };
        if !job.running {
            return;
        }
        job.cancelled = true;
        job.halt.call();
        if let Some(stop) = job.serving.clone() {
            stop.press(&all);
        }
    }
    update_job(id, |_| {});
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
    /// When the statement whose result comes next began, if its engine said so (`RowSink::started`).
    began: Option<Instant>,
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
            began: None,
        }
    }

    fn for_table(job: JobId, owner: &str, statement: &str) -> Self {
        Self { table: true, ..Self::new(job, owner, statement) }
    }

    fn start(&mut self) -> Arc<Mutex<ResultSet>> {
        let set = results::create(&self.owner, &self.statement);
        if let Some(at) = self.began.take() {
            results::lock(&set).started = at;
        }
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
    fn started(&mut self, at: Instant) {
        self.began = Some(at);
    }

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
    let id = new_job(owner);
    let (connection, database, owner, sql) =
        (connection.to_owned(), database.map(str::to_owned), owner.to_owned(), sql.to_owned());
    spawn_job(id, async move {
        let mut sink = JobSink::new(id, &owner, &sql);
        let started = Instant::now();
        let outcome = match lease(&connection, database.as_deref(), &owner).await {
            Ok(mut l) => {
                let _serving = serving(id, Stop::Statement(l.conn().canceller()));
                l.conn().execute(&sql, &mut sink).await
            }
            Err(e) => Err(e),
        };
        // In the history before the job reads as finished.
        crate::history::record(&connection, &sql, started.elapsed().as_millis() as u64, outcome.is_ok());
        finish_job(id, outcome, &mut sink);
    });
    id
}

/// What a table's description came to (its key, columns, DDL), or why it
/// could not be had; one per open, shared by whoever waits for it.
type Info = Arc<DbResult<TableInfo>>;
type InfoRx = tokio::sync::watch::Receiver<Option<Info>>;

/// Starts reading a table's rows for `owner`: `SELECT *` (capped, optionally
/// ordered), its `count(*)`, and its key so the result can be edited.
///
/// The table's description is asked for at the same moment, on the sidebar's
/// connection, and the rows do not wait for it: they are read on the table's
/// own connection ([`table_owner`]), and what they are (editable or not) is
/// settled when it is in. It is read afresh for every open, never kept: a
/// refresh sees the key the table has now. The pane's Structure tab takes it
/// from the job ([`table_info`]) instead of asking for it again.
pub fn open_table(
    connection: &str,
    database: Option<&str>,
    owner: &str,
    table: &ObjectRef,
    order: Option<(String, bool)>,
) -> JobId {
    let id = new_job(owner);
    let (info_tx, mut info_rx) = tokio::sync::watch::channel(None);
    let (held_tx, held_rx) = tokio::sync::oneshot::channel::<()>();
    let halt = jobs().1.get_mut(&id).map(|job| {
        job.info = Some(info_rx.clone());
        job.halt.clone()
    });
    let halt = halt.unwrap_or_else(StandDown::new);
    let (connection, database, owner, table) =
        (connection.to_owned(), database.map(str::to_owned), owner.to_owned(), table.clone());
    spawn_job(id, async move {
        let opened_with = match config::get(&connection) {
            Ok(c) => c,
            Err(e) => {
                let _ = info_tx.send(Some(Arc::new(Err(e.clone()))));
                let mut sink = JobSink::new(id, &owner, "");
                finish_job(id, Err(e), &mut sink);
                return;
            }
        };
        let (engine, read_only) = (opened_with.engine, opened_with.read_only);
        {
            let (connection, database, table) = (connection.clone(), database.clone(), table.clone());
            runtime::spawn(async move {
                let leased = lease(&connection, database.as_deref(), META).await;
                let _ = held_tx.send(());
                let described = match leased {
                    Ok(mut l) => l.conn().describe(&table).await,
                    Err(e) => Err(e),
                };
                let _ = info_tx.send(Some(Arc::new(described)));
            });
        }
        let mut l = match lease(&connection, database.as_deref(), &table_owner(&owner)).await {
            Ok(l) => l,
            Err(e) => return finish_job(id, Err(e), &mut JobSink::new(id, &owner, "")),
        };
        let home = l.conn().database().map(str::to_owned);
        // One row past the cap, so a capped table reads as truncated.
        let limit = max_rows() as u64 + 1;
        let order = order.as_ref().map(|(c, d)| (c.as_str(), *d));
        let sql = engine::select_sql(engine, home.as_deref(), &table, order, limit);
        let mut sink = JobSink::for_table(id, &owner, &sql);
        let reading = serving(id, Stop::Statement(l.conn().canceller()));
        let outcome = l.conn().execute(&format!("{}{sql}", engine::table_prelude(engine)), &mut sink).await;
        // The table's connection is free from here: the count and any write-back
        // do not wait for each other, and the job's Stop is no longer for it.
        drop(reading);
        drop(l);
        // The rows are in, whether or not the engine said so (a read that hit
        // the cap ends mid-stream): the result is complete. What it is comes
        // with the description, and the count with its own connection: neither
        // waits for the other.
        let read_failed = match &outcome {
            Ok(()) | Err(DbError::Cancelled) => None,
            Err(e) => Some(e.to_string()),
        };
        sink.finish(read_failed.clone());
        let set = sink.last.clone();
        let settle = async {
            let Some(set) = set.clone() else { return };
            let known = if read_failed.is_some() {
                Some(Editing::ReadOnly(ReadOnly::ReadFailed))
            } else if read_only {
                Some(Editing::ReadOnly(ReadOnly::Connection))
            } else {
                None
            };
            let editing = match known {
                Some(editing) => editing,
                None => match described(&mut info_rx, &halt).await {
                    None => Editing::ReadOnly(ReadOnly::NoInfo(crate::i18n::t("error.info_stopped", &[]))),
                    Some(info) => {
                        // The key as column positions; no key (or a key column
                        // not in the result) means read-only, with the reason shown.
                        let r = results::lock(&set);
                        let names: Vec<String> = info.as_ref().as_ref().map(TableInfo::key_columns).unwrap_or_default();
                        let positions: Option<Vec<usize>> =
                            names.iter().map(|n| r.columns.iter().position(|c| &c.name == n)).collect();
                        match (info.as_ref(), positions) {
                            (Err(e), _) => Editing::ReadOnly(ReadOnly::NoInfo(e.to_string())),
                            // A write-back finds its row by the names of the key's columns and the values
                            // the rows hold: where the description's names are not the rows', that is
                            // another row, or another column.
                            (Ok(described), _) if !described.describes(&r.columns) => {
                                Editing::ReadOnly(ReadOnly::Changed)
                            }
                            (Ok(_), Some(key)) if !key.is_empty() => Editing::Editable(Box::new(EditTarget {
                                connection: connection.clone(),
                                config: opened_with.clone(),
                                database: database.clone(),
                                owner: table_owner(&owner),
                                table: table.clone(),
                                key,
                                read_only: info
                                    .as_ref()
                                    .iter()
                                    .flat_map(|i| &i.columns)
                                    .filter(|c| c.generated)
                                    .filter_map(|c| r.columns.iter().position(|rc| rc.name == c.name))
                                    .collect(),
                            })),
                            _ => Editing::ReadOnly(ReadOnly::NoKey),
                        }
                    }
                },
            };
            let result = {
                let mut r = results::lock(&set);
                r.editing = editing;
                r.version += 1;
                r.id
            };
            results::changed(result);
            update_job(id, |_| {});
        };
        // The row count, after the rows and while the job still runs, so Stop
        // reaches it too. It has a connection of its own: a slow `count(*)`
        // on a big table neither holds back editing nor queues a write-back.
        let count = async {
            if outcome.is_ok() && !job_cancelled(id) {
                // The sidebar's connection is claimed before the count's, as it was when the description
                // came first: on a login limited to two connections the count's place is the one the
                // description needs, and the count (which can borrow the table's) is the one to go without.
                let _ = select(Box::pin(held_rx), Box::pin(halt.called())).await;
                if job_cancelled(id) {
                    return;
                }
                let total = count_rows(&connection, database.as_deref(), &owner, engine, &table, id).await;
                if let (Some(total), Some(set)) = (total, set.clone()) {
                    let result = {
                        let mut r = results::lock(&set);
                        r.total = Some(total);
                        r.version += 1;
                        r.id
                    };
                    results::changed(result);
                }
            }
        };
        futures_util::future::join(settle, count).await;
        finish_job(id, outcome, &mut sink);
    });
    id
}

/// The description of a table once it is in: what the job has by then,
/// `None` if the job is stopped first (the sidebar's connection it waits on is
/// not the job's to cancel, so it stops waiting instead).
async fn described(info: &mut InfoRx, halt: &StandDown) -> Option<Info> {
    if let Some(ready) = info.borrow().clone() {
        return Some(ready);
    }
    let arrived = Box::pin(async {
        match info.wait_for(Option::is_some).await {
            Ok(ready) => ready.clone(),
            // The task that was to read it is gone.
            Err(_) => Some(Arc::new(Err(DbError::Io(crate::i18n::t("error.job_panicked", &[]))))),
        }
    });
    match select(arrived, Box::pin(halt.called())).await {
        Either::Left((info, _)) => info,
        Either::Right(_) => None,
    }
}

/// The description of the table `job` is opening, once it is in: the same one
/// the job decides editing by, so a pane does not ask for it a second time on
/// the same connection. A job the core no longer has is an error.
pub async fn table_info(job: JobId) -> DbResult<TableInfo> {
    let gone = || DbError::NotFound(crate::i18n::t("error.no_job", &[]));
    let mut info = jobs().1.get(&job).and_then(|j| j.info.clone()).ok_or_else(gone)?;
    let ready = match info.wait_for(Option::is_some).await {
        Ok(ready) => ready.clone(),
        Err(_) => return Err(DbError::Io(crate::i18n::t("error.job_panicked", &[]))),
    };
    match ready {
        Some(described) => described.as_ref().clone(),
        None => Err(gone()),
    }
}

/// `count(*)` of `table`. It has a connection of its own (leased under
/// [`count_owner`], closed again right after), so a slow count holds nothing
/// up. Where one more connection is not to be had (a limit on the login, a
/// network blip) the table's own does it, when work has no use for it: the
/// total is worth that, and the connection is not failed for it.
async fn count_rows(
    connection: &str,
    database: Option<&str>,
    owner: &str,
    engine: config::Engine,
    table: &ObjectRef,
    job: JobId,
) -> Option<u64> {
    let counting = count_owner(owner);
    match open_lease(connection, database, &counting, Use::Count).await {
        Ok(l) => {
            let counted = count_on(l, engine, table, job, None).await;
            pool().remove(&(connection.to_owned(), database.map(str::to_owned), counting));
            counted
        }
        Err(_) => count_between(connection, database, &table_owner(owner), engine, table, job).await,
    }
}

/// Counts on the table's own connection, in the time work leaves it. A save or
/// a read that asks for the connection meanwhile has it at once: the count is
/// stopped (the person is waiting, the count only informs) and begins again
/// after them. A SQL tab's query is not that: it runs on the connection of its
/// own.
async fn count_between(
    connection: &str,
    database: Option<&str>,
    owner: &str,
    engine: config::Engine,
    table: &ObjectRef,
    job: JobId,
) -> Option<u64> {
    let key: Key = (connection.to_owned(), database.map(str::to_owned), owner.to_owned());
    loop {
        if job_cancelled(job) {
            return None;
        }
        // Behind whoever is already waiting for it.
        let l = open_lease(connection, database, owner, Use::Count).await.ok()?;
        let stand_down = StandDown::new();
        let Some(borrowing) = live(&key).borrow(stand_down.clone()) else {
            // Work asked while this waited: it has the connection next.
            drop(l);
            tokio::task::yield_now().await;
            continue;
        };
        let counted = count_on(l, engine, table, job, Some(stand_down)).await;
        let stepped_aside = borrowing.stepped_aside();
        drop(borrowing);
        if counted.is_some() || !stepped_aside {
            return counted;
        }
    }
}

/// How long a count that was told to step aside has to end before it is dropped.
const STEP_ASIDE: Duration = Duration::from_secs(3);

/// Counts on the connection `l` holds, which the job's Stop reaches meanwhile.
/// `borrowed`: the connection is the pane's, and `stand_down` is how the count
/// is asked to give it up.
async fn count_on(
    mut l: Lease,
    engine: config::Engine,
    table: &ObjectRef,
    job: JobId,
    borrowed: Option<StandDown>,
) -> Option<u64> {
    let canceller = l.conn().canceller();
    let _serving = serving(job, borrowed.clone().map_or(Stop::Statement(canceller.clone()), Stop::Count));
    let home = l.conn().database().map(str::to_owned);
    let mut one = FirstCell { job, cell: None };
    let sql = engine::count_sql(engine, home.as_deref(), table);
    let (mut asked, mut gave_up) = (false, false);
    let outcome = {
        let counting = Box::pin(l.conn().execute(&sql, &mut one));
        match borrowed {
            None => counting.await,
            Some(stand_down) => match select(counting, Box::pin(stand_down.called())).await {
                Either::Left((outcome, _)) => outcome,
                Either::Right((_, counting)) => {
                    asked = true;
                    canceller.cancel().await;
                    match tokio::time::timeout(STEP_ASIDE, counting).await {
                        Ok(outcome) => outcome,
                        Err(_) => {
                            gave_up = true;
                            Err(DbError::Cancelled)
                        }
                    }
                }
            },
        }
    };
    // A stop that did not reach the count is still on its way to the server,
    // and would find whatever the connection runs next.
    if gave_up || (asked && !l.conn().cancel_landed()) {
        l.retire();
    }
    outcome.ok()?;
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
    let target = set.target().cloned().ok_or_else(|| {
        DbError::NotEditable(set.editing.blocked(set.complete).unwrap_or_else(|| crate::i18n::t("edit.no_key", &[])))
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

/// The owner the connection that reads a table and writes it back is leased
/// under. A pane's SQL tab runs the person's own scripts on `owner`'s
/// connection, and whatever they leave there (a setting, a transaction, a
/// temporary table) stays there: the table's rows are read and written by a
/// connection that only this crate's own statements have run on.
pub fn table_owner(owner: &str) -> String {
    format!("{owner}{TABLE}")
}

const TABLE: &str = ":table";

fn is_table_owner(owner: &str) -> bool {
    owner.ends_with(TABLE)
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
    // What a stop needs to reach the server, once there is a connection; it
    // also knows whether the write-back had sent COMMIT.
    let engine_stop: Mutex<Option<Canceller>> = Mutex::new(None);
    let work = Box::pin(async {
        if !unchanged() {
            return Err(changed());
        }
        let mut l = lease(&target.connection, target.database.as_deref(), &target.owner).await?;
        if !unchanged() {
            return Err(changed());
        }
        let canceller = l.conn().canceller();
        canceller.write_back_starts(&stop);
        *engine_stop.lock().unwrap_or_else(|e| e.into_inner()) = Some(canceller);
        l.conn().apply_edits(&target.table, &edits).await
    });
    let stopped = Box::pin(stop.stopped());
    let outcome = match futures_util::future::select(work, stopped).await {
        futures_util::future::Either::Left((outcome, _)) => outcome,
        futures_util::future::Either::Right((_, work)) => {
            // Waiting for the connection ends with the future; on the server
            // the statement has to be told (PostgreSQL, SQLite), or the socket
            // dropped with it (SQL Server).
            stop_work(work, async {
                let canceller = engine_stop.lock().unwrap_or_else(|e| e.into_inner()).clone();
                if let Some(canceller) = canceller {
                    canceller.abandon().await;
                }
            })
            .await;
            Err(DbError::Cancelled)
        }
    };
    {
        let mut r = results::lock(set);
        match &outcome {
            Ok(_) => r.commit_edits(&written),
            Err(DbError::Cancelled) => {
                // Only a COMMIT that was sent leaves it to the server whether
                // the changes stand; a stop before that wrote nothing.
                let sent =
                    engine_stop.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(Canceller::commit_sent);
                let words = if sent { "edit.stopped_maybe" } else { "edit.stopped" };
                r.write = WriteState::failed(crate::i18n::t(words, &[]));
            }
            Err(e) => r.write = WriteState::failed(e.to_string()),
        }
        r.version += 1;
    }
    results::changed(result);
    outcome
}

/// Ends work the person stopped: `signal` tells the engine, and returns once
/// the server has dealt with the stop, while `work` still holds the connection
/// it runs on. What dropping `work` sends next (a write-back's ROLLBACK) is
/// therefore the first the server hears after the stop, not something the stop
/// can land on. The connection does not outlive it all the same
/// ([`Canceller::abandon`]): `work` is dropped where it stands.
async fn stop_work<W>(work: W, signal: impl std::future::Future<Output = ()>) {
    signal.await;
    drop(work);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tables_rows_are_loading_until_known_and_a_querys_are_read_only() {
        let id = new_job("sink-test");
        let mut table = JobSink::for_table(id, "sink-test", "select * from t");
        table.columns(vec![Column::new("id", "int4")]);
        assert_eq!(results::lock(table.last.as_ref().unwrap()).editing, Editing::Loading);
        let mut query = JobSink::new(id, "sink-test", "select 1");
        query.columns(vec![Column::new("x", "int4")]);
        assert_eq!(results::lock(query.last.as_ref().unwrap()).editing, Editing::ReadOnly(ReadOnly::Query));
        supersede("sink-test");
    }

    /// A result's time is the statement's, from when it began: an engine that only knows the columns
    /// once the statement has produced a row says when it began (`RowSink::started`) before that.
    #[test]
    fn a_result_counts_its_time_from_when_its_statement_began() {
        let id = new_job("began-test");
        let mut sink = JobSink::new(id, "began-test", "select 1");
        let began = Instant::now();
        std::thread::sleep(Duration::from_millis(50));
        sink.started(began);
        sink.columns(vec![Column::new("x", "int4")]);
        sink.finish(None);
        let elapsed = results::lock(sink.last.as_ref().unwrap()).elapsed_ms;
        supersede("began-test");
        assert!(elapsed >= 50, "the result took {elapsed} ms of a statement that began 50 ms before its columns");
    }

    /// The stop is counted on its way under the lock `Serving` lets go of its
    /// stop under, which the job does before it lets go of the connection. The
    /// race this closes (the connection handed to the next work between the
    /// stop being found and being counted) cannot be made to happen on cue, so
    /// the test looks at what it depends on: the stop is pressed with the lock held.
    #[test]
    fn a_stop_is_pressed_with_the_jobs_lock_held() {
        let id = new_job("stop-under-lock");
        PRESSED_WITH_THE_LOCK_HELD.with(|held| held.set(None));
        let count = StandDown::new();
        let _serving = serving(id, Stop::Count(count.clone()));
        cancel(id);
        let held = PRESSED_WITH_THE_LOCK_HELD.with(|held| held.get());
        supersede("stop-under-lock");
        assert_eq!(held, Some(true), "the stop was pressed without the jobs lock (None: not pressed at all)");
        assert!(count.0.subscribe().borrow().to_owned(), "the stop reached the count");
    }

    /// A count that borrowed the table's connection steps aside for work that
    /// asks for that connection, and a SQL tab's query is not that: it asks for
    /// the connection of its own.
    #[test]
    fn a_count_on_the_tables_connection_steps_aside_for_the_tables_work_and_not_for_the_sql_tabs() {
        let key = |owner: String| ("stand-down-test".to_owned(), None, owner);
        let tab = live(&key("stand-down-pane".to_owned()));
        let table = live(&key(table_owner("stand-down-pane")));
        let stand_down = StandDown::new();
        let counting = table.borrow(stand_down.clone()).expect("nothing waits for the table's connection");
        let tab_work = tab.want();
        assert!(
            !*stand_down.0.subscribe().borrow(),
            "a SQL tab's query told the count on the table's connection to step aside"
        );
        assert!(!counting.stepped_aside());
        let table_work = table.want();
        assert!(*stand_down.0.subscribe().borrow(), "work for the table's connection did not");
        assert!(counting.stepped_aside());
        drop((tab_work, table_work, counting));
        pool().retain(|(connection, _, _), _| connection != "stand-down-test");
    }

    /// What a job waits for before it settles editing: the description once it is in (even if a
    /// Stop came after), nothing once a Stop comes first, and the words for a reader that is gone.
    #[test]
    fn the_wait_for_a_description_ends_with_the_description_a_stop_or_the_reader_gone() {
        let boom = || Arc::new(Err(DbError::NotFound("boom".into())));
        runtime::block_on(async {
            let patience = Duration::from_secs(5);

            let (tx, mut rx) = tokio::sync::watch::channel::<Option<Info>>(None);
            let halt = StandDown::new();
            tx.send(Some(boom())).unwrap();
            halt.call();
            let got =
                tokio::time::timeout(patience, described(&mut rx, &halt)).await.expect("a description that is in");
            assert_eq!(got.as_deref(), Some(&Err(DbError::NotFound("boom".into()))), "what is in comes before a stop");

            let (_held, mut rx) = tokio::sync::watch::channel::<Option<Info>>(None);
            let halt = StandDown::new();
            let (got, _) =
                futures_util::future::join(tokio::time::timeout(patience, described(&mut rx, &halt)), async {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    halt.call();
                })
                .await;
            assert_eq!(got.expect("a stop ends the wait"), None);

            let (tx, mut rx) = tokio::sync::watch::channel::<Option<Info>>(None);
            drop(tx);
            let got = tokio::time::timeout(patience, described(&mut rx, &StandDown::new()))
                .await
                .expect("a reader that is gone");
            assert_eq!(
                got.as_deref(),
                Some(&Err(DbError::Io(crate::i18n::t("error.job_panicked", &[])))),
                "the task that was to read it is gone"
            );
        });
    }

    #[test]
    fn the_engine_is_told_to_stop_while_the_work_still_holds_the_connection() {
        runtime::block_on(async {
            let slot = Arc::new(tokio::sync::Mutex::new(None::<Conn>));
            let held = slot.clone().lock_owned().await;
            let work = Box::pin(async move {
                let _held = held;
                std::future::pending::<()>().await
            });
            let signal = async {
                assert!(slot.try_lock().is_err(), "the connection was let go before the engine was told to stop");
            };
            stop_work(work, signal).await;
            assert!(slot.try_lock().is_ok(), "the connection is let go of once the engine was told");
        });
    }

    /// A Stop is made where it is pressed, not by a task that waits for a free
    /// worker of the runtime: with both of them busy, SQLite's statement still ends.
    #[test]
    fn the_toolbars_stop_acts_where_it_is_pressed() {
        let file = std::env::temp_dir().join(format!("yuzora-db-stop-now-{}.sqlite", std::process::id()));
        std::fs::File::create(&file).unwrap();
        let config = ConnectionConfig {
            name: "stop-now".into(),
            engine: config::Engine::Sqlite,
            path: file.to_string_lossy().into_owned(),
            ..ConnectionConfig::default()
        };
        let id = new_job("stop-now-test");
        let ended = runtime::block_on(async {
            let mut conn = Conn::connect(&ConnectSpec::new(config, None)).await.unwrap();
            let _serving = serving(id, Stop::Statement(conn.canceller()));
            let go = Arc::new(AtomicBool::new(false));
            let busy = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            for _ in 0..2 {
                let (go, busy) = (go.clone(), busy.clone());
                runtime::spawn(async move {
                    busy.fetch_add(1, Ordering::SeqCst);
                    while !go.load(Ordering::SeqCst) {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                });
            }
            while busy.load(Ordering::SeqCst) < 2 {
                std::thread::sleep(Duration::from_millis(1));
            }
            let stop = std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(100));
                cancel(id);
            });
            let release = go.clone();
            let freed = std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(1500));
                release.store(true, Ordering::SeqCst);
            });
            let started = Instant::now();
            let sql = "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c WHERE x < 200000000) SELECT count(*) FROM c";
            let outcome = conn.execute(sql, &mut FirstCell { job: id, cell: None }).await;
            let took = started.elapsed();
            go.store(true, Ordering::SeqCst);
            let _ = (stop.join(), freed.join());
            (outcome, took)
        });
        supersede("stop-now-test");
        let _ = std::fs::remove_file(&file);
        assert_eq!(ended.0, Err(DbError::Cancelled));
        assert!(ended.1 < Duration::from_millis(1000), "the Stop waited for the runtime: {:?}", ended.1);
    }

    #[test]
    fn a_job_whose_work_panics_ends_with_an_error() {
        let id = new_job("panic-test");
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        spawn_job(id, async move {
            let _tx = tx;
            panic!("a driver bug");
        });
        // The sender drops when the panicking task unwinds.
        assert!(rx.recv().is_err());
        let started = Instant::now();
        let state = loop {
            let state = job_state(id).expect("the job");
            if !state.running {
                break state;
            }
            assert!(started.elapsed() < Duration::from_secs(5), "the job is still running after its work panicked");
            std::thread::sleep(Duration::from_millis(5));
        };
        assert!(state.error.is_some_and(|e| !e.is_empty()));
        supersede("panic-test");
    }
}
