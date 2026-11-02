//! Query history per saved connection: `<app home>/yuzora/db-history.json`,
//! newest first, at most [`KEEP`] entries each. Statements only, never results.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

pub const KEEP: usize = 200;
/// Longer statements are kept cut to this many characters.
const MAX_CHARS: usize = 20_000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub sql: String,
    /// Milliseconds since the Unix epoch.
    pub at: u64,
    pub elapsed_ms: u64,
    pub ok: bool,
}

static LOCK: Mutex<()> = Mutex::new(());

pub fn path() -> PathBuf {
    yuzora_core::paths::yuzora_dir().join("db-history.json")
}

fn load_from(file: &Path) -> HashMap<String, Vec<Entry>> {
    std::fs::read_to_string(file).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn store_at(file: &Path, all: &HashMap<String, Vec<Entry>>) -> std::io::Result<()> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = file.with_extension(format!("json.tmp-{}", std::process::id()));
    std::fs::write(&tmp, serde_json::to_string(all).unwrap_or_default())?;
    std::fs::rename(&tmp, file)
}

pub fn record_at(file: &Path, connection: &str, sql: &str, elapsed_ms: u64, ok: bool) {
    let sql = sql.trim();
    if sql.is_empty() {
        return;
    }
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut all = load_from(file);
    let list = all.entry(connection.to_owned()).or_default();
    let sql: String = sql.chars().take(MAX_CHARS).collect();
    // The same statement again moves to the top instead of repeating.
    list.retain(|e| e.sql != sql);
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    list.insert(0, Entry { sql, at, elapsed_ms, ok });
    list.truncate(KEEP);
    let _ = store_at(file, &all);
}

pub fn list_at(file: &Path, connection: &str) -> Vec<Entry> {
    load_from(file).remove(connection).unwrap_or_default()
}

pub fn clear_at(file: &Path, connection: &str) {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut all = load_from(file);
    if all.remove(connection).is_some() {
        let _ = store_at(file, &all);
    }
}

pub fn record(connection: &str, sql: &str, elapsed_ms: u64, ok: bool) {
    record_at(&path(), connection, sql, elapsed_ms, ok)
}

pub fn list(connection: &str) -> Vec<Entry> {
    list_at(&path(), connection)
}

pub fn clear(connection: &str) {
    clear_at(&path(), connection)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newest_first_without_repeats_and_per_connection() {
        let dir = std::env::temp_dir().join(format!("yuzora-db-history-{}", std::process::id()));
        let file = dir.join("db-history.json");
        let _ = std::fs::remove_dir_all(&dir);
        record_at(&file, "a", "select 1", 3, true);
        record_at(&file, "a", "select 2", 4, false);
        record_at(&file, "a", "  select 1 ", 5, true);
        record_at(&file, "b", "select 3", 1, true);
        record_at(&file, "a", "   ", 1, true);
        let a: Vec<_> = list_at(&file, "a").into_iter().map(|e| (e.sql, e.elapsed_ms)).collect();
        assert_eq!(a, [("select 1".to_owned(), 5), ("select 2".to_owned(), 4)]);
        clear_at(&file, "a");
        assert!(list_at(&file, "a").is_empty());
        assert_eq!(list_at(&file, "b").len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
