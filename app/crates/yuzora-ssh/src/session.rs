//! SSH sessions: one per host, shared by SFTP, tunnels and anything else that
//! needs the host. [`Manager::session`] reuses an open one or connects —
//! through ProxyJump hops or a ProxyCommand when the config says so — checks
//! the host key against known_hosts (asking the person about a new or changed
//! key), and logs in ([`crate::auth`]).

use crate::config::{StrictHostKeyChecking, safe_name};
use crate::context::Context;
use crate::error::{ErrorKind, Result, SshError};
use crate::hosts::{self, HostDef, Machine};
use crate::known_hosts::{self, Verdict};
use crate::prompt::{Answer, PROMPT_TIMEOUT, Prompt, Prompts};
use base64::Engine;
use russh::client::{self, Handle};
use russh::keys::{HashAlg, PublicKey};
use russh_sftp::client::SftpSession;
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};
use tokio::io::{AsyncRead, AsyncWrite};

/// TCP connect (ConnectTimeout overrides it).
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Key exchange and login, not counting time spent waiting for the person.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);
/// ProxyJump hops a connection may go through, counting a jump host's own.
const MAX_HOPS: usize = 6;
/// SFTP requests fail after this long without an answer (seconds).
const SFTP_TIMEOUT_SECS: u64 = 60;

/// Whether a connection may ask the person (host keys, passwords).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interaction {
    Ask,
    Never,
}

/// The host key a session was verified with.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Verified {
    pub algorithm: String,
    /// "ED25519", "RSA", "ECDSA"…
    pub kind: String,
    pub fingerprint: String,
    /// The known_hosts file that vouched for it.
    pub source: Option<PathBuf>,
    /// Trusted during this connection (a new or changed key accepted).
    pub newly_trusted: bool,
}

/// "ssh-ed25519" → "ED25519", "ecdsa-sha2-nistp256" → "ECDSA", "ssh-rsa" → "RSA".
pub fn key_kind(algorithm: &str) -> String {
    let a = algorithm.to_ascii_lowercase();
    if a.contains("ed25519") {
        "ED25519".into()
    } else if a.contains("ed448") {
        "ED448".into()
    } else if a.contains("ecdsa") {
        "ECDSA".into()
    } else if a.contains("rsa") {
        "RSA".into()
    } else if a.contains("dss") {
        "DSA".into()
    } else {
        algorithm.to_ascii_uppercase()
    }
}

/// Checks the server's key for one connection; remembers what it decided.
struct KeyCheck {
    host_id: String,
    label: String,
    /// The name looked up in known_hosts (HostKeyAlias, else the host name) and port.
    lookup: String,
    port: u16,
    files: Vec<PathBuf>,
    own_file: PathBuf,
    strict: StrictHostKeyChecking,
    prompts: Option<Arc<Prompts>>,
    outcome: Mutex<Option<Result<Verified>>>,
}

