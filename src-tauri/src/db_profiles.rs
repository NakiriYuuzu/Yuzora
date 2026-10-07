//! Crash-consistent, non-secret database profile persistence and credential sagas.
//!
//! This module is deliberately the only persistence authority for saved database
//! descriptors. The file contains no credential material: vault mutations are
//! coordinated through a write-ahead `PendingOperation` ledger.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::future::Future;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::db_credentials::{
    CredentialGeneration, DatabaseCredentialStore, VaultError, VaultErrorKind,
};
use crate::db_result_session::ResultSessionState;
use crate::db_service::{
    self, ConnectionGeneration, ConnectionId, CredentialInput, CredentialState, DbHandle,
    DbOpenConfig, DbState, DescriptorId, LiveConnection, LiveDatabaseEngine,
    PostgresInsecureException, PostgresTransportMode, ProfileCreateRequest, ProfileDescriptor,
    ProfileTarget, ProfileUpdateRequest, TestConnectionRequest, TestConnectionResult,
};

const PROFILE_REPOSITORY_VERSION: u32 = 1;

fn parse_profile_document(bytes: &[u8]) -> Result<ProfileDocument, ProfileRepositoryError> {
    let mut value: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|_| ProfileRepositoryError::new(ProfileRepositoryErrorKind::Corrupt))?;
    migrate_legacy_postgres_targets(&mut value);
    serde_json::from_value(value)
        .map_err(|_| ProfileRepositoryError::new(ProfileRepositoryErrorKind::Corrupt))
}

fn migrate_legacy_postgres_targets(value: &mut serde_json::Value) {
    if let Some(profiles) = value
        .get_mut("profiles")
        .and_then(serde_json::Value::as_array_mut)
    {
        for profile in profiles {
            if let Some(target) = profile.get_mut("target") {
                migrate_legacy_postgres_target(target);
            }
        }
    }
    if let Some(operations) = value
        .get_mut("pendingOperations")
        .and_then(serde_json::Value::as_array_mut)
    {
        for operation in operations {
            if let Some(target) = operation
                .get_mut("profile")
                .and_then(|profile| profile.get_mut("target"))
            {
                migrate_legacy_postgres_target(target);
            }
            if let Some(target) = operation
                .get_mut("replacement")
                .and_then(|profile| profile.get_mut("target"))
            {
                migrate_legacy_postgres_target(target);
            }
        }
    }
}

fn migrate_legacy_postgres_target(target: &mut serde_json::Value) {
    let Some(object) = target.as_object_mut() else {
        return;
    };
    if object.get("kind").and_then(serde_json::Value::as_str) != Some("postgres") {
        return;
    }
    if object.contains_key("transportMode") {
        object.remove("ssl");
        object.remove("trustCert");
        return;
    }
    let ssl = object.remove("ssl").and_then(|value| value.as_bool());
    let trust_cert = object
        .remove("trustCert")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let (mode, acknowledged_trust) = match ssl {
        Some(false) => ("insecurePlaintext", false),
        Some(true) if trust_cert => ("encryptedTrustServerCert", true),
        _ => ("verifyFull", false),
    };
    object.insert(
        "transportMode".to_string(),
        serde_json::Value::String(mode.to_string()),
    );
    if acknowledged_trust {
        object.insert(
            "trustServerCertAcknowledged".to_string(),
            serde_json::Value::Bool(true),
        );
    }
}

const TRANSPORT_CHALLENGE_TTL: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PostgresTransportChallengeDto {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via_host: Option<String>,
    pub challenge_id: String,
    pub transport_mode: PostgresTransportMode,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub database: String,
    pub expires_at: u64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PostgresTransportChallengeRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via_host: Option<String>,
    pub transport_mode: PostgresTransportMode,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub database: String,
}

struct TransportChallenge {
    via_host: Option<String>,
    id: String,
    mode: PostgresTransportMode,
    host: String,
    port: u16,
    user: String,
    database: String,
    expires: Instant,
}

#[derive(Default)]
struct PostgresTransportChallengeRegistry {
    items: Mutex<HashMap<String, TransportChallenge>>,
}

