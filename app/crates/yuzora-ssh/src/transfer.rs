//! The transfer queue: uploads and downloads of files or whole folders, a few
//! at a time, with progress, speed, cancel and retry.
//!
//! Each file is written to `<name>.part` beside its destination and renamed
//! when complete. A `.part` left by a cancelled or broken transfer is resumed
//! from its size the next time (retry, or the same transfer again), as
//! `sftp reget`/`reput` do. With `preserve`, the file's permissions and
//! modification time follow it.

use crate::error::{ErrorKind, Result, SshError};
use crate::session::{Interaction, Manager};
use crate::sftp::{base_name, remote_join};
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::{FileAttributes, OpenFlags};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

/// Transfers running at once; the rest wait their turn.
pub const PARALLEL: usize = 3;
const CHUNK: usize = 256 * 1024;
/// The suffix of a file still being written.
pub const PART: &str = ".part";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Upload,
    Download,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Queued,
    Running,
    Done,
    Failed,
    Cancelled,
}

/// What to copy: `from` (a file or folder) into the folder `to_dir` on the other side.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferSpec {
    pub host: String,
    pub direction: Direction,
    pub from: String,
    pub to_dir: String,
    #[serde(default)]
    pub preserve: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferInfo {
    pub id: u64,
    pub host: String,
    pub direction: Direction,
    pub name: String,
    pub from: String,
    pub to: String,
    pub preserve: bool,
    pub state: State,
    /// Bytes in all, and copied so far (resumed bytes count as copied).
    pub total: u64,
    pub done: u64,
    /// Bytes per second, smoothed.
    pub rate: u64,
    pub files: u32,
    pub files_done: u32,
    /// Picked up from a `.part`, and from how far.
    pub resumed: bool,
    pub resumed_from: u64,
    pub error: Option<String>,
    /// ms since the epoch.
    pub queued_at: u64,
    pub finished_at: Option<u64>,
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

struct Job {
    spec: TransferSpec,
    info: Mutex<TransferInfo>,
    done: AtomicU64,
    cancel: AtomicBool,
    speed: Mutex<(Instant, u64, f64)>,
}

impl Job {
    fn info(&self) -> TransferInfo {
        let mut i = self.info.lock().unwrap_or_else(|e| e.into_inner()).clone();
        i.done = self.done.load(Ordering::Relaxed);
        i
    }

    fn edit(&self, f: impl FnOnce(&mut TransferInfo)) {
        f(&mut self.info.lock().unwrap_or_else(|e| e.into_inner()));
    }

    fn cancelled(&self) -> Result<()> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(SshError::text(ErrorKind::Cancelled, "cancelled", &[]));
        }
        Ok(())
    }

    fn add(&self, n: u64) {
        let done = self.done.fetch_add(n, Ordering::Relaxed) + n;
        let mut speed = self.speed.lock().unwrap_or_else(|e| e.into_inner());
        let (at, from, rate) = *speed;
        let dt = at.elapsed().as_secs_f64();
        if dt >= 0.5 {
            let now = (done.saturating_sub(from)) as f64 / dt;
            let smoothed = if rate == 0.0 { now } else { rate * 0.5 + now * 0.5 };
            *speed = (Instant::now(), done, smoothed);
            self.edit(|i| i.rate = smoothed as u64);
        }
    }
}

/// The queue the manager owns.
pub struct Queue {
    jobs: Mutex<Vec<Arc<Job>>>,
    next: AtomicU64,
    slots: Arc<tokio::sync::Semaphore>,
}

impl Default for Queue {
    fn default() -> Self {
        Self { jobs: Mutex::new(Vec::new()), next: AtomicU64::new(1), slots: Arc::new(tokio::sync::Semaphore::new(PARALLEL)) }
    }
}

