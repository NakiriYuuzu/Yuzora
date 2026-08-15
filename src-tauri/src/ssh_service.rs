use std::collections::{BTreeMap, HashMap};
use std::net::{Ipv4Addr, Ipv6Addr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use russh::client::{self, Handle};
use russh::keys::{load_secret_key, HashAlg, PrivateKeyWithHashAlg, PublicKey};
use russh::{ChannelMsg, Disconnect};
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::OpenFlags;
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Mutex as AsyncMutex;
use tokio::sync::{mpsc, oneshot, Notify};

use crate::logging;
use crate::path_capability::{self, PathCapabilityError, SafeLeafName, SafeRelativePath};

// russh's connect() has no built-in dial timeout; wrap it so a black-holed host
// fails fast instead of hanging the connect command indefinitely.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

type LogFn = Box<dyn Fn(logging::LogEvent) + Send + Sync>;
static NEXT_SESSION_ID: AtomicU64 = AtomicU64::new(1);

/// Authentication input from the front-end. Password is prompted per connection
/// and never persisted; key auth loads a private key file at connect time.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SshAuth {
    Password {
        password: String,
    },
    Key {
        #[serde(rename = "keyPath")]
        key_path: String,
        passphrase: Option<String>,
    },
}

/// Result of a successful connect: the opaque session id, the server's SHA256
/// host-key fingerprint, and whether that key matched a previously-pinned entry
/// in the known-hosts store (`false` on first contact). Surfaced so the UI can
/// show the fingerprint and flag a first-seen vs. an already-known host.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SshConnectResult {
    pub session_id: String,
    pub fingerprint: String,
    pub known_host: bool,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SshDataPayload {
    session_id: String,
    chunk: String,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SshExitPayload {
    session_id: String,
}

/// Format an SSH public key as its OpenSSH `SHA256:<base64>` fingerprint — the
/// same string `ssh-keygen -lf` prints. Pure so it can be unit-tested.
pub fn fingerprint_sha256(key: &PublicKey) -> String {
    key.fingerprint(HashAlg::Sha256).to_string()
}

/// Persistent known-hosts store path: `~/.yuzora/known_hosts.json`, mirroring the
/// logging dir convention (`logging::default_log_dir`).
fn default_known_hosts_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".yuzora")
        .join("known_hosts.json")
}

const HOST_KEY_CHALLENGE_TIMEOUT: Duration = Duration::from_secs(60);
const HOST_KEY_PROMPT_EVENT: &str = "ssh://host-key-prompt";
const HOST_KEY_LOCK_FILE_NAME: &str = "known_hosts.lock";

/// Stable known-hosts map key for a host endpoint.
///
/// Hostnames are lowercased and stripped of a trailing dot. IPv6 literals are
/// always wrapped in brackets so `::1:22` cannot be confused with host `::1`
/// on port 22. The port is always included, including the SSH default 22.
fn canonical_endpoint(host: &str, port: u16) -> String {
    let host = host.trim();
    let host = host
        .strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
        .unwrap_or(host);
    if host.parse::<Ipv6Addr>().is_ok() || host.contains(':') {
        format!("[{}]:{port}", host.to_ascii_lowercase())
    } else if host.parse::<Ipv4Addr>().is_ok() {
        format!("{host}:{port}")
    } else {
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        format!("{host}:{port}")
    }
}

#[cfg(test)]
fn host_port_key(host: &str, port: u16) -> String {
    canonical_endpoint(host, port)
}

/// Read the pinned known-hosts store from `path`. A missing file is a legitimate
/// first-run state → empty map. A file that exists but can't be read, or whose
/// contents don't parse into the expected map, is *corrupt* → `Err`, so a damaged
/// store fails closed (see `check_server_key`) instead of silently degrading to
/// first-seen trust and re-pinning a possibly-hostile key.
fn read_known_hosts(path: &Path) -> Result<BTreeMap<String, String>, String> {
    match std::fs::read_to_string(path) {
        Ok(content) => parse_known_hosts(&content),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(e) => Err(format!("無法讀取 known_hosts 檔案 {}：{e}", path.display())),
    }
}

/// Parse known-hosts store contents (`"host:port"` → SHA256 fingerprint). Empty
/// (or whitespace-only) content is an empty store; any other content that isn't a
/// JSON object of `string → string` is corrupt → `Err`.
fn parse_known_hosts(content: &str) -> Result<BTreeMap<String, String>, String> {
    if content.trim().is_empty() {
        return Ok(BTreeMap::new());
    }
    serde_json::from_str(content).map_err(|e| format!("known_hosts 內容無法解析：{e}"))
}

/// Serialize the known-hosts store deterministically (BTreeMap → sorted keys).
fn serialize_known_hosts(hosts: &BTreeMap<String, String>) -> Result<String, String> {
    serde_json::to_string_pretty(hosts).map_err(|e| format!("無法序列化 known_hosts：{e}"))
}

/// TOFU decision for a presented host key given what (if anything) is pinned.
#[derive(Debug, PartialEq, Eq)]
enum HostKeyDecision {
    /// Endpoint not seen before — pause for an explicit, durable accept.
    New,
    /// Presented key matches the pinned fingerprint — trust silently.
    Match,
    /// Presented key differs from the pinned fingerprint — reject (a re-keyed
    /// server or a MITM); the handshake is aborted before any credential.
    Changed,
}

fn decide_host_key(pinned: Option<&str>, presented: &str) -> HostKeyDecision {
    match pinned {
        None => HostKeyDecision::New,
        Some(fp) if fp == presented => HostKeyDecision::Match,
        Some(_) => HostKeyDecision::Changed,
    }
}

/// The full host-key verdict, folding the store's read result into match /
/// changed / new. Pure over `read`, so the fail-closed branches are unit-tested
/// without a live handshake. A `New` result does *not* pin anything.
#[derive(Debug, PartialEq, Eq)]
enum HostKeyEval {
    Match,
    New,
    Changed { previous: String },
    Corrupt { reason: String },
}

fn evaluate_host_key(
    read: Result<BTreeMap<String, String>, String>,
    endpoint: &str,
    presented: &str,
) -> HostKeyEval {
    let hosts = match read {
        Ok(hosts) => hosts,
        // A corrupt store must never be read as "nothing pinned": fail closed so
        // a changed/hostile key is not silently accepted and re-pinned.
        Err(reason) => return HostKeyEval::Corrupt { reason },
    };
    match decide_host_key(hosts.get(endpoint).map(String::as_str), presented) {
        HostKeyDecision::Changed => HostKeyEval::Changed {
            previous: hosts.get(endpoint).cloned().unwrap_or_default(),
        },
        HostKeyDecision::Match => HostKeyEval::Match,
        HostKeyDecision::New => HostKeyEval::New,
    }
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PersistFailPoint {
    Lock,
    Write,
    Fsync,
    Rename,
    ParentSync,
    Unlock,
}

trait HostKeyPersistIo: Send + Sync {
    fn create_dir_all(&self, path: &Path) -> std::io::Result<()>;
    fn lock_file(&self, file: &std::fs::File) -> std::io::Result<()>;
    fn unlock_file(&self, file: &std::fs::File) -> std::io::Result<()>;
    fn create_temp(&self, parent: &Path, contents: &[u8]) -> std::io::Result<PathBuf>;
    fn set_restrictive_permissions(&self, path: &Path) -> std::io::Result<()>;
    fn sync_file(&self, path: &Path) -> std::io::Result<()>;
    fn rename(&self, from: &Path, to: &Path) -> std::io::Result<()>;
    fn sync_dir(&self, path: &Path) -> std::io::Result<()>;
}

struct StdHostKeyIo;

impl HostKeyPersistIo for StdHostKeyIo {
    fn create_dir_all(&self, path: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(path)
    }

    fn lock_file(&self, file: &std::fs::File) -> std::io::Result<()> {
        lock_known_hosts_file(file)
    }

    fn unlock_file(&self, file: &std::fs::File) -> std::io::Result<()> {
        unlock_known_hosts_file(file)
    }

    fn create_temp(&self, parent: &Path, contents: &[u8]) -> std::io::Result<PathBuf> {
        use std::io::Write;
        let tmp = parent.join(format!(".known_hosts-{}.tmp", uuid::Uuid::new_v4()));
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(contents)?;
        file.flush()?;
        Ok(tmp)
    }

    fn set_restrictive_permissions(&self, path: &Path) -> std::io::Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
        #[cfg(not(unix))]
        {
            let _ = path;
        }
        Ok(())
    }

    fn sync_file(&self, path: &Path) -> std::io::Result<()> {
        std::fs::File::open(path)?.sync_all()
    }

    fn rename(&self, from: &Path, to: &Path) -> std::io::Result<()> {
        std::fs::rename(from, to)
    }

    fn sync_dir(&self, path: &Path) -> std::io::Result<()> {
        #[cfg(unix)]
        {
            std::fs::File::open(path)?.sync_all()
        }
        #[cfg(not(unix))]
        {
            let _ = path;
            Ok(())
        }
    }
}

#[cfg(test)]
struct FaultyHostKeyIo {
    inner: StdHostKeyIo,
    fail: Mutex<Option<PersistFailPoint>>,
}

#[cfg(test)]
impl FaultyHostKeyIo {
    fn new(point: PersistFailPoint) -> Self {
        Self {
            inner: StdHostKeyIo,
            fail: Mutex::new(Some(point)),
        }
    }

