//! Bringing data over from the Tauri app, Yuzora 0.1.0-beta.1 and older
//! (docs/beta2/release.md, "beta.1 → beta.2"). Added by L7.
//!
//! The old app's files and credentials are only ever read: they stay as they
//! were, so the Tauri app still works if someone goes back to it (SQLite
//! still notes its read lock in a WAL database's `-shm`; the data stays). Each lane
//! brings its own part with an [`Importer`], into its own section of Yuzora's
//! settings.json and its own secrets namespace: `db` (L3), `ssh` (L4),
//! `general` (L6). [`run`] runs each importer once and remembers it in
//! `<yuzora dir>/migrations.json`; it runs again only after it failed or
//! when its [`Importer::version`] goes up.
//!
//! What beta.1 left, and what [`Beta1`] reads of it:
//!
//! | What | Where | Read with |
//! | --- | --- | --- |
//! | DB connections | `<data>/database-profiles-v1.json` (`version: 1`) | [`Beta1::db_profiles`] |
//! | DB passwords | credential store, service `io.yuzora.database` | [`Beta1::db_password`] |
//! | SSH host keys | `~/.yuzora/known_hosts.json` | [`Beta1::known_hosts`] |
//! | SSH hosts, language, look, terminal, editor, keys | the WebView's localStorage | [`Beta1::local_storage`] |
//!
//! `<data>` is the Tauri app's data folder for `dev.yuuzu.yuzora`
//! (`~/Library/Application Support/…` on macOS, `%APPDATA%\…` on Windows,
//! `~/.local/share/…` on Linux). localStorage is readable on macOS, where
//! WebKit keeps it in SQLite (read with the system's `/usr/bin/sqlite3`,
//! read-only). On Windows it's WebView2's LevelDB, which this can't read, so
//! importers say what they couldn't bring ([`Imported::left`]). The beta.1
//! app never kept SSH passwords or passphrases.

use crate::secrets::{SecretError, SecretKey, SecretStore};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The record of what ran, in Yuzora's own folder (`<app home>/yuzora`).
pub const STATE_FILE: &str = "migrations.json";
/// The Tauri app's identifier: its data folders are named for it.
pub const TAURI_ID: &str = "dev.yuuzu.yuzora";
/// beta.1's DB password service (src-tauri/src/db_credentials.rs at v0.1.0-beta.1).
pub const DB_SERVICE: &str = "io.yuzora.database";
/// A file larger than this isn't something beta.1 wrote.
const MAX_FILE: u64 = 8 * 1024 * 1024;

/// The old app's credentials, by its own service and account names.
pub trait LegacySecrets: Send + Sync {
    /// `Ok(None)` when nothing is stored there.
    fn get(&self, service: &str, account: &str) -> Result<Option<String>, SecretError>;
}

/// The system credential store, read-only.
#[derive(Clone, Copy, Debug, Default)]
pub struct KeyringLegacy;

impl LegacySecrets for KeyringLegacy {
    fn get(&self, service: &str, account: &str) -> Result<Option<String>, SecretError> {
        let entry = keyring::Entry::new(service, account).map_err(|e| SecretError::Failed(e.to_string()))?;
        match entry.get_password() {
            Ok(s) => Ok(Some(s)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(keyring::Error::NoStorageAccess(why)) => Err(SecretError::Denied(why.to_string())),
            Err(keyring::Error::BadEncoding(_)) => Err(SecretError::Corrupt),
            Err(e) => Err(SecretError::Failed(e.to_string())),
        }
    }
}

/// Credentials in memory, for tests.
#[derive(Debug, Default)]
pub struct MemoryLegacy(pub Mutex<HashMap<(String, String), String>>);

impl MemoryLegacy {
    pub fn with(entries: &[(&str, &str, &str)]) -> Self {
        Self(Mutex::new(entries.iter().map(|(s, a, v)| (((*s).to_owned(), (*a).to_owned()), (*v).to_owned())).collect()))
    }
}

impl LegacySecrets for MemoryLegacy {
    fn get(&self, service: &str, account: &str) -> Result<Option<String>, SecretError> {
        Ok(self.0.lock().unwrap().get(&(service.to_owned(), account.to_owned())).cloned())
    }
}

/// A DB connection as beta.1 kept it. `target` is beta.1's own shape
/// (`{"kind": "sqlite" | "postgres" | "mssql", …}`); L3 maps it.
#[derive(Clone, Debug, PartialEq)]
pub struct DbProfile {
    pub id: String,
    pub name: String,
    pub target: Value,
    /// The stored password's generation, when beta.1 had one.
    pub credential: Option<String>,
}

/// beta.1's DB connections, with what it was still saving when it closed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DbProfiles {
    pub profiles: Vec<DbProfile>,
    /// A new connection, or new settings for one in `profiles`, that beta.1
    /// hadn't finished saving (it would have offered to finish or drop it).
    /// An importer brings or skips each one and says so ([`Imported::left`]).
    pub unfinished: Vec<DbProfile>,
}