impl Queue {
    fn jobs(&self) -> Vec<Arc<Job>> {
        self.jobs.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn find(&self, id: u64) -> Option<Arc<Job>> {
        self.jobs().into_iter().find(|j| j.info().id == id)
    }

    /// Every transfer, oldest first.
    pub fn list(&self) -> Vec<TransferInfo> {
        self.jobs().iter().map(|j| j.info()).collect()
    }

    pub fn get(&self, id: u64) -> Option<TransferInfo> {
        self.find(id).map(|j| j.info())
    }

    /// Stops a queued or running transfer; its `.part` stays for a retry.
    pub fn cancel(&self, id: u64) -> bool {
        match self.find(id) {
            Some(j) if matches!(j.info().state, State::Queued | State::Running) => {
                j.cancel.store(true, Ordering::Relaxed);
                true
            }
            _ => false,
        }
    }

    /// Forgets finished transfers (done, failed, cancelled).
    pub fn clear_finished(&self) {
        self.jobs.lock().unwrap_or_else(|e| e.into_inner()).retain(|j| matches!(j.info().state, State::Queued | State::Running));
    }

    /// "↑ 1 running · 62%" while anything moves.
    pub fn status_line(&self) -> Option<(String, &'static str)> {
        let active: Vec<TransferInfo> = self.list().into_iter().filter(|i| i.state == State::Running).collect();
        if active.is_empty() {
            return None;
        }
        let (done, total) = active.iter().fold((0u64, 0u64), |(d, t), i| (d + i.done, t + i.total));
        let ups = active.iter().filter(|i| i.direction == Direction::Upload).count();
        let arrow = if ups == active.len() { "↑" } else if ups == 0 { "↓" } else { "⇅" };
        let pct = if total == 0 { 0 } else { (done * 100 / total).min(100) };
        Some((crate::strings::t("status.transfers", &[("arrow", arrow.into()), ("count", active.len().to_string()), ("pct", pct.to_string())]), "accent"))
    }
}

/// One file of a transfer: where from, where to, how big.
struct Item {
    from: String,
    to: String,
    size: u64,
    mtime: Option<u64>,
    mode: Option<u32>,
}

/// The plan for a transfer: folders to make (in order) and files to copy.
struct Plan {
    dirs: Vec<(String, Option<u32>)>,
    files: Vec<Item>,
}

fn local_plan(from: &Path, to: String) -> Result<Plan> {
    let meta = std::fs::metadata(from).map_err(SshError::io)?;
    let mode = |m: &std::fs::Metadata| -> Option<u32> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            Some(m.permissions().mode() & 0o7777)
        }
        #[cfg(not(unix))]
        {
            let _ = m;
            None
        }
    };
    let mtime = |m: &std::fs::Metadata| m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs());
    if !meta.is_dir() {
        return Ok(Plan { dirs: Vec::new(), files: vec![Item { from: from.to_string_lossy().into_owned(), to, size: meta.len(), mtime: mtime(&meta), mode: mode(&meta) }] });
    }
    let mut plan = Plan { dirs: vec![(to.clone(), mode(&meta))], files: Vec::new() };
    let mut stack = vec![(from.to_path_buf(), to)];
    while let Some((dir, remote)) = stack.pop() {
        let mut entries: Vec<_> = std::fs::read_dir(&dir).map_err(SshError::io)?.filter_map(|e| e.ok()).collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let name = e.file_name().to_string_lossy().into_owned();
            let path = e.path();
            // Links are followed, as scp and sftp do; broken ones are skipped.
            let Ok(m) = std::fs::metadata(&path) else { continue };
            let target = remote_join(&remote, &name);
            if m.is_dir() {
                plan.dirs.push((target.clone(), mode(&m)));
                stack.push((path, target));
            } else if m.is_file() {
                plan.files.push(Item { from: path.to_string_lossy().into_owned(), to: target, size: m.len(), mtime: mtime(&m), mode: mode(&m) });
            }
        }
    }
    Ok(plan)
}