impl PostgresTransportChallengeRegistry {
    fn issue(
        &self,
        request: &PostgresTransportChallengeRequest,
    ) -> Result<PostgresTransportChallengeDto, ProfileError> {
        if matches!(request.transport_mode, PostgresTransportMode::VerifyFull) {
            return Err(ProfileError::new(
                ProfileErrorCode::InvalidRequest,
                "verify-full transport does not require a challenge",
            ));
        }
        if request.host.trim().is_empty() || request.user.trim().is_empty() {
            return Err(ProfileError::new(
                ProfileErrorCode::InvalidRequest,
                "PostgreSQL transport challenge requires host and user",
            ));
        }
        self.sweep();
        let challenge = TransportChallenge {
            via_host: request.via_host.clone(),
            id: format!("pg-chal-{}", uuid::Uuid::new_v4()),
            mode: request.transport_mode,
            host: request.host.clone(),
            port: request.port,
            user: request.user.clone(),
            database: request.database.clone(),
            expires: Instant::now() + TRANSPORT_CHALLENGE_TTL,
        };
        let dto = PostgresTransportChallengeDto {
            via_host: challenge.via_host.clone(),
            challenge_id: challenge.id.clone(),
            transport_mode: challenge.mode,
            host: challenge.host.clone(),
            port: challenge.port,
            user: challenge.user.clone(),
            database: challenge.database.clone(),
            expires_at: now_ms().saturating_add(TRANSPORT_CHALLENGE_TTL.as_millis() as u64),
        };
        self.items
            .lock()
            .map_err(|_| {
                ProfileError::new(
                    ProfileErrorCode::RepositoryUnavailable,
                    "database profile storage is unavailable",
                )
            })?
            .insert(challenge.id.clone(), challenge);
        Ok(dto)
    }

    fn consume(&self, challenge_id: &str, target: &ProfileTarget) -> Result<(), ProfileError> {
        let mut items = self.items.lock().map_err(|_| {
            ProfileError::new(
                ProfileErrorCode::RepositoryUnavailable,
                "database profile storage is unavailable",
            )
        })?;
        let Some(challenge) = items.remove(challenge_id) else {
            return Err(ProfileError::new(
                ProfileErrorCode::PostgresTransportChallengeReplay,
                "PostgreSQL transport challenge was already used",
            ));
        };
        if Instant::now() >= challenge.expires {
            return Err(ProfileError::new(
                ProfileErrorCode::PostgresTransportChallengeExpired,
                "PostgreSQL transport challenge expired",
            ));
        }
        if !challenge_matches_target(&challenge, target) {
            return Err(ProfileError::new(
                ProfileErrorCode::PostgresTransportChallengeMismatch,
                "PostgreSQL transport challenge does not match the target",
            ));
        }
        Ok(())
    }

    fn sweep(&self) {
        if let Ok(mut items) = self.items.lock() {
            let now = Instant::now();
            items.retain(|_, challenge| now < challenge.expires);
        }
    }

    #[cfg(test)]
    fn expire_all(&self) {
        if let Ok(mut items) = self.items.lock() {
            let past = Instant::now() - Duration::from_secs(1);
            for challenge in items.values_mut() {
                challenge.expires = past;
            }
        }
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn challenge_matches_target(challenge: &TransportChallenge, target: &ProfileTarget) -> bool {
    match target {
        ProfileTarget::Postgres {
            via_host,
            host,
            port,
            user,
            database,
            transport_mode,
            ..
        } => {
            challenge.via_host == *via_host
                && challenge.mode == *transport_mode
                && challenge.host == *host
                && challenge.port == *port
                && challenge.user == *user
                && challenge.database == *database
        }
        ProfileTarget::Sqlite { .. } | ProfileTarget::Mssql { .. } => false,
    }
}

fn strip_postgres_attestation(target: ProfileTarget) -> ProfileTarget {
    match target {
        ProfileTarget::Postgres {
            via_host,
            host,
            port,
            database,
            user,
            transport_mode,
            ..
        } => ProfileTarget::Postgres {
            via_host,
            host,
            port,
            database,
            user,
            transport_mode,
            insecure_exception: None,
            trust_server_cert_acknowledged: false,
        },
        other => other,
    }
}

fn same_postgres_identity(left: &ProfileTarget, right: &ProfileTarget) -> bool {
    match (left, right) {
        (
            ProfileTarget::Postgres {
                via_host: via_a,
                host: host_a,
                port: port_a,
                user: user_a,
                database: database_a,
                transport_mode: mode_a,
                ..
            },
            ProfileTarget::Postgres {
                via_host: via_b,
                host: host_b,
                port: port_b,
                user: user_b,
                database: database_b,
                transport_mode: mode_b,
                ..
            },
        ) => {
            via_a == via_b
                && host_a == host_b
                && port_a == port_b
                && user_a == user_b
                && database_a == database_b
                && mode_a == mode_b
        }
        _ => false,
    }
}

fn apply_backend_postgres_authorization(target: ProfileTarget) -> ProfileTarget {
    match target {
        ProfileTarget::Postgres {
            via_host,
            host,
            port,
            database,
            user,
            transport_mode,
            ..
        } => match transport_mode {
            PostgresTransportMode::VerifyFull => ProfileTarget::Postgres {
                via_host,
                host,
                port,
                database,
                user,
                transport_mode,
                insecure_exception: None,
                trust_server_cert_acknowledged: false,
            },
            PostgresTransportMode::InsecurePlaintext => {
                let exception = PostgresInsecureException::new(
                    host.clone(),
                    port,
                    user.clone(),
                    database.clone(),
                );
                ProfileTarget::Postgres {
                    via_host,
                    host,
                    port,
                    database,
                    user,
                    transport_mode,
                    insecure_exception: Some(exception),
                    trust_server_cert_acknowledged: false,
                }
            }
            PostgresTransportMode::EncryptedTrustServerCert => ProfileTarget::Postgres {
                via_host,
                host,
                port,
                database,
                user,
                transport_mode,
                insecure_exception: None,
                trust_server_cert_acknowledged: true,
            },
        },
        other => other,
    }
}

fn authorize_postgres_target(target: &ProfileTarget) -> Result<(), ProfileError> {
    if target.postgres_transport_authorized() {
        Ok(())
    } else {
        Err(ProfileError::new(
            ProfileErrorCode::PostgresTransportRejected,
            "PostgreSQL transport requires an explicit acknowledged exception",
        ))
    }
}

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
        let directory = std::fs::OpenOptions::new()
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
        let document = parse_profile_document(&bytes)?;
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
    fn seed_durable_bytes(&self, bytes: Vec<u8>) {
        self.disk
            .lock()
            .expect("fake repository poisoned")
            .durable_bytes = Some(bytes);
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
        let document = parse_profile_document(bytes)?;
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
    PostgresTransportRejected,
    PostgresTransportChallengeExpired,
    PostgresTransportChallengeMismatch,
    PostgresTransportChallengeReplay,
}

/// IPC-safe domain error. Filesystem and keyring details are discarded. A
/// connection failure may carry an engine diagnostic only after the database
/// service has removed URL userinfo and the exact in-flight credential.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileError {
    pub code: ProfileErrorCode,
    pub message: &'static str,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<Box<db_service::DatabaseError>>,
}

impl ProfileError {
    fn new(code: ProfileErrorCode, message: &'static str) -> Self {
        Self {
            code,
            message,
            error: None,
        }
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
    challenges: PostgresTransportChallengeRegistry,
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
            challenges: PostgresTransportChallengeRegistry::default(),
        }
    }

