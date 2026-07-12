//! Crash-consistent, non-secret database profile persistence and credential sagas.
//!
//! This module is deliberately the only persistence authority for saved database
//! descriptors. The file contains no credential material: vault mutations are
//! coordinated through a write-ahead `PendingOperation` ledger.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::{self, OpenOptions};
use std::future::Future;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::db_credentials::{
    CredentialGeneration, DatabaseCredentialStore, VaultError, VaultErrorKind,
};
use crate::db_result_session::ResultSessionState;
use crate::db_service::{
    self, ConnectionGeneration, ConnectionId, CredentialInput, CredentialState, DbHandle,
    DbOpenConfig, DbState, DescriptorId, LiveConnection, LiveDatabaseEngine, ProfileCreateRequest,
    ProfileDescriptor, ProfileTarget, ProfileUpdateRequest, TestConnectionRequest,
    TestConnectionResult,
};

const PROFILE_REPOSITORY_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredProfile {
    pub descriptor_id: DescriptorId,
    #[serde(default = "default_config_generation")]
    pub config_generation: u64,
    pub name: String,
    pub target: ProfileTarget,
    pub credential_state: CredentialState,
    pub active_credential_generation: Option<CredentialGeneration>,
}

fn default_config_generation() -> u64 {
    1
}

impl StoredProfile {
    fn descriptor(&self) -> ProfileDescriptor {
        ProfileDescriptor {
            descriptor_id: self.descriptor_id.clone(),
            config_generation: self.config_generation,
            name: self.name.clone(),
            target: self.target.clone(),
            credential_state: self.credential_state.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PendingOperationKind {
    PendingCreate,
    PendingReplace,
    CleanupOld,
    PendingForget,
    PendingRemoveCredential,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum PendingOperation {
    PendingCreate {
        operation_id: String,
        profile: StoredProfile,
        credential_generation: CredentialGeneration,
    },
    PendingReplace {
        operation_id: String,
        descriptor_id: DescriptorId,
        replacement: StoredProfile,
        old_generation: Option<CredentialGeneration>,
        new_generation: CredentialGeneration,
    },
    CleanupOld {
        operation_id: String,
        descriptor_id: DescriptorId,
        old_generation: CredentialGeneration,
        active_generation: CredentialGeneration,
    },
    PendingForget {
        operation_id: String,
        descriptor_id: DescriptorId,
        generations: Vec<CredentialGeneration>,
    },
    PendingRemoveCredential {
        operation_id: String,
        descriptor_id: DescriptorId,
        generations: Vec<CredentialGeneration>,
    },
}

impl PendingOperation {
    fn operation_id(&self) -> &str {
        match self {
            Self::PendingCreate { operation_id, .. }
            | Self::PendingReplace { operation_id, .. }
            | Self::CleanupOld { operation_id, .. }
            | Self::PendingForget { operation_id, .. }
            | Self::PendingRemoveCredential { operation_id, .. } => operation_id,
        }
    }

    fn descriptor_id(&self) -> &DescriptorId {
        match self {
            Self::PendingCreate { profile, .. } => &profile.descriptor_id,
            Self::PendingReplace { descriptor_id, .. }
            | Self::CleanupOld { descriptor_id, .. }
            | Self::PendingForget { descriptor_id, .. }
            | Self::PendingRemoveCredential { descriptor_id, .. } => descriptor_id,
        }
    }

    fn kind(&self) -> PendingOperationKind {
        match self {
            Self::PendingCreate { .. } => PendingOperationKind::PendingCreate,
            Self::PendingReplace { .. } => PendingOperationKind::PendingReplace,
            Self::CleanupOld { .. } => PendingOperationKind::CleanupOld,
            Self::PendingForget { .. } => PendingOperationKind::PendingForget,
            Self::PendingRemoveCredential { .. } => PendingOperationKind::PendingRemoveCredential,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileDocument {
    pub version: u32,
    pub profiles: Vec<StoredProfile>,
    pub pending_operations: Vec<PendingOperation>,
}

impl Default for ProfileDocument {
    fn default() -> Self {
        Self {
            version: PROFILE_REPOSITORY_VERSION,
            profiles: Vec::new(),
            pending_operations: Vec::new(),
        }
    }
}

impl ProfileDocument {
    fn profile_credential_is_consistent(profile: &StoredProfile) -> bool {
        let has_active_generation = profile.active_credential_generation.is_some();
        match &profile.target {
            ProfileTarget::Sqlite { .. } => {
                profile.credential_state == CredentialState::NotRequired && !has_active_generation
            }
            ProfileTarget::Postgres { .. } | ProfileTarget::Mssql { .. } => {
                match profile.credential_state {
                    CredentialState::Stored | CredentialState::Unavailable => has_active_generation,
                    CredentialState::Required => !has_active_generation,
                    CredentialState::NotRequired => false,
                }
            }
        }
    }

    fn generations_match_active(
        profile: &StoredProfile,
        generations: &[CredentialGeneration],
    ) -> bool {
        let mut unique = HashSet::new();
        if generations
            .iter()
            .any(|generation| !unique.insert(generation.0.as_str()))
        {
            return false;
        }
        match profile.active_credential_generation.as_ref() {
            Some(active) => generations == [active.clone()],
            None => generations.is_empty(),
        }
    }

    fn validate(&self) -> Result<(), ProfileRepositoryError> {
        if self.version != PROFILE_REPOSITORY_VERSION {
            return Err(ProfileRepositoryError::new(
                ProfileRepositoryErrorKind::UnsupportedVersion,
            ));
        }
        let mut profiles_by_descriptor = HashMap::new();
        for profile in &self.profiles {
            if profiles_by_descriptor
                .insert(profile.descriptor_id.0.as_str(), profile)
                .is_some()
                || !Self::profile_credential_is_consistent(profile)
            {
                return Err(ProfileRepositoryError::new(
                    ProfileRepositoryErrorKind::Corrupt,
                ));
            }
        }
        let mut operation_ids = HashSet::new();
        let mut pending_descriptors = HashSet::new();
        for operation in &self.pending_operations {
            if !operation_ids.insert(operation.operation_id())
                || !pending_descriptors.insert(operation.descriptor_id().0.as_str())
            {
                return Err(ProfileRepositoryError::new(
                    ProfileRepositoryErrorKind::Corrupt,
                ));
            }
            let invalid_reference = match operation {
                PendingOperation::PendingCreate { profile, .. } => {
                    profiles_by_descriptor.contains_key(profile.descriptor_id.0.as_str())
                        || matches!(&profile.target, ProfileTarget::Sqlite { .. })
                        || profile.credential_state != CredentialState::Stored
                        || profile.active_credential_generation.is_some()
                }
                PendingOperation::PendingReplace {
                    descriptor_id,
                    replacement,
                    old_generation,
                    new_generation,
                    ..
                } => profiles_by_descriptor
                    .get(descriptor_id.0.as_str())
                    .map(|current| {
                        &replacement.descriptor_id != descriptor_id
                            || current.active_credential_generation.as_ref()
                                != old_generation.as_ref()
                            || replacement.active_credential_generation.as_ref()
                                != Some(new_generation)
                            || replacement.credential_state != CredentialState::Stored
                            || !Self::profile_credential_is_consistent(replacement)
                            || old_generation.as_ref() == Some(new_generation)
                    })
                    .unwrap_or(true),
                PendingOperation::CleanupOld {
                    descriptor_id,
                    old_generation,
                    active_generation,
                    ..
                } => profiles_by_descriptor
                    .get(descriptor_id.0.as_str())
                    .map(|current| {
                        current.active_credential_generation.as_ref() != Some(active_generation)
                            || old_generation == active_generation
                    })
                    .unwrap_or(true),
                PendingOperation::PendingForget {
                    descriptor_id,
                    generations,
                    ..
                } => profiles_by_descriptor
                    .get(descriptor_id.0.as_str())
                    .map(|profile| !Self::generations_match_active(profile, generations))
                    .unwrap_or(true),
                PendingOperation::PendingRemoveCredential {
                    descriptor_id,
                    generations,
                    ..
                } => profiles_by_descriptor
                    .get(descriptor_id.0.as_str())
                    .map(|profile| {
                        matches!(&profile.target, ProfileTarget::Sqlite { .. })
                            || !Self::generations_match_active(profile, generations)
                    })
                    .unwrap_or(true),
            };
            if invalid_reference {
                return Err(ProfileRepositoryError::new(
                    ProfileRepositoryErrorKind::Corrupt,
                ));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProfileRepositoryErrorKind {
    ReadFailed,
    PermissionDenied,
    QuotaExceeded,
    TempWriteFailed,
    SyncFailed,
    RenameFailed,
    ParentSyncFailed,
    Corrupt,
    UnsupportedVersion,
}

/// Repository errors intentionally retain only a stable, non-sensitive kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileRepositoryError {
    kind: ProfileRepositoryErrorKind,
}

impl ProfileRepositoryError {
    fn new(kind: ProfileRepositoryErrorKind) -> Self {
        Self { kind }
    }

    pub fn kind(&self) -> ProfileRepositoryErrorKind {
        self.kind
    }
}

impl std::fmt::Display for ProfileRepositoryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "database profile repository operation failed ({:?})",
            self.kind
        )
    }
}

impl std::error::Error for ProfileRepositoryError {}

pub trait DatabaseProfileRepository: Send + Sync {
    fn load(&self) -> Result<ProfileDocument, ProfileRepositoryError>;
    fn replace(&self, document: &ProfileDocument) -> Result<(), ProfileRepositoryError>;
}

pub struct FileProfileRepository {
    path: PathBuf,
}

impl FileProfileRepository {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    fn map_io(
        error: &std::io::Error,
        fallback: ProfileRepositoryErrorKind,
    ) -> ProfileRepositoryError {
        let kind = match error.kind() {
            std::io::ErrorKind::PermissionDenied => ProfileRepositoryErrorKind::PermissionDenied,
            std::io::ErrorKind::StorageFull => ProfileRepositoryErrorKind::QuotaExceeded,
            _ => fallback,
        };
        ProfileRepositoryError::new(kind)
    }

    #[cfg(unix)]
    fn sync_parent(parent: &Path) -> Result<(), ProfileRepositoryError> {
        let directory = OpenOptions::new()
            .read(true)
            .open(parent)
            .map_err(|error| Self::map_io(&error, ProfileRepositoryErrorKind::ParentSyncFailed))?;
        directory
            .sync_all()
            .map_err(|error| Self::map_io(&error, ProfileRepositoryErrorKind::ParentSyncFailed))
    }

    #[cfg(not(unix))]
    fn sync_parent(_parent: &Path) -> Result<(), ProfileRepositoryError> {
        // `NamedTempFile::persist` uses the platform replace primitive. Windows
        // does not expose a portable directory fsync through std; the file itself
        // is synced before the atomic replace.
        Ok(())
    }
}

impl DatabaseProfileRepository for FileProfileRepository {
    fn load(&self) -> Result<ProfileDocument, ProfileRepositoryError> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(ProfileDocument::default())
            }
            Err(error) => return Err(Self::map_io(&error, ProfileRepositoryErrorKind::ReadFailed)),
        };
        let document: ProfileDocument = serde_json::from_slice(&bytes)
            .map_err(|_| ProfileRepositoryError::new(ProfileRepositoryErrorKind::Corrupt))?;
        document.validate()?;
        Ok(document)
    }

    fn replace(&self, document: &ProfileDocument) -> Result<(), ProfileRepositoryError> {
        document.validate()?;
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)
            .map_err(|error| Self::map_io(&error, ProfileRepositoryErrorKind::PermissionDenied))?;
        let bytes = serde_json::to_vec(document)
            .map_err(|_| ProfileRepositoryError::new(ProfileRepositoryErrorKind::Corrupt))?;
        let mut temporary = tempfile::Builder::new()
            .prefix(".database-profiles-")
            .tempfile_in(parent)
            .map_err(|error| Self::map_io(&error, ProfileRepositoryErrorKind::TempWriteFailed))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            temporary
                .as_file()
                .set_permissions(fs::Permissions::from_mode(0o600))
                .map_err(|error| {
                    Self::map_io(&error, ProfileRepositoryErrorKind::PermissionDenied)
                })?;
        }
        temporary
            .write_all(&bytes)
            .map_err(|error| Self::map_io(&error, ProfileRepositoryErrorKind::TempWriteFailed))?;
        temporary
            .as_file()
            .sync_all()
            .map_err(|error| Self::map_io(&error, ProfileRepositoryErrorKind::SyncFailed))?;
        temporary.persist(&self.path).map_err(|error| {
            Self::map_io(&error.error, ProfileRepositoryErrorKind::RenameFailed)
        })?;
        Self::sync_parent(parent)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepositoryFailurePoint {
    Read,
    Permission,
    Quota,
    TempWrite,
    Sync,
    Rename,
    ParentSync,
}

#[derive(Default)]
struct FakeRepositoryDisk {
    durable_bytes: Option<Vec<u8>>,
    failures: VecDeque<RepositoryFailurePoint>,
    #[cfg(test)]
    replace_calls: usize,
    #[cfg(test)]
    scheduled_replace_failures: VecDeque<(usize, RepositoryFailurePoint)>,
}

/// Deterministic crash/reopen repository. Clones share only durable bytes; each
/// `reopen` behaves like a new process reading the last atomic replacement.
#[derive(Clone, Default)]
pub struct FakeProfileRepository {
    disk: Arc<Mutex<FakeRepositoryDisk>>,
}

impl FakeProfileRepository {
    pub fn reopen(&self) -> Self {
        Self {
            disk: Arc::clone(&self.disk),
        }
    }

    pub fn fail_next(&self, point: RepositoryFailurePoint) {
        self.disk
            .lock()
            .expect("fake repository poisoned")
            .failures
            .push_back(point);
    }

    #[cfg(test)]
    fn fail_nth_replace(&self, nth: usize, point: RepositoryFailurePoint) {
        assert!(nth > 0, "replace failure must target a future call");
        let mut disk = self.disk.lock().expect("fake repository poisoned");
        let target_call = disk
            .replace_calls
            .checked_add(nth)
            .expect("replace call counter overflowed");
        disk.scheduled_replace_failures
            .push_back((target_call, point));
    }

    #[cfg(test)]
    fn durable_bytes(&self) -> Vec<u8> {
        self.disk
            .lock()
            .expect("fake repository poisoned")
            .durable_bytes
            .clone()
            .unwrap_or_default()
    }

    fn take_failure(disk: &mut FakeRepositoryDisk, point: RepositoryFailurePoint) -> bool {
        if disk.failures.front() == Some(&point) {
            disk.failures.pop_front();
            true
        } else {
            false
        }
    }

    fn take_replace_failure(disk: &mut FakeRepositoryDisk, point: RepositoryFailurePoint) -> bool {
        #[cfg(test)]
        if let Some(index) =
            disk.scheduled_replace_failures
                .iter()
                .position(|(call, scheduled_point)| {
                    *call == disk.replace_calls && *scheduled_point == point
                })
        {
            disk.scheduled_replace_failures.remove(index);
            return true;
        }
        Self::take_failure(disk, point)
    }
}

impl DatabaseProfileRepository for FakeProfileRepository {
    fn load(&self) -> Result<ProfileDocument, ProfileRepositoryError> {
        let mut disk = self.disk.lock().expect("fake repository poisoned");
        if Self::take_failure(&mut disk, RepositoryFailurePoint::Read) {
            return Err(ProfileRepositoryError::new(
                ProfileRepositoryErrorKind::ReadFailed,
            ));
        }
        let Some(bytes) = disk.durable_bytes.as_ref() else {
            return Ok(ProfileDocument::default());
        };
        let document: ProfileDocument = serde_json::from_slice(bytes)
            .map_err(|_| ProfileRepositoryError::new(ProfileRepositoryErrorKind::Corrupt))?;
        document.validate()?;
        Ok(document)
    }

    fn replace(&self, document: &ProfileDocument) -> Result<(), ProfileRepositoryError> {
        document.validate()?;
        let bytes = serde_json::to_vec(document)
            .map_err(|_| ProfileRepositoryError::new(ProfileRepositoryErrorKind::Corrupt))?;
        let mut disk = self.disk.lock().expect("fake repository poisoned");
        #[cfg(test)]
        {
            disk.replace_calls += 1;
        }
        let ordered = [
            (
                RepositoryFailurePoint::Permission,
                ProfileRepositoryErrorKind::PermissionDenied,
            ),
            (
                RepositoryFailurePoint::Quota,
                ProfileRepositoryErrorKind::QuotaExceeded,
            ),
            (
                RepositoryFailurePoint::TempWrite,
                ProfileRepositoryErrorKind::TempWriteFailed,
            ),
            (
                RepositoryFailurePoint::Sync,
                ProfileRepositoryErrorKind::SyncFailed,
            ),
            (
                RepositoryFailurePoint::Rename,
                ProfileRepositoryErrorKind::RenameFailed,
            ),
        ];
        for (point, kind) in ordered {
            if Self::take_replace_failure(&mut disk, point) {
                return Err(ProfileRepositoryError::new(kind));
            }
        }
        disk.durable_bytes = Some(bytes);
        if Self::take_replace_failure(&mut disk, RepositoryFailurePoint::ParentSync) {
            return Err(ProfileRepositoryError::new(
                ProfileRepositoryErrorKind::ParentSyncFailed,
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ProfileErrorCode {
    RepositoryUnavailable,
    VaultMissing,
    VaultDenied,
    VaultUnavailable,
    VaultCorrupt,
    VaultWriteFailed,
    VaultDeleteFailed,
    ProfileNotFound,
    PendingOperationConflict,
    RecoveryNotFound,
    RecoveryActionInvalid,
    CredentialRequired,
    LifecycleCancelFailed,
    LifecycleCloseFailed,
    ConnectionFailed,
    ConnectionBusy,
    ServerDisconnected,
    MetadataFailed,
    SqlitePathMissing,
    SqlitePathNotFile,
    SqlitePathUnreadable,
    SqlitePathInvalid,
    SqliteOpenFailed,
    StaleConnection,
    InvalidRequest,
}

/// IPC-safe domain error. Underlying filesystem, keyring and driver messages are
/// deliberately discarded so neither paths nor credential material can escape.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileError {
    pub code: ProfileErrorCode,
    pub message: &'static str,
}

impl ProfileError {
    fn new(code: ProfileErrorCode, message: &'static str) -> Self {
        Self { code, message }
    }
}

impl From<ProfileRepositoryError> for ProfileError {
    fn from(_: ProfileRepositoryError) -> Self {
        Self::new(
            ProfileErrorCode::RepositoryUnavailable,
            "database profile storage is unavailable",
        )
    }
}

impl From<VaultError> for ProfileError {
    fn from(error: VaultError) -> Self {
        let code = match error.kind() {
            VaultErrorKind::Missing => ProfileErrorCode::VaultMissing,
            VaultErrorKind::Denied => ProfileErrorCode::VaultDenied,
            VaultErrorKind::Unavailable => ProfileErrorCode::VaultUnavailable,
            VaultErrorKind::Corrupt => ProfileErrorCode::VaultCorrupt,
            VaultErrorKind::WriteFailed => ProfileErrorCode::VaultWriteFailed,
            VaultErrorKind::DeleteFailed => ProfileErrorCode::VaultDeleteFailed,
        };
        Self::new(code, "database credential vault operation failed")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RecoveryAction {
    Resume,
    Abort,
    RetryCleanup,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileRecoveryRequest {
    pub operation_id: String,
    pub action: RecoveryAction,
    pub credential: Option<CredentialInput>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileRecoveryRow {
    pub operation_id: String,
    pub descriptor_id: DescriptorId,
    pub kind: PendingOperationKind,
    pub allowed_actions: Vec<RecoveryAction>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileLoadResult {
    pub profiles: Vec<ProfileDescriptor>,
    pub recovery: Vec<ProfileRecoveryRow>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyProfileImportRequest {
    pub profiles: Vec<ProfileDescriptor>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LifecycleCloseEvidence {
    NoLiveHandle,
    CancelledAndClosed,
    HandleClosedAndSettled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LifecycleCloseErrorKind {
    CancelFailed,
    CloseFailed,
}

pub trait DatabaseLifecycleCloser: Send + Sync {
    fn cancel_and_close(
        &self,
        descriptor_id: &DescriptorId,
    ) -> Result<LifecycleCloseEvidence, LifecycleCloseErrorKind>;
}

/// Production-safe seam until the P3 profile actor owns a live handle. It is
/// explicit evidence that there is no profile-owned handle, not an assertion
/// that cancellation happened.
#[derive(Default)]
pub struct NoLiveProfileCloser;

impl DatabaseLifecycleCloser for NoLiveProfileCloser {
    fn cancel_and_close(
        &self,
        _descriptor_id: &DescriptorId,
    ) -> Result<LifecycleCloseEvidence, LifecycleCloseErrorKind> {
        Ok(LifecycleCloseEvidence::NoLiveHandle)
    }
}

#[derive(Clone)]
struct OpenCompletion {
    result: Arc<Mutex<Option<Result<LiveConnection, ProfileError>>>>,
    notify: Arc<tokio::sync::Notify>,
    #[cfg(test)]
    waiters: Arc<std::sync::atomic::AtomicUsize>,
}

impl OpenCompletion {
    fn new() -> Self {
        Self {
            result: Arc::new(Mutex::new(None)),
            notify: Arc::new(tokio::sync::Notify::new()),
            #[cfg(test)]
            waiters: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }

    fn finish(&self, result: Result<LiveConnection, ProfileError>) {
        if let Ok(mut slot) = self.result.lock() {
            if slot.is_none() {
                *slot = Some(result);
                self.notify.notify_waiters();
            }
        }
    }

    async fn wait(&self) -> Result<LiveConnection, ProfileError> {
        #[cfg(test)]
        self.waiters
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        loop {
            let notified = self.notify.notified();
            if let Some(result) = self.result.lock().ok().and_then(|slot| slot.clone()) {
                return result;
            }
            notified.await;
        }
    }
}

#[derive(Clone)]
struct OpeningReservation {
    descriptor_id: DescriptorId,
    ticket: u64,
    config_generation: u64,
    completion: OpenCompletion,
}

enum OpenDecision {
    Live(LiveConnection),
    Wait(OpenCompletion),
    Open(OpeningReservation),
    Unavailable(ProfileError),
}

enum ProfileRuntimeEntry {
    Opening(OpeningReservation),
    Live(LiveConnection),
    Closing(LiveConnection),
}

struct ProfileRuntimeState {
    entries: HashMap<String, ProfileRuntimeEntry>,
    terminated: HashMap<String, db_service::ConnectionIdentity>,
    next_ticket: u64,
}

impl Default for ProfileRuntimeState {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            terminated: HashMap::new(),
            next_ticket: 1,
        }
    }
}

impl ProfileRuntimeState {
    #[cfg(test)]
    fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[derive(Clone, Default)]
struct ProfileRuntimeRegistry {
    connections: Arc<Mutex<ProfileRuntimeState>>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProfileRuntimeShutdownReport {
    pub opening: usize,
    pub live: usize,
    pub closing: usize,
    pub tombstones: usize,
    pub reset: bool,
    pub error: Option<&'static str>,
}

impl ProfileRuntimeRegistry {
    fn shutdown_reset(&self) -> ProfileRuntimeShutdownReport {
        let previous = match self.connections.lock() {
            Ok(mut state) => std::mem::take(&mut *state),
            Err(_) => {
                return ProfileRuntimeShutdownReport {
                    error: Some("database profile runtime reset failed"),
                    ..ProfileRuntimeShutdownReport::default()
                };
            }
        };
        let mut report = ProfileRuntimeShutdownReport {
            tombstones: previous.terminated.len(),
            reset: true,
            ..ProfileRuntimeShutdownReport::default()
        };
        for entry in previous.entries.into_values() {
            match entry {
                ProfileRuntimeEntry::Opening(reservation) => {
                    report.opening += 1;
                    reservation.completion.finish(Err(ProfileError::new(
                        ProfileErrorCode::StaleConnection,
                        "database runtime shut down before the connection opened",
                    )));
                }
                ProfileRuntimeEntry::Live(_) => report.live += 1,
                ProfileRuntimeEntry::Closing(_) => report.closing += 1,
            }
        }
        report
    }

    fn get(&self, descriptor_id: &DescriptorId) -> Option<LiveConnection> {
        let state = self.connections.lock().ok()?;
        match state.entries.get(&descriptor_id.0) {
            Some(ProfileRuntimeEntry::Live(connection)) => Some(connection.clone()),
            _ => None,
        }
    }

    fn begin_open(
        &self,
        descriptor_id: &DescriptorId,
        config_generation: u64,
    ) -> Result<OpenDecision, LifecycleCloseErrorKind> {
        let mut state = self
            .connections
            .lock()
            .map_err(|_| LifecycleCloseErrorKind::CloseFailed)?;
        if let Some(entry) = state.entries.get(&descriptor_id.0) {
            return Ok(match entry {
                ProfileRuntimeEntry::Live(connection) => OpenDecision::Live(connection.clone()),
                ProfileRuntimeEntry::Closing(_) => OpenDecision::Unavailable(ProfileError::new(
                    ProfileErrorCode::ConnectionBusy,
                    "database connection is closing",
                )),
                ProfileRuntimeEntry::Opening(reservation)
                    if reservation.config_generation == config_generation =>
                {
                    OpenDecision::Wait(reservation.completion.clone())
                }
                ProfileRuntimeEntry::Opening(reservation) => {
                    reservation.completion.finish(Err(ProfileError::new(
                        ProfileErrorCode::StaleConnection,
                        "database connection open was invalidated",
                    )));
                    state.entries.remove(&descriptor_id.0);
                    return Self::reserve_locked(&mut state, descriptor_id, config_generation);
                }
            });
        }
        Self::reserve_locked(&mut state, descriptor_id, config_generation)
    }

    fn reserve_locked(
        state: &mut ProfileRuntimeState,
        descriptor_id: &DescriptorId,
        config_generation: u64,
    ) -> Result<OpenDecision, LifecycleCloseErrorKind> {
        // Once a descriptor starts opening a new generation, an older
        // termination finalizer must no longer be accepted as idempotent.
        state.terminated.remove(&descriptor_id.0);
        let ticket = state.next_ticket;
        state.next_ticket = state
            .next_ticket
            .checked_add(1)
            .ok_or(LifecycleCloseErrorKind::CloseFailed)?;
        let reservation = OpeningReservation {
            descriptor_id: descriptor_id.clone(),
            ticket,
            config_generation,
            completion: OpenCompletion::new(),
        };
        state.entries.insert(
            descriptor_id.0.clone(),
            ProfileRuntimeEntry::Opening(reservation.clone()),
        );
        Ok(OpenDecision::Open(reservation))
    }

    fn publish_open(
        &self,
        reservation: &OpeningReservation,
        connection: LiveConnection,
    ) -> Result<bool, LifecycleCloseErrorKind> {
        let mut state = self
            .connections
            .lock()
            .map_err(|_| LifecycleCloseErrorKind::CloseFailed)?;
        let exact = matches!(
            state.entries.get(&reservation.descriptor_id.0),
            Some(ProfileRuntimeEntry::Opening(current))
                if current.ticket == reservation.ticket
                    && current.config_generation == reservation.config_generation
        );
        if exact {
            state.entries.insert(
                reservation.descriptor_id.0.clone(),
                ProfileRuntimeEntry::Live(connection.clone()),
            );
            reservation.completion.finish(Ok(connection));
        }
        Ok(exact)
    }

    fn fail_open(
        &self,
        reservation: &OpeningReservation,
        error: ProfileError,
    ) -> Result<(), LifecycleCloseErrorKind> {
        let mut state = self
            .connections
            .lock()
            .map_err(|_| LifecycleCloseErrorKind::CloseFailed)?;
        let exact = matches!(
            state.entries.get(&reservation.descriptor_id.0),
            Some(ProfileRuntimeEntry::Opening(current)) if current.ticket == reservation.ticket
        );
        if exact {
            state.entries.remove(&reservation.descriptor_id.0);
        }
        reservation.completion.finish(Err(error));
        Ok(())
    }

    #[cfg(test)]
    fn invalidate_open(
        &self,
        descriptor_id: &DescriptorId,
    ) -> Result<Option<LiveConnection>, LifecycleCloseErrorKind> {
        let mut state = self
            .connections
            .lock()
            .map_err(|_| LifecycleCloseErrorKind::CloseFailed)?;
        match state.entries.remove(&descriptor_id.0) {
            Some(ProfileRuntimeEntry::Opening(reservation)) => {
                reservation.completion.finish(Err(ProfileError::new(
                    ProfileErrorCode::StaleConnection,
                    "database connection open was invalidated",
                )));
                Ok(None)
            }
            Some(ProfileRuntimeEntry::Live(connection)) => Ok(Some(connection)),
            Some(ProfileRuntimeEntry::Closing(connection)) => Ok(Some(connection)),
            None => Ok(None),
        }
    }

    fn begin_close(
        &self,
        descriptor_id: &DescriptorId,
    ) -> Result<Option<LiveConnection>, LifecycleCloseErrorKind> {
        let mut state = self
            .connections
            .lock()
            .map_err(|_| LifecycleCloseErrorKind::CloseFailed)?;
        match state.entries.remove(&descriptor_id.0) {
            Some(ProfileRuntimeEntry::Opening(reservation)) => {
                reservation.completion.finish(Err(ProfileError::new(
                    ProfileErrorCode::StaleConnection,
                    "database connection open was invalidated",
                )));
                Ok(None)
            }
            Some(ProfileRuntimeEntry::Live(connection)) => {
                state.entries.insert(
                    descriptor_id.0.clone(),
                    ProfileRuntimeEntry::Closing(connection.clone()),
                );
                Ok(Some(connection))
            }
            Some(ProfileRuntimeEntry::Closing(connection)) => {
                state.entries.insert(
                    descriptor_id.0.clone(),
                    ProfileRuntimeEntry::Closing(connection),
                );
                Err(LifecycleCloseErrorKind::CloseFailed)
            }
            None => Ok(None),
        }
    }

    fn finish_close(
        &self,
        identity: &LiveConnection,
        closed: bool,
    ) -> Result<(), LifecycleCloseErrorKind> {
        let mut state = self
            .connections
            .lock()
            .map_err(|_| LifecycleCloseErrorKind::CloseFailed)?;
        let exact = matches!(
            state.entries.get(&identity.descriptor_id.0),
            Some(ProfileRuntimeEntry::Closing(current)) if current == identity
        );
        if !exact {
            return Err(LifecycleCloseErrorKind::CloseFailed);
        }
        if closed {
            state.entries.remove(&identity.descriptor_id.0);
        } else {
            state.entries.insert(
                identity.descriptor_id.0.clone(),
                ProfileRuntimeEntry::Live(identity.clone()),
            );
        }
        Ok(())
    }

    fn discard_exact(&self, identity: &LiveConnection) -> Result<bool, LifecycleCloseErrorKind> {
        let mut state = self
            .connections
            .lock()
            .map_err(|_| LifecycleCloseErrorKind::CloseFailed)?;
        let exact = matches!(
            state.entries.get(&identity.descriptor_id.0),
            Some(ProfileRuntimeEntry::Live(current) | ProfileRuntimeEntry::Closing(current))
                if current == identity
        );
        if exact {
            state.entries.remove(&identity.descriptor_id.0);
        }
        Ok(exact)
    }

    #[cfg(test)]
    fn insert(&self, connection: LiveConnection) -> Result<(), LifecycleCloseErrorKind> {
        self.connections
            .lock()
            .map_err(|_| LifecycleCloseErrorKind::CloseFailed)?
            .entries
            .insert(
                connection.descriptor_id.0.clone(),
                ProfileRuntimeEntry::Live(connection),
            );
        Ok(())
    }

    #[cfg(test)]
    fn opening_waiter_count(&self, descriptor_id: &DescriptorId) -> usize {
        self.connections
            .lock()
            .ok()
            .and_then(|state| match state.entries.get(&descriptor_id.0) {
                Some(ProfileRuntimeEntry::Opening(reservation)) => Some(
                    reservation
                        .completion
                        .waiters
                        .load(std::sync::atomic::Ordering::SeqCst),
                ),
                _ => None,
            })
            .unwrap_or(0)
    }
}

/// Ensures cancellation/abort of the elected opener cannot strand an Opening
/// entry or leave joined callers waiting forever. Exact-ticket checks in
/// `fail_open` make this safe even if edit/remove already invalidated it.
struct OpenReservationGuard {
    runtime: ProfileRuntimeRegistry,
    reservation: Option<OpeningReservation>,
}

impl OpenReservationGuard {
    fn new(runtime: ProfileRuntimeRegistry, reservation: OpeningReservation) -> Self {
        Self {
            runtime,
            reservation: Some(reservation),
        }
    }

    fn reservation(&self) -> &OpeningReservation {
        self.reservation
            .as_ref()
            .expect("opening reservation guard is still armed")
    }

    fn fail(&mut self, error: ProfileError) {
        if let Some(reservation) = self.reservation.take() {
            let _ = self.runtime.fail_open(&reservation, error);
        }
    }

    fn defuse(&mut self) {
        self.reservation = None;
    }
}

impl Drop for OpenReservationGuard {
    fn drop(&mut self) {
        if let Some(reservation) = self.reservation.take() {
            let _ = self.runtime.fail_open(
                &reservation,
                ProfileError::new(
                    ProfileErrorCode::StaleConnection,
                    "database connection opener ended before completion",
                ),
            );
        }
    }
}

/// Actor-backed lifecycle closer used by the P2 vault sagas. It marks the
/// descriptor Closing, then accepts cleanup only after exact actor teardown (or
/// proof that the old exact actor is already gone).
struct RegisteredProfileCloser {
    database_state: DbState,
    runtime: ProfileRuntimeRegistry,
    result_sessions: ResultSessionState,
}

impl RegisteredProfileCloser {
    fn release_exact_sessions(
        &self,
        identity: &db_service::ConnectionIdentity,
    ) -> Result<(), LifecycleCloseErrorKind> {
        self.result_sessions
            .lock()
            .map_err(|_| LifecycleCloseErrorKind::CloseFailed)?
            .release_connection(identity)
            .map_err(|_| LifecycleCloseErrorKind::CloseFailed)
    }
}

impl DatabaseLifecycleCloser for RegisteredProfileCloser {
    fn cancel_and_close(
        &self,
        descriptor_id: &DescriptorId,
    ) -> Result<LifecycleCloseEvidence, LifecycleCloseErrorKind> {
        let Some(connection) = self
            .runtime
            .begin_close(descriptor_id)
            .map_err(|_| LifecycleCloseErrorKind::CloseFailed)?
        else {
            return Ok(LifecycleCloseEvidence::NoLiveHandle);
        };
        let identity = db_service::ConnectionIdentity {
            descriptor_id: connection.descriptor_id.clone(),
            connection_id: connection.connection_id.clone(),
            connection_generation: connection.connection_generation.clone(),
        };
        match db_service::close_exact_in_state(&self.database_state, &identity) {
            Ok(report)
                if !report.unreleased_execution
                    && !report.metadata_in_flight
                    && report.unreleased_result_sessions == 0 =>
            {
                self.release_exact_sessions(&identity)?;
                self.runtime.finish_close(&connection, true)?;
                Ok(LifecycleCloseEvidence::HandleClosedAndSettled)
            }
            Err(error)
                if matches!(
                    error.code,
                    db_service::DatabaseOperationalErrorCode::StaleConnection
                        | db_service::DatabaseOperationalErrorCode::ServerDisconnected
                ) =>
            {
                // The exact old actor is already gone. Removing only this
                // descriptor runtime identity cannot affect a newer generation.
                self.release_exact_sessions(&identity)?;
                self.runtime.finish_close(&connection, true)?;
                Ok(LifecycleCloseEvidence::HandleClosedAndSettled)
            }
            Ok(_) | Err(_) => {
                // The descriptor remains manageable while exact actor teardown
                // is incomplete. A later explicit retry re-enters this path.
                self.runtime.finish_close(&connection, false)?;
                Err(LifecycleCloseErrorKind::CloseFailed)
            }
        }
    }
}

#[derive(Default)]
struct FakeCloserState {
    failures: VecDeque<LifecycleCloseErrorKind>,
    calls: HashMap<String, usize>,
}

#[derive(Clone, Default)]
pub struct FakeLifecycleCloser {
    state: Arc<Mutex<FakeCloserState>>,
}

impl FakeLifecycleCloser {
    pub fn fail_next(&self, failure: LifecycleCloseErrorKind) {
        self.state
            .lock()
            .expect("fake closer poisoned")
            .failures
            .push_back(failure);
    }

    pub fn call_count(&self, descriptor_id: &str) -> usize {
        *self
            .state
            .lock()
            .expect("fake closer poisoned")
            .calls
            .get(descriptor_id)
            .unwrap_or(&0)
    }
}

impl DatabaseLifecycleCloser for FakeLifecycleCloser {
    fn cancel_and_close(
        &self,
        descriptor_id: &DescriptorId,
    ) -> Result<LifecycleCloseEvidence, LifecycleCloseErrorKind> {
        let mut state = self.state.lock().expect("fake closer poisoned");
        *state.calls.entry(descriptor_id.0.clone()).or_default() += 1;
        if let Some(failure) = state.failures.pop_front() {
            return Err(failure);
        }
        Ok(LifecycleCloseEvidence::CancelledAndClosed)
    }
}

pub struct DatabaseProfiles {
    repository: Arc<dyn DatabaseProfileRepository>,
    vault: Arc<dyn DatabaseCredentialStore>,
    closer: Arc<dyn DatabaseLifecycleCloser>,
}

impl DatabaseProfiles {
    pub fn new(
        repository: Arc<dyn DatabaseProfileRepository>,
        vault: Arc<dyn DatabaseCredentialStore>,
        closer: Arc<dyn DatabaseLifecycleCloser>,
    ) -> Self {
        Self {
            repository,
            vault,
            closer,
        }
    }

    fn new_id(prefix: &str) -> String {
        format!("{prefix}-{}", uuid::Uuid::new_v4())
    }

    fn is_network(target: &ProfileTarget) -> bool {
        !matches!(target, ProfileTarget::Sqlite { .. })
    }

    fn recovery_row(operation: &PendingOperation) -> ProfileRecoveryRow {
        let allowed_actions = match operation {
            PendingOperation::PendingCreate { .. } | PendingOperation::PendingReplace { .. } => {
                vec![RecoveryAction::Resume, RecoveryAction::Abort]
            }
            PendingOperation::CleanupOld { .. }
            | PendingOperation::PendingForget { .. }
            | PendingOperation::PendingRemoveCredential { .. } => {
                vec![RecoveryAction::RetryCleanup]
            }
        };
        ProfileRecoveryRow {
            operation_id: operation.operation_id().to_string(),
            descriptor_id: operation.descriptor_id().clone(),
            kind: operation.kind(),
            allowed_actions,
        }
    }

    fn view(document: &ProfileDocument) -> ProfileLoadResult {
        ProfileLoadResult {
            profiles: document
                .profiles
                .iter()
                .map(StoredProfile::descriptor)
                .collect(),
            recovery: document
                .pending_operations
                .iter()
                .map(Self::recovery_row)
                .collect(),
        }
    }

    /// Startup-safe load. This method has no vault call and never auto-connects.
    pub fn load(&self) -> Result<ProfileLoadResult, ProfileError> {
        let document = self.repository.load()?;
        Ok(Self::view(&document))
    }

    pub fn import_legacy(
        &self,
        request: LegacyProfileImportRequest,
    ) -> Result<ProfileLoadResult, ProfileError> {
        let mut document = self.repository.load()?;
        for profile in request.profiles {
            if document
                .profiles
                .iter()
                .any(|existing| existing.descriptor_id == profile.descriptor_id)
            {
                continue;
            }
            let credential_state = if Self::is_network(&profile.target) {
                // v1 localStorage never had a vault generation. Never infer that
                // a credential exists merely from legacy display state.
                CredentialState::Required
            } else {
                CredentialState::NotRequired
            };
            document.profiles.push(StoredProfile {
                descriptor_id: profile.descriptor_id,
                config_generation: profile.config_generation,
                name: profile.name,
                target: profile.target,
                credential_state,
                active_credential_generation: None,
            });
        }
        self.repository.replace(&document)?;
        Ok(Self::view(&document))
    }

    fn ensure_no_pending(
        document: &ProfileDocument,
        descriptor_id: &DescriptorId,
    ) -> Result<(), ProfileError> {
        if document
            .pending_operations
            .iter()
            .any(|operation| operation.descriptor_id() == descriptor_id)
        {
            return Err(ProfileError::new(
                ProfileErrorCode::PendingOperationConflict,
                "finish the pending profile recovery first",
            ));
        }
        Ok(())
    }

    pub fn create(&self, request: ProfileCreateRequest) -> Result<ProfileDescriptor, ProfileError> {
        let mut document = self.repository.load()?;
        let descriptor_id = DescriptorId(Self::new_id("dbc"));
        let mut stored = StoredProfile {
            descriptor_id: descriptor_id.clone(),
            config_generation: 1,
            name: request.name,
            target: request.target,
            credential_state: CredentialState::NotRequired,
            active_credential_generation: None,
        };
        if !Self::is_network(&stored.target) {
            if request.credential.is_some() {
                return Err(ProfileError::new(
                    ProfileErrorCode::InvalidRequest,
                    "SQLite profiles do not accept credentials",
                ));
            }
            document.profiles.push(stored.clone());
            self.repository.replace(&document)?;
            return Ok(stored.descriptor());
        }

        let Some(credential) = request.credential else {
            stored.credential_state = CredentialState::Required;
            document.profiles.push(stored.clone());
            self.repository.replace(&document)?;
            return Ok(stored.descriptor());
        };
        let generation = CredentialGeneration(Self::new_id("credential"));
        let operation_id = Self::new_id("profile-op");
        stored.credential_state = CredentialState::Stored;
        let pending = PendingOperation::PendingCreate {
            operation_id,
            profile: stored.clone(),
            credential_generation: generation.clone(),
        };
        // Write-ahead record is durable before the first vault mutation.
        document.pending_operations.push(pending);
        self.repository.replace(&document)?;
        self.vault
            .store(&descriptor_id, &generation, credential.password.into())?;

        stored.active_credential_generation = Some(generation);
        document.profiles.push(stored.clone());
        document
            .pending_operations
            .retain(|operation| operation.descriptor_id() != &descriptor_id);
        self.repository.replace(&document)?;
        Ok(stored.descriptor())
    }

    pub fn update(&self, request: ProfileUpdateRequest) -> Result<ProfileDescriptor, ProfileError> {
        let mut document = self.repository.load()?;
        Self::ensure_no_pending(&document, &request.descriptor_id)?;
        let index = document
            .profiles
            .iter()
            .position(|profile| profile.descriptor_id == request.descriptor_id)
            .ok_or_else(|| {
                ProfileError::new(
                    ProfileErrorCode::ProfileNotFound,
                    "database profile was not found",
                )
            })?;
        let current = document.profiles[index].clone();
        if request.replacement_credential.is_none() {
            if Self::is_network(&current.target) != Self::is_network(&request.target) {
                return Err(ProfileError::new(
                    ProfileErrorCode::InvalidRequest,
                    "changing credential mode requires a new profile",
                ));
            }
            if current.target != request.target {
                Self::map_close(self.closer.cancel_and_close(&request.descriptor_id))?;
            }
            let mut updated = current;
            updated.name = request.name;
            updated.target = request.target;
            if updated.target != document.profiles[index].target {
                updated.config_generation =
                    updated.config_generation.checked_add(1).ok_or_else(|| {
                        ProfileError::new(
                            ProfileErrorCode::InvalidRequest,
                            "database profile generation is exhausted",
                        )
                    })?;
            }
            document.profiles[index] = updated.clone();
            self.repository.replace(&document)?;
            return Ok(updated.descriptor());
        }
        if !Self::is_network(&request.target) {
            return Err(ProfileError::new(
                ProfileErrorCode::InvalidRequest,
                "SQLite profiles do not accept credentials",
            ));
        }
        Self::map_close(self.closer.cancel_and_close(&request.descriptor_id))?;

        let replacement_credential = request.replacement_credential.expect("checked above");
        let new_generation = CredentialGeneration(Self::new_id("credential"));
        let old_generation = current.active_credential_generation.clone();
        let mut replacement = current.clone();
        replacement.name = request.name;
        replacement.target = request.target;
        replacement.config_generation =
            replacement
                .config_generation
                .checked_add(1)
                .ok_or_else(|| {
                    ProfileError::new(
                        ProfileErrorCode::InvalidRequest,
                        "database profile generation is exhausted",
                    )
                })?;
        replacement.credential_state = CredentialState::Stored;
        replacement.active_credential_generation = Some(new_generation.clone());
        let operation_id = Self::new_id("profile-op");
        document
            .pending_operations
            .push(PendingOperation::PendingReplace {
                operation_id: operation_id.clone(),
                descriptor_id: request.descriptor_id.clone(),
                replacement: replacement.clone(),
                old_generation: old_generation.clone(),
                new_generation: new_generation.clone(),
            });
        self.repository.replace(&document)?;
        self.vault.store(
            &request.descriptor_id,
            &new_generation,
            replacement_credential.password.into(),
        )?;

        document.profiles[index] = replacement.clone();
        document
            .pending_operations
            .retain(|operation| operation.operation_id() != operation_id);
        if let Some(old_generation) = old_generation {
            // Descriptor switch and cleanup transition share one atomic replace.
            document
                .pending_operations
                .push(PendingOperation::CleanupOld {
                    operation_id: operation_id.clone(),
                    descriptor_id: request.descriptor_id.clone(),
                    old_generation: old_generation.clone(),
                    active_generation: new_generation,
                });
            self.repository.replace(&document)?;
            self.vault.delete(&request.descriptor_id, &old_generation)?;
            document
                .pending_operations
                .retain(|operation| operation.operation_id() != operation_id);
        }
        self.repository.replace(&document)?;
        Ok(replacement.descriptor())
    }

    fn map_close(
        result: Result<LifecycleCloseEvidence, LifecycleCloseErrorKind>,
    ) -> Result<LifecycleCloseEvidence, ProfileError> {
        result.map_err(|kind| match kind {
            LifecycleCloseErrorKind::CancelFailed => ProfileError::new(
                ProfileErrorCode::LifecycleCancelFailed,
                "database activity could not be cancelled",
            ),
            LifecycleCloseErrorKind::CloseFailed => ProfileError::new(
                ProfileErrorCode::LifecycleCloseFailed,
                "database connection could not be closed",
            ),
        })
    }

    fn known_generations(profile: &StoredProfile) -> Vec<CredentialGeneration> {
        profile
            .active_credential_generation
            .iter()
            .cloned()
            .collect()
    }

    fn persist_cleanup_after_close_failure(
        &self,
        document: &mut ProfileDocument,
        operation: PendingOperation,
        close_error: ProfileError,
    ) -> Result<ProfileError, ProfileError> {
        document.pending_operations.push(operation);
        self.repository.replace(document)?;
        Ok(close_error)
    }

    pub fn forget(&self, descriptor_id: &DescriptorId) -> Result<ProfileLoadResult, ProfileError> {
        let mut document = self.repository.load()?;
        Self::ensure_no_pending(&document, descriptor_id)?;
        let profile = document
            .profiles
            .iter()
            .find(|profile| &profile.descriptor_id == descriptor_id)
            .cloned()
            .ok_or_else(|| {
                ProfileError::new(
                    ProfileErrorCode::ProfileNotFound,
                    "database profile was not found",
                )
            })?;
        let operation = PendingOperation::PendingForget {
            operation_id: Self::new_id("profile-op"),
            descriptor_id: descriptor_id.clone(),
            generations: Self::known_generations(&profile),
        };
        if let Err(error) = Self::map_close(self.closer.cancel_and_close(descriptor_id)) {
            return Err(self.persist_cleanup_after_close_failure(
                &mut document,
                operation,
                error,
            )?);
        }
        document.pending_operations.push(operation);
        self.repository.replace(&document)?;
        self.finish_forget(&mut document, descriptor_id)?;
        Ok(Self::view(&document))
    }

    fn finish_forget(
        &self,
        document: &mut ProfileDocument,
        descriptor_id: &DescriptorId,
    ) -> Result<(), ProfileError> {
        let generations = document
            .pending_operations
            .iter()
            .find_map(|operation| match operation {
                PendingOperation::PendingForget {
                    descriptor_id: pending_id,
                    generations,
                    ..
                } if pending_id == descriptor_id => Some(generations.clone()),
                _ => None,
            })
            .ok_or_else(|| {
                ProfileError::new(
                    ProfileErrorCode::RecoveryNotFound,
                    "profile recovery was not found",
                )
            })?;
        for generation in generations {
            self.vault.delete(descriptor_id, &generation)?;
        }
        document
            .profiles
            .retain(|profile| &profile.descriptor_id != descriptor_id);
        document
            .pending_operations
            .retain(|operation| operation.descriptor_id() != descriptor_id);
        self.repository.replace(document)?;
        Ok(())
    }

    pub fn remove_credential(
        &self,
        descriptor_id: &DescriptorId,
    ) -> Result<ProfileLoadResult, ProfileError> {
        let mut document = self.repository.load()?;
        Self::ensure_no_pending(&document, descriptor_id)?;
        let profile = document
            .profiles
            .iter()
            .find(|profile| &profile.descriptor_id == descriptor_id)
            .cloned()
            .ok_or_else(|| {
                ProfileError::new(
                    ProfileErrorCode::ProfileNotFound,
                    "database profile was not found",
                )
            })?;
        if !Self::is_network(&profile.target) {
            return Err(ProfileError::new(
                ProfileErrorCode::InvalidRequest,
                "SQLite profiles do not have credentials",
            ));
        }
        let operation = PendingOperation::PendingRemoveCredential {
            operation_id: Self::new_id("profile-op"),
            descriptor_id: descriptor_id.clone(),
            generations: Self::known_generations(&profile),
        };
        if let Err(error) = Self::map_close(self.closer.cancel_and_close(descriptor_id)) {
            return Err(self.persist_cleanup_after_close_failure(
                &mut document,
                operation,
                error,
            )?);
        }
        document.pending_operations.push(operation);
        self.repository.replace(&document)?;
        self.finish_remove_credential(&mut document, descriptor_id)?;
        Ok(Self::view(&document))
    }

    fn finish_remove_credential(
        &self,
        document: &mut ProfileDocument,
        descriptor_id: &DescriptorId,
    ) -> Result<(), ProfileError> {
        let generations = document
            .pending_operations
            .iter()
            .find_map(|operation| match operation {
                PendingOperation::PendingRemoveCredential {
                    descriptor_id: pending_id,
                    generations,
                    ..
                } if pending_id == descriptor_id => Some(generations.clone()),
                _ => None,
            })
            .ok_or_else(|| {
                ProfileError::new(
                    ProfileErrorCode::RecoveryNotFound,
                    "profile recovery was not found",
                )
            })?;
        for generation in generations {
            self.vault.delete(descriptor_id, &generation)?;
        }
        let profile = document
            .profiles
            .iter_mut()
            .find(|profile| &profile.descriptor_id == descriptor_id)
            .ok_or_else(|| {
                ProfileError::new(
                    ProfileErrorCode::ProfileNotFound,
                    "database profile was not found",
                )
            })?;
        profile.active_credential_generation = None;
        profile.credential_state = CredentialState::Required;
        document
            .pending_operations
            .retain(|operation| operation.descriptor_id() != descriptor_id);
        self.repository.replace(document)?;
        Ok(())
    }

    pub fn recover(
        &self,
        request: ProfileRecoveryRequest,
    ) -> Result<ProfileLoadResult, ProfileError> {
        let mut document = self.repository.load()?;
        let operation = document
            .pending_operations
            .iter()
            .find(|operation| operation.operation_id() == request.operation_id)
            .cloned()
            .ok_or_else(|| {
                ProfileError::new(
                    ProfileErrorCode::RecoveryNotFound,
                    "profile recovery was not found",
                )
            })?;
        match (operation, request.action) {
            (
                PendingOperation::PendingCreate {
                    operation_id,
                    mut profile,
                    credential_generation,
                },
                RecoveryAction::Resume,
            ) => {
                self.ensure_generation(
                    &profile.descriptor_id,
                    &credential_generation,
                    request.credential,
                )?;
                profile.active_credential_generation = Some(credential_generation);
                profile.credential_state = CredentialState::Stored;
                document
                    .profiles
                    .retain(|item| item.descriptor_id != profile.descriptor_id);
                document.profiles.push(profile);
                document
                    .pending_operations
                    .retain(|item| item.operation_id() != operation_id);
                self.repository.replace(&document)?;
            }
            (
                PendingOperation::PendingReplace {
                    operation_id,
                    descriptor_id,
                    replacement,
                    old_generation,
                    new_generation,
                },
                RecoveryAction::Resume,
            ) => {
                self.ensure_generation(&descriptor_id, &new_generation, request.credential)?;
                let index = document
                    .profiles
                    .iter()
                    .position(|profile| profile.descriptor_id == descriptor_id)
                    .ok_or_else(|| {
                        ProfileError::new(
                            ProfileErrorCode::ProfileNotFound,
                            "database profile was not found",
                        )
                    })?;
                document.profiles[index] = replacement;
                document
                    .pending_operations
                    .retain(|item| item.operation_id() != operation_id);
                if let Some(old_generation) = old_generation {
                    document
                        .pending_operations
                        .push(PendingOperation::CleanupOld {
                            operation_id: operation_id.clone(),
                            descriptor_id: descriptor_id.clone(),
                            old_generation: old_generation.clone(),
                            active_generation: new_generation,
                        });
                    self.repository.replace(&document)?;
                    self.vault.delete(&descriptor_id, &old_generation)?;
                    document
                        .pending_operations
                        .retain(|item| item.operation_id() != operation_id);
                }
                self.repository.replace(&document)?;
            }
            (
                PendingOperation::PendingCreate {
                    operation_id,
                    profile,
                    credential_generation,
                },
                RecoveryAction::Abort,
            ) => {
                self.vault
                    .delete(&profile.descriptor_id, &credential_generation)?;
                document
                    .pending_operations
                    .retain(|item| item.operation_id() != operation_id);
                self.repository.replace(&document)?;
            }
            (
                PendingOperation::PendingReplace {
                    operation_id,
                    descriptor_id,
                    new_generation,
                    ..
                },
                RecoveryAction::Abort,
            ) => {
                self.vault.delete(&descriptor_id, &new_generation)?;
                document
                    .pending_operations
                    .retain(|item| item.operation_id() != operation_id);
                self.repository.replace(&document)?;
            }
            (
                PendingOperation::CleanupOld {
                    operation_id,
                    descriptor_id,
                    old_generation,
                    ..
                },
                RecoveryAction::RetryCleanup,
            ) => {
                self.vault.delete(&descriptor_id, &old_generation)?;
                document
                    .pending_operations
                    .retain(|item| item.operation_id() != operation_id);
                self.repository.replace(&document)?;
            }
            (
                PendingOperation::PendingForget { descriptor_id, .. },
                RecoveryAction::RetryCleanup,
            ) => {
                Self::map_close(self.closer.cancel_and_close(&descriptor_id))?;
                self.finish_forget(&mut document, &descriptor_id)?;
            }
            (
                PendingOperation::PendingRemoveCredential { descriptor_id, .. },
                RecoveryAction::RetryCleanup,
            ) => {
                Self::map_close(self.closer.cancel_and_close(&descriptor_id))?;
                self.finish_remove_credential(&mut document, &descriptor_id)?;
            }
            _ => {
                return Err(ProfileError::new(
                    ProfileErrorCode::RecoveryActionInvalid,
                    "recovery action is not valid for this operation",
                ))
            }
        }
        Ok(Self::view(&document))
    }

    fn ensure_generation(
        &self,
        descriptor_id: &DescriptorId,
        generation: &CredentialGeneration,
        credential: Option<CredentialInput>,
    ) -> Result<(), ProfileError> {
        match self.vault.resolve(descriptor_id, generation) {
            Ok(secret) => {
                drop(secret);
                Ok(())
            }
            Err(error)
                if matches!(
                    error.kind(),
                    VaultErrorKind::Missing | VaultErrorKind::Corrupt | VaultErrorKind::Denied
                ) =>
            {
                let credential = credential.ok_or_else(|| {
                    ProfileError::new(
                        ProfileErrorCode::CredentialRequired,
                        "credential input is required to resume this recovery",
                    )
                })?;
                if error.kind() == VaultErrorKind::Corrupt {
                    // The write-ahead operation already owns this generation, so
                    // explicit user recovery may safely clear and recreate it.
                    self.vault.delete(descriptor_id, generation)?;
                }
                self.vault
                    .store(descriptor_id, generation, credential.password.into())?;
                Ok(())
            }
            Err(error) => Err(error.into()),
        }
    }

    pub(crate) fn resolve_saved_credential(
        &self,
        descriptor_id: &DescriptorId,
    ) -> Result<(ProfileDescriptor, Option<secrecy::SecretString>), ProfileError> {
        let document = self.repository.load()?;
        Self::ensure_no_pending(&document, descriptor_id)?;
        let profile = document
            .profiles
            .iter()
            .find(|profile| &profile.descriptor_id == descriptor_id)
            .ok_or_else(|| {
                ProfileError::new(
                    ProfileErrorCode::ProfileNotFound,
                    "database profile was not found",
                )
            })?;
        let secret = match profile.active_credential_generation.as_ref() {
            Some(generation) => Some(self.vault.resolve(descriptor_id, generation)?),
            None if Self::is_network(&profile.target) => {
                return Err(ProfileError::new(
                    ProfileErrorCode::CredentialRequired,
                    "database profile requires a credential",
                ))
            }
            None => None,
        };
        Ok((profile.descriptor(), secret))
    }

    fn openable_descriptor(
        &self,
        descriptor_id: &DescriptorId,
    ) -> Result<ProfileDescriptor, ProfileError> {
        let document = self.repository.load()?;
        Self::ensure_no_pending(&document, descriptor_id)?;
        document
            .profiles
            .iter()
            .find(|profile| &profile.descriptor_id == descriptor_id)
            .map(StoredProfile::descriptor)
            .ok_or_else(|| {
                ProfileError::new(
                    ProfileErrorCode::ProfileNotFound,
                    "database profile was not found",
                )
            })
    }
}

type DatabaseOpenFuture<'a> = Pin<
    Box<dyn Future<Output = Result<DbHandle, db_service::DatabaseOperationalError>> + Send + 'a>,
>;

trait DatabaseConnectionOpener: Send + Sync {
    fn open(&self, config: DbOpenConfig) -> DatabaseOpenFuture<'_>;
}

#[derive(Default)]
struct ProductionDatabaseConnectionOpener;

impl DatabaseConnectionOpener for ProductionDatabaseConnectionOpener {
    fn open(&self, config: DbOpenConfig) -> DatabaseOpenFuture<'_> {
        Box::pin(db_service::open_unregistered(config))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResultSessionShutdownReport {
    pub sessions_before: usize,
    pub bytes_before: usize,
    pub sessions_after: usize,
    pub bytes_after: usize,
    pub reset: bool,
    pub error: Option<&'static str>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DatabaseRuntimeShutdownReport {
    pub database: db_service::DatabaseShutdownReport,
    pub profiles: ProfileRuntimeShutdownReport,
    pub result_sessions: ResultSessionShutdownReport,
}

impl DatabaseRuntimeShutdownReport {
    pub fn has_failures(&self) -> bool {
        self.database.has_failures()
            || self.profiles.error.is_some()
            || !self.profiles.reset
            || self.result_sessions.error.is_some()
            || !self.result_sessions.reset
            || self.result_sessions.sessions_after != 0
            || self.result_sessions.bytes_after != 0
    }
}

#[derive(Clone)]
pub struct DatabaseProfileState {
    profiles: Arc<Mutex<DatabaseProfiles>>,
    runtime: ProfileRuntimeRegistry,
    database_state: DbState,
    result_sessions: ResultSessionState,
    opener: Arc<dyn DatabaseConnectionOpener>,
}

impl DatabaseProfileState {
    pub fn production(
        repository_path: PathBuf,
        database_state: DbState,
        result_sessions: ResultSessionState,
    ) -> Self {
        let runtime = ProfileRuntimeRegistry::default();
        let closer: Arc<dyn DatabaseLifecycleCloser> = Arc::new(RegisteredProfileCloser {
            database_state: database_state.clone(),
            runtime: runtime.clone(),
            result_sessions: result_sessions.clone(),
        });
        let profiles = DatabaseProfiles::new(
            Arc::new(FileProfileRepository::new(repository_path)),
            Arc::new(crate::db_credentials::KeyringCredentialStore::default()),
            closer.clone(),
        );
        Self {
            profiles: Arc::new(Mutex::new(profiles)),
            runtime,
            database_state,
            result_sessions,
            opener: Arc::new(ProductionDatabaseConnectionOpener),
        }
    }

    #[cfg(test)]
    fn deterministic_test(
        profiles: DatabaseProfiles,
        runtime: ProfileRuntimeRegistry,
        database_state: DbState,
        result_sessions: ResultSessionState,
    ) -> Self {
        Self {
            profiles: Arc::new(Mutex::new(profiles)),
            runtime,
            database_state,
            result_sessions,
            opener: Arc::new(ProductionDatabaseConnectionOpener),
        }
    }

    pub async fn shutdown_database_runtime(
        &self,
        timeouts: db_service::DatabaseShutdownTimeouts,
    ) -> DatabaseRuntimeShutdownReport {
        let database = db_service::shutdown_all_connections(&self.database_state, timeouts).await;
        let profiles = self.runtime.shutdown_reset();
        let result_sessions = match self.result_sessions.lock() {
            Ok(mut sessions) => {
                let sessions_before = sessions.session_count();
                let bytes_before = sessions.total_bytes();
                *sessions = Default::default();
                ResultSessionShutdownReport {
                    sessions_before,
                    bytes_before,
                    sessions_after: sessions.session_count(),
                    bytes_after: sessions.total_bytes(),
                    reset: true,
                    error: None,
                }
            }
            Err(_) => ResultSessionShutdownReport {
                error: Some("database result session reset failed"),
                ..ResultSessionShutdownReport::default()
            },
        };
        DatabaseRuntimeShutdownReport {
            database,
            profiles,
            result_sessions,
        }
    }

    pub(crate) fn mark_exact_connection_offline(
        &self,
        identity: &db_service::ConnectionIdentity,
    ) -> Result<(), ProfileError> {
        let mut runtime = self.runtime.connections.lock().map_err(|_| {
            ProfileError::new(
                ProfileErrorCode::ConnectionFailed,
                "database connection registry is unavailable",
            )
        })?;
        let exact_live = matches!(
            runtime.entries.get(&identity.descriptor_id.0),
            Some(ProfileRuntimeEntry::Live(current) | ProfileRuntimeEntry::Closing(current))
                if current.descriptor_id == identity.descriptor_id
                    && current.connection_id == identity.connection_id
                    && current.connection_generation == identity.connection_generation
        );
        let exact_tombstone = runtime
            .terminated
            .get(&identity.descriptor_id.0)
            .is_some_and(|terminated| terminated == identity);
        if !exact_live && !exact_tombstone {
            return Err(ProfileError::new(
                ProfileErrorCode::StaleConnection,
                "database connection is no longer active",
            ));
        }
        if exact_live {
            runtime.entries.remove(&identity.descriptor_id.0);
            runtime
                .terminated
                .insert(identity.descriptor_id.0.clone(), identity.clone());
        }
        Ok(())
    }

    fn connection_error(error: db_service::DatabaseOperationalError) -> ProfileError {
        use db_service::DatabaseOperationalErrorCode as Code;

        let code = match error.code {
            Code::ConnectionBusy => ProfileErrorCode::ConnectionBusy,
            Code::ServerDisconnected => ProfileErrorCode::ServerDisconnected,
            Code::MetadataFailed => ProfileErrorCode::MetadataFailed,
            Code::SqlitePathMissing => ProfileErrorCode::SqlitePathMissing,
            Code::SqlitePathNotFile => ProfileErrorCode::SqlitePathNotFile,
            Code::SqlitePathUnreadable => ProfileErrorCode::SqlitePathUnreadable,
            Code::SqlitePathInvalid => ProfileErrorCode::SqlitePathInvalid,
            Code::SqliteOpenFailed => ProfileErrorCode::SqliteOpenFailed,
            Code::StaleConnection => ProfileErrorCode::StaleConnection,
            Code::ConnectionFailed | Code::QueryFailed => ProfileErrorCode::ConnectionFailed,
        };
        ProfileError::new(code, error.message)
    }

    async fn list_profiles(&self) -> Result<ProfileLoadResult, ProfileError> {
        self.with_profiles(DatabaseProfiles::load).await
    }

    async fn update_profile(
        &self,
        request: ProfileUpdateRequest,
    ) -> Result<ProfileDescriptor, ProfileError> {
        self.with_profiles(move |profiles| profiles.update(request))
            .await
    }

    async fn with_profiles<T, F>(&self, operation: F) -> Result<T, ProfileError>
    where
        T: Send + 'static,
        F: FnOnce(&DatabaseProfiles) -> Result<T, ProfileError> + Send + 'static,
    {
        let profiles = Arc::clone(&self.profiles);
        tauri::async_runtime::spawn_blocking(move || {
            let profiles = profiles.lock().map_err(|_| {
                ProfileError::new(
                    ProfileErrorCode::RepositoryUnavailable,
                    "database profile storage is unavailable",
                )
            })?;
            operation(&profiles)
        })
        .await
        .map_err(|_| {
            ProfileError::new(
                ProfileErrorCode::RepositoryUnavailable,
                "database profile storage is unavailable",
            )
        })?
    }

    fn required_secret(
        secret: Option<secrecy::SecretString>,
    ) -> Result<secrecy::SecretString, ProfileError> {
        secret.ok_or_else(|| {
            ProfileError::new(
                ProfileErrorCode::CredentialRequired,
                "database profile requires a credential",
            )
        })
    }

    fn open_config(
        target: ProfileTarget,
        secret: Option<secrecy::SecretString>,
    ) -> Result<DbOpenConfig, ProfileError> {
        match target {
            ProfileTarget::Sqlite { path } => Ok(DbOpenConfig::Sqlite { path }),
            ProfileTarget::Postgres {
                host,
                port,
                database,
                user,
                ssl,
                trust_cert,
            } => {
                let password = Self::required_secret(secret)?;
                Ok(DbOpenConfig::Postgres {
                    host,
                    port,
                    database,
                    user,
                    password,
                    ssl,
                    trust_cert,
                })
            }
            ProfileTarget::Mssql {
                host,
                port,
                database,
                user,
                trust_cert,
            } => {
                let password = Self::required_secret(secret)?;
                Ok(DbOpenConfig::Mssql {
                    host,
                    port,
                    database,
                    user,
                    password,
                    trust_cert,
                })
            }
        }
    }

    fn engine(target: &ProfileTarget) -> LiveDatabaseEngine {
        match target {
            ProfileTarget::Sqlite { .. } => LiveDatabaseEngine::Sqlite,
            ProfileTarget::Postgres { .. } => LiveDatabaseEngine::Postgres,
            ProfileTarget::Mssql { .. } => LiveDatabaseEngine::Mssql,
        }
    }

    async fn open_saved(
        &self,
        descriptor_id: &DescriptorId,
    ) -> Result<LiveConnection, ProfileError> {
        let descriptor_id_for_check = descriptor_id.clone();
        let checked = self
            .with_profiles(move |profiles| profiles.openable_descriptor(&descriptor_id_for_check))
            .await?;
        let mut decision = self
            .runtime
            .begin_open(descriptor_id, checked.config_generation)
            .map_err(|_| {
                ProfileError::new(
                    ProfileErrorCode::ConnectionFailed,
                    "database connection registry is unavailable",
                )
            })?;
        if let OpenDecision::Live(existing) = &decision {
            let identity = db_service::ConnectionIdentity {
                descriptor_id: existing.descriptor_id.clone(),
                connection_id: existing.connection_id.clone(),
                connection_generation: existing.connection_generation.clone(),
            };
            if !db_service::has_exact_actor(&self.database_state, &identity) {
                self.result_sessions
                    .lock()
                    .map_err(|_| {
                        ProfileError::new(
                            ProfileErrorCode::ConnectionFailed,
                            "result session registry is unavailable",
                        )
                    })?
                    .release_connection(&identity)
                    .map_err(|_| {
                        ProfileError::new(
                            ProfileErrorCode::ConnectionFailed,
                            "result session cleanup failed",
                        )
                    })?;
                self.runtime.discard_exact(existing).map_err(|_| {
                    ProfileError::new(
                        ProfileErrorCode::ConnectionFailed,
                        "database connection registry is unavailable",
                    )
                })?;
                decision = self
                    .runtime
                    .begin_open(descriptor_id, checked.config_generation)
                    .map_err(|_| {
                        ProfileError::new(
                            ProfileErrorCode::ConnectionFailed,
                            "database connection registry is unavailable",
                        )
                    })?;
            } else if db_service::exact_actor_is_terminating(&self.database_state, &identity) {
                decision = OpenDecision::Unavailable(ProfileError::new(
                    ProfileErrorCode::ConnectionBusy,
                    "database connection termination is waiting for execution settlement",
                ));
            }
        }
        let reservation = match decision {
            OpenDecision::Live(existing) => return Ok(existing),
            OpenDecision::Wait(completion) => return completion.wait().await,
            OpenDecision::Open(reservation) => reservation,
            OpenDecision::Unavailable(error) => return Err(error),
        };
        let mut reservation_guard = OpenReservationGuard::new(self.runtime.clone(), reservation);

        let descriptor_id_for_resolve = descriptor_id.clone();
        let resolved = self
            .with_profiles(move |profiles| {
                profiles.resolve_saved_credential(&descriptor_id_for_resolve)
            })
            .await;
        let (profile, secret) = match resolved {
            Ok(resolved) => resolved,
            Err(error) => {
                reservation_guard.fail(error.clone());
                return Err(error);
            }
        };
        if profile.config_generation != reservation_guard.reservation().config_generation {
            let error = ProfileError::new(
                ProfileErrorCode::StaleConnection,
                "database connection open was invalidated",
            );
            reservation_guard.fail(error.clone());
            return Err(error);
        }
        let engine = Self::engine(&profile.target);
        let config = match Self::open_config(profile.target, secret) {
            Ok(config) => config,
            Err(error) => {
                reservation_guard.fail(error.clone());
                return Err(error);
            }
        };
        let handle = match self.opener.open(config).await {
            Ok(handle) => handle,
            Err(open_error) => {
                let error = Self::connection_error(open_error);
                reservation_guard.fail(error.clone());
                return Err(error);
            }
        };
        let connection_id = ConnectionId(db_service::next_conn_id());
        let connection = LiveConnection {
            descriptor_id: descriptor_id.clone(),
            connection_id: connection_id.clone(),
            connection_generation: ConnectionGeneration(DatabaseProfiles::new_id("connection")),
            engine,
        };
        let identity = db_service::ConnectionIdentity {
            descriptor_id: descriptor_id.clone(),
            connection_id,
            connection_generation: connection.connection_generation.clone(),
        };
        let actor = Arc::new(crate::db_connection_actor::ProductionConnectionActor::new(
            identity.clone(),
            handle,
        ));
        if let Err(register_error) = db_service::register_actor(&self.database_state, actor) {
            let error = Self::connection_error(register_error);
            reservation_guard.fail(error.clone());
            return Err(error);
        }
        match self
            .runtime
            .publish_open(reservation_guard.reservation(), connection.clone())
        {
            Ok(true) => {
                reservation_guard.defuse();
                Ok(connection)
            }
            Ok(false) => {
                let _ = db_service::close_exact_in_state(&self.database_state, &identity);
                reservation_guard.defuse();
                Err(ProfileError::new(
                    ProfileErrorCode::StaleConnection,
                    "database connection open was invalidated",
                ))
            }
            Err(_) => {
                let _ = db_service::close_exact_in_state(&self.database_state, &identity);
                let error = ProfileError::new(
                    ProfileErrorCode::ConnectionFailed,
                    "database connection registry is unavailable",
                );
                reservation_guard.fail(error.clone());
                Err(error)
            }
        }
    }

    async fn test_connection(
        &self,
        request: TestConnectionRequest,
    ) -> Result<TestConnectionResult, ProfileError> {
        let (target, secret) = match request {
            TestConnectionRequest::Ephemeral { target, credential } => {
                (target, credential.map(|credential| credential.password))
            }
            TestConnectionRequest::Saved { descriptor_id } => {
                let (profile, secret) = self
                    .with_profiles(move |profiles| {
                        profiles.resolve_saved_credential(&descriptor_id)
                    })
                    .await?;
                (profile.target, secret)
            }
        };
        let config = Self::open_config(target, secret)?;
        let started = Instant::now();
        let server_version = db_service::test_unregistered(config)
            .await
            .map_err(Self::connection_error)?;
        Ok(TestConnectionResult {
            elapsed_ms: started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
            server_version,
        })
    }
}

#[derive(Serialize)]
#[serde(
    tag = "outcome",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SaveAndConnectOutcome {
    Connected {
        profile: ProfileDescriptor,
        connection: LiveConnection,
    },
    SavedButConnectFailed {
        profile: ProfileDescriptor,
        error: ProfileError,
    },
}

#[tauri::command]
pub async fn db_profile_list(
    state: tauri::State<'_, DatabaseProfileState>,
) -> Result<ProfileLoadResult, ProfileError> {
    state.list_profiles().await
}

#[tauri::command]
pub async fn db_profile_import_legacy(
    state: tauri::State<'_, DatabaseProfileState>,
    request: LegacyProfileImportRequest,
) -> Result<ProfileLoadResult, ProfileError> {
    state
        .with_profiles(move |profiles| profiles.import_legacy(request))
        .await
}

#[tauri::command]
pub async fn db_profile_create(
    state: tauri::State<'_, DatabaseProfileState>,
    request: ProfileCreateRequest,
) -> Result<SaveAndConnectOutcome, ProfileError> {
    let profile = state
        .with_profiles(move |profiles| profiles.create(request))
        .await?;
    match state.open_saved(&profile.descriptor_id).await {
        Ok(connection) => Ok(SaveAndConnectOutcome::Connected {
            profile,
            connection,
        }),
        Err(error) => Ok(SaveAndConnectOutcome::SavedButConnectFailed { profile, error }),
    }
}

#[tauri::command]
pub async fn db_profile_update(
    state: tauri::State<'_, DatabaseProfileState>,
    request: ProfileUpdateRequest,
) -> Result<ProfileDescriptor, ProfileError> {
    state.update_profile(request).await
}

#[tauri::command]
pub async fn db_profile_remove_credential(
    state: tauri::State<'_, DatabaseProfileState>,
    descriptor_id: DescriptorId,
) -> Result<ProfileLoadResult, ProfileError> {
    state
        .with_profiles(move |profiles| profiles.remove_credential(&descriptor_id))
        .await
}

#[tauri::command]
pub async fn db_profile_forget(
    state: tauri::State<'_, DatabaseProfileState>,
    descriptor_id: DescriptorId,
) -> Result<ProfileLoadResult, ProfileError> {
    state
        .with_profiles(move |profiles| profiles.forget(&descriptor_id))
        .await
}

#[tauri::command]
pub async fn db_profile_recover(
    state: tauri::State<'_, DatabaseProfileState>,
    request: ProfileRecoveryRequest,
) -> Result<ProfileLoadResult, ProfileError> {
    state
        .with_profiles(move |profiles| profiles.recover(request))
        .await
}

#[tauri::command]
pub async fn db_profile_open(
    state: tauri::State<'_, DatabaseProfileState>,
    descriptor_id: DescriptorId,
) -> Result<LiveConnection, ProfileError> {
    state.open_saved(&descriptor_id).await
}

#[tauri::command]
pub async fn db_profile_disconnect(
    state: tauri::State<'_, DatabaseProfileState>,
    identity: db_service::ConnectionIdentity,
) -> Result<(), ProfileError> {
    let current = state.runtime.get(&identity.descriptor_id).ok_or_else(|| {
        ProfileError::new(
            ProfileErrorCode::StaleConnection,
            "database connection is no longer active",
        )
    })?;
    if current.connection_id != identity.connection_id
        || current.connection_generation != identity.connection_generation
    {
        return Err(ProfileError::new(
            ProfileErrorCode::StaleConnection,
            "database connection is no longer active",
        ));
    }
    state
        .runtime
        .begin_close(&identity.descriptor_id)
        .map_err(|_| {
            ProfileError::new(
                ProfileErrorCode::ConnectionBusy,
                "database connection is closing",
            )
        })?;
    match db_service::close_exact_in_state(&state.database_state, &identity) {
        Ok(_) => {
            state
                .result_sessions
                .lock()
                .map_err(|_| {
                    ProfileError::new(
                        ProfileErrorCode::ConnectionFailed,
                        "result session registry is unavailable",
                    )
                })?
                .release_connection(&identity)
                .map_err(|_| {
                    ProfileError::new(
                        ProfileErrorCode::ConnectionFailed,
                        "result session cleanup failed",
                    )
                })?;
            state.runtime.finish_close(&current, true).map_err(|_| {
                ProfileError::new(
                    ProfileErrorCode::ConnectionFailed,
                    "database connection registry is unavailable",
                )
            })?;
            Ok(())
        }
        Err(error)
            if matches!(
                error.code,
                db_service::DatabaseOperationalErrorCode::StaleConnection
                    | db_service::DatabaseOperationalErrorCode::ServerDisconnected
            ) =>
        {
            state
                .result_sessions
                .lock()
                .map_err(|_| {
                    ProfileError::new(
                        ProfileErrorCode::ConnectionFailed,
                        "result session registry is unavailable",
                    )
                })?
                .release_connection(&identity)
                .map_err(|_| {
                    ProfileError::new(
                        ProfileErrorCode::ConnectionFailed,
                        "result session cleanup failed",
                    )
                })?;
            state.runtime.finish_close(&current, true).map_err(|_| {
                ProfileError::new(
                    ProfileErrorCode::ConnectionFailed,
                    "database connection registry is unavailable",
                )
            })?;
            Ok(())
        }
        Err(error) => {
            let _ = state.runtime.finish_close(&current, false);
            Err(DatabaseProfileState::connection_error(error))
        }
    }
}

#[tauri::command]
pub async fn db_test_connection(
    state: tauri::State<'_, DatabaseProfileState>,
    request: TestConnectionRequest,
) -> Result<TestConnectionResult, ProfileError> {
    state.test_connection(request).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::{ExposeSecret, SecretString};
    use std::sync::atomic::{AtomicUsize, Ordering};

    const SENTINEL: &str = "YUZORA_PROFILE_SECRET_SENTINEL";

    fn runtime_connection(descriptor_id: &str, suffix: &str) -> LiveConnection {
        LiveConnection {
            descriptor_id: DescriptorId(descriptor_id.to_string()),
            connection_id: ConnectionId(format!("connection-{suffix}")),
            connection_generation: ConnectionGeneration(format!("generation-{suffix}")),
            engine: LiveDatabaseEngine::Sqlite,
        }
    }

    fn termination_finalizer_fixture(
        suffix: &str,
    ) -> (
        DbState,
        ResultSessionState,
        DatabaseProfileState,
        db_service::ConnectionIdentity,
    ) {
        let database_state = DbState::default();
        let result_sessions = ResultSessionState::default();
        let runtime = ProfileRuntimeRegistry::default();
        let connection = runtime_connection(&format!("descriptor-{suffix}"), suffix);
        runtime.insert(connection.clone()).unwrap();
        let identity = db_service::ConnectionIdentity {
            descriptor_id: connection.descriptor_id.clone(),
            connection_id: connection.connection_id.clone(),
            connection_generation: connection.connection_generation.clone(),
        };
        db_service::register_actor(
            &database_state,
            Arc::new(crate::db_connection_actor::ProductionConnectionActor::new(
                identity.clone(),
                DbHandle::Sqlite(Mutex::new(rusqlite::Connection::open_in_memory().unwrap())),
            )),
        )
        .unwrap();
        let run_owner = db_service::QueryRunOwner {
            descriptor_id: identity.descriptor_id.clone(),
            connection_id: identity.connection_id.clone(),
            connection_generation: identity.connection_generation.clone(),
            query_run_id: db_service::QueryRunId(format!("run-{suffix}")),
        };
        let session_owner = db_service::ResultSessionOwner {
            descriptor_id: identity.descriptor_id.clone(),
            connection_id: identity.connection_id.clone(),
            connection_generation: identity.connection_generation.clone(),
            query_run_id: run_owner.query_run_id.clone(),
            statement_execution_id: db_service::StatementExecutionId(format!("statement-{suffix}")),
            result_session_id: db_service::ResultSessionId(format!("session-{suffix}")),
        };
        {
            let mut sessions = result_sessions.lock().unwrap();
            sessions.begin_run(&run_owner).unwrap();
            sessions
                .begin_session(session_owner, vec!["value".to_string()])
                .unwrap();
        }
        let profile_state = DatabaseProfileState {
            profiles: Arc::new(Mutex::new(DatabaseProfiles::new(
                Arc::new(FakeProfileRepository::default()),
                Arc::new(TestVault::default()),
                Arc::new(NoLiveProfileCloser),
            ))),
            runtime,
            database_state: database_state.clone(),
            result_sessions: result_sessions.clone(),
            opener: Arc::new(ProductionDatabaseConnectionOpener),
        };
        (database_state, result_sessions, profile_state, identity)
    }

    fn finalize_as_cancel(
        database_state: &DbState,
        result_sessions: &ResultSessionState,
        profile_state: &DatabaseProfileState,
        identity: &db_service::ConnectionIdentity,
    ) -> Result<db_service::QueryCancelResult, db_service::DatabaseOperationalError> {
        db_service::finalize_terminated_connection(
            database_state,
            result_sessions,
            profile_state,
            identity,
        )?;
        Ok(db_service::QueryCancelResult {
            outcome: db_service::QueryCancelOutcome::CancelledConnectionTerminated,
        })
    }

    #[test]
    fn termination_finalizer_is_order_independent_when_run_finalizes_first() {
        let (database_state, result_sessions, profile_state, identity) =
            termination_finalizer_fixture("run-first");

        db_service::finalize_terminated_connection(
            &database_state,
            &result_sessions,
            &profile_state,
            &identity,
        )
        .unwrap();
        let cancel =
            finalize_as_cancel(&database_state, &result_sessions, &profile_state, &identity)
                .unwrap();

        assert_eq!(
            cancel.outcome,
            db_service::QueryCancelOutcome::CancelledConnectionTerminated
        );
        assert!(!db_service::has_exact_actor(&database_state, &identity));
        assert!(profile_state.runtime.get(&identity.descriptor_id).is_none());
        assert_eq!(result_sessions.lock().unwrap().session_count(), 0);
    }

    #[test]
    fn termination_finalizer_is_order_independent_when_cancel_finalizes_first() {
        let (database_state, result_sessions, profile_state, identity) =
            termination_finalizer_fixture("cancel-first");

        let cancel =
            finalize_as_cancel(&database_state, &result_sessions, &profile_state, &identity)
                .unwrap();
        db_service::finalize_terminated_connection(
            &database_state,
            &result_sessions,
            &profile_state,
            &identity,
        )
        .unwrap();

        assert_eq!(
            cancel.outcome,
            db_service::QueryCancelOutcome::CancelledConnectionTerminated
        );
        assert!(!db_service::has_exact_actor(&database_state, &identity));
        assert!(profile_state.runtime.get(&identity.descriptor_id).is_none());
        assert_eq!(result_sessions.lock().unwrap().session_count(), 0);
    }

    #[tokio::test]
    async fn app_shutdown_settles_stream_and_resets_actor_profile_and_session_registries() {
        let database_state = DbState::default();
        let result_sessions = ResultSessionState::default();
        let runtime = ProfileRuntimeRegistry::default();
        let live = runtime_connection("descriptor-shutdown-live", "shutdown-live");
        let closing = runtime_connection("descriptor-shutdown-closing", "shutdown-closing");
        runtime.insert(live.clone()).unwrap();
        runtime.insert(closing.clone()).unwrap();
        assert_eq!(
            runtime.begin_close(&closing.descriptor_id).unwrap(),
            Some(closing.clone())
        );
        let opening_completion = match runtime
            .begin_open(&DescriptorId("descriptor-shutdown-opening".to_string()), 1)
            .unwrap()
        {
            OpenDecision::Open(reservation) => reservation.completion,
            _ => panic!("expected deterministic opening reservation"),
        };
        runtime.connections.lock().unwrap().terminated.insert(
            "descriptor-shutdown-tombstone".to_string(),
            db_service::ConnectionIdentity {
                descriptor_id: DescriptorId("descriptor-shutdown-tombstone".to_string()),
                connection_id: ConnectionId("connection-shutdown-tombstone".to_string()),
                connection_generation: ConnectionGeneration(
                    "generation-shutdown-tombstone".to_string(),
                ),
            },
        );

        let live_identity = db_service::ConnectionIdentity {
            descriptor_id: live.descriptor_id.clone(),
            connection_id: live.connection_id.clone(),
            connection_generation: live.connection_generation.clone(),
        };
        let live_actor = Arc::new(crate::db_connection_actor::ProductionConnectionActor::new(
            live_identity.clone(),
            DbHandle::Sqlite(Mutex::new(rusqlite::Connection::open_in_memory().unwrap())),
        ));
        db_service::register_actor(&database_state, Arc::clone(&live_actor)).unwrap();
        let closing_identity = db_service::ConnectionIdentity {
            descriptor_id: closing.descriptor_id.clone(),
            connection_id: closing.connection_id.clone(),
            connection_generation: closing.connection_generation.clone(),
        };
        db_service::register_actor(
            &database_state,
            Arc::new(crate::db_connection_actor::ProductionConnectionActor::new(
                closing_identity,
                DbHandle::Sqlite(Mutex::new(rusqlite::Connection::open_in_memory().unwrap())),
            )),
        )
        .unwrap();

        let run_owner = db_service::QueryRunOwner {
            descriptor_id: live_identity.descriptor_id.clone(),
            connection_id: live_identity.connection_id.clone(),
            connection_generation: live_identity.connection_generation.clone(),
            query_run_id: db_service::QueryRunId("run-shutdown-live".to_string()),
        };
        let lease = live_actor
            .acquire_execution(
                run_owner.clone(),
                crate::db_connection_actor::CancelCapability::SqliteInterrupt,
            )
            .unwrap();
        let session_owner = db_service::ResultSessionOwner {
            descriptor_id: run_owner.descriptor_id.clone(),
            connection_id: run_owner.connection_id.clone(),
            connection_generation: run_owner.connection_generation.clone(),
            query_run_id: run_owner.query_run_id.clone(),
            statement_execution_id: db_service::StatementExecutionId(
                "statement-shutdown-live".to_string(),
            ),
            result_session_id: db_service::ResultSessionId("session-shutdown-live".to_string()),
        };
        {
            let mut sessions = result_sessions.lock().unwrap();
            sessions.begin_run(&run_owner).unwrap();
            sessions
                .begin_session(session_owner.clone(), vec!["value".to_string()])
                .unwrap();
            sessions
                .push_row(
                    &session_owner,
                    vec![db_service::DbValue::Text {
                        value: "cached".to_string(),
                    }],
                )
                .unwrap();
        }
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        live_actor
            .install_result_continuation(&lease, session_owner.clone(), sender)
            .unwrap();
        let worker_actor = Arc::clone(&live_actor);
        let worker_lease = lease.clone();
        let worker = tokio::spawn(async move {
            assert!(receiver.recv().await.is_none());
            worker_actor.settle_execution(&worker_lease).unwrap();
        });

        let profiles = DatabaseProfiles::new(
            Arc::new(FakeProfileRepository::default()),
            Arc::new(TestVault::default()),
            Arc::new(NoLiveProfileCloser),
        );
        let state = DatabaseProfileState::deterministic_test(
            profiles,
            runtime.clone(),
            database_state.clone(),
            result_sessions.clone(),
        );
        let report = state
            .shutdown_database_runtime(db_service::DatabaseShutdownTimeouts {
                per_actor: std::time::Duration::from_secs(1),
                overall: std::time::Duration::from_secs(1),
            })
            .await;
        worker.await.unwrap();

        assert!(!report.has_failures(), "{report:?}");
        assert_eq!(report.database.snapshot_count, 2);
        assert_eq!(report.database.registry_remaining, Some(0));
        assert_eq!(report.profiles.opening, 1);
        assert_eq!(report.profiles.live, 1);
        assert_eq!(report.profiles.closing, 1);
        assert_eq!(report.profiles.tombstones, 1);
        assert!(report.profiles.reset);
        assert_eq!(report.result_sessions.sessions_before, 1);
        assert!(report.result_sessions.bytes_before > 0);
        assert_eq!(report.result_sessions.sessions_after, 0);
        assert_eq!(report.result_sessions.bytes_after, 0);
        assert!(database_state.0.lock().unwrap().is_empty());
        let runtime_state = runtime.connections.lock().unwrap();
        assert!(runtime_state.entries.is_empty());
        assert!(runtime_state.terminated.is_empty());
        drop(runtime_state);
        let opening_error = opening_completion.wait().await.unwrap_err();
        assert_eq!(opening_error.code, ProfileErrorCode::StaleConnection);
        let mut sessions = result_sessions.lock().unwrap();
        assert_eq!(sessions.session_count(), 0);
        assert_eq!(sessions.total_bytes(), 0);
        assert_eq!(
            sessions.begin_session(session_owner, vec!["value".to_string()]),
            Err(crate::db_result_session::SessionError::OwnerMismatch),
            "shutdown must clear the old active run as well as its cache"
        );
        drop(sessions);

        let repeated = state
            .shutdown_database_runtime(db_service::DatabaseShutdownTimeouts {
                per_actor: std::time::Duration::from_millis(20),
                overall: std::time::Duration::from_millis(20),
            })
            .await;
        assert!(repeated.database.already_started);
        assert_eq!(repeated.database.snapshot_count, 0);
        assert_eq!(
            repeated.profiles,
            ProfileRuntimeShutdownReport {
                reset: true,
                ..ProfileRuntimeShutdownReport::default()
            }
        );
        assert!(!repeated.has_failures(), "{repeated:?}");
    }

    struct DeferredProductionPathOpener {
        started: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
        release: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
        calls: AtomicUsize,
    }

    impl DeferredProductionPathOpener {
        fn new() -> (
            Arc<Self>,
            tokio::sync::oneshot::Receiver<()>,
            tokio::sync::oneshot::Sender<()>,
        ) {
            let (started_tx, started_rx) = tokio::sync::oneshot::channel();
            let (release_tx, release_rx) = tokio::sync::oneshot::channel();
            (
                Arc::new(Self {
                    started: Mutex::new(Some(started_tx)),
                    release: Mutex::new(Some(release_rx)),
                    calls: AtomicUsize::new(0),
                }),
                started_rx,
                release_tx,
            )
        }
    }

    impl DatabaseConnectionOpener for DeferredProductionPathOpener {
        fn open(&self, config: DbOpenConfig) -> DatabaseOpenFuture<'_> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let started = self.started.lock().unwrap().take();
            let release = self.release.lock().unwrap().take();
            Box::pin(async move {
                if let Some(started) = started {
                    let _ = started.send(());
                }
                if let Some(release) = release {
                    let _ = release.await;
                }
                db_service::open_unregistered(config).await
            })
        }
    }

    fn deferred_sqlite_state(
        path: &Path,
        opener: Arc<dyn DatabaseConnectionOpener>,
    ) -> (Arc<DatabaseProfileState>, ProfileDescriptor) {
        let repository = Arc::new(FakeProfileRepository::default());
        let vault = Arc::new(TestVault::default());
        let database_state = DbState::default();
        let runtime = ProfileRuntimeRegistry::default();
        let closer: Arc<dyn DatabaseLifecycleCloser> = Arc::new(RegisteredProfileCloser {
            database_state: database_state.clone(),
            runtime: runtime.clone(),
            result_sessions: ResultSessionState::default(),
        });
        let profiles = DatabaseProfiles::new(repository, vault, closer.clone());
        let profile = profiles
            .create(ProfileCreateRequest {
                name: "Deferred SQLite".to_string(),
                target: ProfileTarget::Sqlite {
                    path: path.to_string_lossy().into_owned(),
                },
                credential: None,
            })
            .unwrap();
        (
            Arc::new(DatabaseProfileState {
                profiles: Arc::new(Mutex::new(profiles)),
                runtime,
                database_state,
                result_sessions: ResultSessionState::default(),
                opener,
            }),
            profile,
        )
    }

    #[tokio::test]
    async fn concurrent_descriptor_reservations_elect_exactly_one_engine_opener() {
        const CALLERS: usize = 12;
        let registry = ProfileRuntimeRegistry::default();
        let barrier = Arc::new(tokio::sync::Barrier::new(CALLERS));
        let opener_count = Arc::new(AtomicUsize::new(0));
        let mut tasks = Vec::new();
        for _ in 0..CALLERS {
            let registry = registry.clone();
            let barrier = Arc::clone(&barrier);
            let opener_count = Arc::clone(&opener_count);
            tasks.push(tokio::spawn(async move {
                barrier.wait().await;
                let decision = registry
                    .begin_open(&DescriptorId("descriptor-single-flight".to_string()), 7)
                    .unwrap();
                if matches!(decision, OpenDecision::Open(_)) {
                    opener_count.fetch_add(1, Ordering::SeqCst);
                }
                decision
            }));
        }

        let mut reservation = None;
        let mut waiters = Vec::new();
        for task in tasks {
            match task.await.unwrap() {
                OpenDecision::Open(candidate) => reservation = Some(candidate),
                OpenDecision::Wait(completion) => waiters.push(completion),
                OpenDecision::Live(_) => panic!("connection cannot be live before publication"),
                OpenDecision::Unavailable(_) => {
                    panic!("opening descriptor cannot already be closing")
                }
            }
        }
        assert_eq!(opener_count.load(Ordering::SeqCst), 1);
        assert_eq!(waiters.len(), CALLERS - 1);

        let reservation = reservation.expect("one caller owns the opening ticket");
        let connection = runtime_connection("descriptor-single-flight", "single");
        assert!(registry
            .publish_open(&reservation, connection.clone())
            .unwrap());
        for waiter in waiters {
            assert_eq!(waiter.wait().await.unwrap(), connection);
        }
        match registry
            .begin_open(&DescriptorId("descriptor-single-flight".to_string()), 7)
            .unwrap()
        {
            OpenDecision::Live(current) => assert_eq!(current, connection),
            _ => panic!("published connection must deduplicate by descriptor"),
        }
    }

    #[tokio::test]
    async fn invalidated_open_ticket_cannot_publish_a_late_connection() {
        let registry = ProfileRuntimeRegistry::default();
        let descriptor_id = DescriptorId("descriptor-invalidated".to_string());
        let reservation = match registry.begin_open(&descriptor_id, 3).unwrap() {
            OpenDecision::Open(reservation) => reservation,
            _ => panic!("first caller must own the opening ticket"),
        };
        let waiter = match registry.begin_open(&descriptor_id, 3).unwrap() {
            OpenDecision::Wait(completion) => completion,
            _ => panic!("second caller must join the opening ticket"),
        };

        assert_eq!(registry.invalidate_open(&descriptor_id).unwrap(), None);
        assert_eq!(
            waiter.wait().await.unwrap_err().code,
            ProfileErrorCode::StaleConnection
        );
        assert!(!registry
            .publish_open(
                &reservation,
                runtime_connection("descriptor-invalidated", "late")
            )
            .unwrap());
        assert!(registry.get(&descriptor_id).is_none());
    }

    #[test]
    fn descriptor_closing_state_blocks_a_reopen_until_exact_teardown_settles() {
        let registry = ProfileRuntimeRegistry::default();
        let connection = runtime_connection("descriptor-closing", "one");
        registry.insert(connection.clone()).unwrap();
        assert_eq!(
            registry.begin_close(&connection.descriptor_id).unwrap(),
            Some(connection.clone())
        );
        match registry.begin_open(&connection.descriptor_id, 1).unwrap() {
            OpenDecision::Unavailable(error) => {
                assert_eq!(error.code, ProfileErrorCode::ConnectionBusy)
            }
            _ => panic!("closing descriptor must not expose or replace its actor"),
        }
        registry.finish_close(&connection, false).unwrap();
        assert_eq!(registry.get(&connection.descriptor_id), Some(connection));
    }

    #[tokio::test]
    async fn connection_affecting_edit_during_engine_open_closes_the_exact_late_handle() {
        let original = tempfile::NamedTempFile::new().unwrap();
        let replacement = tempfile::NamedTempFile::new().unwrap();
        let (opener, started, release) = DeferredProductionPathOpener::new();
        let opener_for_assert = Arc::clone(&opener);
        let (state, profile) = deferred_sqlite_state(original.path(), opener);
        let descriptor_id = profile.descriptor_id.clone();
        let open_state = Arc::clone(&state);
        let open_descriptor = descriptor_id.clone();
        let pending = tokio::spawn(async move { open_state.open_saved(&open_descriptor).await });
        started.await.unwrap();

        state
            .update_profile(ProfileUpdateRequest {
                descriptor_id: descriptor_id.clone(),
                name: "Deferred SQLite moved".to_string(),
                target: ProfileTarget::Sqlite {
                    path: replacement.path().to_string_lossy().into_owned(),
                },
                replacement_credential: None,
            })
            .await
            .unwrap();
        release.send(()).unwrap();

        assert_eq!(
            pending.await.unwrap().unwrap_err().code,
            ProfileErrorCode::StaleConnection
        );
        assert_eq!(opener_for_assert.calls.load(Ordering::SeqCst), 1);
        assert!(state.runtime.get(&descriptor_id).is_none());
        assert!(state.database_state.0.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn concurrent_saved_open_uses_one_engine_opener_and_registers_the_returned_identity() {
        const CALLERS: usize = 10;
        let sqlite = tempfile::NamedTempFile::new().unwrap();
        let (opener, started, release) = DeferredProductionPathOpener::new();
        let opener_for_assert = Arc::clone(&opener);
        let (state, profile) = deferred_sqlite_state(sqlite.path(), opener);
        let mut tasks = Vec::new();
        for _ in 0..CALLERS {
            let state = Arc::clone(&state);
            let descriptor_id = profile.descriptor_id.clone();
            tasks.push(tokio::spawn(async move {
                state.open_saved(&descriptor_id).await
            }));
        }
        started.await.unwrap();
        tokio::task::yield_now().await;
        release.send(()).unwrap();

        let mut opened = Vec::new();
        for task in tasks {
            opened.push(task.await.unwrap().unwrap());
        }
        assert_eq!(opener_for_assert.calls.load(Ordering::SeqCst), 1);
        assert!(opened.iter().all(|connection| connection == &opened[0]));
        let actors = state.database_state.0.lock().unwrap();
        assert_eq!(actors.len(), 1);
        let actor = actors.get(&opened[0].connection_id.0).unwrap();
        assert_eq!(actor.identity().descriptor_id, opened[0].descriptor_id);
        assert_eq!(actor.identity().connection_id, opened[0].connection_id);
        assert_eq!(
            actor.identity().connection_generation,
            opened[0].connection_generation
        );
    }

    #[tokio::test]
    async fn aborted_open_owner_drops_exact_ticket_and_wakes_joined_waiter() {
        let sqlite = tempfile::NamedTempFile::new().unwrap();
        let (opener, started, release) = DeferredProductionPathOpener::new();
        let (state, profile) = deferred_sqlite_state(sqlite.path(), opener);
        let owner_state = Arc::clone(&state);
        let owner_descriptor = profile.descriptor_id.clone();
        let owner = tokio::spawn(async move { owner_state.open_saved(&owner_descriptor).await });
        started.await.unwrap();

        let waiter_state = Arc::clone(&state);
        let waiter_descriptor = profile.descriptor_id.clone();
        let waiter = tokio::spawn(async move { waiter_state.open_saved(&waiter_descriptor).await });
        for _ in 0..100 {
            if state.runtime.opening_waiter_count(&profile.descriptor_id) >= 1 {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(
            state.runtime.opening_waiter_count(&profile.descriptor_id) >= 1,
            "second caller never joined the elected opener"
        );
        owner.abort();
        assert!(owner.await.unwrap_err().is_cancelled());

        let waiter_error = tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
            .await
            .expect("joined opener waiter must be notified when its owner is aborted")
            .unwrap()
            .unwrap_err();
        assert_eq!(waiter_error.code, ProfileErrorCode::StaleConnection);
        assert!(state.runtime.get(&profile.descriptor_id).is_none());
        assert!(state.database_state.0.lock().unwrap().is_empty());
        assert!(
            release.send(()).is_err(),
            "aborted opener must drop its wait"
        );
    }

    #[tokio::test]
    async fn reconnect_discards_a_runtime_identity_whose_exact_actor_is_gone() {
        let sqlite = tempfile::NamedTempFile::new().unwrap();
        let (state, profile) =
            deferred_sqlite_state(sqlite.path(), Arc::new(ProductionDatabaseConnectionOpener));
        let stale = LiveConnection {
            descriptor_id: profile.descriptor_id.clone(),
            connection_id: ConnectionId("connection-gone".to_string()),
            connection_generation: ConnectionGeneration("generation-gone".to_string()),
            engine: LiveDatabaseEngine::Sqlite,
        };
        state.runtime.insert(stale.clone()).unwrap();
        let stale_run = db_service::QueryRunOwner {
            descriptor_id: stale.descriptor_id.clone(),
            connection_id: stale.connection_id.clone(),
            connection_generation: stale.connection_generation.clone(),
            query_run_id: db_service::QueryRunId("run-gone".to_string()),
        };
        let stale_session = db_service::ResultSessionOwner {
            descriptor_id: stale.descriptor_id.clone(),
            connection_id: stale.connection_id.clone(),
            connection_generation: stale.connection_generation.clone(),
            query_run_id: stale_run.query_run_id.clone(),
            statement_execution_id: db_service::StatementExecutionId("statement-gone".to_string()),
            result_session_id: db_service::ResultSessionId("session-gone".to_string()),
        };
        {
            let mut sessions = state.result_sessions.lock().unwrap();
            sessions.begin_run(&stale_run).unwrap();
            sessions
                .begin_session(stale_session, vec!["value".to_string()])
                .unwrap();
        }
        assert_eq!(state.result_sessions.lock().unwrap().session_count(), 1);

        let reconnected = state.open_saved(&profile.descriptor_id).await.unwrap();

        assert_ne!(reconnected.connection_id, stale.connection_id);
        assert_ne!(
            reconnected.connection_generation,
            stale.connection_generation
        );
        assert_eq!(state.runtime.get(&profile.descriptor_id), Some(reconnected));
        assert_eq!(state.database_state.0.lock().unwrap().len(), 1);
        assert_eq!(state.result_sessions.lock().unwrap().session_count(), 0);
    }

    #[tokio::test]
    async fn forget_during_engine_open_closes_the_exact_late_handle_without_a_ghost() {
        let original = tempfile::NamedTempFile::new().unwrap();
        let (opener, started, release) = DeferredProductionPathOpener::new();
        let (state, profile) = deferred_sqlite_state(original.path(), opener);
        let descriptor_id = profile.descriptor_id.clone();
        let open_state = Arc::clone(&state);
        let open_descriptor = descriptor_id.clone();
        let pending = tokio::spawn(async move { open_state.open_saved(&open_descriptor).await });
        started.await.unwrap();

        let forget_descriptor = descriptor_id.clone();
        let snapshot = state
            .with_profiles(move |profiles| profiles.forget(&forget_descriptor))
            .await
            .unwrap();
        assert!(snapshot.profiles.is_empty());
        release.send(()).unwrap();

        assert_eq!(
            pending.await.unwrap().unwrap_err().code,
            ProfileErrorCode::StaleConnection
        );
        assert!(state.runtime.get(&descriptor_id).is_none());
        assert!(state.database_state.0.lock().unwrap().is_empty());
    }

    #[derive(Clone, Copy)]
    enum TestVaultOperation {
        Store,
        Resolve,
        Delete,
    }

    #[derive(Default)]
    struct TestVaultState {
        values: HashMap<(String, String), SecretString>,
        failures: VecDeque<(TestVaultOperation, VaultErrorKind)>,
        store_calls: usize,
        resolve_calls: usize,
        delete_calls: usize,
    }

    #[derive(Default)]
    struct TestVault {
        state: Mutex<TestVaultState>,
    }

    impl TestVault {
        fn fail_next(&self, operation: TestVaultOperation, kind: VaultErrorKind) {
            self.state
                .lock()
                .unwrap()
                .failures
                .push_back((operation, kind));
        }

        fn counts(&self) -> (usize, usize, usize) {
            let state = self.state.lock().unwrap();
            (state.store_calls, state.resolve_calls, state.delete_calls)
        }

        fn generation_count(&self, descriptor_id: &str) -> usize {
            self.state
                .lock()
                .unwrap()
                .values
                .keys()
                .filter(|(descriptor, _)| descriptor == descriptor_id)
                .count()
        }

        fn take_failure(
            state: &mut TestVaultState,
            operation: TestVaultOperation,
        ) -> Option<VaultError> {
            let matches = state
                .failures
                .front()
                .map(|(queued, _)| {
                    std::mem::discriminant(queued) == std::mem::discriminant(&operation)
                })
                .unwrap_or(false);
            matches.then(|| {
                let (_, kind) = state.failures.pop_front().unwrap();
                VaultError::new(kind)
            })
        }
    }

    impl DatabaseCredentialStore for TestVault {
        fn store(
            &self,
            descriptor_id: &DescriptorId,
            generation: &CredentialGeneration,
            secret: SecretString,
        ) -> Result<(), VaultError> {
            let mut state = self.state.lock().unwrap();
            state.store_calls += 1;
            if let Some(error) = Self::take_failure(&mut state, TestVaultOperation::Store) {
                return Err(error);
            }
            state
                .values
                .insert((descriptor_id.0.clone(), generation.0.clone()), secret);
            Ok(())
        }

        fn resolve(
            &self,
            descriptor_id: &DescriptorId,
            generation: &CredentialGeneration,
        ) -> Result<SecretString, VaultError> {
            let mut state = self.state.lock().unwrap();
            state.resolve_calls += 1;
            if let Some(error) = Self::take_failure(&mut state, TestVaultOperation::Resolve) {
                return Err(error);
            }
            state
                .values
                .get(&(descriptor_id.0.clone(), generation.0.clone()))
                .cloned()
                .ok_or_else(|| VaultError::new(VaultErrorKind::Missing))
        }

        fn delete(
            &self,
            descriptor_id: &DescriptorId,
            generation: &CredentialGeneration,
        ) -> Result<(), VaultError> {
            let mut state = self.state.lock().unwrap();
            state.delete_calls += 1;
            if let Some(error) = Self::take_failure(&mut state, TestVaultOperation::Delete) {
                return Err(error);
            }
            state
                .values
                .remove(&(descriptor_id.0.clone(), generation.0.clone()));
            Ok(())
        }
    }

    fn harness() -> (
        DatabaseProfiles,
        Arc<FakeProfileRepository>,
        Arc<TestVault>,
        Arc<FakeLifecycleCloser>,
    ) {
        let repository = Arc::new(FakeProfileRepository::default());
        let vault = Arc::new(TestVault::default());
        let closer = Arc::new(FakeLifecycleCloser::default());
        let profiles = DatabaseProfiles::new(repository.clone(), vault.clone(), closer.clone());
        (profiles, repository, vault, closer)
    }

    fn postgres_request(secret: &str) -> ProfileCreateRequest {
        ProfileCreateRequest {
            name: "Production".to_string(),
            target: ProfileTarget::Postgres {
                host: "db.internal".to_string(),
                port: 5432,
                database: "app".to_string(),
                user: "alice".to_string(),
                ssl: true,
                trust_cert: false,
            },
            credential: Some(CredentialInput {
                password: SecretString::from(secret),
            }),
        }
    }

    fn sqlite_profile(id: &str) -> StoredProfile {
        StoredProfile {
            descriptor_id: DescriptorId(id.to_string()),
            config_generation: 1,
            name: "Local".to_string(),
            target: ProfileTarget::Sqlite {
                path: "/tmp/local.sqlite".to_string(),
            },
            credential_state: CredentialState::NotRequired,
            active_credential_generation: None,
        }
    }

    fn postgres_profile(
        id: &str,
        credential_state: CredentialState,
        generation: Option<&str>,
    ) -> StoredProfile {
        StoredProfile {
            descriptor_id: DescriptorId(id.to_string()),
            config_generation: 1,
            name: "Production".to_string(),
            target: ProfileTarget::Postgres {
                host: "db.internal".to_string(),
                port: 5432,
                database: "app".to_string(),
                user: "alice".to_string(),
                ssl: true,
                trust_cert: false,
            },
            credential_state,
            active_credential_generation: generation
                .map(|generation| CredentialGeneration(generation.to_string())),
        }
    }

    #[test]
    fn file_repository_atomically_reopens_the_last_document() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("profiles.json");
        let repository = FileProfileRepository::new(path.clone());
        assert_eq!(repository.load().unwrap(), ProfileDocument::default());

        let mut document = ProfileDocument::default();
        document.profiles.push(sqlite_profile("descriptor-1"));
        repository.replace(&document).unwrap();

        let reopened = FileProfileRepository::new(path);
        assert_eq!(reopened.load().unwrap(), document);
    }

    #[test]
    fn config_generation_defaults_for_p2_documents_and_increments_on_target_edit() {
        let legacy: StoredProfile = serde_json::from_value(serde_json::json!({
            "descriptorId": "descriptor-legacy",
            "name": "Legacy",
            "target": { "kind": "sqlite", "path": "/tmp/legacy.sqlite" },
            "credentialState": "notRequired",
            "activeCredentialGeneration": null
        }))
        .unwrap();
        assert_eq!(legacy.config_generation, 1);

        let (profiles, _, _, _) = harness();
        let created = profiles
            .create(ProfileCreateRequest {
                name: "Local".to_string(),
                target: ProfileTarget::Sqlite {
                    path: "/tmp/first.sqlite".to_string(),
                },
                credential: None,
            })
            .unwrap();
        assert_eq!(created.config_generation, 1);
        let updated = profiles
            .update(ProfileUpdateRequest {
                descriptor_id: created.descriptor_id,
                name: "Local moved".to_string(),
                target: ProfileTarget::Sqlite {
                    path: "/tmp/second.sqlite".to_string(),
                },
                replacement_credential: None,
            })
            .unwrap();
        assert_eq!(updated.config_generation, 2);
    }

    #[test]
    fn runtime_isolates_same_target_profiles_by_opaque_descriptor() {
        let runtime = ProfileRuntimeRegistry::default();
        let alice = runtime_connection("descriptor-alice", "alice");
        let bob_tls = runtime_connection("descriptor-bob-tls", "bob-tls");

        runtime.insert(alice.clone()).unwrap();
        runtime.insert(bob_tls.clone()).unwrap();

        assert_eq!(runtime.get(&alice.descriptor_id), Some(alice));
        assert_eq!(runtime.get(&bob_tls.descriptor_id), Some(bob_tls));
        assert_eq!(runtime.connections.lock().unwrap().entries.len(), 2);
    }

    #[test]
    fn fake_repository_failures_never_replace_the_durable_snapshot_before_rename() {
        let repository = FakeProfileRepository::default();
        let original = ProfileDocument::default();
        repository.replace(&original).unwrap();
        let mut changed = original.clone();
        changed.profiles.push(sqlite_profile("descriptor-1"));

        let cases = [
            (
                RepositoryFailurePoint::Permission,
                ProfileRepositoryErrorKind::PermissionDenied,
            ),
            (
                RepositoryFailurePoint::Quota,
                ProfileRepositoryErrorKind::QuotaExceeded,
            ),
            (
                RepositoryFailurePoint::TempWrite,
                ProfileRepositoryErrorKind::TempWriteFailed,
            ),
            (
                RepositoryFailurePoint::Sync,
                ProfileRepositoryErrorKind::SyncFailed,
            ),
            (
                RepositoryFailurePoint::Rename,
                ProfileRepositoryErrorKind::RenameFailed,
            ),
        ];
        for (point, expected) in cases {
            repository.fail_next(point);
            assert_eq!(repository.replace(&changed).unwrap_err().kind(), expected);
            assert_eq!(repository.reopen().load().unwrap(), original);
        }
    }

    #[test]
    fn parent_sync_failure_reports_uncertainty_but_reopen_observes_the_atomic_replace() {
        let repository = FakeProfileRepository::default();
        let mut changed = ProfileDocument::default();
        changed.profiles.push(sqlite_profile("descriptor-1"));
        repository.fail_next(RepositoryFailurePoint::ParentSync);

        assert_eq!(
            repository.replace(&changed).unwrap_err().kind(),
            ProfileRepositoryErrorKind::ParentSyncFailed
        );
        assert_eq!(repository.reopen().load().unwrap(), changed);
    }

    #[test]
    fn repository_rejects_duplicate_descriptors_and_pending_rows() {
        let repository = FakeProfileRepository::default();
        let profile = sqlite_profile("descriptor-1");
        let mut duplicate = ProfileDocument::default();
        duplicate.profiles = vec![profile.clone(), profile.clone()];
        assert_eq!(
            repository.replace(&duplicate).unwrap_err().kind(),
            ProfileRepositoryErrorKind::Corrupt
        );

        let generation = CredentialGeneration("credential-1".to_string());
        let operation = PendingOperation::PendingCreate {
            operation_id: "operation-1".to_string(),
            profile,
            credential_generation: generation,
        };
        let mut duplicate_pending = ProfileDocument::default();
        duplicate_pending.pending_operations = vec![operation.clone(), operation];
        assert_eq!(
            repository.replace(&duplicate_pending).unwrap_err().kind(),
            ProfileRepositoryErrorKind::Corrupt
        );
    }

    #[test]
    fn repository_rejects_inconsistent_profile_credential_state() {
        let repository = FakeProfileRepository::default();
        let mut sqlite_required = sqlite_profile("sqlite-required");
        sqlite_required.credential_state = CredentialState::Required;
        let mut sqlite_stored = sqlite_profile("sqlite-stored");
        sqlite_stored.credential_state = CredentialState::Stored;
        sqlite_stored.active_credential_generation =
            Some(CredentialGeneration("credential-sqlite".to_string()));
        let cases = [
            (
                "stored network profile without an active generation",
                postgres_profile("stored-missing", CredentialState::Stored, None),
            ),
            (
                "required network profile with an active generation",
                postgres_profile(
                    "required-active",
                    CredentialState::Required,
                    Some("credential-required"),
                ),
            ),
            (
                "network profile claiming credentials are not required",
                postgres_profile("network-not-required", CredentialState::NotRequired, None),
            ),
            (
                "unavailable network profile without a known generation",
                postgres_profile("unavailable-missing", CredentialState::Unavailable, None),
            ),
            (
                "SQLite profile claiming a credential is required",
                sqlite_required,
            ),
            (
                "SQLite profile carrying a credential generation",
                sqlite_stored,
            ),
        ];

        for (case, profile) in cases {
            let document = ProfileDocument {
                profiles: vec![profile],
                ..ProfileDocument::default()
            };
            assert_eq!(
                repository.replace(&document).unwrap_err().kind(),
                ProfileRepositoryErrorKind::Corrupt,
                "{case}"
            );
        }

        let valid_unavailable = ProfileDocument {
            profiles: vec![postgres_profile(
                "unavailable-known",
                CredentialState::Unavailable,
                Some("credential-known"),
            )],
            ..ProfileDocument::default()
        };
        repository.replace(&valid_unavailable).unwrap();
    }

    #[test]
    fn repository_rejects_referentially_inconsistent_pending_operations() {
        let old_generation = CredentialGeneration("credential-old".to_string());
        let new_generation = CredentialGeneration("credential-new".to_string());
        let current = postgres_profile(
            "descriptor-1",
            CredentialState::Stored,
            Some(&old_generation.0),
        );
        let replacement = postgres_profile(
            "descriptor-1",
            CredentialState::Stored,
            Some(&new_generation.0),
        );
        let pending_create_profile =
            postgres_profile("descriptor-1", CredentialState::Stored, None);
        let cases = vec![
            (
                "pendingCreate descriptor already exists",
                ProfileDocument {
                    profiles: vec![current.clone()],
                    pending_operations: vec![PendingOperation::PendingCreate {
                        operation_id: "operation-create".to_string(),
                        profile: pending_create_profile,
                        credential_generation: new_generation.clone(),
                    }],
                    ..ProfileDocument::default()
                },
            ),
            (
                "pendingReplace current descriptor is missing",
                ProfileDocument {
                    pending_operations: vec![PendingOperation::PendingReplace {
                        operation_id: "operation-replace-missing".to_string(),
                        descriptor_id: DescriptorId("descriptor-1".to_string()),
                        replacement: replacement.clone(),
                        old_generation: Some(old_generation.clone()),
                        new_generation: new_generation.clone(),
                    }],
                    ..ProfileDocument::default()
                },
            ),
            (
                "pendingReplace replacement descriptor does not match",
                ProfileDocument {
                    profiles: vec![current.clone()],
                    pending_operations: vec![PendingOperation::PendingReplace {
                        operation_id: "operation-replace-descriptor".to_string(),
                        descriptor_id: DescriptorId("descriptor-1".to_string()),
                        replacement: postgres_profile(
                            "descriptor-2",
                            CredentialState::Stored,
                            Some(&new_generation.0),
                        ),
                        old_generation: Some(old_generation.clone()),
                        new_generation: new_generation.clone(),
                    }],
                    ..ProfileDocument::default()
                },
            ),
            (
                "pendingReplace old generation is not current",
                ProfileDocument {
                    profiles: vec![current.clone()],
                    pending_operations: vec![PendingOperation::PendingReplace {
                        operation_id: "operation-replace-old".to_string(),
                        descriptor_id: DescriptorId("descriptor-1".to_string()),
                        replacement: replacement.clone(),
                        old_generation: Some(CredentialGeneration("credential-other".to_string())),
                        new_generation: new_generation.clone(),
                    }],
                    ..ProfileDocument::default()
                },
            ),
            (
                "pendingReplace replacement does not activate new generation",
                ProfileDocument {
                    profiles: vec![current.clone()],
                    pending_operations: vec![PendingOperation::PendingReplace {
                        operation_id: "operation-replace-new".to_string(),
                        descriptor_id: DescriptorId("descriptor-1".to_string()),
                        replacement: postgres_profile(
                            "descriptor-1",
                            CredentialState::Stored,
                            Some("credential-other"),
                        ),
                        old_generation: Some(old_generation.clone()),
                        new_generation: new_generation.clone(),
                    }],
                    ..ProfileDocument::default()
                },
            ),
            (
                "cleanupOld active generation is not current",
                ProfileDocument {
                    profiles: vec![current.clone()],
                    pending_operations: vec![PendingOperation::CleanupOld {
                        operation_id: "operation-cleanup-active".to_string(),
                        descriptor_id: DescriptorId("descriptor-1".to_string()),
                        old_generation: CredentialGeneration("credential-older".to_string()),
                        active_generation: new_generation.clone(),
                    }],
                    ..ProfileDocument::default()
                },
            ),
            (
                "cleanupOld tries to delete the active generation",
                ProfileDocument {
                    profiles: vec![current.clone()],
                    pending_operations: vec![PendingOperation::CleanupOld {
                        operation_id: "operation-cleanup-same".to_string(),
                        descriptor_id: DescriptorId("descriptor-1".to_string()),
                        old_generation: old_generation.clone(),
                        active_generation: old_generation.clone(),
                    }],
                    ..ProfileDocument::default()
                },
            ),
            (
                "pendingForget descriptor is missing",
                ProfileDocument {
                    pending_operations: vec![PendingOperation::PendingForget {
                        operation_id: "operation-forget-missing".to_string(),
                        descriptor_id: DescriptorId("descriptor-1".to_string()),
                        generations: vec![old_generation.clone()],
                    }],
                    ..ProfileDocument::default()
                },
            ),
            (
                "pendingForget repeats a generation",
                ProfileDocument {
                    profiles: vec![current.clone()],
                    pending_operations: vec![PendingOperation::PendingForget {
                        operation_id: "operation-forget-duplicate".to_string(),
                        descriptor_id: DescriptorId("descriptor-1".to_string()),
                        generations: vec![old_generation.clone(), old_generation.clone()],
                    }],
                    ..ProfileDocument::default()
                },
            ),
            (
                "pendingRemoveCredential names an unmanaged generation",
                ProfileDocument {
                    profiles: vec![current],
                    pending_operations: vec![PendingOperation::PendingRemoveCredential {
                        operation_id: "operation-remove-unknown".to_string(),
                        descriptor_id: DescriptorId("descriptor-1".to_string()),
                        generations: vec![CredentialGeneration("credential-other".to_string())],
                    }],
                    ..ProfileDocument::default()
                },
            ),
        ];

        for (case, document) in cases {
            let repository = FakeProfileRepository::default();
            assert_eq!(
                repository.replace(&document).unwrap_err().kind(),
                ProfileRepositoryErrorKind::Corrupt,
                "{case}"
            );
        }
    }

    #[test]
    fn startup_load_reads_only_non_secret_repository_and_never_touches_vault() {
        let (profiles, repository, vault, _) = harness();
        let mut document = ProfileDocument::default();
        document.profiles.push(sqlite_profile("descriptor-1"));
        repository.replace(&document).unwrap();

        let loaded = profiles.load().unwrap();

        assert_eq!(loaded.profiles.len(), 1);
        assert!(loaded.recovery.is_empty());
        assert_eq!(vault.counts(), (0, 0, 0));
    }

    #[tokio::test]
    async fn test_connection_is_ephemeral_and_never_registers_a_live_handle() {
        let repository = Arc::new(FakeProfileRepository::default());
        let vault = Arc::new(TestVault::default());
        let closer: Arc<dyn DatabaseLifecycleCloser> = Arc::new(FakeLifecycleCloser::default());
        let database_state = DbState::default();
        let runtime = ProfileRuntimeRegistry::default();
        let state = DatabaseProfileState {
            profiles: Arc::new(Mutex::new(DatabaseProfiles::new(
                repository,
                vault.clone(),
                closer.clone(),
            ))),
            runtime: runtime.clone(),
            database_state: database_state.clone(),
            result_sessions: ResultSessionState::default(),
            opener: Arc::new(ProductionDatabaseConnectionOpener),
        };
        assert!(database_state.0.lock().unwrap().is_empty());
        assert!(runtime.connections.lock().unwrap().is_empty());
        let sqlite_file = tempfile::NamedTempFile::new().unwrap();

        let result = state
            .test_connection(TestConnectionRequest::Ephemeral {
                target: ProfileTarget::Sqlite {
                    path: sqlite_file.path().to_string_lossy().into_owned(),
                },
                credential: None,
            })
            .await
            .unwrap();

        assert!(result.server_version.is_some());
        assert!(database_state.0.lock().unwrap().is_empty());
        assert!(runtime.connections.lock().unwrap().is_empty());
        let loaded = state.profiles.lock().unwrap().load().unwrap();
        assert!(loaded.profiles.is_empty());
        assert!(loaded.recovery.is_empty());
        assert_eq!(vault.counts(), (0, 0, 0));
    }

    #[tokio::test]
    async fn sqlite_missing_path_is_typed_for_open_and_test_without_creating_a_file() {
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("missing.sqlite");
        let (state, profile) =
            deferred_sqlite_state(&missing, Arc::new(ProductionDatabaseConnectionOpener));

        assert_eq!(
            state
                .open_saved(&profile.descriptor_id)
                .await
                .unwrap_err()
                .code,
            ProfileErrorCode::SqlitePathMissing
        );
        assert_eq!(
            state
                .test_connection(TestConnectionRequest::Ephemeral {
                    target: ProfileTarget::Sqlite {
                        path: missing.to_string_lossy().into_owned(),
                    },
                    credential: None,
                })
                .await
                .unwrap_err()
                .code,
            ProfileErrorCode::SqlitePathMissing
        );
        assert!(!missing.exists());
        assert!(state.runtime.get(&profile.descriptor_id).is_none());
        assert!(state.database_state.0.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn profile_list_runtime_path_reads_ledger_without_resolving_the_vault() {
        let repository = Arc::new(FakeProfileRepository::default());
        let vault = Arc::new(TestVault::default());
        let closer: Arc<dyn DatabaseLifecycleCloser> = Arc::new(FakeLifecycleCloser::default());
        let profiles = DatabaseProfiles::new(repository, vault.clone(), closer.clone());
        let created = profiles.create(postgres_request(SENTINEL)).unwrap();
        let counts_before_list = vault.counts();
        let state = DatabaseProfileState {
            profiles: Arc::new(Mutex::new(profiles)),
            runtime: ProfileRuntimeRegistry::default(),
            database_state: DbState::default(),
            result_sessions: ResultSessionState::default(),
            opener: Arc::new(ProductionDatabaseConnectionOpener),
        };

        let loaded = state.list_profiles().await.unwrap();

        assert_eq!(loaded.profiles[0].descriptor_id, created.descriptor_id);
        assert!(loaded.recovery.is_empty());
        assert_eq!(vault.counts(), counts_before_list);
        assert!(state.database_state.0.lock().unwrap().is_empty());
        assert!(state.runtime.connections.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn keep_credential_target_edit_closes_registered_handle_before_repository_update() {
        let repository = Arc::new(FakeProfileRepository::default());
        let vault = Arc::new(TestVault::default());
        let database_state = DbState::default();
        let runtime = ProfileRuntimeRegistry::default();
        let closer: Arc<dyn DatabaseLifecycleCloser> = Arc::new(RegisteredProfileCloser {
            database_state: database_state.clone(),
            runtime: runtime.clone(),
            result_sessions: ResultSessionState::default(),
        });
        let profiles = DatabaseProfiles::new(repository.clone(), vault.clone(), closer.clone());
        let created = profiles.create(postgres_request(SENTINEL)).unwrap();
        let active_generation = repository.reopen().load().unwrap().profiles[0]
            .active_credential_generation
            .clone();
        let counts_before_update = vault.counts();
        let connection = LiveConnection {
            descriptor_id: created.descriptor_id.clone(),
            connection_id: ConnectionId("connection-edit".to_string()),
            connection_generation: ConnectionGeneration("generation-edit".to_string()),
            engine: LiveDatabaseEngine::Postgres,
        };
        runtime.insert(connection.clone()).unwrap();
        db_service::register_actor(
            &database_state,
            Arc::new(crate::db_connection_actor::ProductionConnectionActor::new(
                db_service::ConnectionIdentity {
                    descriptor_id: connection.descriptor_id.clone(),
                    connection_id: connection.connection_id.clone(),
                    connection_generation: connection.connection_generation.clone(),
                },
                DbHandle::Sqlite(Mutex::new(rusqlite::Connection::open_in_memory().unwrap())),
            )),
        )
        .unwrap();
        let state = DatabaseProfileState {
            profiles: Arc::new(Mutex::new(profiles)),
            runtime: runtime.clone(),
            database_state: database_state.clone(),
            result_sessions: ResultSessionState::default(),
            opener: Arc::new(ProductionDatabaseConnectionOpener),
        };

        let updated = state
            .update_profile(ProfileUpdateRequest {
                descriptor_id: created.descriptor_id.clone(),
                name: "Production moved".to_string(),
                target: ProfileTarget::Postgres {
                    host: "db-moved.internal".to_string(),
                    port: 5432,
                    database: "app".to_string(),
                    user: "alice".to_string(),
                    ssl: true,
                    trust_cert: false,
                },
                replacement_credential: None,
            })
            .await
            .unwrap();

        assert!(database_state.0.lock().unwrap().is_empty());
        assert!(runtime.get(&created.descriptor_id).is_none());
        assert_eq!(updated.name, "Production moved");
        assert!(matches!(
            updated.target,
            ProfileTarget::Postgres { ref host, .. } if host == "db-moved.internal"
        ));
        let durable = repository.reopen().load().unwrap();
        assert_eq!(
            durable.profiles[0].active_credential_generation,
            active_generation
        );
        assert!(durable.pending_operations.is_empty());
        assert_eq!(vault.counts(), counts_before_update);
    }

    #[test]
    fn keep_credential_target_edit_close_failure_preserves_repository_and_vault() {
        let (profiles, repository, vault, closer) = harness();
        let created = profiles.create(postgres_request("old-secret")).unwrap();
        let durable_before = repository.reopen().load().unwrap();
        let counts_before = vault.counts();
        closer.fail_next(LifecycleCloseErrorKind::CloseFailed);

        let error = profiles
            .update(ProfileUpdateRequest {
                descriptor_id: created.descriptor_id.clone(),
                name: "Production moved".to_string(),
                target: ProfileTarget::Postgres {
                    host: "db-moved.internal".to_string(),
                    port: 5432,
                    database: "app".to_string(),
                    user: "alice".to_string(),
                    ssl: true,
                    trust_cert: false,
                },
                replacement_credential: None,
            })
            .unwrap_err();

        assert_eq!(error.code, ProfileErrorCode::LifecycleCloseFailed);
        assert_eq!(closer.call_count(&created.descriptor_id.0), 1);
        assert_eq!(repository.reopen().load().unwrap(), durable_before);
        assert_eq!(vault.counts(), counts_before);
    }

    #[test]
    fn create_persists_pending_before_vault_write_and_keeps_secret_out_of_repository() {
        let (profiles, repository, vault, _) = harness();
        repository.fail_next(RepositoryFailurePoint::Rename);

        let error = profiles.create(postgres_request(SENTINEL)).unwrap_err();

        assert_eq!(error.code, ProfileErrorCode::RepositoryUnavailable);
        assert_eq!(vault.counts(), (0, 0, 0));
        assert!(!String::from_utf8_lossy(&repository.durable_bytes()).contains(SENTINEL));
    }

    #[test]
    fn vault_write_failure_reopens_as_pending_create_and_explicit_resume_is_idempotent() {
        let (profiles, repository, vault, closer) = harness();
        vault.fail_next(TestVaultOperation::Store, VaultErrorKind::WriteFailed);
        assert_eq!(
            profiles
                .create(postgres_request(SENTINEL))
                .unwrap_err()
                .code,
            ProfileErrorCode::VaultWriteFailed
        );
        let reopened = DatabaseProfiles::new(Arc::new(repository.reopen()), vault.clone(), closer);
        let pending = reopened.load().unwrap();
        assert!(pending.profiles.is_empty());
        assert_eq!(
            pending.recovery[0].kind,
            PendingOperationKind::PendingCreate
        );
        assert!(!String::from_utf8_lossy(&repository.durable_bytes()).contains(SENTINEL));

        let completed = reopened
            .recover(ProfileRecoveryRequest {
                operation_id: pending.recovery[0].operation_id.clone(),
                action: RecoveryAction::Resume,
                credential: Some(CredentialInput {
                    password: SecretString::from(SENTINEL),
                }),
            })
            .unwrap();
        assert_eq!(
            completed.profiles[0].credential_state,
            CredentialState::Stored
        );
        assert!(completed.recovery.is_empty());
        assert_eq!(
            vault.generation_count(&completed.profiles[0].descriptor_id.0),
            1
        );
        assert_eq!(reopened.load().unwrap(), completed);
    }

    #[test]
    fn create_final_replace_failure_reopens_pending_and_resume_keeps_single_generation() {
        let (profiles, repository, vault, closer) = harness();
        repository.fail_nth_replace(2, RepositoryFailurePoint::Rename);

        assert_eq!(
            profiles
                .create(postgres_request(SENTINEL))
                .unwrap_err()
                .code,
            ProfileErrorCode::RepositoryUnavailable
        );
        assert_eq!(vault.counts().0, 1);

        let durable_pending = repository.reopen().load().unwrap();
        assert!(durable_pending.profiles.is_empty());
        let (descriptor_id, generation) = match durable_pending.pending_operations.as_slice() {
            [PendingOperation::PendingCreate {
                profile,
                credential_generation,
                ..
            }] => (profile.descriptor_id.clone(), credential_generation.clone()),
            operations => panic!("expected one pendingCreate operation, got {operations:?}"),
        };
        assert_eq!(vault.generation_count(&descriptor_id.0), 1);

        let reopened = DatabaseProfiles::new(Arc::new(repository.reopen()), vault.clone(), closer);
        let pending = reopened.load().unwrap();
        assert!(pending.profiles.is_empty());
        assert_eq!(
            pending.recovery[0].kind,
            PendingOperationKind::PendingCreate
        );
        let completed = reopened
            .recover(ProfileRecoveryRequest {
                operation_id: pending.recovery[0].operation_id.clone(),
                action: RecoveryAction::Resume,
                credential: None,
            })
            .unwrap();

        assert!(completed.recovery.is_empty());
        assert_eq!(completed.profiles.len(), 1);
        assert_eq!(
            completed.profiles[0].credential_state,
            CredentialState::Stored
        );
        assert_eq!(
            vault.counts().0,
            1,
            "resume must not rewrite an existing generation"
        );
        assert_eq!(vault.generation_count(&descriptor_id.0), 1);
        let durable_completed = repository.reopen().load().unwrap();
        assert!(durable_completed.pending_operations.is_empty());
        assert_eq!(
            durable_completed.profiles[0].active_credential_generation,
            Some(generation)
        );
    }

    #[test]
    fn pending_create_abort_delete_failure_reopens_and_retries_idempotently() {
        let (profiles, repository, vault, closer) = harness();
        repository.fail_nth_replace(2, RepositoryFailurePoint::Rename);
        assert_eq!(
            profiles
                .create(postgres_request("abort-create-secret"))
                .unwrap_err()
                .code,
            ProfileErrorCode::RepositoryUnavailable
        );
        let durable_pending = repository.reopen().load().unwrap();
        let (operation_id, descriptor_id, generation) =
            match durable_pending.pending_operations.as_slice() {
                [PendingOperation::PendingCreate {
                    operation_id,
                    profile,
                    credential_generation,
                }] => (
                    operation_id.clone(),
                    profile.descriptor_id.clone(),
                    credential_generation.clone(),
                ),
                operations => panic!("expected one pendingCreate operation, got {operations:?}"),
            };
        assert!(durable_pending.profiles.is_empty());
        assert_eq!(vault.generation_count(&descriptor_id.0), 1);

        vault.fail_next(TestVaultOperation::Delete, VaultErrorKind::DeleteFailed);
        let reopened =
            DatabaseProfiles::new(Arc::new(repository.reopen()), vault.clone(), closer.clone());
        assert_eq!(
            reopened
                .recover(ProfileRecoveryRequest {
                    operation_id: operation_id.clone(),
                    action: RecoveryAction::Abort,
                    credential: None,
                })
                .unwrap_err()
                .code,
            ProfileErrorCode::VaultDeleteFailed
        );
        let after_delete_failure = repository.reopen().load().unwrap();
        assert!(after_delete_failure.profiles.is_empty());
        assert_eq!(after_delete_failure.pending_operations.len(), 1);
        assert_eq!(vault.generation_count(&descriptor_id.0), 1);

        let retried = DatabaseProfiles::new(Arc::new(repository.reopen()), vault.clone(), closer);
        let completed = retried
            .recover(ProfileRecoveryRequest {
                operation_id: operation_id.clone(),
                action: RecoveryAction::Abort,
                credential: None,
            })
            .unwrap();
        assert!(completed.profiles.is_empty());
        assert!(completed.recovery.is_empty());
        assert_eq!(vault.generation_count(&descriptor_id.0), 0);
        let durable_completed = repository.reopen().load().unwrap();
        assert!(durable_completed.profiles.is_empty());
        assert!(durable_completed.pending_operations.is_empty());
        assert_eq!(
            retried
                .recover(ProfileRecoveryRequest {
                    operation_id,
                    action: RecoveryAction::Abort,
                    credential: None,
                })
                .unwrap_err()
                .code,
            ProfileErrorCode::RecoveryNotFound
        );
        assert_eq!(
            vault
                .resolve(&descriptor_id, &generation)
                .unwrap_err()
                .kind(),
            VaultErrorKind::Missing
        );
    }

    #[test]
    fn replace_switch_failure_reopens_old_active_and_resume_keeps_only_new_generation() {
        let (profiles, repository, vault, closer) = harness();
        let created = profiles.create(postgres_request("old-secret")).unwrap();
        let old_generation = repository.reopen().load().unwrap().profiles[0]
            .active_credential_generation
            .clone()
            .unwrap();
        repository.fail_nth_replace(2, RepositoryFailurePoint::Rename);

        let error = profiles
            .update(ProfileUpdateRequest {
                descriptor_id: created.descriptor_id.clone(),
                name: "Production v2".to_string(),
                target: ProfileTarget::Postgres {
                    host: "db-v2.internal".to_string(),
                    port: 5432,
                    database: "app".to_string(),
                    user: "alice".to_string(),
                    ssl: true,
                    trust_cert: false,
                },
                replacement_credential: Some(CredentialInput {
                    password: SecretString::from("new-secret"),
                }),
            })
            .unwrap_err();
        assert_eq!(error.code, ProfileErrorCode::RepositoryUnavailable);

        let durable_pending = repository.reopen().load().unwrap();
        assert_eq!(durable_pending.profiles[0].name, "Production");
        assert_eq!(
            durable_pending.profiles[0].active_credential_generation,
            Some(old_generation.clone())
        );
        let new_generation = match durable_pending.pending_operations.as_slice() {
            [PendingOperation::PendingReplace {
                descriptor_id,
                replacement,
                old_generation: pending_old,
                new_generation,
                ..
            }] => {
                assert_eq!(descriptor_id, &created.descriptor_id);
                assert_eq!(replacement.descriptor_id, created.descriptor_id);
                assert_eq!(replacement.name, "Production v2");
                assert_eq!(pending_old.as_ref(), Some(&old_generation));
                assert_eq!(
                    replacement.active_credential_generation.as_ref(),
                    Some(new_generation)
                );
                new_generation.clone()
            }
            operations => panic!("expected one pendingReplace operation, got {operations:?}"),
        };
        assert_eq!(vault.counts().0, 2);
        assert_eq!(vault.generation_count(&created.descriptor_id.0), 2);

        let reopened = DatabaseProfiles::new(Arc::new(repository.reopen()), vault.clone(), closer);
        let pending = reopened.load().unwrap();
        assert_eq!(pending.profiles[0].name, "Production");
        assert_eq!(
            pending.recovery[0].kind,
            PendingOperationKind::PendingReplace
        );
        let completed = reopened
            .recover(ProfileRecoveryRequest {
                operation_id: pending.recovery[0].operation_id.clone(),
                action: RecoveryAction::Resume,
                credential: None,
            })
            .unwrap();

        assert_eq!(completed.profiles[0].name, "Production v2");
        assert!(completed.recovery.is_empty());
        assert_eq!(
            vault.counts().0,
            2,
            "resume must reuse the stored new generation"
        );
        assert_eq!(vault.generation_count(&created.descriptor_id.0), 1);
        assert!(vault
            .resolve(&created.descriptor_id, &new_generation)
            .is_ok());
        assert_eq!(
            vault
                .resolve(&created.descriptor_id, &old_generation)
                .unwrap_err()
                .kind(),
            VaultErrorKind::Missing
        );
        let durable_completed = repository.reopen().load().unwrap();
        assert!(durable_completed.pending_operations.is_empty());
        assert_eq!(
            durable_completed.profiles[0].active_credential_generation,
            Some(new_generation)
        );
    }

    #[test]
    fn pending_replace_abort_finalization_failure_reopens_and_preserves_old_active() {
        let (profiles, repository, vault, closer) = harness();
        let created = profiles.create(postgres_request("old-secret")).unwrap();
        let old_generation = repository.reopen().load().unwrap().profiles[0]
            .active_credential_generation
            .clone()
            .unwrap();
        repository.fail_nth_replace(2, RepositoryFailurePoint::Rename);
        assert_eq!(
            profiles
                .update(ProfileUpdateRequest {
                    descriptor_id: created.descriptor_id.clone(),
                    name: "Production replacement".to_string(),
                    target: ProfileTarget::Postgres {
                        host: "replacement.internal".to_string(),
                        port: 5432,
                        database: "app".to_string(),
                        user: "alice".to_string(),
                        ssl: true,
                        trust_cert: false,
                    },
                    replacement_credential: Some(CredentialInput {
                        password: SecretString::from("new-secret"),
                    }),
                })
                .unwrap_err()
                .code,
            ProfileErrorCode::RepositoryUnavailable
        );
        let durable_pending = repository.reopen().load().unwrap();
        let (operation_id, new_generation) = match durable_pending.pending_operations.as_slice() {
            [PendingOperation::PendingReplace {
                operation_id,
                descriptor_id,
                old_generation: pending_old,
                new_generation,
                ..
            }] => {
                assert_eq!(descriptor_id, &created.descriptor_id);
                assert_eq!(pending_old.as_ref(), Some(&old_generation));
                (operation_id.clone(), new_generation.clone())
            }
            operations => panic!("expected one pendingReplace operation, got {operations:?}"),
        };
        assert_eq!(
            durable_pending.profiles[0].active_credential_generation,
            Some(old_generation.clone())
        );
        assert_eq!(vault.generation_count(&created.descriptor_id.0), 2);

        repository.fail_next(RepositoryFailurePoint::Rename);
        let reopened =
            DatabaseProfiles::new(Arc::new(repository.reopen()), vault.clone(), closer.clone());
        assert_eq!(
            reopened
                .recover(ProfileRecoveryRequest {
                    operation_id: operation_id.clone(),
                    action: RecoveryAction::Abort,
                    credential: None,
                })
                .unwrap_err()
                .code,
            ProfileErrorCode::RepositoryUnavailable
        );
        let after_finalize_failure = repository.reopen().load().unwrap();
        assert_eq!(after_finalize_failure.pending_operations.len(), 1);
        assert_eq!(
            after_finalize_failure.profiles[0].active_credential_generation,
            Some(old_generation.clone())
        );
        assert_eq!(vault.generation_count(&created.descriptor_id.0), 1);
        assert!(vault
            .resolve(&created.descriptor_id, &old_generation)
            .is_ok());
        assert_eq!(
            vault
                .resolve(&created.descriptor_id, &new_generation)
                .unwrap_err()
                .kind(),
            VaultErrorKind::Missing
        );

        let retried = DatabaseProfiles::new(Arc::new(repository.reopen()), vault.clone(), closer);
        let completed = retried
            .recover(ProfileRecoveryRequest {
                operation_id: operation_id.clone(),
                action: RecoveryAction::Abort,
                credential: None,
            })
            .unwrap();
        assert!(completed.recovery.is_empty());
        assert_eq!(completed.profiles.len(), 1);
        assert_eq!(
            completed.profiles[0].credential_state,
            CredentialState::Stored
        );
        assert_eq!(vault.generation_count(&created.descriptor_id.0), 1);
        let durable_completed = repository.reopen().load().unwrap();
        assert!(durable_completed.pending_operations.is_empty());
        assert_eq!(
            durable_completed.profiles[0].active_credential_generation,
            Some(old_generation)
        );
        assert_eq!(
            retried
                .recover(ProfileRecoveryRequest {
                    operation_id,
                    action: RecoveryAction::Abort,
                    credential: None,
                })
                .unwrap_err()
                .code,
            ProfileErrorCode::RecoveryNotFound
        );
    }

    #[test]
    fn cleanup_clear_failure_reopens_cleanup_and_retry_keeps_only_active_generation() {
        let (profiles, repository, vault, closer) = harness();
        let created = profiles.create(postgres_request("old-secret")).unwrap();
        let old_generation = repository.reopen().load().unwrap().profiles[0]
            .active_credential_generation
            .clone()
            .unwrap();
        repository.fail_nth_replace(3, RepositoryFailurePoint::Rename);

        let error = profiles
            .update(ProfileUpdateRequest {
                descriptor_id: created.descriptor_id.clone(),
                name: "Production v2".to_string(),
                target: ProfileTarget::Postgres {
                    host: "db-v2.internal".to_string(),
                    port: 5432,
                    database: "app".to_string(),
                    user: "alice".to_string(),
                    ssl: true,
                    trust_cert: false,
                },
                replacement_credential: Some(CredentialInput {
                    password: SecretString::from("new-secret"),
                }),
            })
            .unwrap_err();
        assert_eq!(error.code, ProfileErrorCode::RepositoryUnavailable);

        let durable_pending = repository.reopen().load().unwrap();
        assert_eq!(durable_pending.profiles[0].name, "Production v2");
        let active_generation = match durable_pending.pending_operations.as_slice() {
            [PendingOperation::CleanupOld {
                descriptor_id,
                old_generation: pending_old,
                active_generation,
                ..
            }] => {
                assert_eq!(descriptor_id, &created.descriptor_id);
                assert_eq!(pending_old, &old_generation);
                active_generation.clone()
            }
            operations => panic!("expected one cleanupOld operation, got {operations:?}"),
        };
        assert_eq!(
            durable_pending.profiles[0]
                .active_credential_generation
                .as_ref(),
            Some(&active_generation)
        );
        assert_eq!(vault.generation_count(&created.descriptor_id.0), 1);
        assert_eq!(
            vault
                .resolve(&created.descriptor_id, &old_generation)
                .unwrap_err()
                .kind(),
            VaultErrorKind::Missing
        );

        let deletes_before_retry = vault.counts().2;
        let reopened = DatabaseProfiles::new(Arc::new(repository.reopen()), vault.clone(), closer);
        let pending = reopened.load().unwrap();
        assert_eq!(pending.recovery[0].kind, PendingOperationKind::CleanupOld);
        let completed = reopened
            .recover(ProfileRecoveryRequest {
                operation_id: pending.recovery[0].operation_id.clone(),
                action: RecoveryAction::RetryCleanup,
                credential: None,
            })
            .unwrap();

        assert!(completed.recovery.is_empty());
        assert_eq!(completed.profiles[0].name, "Production v2");
        assert_eq!(vault.counts().2, deletes_before_retry + 1);
        assert_eq!(vault.generation_count(&created.descriptor_id.0), 1);
        assert!(vault
            .resolve(&created.descriptor_id, &active_generation)
            .is_ok());
        let durable_completed = repository.reopen().load().unwrap();
        assert!(durable_completed.pending_operations.is_empty());
        assert_eq!(
            durable_completed.profiles[0].active_credential_generation,
            Some(active_generation)
        );
    }

    #[test]
    fn replace_switches_active_generation_before_delete_and_retries_cleanup_after_reopen() {
        let (profiles, repository, vault, closer) = harness();
        let created = profiles.create(postgres_request("old-secret")).unwrap();
        vault.fail_next(TestVaultOperation::Delete, VaultErrorKind::DeleteFailed);

        let error = profiles
            .update(ProfileUpdateRequest {
                descriptor_id: created.descriptor_id.clone(),
                name: "Production v2".to_string(),
                target: ProfileTarget::Postgres {
                    host: "db-v2.internal".to_string(),
                    port: 5432,
                    database: "app".to_string(),
                    user: "alice".to_string(),
                    ssl: true,
                    trust_cert: false,
                },
                replacement_credential: Some(CredentialInput {
                    password: SecretString::from("new-secret"),
                }),
            })
            .unwrap_err();
        assert_eq!(error.code, ProfileErrorCode::VaultDeleteFailed);
        let reopened = DatabaseProfiles::new(Arc::new(repository.reopen()), vault.clone(), closer);
        let pending = reopened.load().unwrap();
        assert_eq!(pending.profiles[0].name, "Production v2");
        assert_eq!(pending.recovery[0].kind, PendingOperationKind::CleanupOld);
        assert_eq!(vault.generation_count(&created.descriptor_id.0), 2);

        let completed = reopened
            .recover(ProfileRecoveryRequest {
                operation_id: pending.recovery[0].operation_id.clone(),
                action: RecoveryAction::RetryCleanup,
                credential: None,
            })
            .unwrap();
        assert!(completed.recovery.is_empty());
        assert_eq!(vault.generation_count(&created.descriptor_id.0), 1);
    }

    #[test]
    fn forget_final_replace_failure_reopens_pending_and_retry_removes_profile_and_ledger() {
        let (profiles, repository, vault, closer) = harness();
        let created = profiles.create(postgres_request("secret")).unwrap();
        let generation = repository.reopen().load().unwrap().profiles[0]
            .active_credential_generation
            .clone()
            .unwrap();
        repository.fail_nth_replace(2, RepositoryFailurePoint::Rename);

        assert_eq!(
            profiles.forget(&created.descriptor_id).unwrap_err().code,
            ProfileErrorCode::RepositoryUnavailable
        );

        let durable_pending = repository.reopen().load().unwrap();
        assert_eq!(durable_pending.profiles.len(), 1);
        match durable_pending.pending_operations.as_slice() {
            [PendingOperation::PendingForget {
                descriptor_id,
                generations,
                ..
            }] => {
                assert_eq!(descriptor_id, &created.descriptor_id);
                assert_eq!(generations, std::slice::from_ref(&generation));
            }
            operations => panic!("expected one pendingForget operation, got {operations:?}"),
        }
        assert_eq!(vault.generation_count(&created.descriptor_id.0), 0);
        assert_eq!(closer.call_count(&created.descriptor_id.0), 1);

        let reopened =
            DatabaseProfiles::new(Arc::new(repository.reopen()), vault.clone(), closer.clone());
        let pending = reopened.load().unwrap();
        assert_eq!(pending.profiles.len(), 1);
        assert_eq!(
            pending.recovery[0].kind,
            PendingOperationKind::PendingForget
        );
        let completed = reopened
            .recover(ProfileRecoveryRequest {
                operation_id: pending.recovery[0].operation_id.clone(),
                action: RecoveryAction::RetryCleanup,
                credential: None,
            })
            .unwrap();

        assert!(completed.profiles.is_empty());
        assert!(completed.recovery.is_empty());
        assert_eq!(closer.call_count(&created.descriptor_id.0), 2);
        assert_eq!(vault.generation_count(&created.descriptor_id.0), 0);
        let durable_completed = repository.reopen().load().unwrap();
        assert!(durable_completed.profiles.is_empty());
        assert!(durable_completed.pending_operations.is_empty());
    }

    #[test]
    fn forget_failure_retains_profile_and_retry_row_until_delete_succeeds() {
        let (profiles, repository, vault, closer) = harness();
        let created = profiles.create(postgres_request("secret")).unwrap();
        vault.fail_next(TestVaultOperation::Delete, VaultErrorKind::DeleteFailed);

        assert_eq!(
            profiles.forget(&created.descriptor_id).unwrap_err().code,
            ProfileErrorCode::VaultDeleteFailed
        );
        let reopened = DatabaseProfiles::new(Arc::new(repository.reopen()), vault.clone(), closer);
        let pending = reopened.load().unwrap();
        assert_eq!(pending.profiles.len(), 1);
        assert_eq!(
            pending.recovery[0].kind,
            PendingOperationKind::PendingForget
        );
        let completed = reopened
            .recover(ProfileRecoveryRequest {
                operation_id: pending.recovery[0].operation_id.clone(),
                action: RecoveryAction::RetryCleanup,
                credential: None,
            })
            .unwrap();
        assert!(completed.profiles.is_empty());
        assert!(completed.recovery.is_empty());
    }

    #[test]
    fn remove_credential_final_replace_failure_reopens_pending_and_retry_keeps_required_profile() {
        let (profiles, repository, vault, closer) = harness();
        let created = profiles.create(postgres_request("secret")).unwrap();
        let generation = repository.reopen().load().unwrap().profiles[0]
            .active_credential_generation
            .clone()
            .unwrap();
        repository.fail_nth_replace(2, RepositoryFailurePoint::Rename);

        assert_eq!(
            profiles
                .remove_credential(&created.descriptor_id)
                .unwrap_err()
                .code,
            ProfileErrorCode::RepositoryUnavailable
        );

        let durable_pending = repository.reopen().load().unwrap();
        assert_eq!(durable_pending.profiles.len(), 1);
        assert_eq!(
            durable_pending.profiles[0].active_credential_generation,
            Some(generation.clone())
        );
        match durable_pending.pending_operations.as_slice() {
            [PendingOperation::PendingRemoveCredential {
                descriptor_id,
                generations,
                ..
            }] => {
                assert_eq!(descriptor_id, &created.descriptor_id);
                assert_eq!(generations, &[generation]);
            }
            operations => {
                panic!("expected one pendingRemoveCredential operation, got {operations:?}")
            }
        }
        assert_eq!(vault.generation_count(&created.descriptor_id.0), 0);
        assert_eq!(closer.call_count(&created.descriptor_id.0), 1);

        let reopened =
            DatabaseProfiles::new(Arc::new(repository.reopen()), vault.clone(), closer.clone());
        let pending = reopened.load().unwrap();
        assert_eq!(
            pending.recovery[0].kind,
            PendingOperationKind::PendingRemoveCredential
        );
        let completed = reopened
            .recover(ProfileRecoveryRequest {
                operation_id: pending.recovery[0].operation_id.clone(),
                action: RecoveryAction::RetryCleanup,
                credential: None,
            })
            .unwrap();

        assert_eq!(completed.profiles.len(), 1);
        assert_eq!(
            completed.profiles[0].credential_state,
            CredentialState::Required
        );
        assert!(completed.recovery.is_empty());
        assert_eq!(closer.call_count(&created.descriptor_id.0), 2);
        assert_eq!(vault.generation_count(&created.descriptor_id.0), 0);
        let durable_completed = repository.reopen().load().unwrap();
        assert!(durable_completed.pending_operations.is_empty());
        assert_eq!(
            durable_completed.profiles[0].credential_state,
            CredentialState::Required
        );
        assert_eq!(
            durable_completed.profiles[0].active_credential_generation,
            None
        );
    }

    #[test]
    fn remove_credential_failure_retains_profile_then_marks_it_required_on_retry() {
        let (profiles, repository, vault, closer) = harness();
        let created = profiles.create(postgres_request("secret")).unwrap();
        vault.fail_next(TestVaultOperation::Delete, VaultErrorKind::DeleteFailed);

        assert_eq!(
            profiles
                .remove_credential(&created.descriptor_id)
                .unwrap_err()
                .code,
            ProfileErrorCode::VaultDeleteFailed
        );
        let reopened = DatabaseProfiles::new(Arc::new(repository.reopen()), vault, closer);
        let pending = reopened.load().unwrap();
        assert_eq!(
            pending.recovery[0].kind,
            PendingOperationKind::PendingRemoveCredential
        );
        let completed = reopened
            .recover(ProfileRecoveryRequest {
                operation_id: pending.recovery[0].operation_id.clone(),
                action: RecoveryAction::RetryCleanup,
                credential: None,
            })
            .unwrap();
        assert_eq!(
            completed.profiles[0].credential_state,
            CredentialState::Required
        );
        assert!(completed.recovery.is_empty());
    }

    #[test]
    fn lifecycle_close_failure_stops_before_vault_delete_and_persists_retry_row() {
        let (profiles, repository, vault, closer) = harness();
        let created = profiles.create(postgres_request("secret")).unwrap();
        closer.fail_next(LifecycleCloseErrorKind::CancelFailed);
        let before = vault.counts();

        assert_eq!(
            profiles.forget(&created.descriptor_id).unwrap_err().code,
            ProfileErrorCode::LifecycleCancelFailed
        );
        assert_eq!(vault.counts().2, before.2);
        let pending = DatabaseProfiles::new(Arc::new(repository.reopen()), vault, closer)
            .load()
            .unwrap();
        assert_eq!(
            pending.recovery[0].kind,
            PendingOperationKind::PendingForget
        );
    }

    #[test]
    fn production_closer_requests_lifecycle_termination_before_recovery_retry() {
        let repository = Arc::new(FakeProfileRepository::default());
        let vault = Arc::new(TestVault::default());
        let database_state = DbState::default();
        let runtime = ProfileRuntimeRegistry::default();
        let closer = Arc::new(RegisteredProfileCloser {
            database_state: database_state.clone(),
            runtime: runtime.clone(),
            result_sessions: ResultSessionState::default(),
        });
        let profiles = DatabaseProfiles::new(repository.clone(), vault.clone(), closer.clone());
        let created = profiles.create(postgres_request("secret")).unwrap();
        let connection = LiveConnection {
            descriptor_id: created.descriptor_id.clone(),
            connection_id: ConnectionId("connection-1".to_string()),
            connection_generation: ConnectionGeneration("generation-1".to_string()),
            engine: LiveDatabaseEngine::Sqlite,
        };
        runtime.insert(connection.clone()).unwrap();
        let identity = db_service::ConnectionIdentity {
            descriptor_id: connection.descriptor_id.clone(),
            connection_id: connection.connection_id.clone(),
            connection_generation: connection.connection_generation.clone(),
        };
        let actor = Arc::new(crate::db_connection_actor::ProductionConnectionActor::new(
            identity.clone(),
            DbHandle::Sqlite(Mutex::new(rusqlite::Connection::open_in_memory().unwrap())),
        ));
        db_service::register_actor(&database_state, actor.clone()).unwrap();
        let execution = actor
            .acquire_execution(
                db_service::QueryRunOwner {
                    descriptor_id: identity.descriptor_id.clone(),
                    connection_id: identity.connection_id.clone(),
                    connection_generation: identity.connection_generation.clone(),
                    query_run_id: db_service::QueryRunId("query-busy".to_string()),
                },
                crate::db_connection_actor::CancelCapability::SqliteInterrupt,
            )
            .unwrap();
        let deletes_before = vault.counts().2;

        assert_eq!(
            profiles.forget(&created.descriptor_id).unwrap_err().code,
            ProfileErrorCode::LifecycleCloseFailed
        );
        assert_eq!(vault.counts().2, deletes_before);
        assert!(runtime.get(&created.descriptor_id).is_some());
        assert_eq!(database_state.0.lock().unwrap().len(), 1);
        assert!(actor.is_terminating());
        assert_eq!(
            actor.acquire_metadata(),
            Err(crate::db_connection_actor::ActorError::Closed)
        );

        let state = DatabaseProfileState {
            profiles: Arc::new(Mutex::new(profiles)),
            runtime: runtime.clone(),
            database_state: database_state.clone(),
            result_sessions: ResultSessionState::default(),
            opener: Arc::new(ProductionDatabaseConnectionOpener),
        };
        let open_error =
            tauri::async_runtime::block_on(state.open_saved(&created.descriptor_id)).unwrap_err();
        assert_eq!(open_error.code, ProfileErrorCode::PendingOperationConflict);

        assert_eq!(
            actor.settle_execution(&execution).unwrap(),
            crate::db_connection_actor::Settlement {
                cancel_requested: true,
                release_requested: true,
                connection_termination_required: false,
            }
        );
        let pending = state.profiles.lock().unwrap().load().unwrap();
        let completed = state
            .profiles
            .lock()
            .unwrap()
            .recover(ProfileRecoveryRequest {
                operation_id: pending.recovery[0].operation_id.clone(),
                action: RecoveryAction::RetryCleanup,
                credential: None,
            })
            .unwrap();
        assert!(completed.profiles.is_empty());
        assert!(runtime.get(&created.descriptor_id).is_none());
        assert_eq!(vault.counts().2, deletes_before + 1);
    }

    #[test]
    fn actor_missing_close_removes_only_the_stale_runtime_identity() {
        let database_state = DbState::default();
        let runtime = ProfileRuntimeRegistry::default();
        let closer = RegisteredProfileCloser {
            database_state: database_state.clone(),
            runtime: runtime.clone(),
            result_sessions: ResultSessionState::default(),
        };
        let stale = runtime_connection("descriptor-a", "stale");
        runtime.insert(stale.clone()).unwrap();

        assert_eq!(
            closer.cancel_and_close(&stale.descriptor_id).unwrap(),
            LifecycleCloseEvidence::HandleClosedAndSettled
        );
        assert!(runtime.get(&stale.descriptor_id).is_none());

        let old_runtime = LiveConnection {
            descriptor_id: DescriptorId("descriptor-a".to_string()),
            connection_id: ConnectionId("connection-reused".to_string()),
            connection_generation: ConnectionGeneration("generation-1".to_string()),
            engine: LiveDatabaseEngine::Sqlite,
        };
        let new_identity = db_service::ConnectionIdentity {
            descriptor_id: old_runtime.descriptor_id.clone(),
            connection_id: old_runtime.connection_id.clone(),
            connection_generation: ConnectionGeneration("generation-2".to_string()),
        };
        let generation_two_actor =
            Arc::new(crate::db_connection_actor::ProductionConnectionActor::new(
                new_identity.clone(),
                DbHandle::Sqlite(Mutex::new(rusqlite::Connection::open_in_memory().unwrap())),
            ));
        db_service::register_actor(&database_state, generation_two_actor.clone()).unwrap();
        runtime.insert(old_runtime.clone()).unwrap();

        assert_eq!(
            closer.cancel_and_close(&old_runtime.descriptor_id).unwrap(),
            LifecycleCloseEvidence::HandleClosedAndSettled
        );
        assert!(runtime.get(&old_runtime.descriptor_id).is_none());
        assert_eq!(database_state.0.lock().unwrap().len(), 1);
        assert_eq!(generation_two_actor.identity(), &new_identity);
        assert!(!generation_two_actor.teardown_report().closed);
    }

    #[test]
    fn explicit_resume_handles_corrupt_or_denied_generations_without_startup_access() {
        for failure in [VaultErrorKind::Corrupt, VaultErrorKind::Denied] {
            let (profiles, _, vault, _) = harness();
            vault.fail_next(TestVaultOperation::Store, VaultErrorKind::WriteFailed);
            assert!(profiles.create(postgres_request("first")).is_err());
            let pending = profiles.load().unwrap();
            assert_eq!(vault.counts().1, 0, "startup load must not resolve vault");
            vault.fail_next(TestVaultOperation::Resolve, failure);

            let completed = profiles
                .recover(ProfileRecoveryRequest {
                    operation_id: pending.recovery[0].operation_id.clone(),
                    action: RecoveryAction::Resume,
                    credential: Some(CredentialInput {
                        password: SecretString::from("replacement"),
                    }),
                })
                .unwrap();
            assert!(completed.recovery.is_empty());
            assert_eq!(
                completed.profiles[0].credential_state,
                CredentialState::Stored
            );
        }
    }

    #[test]
    fn legacy_import_is_one_atomic_non_secret_merge_and_never_claims_a_vault_secret() {
        let (profiles, repository, vault, _) = harness();
        let imported = profiles
            .import_legacy(LegacyProfileImportRequest {
                profiles: vec![ProfileDescriptor {
                    descriptor_id: DescriptorId("legacy-1".to_string()),
                    config_generation: 1,
                    name: "Legacy".to_string(),
                    target: postgres_request("unused").target,
                    credential_state: CredentialState::Stored,
                }],
            })
            .unwrap();
        assert_eq!(
            imported.profiles[0].credential_state,
            CredentialState::Required
        );
        assert_eq!(vault.counts(), (0, 0, 0));
        let persisted = String::from_utf8(repository.durable_bytes()).unwrap();
        assert!(persisted.contains("legacy-1"));
        assert!(!persisted.contains(SENTINEL));
    }

    #[test]
    fn errors_debug_and_repository_bytes_never_contain_secret_sentinel() {
        let (profiles, repository, vault, _) = harness();
        vault.fail_next(TestVaultOperation::Store, VaultErrorKind::WriteFailed);
        let error = profiles.create(postgres_request(SENTINEL)).unwrap_err();
        assert!(!format!("{error:?}").contains(SENTINEL));
        assert!(!serde_json::to_string(&error).unwrap().contains(SENTINEL));
        assert!(!String::from_utf8_lossy(&repository.durable_bytes()).contains(SENTINEL));

        // The sentinel exists only inside the secret type when a write succeeds;
        // neither its Debug representation nor repository state exposes it.
        let secret = SecretString::from(SENTINEL);
        assert_eq!(secret.expose_secret(), SENTINEL);
        assert!(!format!("{secret:?}").contains(SENTINEL));
    }
}
