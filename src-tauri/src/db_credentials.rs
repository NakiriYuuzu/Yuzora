use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::sync::Mutex;

use secrecy::{ExposeSecret, SecretString};
use zeroize::Zeroizing;

use crate::db_service::DescriptorId;

pub const DATABASE_CREDENTIAL_NAMESPACE: &str = "io.yuzora.database";

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq, Hash)]
#[serde(transparent)]
pub struct CredentialGeneration(pub String);

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub enum VaultErrorKind {
    Missing,
    Denied,
    Unavailable,
    Corrupt,
    WriteFailed,
    DeleteFailed,
}

#[derive(Clone, PartialEq, Eq)]
pub struct VaultError {
    kind: VaultErrorKind,
}

impl VaultError {
    pub const fn new(kind: VaultErrorKind) -> Self {
        Self { kind }
    }

    pub const fn kind(&self) -> VaultErrorKind {
        self.kind
    }
}

impl fmt::Display for VaultError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.kind {
            VaultErrorKind::Missing => "database credential is missing",
            VaultErrorKind::Denied => "database credential access was denied",
            VaultErrorKind::Unavailable => "database credential vault is unavailable",
            VaultErrorKind::Corrupt => "database credential is corrupt",
            VaultErrorKind::WriteFailed => "database credential could not be stored",
            VaultErrorKind::DeleteFailed => "database credential could not be deleted",
        })
    }
}

impl fmt::Debug for VaultError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("VaultError")
            .field(&self.kind)
            .finish()
    }
}

impl std::error::Error for VaultError {}

/// Rust-side secret custody boundary. Callers identify a credential only by
/// descriptor and generation; platform storage coordinates stay private.
pub trait DatabaseCredentialStore: Send + Sync {
    fn store(
        &self,
        descriptor_id: &DescriptorId,
        generation: &CredentialGeneration,
        secret: SecretString,
    ) -> Result<(), VaultError>;

    fn resolve(
        &self,
        descriptor_id: &DescriptorId,
        generation: &CredentialGeneration,
    ) -> Result<SecretString, VaultError>;

    fn delete(
        &self,
        descriptor_id: &DescriptorId,
        generation: &CredentialGeneration,
    ) -> Result<(), VaultError>;
}

pub struct KeyringCredentialStore {
    namespace: String,
}

impl KeyringCredentialStore {
    pub fn new(namespace: impl Into<String>) -> Self {
        Self {
            namespace: namespace.into(),
        }
    }

    fn entry_location(
        &self,
        descriptor_id: &DescriptorId,
        generation: &CredentialGeneration,
    ) -> KeyringEntryLocation {
        // Length-prefix every opaque component so caller-controlled separators
        // cannot make two descriptor/generation pairs address the same entry.
        let username = format!(
            "v1:{}:{}:{}:{}:{}:{}",
            self.namespace.len(),
            self.namespace,
            descriptor_id.0.len(),
            descriptor_id.0,
            generation.0.len(),
            generation.0,
        );
        KeyringEntryLocation {
            service: self.namespace.clone(),
            username,
        }
    }

    fn entry(
        &self,
        descriptor_id: &DescriptorId,
        generation: &CredentialGeneration,
        operation: VaultOperation,
    ) -> Result<keyring::Entry, VaultError> {
        let location = self.entry_location(descriptor_id, generation);
        keyring::Entry::new(&location.service, &location.username)
            .map_err(|error| map_keyring_error(error, operation))
    }
}

impl Default for KeyringCredentialStore {
    fn default() -> Self {
        Self::new(DATABASE_CREDENTIAL_NAMESPACE)
    }
}

impl DatabaseCredentialStore for KeyringCredentialStore {
    fn store(
        &self,
        descriptor_id: &DescriptorId,
        generation: &CredentialGeneration,
        secret: SecretString,
    ) -> Result<(), VaultError> {
        let entry = self.entry(descriptor_id, generation, VaultOperation::Store)?;
        entry
            .set_secret(secret.expose_secret().as_bytes())
            .map_err(|error| map_keyring_error(error, VaultOperation::Store))
    }

    fn resolve(
        &self,
        descriptor_id: &DescriptorId,
        generation: &CredentialGeneration,
    ) -> Result<SecretString, VaultError> {
        let entry = self.entry(descriptor_id, generation, VaultOperation::Resolve)?;
        let secret_bytes = Zeroizing::new(
            entry
                .get_secret()
                .map_err(|error| map_keyring_error(error, VaultOperation::Resolve))?,
        );
        let secret = std::str::from_utf8(secret_bytes.as_slice())
            .map_err(|_| VaultError::new(VaultErrorKind::Corrupt))?;
        Ok(SecretString::from(secret))
    }

