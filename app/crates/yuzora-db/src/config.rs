//! Saved connections: Yuzora settings' `db` section holds everything but the
//! password, which goes to the system credential store (namespace `db`, id =
//! the connection's id).
//!
//! ```json
//! { "db": { "connections": [ { "id": "c1", "name": "local-pg", "engine": "postgres", … } ] } }
//! ```

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use yuzora_core::secrets::{KeyringStore, MemoryStore, SecretKey, SecretStore};

use crate::error::{DbError, DbResult};

/// The settings section (and the credential namespace).
pub const SECTION: &str = "db";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    Postgres,
    Mssql,
    Sqlite,
}

impl Engine {
    pub fn label(self) -> &'static str {
        match self {
            Engine::Postgres => "PostgreSQL",
            Engine::Mssql => "SQL Server",
            Engine::Sqlite => "SQLite",
        }
    }

    pub fn default_port(self) -> Option<u16> {
        match self {
            Engine::Postgres => Some(5432),
            Engine::Mssql => Some(1433),
            Engine::Sqlite => None,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Engine::Postgres => "postgres",
            Engine::Mssql => "mssql",
            Engine::Sqlite => "sqlite",
        }
    }
}

/// How hard TLS is insisted on. One scale for both network engines:
/// `require` encrypts without checking the certificate, the `verify-*` modes
/// check it (PostgreSQL's names; SQL Server maps them onto encrypt / trust
/// server certificate).
/// Ordered weakest to strictest, which [`ConnectionConfig::password_may_follow`] relies on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SslMode {
    Disable,
    #[default]
    Prefer,
    Require,
    VerifyCa,
    VerifyFull,
}

/// Where an SSH tunnel would start (L4's `yuzora_ssh::tunnel`): the saved SSH
/// host's id. Kept and shown, not used yet: see docs/beta2/db.md, "SSH tunnel".
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SshTunnel {
    pub host: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ConnectionConfig {
    pub id: String,
    pub name: String,
    pub engine: Engine,
    pub host: String,
    pub port: Option<u16>,
    pub user: String,
    /// The database to open first (PostgreSQL needs one; empty = the user's default).
    pub database: String,
    pub ssl: SslMode,
    /// A PEM file of extra trusted roots (`verify-*`).
    pub ca_file: String,
    /// SQL Server named instance (`host\instance`).
    pub instance: String,
    /// SQLite file.
    pub path: String,
    pub read_only: bool,
    pub save_password: bool,
    pub ssh: Option<SshTunnel>,
}

impl Default for ConnectionConfig {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            engine: Engine::Postgres,
            host: String::new(),
            port: None,
            user: String::new(),
            database: String::new(),
            ssl: SslMode::Prefer,
            ca_file: String::new(),
            instance: String::new(),
            path: String::new(),
            read_only: false,
            save_password: true,
            ssh: None,
        }
    }
}

impl ConnectionConfig {
    pub fn port(&self) -> u16 {
        self.port.or(self.engine.default_port()).unwrap_or(0)
    }

    /// Whether the password saved for `self` may be sent for `edited`: the
    /// same engine, server, login and tunnel, over TLS at least as strict
    /// (and checked against the same CA). Anything else needs it typed again.
    pub fn password_may_follow(&self, edited: &ConnectionConfig) -> bool {
        let verifies = matches!(self.ssl, SslMode::VerifyCa | SslMode::VerifyFull);
        self.engine == edited.engine
            && self.host.trim().eq_ignore_ascii_case(edited.host.trim())
            && self.port() == edited.port()
            && self.user == edited.user
            && self.instance.trim().eq_ignore_ascii_case(edited.instance.trim())
            && self.ssh == edited.ssh
            && edited.ssl >= self.ssl
            && (!verifies || self.ca_file.trim() == edited.ca_file.trim())
    }

    /// `~` in a SQLite path is the home folder.
    pub fn sqlite_path(&self) -> PathBuf {
        let p = self.path.trim();
        match p.strip_prefix("~/").or_else(|| (p == "~").then_some("")) {
            Some(rest) => yuzora_core::paths::home_dir().join(rest),
            None => PathBuf::from(p),
        }
    }

    /// What the sidebar shows under the name: `local · 127.0.0.1:5432`, `~/x.sqlite`.
    pub fn target(&self) -> String {
        match self.engine {
            Engine::Sqlite => self.path.clone(),
            _ if self.instance.is_empty() => format!("{}:{}", self.host, self.port()),
            _ => format!("{}\\{}", self.host, self.instance),
        }
    }

    /// Refuses what no engine could connect with.
    pub fn validate(&self) -> DbResult<()> {
        let missing = |field: &str| Err(DbError::Config(crate::i18n::t("error.missing", &[("field", field.into())])));
        if self.name.trim().is_empty() {
            return missing("name");
        }
        match self.engine {
            Engine::Sqlite if self.path.trim().is_empty() => missing("path"),
            Engine::Sqlite => Ok(()),
            _ if self.host.trim().is_empty() => missing("host"),
            _ if self.user.trim().is_empty() => missing("user"),
            _ => Ok(()),
        }
    }
}