impl KeyCheck {
    async fn decide(&self, key: &PublicKey) -> Result<Verified> {
        let algorithm = key.algorithm().as_str().to_owned();
        let blob = key.to_bytes().map_err(|e| SshError::new(ErrorKind::Connect, e.to_string()))?;
        let encoded = base64::engine::general_purpose::STANDARD.encode(blob);
        let fingerprint = key.fingerprint(HashAlg::Sha256).to_string();
        let endpoint = known_hosts::lookup_name(&self.lookup, self.port);
        let verified = |source: Option<PathBuf>, newly_trusted: bool| Verified { algorithm: algorithm.clone(), kind: key_kind(&algorithm), fingerprint: fingerprint.clone(), source, newly_trusted };
        let trust = || {
            known_hosts::trust(&self.own_file, &self.lookup, self.port, &algorithm, &encoded)
                .map(|_| verified(Some(self.own_file.clone()), true))
                .map_err(|e| SshError::text(ErrorKind::Io, "known-hosts-write", &[("why", e.to_string())]))
        };
        let ask = |kind: &'static str| {
            let mut p = Prompt::new(&self.host_id, &self.label, kind);
            p.endpoint = Some(endpoint.clone());
            p.algorithm = Some(algorithm.clone());
            p.fingerprint = Some(fingerprint.clone());
            p
        };
        match known_hosts::check(&self.files, &self.lookup, self.port, &algorithm, &encoded) {
            Verdict::Trusted { line } => Ok(verified(Some(line.file), false)),
            Verdict::Revoked { .. } => Err(SshError::text(ErrorKind::HostKeyRevoked, "host-key-revoked", &[("host", endpoint.clone()), ("fingerprint", fingerprint.clone())])),
            Verdict::Changed { previous } => {
                let changed = || SshError::text(ErrorKind::HostKeyChanged, "host-key-changed", &[("host", endpoint.clone()), ("fingerprint", fingerprint.clone())]);
                let Some(prompts) = &self.prompts else { return Err(changed()) };
                let mut p = ask("hostKeyChanged");
                p.previous = previous.iter().map(|l| l.fingerprint.clone()).collect();
                p.previous_source = previous.first().map(|l| format!("{}:{}", l.file.display(), l.line));
                match prompts.ask(p, PROMPT_TIMEOUT).await {
                    Some(Answer::Trust(true)) => trust(),
                    None => Err(changed()),
                    Some(_) => Err(SshError::text(ErrorKind::HostKeyRejected, "host-key-rejected", &[("host", endpoint.clone())])),
                }
            }
            Verdict::Unknown { .. } => match self.strict {
                StrictHostKeyChecking::No | StrictHostKeyChecking::AcceptNew => trust(),
                StrictHostKeyChecking::Yes => Err(SshError::text(ErrorKind::HostKeyUnknown, "host-key-strict", &[("host", endpoint.clone()), ("fingerprint", fingerprint.clone())])),
                StrictHostKeyChecking::Ask => {
                    let Some(prompts) = &self.prompts else {
                        return Err(SshError::text(ErrorKind::HostKeyUnknown, "host-key-unknown", &[("host", endpoint.clone()), ("fingerprint", fingerprint.clone())]));
                    };
                    match prompts.ask(ask("hostKey"), PROMPT_TIMEOUT).await {
                        Some(Answer::Trust(true)) => trust(),
                        None => Err(SshError::text(ErrorKind::Timeout, "prompt-timeout", &[])),
                        Some(_) => Err(SshError::text(ErrorKind::HostKeyRejected, "host-key-rejected", &[("host", endpoint.clone())])),
                    }
                }
            },
        }
    }
}

/// russh's side of a connection: the host key check.
pub struct Client {
    check: Arc<KeyCheck>,
}

impl client::Handler for Client {
    type Error = russh::Error;

    async fn check_server_key(&mut self, key: &PublicKey) -> std::result::Result<bool, Self::Error> {
        let decided = self.check.decide(key).await;
        let ok = decided.is_ok();
        *self.check.outcome.lock().unwrap_or_else(|e| e.into_inner()) = Some(decided);
        Ok(ok)
    }
}

trait Stream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Stream for T {}

/// One logged-in SSH connection to a host.
pub struct HostSession {
    pub host_id: String,
    pub label: String,
    pub target: String,
    handle: Handle<Client>,
    pub verified: Verified,
    /// "publickey", "password", "keyboard-interactive" or "none".
    pub method: &'static str,
    pub connected_at: SystemTime,
    latency_ms: AtomicU64,
    /// Jump hosts this connection runs through; dropping them ends it.
    _hops: Vec<Arc<HostSession>>,
    _proxy: Option<tokio::process::Child>,
    sftp: tokio::sync::Mutex<Option<Arc<SftpSession>>>,
}

impl HostSession {
    pub fn is_closed(&self) -> bool {
        self.handle.is_closed()
    }

    /// The last measured round trip, in ms.
    pub fn latency_ms(&self) -> Option<u64> {
        match self.latency_ms.load(Ordering::Relaxed) {
            u64::MAX => None,
            ms => Some(ms),
        }
    }

    /// Measures one round trip (an SSH keepalive with a reply).
    pub async fn ping(&self) -> Result<u64> {
        let start = Instant::now();
        tokio::time::timeout(Duration::from_secs(10), self.handle.send_ping()).await.map_err(|_| SshError::text(ErrorKind::Timeout, "timeout", &[]))?.map_err(|_| SshError::closed())?;
        let ms = start.elapsed().as_millis() as u64;
        self.latency_ms.store(ms, Ordering::Relaxed);
        Ok(ms)
    }

