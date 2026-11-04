//! UI text, in English.
//!
//! Every Yuzora mod keeps its strings next to its code:
//!
//! ```text
//! mods/<mod>/i18n/en.json   { "title": "Git", "commit": { "button": "Commit {count}" } }
//! ```
//!
//! Nested objects flatten to dotted keys (`commit.button`); `{name}` is
//! replaced from the variables. A key missing from `en.json` shows up as the
//! key itself, so a missing string is visible instead of blank. A Rust crate
//! keeps its own catalog the same way and loads it with [`Catalog::from_json`]
//! (`include_str!`).
//!
//! The interface is English only. [`current`] is always `"en"`, whatever an
//! older `general.locale` setting says.

use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

pub const EN: &str = "en";

/// The language shown: always English.
pub fn current() -> &'static str {
    EN
}

/// A set of strings.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Catalog {
    strings: HashMap<String, String>,
}

fn flatten(prefix: &str, v: &Value, out: &mut HashMap<String, String>) {
    match v {
        Value::Object(m) => {
            for (k, v) in m {
                let key = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
                flatten(&key, v, out);
            }
        }
        Value::String(s) => {
            out.insert(prefix.to_owned(), s.clone());
        }
        Value::Null => {}
        other => {
            out.insert(prefix.to_owned(), other.to_string());
        }
    }
}

impl Catalog {
    /// From a JSON text (bad JSON reads as empty).
    pub fn from_json(en: &str) -> Self {
        let mut strings = HashMap::new();
        if let Ok(v) = serde_json::from_str::<Value>(en) {
            flatten("", &v, &mut strings);
        }
        Self { strings }
    }

    /// A mod's `i18n/` folder (`en.json`).
    pub fn load(i18n_dir: &Path) -> Self {
        let en = std::fs::read_to_string(i18n_dir.join(format!("{EN}.json"))).unwrap_or_default();
        Self::from_json(&en)
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.strings.get(key).map(String::as_str)
    }

    /// The text for `key` with `{name}` filled from `vars`; the key itself when missing.
    pub fn t(&self, key: &str, vars: &[(&str, String)]) -> String {
        let mut text = self.get(key).unwrap_or(key).to_owned();
        for (name, value) in vars {
            text = text.replace(&format!("{{{name}}}"), value);
        }
        text
    }

    pub fn len(&self) -> usize {
        self.strings.len()
    }

    pub fn is_empty(&self) -> bool {
        self.strings.is_empty()
    }
}

static CACHE: Mutex<Option<HashMap<String, Arc<Catalog>>>> = Mutex::new(None);

/// The catalog of a mod (by instance name: `yuzora-git`, `frame-settings`).
pub fn catalog(mod_name: &str) -> Arc<Catalog> {
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    cache
        .get_or_insert_with(HashMap::new)
        .entry(mod_name.to_owned())
        .or_insert_with(|| Arc::new(Catalog::load(&crate::paths::mod_dir(mod_name).join("i18n"))))
        .clone()
}

/// Forgets a mod's cached strings (its files changed and it is loading again).
pub fn forget(mod_name: &str) {
    if let Some(map) = CACHE.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        map.remove(mod_name);
    }
}

/// `key` in `mod_name`'s strings.
pub fn translate(mod_name: &str, key: &str, vars: &[(&str, String)]) -> String {
    catalog(mod_name).t(key, vars)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_language_is_always_english() {
        assert_eq!(current(), EN);
    }

    #[test]
    fn missing_strings_show_the_key() {
        let c = Catalog::from_json(r#"{ "title": "Git", "commit": { "button": "Commit {count}" } }"#);
        assert_eq!(c.t("commit.button", &[("count", "3".into())]), "Commit 3");
        assert_eq!(c.t("nope.missing", &[]), "nope.missing");
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn catalogs_load_from_a_mod_folder_and_only_read_en_json() {
        let dir = std::env::temp_dir().join(format!("yuzora-i18n-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("en.json"), r#"{"a":"A","b":"B"}"#).unwrap();
        std::fs::write(dir.join("zh-TW.json"), r#"{"a":"甲"}"#).unwrap();
        let c = Catalog::load(&dir);
        assert_eq!((c.t("a", &[]), c.t("b", &[])), ("A".into(), "B".into()));
        assert!(Catalog::load(&dir.join("missing")).is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
