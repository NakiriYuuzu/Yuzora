//! Passwords and keys in the system's credential store (macOS Keychain,
//! Windows Credential Manager, Secret Service), never in files.
//!
//! Every Yuzora secret lives under one service, `dev.yuuzu.yuzora`. The
//! account is `v2:<len>:<namespace>:<len>:<id>`: the lengths keep a
//! separator inside an id from making two secrets share an entry, and `v2`
//! keeps clear of the Tauri app's (v0.1.0-beta.1 and older) entries.
//! A namespace is the lane's (`db`, `ssh`, …); the id is the caller's.

use std::collections::HashMap;
use std::sync::Mutex;

/// The credential store service every Yuzora secret is filed under.
pub const SERVICE: &str = "dev.yuuzu.yuzora";

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SecretKey {
    pub namespace: String,
    pub id: String,
}

impl SecretKey {
    pub fn new(namespace: impl Into<String>, id: impl Into<String>) -> Self {
        Self { namespace: namespace.into(), id: id.into() }
    }

    /// The store's account name for this key.
    pub fn account(&self) -> String {
        format!("v2:{}:{}:{}:{}", self.namespace.len(), self.namespace, self.id.len(), self.id)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SecretError {
    /// The person (or the OS) refused access.
    Denied(String),
    /// No credential store on this system.
    Unavailable(String),
    /// The stored bytes are not UTF-8.
    Corrupt,
    Failed(String),
}

impl std::fmt::Display for SecretError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SecretError::Denied(e) => write!(f, "credential store access denied: {e}"),
            SecretError::Unavailable(e) => write!(f, "no credential store: {e}"),
            SecretError::Corrupt => f.write_str("the stored secret is not text"),
            SecretError::Failed(e) => f.write_str(e),
        }
    }
}

impl std::error::Error for SecretError {}

/// Where secrets go. [`KeyringStore`] in the app, [`MemoryStore`] in tests.
pub trait SecretStore: Send + Sync {
    fn set(&self, key: &SecretKey, secret: &str) -> Result<(), SecretError>;
    /// `Ok(None)` when nothing is stored under the key.
    fn get(&self, key: &SecretKey) -> Result<Option<String>, SecretError>;
    /// Deleting a missing secret is not an error.
    fn delete(&self, key: &SecretKey) -> Result<(), SecretError>;
}

/// The system credential store (the `keyring` crate's native stores).
#[derive(Clone, Copy, Debug, Default)]
pub struct KeyringStore;

fn keyring_error(e: keyring::Error) -> SecretError {
    match e {
        keyring::Error::NoStorageAccess(why) => SecretError::Denied(why.to_string()),
        keyring::Error::NoDefaultStore => SecretError::Unavailable("no default store".into()),
        keyring::Error::NotSupportedByStore(why) => SecretError::Unavailable(why),
        keyring::Error::BadEncoding(_) => SecretError::Corrupt,
        other => SecretError::Failed(other.to_string()),
    }
}

impl KeyringStore {
    fn entry(key: &SecretKey) -> Result<keyring::Entry, SecretError> {
        keyring::Entry::new(SERVICE, &key.account()).map_err(keyring_error)
    }
}

impl SecretStore for KeyringStore {
    fn set(&self, key: &SecretKey, secret: &str) -> Result<(), SecretError> {
        Self::entry(key)?.set_password(secret).map_err(keyring_error)
    }

    fn get(&self, key: &SecretKey) -> Result<Option<String>, SecretError> {
        match Self::entry(key)?.get_password() {
            Ok(s) => Ok(Some(s)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(keyring_error(e)),
        }
    }

    fn delete(&self, key: &SecretKey) -> Result<(), SecretError> {
        match Self::entry(key)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(keyring_error(e)),
        }
    }
}

/// An in-process store: tests, and runs that must not touch the real one.
#[derive(Debug, Default)]
pub struct MemoryStore(Mutex<HashMap<String, String>>);

impl SecretStore for MemoryStore {
    fn set(&self, key: &SecretKey, secret: &str) -> Result<(), SecretError> {
        self.0.lock().unwrap().insert(key.account(), secret.to_owned());
        Ok(())
    }

    fn get(&self, key: &SecretKey) -> Result<Option<String>, SecretError> {
        Ok(self.0.lock().unwrap().get(&key.account()).cloned())
    }

    fn delete(&self, key: &SecretKey) -> Result<(), SecretError> {
        self.0.lock().unwrap().remove(&key.account());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accounts_are_length_prefixed_so_separators_cannot_collide() {
        assert_eq!(SecretKey::new("db", "prod").account(), "v2:2:db:4:prod");
        assert_ne!(SecretKey::new("db:a", "b").account(), SecretKey::new("db", "a:b").account());
    }

    #[test]
    fn memory_store_round_trips_and_deleting_twice_is_fine() {
        let store = MemoryStore::default();
        let key = SecretKey::new("ssh", "lab-01");
        assert_eq!(store.get(&key), Ok(None));
        store.set(&key, "hunter2").unwrap();
        assert_eq!(store.get(&key), Ok(Some("hunter2".into())));
        store.delete(&key).unwrap();
        store.delete(&key).unwrap();
        assert_eq!(store.get(&key), Ok(None));
    }
}