    /// A TCP connection from the server to `host:port` (a tunnel's far end, the next jump).
    pub async fn direct_tcpip(&self, host: &str, port: u16) -> Result<russh::Channel<client::Msg>> {
        self.handle
            .channel_open_direct_tcpip(host.to_owned(), port as u32, "127.0.0.1", 0)
            .await
            .map_err(|e| SshError::text(ErrorKind::Connect, "forward-refused", &[("target", format!("{host}:{port}")), ("why", e.to_string())]))
    }

    /// The host's SFTP subsystem, opened once and shared.
    pub async fn sftp(&self) -> Result<Arc<SftpSession>> {
        let mut slot = self.sftp.lock().await;
        if let Some(s) = slot.as_ref()
            && !s.is_closed()
        {
            return Ok(s.clone());
        }
        let channel = self.handle.channel_open_session().await.map_err(|e| SshError::new(ErrorKind::Sftp, e.to_string()))?;
        channel.request_subsystem(true, "sftp").await.map_err(|e| SshError::new(ErrorKind::Sftp, e.to_string()))?;
        let sftp = SftpSession::new(channel.into_stream()).await.map_err(|e| SshError::text(ErrorKind::Sftp, "sftp-unavailable", &[("why", e.to_string())]))?;
        sftp.set_timeout(SFTP_TIMEOUT_SECS);
        let sftp = Arc::new(sftp);
        *slot = Some(sftp.clone());
        Ok(sftp)
    }

    pub async fn close(&self) {
        if let Some(s) = self.sftp.lock().await.take() {
            let _ = s.close().await;
        }
        let _ = self.handle.disconnect(russh::Disconnect::ByApplication, "", "en").await;
    }
}

/// What the host list shows about a host's connection.
#[derive(Clone, Debug, Default)]
struct HostState {
    connecting: bool,
    error: Option<SshError>,
}

/// Clears a host's "connecting" when its connect future is dropped unfinished.
struct Connecting<'a> {
    states: &'a Mutex<HashMap<String, HostState>>,
    id: String,
    settled: bool,
}

impl Drop for Connecting<'_> {
    fn drop(&mut self) {
        if !self.settled {
            lock(self.states).remove(&self.id);
        }
    }
}

/// Every session, the questions they ask, and what runs on them.
pub struct Manager {
    pub ctx: Context,
    pub prompts: Arc<Prompts>,
    machines: Mutex<Vec<Machine>>,
    sessions: Mutex<HashMap<String, Arc<HostSession>>>,
    locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    states: Mutex<HashMap<String, HostState>>,
    pub(crate) transfers: crate::transfer::Queue,
    pub(crate) tunnels: crate::tunnel::Registry,
    pub(crate) edits: crate::edit::Registry,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The app's manager, over the person's real files.
pub fn manager() -> Arc<Manager> {
    static GLOBAL: OnceLock<Arc<Manager>> = OnceLock::new();
    GLOBAL.get_or_init(|| Manager::new(Context::system())).clone()
}

impl Manager {
    pub fn new(ctx: Context) -> Arc<Self> {
        Arc::new(Self {
            ctx,
            prompts: Arc::new(Prompts::default()),
            machines: Mutex::new(Vec::new()),
            sessions: Mutex::new(HashMap::new()),
            locks: Mutex::new(HashMap::new()),
            states: Mutex::new(HashMap::new()),
            transfers: Default::default(),
            tunnels: Default::default(),
            edits: Default::default(),
        })
    }

    /// The herdr machines the last listing was given (`hc machine ls`).
    pub fn set_machines(&self, machines: Vec<Machine>) {
        *lock(&self.machines) = machines;
    }

    pub fn machines(&self) -> Vec<Machine> {
        lock(&self.machines).clone()
    }

    pub fn lookup(&self, key: &str) -> Result<HostDef> {
        hosts::lookup(&self.ctx, &self.machines(), key)
    }

