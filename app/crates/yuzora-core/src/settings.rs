//! Yuzora's own settings: `<app home>/yuzora/settings.json`, one JSON object
//! per section. Herdr's `settings.json` (look, keys, sounds) stays Herdr's.
//!
//! ```json
//! { "general": { "locale": "system" }, "git": { … }, "db": { … } }
//! ```
//!
//! A section belongs to one lane (`general` to settings, L6); a lane reads and
//! writes only its own. Writes go through a temporary file and a rename.

use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The section holding app-wide choices such as `locale`.
pub const GENERAL: &str = "general";

static WRITE: Mutex<()> = Mutex::new(());

pub fn path() -> PathBuf {
    crate::paths::yuzora_dir().join("settings.json")
}

/// The whole file; an unreadable or missing one reads as `{}`.
pub fn load_from(file: &Path) -> Map<String, Value> {
    std::fs::read_to_string(file)
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| match v {
            Value::Object(m) => Some(m),
            _ => None,
        })
        .unwrap_or_default()
}

/// One section (`null` when unset).
pub fn section_from(file: &Path, name: &str) -> Value {
    load_from(file).remove(name).unwrap_or(Value::Null)
}

/// Replaces one section (`null` removes it), keeping the others.
pub fn set_section_at(file: &Path, name: &str, value: Value) -> std::io::Result<()> {
    let _guard = WRITE.lock().unwrap_or_else(|e| e.into_inner());
    let mut all = load_from(file);
    if value.is_null() {
        all.remove(name);
    } else {
        all.insert(name.to_owned(), value);
    }
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = file.with_extension(format!("json.tmp-{}", std::process::id()));
    std::fs::write(&tmp, serde_json::to_string_pretty(&Value::Object(all)).unwrap_or_default() + "\n")?;
    std::fs::rename(&tmp, file)
}

pub fn section(name: &str) -> Value {
    section_from(&path(), name)
}

pub fn set_section(name: &str, value: Value) -> std::io::Result<()> {
    set_section_at(&path(), name, value)
}

/// `general.locale`: `"system"` (the default), `"en"` or `"zh-TW"`.
pub fn locale_choice() -> Option<String> {
    section(GENERAL).get("locale").and_then(Value::as_str).map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sections_are_kept_apart_and_null_removes_one() {
        let dir = std::env::temp_dir().join(format!("yuzora-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = dir.join("yuzora/settings.json");
        assert_eq!(section_from(&file, "git"), Value::Null);
        set_section_at(&file, "git", json!({ "fetch": "auto" })).unwrap();
        set_section_at(&file, GENERAL, json!({ "locale": "zh-TW" })).unwrap();
        assert_eq!(section_from(&file, "git"), json!({ "fetch": "auto" }));
        assert_eq!(section_from(&file, GENERAL)["locale"], "zh-TW");
        set_section_at(&file, "git", Value::Null).unwrap();
        assert_eq!(load_from(&file).keys().collect::<Vec<_>>(), [GENERAL]);
        std::fs::write(&file, "not json").unwrap();
        assert!(load_from(&file).is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
