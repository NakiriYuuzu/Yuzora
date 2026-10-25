//! Read-only Git for the Spaces the desktop mapped to a folder. Git runs through
//! the Host crate under the desktop's workspace trust file, which is only read
//! here, never granted or revoked. Host errors can carry paths, so each one is
//! mapped to a fixed code and none is ever forwarded to a peer.
use crate::{admin::SpaceRoot, clock::Timestamp, herdr::valid_id, secrets};
use chrono::DateTime;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, PoisonError,
    },
    time::Duration,
};
use yuzora_host::{
    git_command::{GitCommand, HostGit},
    git_log, git_oid,
    git_service::{self, UnifiedDiffTarget},
    workspace_trust::WorkspaceTrustState,
};

const MAX_ROOTS: usize = 512;
const ID_BYTES: usize = 256;
const ROOT_BYTES: usize = 4096;
const PATH_BYTES: usize = 4096;
const STATUS_ENTRIES: usize = 1000;
const BRANCHES: usize = 500;
const TAGS: usize = 200;
const COMMIT_FILES: usize = 1000;
const DEFAULT_LOG_LIMIT: u32 = 50;
const MAX_LOG_LIMIT: u32 = 100;
const DIFF_BYTES: usize = 256 * 1024;
const AUDIT_WINDOW: Duration = Duration::from_secs(60);
const CURSORS: usize = 512;
const CURSOR_TTL: Duration = Duration::from_secs(30 * 60);
/// Random bytes of a cursor token (22 base64url characters).
const TOKEN_BYTES: usize = 16;
/// Key of the one repository each Space's `HostGit` opens.
const KEY: &str = "space";
/// Host answers that mean its cached root is stale, not that the read failed.
const STALE: [&str; 3] = [
    "git-repository-not-open",
    "git-repository-identity-mismatch",
    "git-repository-replaced",
];
/// Set by the log read itself, never by Host text.
const CURSOR_INVALID: &str = "git-cursor-invalid";

pub(crate) fn is_git(method: &str) -> bool {
    matches!(
        method,
        "git.status" | "git.branches" | "git.log" | "git.commit" | "git.diff"
    )
}

fn id_ok(value: &str) -> bool {
    (1..=ID_BYTES).contains(&value.len())
}
fn path_ok(value: &str) -> bool {
    (1..=PATH_BYTES).contains(&value.len()) && !value.contains('\0')
}
fn roots_ok(roots: &[SpaceRoot]) -> bool {
    let mut seen = HashSet::new();
    roots.len() <= MAX_ROOTS
        && roots.iter().all(|entry| {
            id_ok(&entry.session)
                && id_ok(&entry.workspace_id)
                && (1..=ROOT_BYTES).contains(&entry.root.len())
                && Path::new(&entry.root).is_absolute()
                && seen.insert((&entry.session, &entry.workspace_id))
        })
}