    fn fail_if(&self, point: PersistFailPoint) -> std::io::Result<()> {
        let mut slot = self.fail.lock().unwrap();
        if *slot == Some(point) {
            *slot = None;
            return Err(std::io::Error::other(format!(
                "injected known_hosts failure: {point:?}"
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
impl HostKeyPersistIo for FaultyHostKeyIo {
    fn create_dir_all(&self, path: &Path) -> std::io::Result<()> {
        self.inner.create_dir_all(path)
    }

    fn lock_file(&self, file: &std::fs::File) -> std::io::Result<()> {
        self.fail_if(PersistFailPoint::Lock)?;
        self.inner.lock_file(file)
    }

    fn unlock_file(&self, file: &std::fs::File) -> std::io::Result<()> {
        self.fail_if(PersistFailPoint::Unlock)?;
        self.inner.unlock_file(file)
    }

    fn create_temp(&self, parent: &Path, contents: &[u8]) -> std::io::Result<PathBuf> {
        self.fail_if(PersistFailPoint::Write)?;
        self.inner.create_temp(parent, contents)
    }

    fn set_restrictive_permissions(&self, path: &Path) -> std::io::Result<()> {
        self.inner.set_restrictive_permissions(path)
    }

    fn sync_file(&self, path: &Path) -> std::io::Result<()> {
        self.fail_if(PersistFailPoint::Fsync)?;
        self.inner.sync_file(path)
    }

    fn rename(&self, from: &Path, to: &Path) -> std::io::Result<()> {
        self.fail_if(PersistFailPoint::Rename)?;
        self.inner.rename(from, to)
    }

    fn sync_dir(&self, path: &Path) -> std::io::Result<()> {
        self.fail_if(PersistFailPoint::ParentSync)?;
        self.inner.sync_dir(path)
    }
}

struct KnownHostsFileLock {
    file: std::fs::File,
    locked: bool,
}

impl KnownHostsFileLock {
    fn release(mut self, io: &dyn HostKeyPersistIo) -> Result<(), String> {
        io.unlock_file(&self.file)
            .map_err(|e| format!("無法解除 known_hosts 鎖定：{e}"))?;
        self.locked = false;
        Ok(())
    }
}

impl Drop for KnownHostsFileLock {
    fn drop(&mut self) {
        if self.locked {
            let _ = unlock_known_hosts_file(&self.file);
        }
    }
}

fn acquire_known_hosts_file_lock(
    store_path: &Path,
    io: &dyn HostKeyPersistIo,
) -> Result<KnownHostsFileLock, String> {
    let parent = store_path
        .parent()
        .ok_or_else(|| "known_hosts 路徑沒有父目錄".to_string())?;
    io.create_dir_all(parent)
        .map_err(|e| format!("無法建立 known_hosts 目錄：{e}"))?;
    let lock_path = parent.join(HOST_KEY_LOCK_FILE_NAME);
    let file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(&lock_path)
        .map_err(|e| format!("無法開啟 known_hosts 鎖定檔：{e}"))?;
    io.lock_file(&file)
        .map_err(|e| format!("無法鎖定 known_hosts：{e}"))?;
    Ok(KnownHostsFileLock { file, locked: true })
}

#[cfg(unix)]
fn lock_known_hosts_file(file: &std::fs::File) -> std::io::Result<()> {
    use std::os::unix::io::AsRawFd;

    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(unix)]
fn unlock_known_hosts_file(file: &std::fs::File) -> std::io::Result<()> {
    use std::os::unix::io::AsRawFd;

    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(windows)]
#[repr(C)]
struct HostKeyOverlapped {
    internal: usize,
    internal_high: usize,
    offset: u32,
    offset_high: u32,
    h_event: *mut core::ffi::c_void,
}

#[cfg(windows)]
fn host_key_overlapped() -> HostKeyOverlapped {
    HostKeyOverlapped {
        internal: 0,
        internal_high: 0,
        offset: 0,
        offset_high: 0,
        h_event: std::ptr::null_mut(),
    }
}

#[cfg(windows)]
fn lock_known_hosts_file(file: &std::fs::File) -> std::io::Result<()> {
    use std::os::windows::io::AsRawHandle;

    extern "system" {
        fn LockFileEx(
            file: *mut core::ffi::c_void,
            flags: u32,
            reserved: u32,
            bytes_to_lock_low: u32,
            bytes_to_lock_high: u32,
            overlapped: *mut HostKeyOverlapped,
        ) -> i32;
    }
    const LOCKFILE_EXCLUSIVE_LOCK: u32 = 0x0000_0002;
    let mut overlapped = host_key_overlapped();
    let ok = unsafe {
        LockFileEx(
            file.as_raw_handle(),
            LOCKFILE_EXCLUSIVE_LOCK,
            0,
            1,
            0,
            &mut overlapped,
        )
    };
    if ok == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(windows)]
fn unlock_known_hosts_file(file: &std::fs::File) -> std::io::Result<()> {
    use std::os::windows::io::AsRawHandle;

    extern "system" {
        fn UnlockFileEx(
            file: *mut core::ffi::c_void,
            reserved: u32,
            bytes_to_unlock_low: u32,
            bytes_to_unlock_high: u32,
            overlapped: *mut HostKeyOverlapped,
        ) -> i32;
    }
    let mut overlapped = host_key_overlapped();
    let ok = unsafe { UnlockFileEx(file.as_raw_handle(), 0, 1, 0, &mut overlapped) };
    if ok == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(any(unix, windows)))]
fn lock_known_hosts_file(_file: &std::fs::File) -> std::io::Result<()> {
    Err(std::io::Error::other(
        "cross-process known_hosts lock is unavailable",
    ))
}

#[cfg(not(any(unix, windows)))]
fn unlock_known_hosts_file(_file: &std::fs::File) -> std::io::Result<()> {
    Err(std::io::Error::other(
        "cross-process known_hosts unlock is unavailable",
    ))
}

/// Atomic known-hosts write: same-directory temp, 0600, flush/sync, rename,
/// parent sync. Any step failing is an error; the caller must fail closed.
fn persist_known_hosts(
    path: &Path,
    hosts: &BTreeMap<String, String>,
    io: &dyn HostKeyPersistIo,
) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "known_hosts 路徑沒有父目錄".to_string())?;
    io.create_dir_all(parent)
        .map_err(|e| format!("無法建立 known_hosts 目錄：{e}"))?;
    let body = serialize_known_hosts(hosts)?;
    let tmp = io
        .create_temp(parent, body.as_bytes())
        .map_err(|e| format!("無法寫入 known_hosts 暫存檔：{e}"))?;
    let persist_result = (|| {
        io.set_restrictive_permissions(&tmp)
            .map_err(|e| format!("無法設定 known_hosts 權限：{e}"))?;
        io.sync_file(&tmp)
            .map_err(|e| format!("無法同步 known_hosts 暫存檔：{e}"))?;
        io.rename(&tmp, path)
            .map_err(|e| format!("無法寫入 known_hosts：{e}"))?;
        io.sync_dir(parent)
            .map_err(|e| format!("無法同步 known_hosts 目錄：{e}"))?;
        Ok(())
    })();
    if persist_result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    persist_result
}

/// Read-merge-write a newly accepted pin while holding both the controller-local
/// mutex and a separate OS-visible lock file. The lock covers load through parent
/// directory sync and is never taken on the JSON inode that atomic rename replaces.
fn persist_accepted_pin(
    path: &Path,
    persist_lock: &Mutex<()>,
    endpoint: &str,
    fingerprint: &str,
    io: &dyn HostKeyPersistIo,
) -> Result<(), String> {
    let _guard = persist_lock
        .lock()
        .map_err(|_| "known_hosts 寫入鎖已損壞".to_string())?;
    let file_lock = acquire_known_hosts_file_lock(path, io)?;
    let transaction = (|| {
        let mut hosts = read_known_hosts(path)?;
        if let Some(existing) = hosts.get(endpoint) {
            if existing != fingerprint {
                return Err(format!(
                    "主機 {endpoint} 已釘選不同的 fingerprint，拒絕覆寫"
                ));
            }
            return Ok(());
        }
        hosts.insert(endpoint.to_string(), fingerprint.to_string());
        persist_known_hosts(path, &hosts, io)
    })();
    let release = file_lock.release(io);
    match (transaction, release) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) => Err(error),
        (Ok(()), Err(error)) => Err(error),
        (Err(error), Err(release_error)) => Err(format!("{error}; {release_error}")),
    }
}

/// What `check_server_key` recorded for the surrounding `connect` to act on.
#[derive(Default)]
struct CheckOutcome {
    fingerprint: Option<String>,
    /// The key matched a previously-pinned entry (vs. first contact).
    known: bool,
    /// The key changed from the pinned fingerprint and was rejected.
    changed: bool,
    previous_fingerprint: Option<String>,
    /// First-use challenge was rejected, cancelled, or timed out.
    challenge_denied: bool,
    /// Durable pin write failed after the user accepted — fail closed.
    persist_failed: Option<String>,
    /// The known-hosts store was corrupt; the handshake was failed closed. Holds
    /// the parse/read reason so `connect` can surface a repair hint.
    corrupt: Option<String>,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
enum SshHostKeyPrompt {
    New {
        challenge_id: String,
        host: String,
        port: u16,
        endpoint: String,
        algorithm: String,
        fingerprint: String,
    },
    Changed {
        host: String,
        port: u16,
        endpoint: String,
        algorithm: String,
        fingerprint: String,
        previous_fingerprint: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HostKeyAnswer {
    Accept,
    Reject,
}

struct PendingHostKeyChallenge {
    #[allow(dead_code)]
    connection_id: String,
    endpoint: String,
    #[allow(dead_code)]
    algorithm: String,
    fingerprint: String,
    expires_at: Instant,
    tx: oneshot::Sender<HostKeyAnswer>,
}

type HostKeyEmit = Arc<dyn Fn(SshHostKeyPrompt) + Send + Sync>;

struct HostKeyController {
    path: PathBuf,
    persist_lock: Mutex<()>,
    pending: Mutex<HashMap<String, PendingHostKeyChallenge>>,
    emit: HostKeyEmit,
    challenge_timeout: Duration,
    persist_io: Arc<dyn HostKeyPersistIo>,
}

impl HostKeyController {
    #[cfg(test)]
    fn new(path: PathBuf, emit: HostKeyEmit, challenge_timeout: Duration) -> Arc<Self> {
        Self::with_io(path, emit, challenge_timeout, Arc::new(StdHostKeyIo))
    }

    fn with_io(
        path: PathBuf,
        emit: HostKeyEmit,
        challenge_timeout: Duration,
        persist_io: Arc<dyn HostKeyPersistIo>,
    ) -> Arc<Self> {
        Arc::new(Self {
            path,
            persist_lock: Mutex::new(()),
            pending: Mutex::new(HashMap::new()),
            emit,
            challenge_timeout,
            persist_io,
        })
    }

    fn emit(&self, prompt: SshHostKeyPrompt) {
        (self.emit)(prompt);
    }

    fn persist_pin(&self, endpoint: &str, fingerprint: &str) -> Result<(), String> {
        persist_accepted_pin(
            &self.path,
            &self.persist_lock,
            endpoint,
            fingerprint,
            self.persist_io.as_ref(),
        )
    }

    fn take_pending(&self, challenge_id: &str) -> Option<PendingHostKeyChallenge> {
        self.pending.lock().unwrap().remove(challenge_id)
    }

    fn reject_all_pending(&self) {
        let pending: Vec<PendingHostKeyChallenge> = {
            let mut map = self.pending.lock().unwrap();
            map.drain().map(|(_, challenge)| challenge).collect()
        };
        for challenge in pending {
            let _ = challenge.tx.send(HostKeyAnswer::Reject);
        }
    }

    fn respond(
        &self,
        challenge_id: &str,
        accept: bool,
        endpoint: &str,
        fingerprint: &str,
    ) -> Result<(), String> {
        let pending = self
            .take_pending(challenge_id)
            .ok_or_else(|| "主機金鑰確認已失效、過期或重複送出".to_string())?;
        if Instant::now() > pending.expires_at {
            let _ = pending.tx.send(HostKeyAnswer::Reject);
            return Err("主機金鑰確認已過期".into());
        }
        if pending.endpoint != endpoint || pending.fingerprint != fingerprint {
            let _ = pending.tx.send(HostKeyAnswer::Reject);
            return Err("主機金鑰確認與連線不符".into());
        }
        let answer = if accept {
            HostKeyAnswer::Accept
        } else {
            HostKeyAnswer::Reject
        };
        pending
            .tx
            .send(answer)
            .map_err(|_| "主機金鑰確認已結束".to_string())
    }

    async fn challenge_and_wait(
        &self,
        connection_id: &str,
        host: &str,
        port: u16,
        endpoint: &str,
        algorithm: &str,
        fingerprint: &str,
    ) -> bool {
        let challenge_id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().unwrap();
            pending.insert(
                challenge_id.clone(),
                PendingHostKeyChallenge {
                    connection_id: connection_id.to_string(),
                    endpoint: endpoint.to_string(),
                    algorithm: algorithm.to_string(),
                    fingerprint: fingerprint.to_string(),
                    expires_at: Instant::now() + self.challenge_timeout,
                    tx,
                },
            );
        }
        self.emit(SshHostKeyPrompt::New {
            challenge_id: challenge_id.clone(),
            host: host.to_string(),
            port,
            endpoint: endpoint.to_string(),
            algorithm: algorithm.to_string(),
            fingerprint: fingerprint.to_string(),
        });
        let decision = tokio::time::timeout(self.challenge_timeout, rx).await;
        match decision {
            Ok(Ok(HostKeyAnswer::Accept)) => true,
            _ => {
                self.take_pending(&challenge_id);
                false
            }
        }
    }
}

/// Host-key handler with known-hosts pinning. A matching pinned key is trusted
/// silently. A changed key is REJECTED here — russh aborts the handshake with
/// `UnknownKey` before any authentication packet. A first-seen key opens a
/// single-use, connection-bound challenge; authentication does not start until
/// the pin is durably stored.
struct Client {
    host: String,
    port: u16,
    connection_id: String,
    host_keys: Arc<HostKeyController>,
    outcome: Arc<Mutex<CheckOutcome>>,
    challenge_pending: Arc<AtomicBool>,
    challenge_notify: Arc<Notify>,
    auth_probe: Option<Arc<Mutex<Vec<&'static str>>>>,
}

impl Client {
    fn note(&self, stage: &'static str) {
        if let Some(probe) = &self.auth_probe {
            probe.lock().unwrap().push(stage);
        }
    }
}

impl client::Handler for Client {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKey,
    ) -> Result<bool, Self::Error> {
        let fp = fingerprint_sha256(server_public_key);
        let algorithm = server_public_key.algorithm().to_string();
        let endpoint = canonical_endpoint(&self.host, self.port);

        match evaluate_host_key(read_known_hosts(&self.host_keys.path), &endpoint, &fp) {
            HostKeyEval::Corrupt { reason } => {
                let mut outcome = self.outcome.lock().unwrap();
                outcome.fingerprint = Some(fp);
                outcome.corrupt = Some(reason);
                Ok(false)
            }
            HostKeyEval::Changed { previous } => {
                self.host_keys.emit(SshHostKeyPrompt::Changed {
                    host: self.host.clone(),
                    port: self.port,
                    endpoint: endpoint.clone(),
                    algorithm,
                    fingerprint: fp.clone(),
                    previous_fingerprint: previous.clone(),
                });
                let mut outcome = self.outcome.lock().unwrap();
                outcome.fingerprint = Some(fp);
                outcome.previous_fingerprint = Some(previous);
                outcome.changed = true;
                Ok(false)
            }
            HostKeyEval::Match => {
                let mut outcome = self.outcome.lock().unwrap();
                outcome.fingerprint = Some(fp);
                outcome.known = true;
                Ok(true)
            }
            HostKeyEval::New => {
                self.note("host_key_challenge");
                self.challenge_pending.store(true, Ordering::SeqCst);
                self.challenge_notify.notify_one();
                let accepted = self
                    .host_keys
                    .challenge_and_wait(
                        &self.connection_id,
                        &self.host,
                        self.port,
                        &endpoint,
                        &algorithm,
                        &fp,
                    )
                    .await;
                if !accepted {
                    let mut outcome = self.outcome.lock().unwrap();
                    outcome.fingerprint = Some(fp);
                    outcome.challenge_denied = true;
                    return Ok(false);
                }
                match self.host_keys.persist_pin(&endpoint, &fp) {
                    Ok(()) => {
                        self.note("host_key_pinned");
                        let mut outcome = self.outcome.lock().unwrap();
                        outcome.fingerprint = Some(fp);
                        outcome.known = false;
                        Ok(true)
                    }
                    Err(reason) => {
                        let mut outcome = self.outcome.lock().unwrap();
                        outcome.fingerprint = Some(fp);
                        outcome.persist_failed = Some(reason);
                        Ok(false)
                    }
                }
            }
        }
    }
}

/// Outbound commands the shell task pumps into the SSH channel. Kept off the
/// Tauri command threads so write/resize return immediately.
enum ShellCmd {
    Data(Vec<u8>),
    Resize(u32, u32),
}

struct SessionEntry {
    handle: Arc<AsyncMutex<Handle<Client>>>,
    shell: Option<mpsc::UnboundedSender<ShellCmd>>,
    /// Lazily-opened SFTP subsystem for this session (F5). `SftpSession` methods
    /// take `&self` and drive an internal request pipeline, so one `Arc` is
    /// shared across concurrent list/transfer commands.
    sftp: Option<Arc<SftpSession>>,
    host: String,
}

/// One remote directory entry (F5). `size` is only meaningful for files.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SftpEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub name_safe: bool,
    pub size: u64,
}

/// Tagged upload source: workspace files are opened through the pinned
/// workspace root; picker/OS-drop files consume a backend-issued capability.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SftpUploadSource {
    Workspace {
        #[serde(rename = "workspaceId")]
        workspace_id: String,
        #[serde(rename = "relativePath")]
        relative_path: String,
    },
    Selected {
        #[serde(rename = "capabilityId")]
        capability_id: String,
    },
}