    /// An open session for `key`, if there is one.
    pub fn open_session(&self, key: &str) -> Option<Arc<HostSession>> {
        let id = self.lookup(key).ok()?.id;
        let s = lock(&self.sessions).get(&id).cloned()?;
        (!s.is_closed()).then_some(s)
    }

    /// The session for a host (id, alias, machine or destination): the open one, or a new connection.
    pub async fn session(&self, key: &str, interaction: Interaction) -> Result<Arc<HostSession>> {
        let def = self.lookup(key)?;
        let gate = lock(&self.locks).entry(def.id.clone()).or_default().clone();
        let _one_at_a_time = gate.lock().await;
        if let Some(s) = lock(&self.sessions).get(&def.id).cloned() {
            if !s.is_closed() {
                return Ok(s);
            }
        }
        lock(&self.states).insert(def.id.clone(), HostState { connecting: true, error: None });
        let mut connecting = Connecting { states: &self.states, id: def.id.clone(), settled: false };
        let result = self.connect(&def, interaction, 0).await.map(Arc::new);
        connecting.settled = true;
        let mut states = lock(&self.states);
        match &result {
            Ok(s) => {
                states.insert(def.id.clone(), HostState::default());
                lock(&self.sessions).insert(def.id.clone(), s.clone());
            }
            Err(e) => {
                states.insert(def.id.clone(), HostState { connecting: false, error: Some(e.clone()) });
            }
        }
        result
    }

    /// Ends a host's session and everything on it (tunnels, SFTP).
    pub async fn disconnect(&self, key: &str) {
        let Ok(def) = self.lookup(key) else { return };
        self.prompts.cancel_host(&def.id);
        self.tunnels.close_host(&def.id);
        let session = lock(&self.sessions).remove(&def.id);
        lock(&self.states).remove(&def.id);
        if let Some(s) = session {
            s.close().await;
        }
    }

    /// The host list with each host's connection state.
    pub fn hosts_json(&self) -> serde_json::Value {
        let sessions = lock(&self.sessions).clone();
        let states = lock(&self.states).clone();
        let list = hosts::list(&self.ctx, &self.machines());
        serde_json::Value::Array(
            list.iter()
                .map(|h| {
                    let mut v = h.to_json();
                    let session = sessions.get(&h.id).filter(|s| !s.is_closed());
                    let st = states.get(&h.id).cloned().unwrap_or_default();
                    v["state"] = serde_json::json!(if session.is_some() {
                        "connected"
                    } else if st.connecting {
                        "connecting"
                    } else if st.error.is_some() {
                        "error"
                    } else {
                        "idle"
                    });
                    v["latencyMs"] = serde_json::json!(session.and_then(|s| s.latency_ms()));
                    v["error"] = serde_json::json!(st.error.as_ref().map(|e| e.message.clone()));
                    v["errorCode"] = serde_json::json!(st.error.as_ref().map(|e| e.code()));
                    v["verified"] = serde_json::json!(session.map(|s| &s.verified));
                    v["method"] = serde_json::json!(session.map(|s| s.method));
                    v
                })
                .collect(),
        )
    }

    /// Only the hosts with a session or a state (connecting, failed): cheap to poll.
    pub fn states_json(&self) -> serde_json::Value {
        let sessions = lock(&self.sessions).clone();
        let states = lock(&self.states).clone();
        let mut ids: Vec<&String> = sessions.keys().chain(states.keys()).collect();
        ids.sort();
        ids.dedup();
        serde_json::Value::Array(
            ids.into_iter()
                .map(|id| {
                    let session = sessions.get(id).filter(|s| !s.is_closed());
                    let st = states.get(id).cloned().unwrap_or_default();
                    let state = if session.is_some() { "connected" } else if st.connecting { "connecting" } else if st.error.is_some() { "error" } else { "idle" };
                    serde_json::json!({ "id": id, "state": state, "latencyMs": session.and_then(|s| s.latency_ms()), "error": st.error.as_ref().map(|e| e.message.clone()) })
                })
                .collect(),
        )
    }

