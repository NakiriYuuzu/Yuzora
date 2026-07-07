use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use russh::client::{self, Handle};
use russh::keys::{load_secret_key, HashAlg, PrivateKeyWithHashAlg, PublicKey};
use russh::{ChannelMsg, Disconnect};
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::OpenFlags;
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio::sync::Mutex as AsyncMutex;

use crate::logging;

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

/// Stable known-hosts map key for a host endpoint.
fn host_port_key(host: &str, port: u16) -> String {
    format!("{host}:{port}")
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
fn serialize_known_hosts(hosts: &BTreeMap<String, String>) -> String {
    serde_json::to_string_pretty(hosts).unwrap_or_else(|_| "{}".to_string())
}

/// TOFU decision for a presented host key given what (if anything) is pinned.
#[derive(Debug, PartialEq, Eq)]
enum HostKeyDecision {
    /// Endpoint not seen before — trust and remember it.
    New,
    /// Presented key matches the pinned fingerprint — trust.
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

/// The full host-key verdict, folding the store's read result into the TOFU
/// decision. Pure over `read`, so the fail-closed / trust / reject branches are
/// unit-tested without a live handshake.
#[derive(Debug, PartialEq, Eq)]
enum HostKeyEval {
    /// Trust the key. `persist` carries the updated store to write back when a
    /// new key was just pinned; `None` on a match (nothing to write).
    Accept {
        known: bool,
        persist: Option<BTreeMap<String, String>>,
    },
    /// Abort the handshake. `corrupt` holds the reason when the store was
    /// unreadable/damaged (fail-closed); `None` for a changed (re-keyed) host.
    Reject { corrupt: Option<String> },
}

fn evaluate_host_key(
    read: Result<BTreeMap<String, String>, String>,
    endpoint: &str,
    presented: &str,
) -> HostKeyEval {
    let mut hosts = match read {
        Ok(hosts) => hosts,
        // A corrupt store must never be read as "nothing pinned": fail closed so
        // a changed/hostile key is not silently accepted and re-pinned.
        Err(reason) => {
            return HostKeyEval::Reject {
                corrupt: Some(reason),
            }
        }
    };
    match decide_host_key(hosts.get(endpoint).map(String::as_str), presented) {
        HostKeyDecision::Changed => HostKeyEval::Reject { corrupt: None },
        HostKeyDecision::Match => HostKeyEval::Accept {
            known: true,
            persist: None,
        },
        HostKeyDecision::New => {
            hosts.insert(endpoint.to_string(), presented.to_string());
            HostKeyEval::Accept {
                known: false,
                persist: Some(hosts),
            }
        }
    }
}

/// Best-effort write of the known-hosts store (creating the parent dir). A
/// failure only means the key isn't remembered next time; it never aborts the
/// current, already-trusted connection.
fn persist_known_hosts(path: &Path, hosts: &BTreeMap<String, String>) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, serialize_known_hosts(hosts));
}

/// What `check_server_key` recorded for the surrounding `connect` to act on.
#[derive(Default)]
struct CheckOutcome {
    fingerprint: Option<String>,
    /// The key matched a previously-pinned entry (vs. first contact).
    known: bool,
    /// The key changed from the pinned fingerprint and was rejected.
    rejected: bool,
    /// The known-hosts store was corrupt; the handshake was failed closed. Holds
    /// the parse/read reason so `connect` can surface a repair hint.
    corrupt: Option<String>,
}

/// TOFU handler with known-hosts pinning. First contact with an endpoint trusts
/// and persists the key; a later matching key is trusted; a later *changed* key
/// is REJECTED here — russh aborts the handshake with `UnknownKey` before any
/// authentication packet, so no password/passphrase is ever sent to an
/// unverified server. Residual gap: a first-seen key is still trusted without an
/// interactive pre-auth confirmation (see the SSH TOFU report).
struct Client {
    host: String,
    port: u16,
    known_hosts_path: PathBuf,
    outcome: Arc<Mutex<CheckOutcome>>,
}