async fn remote_plan(sftp: &SftpSession, from: &str, to: PathBuf) -> Result<Plan> {
    let meta = sftp.metadata(from.to_owned()).await.map_err(SshError::sftp)?;
    let path = |p: &PathBuf| p.to_string_lossy().into_owned();
    if !meta.is_dir() {
        return Ok(Plan { dirs: Vec::new(), files: vec![Item { from: from.to_owned(), to: path(&to), size: meta.size.unwrap_or(0), mtime: meta.mtime.map(u64::from), mode: meta.permissions.map(|p| p & 0o7777) }] });
    }
    let mut plan = Plan { dirs: vec![(path(&to), meta.permissions.map(|p| p & 0o7777))], files: Vec::new() };
    let mut stack = vec![(from.to_owned(), to)];
    while let Some((dir, local)) = stack.pop() {
        let mut entries: Vec<_> = sftp.read_dir(dir.clone()).await.map_err(SshError::sftp)?.collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let name = e.file_name();
            if name == "." || name == ".." || crate::sftp::check_name(&name).is_err() {
                continue;
            }
            let remote = remote_join(&dir, &name);
            let mut m = e.metadata();
            if m.is_symlink() {
                match sftp.metadata(remote.clone()).await {
                    Ok(t) => m = t,
                    Err(_) => continue,
                }
            }
            let target = local.join(&name);
            if m.is_dir() {
                plan.dirs.push((path(&target), m.permissions.map(|p| p & 0o7777)));
                stack.push((remote, target));
            } else if m.is_regular() {
                plan.files.push(Item { from: remote, to: path(&target), size: m.size.unwrap_or(0), mtime: m.mtime.map(u64::from), mode: m.permissions.map(|p| p & 0o7777) });
            }
        }
    }
    Ok(plan)
}

fn part_name(to: &str) -> String {
    format!("{to}{PART}")
}

async fn upload_file(sftp: &SftpSession, job: &Job, item: &Item, preserve: bool) -> Result<()> {
    let part = part_name(&item.to);
    let resume = match sftp.metadata(part.clone()).await {
        Ok(m) if m.is_regular() && m.size.unwrap_or(0) > 0 && m.size.unwrap_or(0) <= item.size => m.size.unwrap_or(0),
        _ => 0,
    };
    let flags = if resume > 0 { OpenFlags::WRITE } else { OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE };
    let mut remote = sftp.open_with_flags(part.clone(), flags).await.map_err(SshError::sftp)?;
    let mut local = tokio::fs::File::open(&item.from).await.map_err(SshError::io)?;
    if resume > 0 {
        remote.seek(std::io::SeekFrom::Start(resume)).await.map_err(SshError::io)?;
        local.seek(std::io::SeekFrom::Start(resume)).await.map_err(SshError::io)?;
        note_resume(job, resume);
    }
    let mut buf = vec![0u8; CHUNK];
    loop {
        job.cancelled()?;
        let n = local.read(&mut buf).await.map_err(SshError::io)?;
        if n == 0 {
            break;
        }
        remote.write_all(&buf[..n]).await.map_err(SshError::io)?;
        job.add(n as u64);
    }
    remote.shutdown().await.map_err(SshError::io)?;
    let _ = remote.close().await;
    // SFTP v3 rename refuses an existing target.
    if let Ok(m) = sftp.symlink_metadata(item.to.clone()).await
        && !m.is_dir()
    {
        sftp.remove_file(item.to.clone()).await.map_err(SshError::sftp)?;
    }
    sftp.rename(part, item.to.clone()).await.map_err(SshError::sftp)?;
    if preserve {
        let mut attrs = FileAttributes::empty();
        attrs.permissions = item.mode;
        if let Some(t) = item.mtime {
            attrs.mtime = Some(t as u32);
            attrs.atime = Some(t as u32);
        }
        let _ = sftp.set_metadata(item.to.clone(), attrs).await;
    }
    Ok(())
}