/// (session, workspace id) to the folder Git is read in.
pub(crate) struct Spaces {
    trust_store: PathBuf,
    allowlist: HashSet<String>,
    spaces: Mutex<HashMap<(String, String), Arc<Space>>>,
    cursors: Arc<Mutex<Cursors>>,
    /// Random per gateway run, so a generation never repeats across restarts.
    salt: String,
    generations: AtomicU64,
}
impl Spaces {
    pub(crate) fn new(
        trust_store: &Path,
        allowlist: &[String],
        roots: &[SpaceRoot],
    ) -> Result<Self, &'static str> {
        if !trust_store.is_absolute() {
            return Err("invalid-trust-store");
        }
        let spaces = Self {
            trust_store: trust_store.to_owned(),
            allowlist: allowlist.iter().cloned().collect(),
            spaces: Mutex::default(),
            cursors: Arc::default(),
            salt: secrets::encode(secrets::random::<6>()?.as_ref()),
            generations: AtomicU64::new(1),
        };
        if !spaces.replace(roots) {
            return Err("invalid-space-roots");
        }
        Ok(spaces)
    }
    /// Swaps the whole mapping. A Space with an unchanged root keeps its Git
    /// state; changed and removed ones lose it. Sessions outside the allowlist
    /// are dropped silently. False (nothing changed) when `roots` is invalid.
    pub(crate) fn replace(&self, roots: &[SpaceRoot]) -> bool {
        if !roots_ok(roots) {
            return false;
        }
        let mut spaces = self.spaces.lock().unwrap();
        let mut next = HashMap::new();
        for entry in roots
            .iter()
            .filter(|entry| self.allowlist.contains(&entry.session))
        {
            let key = (entry.session.clone(), entry.workspace_id.clone());
            let space = spaces
                .get(&key)
                .filter(|space| space.root == entry.root)
                .cloned()
                .unwrap_or_else(|| {
                    // A new, changed or re-added root is a new repository for
                    // the phone: its caches of the old one must go.
                    let generation = format!(
                        "{}{}",
                        self.salt,
                        self.generations.fetch_add(1, Ordering::Relaxed)
                    );
                    Space::new(entry.root.clone(), self.trust_store.clone(), generation)
                });
            next.insert(key, space);
        }
        *spaces = next;
        true
    }
    pub(crate) fn get(
        &self,
        session: &str,
        workspace_id: &str,
    ) -> Result<Arc<Space>, &'static str> {
        if !valid_id(session) || !self.allowlist.contains(session) {
            return Err("session-not-allowed");
        }
        let key = (session.to_owned(), workspace_id.to_owned());
        self.spaces
            .lock()
            .unwrap()
            .get(&key)
            .cloned()
            .ok_or("git-unavailable")
    }
    /// The cursor tokens `device` may use at time `now`.
    pub(crate) fn tokens(&self, device: &str, now: Timestamp) -> Tokens {
        Tokens {
            cursors: self.cursors.clone(),
            device: device.to_owned(),
            now,
        }
    }
    /// Adds `git: bool` to each workspace of a normalized snapshot, and for a
    /// mapped one `gitGeneration`: an opaque token that changes whenever the
    /// Space's root mapping does (never the folder itself).
    pub(crate) fn mark(&self, session: &str, snapshot: &mut Value) {
        let spaces = self.spaces.lock().unwrap();
        for workspace in snapshot["workspaces"].as_array_mut().into_iter().flatten() {
            let space = workspace["id"]
                .as_str()
                .and_then(|id| spaces.get(&(session.to_owned(), id.to_owned())));
            workspace["git"] = json!(space.is_some());
            if let Some(space) = space {
                workspace["gitGeneration"] = json!(space.generation);
            }
        }
    }
}

/// A short random token stands for one Host log cursor, which grows with the
/// number of refs and can outgrow a WS frame. Host cursors never reach a peer.
struct Cursor {
    device: String,
    session: String,
    workspace_id: String,
    host: String,
    expires: Timestamp,
}
#[derive(Default)]
struct Cursors {
    by_token: HashMap<String, Cursor>,
    /// Tokens, oldest first. Every one of them is in `by_token`.
    order: VecDeque<String>,
}

/// One caller's cursor tokens: only those given to its device, while they live.
pub(crate) struct Tokens {
    cursors: Arc<Mutex<Cursors>>,
    device: String,
    now: Timestamp,
}
impl Tokens {
    /// The Host cursor behind `token`, if it was issued to this device for this
    /// Space and has not expired.
    fn open(&self, session: &str, workspace_id: &str, token: &str) -> Option<String> {
        let cursors = self.cursors.lock().unwrap_or_else(PoisonError::into_inner);
        let cursor = cursors.by_token.get(token)?;
        (cursor.device == self.device
            && cursor.session == session
            && cursor.workspace_id == workspace_id
            && self.now < cursor.expires)
            .then(|| cursor.host.clone())
    }
    /// A new token for `host`. Past 512 tokens the oldest go; so do expired ones.
    fn issue(
        &self,
        session: &str,
        workspace_id: &str,
        host: String,
    ) -> Result<String, &'static str> {
        let random = secrets::random::<TOKEN_BYTES>().map_err(|_| "git-failed")?;
        let token = secrets::encode(random.as_ref());
        let mut guard = self.cursors.lock().unwrap_or_else(PoisonError::into_inner);
        let cursors = &mut *guard;
        while let Some(oldest) = cursors.order.front() {
            let expired = cursors.by_token[oldest].expires <= self.now;
            if !expired && cursors.order.len() < CURSORS {
                break;
            }
            let oldest = cursors.order.pop_front().expect("front exists");
            cursors.by_token.remove(&oldest);
        }
        cursors.order.push_back(token.clone());
        cursors.by_token.insert(
            token.clone(),
            Cursor {
                device: self.device.clone(),
                session: session.to_owned(),
                workspace_id: workspace_id.to_owned(),
                host,
                expires: self.now.after(CURSOR_TTL),
            },
        );
        Ok(token)
    }
}