/// The saved connections, in the order the person made them.
pub fn load() -> Vec<ConnectionConfig> {
    parse(&yuzora_core::settings::section(SECTION))
}

fn parse(section: &Value) -> Vec<ConnectionConfig> {
    section
        .get("connections")
        .and_then(Value::as_array)
        .map(|list| list.iter().filter_map(|c| serde_json::from_value(c.clone()).ok()).collect())
        .unwrap_or_default()
}

fn store_list(list: &[ConnectionConfig]) -> DbResult<()> {
    let mut section = yuzora_core::settings::section(SECTION);
    if !section.is_object() {
        section = json!({});
    }
    section["connections"] = serde_json::to_value(list).map_err(|e| DbError::Io(e.to_string()))?;
    yuzora_core::settings::set_section(SECTION, section).map_err(DbError::from)
}

pub fn get(id: &str) -> DbResult<ConnectionConfig> {
    load()
        .into_iter()
        .find(|c| c.id == id)
        .ok_or_else(|| DbError::NotFound(crate::i18n::t("error.no_connection", &[("id", id.into())])))
}

/// Adds or replaces (by id; a new one gets an id). `password`: `Some` sets
/// it, `None` keeps the one there is unless the connection now points at
/// another server or login (see [`keep_password`]).
pub fn save(mut config: ConnectionConfig, password: Option<&str>) -> DbResult<ConnectionConfig> {
    config.validate()?;
    let mut list = load();
    if config.id.is_empty() {
        config.id = new_id(&list);
    }
    let mut moved = false;
    match list.iter_mut().find(|c| c.id == config.id) {
        Some(existing) => {
            moved = !existing.password_may_follow(&config);
            *existing = config.clone();
        }
        None => list.push(config.clone()),
    }
    keep_password(&*secrets(), &mut unsaved(), &config, password, moved)?;
    store_list(&list)?;
    Ok(config)
}

/// Where the password goes when `config` is saved: the credential store when
/// it saves passwords, this process only when it doesn't, and nowhere when
/// the connection `moved` to another server or login without a new one.
fn keep_password(
    store: &dyn SecretStore,
    unsaved: &mut HashMap<String, String>,
    config: &ConnectionConfig,
    password: Option<&str>,
    moved: bool,
) -> DbResult<()> {
    let key = secret_key(&config.id);
    match (config.save_password, password) {
        (true, Some(p)) => {
            store.set(&key, p)?;
            unsaved.remove(&config.id);
        }
        (false, Some(p)) => {
            store.delete(&key)?;
            unsaved.insert(config.id.clone(), p.to_owned());
        }
        (_, None) if moved => {
            store.delete(&key)?;
            unsaved.remove(&config.id);
        }
        (false, None) => store.delete(&key)?,
        (true, None) => {
            if let Some(p) = unsaved.remove(&config.id) {
                store.set(&key, &p)?;
            }
        }
    }
    Ok(())
}

pub fn delete(id: &str) -> DbResult<()> {
    let mut list = load();
    list.retain(|c| c.id != id);
    store_list(&list)?;
    unsaved().remove(id);
    secrets().delete(&secret_key(id))?;
    Ok(())
}

/// The connection's password: typed this session for a connection that
/// doesn't save it, else the credential store's.
pub fn password(id: &str) -> DbResult<Option<String>> {
    if let Some(p) = unsaved().get(id) {
        return Ok(Some(p.clone()));
    }
    Ok(secrets().get(&secret_key(id))?)
}

/// Passwords of connections with `savePassword` off: this process only.
fn unsaved() -> std::sync::MutexGuard<'static, HashMap<String, String>> {
    static UNSAVED: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    UNSAVED.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner())
}

/// The password saved under `config.id`, only while `config` still points at
/// the server and login it was saved for (see [`ConnectionConfig::password_may_follow`]).
pub fn saved_password_for(config: &ConnectionConfig) -> DbResult<Option<String>> {
    if config.id.is_empty() || config.engine == Engine::Sqlite {
        return Ok(None);
    }
    match get(&config.id) {
        Ok(saved) if saved.password_may_follow(config) => password(&config.id),
        _ => Ok(None),
    }
}

fn secret_key(id: &str) -> SecretKey {
    SecretKey::new(SECTION, id)
}

fn new_id(list: &[ConnectionConfig]) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut n = (nanos % 0xffff_ffff) as u64;
    loop {
        let id = format!("c{n:x}");
        if !list.iter().any(|c| c.id == id) {
            return id;
        }
        n += 1;
    }
}