async fn download_file(sftp: &SftpSession, job: &Job, item: &Item, preserve: bool) -> Result<()> {
    let part = PathBuf::from(part_name(&item.to));
    let resume = match tokio::fs::metadata(&part).await {
        Ok(m) if m.is_file() && m.len() > 0 && m.len() <= item.size => m.len(),
        _ => 0,
    };
    let mut remote = sftp.open_with_flags(item.from.clone(), OpenFlags::READ).await.map_err(SshError::sftp)?;
    let mut local = if resume > 0 {
        let mut f = tokio::fs::OpenOptions::new().write(true).open(&part).await.map_err(SshError::io)?;
        f.seek(std::io::SeekFrom::Start(resume)).await.map_err(SshError::io)?;
        remote.seek(std::io::SeekFrom::Start(resume)).await.map_err(SshError::io)?;
        note_resume(job, resume);
        f
    } else {
        tokio::fs::File::create(&part).await.map_err(SshError::io)?
    };
    let mut buf = vec![0u8; CHUNK];
    loop {
        job.cancelled()?;
        let n = remote.read(&mut buf).await.map_err(SshError::io)?;
        if n == 0 {
            break;
        }
        local.write_all(&buf[..n]).await.map_err(SshError::io)?;
        job.add(n as u64);
    }
    local.flush().await.map_err(SshError::io)?;
    local.sync_all().await.map_err(SshError::io)?;
    drop(local);
    let _ = remote.close().await;
    tokio::fs::rename(&part, &item.to).await.map_err(SshError::io)?;
    if preserve {
        let to = PathBuf::from(&item.to);
        let (mode, mtime) = (item.mode, item.mtime);
        let _ = tokio::task::spawn_blocking(move || {
            #[cfg(unix)]
            if let Some(mode) = mode {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&to, std::fs::Permissions::from_mode(mode));
            }
            #[cfg(not(unix))]
            let _ = mode;
            if let Some(t) = mtime {
                let when = UNIX_EPOCH + Duration::from_secs(t);
                if let Ok(f) = std::fs::File::options().write(true).open(&to) {
                    let _ = f.set_times(std::fs::FileTimes::new().set_modified(when).set_accessed(when));
                }
            }
        })
        .await;
    }
    Ok(())
}

fn note_resume(job: &Job, from: u64) {
    job.done.fetch_add(from, Ordering::Relaxed);
    job.edit(|i| {
        if !i.resumed {
            i.resumed = true;
            i.resumed_from = from;
        }
    });
    let mut speed = job.speed.lock().unwrap_or_else(|e| e.into_inner());
    *speed = (Instant::now(), job.done.load(Ordering::Relaxed), 0.0);
}

impl Manager {
    /// Queues a transfer; returns its id at once. It starts when a slot frees.
    pub fn enqueue(self: &Arc<Self>, spec: TransferSpec) -> Result<u64> {
        if spec.from.trim().is_empty() || spec.to_dir.trim().is_empty() {
            return Err(SshError::text(ErrorKind::Invalid, "bad-path", &[]));
        }
        let def = self.lookup(&spec.host)?;
        let id = self.transfers.next.fetch_add(1, Ordering::Relaxed);
        let name = base_name(&spec.from);
        crate::sftp::check_name(&name)?;
        let to = match spec.direction {
            Direction::Upload => remote_join(&spec.to_dir, &name),
            Direction::Download => Path::new(&spec.to_dir).join(&name).to_string_lossy().into_owned(),
        };
        let info = TransferInfo {
            id,
            host: def.id.clone(),
            direction: spec.direction,
            name,
            from: spec.from.clone(),
            to,
            preserve: spec.preserve,
            state: State::Queued,
            total: 0,
            done: 0,
            rate: 0,
            files: 0,
            files_done: 0,
            resumed: false,
            resumed_from: 0,
            error: None,
            queued_at: now_ms(),
            finished_at: None,
        };
        let job = Arc::new(Job { spec: TransferSpec { host: def.id, ..spec }, info: Mutex::new(info), done: AtomicU64::new(0), cancel: AtomicBool::new(false), speed: Mutex::new((Instant::now(), 0, 0.0)) });
        self.transfers.jobs.lock().unwrap_or_else(|e| e.into_inner()).push(job.clone());
        self.start(job);
        Ok(id)
    }