/// What the Tauri app left on this machine.
pub struct Beta1 {
    /// Its data folder (`<data dir>/dev.yuuzu.yuzora`).
    pub data: PathBuf,
    /// `~/.yuzora`.
    pub dot: PathBuf,
    /// macOS: `~/Library/WebKit/dev.yuuzu.yuzora` (localStorage). None elsewhere.
    pub webkit: Option<PathBuf>,
    secrets: Box<dyn LegacySecrets>,
}

impl Beta1 {
    pub fn at(data: PathBuf, dot: PathBuf, webkit: Option<PathBuf>, secrets: Box<dyn LegacySecrets>) -> Self {
        Self { data, dot, webkit, secrets }
    }

    /// Where the Tauri app keeps its data here, if it left any.
    pub fn locate() -> Option<Beta1> {
        let home = crate::paths::home_dir();
        let var = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
        let data_root = if cfg!(target_os = "macos") {
            home.join("Library/Application Support")
        } else if cfg!(windows) {
            var("APPDATA").unwrap_or_else(|| home.join("AppData/Roaming"))
        } else {
            var("XDG_DATA_HOME").unwrap_or_else(|| home.join(".local/share"))
        };
        let webkit = cfg!(target_os = "macos").then(|| home.join("Library/WebKit").join(TAURI_ID));
        let found = Beta1::at(data_root.join(TAURI_ID), home.join(".yuzora"), webkit, Box::new(KeyringLegacy));
        found.left_anything().then_some(found)
    }

    /// Whether any of its folders is there: the WebKit one alone (a language
    /// picked, nothing else) counts too.
    pub fn left_anything(&self) -> bool {
        self.data.is_dir() || self.dot.is_dir() || self.webkit.as_deref().is_some_and(Path::is_dir)
    }