/// The system credential store; `YUZORA_DB_SECRETS=memory` keeps passwords in
/// this process only (isolated test runs that must not touch the real store).
pub fn secrets() -> Arc<dyn SecretStore> {
    static STORE: OnceLock<Mutex<Arc<dyn SecretStore>>> = OnceLock::new();
    STORE
        .get_or_init(|| {
            let memory = std::env::var("YUZORA_DB_SECRETS").is_ok_and(|v| v == "memory");
            Mutex::new(if memory { Arc::new(MemoryStore::default()) } else { Arc::new(KeyringStore) })
        })
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configs_round_trip_with_camel_case_and_defaults() {
        let c: ConnectionConfig = serde_json::from_value(json!({
            "id": "c1", "name": "local-pg", "engine": "postgres", "host": "127.0.0.1",
            "user": "u", "ssl": "verify-full", "caFile": "/x.pem"
        }))
        .unwrap();
        assert_eq!(c.port(), 5432);
        assert_eq!(c.ssl, SslMode::VerifyFull);
        assert_eq!(c.ca_file, "/x.pem");
        assert!(c.save_password);
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!(v["caFile"], "/x.pem");
        assert_eq!(v["ssl"], "verify-full");
        assert_eq!(parse(&json!({ "connections": [v, {"engine": "nope"}] })), vec![c]);
    }

    #[test]
    fn a_saved_password_only_follows_the_same_server_login_and_tls() {
        let saved: ConnectionConfig = serde_json::from_value(json!({
            "id": "c1", "name": "pg", "engine": "postgres", "host": "DB.local", "user": "app"
        }))
        .unwrap();
        let mut edit = saved.clone();
        edit.name = "renamed".into();
        edit.host = " db.local ".into();
        edit.port = Some(5432);
        edit.ssl = SslMode::VerifyFull;
        assert!(saved.password_may_follow(&edit), "name, case, the default port spelled out and stricter TLS are the same server");
        for change in [
            |c: &mut ConnectionConfig| c.host = "evil.example".into(),
            |c: &mut ConnectionConfig| c.port = Some(6432),
            |c: &mut ConnectionConfig| c.user = "postgres".into(),
            |c: &mut ConnectionConfig| c.engine = Engine::Mssql,
            |c: &mut ConnectionConfig| c.ssh = Some(SshTunnel { host: "jump".into() }),
            |c: &mut ConnectionConfig| c.ssl = SslMode::Disable,
        ] {
            let mut other = saved.clone();
            change(&mut other);
            assert!(!saved.password_may_follow(&other), "{other:?}");
        }
        let mut verified = saved.clone();
        verified.ssl = SslMode::VerifyFull;
        verified.ca_file = "/etc/ca.pem".into();
        let mut weaker = verified.clone();
        weaker.ssl = SslMode::Require;
        let mut other_ca = verified.clone();
        other_ca.ca_file = "/tmp/mine.pem".into();
        assert!(!verified.password_may_follow(&weaker), "a weaker TLS mode needs the password again");
        assert!(!verified.password_may_follow(&other_ca), "so does trusting another CA");
    }

    #[test]
    fn passwords_follow_save_password_and_never_a_moved_connection() {
        let store = MemoryStore::default();
        let mut unsaved = HashMap::new();
        let mut c: ConnectionConfig = serde_json::from_value(json!({"id": "c1", "engine": "postgres", "host": "h"})).unwrap();
        let key = secret_key("c1");

        keep_password(&store, &mut unsaved, &c, Some("pw"), false).unwrap();
        assert_eq!(store.get(&key).unwrap().as_deref(), Some("pw"));
        keep_password(&store, &mut unsaved, &c, None, false).unwrap();
        assert_eq!(store.get(&key).unwrap().as_deref(), Some("pw"), "an edit without a password keeps it");

        keep_password(&store, &mut unsaved, &c, None, true).unwrap();
        assert_eq!(store.get(&key).unwrap(), None, "pointed elsewhere, the saved password is dropped");

        c.save_password = false;
        keep_password(&store, &mut unsaved, &c, Some("typed"), false).unwrap();
        assert_eq!(store.get(&key).unwrap(), None, "not saving keeps it out of the store");
        assert_eq!(unsaved.get("c1").map(String::as_str), Some("typed"), "but this session can connect");

        c.save_password = true;
        keep_password(&store, &mut unsaved, &c, None, false).unwrap();
        assert_eq!(store.get(&key).unwrap().as_deref(), Some("typed"), "turning saving on keeps the typed one");
        assert!(unsaved.is_empty());
    }

    #[test]
    fn validation_names_the_missing_field() {
        let mut c = ConnectionConfig { name: "x".into(), engine: Engine::Sqlite, ..Default::default() };
        assert!(matches!(c.validate(), Err(DbError::Config(_))));
        c.path = "~/a.sqlite".into();
        assert!(c.validate().is_ok());
        assert_eq!(c.sqlite_path(), yuzora_core::paths::home_dir().join("a.sqlite"));
        c.engine = Engine::Mssql;
        c.host = "lab".into();
        assert!(c.validate().is_err(), "a user is required");
        c.user = "sa".into();
        c.instance = "SQLEXPRESS".into();
        assert_eq!(c.target(), "lab\\SQLEXPRESS");
    }
}