fn open_sftp_upload_source(
    selected: &path_capability::SelectedPathRegistry,
    workspaces: &path_capability::WorkspacePathRegistry,
    trust: &crate::workspace_trust::WorkspaceTrustState,
    source: SftpUploadSource,
) -> Result<path_capability::OpenedFile, String> {
    match source {
        SftpUploadSource::Workspace {
            workspace_id,
            relative_path,
        } => {
            let canonical_root = workspaces.canonical_root(&workspace_id)?;
            trust.require_trusted(&canonical_root)?;
            workspaces
                .open_file(&workspace_id, &relative_path)
                .map_err(String::from)
        }
        SftpUploadSource::Selected { capability_id } => {
            selected.take(&capability_id).map_err(String::from)
        }
    }
}

/// A directory listing plus the canonical cwd it was read from — the front-end
/// shows `cwd` and derives `..` / navigation from it.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SftpListing {
    pub cwd: String,
    pub entries: Vec<SftpEntry>,
}

/// Progress ticks for an in-flight transfer, emitted on `sftp://progress` and
/// correlated by the front-end-supplied `transfer_id` (mirrors the `ssh://data`
/// event pattern). A terminal tick carries `done: true`.
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SftpProgressPayload {
    session_id: String,
    transfer_id: String,
    transferred: u64,
    total: u64,
    done: bool,
}

// Streamed transfers read/write in 32 KiB slices (safely under russh-sftp's
// 256 KiB default packet cap) so a large file never lands wholesale in memory;
// a progress tick is emitted at most every 256 KiB (plus start and completion).
const SFTP_CHUNK: usize = 32 * 1024;
const SFTP_PROGRESS_STEP: u64 = 256 * 1024;

/// POSIX-join a remote directory with a leaf name (SFTP is always `/`-separated,
/// regardless of the local platform). Pure, so the path math is unit-tested.
fn reject_unsafe_remote_leaf(path: &str) -> Result<(), String> {
    let mut saw_name = false;
    for (index, name) in path.split('/').enumerate() {
        if name.is_empty() {
            if index == 0 {
                continue;
            }
            return Err(PathCapabilityError::UnsafeLeaf.into());
        }
        SafeLeafName::parse(name)?;
        saw_name = true;
    }
    if !saw_name {
        return Err(PathCapabilityError::UnsafeLeaf.into());
    }
    Ok(())
}