    fn delete(
        &self,
        descriptor_id: &DescriptorId,
        generation: &CredentialGeneration,
    ) -> Result<(), VaultError> {
        let entry = self.entry(descriptor_id, generation, VaultOperation::Delete)?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(map_keyring_error(error, VaultOperation::Delete)),
        }
    }
}

struct KeyringEntryLocation {
    service: String,
    username: String,
}

#[derive(Clone, Copy)]
enum VaultOperation {
    Store,
    Resolve,
    Delete,
}

impl VaultOperation {
    const fn fallback_error_kind(self) -> VaultErrorKind {
        match self {
            Self::Store => VaultErrorKind::WriteFailed,
            Self::Resolve => VaultErrorKind::Unavailable,
            Self::Delete => VaultErrorKind::DeleteFailed,
        }
    }
}

fn map_keyring_error(error: keyring::Error, operation: VaultOperation) -> VaultError {
    let kind = match error {
        keyring::Error::NoEntry => VaultErrorKind::Missing,
        keyring::Error::NoStorageAccess(_) => VaultErrorKind::Denied,
        keyring::Error::NoDefaultStore => VaultErrorKind::Unavailable,
        keyring::Error::NotSupportedByStore(reason) => {
            drop(Zeroizing::new(reason));
            VaultErrorKind::Unavailable
        }
        keyring::Error::BadEncoding(bytes) => {
            drop(Zeroizing::new(bytes));
            VaultErrorKind::Corrupt
        }
        keyring::Error::BadDataFormat(bytes, _) => {
            drop(Zeroizing::new(bytes));
            VaultErrorKind::Corrupt
        }
        keyring::Error::BadStoreFormat(reason) => {
            drop(Zeroizing::new(reason));
            VaultErrorKind::Corrupt
        }
        keyring::Error::Ambiguous(_) => VaultErrorKind::Corrupt,
        keyring::Error::TooLong(attribute, _) => {
            drop(Zeroizing::new(attribute));
            operation.fallback_error_kind()
        }
        keyring::Error::Invalid(attribute, reason) => {
            drop(Zeroizing::new(attribute));
            drop(Zeroizing::new(reason));
            operation.fallback_error_kind()
        }
        keyring::Error::PlatformFailure(_) => operation.fallback_error_kind(),
        _ => operation.fallback_error_kind(),
    };
    VaultError::new(kind)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FakeCredentialOperation {
    Store,
    Resolve,
    Delete,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FakeCredentialOutcome {
    Succeeded,
    Failed(VaultErrorKind),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FakeCredentialEvent {
    pub operation: FakeCredentialOperation,
    pub descriptor_id: DescriptorId,
    pub generation: CredentialGeneration,
    pub outcome: FakeCredentialOutcome,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FakeCredentialAccessCounts {
    pub store: usize,
    pub resolve: usize,
    pub delete: usize,
}

pub struct FakeCredentialStore {
    state: Mutex<FakeCredentialState>,
}

impl FakeCredentialStore {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(FakeCredentialState::default()),
        }
    }

    pub fn fail_next(&self, operation: FakeCredentialOperation, kind: VaultErrorKind) {
        let mut state = self.state();
        state.failures.queue(operation).push_back(kind);
    }

    pub fn fail_next_store(&self, kind: VaultErrorKind) {
        self.fail_next(FakeCredentialOperation::Store, kind);
    }

    pub fn fail_next_resolve(&self, kind: VaultErrorKind) {
        self.fail_next(FakeCredentialOperation::Resolve, kind);
    }

    pub fn fail_next_delete(&self, kind: VaultErrorKind) {
        self.fail_next(FakeCredentialOperation::Delete, kind);
    }

    pub fn access_counts(&self) -> FakeCredentialAccessCounts {
        self.state().counts
    }

    pub fn events(&self) -> Vec<FakeCredentialEvent> {
        self.state().events.clone()
    }

    fn state(&self) -> std::sync::MutexGuard<'_, FakeCredentialState> {
        self.state
            .lock()
            .expect("fake credential store lock poisoned")
    }
}

impl Default for FakeCredentialStore {
    fn default() -> Self {
        Self::new()
    }
}

impl DatabaseCredentialStore for FakeCredentialStore {
    fn store(
        &self,
        descriptor_id: &DescriptorId,
        generation: &CredentialGeneration,
        secret: SecretString,
    ) -> Result<(), VaultError> {
        let mut state = self.state();
        state.counts.store += 1;
        if let Some(kind) = state.failures.take(FakeCredentialOperation::Store) {
            state.record(
                FakeCredentialOperation::Store,
                descriptor_id,
                generation,
                FakeCredentialOutcome::Failed(kind),
            );
            return Err(VaultError::new(kind));
        }

        state
            .secrets
            .insert(LogicalCredentialKey::new(descriptor_id, generation), secret);
        state.record(
            FakeCredentialOperation::Store,
            descriptor_id,
            generation,
            FakeCredentialOutcome::Succeeded,
        );
        Ok(())
    }

    fn resolve(
        &self,
        descriptor_id: &DescriptorId,
        generation: &CredentialGeneration,
    ) -> Result<SecretString, VaultError> {
        let mut state = self.state();
        state.counts.resolve += 1;
        if let Some(kind) = state.failures.take(FakeCredentialOperation::Resolve) {
            state.record(
                FakeCredentialOperation::Resolve,
                descriptor_id,
                generation,
                FakeCredentialOutcome::Failed(kind),
            );
            return Err(VaultError::new(kind));
        }

        let secret = state
            .secrets
            .get(&LogicalCredentialKey::new(descriptor_id, generation))
            .cloned();
        let outcome = if secret.is_some() {
            FakeCredentialOutcome::Succeeded
        } else {
            FakeCredentialOutcome::Failed(VaultErrorKind::Missing)
        };
        state.record(
            FakeCredentialOperation::Resolve,
            descriptor_id,
            generation,
            outcome,
        );
        secret.ok_or_else(|| VaultError::new(VaultErrorKind::Missing))
    }

    fn delete(
        &self,
        descriptor_id: &DescriptorId,
        generation: &CredentialGeneration,
    ) -> Result<(), VaultError> {
        let mut state = self.state();
        state.counts.delete += 1;
        if let Some(kind) = state.failures.take(FakeCredentialOperation::Delete) {
            state.record(
                FakeCredentialOperation::Delete,
                descriptor_id,
                generation,
                FakeCredentialOutcome::Failed(kind),
            );
            return Err(VaultError::new(kind));
        }

        state
            .secrets
            .remove(&LogicalCredentialKey::new(descriptor_id, generation));
        state.record(
            FakeCredentialOperation::Delete,
            descriptor_id,
            generation,
            FakeCredentialOutcome::Succeeded,
        );
        Ok(())
    }
}

#[derive(PartialEq, Eq, Hash)]
struct LogicalCredentialKey {
    descriptor_id: DescriptorId,
    generation: CredentialGeneration,
}

impl LogicalCredentialKey {
    fn new(descriptor_id: &DescriptorId, generation: &CredentialGeneration) -> Self {
        Self {
            descriptor_id: descriptor_id.clone(),
            generation: generation.clone(),
        }
    }
}

#[derive(Default)]
struct FakeCredentialState {
    secrets: HashMap<LogicalCredentialKey, SecretString>,
    failures: FakeCredentialFailures,
    counts: FakeCredentialAccessCounts,
    events: Vec<FakeCredentialEvent>,
}

impl FakeCredentialState {
    fn record(
        &mut self,
        operation: FakeCredentialOperation,
        descriptor_id: &DescriptorId,
        generation: &CredentialGeneration,
        outcome: FakeCredentialOutcome,
    ) {
        self.events.push(FakeCredentialEvent {
            operation,
            descriptor_id: descriptor_id.clone(),
            generation: generation.clone(),
            outcome,
        });
    }
}

#[derive(Default)]
struct FakeCredentialFailures {
    store: VecDeque<VaultErrorKind>,
    resolve: VecDeque<VaultErrorKind>,
    delete: VecDeque<VaultErrorKind>,
}

impl FakeCredentialFailures {
    fn queue(&mut self, operation: FakeCredentialOperation) -> &mut VecDeque<VaultErrorKind> {
        match operation {
            FakeCredentialOperation::Store => &mut self.store,
            FakeCredentialOperation::Resolve => &mut self.resolve,
            FakeCredentialOperation::Delete => &mut self.delete,
        }
    }

    fn take(&mut self, operation: FakeCredentialOperation) -> Option<VaultErrorKind> {
        self.queue(operation).pop_front()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeroize::ZeroizeOnDrop;

    const SENTINEL: &str = "YUZORA_CREDENTIAL_SENTINEL_DO_NOT_LEAK";

    fn descriptor(value: &str) -> DescriptorId {
        DescriptorId(value.to_string())
    }

    fn generation(value: &str) -> CredentialGeneration {
        CredentialGeneration(value.to_string())
    }

    fn assert_resolved_eq(
        store: &impl DatabaseCredentialStore,
        id: &DescriptorId,
        gen: &CredentialGeneration,
        expected: &str,
    ) {
        let secret = store.resolve(id, gen).unwrap();
        assert_eq!(secret.expose_secret(), expected);
    }

    #[test]
    fn credential_generation_is_opaque_and_serde_transparent() {
        let generation = generation("generation-7");
        assert_eq!(
            serde_json::to_value(&generation).unwrap(),
            serde_json::json!("generation-7")
        );
        assert_eq!(
            serde_json::from_value::<CredentialGeneration>(serde_json::json!("generation-7"))
                .unwrap(),
            generation
        );
    }

    #[test]
    fn namespaced_versioned_keys_are_deterministic_and_collision_safe_without_os_access() {
        let store = KeyringCredentialStore::new(DATABASE_CREDENTIAL_NAMESPACE);
        let first = store.entry_location(&descriptor("descriptor-a"), &generation("generation-1"));
        let repeated =
            store.entry_location(&descriptor("descriptor-a"), &generation("generation-1"));
        let next_generation =
            store.entry_location(&descriptor("descriptor-a"), &generation("generation-2"));

        assert_eq!(first.service, "io.yuzora.database");
        assert_eq!(
            first.username,
            "v1:18:io.yuzora.database:12:descriptor-a:12:generation-1"
        );
        assert_eq!(first.service, repeated.service);
        assert_eq!(first.username, repeated.username);
        assert_ne!(first.username, next_generation.username);

        let left = store.entry_location(&descriptor("a:b"), &generation("c"));
        let right = store.entry_location(&descriptor("a"), &generation("b:c"));
        assert_ne!(left.username, right.username);
    }

    #[test]
    fn fake_store_write_and_delete_are_idempotent_and_generation_scoped() {
        let store = FakeCredentialStore::new();
        let id = descriptor("descriptor-a");
        let first = generation("generation-1");
        let second = generation("generation-2");

        store
            .store(&id, &first, SecretString::from("alpha"))
            .unwrap();
        store
            .store(&id, &first, SecretString::from("alpha"))
            .unwrap();
        store
            .store(&id, &second, SecretString::from("beta"))
            .unwrap();
        assert_resolved_eq(&store, &id, &first, "alpha");
        assert_resolved_eq(&store, &id, &second, "beta");

        store.delete(&id, &first).unwrap();
        store.delete(&id, &first).unwrap();
        assert_eq!(
            store.resolve(&id, &first).unwrap_err().kind(),
            VaultErrorKind::Missing
        );
        assert_resolved_eq(&store, &id, &second, "beta");
    }

    #[test]
    fn fake_store_failure_injection_is_one_shot_and_preserves_state() {
        let store = FakeCredentialStore::new();
        let id = descriptor("descriptor-a");
        let generation = generation("generation-1");

        store.fail_next_store(VaultErrorKind::WriteFailed);
        assert_eq!(
            store
                .store(&id, &generation, SecretString::from("first"))
                .unwrap_err()
                .kind(),
            VaultErrorKind::WriteFailed
        );
        assert_eq!(
            store.resolve(&id, &generation).unwrap_err().kind(),
            VaultErrorKind::Missing
        );

        store
            .store(&id, &generation, SecretString::from("second"))
            .unwrap();
        store.fail_next_resolve(VaultErrorKind::Denied);
        assert_eq!(
            store.resolve(&id, &generation).unwrap_err().kind(),
            VaultErrorKind::Denied
        );
        assert_resolved_eq(&store, &id, &generation, "second");

        store.fail_next_delete(VaultErrorKind::DeleteFailed);
        assert_eq!(
            store.delete(&id, &generation).unwrap_err().kind(),
            VaultErrorKind::DeleteFailed
        );
        assert_resolved_eq(&store, &id, &generation, "second");
        store.delete(&id, &generation).unwrap();
    }

    #[test]
    fn fake_store_counters_and_events_contain_metadata_but_never_secrets() {
        let store = FakeCredentialStore::new();
        let id = descriptor("descriptor-a");
        let generation = generation("generation-1");

        store
            .store(&id, &generation, SecretString::from(SENTINEL))
            .unwrap();
        assert_resolved_eq(&store, &id, &generation, SENTINEL);
        store.fail_next_delete(VaultErrorKind::DeleteFailed);
        let error = store.delete(&id, &generation).unwrap_err();

        assert_eq!(
            store.access_counts(),
            FakeCredentialAccessCounts {
                store: 1,
                resolve: 1,
                delete: 1,
            }
        );
        let events = store.events();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].operation, FakeCredentialOperation::Store);
        assert_eq!(
            events[2].outcome,
            FakeCredentialOutcome::Failed(VaultErrorKind::DeleteFailed)
        );
        assert!(!format!("{events:?}").contains(SENTINEL));
        assert!(!format!("{error:?}").contains(SENTINEL));
        assert!(!error.to_string().contains(SENTINEL));
    }

    #[test]
    fn vault_errors_have_stable_redacted_kinds_display_and_debug() {
        let cases = [
            (
                VaultErrorKind::Missing,
                "database credential is missing",
                "VaultError(Missing)",
            ),
            (
                VaultErrorKind::Denied,
                "database credential access was denied",
                "VaultError(Denied)",
            ),
            (
                VaultErrorKind::Unavailable,
                "database credential vault is unavailable",
                "VaultError(Unavailable)",
            ),
            (
                VaultErrorKind::Corrupt,
                "database credential is corrupt",
                "VaultError(Corrupt)",
            ),
            (
                VaultErrorKind::WriteFailed,
                "database credential could not be stored",
                "VaultError(WriteFailed)",
            ),
            (
                VaultErrorKind::DeleteFailed,
                "database credential could not be deleted",
                "VaultError(DeleteFailed)",
            ),
        ];

        for (kind, display, debug) in cases {
            let error = VaultError::new(kind);
            assert_eq!(error.kind(), kind);
            assert_eq!(error.to_string(), display);
            assert_eq!(format!("{error:?}"), debug);
        }
    }

    #[test]
    fn keyring_errors_are_classified_without_retaining_underlying_details() {
        let missing = map_keyring_error(keyring::Error::NoEntry, VaultOperation::Resolve);
        let denied = map_keyring_error(
            keyring::Error::NoStorageAccess(Box::new(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                SENTINEL,
            ))),
            VaultOperation::Resolve,
        );
        let corrupt = map_keyring_error(
            keyring::Error::BadEncoding(SENTINEL.as_bytes().to_vec()),
            VaultOperation::Resolve,
        );
        let write_failed = map_keyring_error(
            keyring::Error::Invalid("credential".to_string(), SENTINEL.to_string()),
            VaultOperation::Store,
        );
        let delete_failed = map_keyring_error(
            keyring::Error::PlatformFailure(Box::new(std::io::Error::other(SENTINEL))),
            VaultOperation::Delete,
        );

        assert_eq!(missing.kind(), VaultErrorKind::Missing);
        assert_eq!(denied.kind(), VaultErrorKind::Denied);
        assert_eq!(corrupt.kind(), VaultErrorKind::Corrupt);
        assert_eq!(write_failed.kind(), VaultErrorKind::WriteFailed);
        assert_eq!(delete_failed.kind(), VaultErrorKind::DeleteFailed);
        for error in [missing, denied, corrupt, write_failed, delete_failed] {
            assert!(!format!("{error:?}").contains(SENTINEL));
            assert!(!error.to_string().contains(SENTINEL));
        }
    }

    #[test]
    fn secret_values_are_zeroize_compatible_and_debug_redacted() {
        fn assert_zeroize_on_drop<T: ZeroizeOnDrop>() {}

        assert_zeroize_on_drop::<SecretString>();
        assert_zeroize_on_drop::<Zeroizing<Vec<u8>>>();
        let secret = SecretString::from(SENTINEL);
        assert!(!format!("{secret:?}").contains(SENTINEL));
    }

    #[test]
    fn stores_and_trait_objects_are_send_and_sync_without_os_keyring_calls() {
        fn assert_send_sync<T: Send + Sync>() {}
        fn accepts_store(_: &dyn DatabaseCredentialStore) {}

        assert_send_sync::<FakeCredentialStore>();
        assert_send_sync::<KeyringCredentialStore>();
        let fake = FakeCredentialStore::new();
        let keyring = KeyringCredentialStore::default();
        accepts_store(&fake);
        accepts_store(&keyring);
    }
}