pub(crate) struct Space {
    trust: WorkspaceTrustState,
    root: String,
    /// Changes with the mapping; see `Spaces::mark`.
    generation: String,
    state: Mutex<SpaceGit>,
}
#[derive(Default)]
struct SpaceGit {
    git: HostGit,
    /// Repository root Detect found; later reads must still be this directory.
    detected: Option<String>,
}
impl Space {
    /// Each Space has a trust state of its own: Detect binds a repository root
    /// to the Space that found it, and Spaces in subfolders of one repository
    /// must not take that binding over from each other.
    fn new(root: String, trust_store: PathBuf, generation: String) -> Arc<Self> {
        Arc::new(Self {
            trust: WorkspaceTrustState::at(trust_store),
            root,
            generation,
            state: Mutex::default(),
        })
    }
    /// Runs `read` on the detected repository root. Reads of one Space share a
    /// `HostGit` and so run one at a time. A stale root, or one that is no longer
    /// trusted (the desktop may have trusted the folder again), is detected again once.
    ///
    /// Once a repository root is detected, the Space's own folder is verified
    /// before every read: the Host check of that root passes on the root's own
    /// trust record, which would keep a Space the desktop revoked readable while
    /// its repository root is trusted. (Detect checks the folder itself, and a
    /// folder that is no repository needs no trust.)
    fn read(&self, read: impl Fn(&Path) -> Result<Value, String>) -> Result<Value, &'static str> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.detected.is_some() {
            if let Err(error) = self.trust.require_trusted(&self.root) {
                state.detected = None;
                return Err(classify(&error));
            }
        }
        let mut retried = false;
        loop {
            let root = match state.detected.clone() {
                Some(root) => root,
                None => {
                    let root = self.detect(&mut state.git)?;
                    state.detected = Some(root.clone());
                    root
                }
            };
            match state
                .git
                .read_root(&self.trust, KEY, Some(&root), |path| read(path))
            {
                Err(error)
                    if !retried
                        && (STALE.contains(&error.as_str())
                            || classify(&error) == "git-untrusted") =>
                {
                    retried = true;
                    state.detected = None;
                }
                result => return result.map_err(|error| classify(&error)),
            }
        }
    }
    fn detect(&self, git: &mut HostGit) -> Result<String, &'static str> {
        let environment = git
            .execute_root(&self.root, &self.trust, KEY, None, GitCommand::Detect)
            .map_err(|error| classify(&error))?;
        match environment["status"].as_str() {
            Some("ready") => environment["root"]
                .as_str()
                .map(str::to_owned)
                .ok_or("git-failed"),
            Some("notARepo") => Err("git-not-repository"),
            _ => Err("git-failed"),
        }
    }
}

/// Trust failures are the JSON `{"error":"<code>",…}` of the workspace trust
/// store (any kind of it, including a changed folder or a corrupt file). All
/// other text is Git's or the filesystem's, and may hold paths.
fn classify(error: &str) -> &'static str {
    if error == CURSOR_INVALID {
        return CURSOR_INVALID;
    }
    // Host's own fixed answers for a path no phone may read: a caller mistake.
    if error == git_service::DIFF_NOT_UNTRACKED || error == git_service::DIFF_GIT_PATH {
        return "invalid-params";
    }
    let code = serde_json::from_str::<Value>(error)
        .ok()
        .and_then(|value| value["error"].as_str().map(str::to_owned));
    match code.as_deref() {
        Some("untrustedWorkspace" | "identityMismatch" | "storeCorrupt" | "storeLocked") => {
            "git-untrusted"
        }
        _ => "git-failed",
    }
}