    /// A JSON file of beta.1's (`path` under [`Beta1::data`] or [`Beta1::dot`]).
    /// None when it isn't there.
    pub fn json(&self, path: &Path) -> Result<Option<Value>, String> {
        let meta = match std::fs::metadata(path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        if meta.len() > MAX_FILE {
            return Err(format!("{}: larger than beta.1 ever wrote", path.display()));
        }
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        serde_json::from_str(&text).map(Some).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// beta.1's DB connections (`database-profiles-v1.json`, version 1), as
    /// they end up once its journal (`pendingOperations`) is done: a delete
    /// or a password removal it hadn't finished is applied here, as beta.1
    /// would have on its next start; a save it hadn't finished is
    /// [`DbProfiles::unfinished`].
    pub fn db_profiles(&self) -> Result<DbProfiles, String> {
        let path = self.data.join("database-profiles-v1.json");
        let Some(doc) = self.json(&path)? else { return Ok(DbProfiles::default()) };
        if doc["version"].as_u64() != Some(1) {
            return Err(format!("{}: version {} (expected 1)", path.display(), doc["version"]));
        }
        let profile = |p: &Value, generation: &Value| {
            let text = |k: &str| p[k].as_str().map(str::to_owned).ok_or_else(|| format!("{}: a profile without {k}", path.display()));
            let stored = p["credentialState"].as_str() == Some("stored");
            Ok::<_, String>(DbProfile { id: text("descriptorId")?, name: text("name")?, target: p["target"].clone(), credential: generation.as_str().filter(|_| stored).map(str::to_owned) })
        };
        let list = doc["profiles"].as_array().ok_or_else(|| format!("{}: no profiles", path.display()))?;
        let mut found = DbProfiles { profiles: list.iter().map(|p| profile(p, &p["activeCredentialGeneration"])).collect::<Result<_, _>>()?, unfinished: Vec::new() };
        for operation in doc["pendingOperations"].as_array().into_iter().flatten() {
            let of = |p: &DbProfile| operation["descriptorId"].as_str() == Some(p.id.as_str());
            match operation["kind"].as_str() {
                Some("pendingCreate") => found.unfinished.push(profile(&operation["profile"], &operation["credentialGeneration"])?),
                Some("pendingReplace") => found.unfinished.push(profile(&operation["replacement"], &operation["newGeneration"])?),
                Some("pendingForget") => found.profiles.retain(|p| !of(p)),
                Some("pendingRemoveCredential") => found.profiles.iter_mut().filter(|p| of(p)).for_each(|p| p.credential = None),
                // cleanupOld: an old password is going; the profile already has its new one.
                Some("cleanupOld") => {}
                other => return Err(format!("{}: a pending operation beta.1 doesn't write ({other:?})", path.display())),
            }
        }
        Ok(found)
    }

    /// The password beta.1 stored for `profile`, if any.
    pub fn db_password(&self, profile: &DbProfile) -> Result<Option<String>, SecretError> {
        let Some(generation) = &profile.credential else { return Ok(None) };
        self.secrets.get(DB_SERVICE, &db_account(&profile.id, generation))
    }

    /// `~/.yuzora/known_hosts.json`: `"host:port"` → `"SHA256:…"`.
    pub fn known_hosts(&self) -> Result<BTreeMap<String, String>, String> {
        let path = self.dot.join("known_hosts.json");
        let Some(doc) = self.json(&path)? else { return Ok(BTreeMap::new()) };
        let map = doc.as_object().ok_or_else(|| format!("{}: not an object", path.display()))?;
        map.iter().map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_owned())).ok_or_else(|| format!("{}: {k} is not a fingerprint", path.display()))).collect()
    }

    /// One localStorage item of the Tauri app's page (`yuzora.hosts.ssh.v2`,
    /// `yuzora:language`, …). Err on systems where it can't be read.
    pub fn local_storage(&self, key: &str) -> Result<Option<String>, String> {
        match &self.webkit {
            Some(webkit) => webkit_local_storage(webkit, key),
            None => Err("the Tauri app's localStorage can't be read on this system".into()),
        }
    }
}

/// beta.1's DB password account: length-prefixed so separators can't collide.
pub fn db_account(descriptor: &str, generation: &str) -> String {
    format!("v1:{}:{DB_SERVICE}:{}:{descriptor}:{}:{generation}", DB_SERVICE.len(), descriptor.len(), generation.len())
}

/// WebKit's localStorage: `WebsiteData/Default/<hash>/<hash>/LocalStorage/
/// localstorage.sqlite3`, one per page origin. Only the app's own page
/// counts: beta.1's preview pane opened web pages in the same data store, so
/// any site could have left a `yuzora.hosts.ssh.v2` of its own.
fn webkit_local_storage(webkit: &Path, key: &str) -> Result<Option<String>, String> {
    // Only "not there" reads as nothing; anything else (a permission, say)
    // is an error, so the import runs again rather than counting as done.
    let missing = |e: &std::io::Error| e.kind() == std::io::ErrorKind::NotFound;
    let fail = |path: &Path, e: std::io::Error| format!("{}: {e}", path.display());
    let dirs = |dir: &Path| -> Result<Vec<PathBuf>, String> {
        let mut found = Vec::new();
        for entry in std::fs::read_dir(dir).map_err(|e| fail(dir, e))? {
            let entry = entry.map_err(|e| fail(dir, e))?;
            // Files beside the origin folders (WebKit's salt) aren't stores.
            if entry.file_type().map_err(|e| fail(&entry.path(), e))?.is_dir() {
                found.push(entry.path());
            }
        }
        Ok(found)
    };
    let mut stores: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    let default = webkit.join("WebsiteData/Default");
    match std::fs::metadata(&default) {
        Err(e) if missing(&e) => return Ok(None),
        Err(e) => return Err(fail(&default, e)),
        Ok(_) => {}
    }
    for top in dirs(&default)? {
        for frame in dirs(&top)? {
            let origin = frame.join("origin");
            match std::fs::read(&origin) {
                Ok(o) if is_app_origin(&o) => {}
                Ok(_) => continue,
                Err(e) if missing(&e) => continue,
                Err(e) => return Err(fail(&origin, e)),
            }
            let db = frame.join("LocalStorage/localstorage.sqlite3");
            match std::fs::metadata(&db) {
                Ok(meta) => stores.push((meta.modified().unwrap_or(std::time::UNIX_EPOCH), db)),
                Err(e) if missing(&e) => {}
                Err(e) => return Err(fail(&db, e)),
            }
        }
    }
    stores.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, db) in stores {
        if let Some(value) = sqlite_item(&db, key)? {
            return Ok(Some(value));
        }
    }
    Ok(None)
}