impl client::Handler for Client {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKey,
    ) -> Result<bool, Self::Error> {
        let fp = fingerprint_sha256(server_public_key);
        let key = host_port_key(&self.host, self.port);

        match evaluate_host_key(read_known_hosts(&self.known_hosts_path), &key, &fp) {
            HostKeyEval::Reject { corrupt } => {
                let mut outcome = self.outcome.lock().unwrap();
                outcome.fingerprint = Some(fp);
                match corrupt {
                    Some(reason) => outcome.corrupt = Some(reason),
                    None => outcome.rejected = true,
                }
                Ok(false)
            }
            HostKeyEval::Accept { known, persist } => {
                if let Some(hosts) = persist {
                    persist_known_hosts(&self.known_hosts_path, &hosts);
                }
                let mut outcome = self.outcome.lock().unwrap();
                outcome.fingerprint = Some(fp);
                outcome.known = known;
                Ok(true)
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
    pub size: u64,
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
    known_hosts_path: PathBuf,
}

pub struct SshState(pub Arc<SshManager>);

impl SshManager {
    pub fn new(_app: AppHandle) -> Self {
        Self::with_log(Box::new(logging::write_global))
    }

    #[cfg(test)]
    fn for_test() -> Self {
        Self::with_log(Box::new(|_| {}))
    }

    fn with_log(log: LogFn) -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            log,
            known_hosts_path: default_known_hosts_path(),
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
        let handler = Client {
            host: host.clone(),
            port,
            known_hosts_path: self.known_hosts_path.clone(),
            outcome: outcome.clone(),
        };

        let connect_fut = client::connect(config, (host.clone(), port), handler);
        let mut session = match tokio::time::timeout(CONNECT_TIMEOUT, connect_fut).await {
            Err(_) => {
                return Err(format!(
                    "連線逾時：{host}:{port} 在 {} 秒內沒有回應",
                    CONNECT_TIMEOUT.as_secs()
                ))
            }
            Ok(Err(e)) => {
                // A rejected host key aborts the handshake with `UnknownKey`
                // before any credential is sent — surface it as a distinct,
                // actionable warning rather than a generic transport error.
                let outcome = outcome.lock().unwrap();
                if let Some(reason) = &outcome.corrupt {
                    return Err(format!(
                        "known_hosts 檔案損毀，連線已中止（{reason}）；請修復或重設 ~/.yuzora/known_hosts.json 後再試"
                    ));
                }
                if outcome.rejected {
                    return Err(format!(
                        "主機金鑰驗證失敗：{host}:{port} 的 fingerprint 與已記錄的不符，連線已中止（伺服器金鑰可能已更換，或遭到中間人攻擊）"
                    ));
                }
                return Err(format!("無法連線到 {host}:{port}：{e}"));
            }
            Ok(Ok(session)) => session,
        };

        let authenticated = match auth {
            SshAuth::Password { password } => session
                .authenticate_password(user.clone(), password)
                .await
                .map_err(|e| format!("SSH 認證發生錯誤：{e}"))?
                .success(),
            SshAuth::Key {
                key_path,
                passphrase,
            } => {
                let key = load_secret_key(&key_path, passphrase.as_deref())
                    .map_err(|e| format!("無法讀取私鑰 {key_path}：{e}"))?;
                let hash = session
                    .best_supported_rsa_hash()
                    .await
                    .map_err(|e| format!("SSH 認證發生錯誤：{e}"))?
                    .flatten();
                session
                    .authenticate_publickey(
                        user.clone(),
                        PrivateKeyWithHashAlg::new(Arc::new(key), hash),
                    )
                    .await
                    .map_err(|e| format!("SSH 認證發生錯誤：{e}"))?
                    .success()
            }
        };

        if !authenticated {
            return Err("SSH 認證失敗：帳號、密碼或金鑰不正確".to_string());
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
                SftpEntry {
                    name: entry.file_name(),
                    path: entry.path(),
                    is_dir: ft.is_dir(),
                    is_symlink: ft.is_symlink(),
                    size: entry.metadata().size.unwrap_or(0),
                }
            })
            .collect();
        sort_sftp_entries(&mut entries);
        Ok(SftpListing { cwd, entries })
    }

    async fn sftp_mkdir(&self, session_id: &str, path: &str) -> Result<(), String> {
        let sftp = self.ensure_sftp(session_id).await?;
        sftp.create_dir(path.to_string())
            .await
            .map_err(|e| format!("無法建立遠端資料夾：{e}"))
    }

    async fn sftp_rename(&self, session_id: &str, from: &str, to: &str) -> Result<(), String> {
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
        session_id: &str,
        transfer_id: &str,
        local_path: &str,
        remote_dir: &str,
    ) -> Result<(), String> {
        let sftp = self.ensure_sftp(session_id).await?;
        let name = std::path::Path::new(local_path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .ok_or_else(|| format!("無法解析本地檔名：{local_path}"))?;
        let remote_path = remote_join(remote_dir, &name);
        // Stream into a temp sibling and rename into place, so an interrupted
        // upload never leaves a half-written file at (or clobbers) the target.
        let temp_path = remote_join(remote_dir, &temp_transfer_name(&name, transfer_id));

        let total = tokio::fs::metadata(local_path)
            .await
            .map(|m| m.len())
            .unwrap_or(0);
        let mut local = tokio::fs::File::open(local_path)
            .await
            .map_err(|e| format!("無法開啟本地檔案 {local_path}：{e}"))?;
        let mut remote = sftp
            .open_with_flags(
                temp_path.clone(),
                OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE,
            )
            .await
            .map_err(|e| format!("無法建立遠端暫存檔 {temp_path}：{e}"))?;

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
                    .map_err(|e| format!("讀取本地檔案失敗：{e}"))?;
                if n == 0 {
                    break;
                }
                remote
                    .write_all(&buf[..n])
                    .await
                    .map_err(|e| format!("寫入遠端檔案失敗：{e}"))?;
                transferred += n as u64;
                if transferred - last_emit >= SFTP_PROGRESS_STEP {
                    last_emit = transferred;
                    self.emit_progress(app, session_id, transfer_id, transferred, total, false);
                }
            }
            remote
                .shutdown()
                .await
                .map_err(|e| format!("關閉遠端暫存檔失敗：{e}"))
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
            if let Err(e) = sftp.remove_file(remote_path.clone()).await {
                let _ = sftp.remove_file(temp_path.clone()).await;
                return Err(format!("無法覆寫遠端檔案 {remote_path}：{e}"));
            }
        }
        if let Err(e) = sftp.rename(temp_path.clone(), remote_path.clone()).await {
            let _ = sftp.remove_file(temp_path.clone()).await;
            return Err(format!("無法將暫存檔移動到 {remote_path}：{e}"));
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
        local_path: &str,
    ) -> Result<(), String> {
        let sftp = self.ensure_sftp(session_id).await?;
        let total = sftp
            .metadata(remote_path.to_string())
            .await
            .ok()
            .and_then(|m| m.size)
            .unwrap_or(0);
        let mut remote = sftp
            .open(remote_path.to_string())
            .await
            .map_err(|e| format!("無法開啟遠端檔案 {remote_path}：{e}"))?;

        // Write into a temp sibling and atomically rename into place, so a partial
        // download never truncates or clobbers an existing local file.
        let dest = std::path::Path::new(local_path);
        let file_name = dest
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .ok_or_else(|| format!("無法解析本地檔名：{local_path}"))?;
        let temp_name = temp_transfer_name(&file_name, transfer_id);
        let temp_local = match dest.parent() {
            Some(parent) => parent.join(&temp_name),
            None => PathBuf::from(&temp_name),
        };
        let mut out = tokio::fs::File::create(&temp_local)
            .await
            .map_err(|e| format!("無法建立本地暫存檔 {}：{e}", temp_local.display()))?;

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
                    .map_err(|e| format!("讀取遠端檔案失敗：{e}"))?;
                if n == 0 {
                    break;
                }
                out.write_all(&buf[..n])
                    .await
                    .map_err(|e| format!("寫入本地檔案失敗：{e}"))?;
                transferred += n as u64;
                if transferred - last_emit >= SFTP_PROGRESS_STEP {
                    last_emit = transferred;
                    self.emit_progress(app, session_id, transfer_id, transferred, total, false);
                }
            }
            out.flush()
                .await
                .map_err(|e| format!("關閉本地暫存檔失敗：{e}"))
        }
        .await;

        // Close the temp file so the rename sees a released handle on every platform.
        drop(out);
        if let Err(e) = copy {
            let _ = tokio::fs::remove_file(&temp_local).await;
            return Err(e);
        }
        if let Err(e) = tokio::fs::rename(&temp_local, dest).await {
            let _ = tokio::fs::remove_file(&temp_local).await;
            return Err(format!("無法將暫存檔移動到 {local_path}：{e}"));
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
        self.sessions.lock().unwrap().clear();
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
                // TOFU: fingerprint verified against the known-hosts store; a
                // changed key would have been rejected before authentication.
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
    session_id: String,
    transfer_id: String,
    local_path: String,
    remote_dir: String,
) -> Result<(), String> {
    state
        .0
        .sftp_upload(&app, &session_id, &transfer_id, &local_path, &remote_dir)
        .await
}