fn remote_join(dir: &str, name: &str) -> String {
    if dir.is_empty() || dir == "/" {
        format!("/{name}")
    } else if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

/// Temp-sibling file name for an atomic transfer: `"<name>.yz-tmp-<token>"`. The
/// caller joins it into the destination directory (remote via `remote_join`,
/// local via `Path::join`) so an interrupted transfer streams into this scratch
/// file and is only renamed onto the real target after a clean flush. `token` is
/// the unique transfer id, so it never collides with an existing file. Pure, so
/// name generation is unit-tested.
fn temp_transfer_name(name: &str, token: &str) -> String {
    format!("{name}.yz-tmp-{token}")
}

/// Directories first, then case-insensitive by name — matching the local tree's
/// ordering (`fs_service::list_dir_entries`). Pure, so it's unit-tested.
fn sort_sftp_entries(entries: &mut [SftpEntry]) {
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
}

pub struct SshManager {
    sessions: Mutex<HashMap<String, SessionEntry>>,
    log: LogFn,
    transfer_dests: path_capability::TransferDestSet,
    host_keys: Arc<HostKeyController>,
    auth_probe: Option<Arc<Mutex<Vec<&'static str>>>>,
}

pub struct SshState(pub Arc<SshManager>);

impl SshManager {
    pub fn new(app: AppHandle) -> Self {
        let emit_app = app.clone();
        Self::with_parts(
            Box::new(logging::write_global),
            default_known_hosts_path(),
            Arc::new(move |prompt| {
                let _ = emit_app.emit(HOST_KEY_PROMPT_EVENT, prompt);
            }),
            HOST_KEY_CHALLENGE_TIMEOUT,
            Arc::new(StdHostKeyIo),
        )
    }

    #[cfg(test)]
    fn for_test() -> Self {
        Self::with_log(Box::new(|_| {}))
    }

    #[cfg(test)]
    fn with_log(log: LogFn) -> Self {
        Self::with_parts(
            log,
            default_known_hosts_path(),
            Arc::new(|_| {}),
            HOST_KEY_CHALLENGE_TIMEOUT,
            Arc::new(StdHostKeyIo),
        )
    }

    fn with_parts(
        log: LogFn,
        known_hosts_path: PathBuf,
        emit: HostKeyEmit,
        challenge_timeout: Duration,
        persist_io: Arc<dyn HostKeyPersistIo>,
    ) -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            log,
            host_keys: HostKeyController::with_io(
                known_hosts_path,
                emit,
                challenge_timeout,
                persist_io,
            ),
            transfer_dests: path_capability::TransferDestSet::new(),
            auth_probe: None,
        }
    }

    fn respond_host_key(
        &self,
        challenge_id: &str,
        accept: bool,
        endpoint: &str,
        fingerprint: &str,
    ) -> Result<(), String> {
        self.host_keys
            .respond(challenge_id, accept, endpoint, fingerprint)
    }

    fn host_key_failure_message(
        host: &str,
        port: u16,
        outcome: &CheckOutcome,
        transport: &russh::Error,
    ) -> String {
        let endpoint = canonical_endpoint(host, port);
        if let Some(reason) = &outcome.corrupt {
            format!(
                "known_hosts 檔案損毀，連線已中止（{reason}）；請修復或重設 ~/.yuzora/known_hosts.json 後再試"
            )
        } else if outcome.changed {
            let previous = outcome
                .previous_fingerprint
                .as_deref()
                .unwrap_or("（未知）");
            let presented = outcome.fingerprint.as_deref().unwrap_or("（未知）");
            format!(
                "主機金鑰驗證失敗：{endpoint} 的 fingerprint 已變更。已記錄：{previous}；目前：{presented}。連線已中止（伺服器金鑰可能已更換，或遭到中間人攻擊）"
            )
        } else if let Some(reason) = &outcome.persist_failed {
            format!("無法保存主機金鑰，連線已中止（{reason}）")
        } else if outcome.challenge_denied {
            format!("使用者拒絕或逾時未確認主機金鑰，連線已中止（{endpoint}）")
        } else {
            format!("無法連線到 {endpoint}：{transport}")
        }
    }

    fn note_auth(&self, stage: &'static str) {
        if let Some(probe) = &self.auth_probe {
            probe.lock().unwrap().push(stage);
        }
    }

    async fn connect(
        &self,
        host: String,
        port: u16,
        user: String,
        auth: SshAuth,
    ) -> Result<SshConnectResult, String> {
        let config = Arc::new(client::Config::default());
        let outcome = Arc::new(Mutex::new(CheckOutcome::default()));
        let challenge_pending = Arc::new(AtomicBool::new(false));
        let challenge_notify = Arc::new(Notify::new());
        let connection_id = uuid::Uuid::new_v4().to_string();
        let handler = Client {
            host: host.clone(),
            port,
            connection_id,
            host_keys: self.host_keys.clone(),
            outcome: outcome.clone(),
            challenge_pending: challenge_pending.clone(),
            challenge_notify: challenge_notify.clone(),
            auth_probe: self.auth_probe.clone(),
        };

        let connect_fut = client::connect(config, (host.clone(), port), handler);
        tokio::pin!(connect_fut);
        let connect_deadline = tokio::time::sleep(CONNECT_TIMEOUT);
        tokio::pin!(connect_deadline);
        let mut challenge_deadline =
            std::pin::pin!(tokio::time::sleep(self.host_keys.challenge_timeout));
        let mut challenge_armed = false;

        let mut session = loop {
            tokio::select! {
                biased;
                result = &mut connect_fut => {
                    break match result {
                        Err(e) => {
                            // A rejected host key aborts the handshake with `UnknownKey`
                            // before any credential is sent — surface it as a distinct,
                            // actionable warning rather than a generic transport error.
                            let outcome = outcome.lock().unwrap();
                            let msg = Self::host_key_failure_message(&host, port, &outcome, &e);
                            self.log_connect_failure(&host, port, &user, &msg);
                            return Err(msg);
                        }
                        Ok(session) => session,
                    };
                }
                _ = challenge_notify.notified(), if !challenge_armed => {
                    challenge_armed = true;
                    challenge_deadline.as_mut().reset(
                        tokio::time::Instant::now() + self.host_keys.challenge_timeout,
                    );
                }
                _ = &mut connect_deadline, if !challenge_armed => {
                    if challenge_pending.load(Ordering::SeqCst) {
                        challenge_armed = true;
                        challenge_deadline.as_mut().reset(
                            tokio::time::Instant::now() + self.host_keys.challenge_timeout,
                        );
                        continue;
                    }
                    let msg = format!(
                        "連線逾時：{} 在 {} 秒內沒有回應",
                        canonical_endpoint(&host, port),
                        CONNECT_TIMEOUT.as_secs()
                    );
                    self.log_connect_failure(&host, port, &user, &msg);
                    return Err(msg);
                }
                _ = &mut challenge_deadline, if challenge_armed => {
                    let msg = format!(
                        "使用者拒絕或逾時未確認主機金鑰，連線已中止（{}）",
                        canonical_endpoint(&host, port)
                    );
                    self.log_connect_failure(&host, port, &user, &msg);
                    return Err(msg);
                }
            }
        };

        self.note_auth("authenticate");
        let authenticated = match auth {
            SshAuth::Password { password } => session
                .authenticate_password(user.clone(), password)
                .await
                .map_err(|e| {
                    let msg = format!("SSH 認證發生錯誤：{e}");
                    self.log_connect_failure(&host, port, &user, &msg);
                    msg
                })?
                .success(),
            SshAuth::Key {
                key_path,
                passphrase,
            } => {
                let key = load_secret_key(&key_path, passphrase.as_deref()).map_err(|e| {
                    let msg = format!("無法讀取私鑰 {key_path}：{e}");
                    self.log_connect_failure(&host, port, &user, &msg);
                    msg
                })?;
                let hash = session
                    .best_supported_rsa_hash()
                    .await
                    .map_err(|e| {
                        let msg = format!("SSH 認證發生錯誤：{e}");
                        self.log_connect_failure(&host, port, &user, &msg);
                        msg
                    })?
                    .flatten();
                session
                    .authenticate_publickey(
                        user.clone(),
                        PrivateKeyWithHashAlg::new(Arc::new(key), hash),
                    )
                    .await
                    .map_err(|e| {
                        let msg = format!("SSH 認證發生錯誤：{e}");
                        self.log_connect_failure(&host, port, &user, &msg);
                        msg
                    })?
                    .success()
            }
        };

        if !authenticated {
            let msg = "SSH 認證失敗：帳號、密碼或金鑰不正確".to_string();
            self.log_connect_failure(&host, port, &user, &msg);
            return Err(msg);
        }

        let (fingerprint, known_host) = {
            let outcome = outcome.lock().unwrap();
            (
                outcome.fingerprint.clone().unwrap_or_default(),
                outcome.known,
            )
        };
        let session_id = format!("ssh-{}", NEXT_SESSION_ID.fetch_add(1, Ordering::Relaxed));
        self.log_connect(&session_id, &host, port, &user, &fingerprint, known_host);

        self.sessions.lock().unwrap().insert(
            session_id.clone(),
            SessionEntry {
                handle: Arc::new(AsyncMutex::new(session)),
                shell: None,
                sftp: None,
                host,
            },
        );

        Ok(SshConnectResult {
            session_id,
            fingerprint,
            known_host,
        })
    }

    async fn open_shell(
        self: &Arc<Self>,
        app: AppHandle,
        session_id: String,
        cols: u32,
        rows: u32,
    ) -> Result<(), String> {
        let handle = self.get_handle(&session_id)?;
        let channel = {
            let handle = handle.lock().await;
            let channel = handle
                .channel_open_session()
                .await
                .map_err(|e| format!("無法開啟 SSH channel：{e}"))?;
            channel
                .request_pty(false, "xterm-256color", cols, rows, 0, 0, &[])
                .await
                .map_err(|e| format!("request_pty 失敗：{e}"))?;
            channel
                .request_shell(false)
                .await
                .map_err(|e| format!("request_shell 失敗：{e}"))?;
            channel
        };

        let (tx, rx) = mpsc::unbounded_channel::<ShellCmd>();
        let registered = {
            let mut map = self.sessions.lock().unwrap();
            match map.get_mut(&session_id) {
                Some(entry) => {
                    entry.shell = Some(tx);
                    true
                }
                None => false,
            }
        };
        if !registered {
            // Disconnected between get_handle and here — tear the channel down.
            let _ = channel.eof().await;
            return Err(format!("SSH session {session_id} 已關閉"));
        }

        let manager = Arc::clone(self);
        tauri::async_runtime::spawn(shell_loop(manager, app, session_id, channel, rx));
        Ok(())
    }

    /// Open (or reuse) the session's SFTP subsystem. A second `channel_open_session`
    /// on the live SSH handle is upgraded to the `sftp` subsystem, then wrapped in
    /// a `SftpSession`. Cached on the entry so every sftp command shares one
    /// subsystem; a race where two commands open concurrently keeps whichever
    /// registered first and drops the loser.
    async fn ensure_sftp(&self, session_id: &str) -> Result<Arc<SftpSession>, String> {
        if let Some(sftp) = self
            .sessions
            .lock()
            .unwrap()
            .get(session_id)
            .and_then(|e| e.sftp.clone())
        {
            return Ok(sftp);
        }

        let handle = self.get_handle(session_id)?;
        let channel = {
            let handle = handle.lock().await;
            let channel = handle
                .channel_open_session()
                .await
                .map_err(|e| format!("無法開啟 SFTP channel：{e}"))?;
            channel
                .request_subsystem(true, "sftp")
                .await
                .map_err(|e| format!("無法啟動 SFTP 子系統：{e}"))?;
            channel
        };
        let sftp = SftpSession::new(channel.into_stream())
            .await
            .map_err(|e| format!("無法建立 SFTP session：{e}"))?;
        let sftp = Arc::new(sftp);

        let mut map = self.sessions.lock().unwrap();
        match map.get_mut(session_id) {
            Some(entry) => match entry.sftp.clone() {
                Some(existing) => Ok(existing),
                None => {
                    entry.sftp = Some(sftp.clone());
                    Ok(sftp)
                }
            },
            None => Err(format!("SSH session {session_id} 已關閉")),
        }
    }

    async fn sftp_list_dir(&self, session_id: &str, path: &str) -> Result<SftpListing, String> {
        let sftp = self.ensure_sftp(session_id).await?;
        // REALPATH resolves "." to the login home and collapses "..", so the
        // front-end can navigate up by appending "/.." without path math.
        let cwd = sftp
            .canonicalize(if path.is_empty() { "." } else { path })
            .await
            .map_err(|e| format!("無法解析遠端路徑：{e}"))?;
        let read = sftp
            .read_dir(cwd.clone())
            .await
            .map_err(|e| format!("無法讀取遠端目錄 {cwd}：{e}"))?;
        let mut entries: Vec<SftpEntry> = read
            .map(|entry| {
                let ft = entry.file_type();
                let name = entry.file_name();
                let name_safe = path_capability::is_safe_leaf_name(&name);
                SftpEntry {
                    name,
                    path: entry.path(),
                    is_dir: ft.is_dir(),
                    is_symlink: ft.is_symlink(),
                    name_safe,
                    size: entry.metadata().size.unwrap_or(0),
                }
            })
            .collect();
        sort_sftp_entries(&mut entries);
        Ok(SftpListing { cwd, entries })
    }

    async fn sftp_mkdir(&self, session_id: &str, path: &str) -> Result<(), String> {
        reject_unsafe_remote_leaf(path)?;
        let sftp = self.ensure_sftp(session_id).await?;
        sftp.create_dir(path.to_string())
            .await
            .map_err(|e| format!("無法建立遠端資料夾：{e}"))
    }

    async fn sftp_rename(&self, session_id: &str, from: &str, to: &str) -> Result<(), String> {
        reject_unsafe_remote_leaf(from)?;
        reject_unsafe_remote_leaf(to)?;
        let sftp = self.ensure_sftp(session_id).await?;
        sftp.rename(from.to_string(), to.to_string())
            .await
            .map_err(|e| format!("無法重新命名遠端項目：{e}"))
    }

    async fn sftp_remove(&self, session_id: &str, path: &str, is_dir: bool) -> Result<(), String> {
        let sftp = self.ensure_sftp(session_id).await?;
        if is_dir {
            sftp.remove_dir(path.to_string())
                .await
                .map_err(|e| format!("無法刪除遠端資料夾（需為空）：{e}"))
        } else {
            sftp.remove_file(path.to_string())
                .await
                .map_err(|e| format!("無法刪除遠端檔案：{e}"))
        }
    }

    async fn sftp_upload(
        &self,
        app: &AppHandle,
        selected: &path_capability::SelectedPathRegistry,
        workspaces: &path_capability::WorkspacePathRegistry,
        trust: &crate::workspace_trust::WorkspaceTrustState,
        session_id: &str,
        transfer_id: &str,
        source: SftpUploadSource,
        remote_dir: &str,
    ) -> Result<(), String> {
        if !path_capability::is_safe_transfer_id(transfer_id) {
            return Err(PathCapabilityError::UnsafeLeaf.into());
        }
        reject_unsafe_remote_leaf(remote_dir)?;
        let leaf = match &source {
            SftpUploadSource::Workspace { relative_path, .. } => {
                SafeRelativePath::parse(relative_path)?
                    .leaf()
                    .as_str()
                    .to_string()
            }
            SftpUploadSource::Selected { capability_id } => selected.peek_leaf(capability_id)?,
        };
        let leaf = SafeLeafName::parse(&leaf)?;
        let remote_path = remote_join(remote_dir, leaf.as_str());
        let _slot = self
            .transfer_dests
            .acquire(path_capability::remote_dest_key(session_id, &remote_path))?;
        let opened = open_sftp_upload_source(selected, workspaces, trust, source)?;
        let sftp = self.ensure_sftp(session_id).await?;
        // Stream into a temp sibling and rename into place, so an interrupted
        // upload never leaves a half-written file at (or clobbers) the target.
        let temp_path = remote_join(remote_dir, &temp_transfer_name(leaf.as_str(), transfer_id));

        let total = opened.len;
        let mut local = tokio::fs::File::from_std(opened.file);
        let mut remote = sftp
            .open_with_flags(
                temp_path.clone(),
                OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE,
            )
            .await
            .map_err(|_| "sftp-remote-open-failed".to_string())?;

        let mut buf = vec![0u8; SFTP_CHUNK];
        let mut transferred = 0u64;
        let mut last_emit = 0u64;
        self.emit_progress(app, session_id, transfer_id, 0, total, false);
        // On any read/write/close failure, fall through to temp cleanup below.
        let copy = async {
            loop {
                let n = local
                    .read(&mut buf)
                    .await
                    .map_err(|_| "sftp-read-failed".to_string())?;
                if n == 0 {
                    break;
                }
                remote
                    .write_all(&buf[..n])
                    .await
                    .map_err(|_| "sftp-write-failed".to_string())?;
                transferred += n as u64;
                if transferred - last_emit >= SFTP_PROGRESS_STEP {
                    last_emit = transferred;
                    self.emit_progress(app, session_id, transfer_id, transferred, total, false);
                }
            }
            remote
                .shutdown()
                .await
                .map_err(|_| "sftp-write-failed".to_string())
        }
        .await;

        if let Err(e) = copy {
            // Best-effort: drop the half-written temp, leave any existing target intact.
            let _ = sftp.remove_file(temp_path.clone()).await;
            return Err(e);
        }

        // SFTP rename does not overwrite; drop an existing target first, then
        // promote the fully-written temp into place.
        if sftp.try_exists(remote_path.clone()).await.unwrap_or(false) {
            if sftp.remove_file(remote_path.clone()).await.is_err() {
                let _ = sftp.remove_file(temp_path.clone()).await;
                return Err("sftp-promote-failed".into());
            }
        }
        if let Err(_e) = sftp.rename(temp_path.clone(), remote_path.clone()).await {
            let _ = sftp.remove_file(temp_path.clone()).await;
            return Err("sftp-promote-failed".into());
        }

        self.emit_progress(app, session_id, transfer_id, transferred, total, true);
        Ok(())
    }

    async fn sftp_download(
        &self,
        app: &AppHandle,
        session_id: &str,
        transfer_id: &str,
        remote_path: &str,
        destination_capability_id: &str,
        destinations: &path_capability::DownloadDestinationRegistry,
    ) -> Result<(), String> {
        if !path_capability::is_safe_transfer_id(transfer_id) {
            return Err(PathCapabilityError::UnsafeLeaf.into());
        }
        reject_unsafe_remote_leaf(remote_path)?;
        let mut scratch = destinations.take_scratch(destination_capability_id, transfer_id)?;
        let _slot = self.transfer_dests.acquire(scratch.dest_key())?;
        let sftp = self.ensure_sftp(session_id).await?;
        let total = sftp
            .metadata(remote_path.to_string())
            .await
            .ok()
            .and_then(|m| m.size)
            .unwrap_or(0);
        let mut remote = match sftp.open(remote_path.to_string()).await {
            Ok(file) => file,
            Err(_) => {
                scratch.discard();
                return Err("sftp-remote-open-failed".into());
            }
        };

        let mut out = tokio::fs::File::from_std(scratch.take_file());

        let mut buf = vec![0u8; SFTP_CHUNK];
        let mut transferred = 0u64;
        let mut last_emit = 0u64;
        self.emit_progress(app, session_id, transfer_id, 0, total, false);
        // On any read/write/flush failure, fall through to temp cleanup below.
        let copy = async {
            loop {
                let n = remote
                    .read(&mut buf)
                    .await
                    .map_err(|_| "sftp-read-failed".to_string())?;
                if n == 0 {
                    break;
                }
                out.write_all(&buf[..n])
                    .await
                    .map_err(|_| "sftp-write-failed".to_string())?;
                transferred += n as u64;
                if transferred - last_emit >= SFTP_PROGRESS_STEP {
                    last_emit = transferred;
                    self.emit_progress(app, session_id, transfer_id, transferred, total, false);
                }
            }
            out.sync_all()
                .await
                .map_err(|_| "sftp-write-failed".to_string())
        }
        .await;

        // Close the temp file so the rename sees a released handle on every platform.
        drop(out);
        if let Err(e) = copy {
            scratch.discard();
            return Err(e);
        }
        if scratch.promote().is_err() {
            return Err("sftp-promote-failed".into());
        }

        self.emit_progress(app, session_id, transfer_id, transferred, total, true);
        Ok(())
    }

    fn emit_progress(
        &self,
        app: &AppHandle,
        session_id: &str,
        transfer_id: &str,
        transferred: u64,
        total: u64,
        done: bool,
    ) {
        let _ = app.emit(
            "sftp://progress",
            SftpProgressPayload {
                session_id: session_id.to_string(),
                transfer_id: transfer_id.to_string(),
                transferred,
                total,
                done,
            },
        );
    }

    fn write(&self, session_id: &str, data: &str) -> Result<(), String> {
        let tx = self.get_shell(session_id)?;
        tx.send(ShellCmd::Data(data.as_bytes().to_vec()))
            .map_err(|_| format!("SSH session {session_id} 的 shell 已結束"))
    }

    fn resize(&self, session_id: &str, cols: u32, rows: u32) -> Result<(), String> {
        let tx = self.get_shell(session_id)?;
        tx.send(ShellCmd::Resize(cols, rows))
            .map_err(|_| format!("SSH session {session_id} 的 shell 已結束"))
    }

    async fn disconnect(&self, session_id: &str) -> Result<(), String> {
        let entry = self.sessions.lock().unwrap().remove(session_id);
        let Some(SessionEntry {
            handle,
            shell,
            host,
            ..
        }) = entry
        else {
            // Idempotent: disconnecting an unknown/already-closed session is fine.
            return Ok(());
        };
        // Dropping the sender ends the shell task (its rx yields None), which
        // sends EOF and emits ssh://exit.
        drop(shell);
        {
            let handle = handle.lock().await;
            let _ = handle.disconnect(Disconnect::ByApplication, "", "").await;
        }
        self.log_disconnect(session_id, &host);
        Ok(())
    }

    pub fn kill_all(&self) {
        // Dropping every entry drops its Handle (closing the transport) and its
        // shell sender (ending the shell task). Called on app exit.
        self.host_keys.reject_all_pending();
        self.sessions.lock().unwrap().clear();
        self.transfer_dests.clear();
    }

    fn get_handle(&self, session_id: &str) -> Result<Arc<AsyncMutex<Handle<Client>>>, String> {
        self.sessions
            .lock()
            .unwrap()
            .get(session_id)
            .map(|entry| entry.handle.clone())
            .ok_or_else(|| format!("找不到 SSH session {session_id}"))
    }

    fn get_shell(&self, session_id: &str) -> Result<mpsc::UnboundedSender<ShellCmd>, String> {
        let map = self.sessions.lock().unwrap();
        let entry = map
            .get(session_id)
            .ok_or_else(|| format!("找不到 SSH session {session_id}"))?;
        entry
            .shell
            .clone()
            .ok_or_else(|| format!("SSH session {session_id} 尚未開啟 shell"))
    }

    fn mark_shell_closed(&self, session_id: &str) {
        if let Some(entry) = self.sessions.lock().unwrap().get_mut(session_id) {
            entry.shell = None;
        }
    }

    fn log_connect(
        &self,
        session_id: &str,
        host: &str,
        port: u16,
        user: &str,
        fingerprint: &str,
        known_host: bool,
    ) {
        (self.log)(logging::LogEvent {
            level: "info".into(),
            kind: "debug".into(),
            source: "ssh".into(),
            workspace_path: None,
            event: "ssh_connect".into(),
            message: format!("ssh session {session_id} connected to {user}@{host}:{port}"),
            metadata: serde_json::json!({
                "sessionId": session_id,
                "host": host,
                "port": port,
                "user": user,
                // Host key verified against the known-hosts store; a changed
                // or unaccepted first-use key is rejected before authentication.
                "fingerprint": fingerprint,
                "knownHost": known_host,
            }),
        });
    }

    fn log_disconnect(&self, session_id: &str, host: &str) {
        (self.log)(logging::LogEvent {
            level: "info".into(),
            kind: "debug".into(),
            source: "ssh".into(),
            workspace_path: None,
            event: "ssh_disconnect".into(),
            message: format!("ssh session {session_id} disconnected"),
            metadata: serde_json::json!({
                "sessionId": session_id,
                "host": host,
            }),
        });
    }

    fn log_connect_failure(&self, host: &str, port: u16, user: &str, reason: &str) {
        (self.log)(logging::connect_failure_event(
            "ssh", host, port, user, reason,
        ));
    }
}