    /// Runs a failed or cancelled transfer again (resuming its `.part`); its new id.
    pub fn retry(self: &Arc<Self>, id: u64) -> Result<u64> {
        let job = self.transfers.find(id).ok_or_else(|| SshError::text(ErrorKind::Invalid, "no-transfer", &[]))?;
        if !matches!(job.info().state, State::Failed | State::Cancelled) {
            return Err(SshError::text(ErrorKind::Invalid, "no-transfer", &[]));
        }
        self.transfers.jobs.lock().unwrap_or_else(|e| e.into_inner()).retain(|j| j.info().id != id);
        self.enqueue(job.spec.clone())
    }

    fn start(self: &Arc<Self>, job: Arc<Job>) {
        let me = self.clone();
        crate::runtime::handle().spawn(async move {
            let Ok(_slot) = me.transfers.slots.clone().acquire_owned().await else { return };
            let outcome = match job.cancelled() {
                Ok(()) => {
                    job.edit(|i| i.state = State::Running);
                    me.run(&job).await
                }
                Err(e) => Err(e),
            };
            job.edit(|i| {
                i.finished_at = Some(now_ms());
                i.rate = 0;
                match outcome {
                    Ok(()) => i.state = State::Done,
                    Err(e) if e.kind == ErrorKind::Cancelled => i.state = State::Cancelled,
                    Err(e) => {
                        i.state = State::Failed;
                        i.error = Some(e.message);
                    }
                }
            });
        });
    }

    async fn run(&self, job: &Job) -> Result<()> {
        let session = self.session(&job.spec.host, Interaction::Ask).await?;
        let sftp = session.sftp().await?;
        let to = job.info().to;
        let plan = match job.spec.direction {
            Direction::Upload => {
                let from = PathBuf::from(&job.spec.from);
                tokio::task::spawn_blocking(move || local_plan(&from, to)).await.map_err(|e| SshError::new(ErrorKind::Io, e.to_string()))??
            }
            Direction::Download => remote_plan(&sftp, &job.spec.from, PathBuf::from(to)).await?,
        };
        let total = plan.files.iter().map(|f| f.size).sum();
        job.edit(|i| {
            i.total = total;
            i.files = plan.files.len() as u32;
        });
        for (dir, mode) in &plan.dirs {
            job.cancelled()?;
            match job.spec.direction {
                Direction::Upload => {
                    if !sftp.try_exists(dir.clone()).await.unwrap_or(false) {
                        sftp.create_dir(dir.clone()).await.map_err(SshError::sftp)?;
                    }
                    if job.spec.preserve && mode.is_some() {
                        let mut attrs = FileAttributes::empty();
                        attrs.permissions = *mode;
                        let _ = sftp.set_metadata(dir.clone(), attrs).await;
                    }
                }
                Direction::Download => tokio::fs::create_dir_all(dir).await.map_err(SshError::io)?,
            }
        }
        for item in &plan.files {
            job.cancelled()?;
            match job.spec.direction {
                Direction::Upload => upload_file(&sftp, job, item, job.spec.preserve).await?,
                Direction::Download => download_file(&sftp, job, item, job.spec.preserve).await?,
            }
            job.edit(|i| i.files_done += 1);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_local_folder_plans_folders_before_their_files() {
        let dir = std::env::temp_dir().join(format!("yuzora-ssh-plan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("site/assets")).unwrap();
        std::fs::write(dir.join("site/index.html"), "<p>").unwrap();
        std::fs::write(dir.join("site/assets/app.js"), "1;").unwrap();
        let plan = local_plan(&dir.join("site"), "/srv/site".into()).unwrap();
        let dirs: Vec<&str> = plan.dirs.iter().map(|(d, _)| d.as_str()).collect();
        assert_eq!(dirs, ["/srv/site", "/srv/site/assets"]);
        let mut files: Vec<(&str, u64)> = plan.files.iter().map(|f| (f.to.as_str(), f.size)).collect();
        files.sort();
        assert_eq!(files, [("/srv/site/assets/app.js", 2), ("/srv/site/index.html", 3)]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