    /// The status bar's line, if there is anything to say.
    pub fn status(&self) -> Option<(String, &'static str)> {
        let t = crate::strings::t;
        if let Some((text, tone)) = self.transfers.status_line() {
            return Some((text, tone));
        }
        let connecting: Vec<String> = {
            let states = lock(&self.states);
            states.iter().filter(|(_, s)| s.connecting).map(|(id, _)| id.clone()).collect()
        };
        if let Some(id) = connecting.first() {
            let name = self.lookup(id).map(|d| d.name).unwrap_or_else(|_| id.clone());
            return Some((t("status.connecting", &[("host", name)]), "working"));
        }
        let newest = lock(&self.sessions).values().filter(|s| !s.is_closed()).max_by_key(|s| s.connected_at).map(|s| (s.label.clone(), s.verified.kind.clone()));
        newest.map(|(host, kind)| (t("status.verified", &[("host", host), ("kind", kind)]), "done"))
    }

    async fn connect(&self, def: &HostDef, interaction: Interaction, depth: usize) -> Result<HostSession> {
        if depth > MAX_HOPS {
            return Err(SshError::text(ErrorKind::Connect, "too-many-hops", &[("host", def.name.clone())]));
        }
        let r = &def.resolved;
        let connect_timeout = r.connect_timeout.map(Duration::from_secs).unwrap_or(CONNECT_TIMEOUT);
        if !r.proxy_jump.is_empty() {
            // The first jump is reached as `ssh <jump>` would (its own config, its own
            // ProxyJump too); later ones through the hop before, ignoring their own.
            let first = self.lookup(&hosts::jump_text(&r.proxy_jump[0]))?;
            let mut hops = vec![Arc::new(Box::pin(self.connect(&first, interaction, depth + 1)).await?)];
            for jump in &r.proxy_jump[1..] {
                let mut next = self.lookup(&hosts::jump_text(jump))?;
                next.resolved.proxy_jump.clear();
                next.resolved.proxy_command = None;
                let prev = hops.last().cloned().ok_or_else(SshError::closed)?;
                let channel = prev.direct_tcpip(&next.resolved.hostname, next.resolved.port).await?;
                hops.push(Arc::new(self.handshake(&next, Box::new(channel.into_stream()), interaction, Vec::new(), None).await?));
            }
            let last = hops.last().cloned().ok_or_else(SshError::closed)?;
            let channel = last.direct_tcpip(&r.hostname, r.port).await?;
            return self.handshake(def, Box::new(channel.into_stream()), interaction, hops, None).await;
        }
        if let Some(command) = r.proxy_command.as_deref() {
            // %h and %r went into the shell line (OpenSSH refuses these too).
            if let Some(bad) = [&r.hostname, &r.user].into_iter().find(|s| !s.is_empty() && !safe_name(s)) {
                return Err(SshError::text(ErrorKind::Invalid, "unsafe-name", &[("name", bad.clone())]));
            }
            let (stream, child) = spawn_proxy(command).map_err(|e| SshError::text(ErrorKind::Connect, "proxy-command", &[("why", e.to_string())]))?;
            return self.handshake(def, Box::new(stream), interaction, Vec::new(), Some(child)).await;
        }
        let addr = (r.hostname.trim_start_matches('[').trim_end_matches(']').to_owned(), r.port);
        let tcp = tokio::time::timeout(connect_timeout, tokio::net::TcpStream::connect(addr))
            .await
            .map_err(|_| SshError::text(ErrorKind::Timeout, "connect-timeout", &[("target", def.target())]))?
            .map_err(|e| SshError::text(ErrorKind::Connect, "connect-failed", &[("target", def.target()), ("why", e.to_string())]))?;
        let _ = tcp.set_nodelay(true);
        self.handshake(def, Box::new(tcp), interaction, Vec::new(), None).await
    }

