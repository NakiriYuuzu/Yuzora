//! Backend-held workspace trust (F1).
//!
//! Grants are bound to a canonical path plus a filesystem identity (inode /
//! file index). Untrusted workspaces never authorize Git or package-script
//! execution. Renderer checks are UX only — this module is the authority.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const STORE_VERSION: u32 = 1;
const CHALLENGE_TTL: Duration = Duration::from_secs(60);
const STORE_FILE_NAME: &str = "workspace-trust.json";
const STORE_LOCK_FILE_NAME: &str = "workspace-trust.lock";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceIdentity {
    pub canonical_path: String,
    pub fs_identity: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TrustRecord {
    canonical_path: String,
    fs_identity: String,
    granted_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TrustDocument {
    version: u32,
    #[serde(default)]
    workspaces: Vec<TrustRecord>,
}

impl Default for TrustDocument {
    fn default() -> Self {
        Self {
            version: STORE_VERSION,
            workspaces: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
enum ChallengeKind {
    GrantWorkspace,
    Execute { command: String, digest: String },
    GrantAndExecute { command: String, digest: String },
}

#[derive(Clone, Debug)]
struct Challenge {
    id: String,
    kind: ChallengeKind,
    identity: WorkspaceIdentity,
    created: Instant,
}

impl Challenge {
    fn expired(&self) -> bool {
        self.created.elapsed() >= CHALLENGE_TTL
    }

    fn expires_at_ms(&self) -> u64 {
        let elapsed = self.created.elapsed();
        let remaining = CHALLENGE_TTL.saturating_sub(elapsed);
        now_ms().saturating_add(remaining.as_millis() as u64)
    }
}

#[derive(Debug)]
pub enum TrustError {
    Untrusted {
        identity: WorkspaceIdentity,
        challenge_id: String,
    },
    IdentityMismatch {
        identity: WorkspaceIdentity,
        challenge_id: String,
    },
    UnsupportedPath {
        reason: String,
    },
    StaleChallenge,
    ChallengeExpired,
    PersistFailed(String),
    StoreCorrupt,
    StoreLocked,
}

impl TrustError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Untrusted { .. } => "untrustedWorkspace",
            Self::IdentityMismatch { .. } => "identityMismatch",
            Self::UnsupportedPath { .. } => "unsupportedPath",
            Self::StaleChallenge => "staleChallenge",
            Self::ChallengeExpired => "challengeExpired",
            Self::PersistFailed(_) => "persistFailed",
            Self::StoreCorrupt => "storeCorrupt",
            Self::StoreLocked => "storeLocked",
        }
    }

    pub fn to_dto(&self) -> WorkspaceTrustErrorDto {
        match self {
            Self::Untrusted {
                identity,
                challenge_id,
            }
            | Self::IdentityMismatch {
                identity,
                challenge_id,
            } => WorkspaceTrustErrorDto {
                error: self.code().to_string(),
                canonical_path: Some(identity.canonical_path.clone()),
                challenge_id: Some(challenge_id.clone()),
            },
            other => WorkspaceTrustErrorDto {
                error: other.code().to_string(),
                canonical_path: None,
                challenge_id: None,
            },
        }
    }

    pub fn to_frontend(&self) -> String {
        serde_json::to_string(&self.to_dto())
            .unwrap_or_else(|_| format!("{{\"error\":\"{}\"}}", self.code()))
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceTrustErrorDto {
    pub error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub canonical_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub challenge_id: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceTrustStatusDto {
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub canonical_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fs_identity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub challenge_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo_present: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceTrustChallengeDto {
    pub challenge_id: String,
    pub canonical_path: String,
    pub fs_identity: String,
    pub repo_present: bool,
    pub expires_at: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceExecutionChallengeDto {
    pub challenge_id: String,
    pub canonical_path: String,
    pub command: String,
    pub command_digest: String,
    pub grants_trust: bool,
    pub trusted: bool,
    pub expires_at: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustedWorkspaceDto {
    pub canonical_path: String,
    pub fs_identity: String,
    pub granted_at: String,
}

#[derive(Clone, Debug)]
pub struct AuthorizedExecution {
    pub canonical_path: String,
    pub command: String,
}

struct TrustInner {
    challenges: HashMap<String, Challenge>,
    /// Session-only: git toplevel observed after a trusted detect, keyed by
    /// the git-root canonical path and bound to the workspace that authorized it.
    session_git_roots: HashMap<String, WorkspaceIdentity>,
}

pub struct WorkspaceTrustStore {
    path: PathBuf,
    inner: Mutex<TrustInner>,
    persist: Mutex<()>,
}

#[derive(Clone)]
pub struct WorkspaceTrustState(pub Arc<WorkspaceTrustStore>);

impl WorkspaceTrustState {
    pub fn production() -> Self {
        Self(Arc::new(WorkspaceTrustStore::at(default_store_path())))
    }

    pub fn at(path: PathBuf) -> Self {
        Self(Arc::new(WorkspaceTrustStore::at(path)))
    }

    pub fn require_trusted(&self, path: &str) -> Result<WorkspaceIdentity, String> {
        self.0
            .require_trusted(path)
            .map_err(|error| error.to_frontend())
    }

    pub fn require_trusted_git(&self, path: &str) -> Result<WorkspaceIdentity, String> {
        self.0
            .require_trusted_git(path)
            .map_err(|error| error.to_frontend())
    }

    pub fn bind_session_git_root(&self, identity: &WorkspaceIdentity, git_root: &str) {
        self.0.bind_session_git_root(identity, git_root);
    }

    pub fn authorize_execution(
        &self,
        workspace: &str,
        command: &str,
        challenge_id: &str,
    ) -> Result<AuthorizedExecution, String> {
        self.0.authorize_execution(workspace, command, challenge_id)
    }
}

pub fn default_store_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".yuzora")
        .join(STORE_FILE_NAME)
}

pub fn command_digest(command: &str) -> String {
    hex_encode(&Sha256::digest(command.as_bytes()))
}

pub fn project_repo_presence(path: &str) -> bool {
    let start = Path::new(path)
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from(path));
    let mut current = Some(start.as_path());
    while let Some(dir) = current {
        let marker = dir.join(".git");
        if marker.is_dir() || marker.is_file() {
            return true;
        }
        current = dir.parent();
    }
    false
}

pub fn observe_identity(path: &str) -> Result<WorkspaceIdentity, TrustError> {
    if path.is_empty() {
        return Err(TrustError::UnsupportedPath {
            reason: "empty path".into(),
        });
    }
    let canonical =
        Path::new(path)
            .canonicalize()
            .map_err(|error| TrustError::UnsupportedPath {
                reason: format!("canonicalize failed: {error}"),
            })?;
    let canonical_path = canonical
        .to_str()
        .ok_or_else(|| TrustError::UnsupportedPath {
            reason: "non-utf8 path".into(),
        })?
        .to_string();
    Ok(WorkspaceIdentity {
        canonical_path,
        fs_identity: platform_identity(&canonical)?,
    })
}

#[cfg(unix)]
fn platform_identity(path: &Path) -> Result<String, TrustError> {
    use std::os::unix::fs::MetadataExt;

    let metadata = fs::metadata(path).map_err(|error| TrustError::UnsupportedPath {
        reason: format!("metadata failed: {error}"),
    })?;
    if !metadata.is_dir() {
        return Err(TrustError::UnsupportedPath {
            reason: "workspace is not a directory".into(),
        });
    }
    Ok(format!("{}:{}", metadata.dev(), metadata.ino()))
}

#[cfg(windows)]
fn platform_identity(path: &Path) -> Result<String, TrustError> {
    use std::os::windows::fs::OpenOptionsExt;

    const FILE_READ_ATTRIBUTES: u32 = 0x0080;
    const FILE_SHARE_READ: u32 = 0x0000_0001;
    const FILE_SHARE_WRITE: u32 = 0x0000_0002;
    const FILE_SHARE_DELETE: u32 = 0x0000_0004;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;

    let file = OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
        .map_err(|error| TrustError::UnsupportedPath {
            reason: format!("open identity handle failed: {error}"),
        })?;
    let (volume, index, is_directory) = crate::path_capability::windows_file_identity(&file)
        .map_err(|_| TrustError::UnsupportedPath {
            reason: "read filesystem identity failed".into(),
        })?;
    if !is_directory {
        return Err(TrustError::UnsupportedPath {
            reason: "workspace is not a directory".into(),
        });
    }
    Ok(format!("{volume}:{index}"))
}

#[cfg(not(any(unix, windows)))]
fn platform_identity(_path: &Path) -> Result<String, TrustError> {
    Err(TrustError::UnsupportedPath {
        reason: "unsupported platform".into(),
    })
}

impl WorkspaceTrustStore {
    pub fn at(path: PathBuf) -> Self {
        Self {
            path,
            inner: Mutex::new(TrustInner {
                challenges: HashMap::new(),
                session_git_roots: HashMap::new(),
            }),
            persist: Mutex::new(()),
        }
    }

    pub fn require_trusted(&self, path: &str) -> Result<WorkspaceIdentity, TrustError> {
        let observed = observe_identity(path)?;
        match self.lookup(&observed)? {
            TrustLookup::Trusted => Ok(observed),
            TrustLookup::Replaced => {
                let challenge_id =
                    self.issue_challenge(ChallengeKind::GrantWorkspace, observed.clone());
                Err(TrustError::IdentityMismatch {
                    identity: observed,
                    challenge_id,
                })
            }
            TrustLookup::Unknown => {
                let challenge_id =
                    self.issue_challenge(ChallengeKind::GrantWorkspace, observed.clone());
                Err(TrustError::Untrusted {
                    identity: observed,
                    challenge_id,
                })
            }
        }
    }

    pub fn require_trusted_git(&self, path: &str) -> Result<WorkspaceIdentity, TrustError> {
        match self.require_trusted(path) {
            Ok(identity) => Ok(identity),
            Err(original) => {
                let observed = match observe_identity(path) {
                    Ok(identity) => identity,
                    Err(_) => return Err(original),
                };
                if self.session_git_root_allowed(&observed)? {
                    return Ok(observed);
                }
                Err(original)
            }
        }
    }

    pub fn bind_session_git_root(&self, workspace: &WorkspaceIdentity, git_root: &str) {
        let Ok(root) = observe_identity(git_root) else {
            return;
        };
        if root.canonical_path == workspace.canonical_path {
            return;
        }
        if let Ok(mut inner) = self.inner.lock() {
            inner
                .session_git_roots
                .insert(root.canonical_path, workspace.clone());
        }
    }

    pub fn status(&self, path: &str) -> Result<WorkspaceTrustStatusDto, String> {
        let observed = match observe_identity(path) {
            Ok(identity) => identity,
            Err(error) => {
                return Ok(WorkspaceTrustStatusDto {
                    state: "invalid".into(),
                    canonical_path: None,
                    fs_identity: None,
                    challenge_id: None,
                    repo_present: None,
                    reason: Some(error.code().to_string()),
                });
            }
        };
        match self
            .lookup(&observed)
            .map_err(|error| error.to_frontend())?
        {
            TrustLookup::Trusted => Ok(WorkspaceTrustStatusDto {
                state: "trusted".into(),
                canonical_path: Some(observed.canonical_path),
                fs_identity: Some(observed.fs_identity),
                challenge_id: None,
                repo_present: Some(project_repo_presence(path)),
                reason: None,
            }),
            TrustLookup::Replaced => {
                let challenge_id =
                    self.issue_challenge(ChallengeKind::GrantWorkspace, observed.clone());
                Ok(WorkspaceTrustStatusDto {
                    state: "invalid".into(),
                    canonical_path: Some(observed.canonical_path),
                    fs_identity: Some(observed.fs_identity),
                    challenge_id: Some(challenge_id),
                    repo_present: Some(project_repo_presence(path)),
                    reason: Some("identityMismatch".into()),
                })
            }
            TrustLookup::Unknown => {
                let challenge_id =
                    self.issue_challenge(ChallengeKind::GrantWorkspace, observed.clone());
                Ok(WorkspaceTrustStatusDto {
                    state: "untrusted".into(),
                    canonical_path: Some(observed.canonical_path),
                    fs_identity: Some(observed.fs_identity),
                    challenge_id: Some(challenge_id),
                    repo_present: Some(project_repo_presence(path)),
                    reason: None,
                })
            }
        }
    }

    pub fn list(&self) -> Result<Vec<TrustedWorkspaceDto>, String> {
        let document = self.load_document().map_err(|error| error.to_frontend())?;
        let mut entries = Vec::new();
        for record in document.workspaces {
            match observe_identity(&record.canonical_path) {
                Ok(identity)
                    if identity.canonical_path == record.canonical_path
                        && identity.fs_identity == record.fs_identity =>
                {
                    entries.push(TrustedWorkspaceDto {
                        canonical_path: record.canonical_path,
                        fs_identity: record.fs_identity,
                        granted_at: record.granted_at,
                    });
                }
                _ => {}
            }
        }
        Ok(entries)
    }

    pub fn issue_workspace_challenge(
        &self,
        path: &str,
    ) -> Result<WorkspaceTrustChallengeDto, String> {
        let observed = observe_identity(path).map_err(|error| error.to_frontend())?;
        let challenge_id = self.issue_challenge(ChallengeKind::GrantWorkspace, observed.clone());
        let expires_at = self.challenge_expiry(&challenge_id);
        Ok(WorkspaceTrustChallengeDto {
            challenge_id,
            canonical_path: observed.canonical_path,
            fs_identity: observed.fs_identity,
            repo_present: project_repo_presence(path),
            expires_at,
        })
    }

    pub fn issue_execution_challenge(
        &self,
        path: &str,
        command: &str,
    ) -> Result<WorkspaceExecutionChallengeDto, String> {
        if command.trim().is_empty() {
            return Err(TrustError::UnsupportedPath {
                reason: "empty command".into(),
            }
            .to_frontend());
        }
        let observed = observe_identity(path).map_err(|error| error.to_frontend())?;
        let trusted = matches!(
            self.lookup(&observed)
                .map_err(|error| error.to_frontend())?,
            TrustLookup::Trusted
        );
        let digest = command_digest(command);
        let kind = if trusted {
            ChallengeKind::Execute {
                command: command.to_string(),
                digest: digest.clone(),
            }
        } else {
            ChallengeKind::GrantAndExecute {
                command: command.to_string(),
                digest: digest.clone(),
            }
        };
        let challenge_id = self.issue_challenge(kind, observed.clone());
        let expires_at = self.challenge_expiry(&challenge_id);
        Ok(WorkspaceExecutionChallengeDto {
            challenge_id,
            canonical_path: observed.canonical_path,
            command: command.to_string(),
            command_digest: digest,
            grants_trust: !trusted,
            trusted,
            expires_at,
        })
    }

    pub fn grant(&self, challenge_id: &str) -> Result<WorkspaceTrustStatusDto, String> {
        let challenge = self
            .take_challenge(challenge_id)
            .map_err(|error| error.to_frontend())?;
        if !matches!(
            challenge.kind,
            ChallengeKind::GrantWorkspace | ChallengeKind::GrantAndExecute { .. }
        ) {
            return Err(TrustError::StaleChallenge.to_frontend());
        }
        let observed = observe_identity(&challenge.identity.canonical_path)
            .map_err(|error| error.to_frontend())?;
        if observed != challenge.identity {
            return Err(TrustError::IdentityMismatch {
                identity: observed,
                challenge_id: challenge_id.to_string(),
            }
            .to_frontend());
        }
        self.persist_grant(&observed)
            .map_err(|error| error.to_frontend())?;
        self.status(&observed.canonical_path)
    }

    pub fn revoke(&self, canonical_path: &str) -> Result<Vec<String>, String> {
        let mut stopped = vec![canonical_path.to_string()];
        if let Ok(observed) = observe_identity(canonical_path) {
            stopped.push(observed.canonical_path.clone());
            self.unbind_session_for(&observed);
            self.clear_challenges_for(&observed);
        }
        self.mutate_document(|document| {
            let before = document.workspaces.len();
            document.workspaces.retain(|record| {
                let keep = record.canonical_path != canonical_path
                    && !stopped.iter().any(|path| path == &record.canonical_path);
                if !keep {
                    stopped.push(record.canonical_path.clone());
                }
                keep
            });
            Ok(document.workspaces.len() != before)
        })
        .map_err(|error| error.to_frontend())?;
        stopped.sort();
        stopped.dedup();
        Ok(stopped)
    }

    pub fn authorize_execution(
        &self,
        workspace: &str,
        command: &str,
        challenge_id: &str,
    ) -> Result<AuthorizedExecution, String> {
        let observed = observe_identity(workspace).map_err(|error| error.to_frontend())?;
        let digest = command_digest(command);
        let challenge = self
            .take_challenge(challenge_id)
            .map_err(|error| error.to_frontend())?;
        if challenge.identity != observed {
            return Err(TrustError::IdentityMismatch {
                identity: observed,
                challenge_id: challenge_id.to_string(),
            }
            .to_frontend());
        }
        let (stored_command, stored_digest, grants_trust) = match challenge.kind {
            ChallengeKind::Execute { command, digest } => (command, digest, false),
            ChallengeKind::GrantAndExecute { command, digest } => (command, digest, true),
            ChallengeKind::GrantWorkspace => {
                return Err(TrustError::StaleChallenge.to_frontend());
            }
        };
        if stored_command != command || stored_digest != digest {
            return Err(TrustError::StaleChallenge.to_frontend());
        }
        if grants_trust {
            self.persist_grant(&observed)
                .map_err(|error| error.to_frontend())?;
        } else if !matches!(
            self.lookup(&observed)
                .map_err(|error| error.to_frontend())?,
            TrustLookup::Trusted
        ) {
            return Err(TrustError::Untrusted {
                identity: observed,
                challenge_id: challenge_id.to_string(),
            }
            .to_frontend());
        }
        Ok(AuthorizedExecution {
            canonical_path: observed.canonical_path,
            command: stored_command,
        })
    }

    pub fn grant_for_tests(&self, path: &str) -> WorkspaceIdentity {
        let identity = observe_identity(path).expect("test workspace identity");
        let challenge_id = self.issue_challenge(ChallengeKind::GrantWorkspace, identity.clone());
        self.grant(&challenge_id).expect("test grant");
        identity
    }

    #[cfg(test)]
    fn insert_expired_challenge(&self, kind: ChallengeKind, identity: WorkspaceIdentity) -> String {
        let id = random_token();
        let mut inner = self.inner.lock().expect("trust lock");
        inner.challenges.insert(
            id.clone(),
            Challenge {
                id: id.clone(),
                kind,
                identity,
                created: Instant::now()
                    .checked_sub(CHALLENGE_TTL + Duration::from_secs(1))
                    .unwrap_or_else(Instant::now),
            },
        );
        id
    }

    fn issue_challenge(&self, kind: ChallengeKind, identity: WorkspaceIdentity) -> String {
        let id = random_token();
        let mut inner = match self.inner.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        inner.challenges.retain(|_, challenge| {
            if challenge.expired() {
                return false;
            }
            let same_identity = challenge.identity == identity;
            let same_family = match (&challenge.kind, &kind) {
                (ChallengeKind::GrantWorkspace, ChallengeKind::GrantWorkspace) => true,
                (
                    ChallengeKind::Execute { .. } | ChallengeKind::GrantAndExecute { .. },
                    ChallengeKind::Execute { .. } | ChallengeKind::GrantAndExecute { .. },
                ) => true,
                _ => false,
            };
            !(same_identity && same_family)
        });
        inner.challenges.insert(
            id.clone(),
            Challenge {
                id: id.clone(),
                kind,
                identity,
                created: Instant::now(),
            },
        );
        id
    }

    fn take_challenge(&self, challenge_id: &str) -> Result<Challenge, TrustError> {
        let mut inner = self.inner.lock().map_err(|_| TrustError::StoreLocked)?;
        let challenge = inner
            .challenges
            .remove(challenge_id)
            .ok_or(TrustError::StaleChallenge)?;
        if challenge.id != challenge_id {
            return Err(TrustError::StaleChallenge);
        }
        if challenge.expired() {
            return Err(TrustError::ChallengeExpired);
        }
        Ok(challenge)
    }

    fn challenge_expiry(&self, challenge_id: &str) -> u64 {
        let inner = match self.inner.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        inner
            .challenges
            .get(challenge_id)
            .map(Challenge::expires_at_ms)
            .unwrap_or_else(now_ms)
    }

    fn lookup(&self, observed: &WorkspaceIdentity) -> Result<TrustLookup, TrustError> {
        let document = self.load_document()?;
        if let Some(record) = document
            .workspaces
            .iter()
            .find(|record| record.canonical_path == observed.canonical_path)
        {
            if record.fs_identity == observed.fs_identity {
                Ok(TrustLookup::Trusted)
            } else {
                Ok(TrustLookup::Replaced)
            }
        } else {
            Ok(TrustLookup::Unknown)
        }
    }

    fn session_git_root_allowed(&self, observed: &WorkspaceIdentity) -> Result<bool, TrustError> {
        let workspace = {
            let inner = self.inner.lock().map_err(|_| TrustError::StoreLocked)?;
            inner
                .session_git_roots
                .get(&observed.canonical_path)
                .cloned()
        };
        let Some(workspace) = workspace else {
            return Ok(false);
        };
        Ok(matches!(self.lookup(&workspace)?, TrustLookup::Trusted))
    }

    fn unbind_session_for(&self, workspace: &WorkspaceIdentity) {
        if let Ok(mut inner) = self.inner.lock() {
            inner
                .session_git_roots
                .retain(|_, bound| bound != workspace);
        }
    }

    fn clear_challenges_for(&self, workspace: &WorkspaceIdentity) {
        if let Ok(mut inner) = self.inner.lock() {
            inner
                .challenges
                .retain(|_, challenge| &challenge.identity != workspace);
        }
    }

    fn persist_grant(&self, identity: &WorkspaceIdentity) -> Result<(), TrustError> {
        self.mutate_document(|document| {
            let granted_at = rfc3339_now();
            if let Some(existing) = document
                .workspaces
                .iter_mut()
                .find(|record| record.canonical_path == identity.canonical_path)
            {
                existing.fs_identity = identity.fs_identity.clone();
                existing.granted_at = granted_at;
            } else {
                document.workspaces.push(TrustRecord {
                    canonical_path: identity.canonical_path.clone(),
                    fs_identity: identity.fs_identity.clone(),
                    granted_at,
                });
            }
            Ok(true)
        })
    }

    fn mutate_document<F>(&self, mutate: F) -> Result<(), TrustError>
    where
        F: FnOnce(&mut TrustDocument) -> Result<bool, TrustError>,
    {
        let _in_process = self.persist.lock().map_err(|_| TrustError::StoreLocked)?;
        let _cross_process = acquire_store_file_lock(&self.path)?;
        let mut document = self.load_document()?;
        if mutate(&mut document)? {
            self.persist_document(&document)?;
        }
        Ok(())
    }

    fn load_document(&self) -> Result<TrustDocument, TrustError> {
        match fs::read(&self.path) {
            Ok(bytes) => {
                let document: TrustDocument =
                    serde_json::from_slice(&bytes).map_err(|_| TrustError::StoreCorrupt)?;
                if document.version != STORE_VERSION {
                    return Err(TrustError::StoreCorrupt);
                }
                Ok(document)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(TrustDocument::default())
            }
            Err(error) => Err(TrustError::PersistFailed(error.to_string())),
        }
    }

    fn persist_document(&self, document: &TrustDocument) -> Result<(), TrustError> {
        let parent = self
            .path
            .parent()
            .ok_or_else(|| TrustError::PersistFailed("trust store path has no parent".into()))?;
        fs::create_dir_all(parent).map_err(|error| TrustError::PersistFailed(error.to_string()))?;
        let bytes = serde_json::to_vec_pretty(document)
            .map_err(|error| TrustError::PersistFailed(error.to_string()))?;
        let mut temporary = tempfile::Builder::new()
            .prefix(".workspace-trust-")
            .tempfile_in(parent)
            .map_err(|error| TrustError::PersistFailed(error.to_string()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            temporary
                .as_file()
                .set_permissions(fs::Permissions::from_mode(0o600))
                .map_err(|error| TrustError::PersistFailed(error.to_string()))?;
        }
        temporary
            .write_all(&bytes)
            .map_err(|error| TrustError::PersistFailed(error.to_string()))?;
        temporary
            .as_file()
            .sync_all()
            .map_err(|error| TrustError::PersistFailed(error.to_string()))?;
        temporary
            .persist(&self.path)
            .map_err(|error| TrustError::PersistFailed(error.error.to_string()))?;
        let dir =
            File::open(parent).map_err(|error| TrustError::PersistFailed(error.to_string()))?;
        dir.sync_all().map_err(|error| {
            TrustError::PersistFailed(format!("parent directory sync failed: {error}"))
        })?;
        Ok(())
    }
}

struct StoreFileLock {
    file: File,
}

fn acquire_store_file_lock(store_path: &Path) -> Result<StoreFileLock, TrustError> {
    let lock_path = store_path
        .parent()
        .map(|parent| parent.join(STORE_LOCK_FILE_NAME))
        .ok_or_else(|| TrustError::PersistFailed("trust store path has no parent".into()))?;
    if let Some(parent) = lock_path.parent() {
        fs::create_dir_all(parent).map_err(|error| TrustError::PersistFailed(error.to_string()))?;
    }
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(&lock_path)
        .map_err(|error| TrustError::PersistFailed(error.to_string()))?;
    lock_exclusive(&file)?;
    Ok(StoreFileLock { file })
}

#[cfg(unix)]
fn lock_exclusive(file: &File) -> Result<(), TrustError> {
    use std::os::unix::io::AsRawFd;
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
    if rc != 0 {
        return Err(TrustError::PersistFailed(
            std::io::Error::last_os_error().to_string(),
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn lock_exclusive(file: &File) -> Result<(), TrustError> {
    use std::os::windows::io::AsRawHandle;
    #[repr(C)]
    struct Overlapped {
        internal: usize,
        internal_high: usize,
        offset: u32,
        offset_high: u32,
        h_event: *mut core::ffi::c_void,
    }
    extern "system" {
        fn LockFileEx(
            file: *mut core::ffi::c_void,
            flags: u32,
            reserved: u32,
            bytes_to_lock_low: u32,
            bytes_to_lock_high: u32,
            overlapped: *mut Overlapped,
        ) -> i32;
    }
    const LOCKFILE_EXCLUSIVE_LOCK: u32 = 0x0000_0002;
    let mut overlapped = Overlapped {
        internal: 0,
        internal_high: 0,
        offset: 0,
        offset_high: 0,
        h_event: std::ptr::null_mut(),
    };
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
        return Err(TrustError::PersistFailed(
            std::io::Error::last_os_error().to_string(),
        ));
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn lock_exclusive(_file: &File) -> Result<(), TrustError> {
    Err(TrustError::PersistFailed(
        "cross-process trust store lock is unavailable".into(),
    ))
}

impl Drop for StoreFileLock {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            unsafe {
                libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
            }
        }
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            #[repr(C)]
            struct Overlapped {
                internal: usize,
                internal_high: usize,
                offset: u32,
                offset_high: u32,
                h_event: *mut core::ffi::c_void,
            }
            extern "system" {
                fn UnlockFileEx(
                    file: *mut core::ffi::c_void,
                    reserved: u32,
                    bytes_to_unlock_low: u32,
                    bytes_to_unlock_high: u32,
                    overlapped: *mut Overlapped,
                ) -> i32;
            }
            let mut overlapped = Overlapped {
                internal: 0,
                internal_high: 0,
                offset: 0,
                offset_high: 0,
                h_event: std::ptr::null_mut(),
            };
            unsafe {
                UnlockFileEx(self.file.as_raw_handle(), 0, 1, 0, &mut overlapped);
            }
        }
    }
}

enum TrustLookup {
    Trusted,
    Replaced,
    Unknown,
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn random_token() -> String {
    hex_encode(&rand::random::<[u8; 32]>())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn rfc3339_now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

#[tauri::command]
pub async fn workspace_trust_status(
    trust: tauri::State<'_, WorkspaceTrustState>,
    processes: tauri::State<'_, crate::process_service::ProcessState>,
    path: String,
) -> Result<WorkspaceTrustStatusDto, String> {
    let trust = trust.inner().clone();
    let manager = processes.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let status = trust.0.status(&path)?;
        if status.state == "invalid" && status.reason.as_deref() == Some("identityMismatch") {
            let _ = manager.stop_workspace(&path);
            if let Some(canonical) = &status.canonical_path {
                let _ = manager.stop_workspace(canonical);
            }
        }
        Ok(status)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn workspace_trust_list(
    trust: tauri::State<'_, WorkspaceTrustState>,
) -> Result<Vec<TrustedWorkspaceDto>, String> {
    let trust = trust.inner().clone();
    tauri::async_runtime::spawn_blocking(move || trust.0.list())
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn workspace_trust_challenge(
    trust: tauri::State<'_, WorkspaceTrustState>,
    path: String,
) -> Result<WorkspaceTrustChallengeDto, String> {
    let trust = trust.inner().clone();
    tauri::async_runtime::spawn_blocking(move || trust.0.issue_workspace_challenge(&path))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn workspace_trust_execution_challenge(
    trust: tauri::State<'_, WorkspaceTrustState>,
    path: String,
    command: String,
) -> Result<WorkspaceExecutionChallengeDto, String> {
    let trust = trust.inner().clone();
    tauri::async_runtime::spawn_blocking(move || trust.0.issue_execution_challenge(&path, &command))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn workspace_trust_grant(
    trust: tauri::State<'_, WorkspaceTrustState>,
    challenge_id: String,
) -> Result<WorkspaceTrustStatusDto, String> {
    let trust = trust.inner().clone();
    tauri::async_runtime::spawn_blocking(move || trust.0.grant(&challenge_id))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn workspace_trust_revoke(
    trust: tauri::State<'_, WorkspaceTrustState>,
    processes: tauri::State<'_, crate::process_service::ProcessState>,
    canonical_path: String,
) -> Result<Vec<TrustedWorkspaceDto>, String> {
    let trust = trust.inner().clone();
    let manager = processes.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let stopped = trust.0.revoke(&canonical_path)?;
        for path in stopped {
            let _ = manager.stop_workspace(&path);
        }
        trust.0.list()
    })
    .await
    .map_err(|error| error.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store() -> (tempfile::TempDir, WorkspaceTrustStore) {
        let tmp = tempfile::tempdir().unwrap();
        let store = WorkspaceTrustStore::at(tmp.path().join("workspace-trust.json"));
        (tmp, store)
    }

    fn make_workspace(tmp: &tempfile::TempDir, name: &str) -> PathBuf {
        let path = tmp.path().join(name);
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn default_store_path_lives_under_yuzora_home() {
        let path = default_store_path();
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some(STORE_FILE_NAME)
        );
        assert_eq!(
            path.parent()
                .and_then(|parent| parent.file_name())
                .and_then(|name| name.to_str()),
            Some(".yuzora")
        );
    }

    #[test]
    fn grant_survives_store_reload() {
        let (tmp, store) = temp_store();
        let workspace = make_workspace(&tmp, "repo");
        let identity = store.grant_for_tests(workspace.to_str().unwrap());
        let reloaded = WorkspaceTrustStore::at(tmp.path().join("workspace-trust.json"));
        let status = reloaded.status(workspace.to_str().unwrap()).unwrap();
        assert_eq!(status.state, "trusted");
        assert_eq!(
            status.canonical_path.as_deref(),
            Some(identity.canonical_path.as_str())
        );
        assert_eq!(
            status.fs_identity.as_deref(),
            Some(identity.fs_identity.as_str())
        );
    }

    #[test]
    fn directory_replacement_invalidates_grant() {
        let (tmp, store) = temp_store();
        let workspace = make_workspace(&tmp, "repo");
        store.grant_for_tests(workspace.to_str().unwrap());
        fs::remove_dir_all(&workspace).unwrap();
        fs::create_dir_all(&workspace).unwrap();
        let status = store.status(workspace.to_str().unwrap()).unwrap();
        assert_eq!(status.state, "invalid");
        assert_eq!(status.reason.as_deref(), Some("identityMismatch"));
        assert!(store.require_trusted(workspace.to_str().unwrap()).is_err());
    }

    #[test]
    fn revoke_drops_grant_and_lists_nothing() {
        let (tmp, store) = temp_store();
        let workspace = make_workspace(&tmp, "repo");
        let identity = store.grant_for_tests(workspace.to_str().unwrap());
        store.revoke(&identity.canonical_path).unwrap();
        let status = store.status(workspace.to_str().unwrap()).unwrap();
        assert_eq!(status.state, "untrusted");
        assert!(store.list().unwrap().is_empty());
    }

    #[test]
    fn stale_and_expired_challenges_fail_closed() {
        let (tmp, store) = temp_store();
        let workspace = make_workspace(&tmp, "repo");
        let identity = observe_identity(workspace.to_str().unwrap()).unwrap();
        assert!(store.grant("missing").is_err());
        let expired = store.insert_expired_challenge(ChallengeKind::GrantWorkspace, identity);
        let error = store.grant(&expired).unwrap_err();
        assert!(error.contains("challengeExpired"));
    }

    #[test]
    fn execution_requires_matching_single_use_challenge() {
        let (tmp, store) = temp_store();
        let workspace = make_workspace(&tmp, "repo");
        store.grant_for_tests(workspace.to_str().unwrap());
        let issued = store
            .issue_execution_challenge(workspace.to_str().unwrap(), "bun run dev")
            .unwrap();
        assert!(!issued.grants_trust);
        let authorized = store
            .authorize_execution(
                workspace.to_str().unwrap(),
                "bun run dev",
                &issued.challenge_id,
            )
            .unwrap();
        assert_eq!(authorized.command, "bun run dev");
        let replay = store.authorize_execution(
            workspace.to_str().unwrap(),
            "bun run dev",
            &issued.challenge_id,
        );
        assert!(replay.unwrap_err().contains("staleChallenge"));
    }

    #[test]
    fn execution_challenge_can_grant_and_rejects_command_mismatch() {
        let (tmp, store) = temp_store();
        let workspace = make_workspace(&tmp, "repo");
        let issued = store
            .issue_execution_challenge(workspace.to_str().unwrap(), "bun run dev")
            .unwrap();
        assert!(issued.grants_trust);
        let mismatch = store.authorize_execution(
            workspace.to_str().unwrap(),
            "bun run evil",
            &issued.challenge_id,
        );
        assert!(mismatch.unwrap_err().contains("staleChallenge"));
        let retry = store
            .issue_execution_challenge(workspace.to_str().unwrap(), "bun run dev")
            .unwrap();
        store
            .authorize_execution(
                workspace.to_str().unwrap(),
                "bun run dev",
                &retry.challenge_id,
            )
            .unwrap();
        assert_eq!(
            store.status(workspace.to_str().unwrap()).unwrap().state,
            "trusted"
        );
    }

    #[test]
    fn unknown_store_version_is_fail_closed() {
        let (tmp, store) = temp_store();
        let workspace = make_workspace(&tmp, "repo");
        fs::write(
            tmp.path().join("workspace-trust.json"),
            r#"{"version":99,"workspaces":[]}"#,
        )
        .unwrap();
        assert!(store.status(workspace.to_str().unwrap()).is_err());
        assert!(store.require_trusted(workspace.to_str().unwrap()).is_err());
    }

    #[test]
    fn missing_path_is_unsupported() {
        let (tmp, store) = temp_store();
        let missing = tmp.path().join("gone");
        let status = store.status(missing.to_str().unwrap()).unwrap();
        assert_eq!(status.state, "invalid");
        assert_eq!(status.reason.as_deref(), Some("unsupportedPath"));
    }

    #[cfg(unix)]
    #[test]
    fn persist_uses_owner_only_mode() {
        use std::os::unix::fs::PermissionsExt;
        let (tmp, store) = temp_store();
        let workspace = make_workspace(&tmp, "repo");
        store.grant_for_tests(workspace.to_str().unwrap());
        let mode = fs::metadata(tmp.path().join("workspace-trust.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn session_git_root_is_allowed_only_while_workspace_remains_trusted() {
        let (tmp, store) = temp_store();
        let workspace = make_workspace(&tmp, "app");
        let git_root = make_workspace(&tmp, "repo");
        let identity = store.grant_for_tests(workspace.to_str().unwrap());
        store.bind_session_git_root(&identity, git_root.to_str().unwrap());
        assert!(store
            .require_trusted_git(git_root.to_str().unwrap())
            .is_ok());
        store.revoke(&identity.canonical_path).unwrap();
        assert!(store
            .require_trusted_git(git_root.to_str().unwrap())
            .is_err());
    }

    #[test]
    fn project_repo_presence_uses_filesystem_only() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = make_workspace(&tmp, "repo");
        assert!(!project_repo_presence(repo.to_str().unwrap()));
        fs::create_dir(repo.join(".git")).unwrap();
        assert!(project_repo_presence(repo.to_str().unwrap()));
        let nested = make_workspace(&tmp, "repo/nested");
        assert!(project_repo_presence(nested.to_str().unwrap()));
    }

    #[test]
    fn concurrent_independent_grants_are_preserved() {
        let (tmp, store) = temp_store();
        let first = make_workspace(&tmp, "one");
        let second = make_workspace(&tmp, "two");
        let store_path = tmp.path().join("workspace-trust.json");
        let other = WorkspaceTrustStore::at(store_path);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                store.grant_for_tests(first.to_str().unwrap());
            });
            scope.spawn(|| {
                other.grant_for_tests(second.to_str().unwrap());
            });
        });
        let listed = store.list().unwrap();
        let paths: Vec<&str> = listed
            .iter()
            .map(|entry| entry.canonical_path.as_str())
            .collect();
        let first_id = observe_identity(first.to_str().unwrap()).unwrap();
        let second_id = observe_identity(second.to_str().unwrap()).unwrap();
        assert!(
            paths.contains(&first_id.canonical_path.as_str()),
            "{paths:?}"
        );
        assert!(
            paths.contains(&second_id.canonical_path.as_str()),
            "{paths:?}"
        );
    }

    #[test]
    fn completed_revoke_cannot_be_resurrected_by_stale_grant() {
        let (tmp, store) = temp_store();
        let keep = make_workspace(&tmp, "keep");
        let revoke = make_workspace(&tmp, "revoke");
        store.grant_for_tests(keep.to_str().unwrap());
        let revoke_identity = store.grant_for_tests(revoke.to_str().unwrap());
        let store_path = tmp.path().join("workspace-trust.json");
        let other = WorkspaceTrustStore::at(store_path);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                store.revoke(&revoke_identity.canonical_path).unwrap();
            });
            scope.spawn(|| {
                other.grant_for_tests(keep.to_str().unwrap());
            });
        });
        let listed = store.list().unwrap();
        let paths: Vec<&str> = listed
            .iter()
            .map(|entry| entry.canonical_path.as_str())
            .collect();
        let keep_id = observe_identity(keep.to_str().unwrap()).unwrap();
        assert!(
            paths.contains(&keep_id.canonical_path.as_str()),
            "{paths:?}"
        );
        assert!(
            !paths.contains(&revoke_identity.canonical_path.as_str()),
            "revoked workspace was resurrected: {paths:?}"
        );
        assert_eq!(
            store.status(revoke.to_str().unwrap()).unwrap().state,
            "untrusted"
        );
    }
}