async fn shell_loop(
    manager: Arc<SshManager>,
    app: AppHandle,
    session_id: String,
    mut channel: russh::Channel<client::Msg>,
    mut rx: mpsc::UnboundedReceiver<ShellCmd>,
) {
    let mut chunker = Utf8Chunker::default();
    let emit_chunk = |chunk: String| {
        if !chunk.is_empty() {
            let _ = app.emit(
                "ssh://data",
                SshDataPayload {
                    session_id: session_id.clone(),
                    chunk,
                },
            );
        }
    };

    loop {
        tokio::select! {
            cmd = rx.recv() => match cmd {
                Some(ShellCmd::Data(bytes)) => {
                    if channel.data(&bytes[..]).await.is_err() {
                        break;
                    }
                }
                Some(ShellCmd::Resize(cols, rows)) => {
                    let _ = channel.window_change(cols, rows, 0, 0).await;
                }
                None => {
                    // All senders dropped (session disconnected) — close the shell.
                    let _ = channel.eof().await;
                    break;
                }
            },
            msg = channel.wait() => match msg {
                Some(ChannelMsg::Data { ref data }) => {
                    if let Some(chunk) = chunker.push(data.as_ref()) {
                        emit_chunk(chunk);
                    }
                }
                Some(ChannelMsg::ExtendedData { ref data, .. }) => {
                    if let Some(chunk) = chunker.push(data.as_ref()) {
                        emit_chunk(chunk);
                    }
                }
                Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) | None => break,
                _ => {}
            },
        }
    }

    if let Some(chunk) = chunker.finish_lossy() {
        emit_chunk(chunk);
    }
    let _ = app.emit(
        "ssh://exit",
        SshExitPayload {
            session_id: session_id.clone(),
        },
    );
    manager.mark_shell_closed(&session_id);
}

// UTF-8 boundary chunker mirroring pty_service's encoding so ssh:// output
// reaches xterm as the same well-formed String stream (multibyte chars split
// across SSH packets are reassembled; invalid bytes become U+FFFD).
#[derive(Default)]
struct Utf8Chunker {
    pending: Vec<u8>,
}

impl Utf8Chunker {
    fn push(&mut self, bytes: &[u8]) -> Option<String> {
        self.pending.extend_from_slice(bytes);
        let mut output = String::new();

        loop {
            match std::str::from_utf8(&self.pending) {
                Ok(valid) => {
                    output.push_str(valid);
                    self.pending.clear();
                    break;
                }
                Err(err) => {
                    let valid_up_to = err.valid_up_to();
                    if valid_up_to > 0 {
                        let complete = self.pending.drain(..valid_up_to).collect::<Vec<_>>();
                        output.push_str(&String::from_utf8(complete).unwrap_or_default());
                    }

                    if let Some(error_len) = err.error_len() {
                        self.pending.drain(..error_len);
                        output.push('\u{fffd}');
                        continue;
                    }

                    break;
                }
            }
        }

        if output.is_empty() {
            None
        } else {
            Some(output)
        }
    }

    fn finish_lossy(&mut self) -> Option<String> {
        if self.pending.is_empty() {
            None
        } else {
            let text = String::from_utf8_lossy(&self.pending).into_owned();
            self.pending.clear();
            Some(text)
        }
    }
}

#[tauri::command]
pub async fn ssh_connect(
    state: tauri::State<'_, SshState>,
    host: String,
    port: u16,
    user: String,
    auth: SshAuth,
) -> Result<SshConnectResult, String> {
    state.0.connect(host, port, user, auth).await
}

#[tauri::command]
pub fn ssh_host_key_respond(
    state: tauri::State<'_, SshState>,
    challenge_id: String,
    accept: bool,
    endpoint: String,
    fingerprint: String,
) -> Result<(), String> {
    state
        .0
        .respond_host_key(&challenge_id, accept, &endpoint, &fingerprint)
}

#[tauri::command]
pub async fn ssh_open_shell(
    app: AppHandle,
    state: tauri::State<'_, SshState>,
    session_id: String,
    cols: u32,
    rows: u32,
) -> Result<(), String> {
    let manager = state.0.clone();
    manager.open_shell(app, session_id, cols, rows).await
}

#[tauri::command]
pub async fn ssh_write(
    state: tauri::State<'_, SshState>,
    session_id: String,
    data: String,
) -> Result<(), String> {
    state.0.write(&session_id, &data)
}

#[tauri::command]
pub async fn ssh_resize(
    state: tauri::State<'_, SshState>,
    session_id: String,
    cols: u32,
    rows: u32,
) -> Result<(), String> {
    state.0.resize(&session_id, cols, rows)
}

#[tauri::command]
pub async fn ssh_disconnect(
    state: tauri::State<'_, SshState>,
    session_id: String,
) -> Result<(), String> {
    state.0.disconnect(&session_id).await
}

#[tauri::command]
pub async fn sftp_list_dir(
    state: tauri::State<'_, SshState>,
    session_id: String,
    path: String,
) -> Result<SftpListing, String> {
    state.0.sftp_list_dir(&session_id, &path).await
}

#[tauri::command]
pub async fn sftp_mkdir(
    state: tauri::State<'_, SshState>,
    session_id: String,
    path: String,
) -> Result<(), String> {
    state.0.sftp_mkdir(&session_id, &path).await
}

#[tauri::command]
pub async fn sftp_rename(
    state: tauri::State<'_, SshState>,
    session_id: String,
    from: String,
    to: String,
) -> Result<(), String> {
    state.0.sftp_rename(&session_id, &from, &to).await
}

#[tauri::command]
pub async fn sftp_remove(
    state: tauri::State<'_, SshState>,
    session_id: String,
    path: String,
    is_dir: bool,
) -> Result<(), String> {
    state.0.sftp_remove(&session_id, &path, is_dir).await
}

#[tauri::command]
pub async fn sftp_upload(
    app: AppHandle,
    state: tauri::State<'_, SshState>,
    selected: tauri::State<'_, path_capability::SelectedPathState>,
    workspaces: tauri::State<'_, path_capability::WorkspacePathState>,
    trust: tauri::State<'_, crate::workspace_trust::WorkspaceTrustState>,
    session_id: String,
    transfer_id: String,
    source: SftpUploadSource,
    remote_dir: String,
) -> Result<(), String> {
    state
        .0
        .sftp_upload(
            &app,
            &selected.0,
            &workspaces.0,
            &trust,
            &session_id,
            &transfer_id,
            source,
            &remote_dir,
        )
        .await
}