/// Whether WebKit's `origin` file says `tauri://localhost` for both the top
/// and the frame origin. Each origin is its protocol and host (a u32 length,
/// 1 for 8-bit characters, the characters) and then 0 for no port; anything
/// else, an iframe in the app included, isn't the app's page.
fn is_app_origin(file: &[u8]) -> bool {
    let mut one = Vec::new();
    for part in ["tauri", "localhost"] {
        one.extend((part.len() as u32).to_le_bytes());
        one.push(1);
        one.extend(part.as_bytes());
    }
    one.push(0);
    file.len() == 2 * one.len() && file.chunks(one.len()).all(|origin| origin == one)
}

/// `ItemTable.value` for `key`: WebKit keeps it as UTF-16LE.
fn sqlite_item(db: &Path, key: &str) -> Result<Option<String>, String> {
    let sql = format!("SELECT hex(value) FROM ItemTable WHERE key = '{}';", key.replace('\'', "''"));
    let out = std::process::Command::new("/usr/bin/sqlite3")
        .arg("-readonly")
        .arg(db)
        .arg(sql)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("sqlite3: {e}"))?;
    if !out.status.success() {
        return Err(format!("{}: {}", db.display(), String::from_utf8_lossy(&out.stderr).trim()));
    }
    let hex = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    if hex.is_empty() {
        return Ok(None);
    }
    let bytes = (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(hex.get(i..i + 2).unwrap_or("zz"), 16)).collect::<Result<Vec<u8>, _>>().map_err(|_| format!("{}: not hex", db.display()))?;
    if bytes.len() % 2 != 0 {
        return Err(format!("{}: {key} is not UTF-16", db.display()));
    }
    let units: Vec<u16> = bytes.chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    String::from_utf16(&units).map(Some).map_err(|_| format!("{}: {key} is not UTF-16", db.display()))
}

/// What an importer brought, and what it couldn't (said for the person).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Imported {
    pub items: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub left: Vec<String>,
}

/// The section an importer fills, and the secrets namespace of the same name.
pub struct Section<'a> {
    name: String,
    value: Value,
    changed: bool,
    secrets: &'a dyn SecretStore,
}

impl Section<'_> {
    pub fn name(&self) -> &str {
        &self.name
    }

    /// What the section holds now (`null` when empty): merge, don't overwrite
    /// what the person already set up in beta.2.
    pub fn value(&self) -> &Value {
        &self.value
    }

    pub fn set(&mut self, value: Value) {
        self.changed |= value != self.value;
        self.value = value;
    }

    /// Stores a secret under this section's namespace (`yuzora_core::secrets`).
    pub fn set_secret(&self, id: &str, secret: &str) -> Result<(), SecretError> {
        self.secrets.set(&SecretKey::new(self.name.clone(), id), secret)
    }
}