    async fn handshake(&self, def: &HostDef, stream: Box<dyn Stream>, interaction: Interaction, hops: Vec<Arc<HostSession>>, proxy: Option<tokio::process::Child>) -> Result<HostSession> {
        let r = &def.resolved;
        let mut files = vec![self.ctx.known_hosts.clone()];
        if self.ctx.system_known_hosts {
            files.extend(r.user_known_hosts_files.iter().cloned());
            files.extend(r.global_known_hosts_files.iter().cloned());
        }
        let prompts = (interaction == Interaction::Ask).then(|| self.prompts.clone());
        let check = Arc::new(KeyCheck {
            host_id: def.id.clone(),
            label: def.name.clone(),
            lookup: r.host_key_alias.clone().unwrap_or_else(|| r.hostname.clone()),
            // OpenSSH looks a HostKeyAlias up as written, never as [alias]:port.
            port: if r.host_key_alias.is_some() { 22 } else { r.port },
            files,
            own_file: self.ctx.known_hosts.clone(),
            strict: r.strict_host_key_checking,
            prompts: prompts.clone(),
            outcome: Mutex::new(None),
        });
        let config = Arc::new(client::Config {
            keepalive_interval: Some(Duration::from_secs(r.server_alive_interval.filter(|s| *s > 0).unwrap_or(30))),
            keepalive_max: r.server_alive_count_max.map(|n| n as usize).unwrap_or(3),
            nodelay: true,
            ..Default::default()
        });
        // Waiting for the person doesn't count against the handshake.
        let limit = HANDSHAKE_TIMEOUT + if prompts.is_some() { PROMPT_TIMEOUT } else { Duration::ZERO };
        let connected = tokio::time::timeout(limit, client::connect_stream(config, stream, Client { check: check.clone() })).await;
        let decided = lock(&check.outcome).take();
        let mut handle = match connected {
            Err(_) => return Err(SshError::text(ErrorKind::Timeout, "handshake-timeout", &[("target", def.target())])),
            Ok(Err(e)) => {
                return Err(match decided {
                    Some(Err(why)) => why,
                    _ => SshError::text(ErrorKind::Connect, "handshake-failed", &[("target", def.target()), ("why", e.to_string())]),
                });
            }
            Ok(Ok(h)) => h,
        };
        let verified = match decided {
            Some(Ok(v)) => v,
            Some(Err(e)) => return Err(e),
            None => return Err(SshError::text(ErrorKind::Connect, "handshake-failed", &[("target", def.target()), ("why", "no host key".into())])),
        };
        let login = crate::auth::Login { ctx: &self.ctx, def, prompts: prompts.as_deref() };
        let method = tokio::time::timeout(limit + PROMPT_TIMEOUT * 2, login.run(&mut handle))
            .await
            .map_err(|_| SshError::text(ErrorKind::Timeout, "handshake-timeout", &[("target", def.target())]))??;
        let session = HostSession {
            host_id: def.id.clone(),
            label: def.name.clone(),
            target: def.target(),
            handle,
            verified,
            method,
            connected_at: SystemTime::now(),
            latency_ms: AtomicU64::new(u64::MAX),
            _hops: hops,
            _proxy: proxy,
            sftp: tokio::sync::Mutex::new(None),
        };
        let _ = session.ping().await;
        Ok(session)
    }
}

/// ProxyCommand: the command's stdin and stdout are the connection.
fn spawn_proxy(command: &str) -> std::io::Result<(impl AsyncRead + AsyncWrite + Unpin + Send + 'static, tokio::process::Child)> {
    #[cfg(windows)]
    let mut cmd = {
        let mut c = tokio::process::Command::new("cmd");
        c.args(["/C", command]);
        c
    };
    #[cfg(not(windows))]
    let mut cmd = {
        let mut c = tokio::process::Command::new("/bin/sh");
        c.args(["-c", &format!("exec {command}")]);
        c
    };
    cmd.stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).kill_on_drop(true);
    let mut child = cmd.spawn()?;
    let out = child.stdout.take().ok_or_else(|| std::io::Error::other("no stdout"))?;
    let input = child.stdin.take().ok_or_else(|| std::io::Error::other("no stdin"))?;
    Ok((tokio::io::join(out, input), child))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_kinds_read_as_people_say_them() {
        assert_eq!(key_kind("ssh-ed25519"), "ED25519");
        assert_eq!(key_kind("ecdsa-sha2-nistp256"), "ECDSA");
        assert_eq!(key_kind("ssh-rsa"), "RSA");
        assert_eq!(key_kind("rsa-sha2-512"), "RSA");
        assert_eq!(key_kind("sk-ssh-ed25519@openssh.com"), "ED25519");
    }
}