pub(crate) struct Request {
    pub(crate) session: String,
    pub(crate) workspace_id: String,
    call: Call,
}
enum Call {
    Status,
    Branches,
    Log { cursor: Option<String>, limit: u32 },
    Commit { hash: String },
    Diff(Diff),
}
enum Diff {
    Worktree {
        path: String,
        untracked: bool,
    },
    Staged {
        path: String,
        orig_path: Option<String>,
    },
    Commit {
        hash: String,
        path: String,
        orig_path: Option<String>,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SpaceParams {
    session: String,
    workspace_id: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LogParams {
    session: String,
    workspace_id: String,
    cursor: Option<String>,
    limit: Option<u32>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CommitParams {
    session: String,
    workspace_id: String,
    hash: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DiffParams {
    session: String,
    workspace_id: String,
    kind: DiffKind,
    path: String,
    orig_path: Option<String>,
    commit: Option<String>,
    untracked: Option<bool>,
}
#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum DiffKind {
    Worktree,
    Staged,
    Commit,
}

fn decode<T: DeserializeOwned>(params: Value) -> Result<T, &'static str> {
    serde_json::from_value(params).map_err(|_| "invalid-params")
}
/// Only a full object name: no refs, prefixes or revision expressions.
fn full_oid(value: &str) -> Result<String, &'static str> {
    git_oid::is_full_oid(value)
        .then(|| value.to_ascii_lowercase())
        .ok_or("invalid-params")
}

/// Everything about a `git.*` call that does not need the Space: shape, bounds
/// and defaults. `orig_path` only counts for staged and commit, `untracked` only
/// for worktree and `commit` only for commit; elsewhere they are ignored.
pub(crate) fn parse(method: &str, value: Value) -> Result<Request, &'static str> {
    let (session, workspace_id, call) = match method {
        "git.status" | "git.branches" => {
            let p: SpaceParams = decode(value)?;
            let call = if method == "git.status" {
                Call::Status
            } else {
                Call::Branches
            };
            (p.session, p.workspace_id, call)
        }
        "git.log" => {
            let p: LogParams = decode(value)?;
            let limit = p.limit.unwrap_or(DEFAULT_LOG_LIMIT);
            if !(1..=MAX_LOG_LIMIT).contains(&limit) {
                return Err("invalid-params");
            }
            let call = Call::Log {
                cursor: p.cursor,
                limit,
            };
            (p.session, p.workspace_id, call)
        }
        "git.commit" => {
            let p: CommitParams = decode(value)?;
            let call = Call::Commit {
                hash: full_oid(&p.hash)?,
            };
            (p.session, p.workspace_id, call)
        }
        "git.diff" => {
            let p: DiffParams = decode(value)?;
            if !path_ok(&p.path) || p.orig_path.as_deref().is_some_and(|path| !path_ok(path)) {
                return Err("invalid-params");
            }
            let diff = match p.kind {
                DiffKind::Worktree => Diff::Worktree {
                    path: p.path,
                    untracked: p.untracked.unwrap_or(false),
                },
                DiffKind::Staged => Diff::Staged {
                    path: p.path,
                    orig_path: p.orig_path,
                },
                DiffKind::Commit => Diff::Commit {
                    hash: full_oid(p.commit.as_deref().ok_or("invalid-params")?)?,
                    path: p.path,
                    orig_path: p.orig_path,
                },
            };
            (p.session, p.workspace_id, Call::Diff(diff))
        }
        _ => return Err("invalid-params"),
    };
    if !id_ok(&session) || !id_ok(&workspace_id) {
        return Err("invalid-params");
    }
    Ok(Request {
        session,
        workspace_id,
        call,
    })
}

impl Request {
    /// Blocking: call only from the gateway's bounded executor.
    pub(crate) fn run(&self, space: &Space, tokens: &Tokens) -> Result<Value, &'static str> {
        match &self.call {
            Call::Status => space.read(status),
            Call::Branches => space.read(branches),
            Call::Log { cursor, limit } => {
                let host = cursor
                    .as_deref()
                    .map(|token| {
                        tokens
                            .open(&self.session, &self.workspace_id, token)
                            .ok_or(CURSOR_INVALID)
                    })
                    .transpose()?;
                let mut page = space.read(|root| log(root, host.as_deref(), *limit))?;
                if let Some(next) = page["nextCursor"].as_str() {
                    page["nextCursor"] =
                        json!(tokens.issue(&self.session, &self.workspace_id, next.to_owned())?);
                }
                Ok(page)
            }
            Call::Commit { hash } => space.read(|root| commit(root, hash)),
            Call::Diff(diff) => space.read(|root| unified(root, diff)),
        }
    }
}