    fn new_id(prefix: &str) -> String {
        format!("{prefix}-{}", uuid::Uuid::new_v4())
    }

    fn is_network(target: &ProfileTarget) -> bool {
        !matches!(target, ProfileTarget::Sqlite { .. })
    }

    pub fn issue_transport_challenge(
        &self,
        request: PostgresTransportChallengeRequest,
    ) -> Result<PostgresTransportChallengeDto, ProfileError> {
        self.challenges.issue(&request)
    }

    fn authorize_incoming_postgres_target(
        &self,
        target: ProfileTarget,
        challenge_id: Option<&str>,
        existing: Option<&ProfileTarget>,
    ) -> Result<ProfileTarget, ProfileError> {
        let stripped = strip_postgres_attestation(target);
        if stripped.postgres_transport_authorized() {
            return Ok(stripped);
        }
        if let Some(existing) = existing {
            if same_postgres_identity(existing, &stripped)
                && existing.postgres_transport_authorized()
            {
                return Ok(existing.clone());
            }
        }
        let Some(challenge_id) = challenge_id.filter(|id| !id.is_empty()) else {
            return Err(ProfileError::new(
                ProfileErrorCode::PostgresTransportRejected,
                "PostgreSQL transport requires an explicit acknowledged exception",
            ));
        };
        self.challenges.consume(challenge_id, &stripped)?;
        let authorized = apply_backend_postgres_authorization(stripped);
        authorize_postgres_target(&authorized)?;
        Ok(authorized)
    }