#[tauri::command]
pub async fn sftp_download(
    app: AppHandle,
    state: tauri::State<'_, SshState>,
    destinations: tauri::State<'_, path_capability::DownloadDestinationState>,
    session_id: String,
    transfer_id: String,
    remote_path: String,
    destination_capability_id: String,
) -> Result<(), String> {
    state
        .0
        .sftp_download(
            &app,
            &session_id,
            &transfer_id,
            &remote_path,
            &destination_capability_id,
            &destinations.0,
        )
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    // A fixed ed25519 public key + its `ssh-keygen -lf` SHA256 fingerprint,
    // captured once so the format assertion is deterministic.
    const SAMPLE_PUBKEY: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIPAUzuZv1lNASWzgxLEUcIqvoX9L717q0LtBXVKu4ABu test@yuzora";
    const SAMPLE_FINGERPRINT: &str = "SHA256:FY8hycuOWgVKhcBgB7NSgKnHxYDCKFUCZt+E4EmHROA";

    #[test]
    fn parse_password_auth() {
        let auth: SshAuth =
            serde_json::from_str(r#"{"kind":"password","password":"hunter2"}"#).unwrap();
        match auth {
            SshAuth::Password { password } => assert_eq!(password, "hunter2"),
            _ => panic!("expected password auth"),
        }
    }

    #[test]
    fn parse_key_auth_with_optional_passphrase() {
        let with_pass: SshAuth = serde_json::from_str(
            r#"{"kind":"key","keyPath":"/home/u/.ssh/id_ed25519","passphrase":"secret"}"#,
        )
        .unwrap();
        match with_pass {
            SshAuth::Key {
                key_path,
                passphrase,
            } => {
                assert_eq!(key_path, "/home/u/.ssh/id_ed25519");
                assert_eq!(passphrase.as_deref(), Some("secret"));
            }
            _ => panic!("expected key auth"),
        }

        let no_pass: SshAuth =
            serde_json::from_str(r#"{"kind":"key","keyPath":"/home/u/.ssh/id_rsa"}"#).unwrap();
        match no_pass {
            SshAuth::Key {
                key_path,
                passphrase,
            } => {
                assert_eq!(key_path, "/home/u/.ssh/id_rsa");
                assert_eq!(passphrase, None);
            }
            _ => panic!("expected key auth"),
        }
    }

    #[test]
    fn unknown_auth_kind_is_rejected() {
        assert!(serde_json::from_str::<SshAuth>(r#"{"kind":"agent"}"#).is_err());
    }

    #[test]
    fn fingerprint_matches_ssh_keygen() {
        let key = PublicKey::from_openssh(SAMPLE_PUBKEY).unwrap();
        assert_eq!(fingerprint_sha256(&key), SAMPLE_FINGERPRINT);
    }

    #[test]
    fn chunker_reassembles_split_multibyte_and_flushes_tail() {
        let mut chunker = Utf8Chunker::default();
        let euro = "€".as_bytes();
        assert_eq!(chunker.push(&euro[..1]), None);
        assert_eq!(chunker.push(&euro[1..]), Some("€".to_string()));
        assert_eq!(chunker.finish_lossy(), None);
    }

    #[tokio::test]
    async fn connect_failure_is_logged() {
        use std::sync::{Arc, Mutex};
        let captured: Arc<Mutex<Vec<logging::LogEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = captured.clone();
        let mgr = SshManager::with_log(Box::new(move |ev| sink.lock().unwrap().push(ev)));

        // 127.0.0.1:1 幾乎必然 ECONNREFUSED → 走傳輸層錯誤分支
        let res = mgr
            .connect(
                "127.0.0.1".into(),
                1,
                "nobody".into(),
                SshAuth::Password {
                    password: "x".into(),
                },
            )
            .await;

        assert!(res.is_err());
        let events = captured.lock().unwrap();
        assert!(events
            .iter()
            .any(|e| e.event == "connect_failed" && e.source == "ssh" && e.level == "warn"));
    }

    #[test]
    fn get_shell_reports_missing_session_and_unopened_shell() {
        let manager = SshManager::for_test();
        assert!(manager.get_shell("nope").is_err());
        assert!(manager.write("nope", "x").is_err());
        assert!(manager.resize("nope", 80, 24).is_err());
    }

    #[test]
    fn host_port_key_formats_endpoint() {
        assert_eq!(canonical_endpoint("example.com", 22), "example.com:22");
        assert_eq!(canonical_endpoint("10.0.0.5", 2222), "10.0.0.5:2222");
        assert_eq!(canonical_endpoint("::1", 22), "[::1]:22");
        assert_eq!(canonical_endpoint("[::1]", 2222), "[::1]:2222");
        assert_eq!(canonical_endpoint("2001:db8::1", 22), "[2001:db8::1]:22");
        assert_eq!(canonical_endpoint("Example.COM.", 22), "example.com:22");
        assert_eq!(host_port_key("example.com", 22), "example.com:22");
    }

    #[test]
    fn first_contact_is_new_and_not_silently_trusted() {
        // Nothing pinned for this endpoint yet → explicit first-use challenge.
        assert_eq!(
            decide_host_key(None, SAMPLE_FINGERPRINT),
            HostKeyDecision::New
        );
    }

    #[test]
    fn matching_pinned_key_is_trusted() {
        assert_eq!(
            decide_host_key(Some(SAMPLE_FINGERPRINT), SAMPLE_FINGERPRINT),
            HostKeyDecision::Match
        );
    }

    #[test]
    fn changed_key_is_rejected() {
        assert_eq!(
            decide_host_key(Some("SHA256:previously-pinned-key"), SAMPLE_FINGERPRINT),
            HostKeyDecision::Changed
        );
    }

    #[test]
    fn known_hosts_read_distinguishes_missing_from_corrupt() {
        // Empty / whitespace-only content is a legitimate empty store.
        assert!(parse_known_hosts("").unwrap().is_empty());
        assert!(parse_known_hosts("   \n").unwrap().is_empty());
        assert!(parse_known_hosts("{}").unwrap().is_empty());
        // Anything else that isn't a JSON string→string object is corrupt → Err.
        assert!(parse_known_hosts("}{ not json").is_err());
        assert!(parse_known_hosts(r#"["array","not","a","map"]"#).is_err());

        // A missing file is first-run, not corruption → Ok(empty).
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("absent").join("known_hosts.json");
        assert!(read_known_hosts(&missing).unwrap().is_empty());
        // A present-but-corrupt file fails closed → Err.
        let corrupt = tmp.path().join("known_hosts.json");
        std::fs::write(&corrupt, "}{ definitely not json").unwrap();
        assert!(read_known_hosts(&corrupt).is_err());
    }

    #[test]
    fn evaluate_host_key_covers_new_match_changed_and_corrupt() {
        let endpoint = canonical_endpoint("example.com", 22);

        // First contact is a challenge — nothing is pinned yet.
        assert_eq!(
            evaluate_host_key(Ok(BTreeMap::new()), &endpoint, SAMPLE_FINGERPRINT),
            HostKeyEval::New
        );

        // A matching pinned key is trusted with nothing to write back.
        let mut pinned = BTreeMap::new();
        pinned.insert(endpoint.clone(), SAMPLE_FINGERPRINT.to_string());
        assert_eq!(
            evaluate_host_key(Ok(pinned.clone()), &endpoint, SAMPLE_FINGERPRINT),
            HostKeyEval::Match
        );

        // A changed key is rejected (not treated as first-use).
        assert_eq!(
            evaluate_host_key(Ok(pinned), &endpoint, "SHA256:some-other-key"),
            HostKeyEval::Changed {
                previous: SAMPLE_FINGERPRINT.to_string(),
            }
        );
    }

    #[test]
    fn corrupt_store_fails_closed_even_for_a_changed_key() {
        // A damaged store must never be read as "nothing pinned": a re-keyed (or
        // hostile) host key is rejected, not silently accepted and re-pinned.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("known_hosts.json");
        std::fs::write(&path, "}{ definitely not json").unwrap();
        match evaluate_host_key(
            read_known_hosts(&path),
            &canonical_endpoint("example.com", 22),
            SAMPLE_FINGERPRINT,
        ) {
            HostKeyEval::Corrupt { reason } => assert!(!reason.is_empty()),
            other => panic!("corrupt store must fail closed, got {other:?}"),
        }
    }

    #[test]
    fn temp_transfer_name_is_a_hidden_scratch_sibling() {
        assert_eq!(
            temp_transfer_name("report.pdf", "xfer-42"),
            "report.pdf.yz-tmp-xfer-42"
        );
        // Joined into a remote dir it stays a sibling of the real target.
        assert_eq!(
            remote_join("/srv/data", &temp_transfer_name("a.bin", "xfer-1")),
            "/srv/data/a.bin.yz-tmp-xfer-1"
        );
    }

    #[test]
    fn known_hosts_round_trip_through_serialize_and_parse() {
        let mut hosts = BTreeMap::new();
        hosts.insert("example.com:22".to_string(), SAMPLE_FINGERPRINT.to_string());
        hosts.insert("10.0.0.5:2222".to_string(), "SHA256:other-host".to_string());
        let restored = parse_known_hosts(&serialize_known_hosts(&hosts).unwrap()).unwrap();
        assert_eq!(restored, hosts);
    }

    #[test]
    fn persist_creates_parent_dir_and_reloads() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nested").join("known_hosts.json");
        let mut hosts = BTreeMap::new();
        hosts.insert(
            canonical_endpoint("example.com", 22),
            SAMPLE_FINGERPRINT.to_string(),
        );
        persist_known_hosts(&path, &hosts, &StdHostKeyIo).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(parse_known_hosts(&content).unwrap(), hosts);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    fn test_manager(
        path: PathBuf,
        timeout: Duration,
        persist_io: Arc<dyn HostKeyPersistIo>,
    ) -> (SshManager, Arc<Mutex<Vec<SshHostKeyPrompt>>>) {
        let prompts = Arc::new(Mutex::new(Vec::new()));
        let sink = prompts.clone();
        let mut manager = SshManager::with_parts(
            Box::new(|_| {}),
            path,
            Arc::new(move |prompt| sink.lock().unwrap().push(prompt)),
            timeout,
            persist_io,
        );
        manager.auth_probe = Some(Arc::new(Mutex::new(Vec::new())));
        (manager, prompts)
    }

    async fn pending_challenge_id(controller: &HostKeyController) -> String {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(id) = controller.pending.lock().unwrap().keys().next().cloned() {
                return id;
            }
            if Instant::now() > deadline {
                panic!("timed out waiting for a pending host-key challenge");
            }
            tokio::task::yield_now().await;
        }
    }

    fn spawn_challenge(
        controller: Arc<HostKeyController>,
        connection_id: &str,
        host: &str,
        port: u16,
        endpoint: &str,
        fingerprint: &str,
    ) -> tokio::task::JoinHandle<bool> {
        let connection_id = connection_id.to_string();
        let host = host.to_string();
        let endpoint = endpoint.to_string();
        let fingerprint = fingerprint.to_string();
        tokio::spawn(async move {
            controller
                .challenge_and_wait(
                    &connection_id,
                    &host,
                    port,
                    &endpoint,
                    "ssh-ed25519",
                    &fingerprint,
                )
                .await
        })
    }

    #[test]
    fn unseen_key_is_a_challenge_not_an_accept() {
        let eval = evaluate_host_key(
            Ok(BTreeMap::new()),
            &canonical_endpoint("example.com", 22),
            SAMPLE_FINGERPRINT,
        );
        assert_eq!(eval, HostKeyEval::New);
    }

    #[tokio::test]
    async fn accept_persists_then_match_permits_without_another_challenge() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("known_hosts.json");
        let controller =
            HostKeyController::new(path.clone(), Arc::new(|_| {}), Duration::from_secs(2));
        let endpoint = canonical_endpoint("example.com", 22);
        let wait = spawn_challenge(
            controller.clone(),
            "conn-1",
            "example.com",
            22,
            &endpoint,
            SAMPLE_FINGERPRINT,
        );
        let challenge_id = pending_challenge_id(&controller).await;
        controller
            .respond(&challenge_id, true, &endpoint, SAMPLE_FINGERPRINT)
            .unwrap();
        assert!(wait.await.unwrap());
        controller
            .persist_pin(&endpoint, SAMPLE_FINGERPRINT)
            .unwrap();
        assert_eq!(
            evaluate_host_key(read_known_hosts(&path), &endpoint, SAMPLE_FINGERPRINT),
            HostKeyEval::Match
        );
        let stored = read_known_hosts(&path).unwrap();
        assert_eq!(
            stored.get(&endpoint).map(String::as_str),
            Some(SAMPLE_FINGERPRINT)
        );
    }

    #[tokio::test]
    async fn reject_timeout_stale_and_replay_deny() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("known_hosts.json");
        let controller =
            HostKeyController::new(path.clone(), Arc::new(|_| {}), Duration::from_millis(40));
        let endpoint = canonical_endpoint("example.com", 22);

        let reject_wait = spawn_challenge(
            controller.clone(),
            "conn-reject",
            "example.com",
            22,
            &endpoint,
            SAMPLE_FINGERPRINT,
        );
        let reject_id = pending_challenge_id(&controller).await;
        controller
            .respond(&reject_id, false, &endpoint, SAMPLE_FINGERPRINT)
            .unwrap();
        assert!(!reject_wait.await.unwrap());
        assert!(read_known_hosts(&path).unwrap().is_empty());
        assert!(controller
            .respond(&reject_id, true, &endpoint, SAMPLE_FINGERPRINT)
            .is_err());

        let timeout_wait = controller.challenge_and_wait(
            "conn-timeout",
            "example.com",
            22,
            &endpoint,
            "ssh-ed25519",
            SAMPLE_FINGERPRINT,
        );
        assert!(!timeout_wait.await);
        assert!(controller.pending.lock().unwrap().is_empty());

        let stale =
            HostKeyController::new(path.clone(), Arc::new(|_| {}), Duration::from_millis(1));
        let (tx, _rx) = oneshot::channel();
        stale.pending.lock().unwrap().insert(
            "stale-id".into(),
            PendingHostKeyChallenge {
                connection_id: "conn-stale".into(),
                endpoint: endpoint.clone(),
                algorithm: "ssh-ed25519".into(),
                fingerprint: SAMPLE_FINGERPRINT.into(),
                expires_at: Instant::now() - Duration::from_secs(1),
                tx,
            },
        );
        assert!(stale
            .respond("stale-id", true, &endpoint, SAMPLE_FINGERPRINT)
            .is_err());
        assert!(stale.pending.lock().unwrap().is_empty());

        let mismatch = HostKeyController::new(path, Arc::new(|_| {}), Duration::from_secs(2));
        let mismatch_wait = spawn_challenge(
            mismatch.clone(),
            "conn-mismatch",
            "example.com",
            22,
            &endpoint,
            SAMPLE_FINGERPRINT,
        );
        let mismatch_id = pending_challenge_id(&mismatch).await;
        assert!(mismatch
            .respond(
                &mismatch_id,
                true,
                &canonical_endpoint("other.example", 22),
                SAMPLE_FINGERPRINT
            )
            .is_err());
        assert!(!mismatch_wait.await.unwrap());
    }

    struct CoordinatedHostKeyIo {
        before_lock: Box<dyn Fn() + Send + Sync>,
        inner: StdHostKeyIo,
    }

    impl CoordinatedHostKeyIo {
        fn new(before_lock: impl Fn() + Send + Sync + 'static) -> Self {
            Self {
                before_lock: Box::new(before_lock),
                inner: StdHostKeyIo,
            }
        }
    }

    impl HostKeyPersistIo for CoordinatedHostKeyIo {
        fn create_dir_all(&self, path: &Path) -> std::io::Result<()> {
            self.inner.create_dir_all(path)
        }

        fn lock_file(&self, file: &std::fs::File) -> std::io::Result<()> {
            (self.before_lock)();
            self.inner.lock_file(file)
        }

        fn unlock_file(&self, file: &std::fs::File) -> std::io::Result<()> {
            self.inner.unlock_file(file)
        }

        fn create_temp(&self, parent: &Path, contents: &[u8]) -> std::io::Result<PathBuf> {
            self.inner.create_temp(parent, contents)
        }

        fn set_restrictive_permissions(&self, path: &Path) -> std::io::Result<()> {
            self.inner.set_restrictive_permissions(path)
        }

        fn sync_file(&self, path: &Path) -> std::io::Result<()> {
            self.inner.sync_file(path)
        }

        fn rename(&self, from: &Path, to: &Path) -> std::io::Result<()> {
            self.inner.rename(from, to)
        }

        fn sync_dir(&self, path: &Path) -> std::io::Result<()> {
            self.inner.sync_dir(path)
        }
    }

    #[test]
    fn concurrent_independent_controllers_preserve_both_pins() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("known_hosts.json");
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let barrier_a = Arc::clone(&barrier);
        let barrier_b = Arc::clone(&barrier);
        let controller_a = HostKeyController::with_io(
            path.clone(),
            Arc::new(|_| {}),
            Duration::from_secs(2),
            Arc::new(CoordinatedHostKeyIo::new(move || {
                barrier_a.wait();
            })),
        );
        let controller_b = HostKeyController::with_io(
            path.clone(),
            Arc::new(|_| {}),
            Duration::from_secs(2),
            Arc::new(CoordinatedHostKeyIo::new(move || {
                barrier_b.wait();
            })),
        );
        let a = std::thread::spawn(move || {
            controller_a.persist_pin(&canonical_endpoint("a.example.com", 22), "SHA256:aaa")
        });
        let b = std::thread::spawn(move || {
            controller_b.persist_pin(&canonical_endpoint("b.example.com", 2222), "SHA256:bbb")
        });
        a.join().unwrap().unwrap();
        b.join().unwrap().unwrap();
        let hosts = read_known_hosts(&path).unwrap();
        assert_eq!(
            hosts
                .get(&canonical_endpoint("a.example.com", 22))
                .map(String::as_str),
            Some("SHA256:aaa")
        );
        assert_eq!(
            hosts
                .get(&canonical_endpoint("b.example.com", 2222))
                .map(String::as_str),
            Some("SHA256:bbb")
        );
    }

    #[test]
    fn stale_independent_controller_cannot_erase_completed_pin() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("known_hosts.json");
        let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(1);
        let (resume_tx, resume_rx) = std::sync::mpsc::sync_channel(1);
        let resume_rx = Arc::new(Mutex::new(resume_rx));
        let resume_for_hook = Arc::clone(&resume_rx);
        let stale = HostKeyController::with_io(
            path.clone(),
            Arc::new(|_| {}),
            Duration::from_secs(2),
            Arc::new(CoordinatedHostKeyIo::new(move || {
                entered_tx.send(()).unwrap();
                resume_for_hook.lock().unwrap().recv().unwrap();
            })),
        );
        let completed =
            HostKeyController::new(path.clone(), Arc::new(|_| {}), Duration::from_secs(2));

        let stale_writer = std::thread::spawn(move || {
            stale.persist_pin(&canonical_endpoint("stale.example.com", 22), "SHA256:stale")
        });
        entered_rx.recv().unwrap();
        completed
            .persist_pin(
                &canonical_endpoint("completed.example.com", 22),
                "SHA256:completed",
            )
            .unwrap();
        resume_tx.send(()).unwrap();
        stale_writer.join().unwrap().unwrap();

        let hosts = read_known_hosts(&path).unwrap();
        assert_eq!(hosts.len(), 2);
        assert_eq!(
            hosts
                .get(&canonical_endpoint("completed.example.com", 22))
                .map(String::as_str),
            Some("SHA256:completed")
        );
        assert_eq!(
            hosts
                .get(&canonical_endpoint("stale.example.com", 22))
                .map(String::as_str),
            Some("SHA256:stale")
        );
    }

    #[test]
    fn injected_lock_write_fsync_rename_parent_sync_and_unlock_failures_deny() {
        let tmp = tempfile::tempdir().unwrap();
        for point in [
            PersistFailPoint::Lock,
            PersistFailPoint::Write,
            PersistFailPoint::Fsync,
            PersistFailPoint::Rename,
            PersistFailPoint::ParentSync,
            PersistFailPoint::Unlock,
        ] {
            let path = tmp.path().join(format!("known_hosts-{point:?}.json"));
            let lock = Mutex::new(());
            let io = FaultyHostKeyIo::new(point);
            let err = persist_accepted_pin(
                &path,
                &lock,
                &canonical_endpoint("example.com", 22),
                SAMPLE_FINGERPRINT,
                &io,
            )
            .unwrap_err();
            assert!(err.contains("injected") || err.contains("無法"), "{err}");
            if matches!(
                point,
                PersistFailPoint::Lock
                    | PersistFailPoint::Write
                    | PersistFailPoint::Fsync
                    | PersistFailPoint::Rename
            ) {
                assert!(!path.exists(), "pre-rename failure must not leave a pin");
            }
        }
    }

    #[test]
    fn persist_failure_does_not_leave_a_match() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("known_hosts.json");
        let lock = Mutex::new(());
        let io = FaultyHostKeyIo::new(PersistFailPoint::Rename);
        assert!(persist_accepted_pin(
            &path,
            &lock,
            &canonical_endpoint("example.com", 22),
            SAMPLE_FINGERPRINT,
            &io,
        )
        .is_err());
        assert_eq!(
            evaluate_host_key(
                read_known_hosts(&path),
                &canonical_endpoint("example.com", 22),
                SAMPLE_FINGERPRINT
            ),
            HostKeyEval::New
        );
    }

    async fn spawn_test_ssh_server(
        host_key: russh::keys::PrivateKey,
        auth_calls: Arc<Mutex<Vec<String>>>,
    ) -> u16 {
        use russh::server::{self, Auth, Server as _};

        struct TestServer {
            auth_calls: Arc<Mutex<Vec<String>>>,
        }
        struct TestHandler {
            auth_calls: Arc<Mutex<Vec<String>>>,
        }
        impl server::Server for TestServer {
            type Handler = TestHandler;
            fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Self::Handler {
                TestHandler {
                    auth_calls: self.auth_calls.clone(),
                }
            }
        }
        impl server::Handler for TestHandler {
            type Error = russh::Error;
            async fn auth_password(
                &mut self,
                user: &str,
                _password: &str,
            ) -> Result<Auth, Self::Error> {
                self.auth_calls
                    .lock()
                    .unwrap()
                    .push(format!("password:{user}"));
                Ok(Auth::Accept)
            }
            async fn auth_publickey(
                &mut self,
                user: &str,
                _key: &russh::keys::PublicKey,
            ) -> Result<Auth, Self::Error> {
                self.auth_calls
                    .lock()
                    .unwrap()
                    .push(format!("publickey:{user}"));
                Ok(Auth::Accept)
            }
        }

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let config = russh::server::Config {
            keys: vec![host_key],
            auth_rejection_time: Duration::from_millis(0),
            auth_rejection_time_initial: Some(Duration::from_millis(0)),
            inactivity_timeout: Some(Duration::from_secs(5)),
            ..Default::default()
        };
        let mut server = TestServer { auth_calls };
        tokio::spawn(async move {
            let _ = server.run_on_socket(Arc::new(config), &listener).await;
        });
        port
    }

    const TEST_HOST_KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW
QyNTUxOQAAACDUGvy/dQi6qt6SkwsGTu3EcAiTFB8VrntPMcvXnOxZoQAAAJDwjSfa8I0n
2gAAAAtzc2gtZWQyNTUxOQAAACDUGvy/dQi6qt6SkwsGTu3EcAiTFB8VrntPMcvXnOxZoQ
AAAECo3jCqAAIhzabgFbjTlGTuA6Cminb+DrfcNAIxPjNV/dQa/L91CLqq3pKTCwZO7cRw
CJMUHxWue08xy9ec7FmhAAAAC3l1em9yYS10ZXN0AQI=
-----END OPENSSH PRIVATE KEY-----";
    const TEST_HOST_FINGERPRINT: &str = "SHA256:pMXX/KzhB+iSQDQYHa283nwklPkTkwlDERt7V0AGdEE";

    fn spawn_connect(
        manager: Arc<SshManager>,
        host: &str,
        port: u16,
        user: &str,
    ) -> tokio::task::JoinHandle<Result<SshConnectResult, String>> {
        let host = host.to_string();
        let user = user.to_string();
        tokio::spawn(async move {
            manager
                .connect(
                    host,
                    port,
                    user,
                    SshAuth::Password {
                        password: "secret".into(),
                    },
                )
                .await
        })
    }

    async fn wait_for_new_prompt(prompts: &Arc<Mutex<Vec<SshHostKeyPrompt>>>) -> SshHostKeyPrompt {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(prompt) = prompts
                .lock()
                .unwrap()
                .iter()
                .find_map(|prompt| match prompt {
                    SshHostKeyPrompt::New { .. } => Some(prompt.clone()),
                    _ => None,
                })
            {
                return prompt;
            }
            if Instant::now() > deadline {
                panic!("timed out waiting for first-use host-key prompt");
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    #[tokio::test]
    async fn unseen_key_blocks_before_auth_and_accept_persists() {
        let host_key = russh::keys::PrivateKey::from_openssh(TEST_HOST_KEY).unwrap();
        assert_eq!(
            fingerprint_sha256(&host_key.public_key()),
            TEST_HOST_FINGERPRINT
        );
        let auth_calls = Arc::new(Mutex::new(Vec::new()));
        let port = spawn_test_ssh_server(host_key, auth_calls.clone()).await;
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("known_hosts.json");
        let (manager, prompts) =
            test_manager(path.clone(), Duration::from_secs(5), Arc::new(StdHostKeyIo));
        let probe = manager.auth_probe.clone().unwrap();
        let manager = Arc::new(manager);
        let connect = spawn_connect(manager.clone(), "127.0.0.1", port, "alice");
        let prompt = wait_for_new_prompt(&prompts).await;
        assert!(auth_calls.lock().unwrap().is_empty());
        assert!(!probe
            .lock()
            .unwrap()
            .iter()
            .any(|stage| *stage == "authenticate"));
        let SshHostKeyPrompt::New {
            challenge_id,
            endpoint,
            algorithm,
            fingerprint,
            ..
        } = prompt
        else {
            panic!("expected new-key prompt");
        };
        assert_eq!(algorithm, "ssh-ed25519");
        assert_eq!(fingerprint, TEST_HOST_FINGERPRINT);
        manager
            .respond_host_key(&challenge_id, true, &endpoint, &fingerprint)
            .unwrap();
        let result = connect.await.unwrap().unwrap();
        assert_eq!(result.fingerprint, TEST_HOST_FINGERPRINT);
        assert!(!result.known_host);
        assert_eq!(
            auth_calls.lock().unwrap().as_slice(),
            ["password:alice".to_string()]
        );
        let stages = probe.lock().unwrap().clone();
        let challenge_at = stages
            .iter()
            .position(|s| *s == "host_key_challenge")
            .unwrap();
        let pinned_at = stages.iter().position(|s| *s == "host_key_pinned").unwrap();
        let auth_at = stages.iter().position(|s| *s == "authenticate").unwrap();
        assert!(
            challenge_at < pinned_at && pinned_at < auth_at,
            "{stages:?}"
        );
        assert_eq!(
            read_known_hosts(&path)
                .unwrap()
                .get(&canonical_endpoint("127.0.0.1", port))
                .map(String::as_str),
            Some(TEST_HOST_FINGERPRINT)
        );

        let (manager2, prompts2) =
            test_manager(path, Duration::from_secs(5), Arc::new(StdHostKeyIo));
        let again = manager2
            .connect(
                "127.0.0.1".into(),
                port,
                "alice".into(),
                SshAuth::Password {
                    password: "secret".into(),
                },
            )
            .await
            .unwrap();
        assert!(again.known_host);
        assert!(prompts2.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn reject_and_timeout_never_run_auth() {
        let host_key = russh::keys::PrivateKey::from_openssh(TEST_HOST_KEY).unwrap();
        let auth_calls = Arc::new(Mutex::new(Vec::new()));
        let port = spawn_test_ssh_server(host_key, auth_calls.clone()).await;
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("known_hosts.json");
        let (manager, prompts) =
            test_manager(path.clone(), Duration::from_secs(5), Arc::new(StdHostKeyIo));
        let manager = Arc::new(manager);
        let connect = spawn_connect(manager.clone(), "127.0.0.1", port, "alice");
        let prompt = wait_for_new_prompt(&prompts).await;
        let SshHostKeyPrompt::New {
            challenge_id,
            endpoint,
            fingerprint,
            ..
        } = prompt
        else {
            panic!("expected new-key prompt");
        };
        manager
            .respond_host_key(&challenge_id, false, &endpoint, &fingerprint)
            .unwrap();
        let err = connect.await.unwrap().unwrap_err();
        assert!(err.contains("拒絕") || err.contains("逾時"), "{err}");
        assert!(auth_calls.lock().unwrap().is_empty());
        assert!(read_known_hosts(&path).unwrap().is_empty());

        let (timeout_mgr, _) = test_manager(
            path.clone(),
            Duration::from_millis(40),
            Arc::new(StdHostKeyIo),
        );
        let err = timeout_mgr
            .connect(
                "127.0.0.1".into(),
                port,
                "alice".into(),
                SshAuth::Password {
                    password: "secret".into(),
                },
            )
            .await
            .unwrap_err();
        assert!(err.contains("拒絕") || err.contains("逾時"), "{err}");
        assert!(auth_calls.lock().unwrap().is_empty());
        assert!(read_known_hosts(&path).unwrap().is_empty());
    }

    #[tokio::test]
    async fn changed_key_denies_without_accept_and_shows_both_fingerprints() {
        let host_key = russh::keys::PrivateKey::from_openssh(TEST_HOST_KEY).unwrap();
        let auth_calls = Arc::new(Mutex::new(Vec::new()));
        let port = spawn_test_ssh_server(host_key, auth_calls.clone()).await;
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("known_hosts.json");
        let mut pinned = BTreeMap::new();
        pinned.insert(
            canonical_endpoint("127.0.0.1", port),
            "SHA256:previously-pinned-key".into(),
        );
        persist_known_hosts(&path, &pinned, &StdHostKeyIo).unwrap();
        let (manager, prompts) = test_manager(path, Duration::from_secs(5), Arc::new(StdHostKeyIo));
        let err = manager
            .connect(
                "127.0.0.1".into(),
                port,
                "alice".into(),
                SshAuth::Password {
                    password: "secret".into(),
                },
            )
            .await
            .unwrap_err();
        assert!(err.contains("SHA256:previously-pinned-key"), "{err}");
        assert!(err.contains(TEST_HOST_FINGERPRINT), "{err}");
        assert!(auth_calls.lock().unwrap().is_empty());
        let emitted = prompts.lock().unwrap().clone();
        match emitted.as_slice() {
            [SshHostKeyPrompt::Changed {
                previous_fingerprint,
                fingerprint,
                ..
            }] => {
                assert_eq!(previous_fingerprint, "SHA256:previously-pinned-key");
                assert_eq!(fingerprint, TEST_HOST_FINGERPRINT);
            }
            other => panic!("expected changed prompt, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn injected_lock_persist_and_unlock_failures_during_accept_deny_auth() {
        let host_key = russh::keys::PrivateKey::from_openssh(TEST_HOST_KEY).unwrap();
        let auth_calls = Arc::new(Mutex::new(Vec::new()));
        let port = spawn_test_ssh_server(host_key, auth_calls.clone()).await;
        let tmp = tempfile::tempdir().unwrap();

        for point in [
            PersistFailPoint::Lock,
            PersistFailPoint::Rename,
            PersistFailPoint::ParentSync,
            PersistFailPoint::Unlock,
        ] {
            let path = tmp.path().join(format!("known_hosts-{point:?}.json"));
            let (manager, prompts) = test_manager(
                path.clone(),
                Duration::from_secs(5),
                Arc::new(FaultyHostKeyIo::new(point)),
            );
            let manager = Arc::new(manager);
            let connect = spawn_connect(manager.clone(), "127.0.0.1", port, "alice");
            let prompt = wait_for_new_prompt(&prompts).await;
            let SshHostKeyPrompt::New {
                challenge_id,
                endpoint,
                fingerprint,
                ..
            } = prompt
            else {
                panic!("expected new-key prompt");
            };
            manager
                .respond_host_key(&challenge_id, true, &endpoint, &fingerprint)
                .unwrap();
            let err = connect.await.unwrap().unwrap_err();
            assert!(err.contains("無法保存"), "{point:?}: {err}");
            assert!(auth_calls.lock().unwrap().is_empty());
            if matches!(point, PersistFailPoint::Lock | PersistFailPoint::Rename) {
                assert!(read_known_hosts(&path).unwrap().is_empty());
            }
        }
    }

    #[test]
    fn remote_join_uses_posix_separator_and_handles_root() {
        assert_eq!(remote_join("/home/u", "file.txt"), "/home/u/file.txt");
        assert_eq!(remote_join("/", "file.txt"), "/file.txt");
        assert_eq!(remote_join("", "file.txt"), "/file.txt");
        // A trailing slash on the dir must not double up.
        assert_eq!(remote_join("/home/u/", "file.txt"), "/home/u/file.txt");
    }

    fn entry(name: &str, is_dir: bool) -> SftpEntry {
        SftpEntry {
            name: name.to_string(),
            path: format!("/{name}"),
            is_dir,
            is_symlink: false,
            name_safe: path_capability::is_safe_leaf_name(name),
            size: 0,
        }
    }

    #[test]
    fn upload_source_deserializes_tagged_workspace_and_selected() {
        let workspace: SftpUploadSource = serde_json::from_str(
            r#"{"kind":"workspace","workspaceId":"ws-opaque","relativePath":"src/a.txt"}"#,
        )
        .unwrap();
        match workspace {
            SftpUploadSource::Workspace {
                workspace_id,
                relative_path,
            } => {
                assert_eq!(workspace_id, "ws-opaque");
                assert_eq!(relative_path, "src/a.txt");
            }
            _ => panic!("expected workspace source"),
        }
        assert!(serde_json::from_str::<SftpUploadSource>(
            r#"{"kind":"workspace","workspaceRoot":"/forged","relativePath":"secret.txt"}"#
        )
        .is_err());
        let selected: SftpUploadSource =
            serde_json::from_str(r#"{"kind":"selected","capabilityId":"sel-1"}"#).unwrap();
        match selected {
            SftpUploadSource::Selected { capability_id } => assert_eq!(capability_id, "sel-1"),
            _ => panic!("expected selected source"),
        }
    }

    #[test]
    fn workspace_upload_source_rejects_forged_capability_id() {
        let selected = path_capability::SelectedPathRegistry::new();
        let workspaces = path_capability::WorkspacePathRegistry::new();
        let trust_dir = tempfile::tempdir().unwrap();
        let trust = crate::workspace_trust::WorkspaceTrustState::at(
            trust_dir.path().join("workspace-trust.json"),
        );

        let error = open_sftp_upload_source(
            &selected,
            &workspaces,
            &trust,
            SftpUploadSource::Workspace {
                workspace_id: "ws-forged".into(),
                relative_path: "secret.txt".into(),
            },
        )
        .err()
        .expect("forged workspace capability must fail");

        assert_eq!(
            error,
            PathCapabilityError::WorkspaceCapabilityMissing.as_code()
        );
    }

    #[test]
    fn workspace_upload_source_rejects_untrusted_workspace_before_file_open() {
        let workspace = tempfile::tempdir().unwrap();
        let selected = path_capability::SelectedPathRegistry::new();
        let workspaces = path_capability::WorkspacePathRegistry::new();
        let workspace_id = workspaces.activate(workspace.path()).unwrap();
        let trust_dir = tempfile::tempdir().unwrap();
        let trust = crate::workspace_trust::WorkspaceTrustState::at(
            trust_dir.path().join("workspace-trust.json"),
        );

        let error = open_sftp_upload_source(
            &selected,
            &workspaces,
            &trust,
            SftpUploadSource::Workspace {
                workspace_id,
                relative_path: "missing.txt".into(),
            },
        )
        .err()
        .expect("untrusted workspace must fail before file open");

        assert!(error.contains("untrustedWorkspace"), "{error}");
    }

    #[test]
    fn workspace_upload_source_accepts_trusted_exact_identity() {
        use std::io::Read as _;

        let workspace = tempfile::tempdir().unwrap();
        std::fs::write(workspace.path().join("report.txt"), "trusted").unwrap();
        let selected = path_capability::SelectedPathRegistry::new();
        let workspaces = path_capability::WorkspacePathRegistry::new();
        let workspace_id = workspaces.activate(workspace.path()).unwrap();
        let trust_dir = tempfile::tempdir().unwrap();
        let trust = crate::workspace_trust::WorkspaceTrustState::at(
            trust_dir.path().join("workspace-trust.json"),
        );
        trust.0.grant_for_tests(workspace.path().to_str().unwrap());

        let mut opened = open_sftp_upload_source(
            &selected,
            &workspaces,
            &trust,
            SftpUploadSource::Workspace {
                workspace_id,
                relative_path: "report.txt".into(),
            },
        )
        .unwrap();
        let mut content = String::new();
        opened.file.read_to_string(&mut content).unwrap();

        assert_eq!(content, "trusted");
    }

    #[cfg(unix)]
    #[test]
    fn workspace_upload_source_rejects_replaced_trusted_directory() {
        let parent = tempfile::tempdir().unwrap();
        let workspace = parent.path().join("workspace");
        let retired = parent.path().join("retired");
        std::fs::create_dir(&workspace).unwrap();
        std::fs::write(workspace.join("report.txt"), "old").unwrap();
        let selected = path_capability::SelectedPathRegistry::new();
        let workspaces = path_capability::WorkspacePathRegistry::new();
        let workspace_id = workspaces.activate(&workspace).unwrap();
        let trust_dir = tempfile::tempdir().unwrap();
        let trust = crate::workspace_trust::WorkspaceTrustState::at(
            trust_dir.path().join("workspace-trust.json"),
        );
        trust.0.grant_for_tests(workspace.to_str().unwrap());
        std::fs::rename(&workspace, &retired).unwrap();
        std::fs::create_dir(&workspace).unwrap();
        std::fs::write(workspace.join("report.txt"), "replacement").unwrap();

        let error = open_sftp_upload_source(
            &selected,
            &workspaces,
            &trust,
            SftpUploadSource::Workspace {
                workspace_id,
                relative_path: "report.txt".into(),
            },
        )
        .err()
        .expect("replaced workspace identity must fail");

        assert!(error.contains("identityMismatch"), "{error}");
    }

    #[test]
    fn selected_upload_source_does_not_require_workspace_trust() {
        use std::io::Read as _;

        let selected_file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(selected_file.path(), "selected").unwrap();
        let selected = path_capability::SelectedPathRegistry::new();
        let capability_id = selected
            .grant(selected_file.path().to_str().unwrap())
            .unwrap();
        let workspaces = path_capability::WorkspacePathRegistry::new();
        let trust_dir = tempfile::tempdir().unwrap();
        let trust = crate::workspace_trust::WorkspaceTrustState::at(
            trust_dir.path().join("workspace-trust.json"),
        );

        let mut opened = open_sftp_upload_source(
            &selected,
            &workspaces,
            &trust,
            SftpUploadSource::Selected { capability_id },
        )
        .unwrap();
        let mut content = String::new();
        opened.file.read_to_string(&mut content).unwrap();

        assert_eq!(content, "selected");
    }

    #[test]
    fn remote_leaf_rejects_hostile_names_before_any_io() {
        for path in [
            "/home/u/../.ssh/config",
            "/home/u/foo\\bar",
            "/home/u/C:foo",
            "/home/u/",
        ] {
            assert_eq!(
                reject_unsafe_remote_leaf(path).unwrap_err(),
                PathCapabilityError::UnsafeLeaf.as_code()
            );
        }
        assert!(reject_unsafe_remote_leaf("/home/u/report.pdf").is_ok());
    }

    #[test]
    fn download_dest_rejects_hostile_leaf_before_any_io() {
        let tmp = tempfile::tempdir().unwrap();
        for leaf in ["../.ssh/config", "foo\\bar", "C:foo", "//server/share/a"] {
            let err = path_capability::DestScratch::create(tmp.path(), leaf, "xfer-1")
                .err()
                .expect("hostile download leaf must fail");
            assert_eq!(err, PathCapabilityError::UnsafeLeaf);
        }
        assert!(tmp.path().read_dir().unwrap().next().is_none());
    }

    #[test]
    fn same_destination_transfers_are_rejected() {
        let set = path_capability::TransferDestSet::new();
        let first = set.acquire("remote:s1:/home/u/a.txt".into()).unwrap();
        assert_eq!(
            set.acquire("remote:s1:/home/u/a.txt".into())
                .err()
                .expect("same dest must be busy"),
            PathCapabilityError::DestinationBusy
        );
        drop(first);
        assert!(set.acquire("remote:s1:/home/u/a.txt".into()).is_ok());
    }

    #[test]
    fn sort_sftp_entries_puts_dirs_first_then_case_insensitive_name() {
        let mut entries = vec![
            entry("Zebra.txt", false),
            entry("apple", true),
            entry("beta.txt", false),
            entry("Alpha", true),
        ];
        sort_sftp_entries(&mut entries);
        let order: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(order, vec!["Alpha", "apple", "beta.txt", "Zebra.txt"]);
    }
}