/// True when the list was cut.
fn cap<T>(list: &mut Vec<T>, limit: usize) -> bool {
    let cut = list.len() > limit;
    list.truncate(limit);
    cut
}
fn to_json(result: impl Serialize) -> Result<Value, String> {
    serde_json::to_value(result).map_err(|error| error.to_string())
}

fn status(root: &Path) -> Result<Value, String> {
    let mut status = git_service::status_of(root, None)?;
    let parsed = &mut status.parsed;
    let truncated = [
        cap(&mut parsed.staged, STATUS_ENTRIES),
        cap(&mut parsed.unstaged, STATUS_ENTRIES),
        cap(&mut parsed.untracked, STATUS_ENTRIES),
        cap(&mut parsed.conflicted, STATUS_ENTRIES),
    ]
    .contains(&true);
    let name = root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    Ok(json!({"repo":{"name":name},"status":status,"truncated":truncated}))
}

fn branches(root: &Path) -> Result<Value, String> {
    let mut list = git_service::branches(root)?;
    // Newest first, so the cap drops the oldest tags; undated ones go last.
    let date = |tag: &git_service::TagInfo| DateTime::parse_from_rfc3339(&tag.date).ok();
    list.tags
        .sort_by(|a, b| date(b).cmp(&date(a)).then_with(|| a.name.cmp(&b.name)));
    let truncated = [
        cap(&mut list.local, BRANCHES),
        cap(&mut list.remote, BRANCHES),
        cap(&mut list.tags, TAGS),
    ]
    .contains(&true);
    Ok(json!({"local":list.local,"remote":list.remote,"tags":list.tags,"truncated":truncated}))
}

fn log(root: &Path, cursor: Option<&str>, limit: u32) -> Result<Value, String> {
    let page = git_log::log_page(root, cursor, limit, None, None, None, None).map_err(|error| {
        if cursor.is_some() && error.starts_with("git log cursor") {
            CURSOR_INVALID.into()
        } else {
            error
        }
    })?;
    to_json(page)
}

fn commit(root: &Path, hash: &str) -> Result<Value, String> {
    let mut detail = git_log::commit_detail(root, hash)?;
    let truncated = cap(&mut detail.files, COMMIT_FILES);
    Ok(json!({
        "hash":hash,"subject":detail.subject,"body":detail.body,
        "authorName":detail.author_name,"authorEmail":detail.author_email,
        "timestamp":detail.timestamp,"parents":detail.parents,"files":detail.files,
        "totalAdditions":detail.total_additions,"totalDeletions":detail.total_deletions,
        "truncated":truncated
    }))
}

fn unified(root: &Path, diff: &Diff) -> Result<Value, String> {
    let target = match diff {
        Diff::Worktree { path, untracked } => UnifiedDiffTarget::Worktree {
            path,
            untracked: *untracked,
        },
        Diff::Staged { path, orig_path } => UnifiedDiffTarget::Staged {
            path,
            orig_path: orig_path.as_deref(),
        },
        Diff::Commit {
            hash,
            path,
            orig_path,
        } => UnifiedDiffTarget::Commit {
            hash,
            path,
            orig_path: orig_path.as_deref(),
        },
    };
    to_json(git_service::unified_diff(root, target, DIFF_BYTES)?)
}

/// Lets `git.read` through at most once a minute per Space for one peer.
/// Entries exist only for mapped Spaces (at most 512) and expire on their own.
#[derive(Default)]
pub(crate) struct AuditWindow(HashMap<(String, String), Timestamp>);
impl AuditWindow {
    pub(crate) fn due(&mut self, session: &str, workspace_id: &str, now: Timestamp) -> bool {
        self.0.retain(|_, until| *until > now);
        let key = (session.to_owned(), workspace_id.to_owned());
        if self.0.contains_key(&key) {
            return false;
        }
        self.0.insert(key, now.after(AUDIT_WINDOW));
        true
    }
}

#[cfg(test)]
#[path = "git_bridge_tests.rs"]
mod tests;