/// One lane's part of the move.
pub trait Importer: Send + Sync {
    /// Its section of Yuzora's settings.json, also its secrets namespace.
    fn section(&self) -> &str;
    /// Raise it when the importer brings more: it runs again once.
    fn version(&self) -> u32 {
        1
    }
    fn import(&self, from: &Beta1, into: &mut Section<'_>) -> Result<Imported, String>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// Ran now.
    Imported(Imported),
    /// Ran before at this version.
    Done,
    /// No Tauri app data on this machine.
    NoBeta1,
    /// Will run again next time.
    Failed(String),
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Record {
    version: u32,
    at: u64,
    #[serde(flatten)]
    imported: Imported,
}

/// Runs each importer that hasn't run at its version yet. `yuzora_dir` holds
/// settings.json and the record; `secrets` is where imported secrets go.
pub fn run(importers: &[&dyn Importer], from: Option<&Beta1>, yuzora_dir: &Path, secrets: &dyn SecretStore) -> Vec<(String, Status)> {
    let state_file = yuzora_dir.join(STATE_FILE);
    let settings_file = yuzora_dir.join("settings.json");
    let mut state = crate::settings::load_from(&state_file);
    let mut done: Map<String, Value> = state.remove("beta1").and_then(|v| v.as_object().cloned()).unwrap_or_default();
    let mut report = Vec::new();
    for importer in importers {
        let name = importer.section().to_owned();
        let Some(from) = from else {
            report.push((name, Status::NoBeta1));
            continue;
        };
        let ran = done.get(&name).and_then(|r| serde_json::from_value::<Record>(r.clone()).ok());
        if ran.is_some_and(|r| r.version >= importer.version()) {
            report.push((name, Status::Done));
            continue;
        }
        let mut section = Section { name: name.clone(), value: crate::settings::section_from(&settings_file, &name), changed: false, secrets };
        let status = match importer.import(from, &mut section) {
            Ok(imported) => {
                let written = if section.changed { crate::settings::set_section_at(&settings_file, &name, section.value).map_err(|e| e.to_string()) } else { Ok(()) };
                match written {
                    Ok(()) => {
                        let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
                        let record = Record { version: importer.version(), at, imported: imported.clone() };
                        done.insert(name.clone(), serde_json::to_value(record).unwrap_or(Value::Null));
                        Status::Imported(imported)
                    }
                    Err(e) => Status::Failed(format!("settings.json: {e}")),
                }
            }
            Err(e) => Status::Failed(e),
        };
        report.push((name, status));
    }
    if from.is_some() {
        state.insert("beta1".into(), Value::Object(done));
        if let Err(e) = write_state(&state_file, &state) {
            report.push((STATE_FILE.into(), Status::Failed(e.to_string())));
        }
    }
    report
}

fn write_state(file: &Path, state: &Map<String, Value>) -> std::io::Result<()> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = file.with_extension(format!("json.tmp-{}", std::process::id()));
    std::fs::write(&tmp, serde_json::to_string_pretty(state).unwrap_or_default() + "\n")?;
    std::fs::rename(&tmp, file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::MemoryStore;
    use serde_json::json;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("yuzora-migrate-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// beta.1's files as the Tauri app wrote them (src-tauri/src/db_profiles.rs at v0.1.0-beta.1).
    fn beta1(root: &Path, secrets: MemoryLegacy) -> Beta1 {
        let data = root.join("data");
        let dot = root.join("dot");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir_all(&dot).unwrap();
        let profiles = json!({
            "version": 1,
            "profiles": [
                {"descriptorId": "dbc-1", "configGeneration": 1, "name": "local", "target": {"kind": "sqlite", "path": "/tmp/a.db"}, "credentialState": "notRequired", "activeCredentialGeneration": null},
                {"descriptorId": "dbc-2", "configGeneration": 3, "name": "prod", "target": {"kind": "postgres", "host": "db", "port": 5432, "database": "app", "user": "me", "transportMode": "require"}, "credentialState": "stored", "activeCredentialGeneration": "credential-9"},
            ],
            "pendingOperations": [],
        });
        std::fs::write(data.join("database-profiles-v1.json"), profiles.to_string()).unwrap();
        std::fs::write(dot.join("known_hosts.json"), json!({"lab-01:22": "SHA256:abc"}).to_string()).unwrap();
        Beta1::at(data, dot, None, Box::new(secrets))
    }

    #[test]
    fn reads_what_beta1_left() {
        let root = temp("read");
        let account = db_account("dbc-2", "credential-9");
        assert_eq!(account, "v1:18:io.yuzora.database:5:dbc-2:12:credential-9");
        let old = beta1(&root, MemoryLegacy::with(&[(DB_SERVICE, &account, "s3cret")]));
        let DbProfiles { profiles, unfinished } = old.db_profiles().unwrap();
        assert_eq!(profiles.len(), 2);
        assert!(unfinished.is_empty());
        assert_eq!((profiles[1].name.as_str(), profiles[1].target["kind"].as_str()), ("prod", Some("postgres")));
        assert_eq!(old.db_password(&profiles[0]), Ok(None));
        assert_eq!(old.db_password(&profiles[1]), Ok(Some("s3cret".into())));
        assert_eq!(old.known_hosts().unwrap().get("lab-01:22").map(String::as_str), Some("SHA256:abc"));
        assert!(old.local_storage("yuzora:language").is_err(), "no localStorage reader without WebKit");
        // Missing files read as nothing; a newer format is refused.
        std::fs::remove_file(old.dot.join("known_hosts.json")).unwrap();
        assert!(old.known_hosts().unwrap().is_empty());
        std::fs::write(old.data.join("database-profiles-v1.json"), r#"{"version": 2, "profiles": []}"#).unwrap();
        assert!(old.db_profiles().unwrap_err().contains("version 2"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn db_profiles_end_as_beta1_would_have_finished_them() {
        let root = temp("journal");
        let old = beta1(&root, MemoryLegacy::default());
        let stored = |id: &str, name: &str, generation: Option<&str>| json!({"descriptorId": id, "configGeneration": 1, "name": name, "target": {"kind": "postgres", "host": "db"}, "credentialState": if generation.is_some() { "stored" } else { "required" }, "activeCredentialGeneration": generation});
        let doc = json!({
            "version": 1,
            "profiles": [stored("dbc-1", "kept", Some("credential-1")), stored("dbc-2", "password going", Some("credential-2")), stored("dbc-3", "being deleted", Some("credential-3"))],
            "pendingOperations": [
                // Written before the password is stored: no active generation yet.
                {"kind": "pendingCreate", "operationId": "profile-op-1", "profile": {"descriptorId": "dbc-4", "configGeneration": 1, "name": "new", "target": {"kind": "postgres", "host": "db"}, "credentialState": "stored", "activeCredentialGeneration": null}, "credentialGeneration": "credential-4"},
                {"kind": "pendingReplace", "operationId": "profile-op-2", "descriptorId": "dbc-1", "replacement": stored("dbc-1", "kept, renamed", Some("credential-5")), "oldGeneration": "credential-1", "newGeneration": "credential-5"},
                {"kind": "pendingRemoveCredential", "operationId": "profile-op-3", "descriptorId": "dbc-2", "generations": ["credential-2"]},
                {"kind": "pendingForget", "operationId": "profile-op-4", "descriptorId": "dbc-3", "generations": ["credential-3"]},
                {"kind": "cleanupOld", "operationId": "profile-op-5", "descriptorId": "dbc-1", "oldGeneration": "credential-0", "activeGeneration": "credential-1"},
            ],
        });
        let file = old.data.join("database-profiles-v1.json");
        std::fs::write(&file, doc.to_string()).unwrap();
        let found = old.db_profiles().unwrap();
        let summary = |list: &[DbProfile]| list.iter().map(|p| (p.id.clone(), p.name.clone(), p.credential.clone())).collect::<Vec<_>>();
        let some = |s: &str| Some(s.to_owned());
        assert_eq!(summary(&found.profiles), vec![("dbc-1".into(), "kept".into(), some("credential-1")), ("dbc-2".into(), "password going".into(), None)]);
        assert_eq!(summary(&found.unfinished), vec![("dbc-4".into(), "new".into(), some("credential-4")), ("dbc-1".into(), "kept, renamed".into(), some("credential-5"))]);
        let mut odd = doc.clone();
        odd["pendingOperations"] = json!([{"kind": "pendingSomethingElse"}]);
        std::fs::write(&file, odd.to_string()).unwrap();
        assert!(old.db_profiles().unwrap_err().contains("pendingSomethingElse"));
        std::fs::remove_dir_all(root).unwrap();
    }

    struct Db;
    impl Importer for Db {
        fn section(&self) -> &str {
            "db"
        }
        fn import(&self, from: &Beta1, into: &mut Section<'_>) -> Result<Imported, String> {
            let mut connections = into.value().get("connections").and_then(Value::as_array).cloned().unwrap_or_default();
            let mut imported = Imported::default();
            for p in from.db_profiles()?.profiles {
                if connections.iter().any(|c| c["from"] == p.id.as_str()) {
                    continue;
                }
                if let Some(password) = from.db_password(&p).map_err(|e| e.to_string())? {
                    into.set_secret(&p.id, &password).map_err(|e| e.to_string())?;
                }
                connections.push(json!({"name": p.name, "from": p.id}));
                imported.items += 1;
            }
            into.set(json!({"connections": connections}));
            Ok(imported)
        }
    }

    struct Failing;
    impl Importer for Failing {
        fn section(&self) -> &str {
            "ssh"
        }
        fn import(&self, from: &Beta1, _: &mut Section<'_>) -> Result<Imported, String> {
            from.local_storage("yuzora.hosts.ssh.v2").map(|_| Imported::default())
        }
    }

    #[test]
    fn runs_each_importer_once_and_leaves_beta1_as_it_was() {
        let root = temp("run");
        let account = db_account("dbc-2", "credential-9");
        let old = beta1(&root, MemoryLegacy::with(&[(DB_SERVICE, &account, "s3cret")]));
        let before = std::fs::read(old.data.join("database-profiles-v1.json")).unwrap();
        let home = root.join("yuzora");
        let store = MemoryStore::default();
        crate::settings::set_section_at(&home.join("settings.json"), "general", json!({"locale": "en"})).unwrap();

        let report = run(&[&Db, &Failing], Some(&old), &home, &store);
        assert_eq!(report[0], ("db".into(), Status::Imported(Imported { items: 2, left: vec![] })));
        assert!(matches!(&report[1], (name, Status::Failed(e)) if name == "ssh" && e.contains("can't be read")));
        let settings = crate::settings::load_from(&home.join("settings.json"));
        assert_eq!(settings["db"]["connections"].as_array().unwrap().len(), 2);
        assert_eq!(settings["general"]["locale"], "en", "other sections stay");
        assert_eq!(store.get(&SecretKey::new("db", "dbc-2")).unwrap().as_deref(), Some("s3cret"));
        assert_eq!(std::fs::read(old.data.join("database-profiles-v1.json")).unwrap(), before);

        // Done once; the failed one tries again.
        let report = run(&[&Db, &Failing], Some(&old), &home, &store);
        assert_eq!(report[0].1, Status::Done);
        assert!(matches!(report[1].1, Status::Failed(_)));
        let state = crate::settings::load_from(&home.join(STATE_FILE));
        assert_eq!(state["beta1"]["db"]["version"], 1);
        assert_eq!(state["beta1"]["db"]["items"], 2);
        assert!(state["beta1"].get("ssh").is_none());

        // A newer importer runs again and merges with what's there.
        struct DbV2;
        impl Importer for DbV2 {
            fn section(&self) -> &str {
                "db"
            }
            fn version(&self) -> u32 {
                2
            }
            fn import(&self, from: &Beta1, into: &mut Section<'_>) -> Result<Imported, String> {
                Db.import(from, into)
            }
        }
        let report = run(&[&DbV2], Some(&old), &home, &store);
        assert_eq!(report[0].1, Status::Imported(Imported { items: 0, left: vec![] }));
        assert_eq!(crate::settings::load_from(&home.join("settings.json"))["db"]["connections"].as_array().unwrap().len(), 2);

        // No beta.1 here: nothing runs, nothing is recorded.
        let fresh = root.join("fresh");
        assert_eq!(run(&[&Db], None, &fresh, &store), vec![("db".into(), Status::NoBeta1)]);
        assert!(!fresh.join(STATE_FILE).exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn webkit_data_alone_is_a_beta1_install() {
        let root = temp("left");
        let at = |webkit: Option<PathBuf>| Beta1::at(root.join("data"), root.join("dot"), webkit, Box::new(MemoryLegacy::default()));
        assert!(!at(None).left_anything());
        assert!(!at(Some(root.join("WebKit"))).left_anything());
        std::fs::create_dir_all(root.join("WebKit")).unwrap();
        assert!(at(Some(root.join("WebKit"))).left_anything());
        std::fs::create_dir_all(root.join("dot")).unwrap();
        assert!(at(None).left_anything());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_webkit_store_is_an_error_not_nothing() {
        use std::os::unix::fs::PermissionsExt;
        let root = temp("unreadable");
        let webkit = root.join("WebKit");
        let old = Beta1::at(root.join("data"), root.join("dot"), Some(webkit.clone()), Box::new(MemoryLegacy::default()));
        assert_eq!(old.local_storage("yuzora:language"), Ok(None), "no WebKit data at all");
        let default = webkit.join("WebsiteData/Default");
        std::fs::create_dir_all(default.join("a/a")).unwrap();
        std::fs::write(default.join("salt"), "x").unwrap();
        assert_eq!(old.local_storage("yuzora:language"), Ok(None), "a frame without an origin, and a file");
        std::fs::set_permissions(&default, std::fs::Permissions::from_mode(0o000)).unwrap();
        let read = old.local_storage("yuzora:language");
        std::fs::set_permissions(&default, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(read.is_err(), "{read:?}");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn reads_only_the_apps_own_webkit_local_storage() {
        let root = temp("webkit");
        let webkit = root.join("WebKit");
        let utf16 = |s: &str| s.encode_utf16().flat_map(u16::to_le_bytes).map(|b| format!("{b:02X}")).collect::<String>();
        let store = |hash: &str, origin: &[u8], key: &str, value: &str| {
            let frame = webkit.join("WebsiteData/Default").join(hash).join(hash);
            std::fs::create_dir_all(frame.join("LocalStorage")).unwrap();
            std::fs::write(frame.join("origin"), origin).unwrap();
            let sql = format!("CREATE TABLE ItemTable (key TEXT UNIQUE ON CONFLICT REPLACE, value BLOB NOT NULL ON CONFLICT FAIL); INSERT INTO ItemTable VALUES ('{key}', X'{}');", utf16(value));
            let ok = std::process::Command::new("/usr/bin/sqlite3").arg(frame.join("LocalStorage/localstorage.sqlite3")).arg(sql).status().unwrap();
            assert!(ok.success());
        };
        // What WebKit writes for tauri://localhost, as the top and the frame origin.
        let app = b"\x05\0\0\0\x01tauri\x09\0\0\0\x01localhost\0";
        let site = b"\x05\0\0\0\x01https\x0d\0\0\0\x01tauri.example\0";
        store("a", &[&app[..], app].concat(), "yuzora:language", "zh-TW");
        std::thread::sleep(std::time::Duration::from_millis(20));
        // Newer pages the preview pane opened, with keys of the app's.
        store("b", &[&site[..], site].concat(), "yuzora:language", "en");
        store("c", &[&site[..], site].concat(), "yuzora.hosts.ssh.v2", "[{\"host\":\"planted\"}]");
        store("d", &[&app[..], site].concat(), "yuzora.hosts.ssh.v2", "[{\"host\":\"framed\"}]");
        store("e", b"\x05tauri\x09localhost", "yuzora.hosts.ssh.v2", "[{\"host\":\"odd\"}]");
        let old = Beta1::at(root.join("data"), root.join("dot"), Some(webkit), Box::new(MemoryLegacy::default()));
        assert_eq!(old.local_storage("yuzora:language").unwrap().as_deref(), Some("zh-TW"));
        assert_eq!(old.local_storage("yuzora.hosts.ssh.v2").unwrap(), None);
        assert_eq!(old.local_storage("it's").unwrap(), None);
        std::fs::remove_dir_all(root).unwrap();
    }
}