    #[cfg(test)]
    fn expire_transport_challenges(&self) {
        self.challenges.expire_all();
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
                target: strip_postgres_attestation(profile.target),
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
        let target = self.authorize_incoming_postgres_target(
            request.target,
            request.transport_challenge_id.as_deref(),
            None,
        )?;
        let mut document = self.repository.load()?;
        let descriptor_id = DescriptorId(Self::new_id("dbc"));
        let mut stored = StoredProfile {
            descriptor_id: descriptor_id.clone(),
            config_generation: 1,
            name: request.name,
            target,
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
        let target = self.authorize_incoming_postgres_target(
            request.target,
            request.transport_challenge_id.as_deref(),
            Some(&current.target),
        )?;
        if request.replacement_credential.is_none() {
            if Self::is_network(&current.target) != Self::is_network(&target) {
                return Err(ProfileError::new(
                    ProfileErrorCode::InvalidRequest,
                    "changing credential mode requires a new profile",
                ));
            }
            if current.target != target {
                Self::map_close(self.closer.cancel_and_close(&request.descriptor_id))?;
            }
            let mut updated = current;
            updated.name = request.name;
            updated.target = target;
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
        if !Self::is_network(&target) {
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
        replacement.target = target;
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
    fn open_with_identity(
        &self,
        config: DbOpenConfig,
        _identity: db_service::ConnectionIdentity,
    ) -> DatabaseOpenFuture<'_> {
        self.open(config)
    }
}

#[derive(Default)]
#[cfg(test)]
struct ProductionDatabaseConnectionOpener;

#[cfg(test)]
impl DatabaseConnectionOpener for ProductionDatabaseConnectionOpener {
    fn open(&self, config: DbOpenConfig) -> DatabaseOpenFuture<'_> {
        Box::pin(db_service::open_unregistered(config))
    }
}

struct HostDatabaseConnectionOpener {
    hosts: Arc<crate::host_service::HostManager>,
    ssh: Arc<crate::ssh_service::SshManager>,
}
impl DatabaseConnectionOpener for HostDatabaseConnectionOpener {
    fn open(&self, config: DbOpenConfig) -> DatabaseOpenFuture<'_> {
        self.open_with_identity(
            config,
            db_service::ConnectionIdentity {
                descriptor_id: DescriptorId(format!("probe-{}", uuid::Uuid::new_v4())),
                connection_id: ConnectionId(db_service::next_conn_id()),
                connection_generation: ConnectionGeneration(uuid::Uuid::new_v4().to_string()),
            },
        )
    }
    fn open_with_identity(
        &self,
        config: DbOpenConfig,
        identity: db_service::ConnectionIdentity,
    ) -> DatabaseOpenFuture<'_> {
        Box::pin(async move {
            if let DbOpenConfig::Sqlite { workspace, path } = config {
                return match workspace {
                    Some(workspace) => {
                        crate::host_sqlite::open(&self.hosts, &self.ssh, workspace, path, identity)
                            .await
                    }
                    None => crate::host_sqlite::open_local(path, identity).await,
                };
            }
            let Some((via, host, port)) = config.route() else {
                return db_service::open_unregistered(config).await;
            };
            let tunnel = crate::db_transport::DatabaseTunnel::open(
                &self.hosts,
                self.ssh.clone(),
                via,
                yuzora_host::tunnel::Endpoint {
                    host: host.into(),
                    port,
                },
            )
            .await
            .map_err(|_| db_service::DatabaseOperationalError::connection_failed())?;
            db_service::open_unregistered_via(config, Some(Box::new(tunnel))).await
        })
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
        hosts: Arc<crate::host_service::HostManager>,
        ssh: Arc<crate::ssh_service::SshManager>,
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
            opener: Arc::new(HostDatabaseConnectionOpener { hosts, ssh }),
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
            Code::PostgresTransportRejected => ProfileErrorCode::PostgresTransportRejected,
        };
        ProfileError {
            code,
            message: error.message,
            error: error.error,
        }
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
        authorize_postgres_target(&target)?;
        match target {
            ProfileTarget::Sqlite { path, workspace } => {
                Ok(DbOpenConfig::Sqlite { path, workspace })
            }
            ProfileTarget::Postgres {
                via_host,
                host,
                port,
                database,
                user,
                transport_mode,
                insecure_exception,
                trust_server_cert_acknowledged,
            } => {
                let password = Self::required_secret(secret)?;
                Ok(DbOpenConfig::Postgres {
                    via_host,
                    host,
                    port,
                    database,
                    user,
                    password,
                    transport_mode,
                    insecure_exception,
                    trust_server_cert_acknowledged,
                })
            }
            ProfileTarget::Mssql {
                via_host,
                host,
                port,
                database,
                user,
                trust_cert,
            } => {
                let password = Self::required_secret(secret)?;
                Ok(DbOpenConfig::Mssql {
                    via_host,
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
        let handle = match self
            .opener
            .open_with_identity(config, identity.clone())
            .await
        {
            Ok(handle) => handle,
            Err(open_error) => {
                let error = Self::connection_error(open_error);
                reservation_guard.fail(error.clone());
                return Err(error);
            }
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
            TestConnectionRequest::Ephemeral {
                target,
                credential,
                transport_challenge_id,
            } => {
                let target = self
                    .with_profiles(move |profiles| {
                        profiles.authorize_incoming_postgres_target(
                            target,
                            transport_challenge_id.as_deref(),
                            None,
                        )
                    })
                    .await?;
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
        let handle = self
            .opener
            .open(config)
            .await
            .map_err(Self::connection_error)?;
        let server_version = db_service::probe_unregistered(handle)
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

#[tauri::command]
pub async fn db_postgres_transport_challenge(
    state: tauri::State<'_, DatabaseProfileState>,
    request: PostgresTransportChallengeRequest,
) -> Result<PostgresTransportChallengeDto, ProfileError> {
    state
        .with_profiles(move |profiles| profiles.issue_transport_challenge(request))
        .await
}

#[cfg(test)]
mod tests;