#[tauri::command]
pub async fn sftp_download(
    app: AppHandle,
    state: tauri::State<'_, SshState>,
    session_id: String,
    transfer_id: String,
    remote_path: String,
    local_path: String,
) -> Result<(), String> {
    state
        .0
        .sftp_download(&app, &session_id, &transfer_id, &remote_path, &local_path)
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

    #[test]
    fn get_shell_reports_missing_session_and_unopened_shell() {
        let manager = SshManager::for_test();
        assert!(manager.get_shell("nope").is_err());
        assert!(manager.write("nope", "x").is_err());
        assert!(manager.resize("nope", 80, 24).is_err());
    }

    #[test]
    fn host_port_key_formats_endpoint() {
        assert_eq!(host_port_key("example.com", 22), "example.com:22");
        assert_eq!(host_port_key("10.0.0.5", 2222), "10.0.0.5:2222");
    }

    #[test]
    fn first_contact_is_new_and_trusted() {
        // Nothing pinned for this endpoint yet → trust-on-first-use.
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
        let endpoint = host_port_key("example.com", 22);

        // First contact pins the presented key and asks to persist it.
        match evaluate_host_key(Ok(BTreeMap::new()), &endpoint, SAMPLE_FINGERPRINT) {
            HostKeyEval::Accept {
                known: false,
                persist: Some(map),
            } => assert_eq!(
                map.get(&endpoint).map(String::as_str),
                Some(SAMPLE_FINGERPRINT)
            ),
            other => panic!("expected first-contact accept, got {other:?}"),
        }

        // A matching pinned key is trusted with nothing to write back.
        let mut pinned = BTreeMap::new();
        pinned.insert(endpoint.clone(), SAMPLE_FINGERPRINT.to_string());
        assert_eq!(
            evaluate_host_key(Ok(pinned.clone()), &endpoint, SAMPLE_FINGERPRINT),
            HostKeyEval::Accept {
                known: true,
                persist: None,
            }
        );

        // A changed key is rejected (not corrupt).
        assert_eq!(
            evaluate_host_key(Ok(pinned), &endpoint, "SHA256:some-other-key"),
            HostKeyEval::Reject { corrupt: None }
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
            &host_port_key("example.com", 22),
            SAMPLE_FINGERPRINT,
        ) {
            HostKeyEval::Reject { corrupt } => assert!(corrupt.is_some()),
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
        let restored = parse_known_hosts(&serialize_known_hosts(&hosts)).unwrap();
        assert_eq!(restored, hosts);
    }

    #[test]
    fn persist_creates_parent_dir_and_reloads() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nested").join("known_hosts.json");
        let mut hosts = BTreeMap::new();
        hosts.insert(
            host_port_key("example.com", 22),
            SAMPLE_FINGERPRINT.to_string(),
        );
        persist_known_hosts(&path, &hosts);
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(parse_known_hosts(&content).unwrap(), hosts);
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
            size: 0,
        }
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
