//! UI text in English and Traditional Chinese (Taiwan).
//!
//! Every Yuzora mod keeps its strings next to its code:
//!
//! ```text
//! mods/<mod>/i18n/en.json      { "title": "Git", "commit": { "button": "Commit {count}" } }
//! mods/<mod>/i18n/zh-TW.json   { "title": "Git", "commit": { "button": "提交 {count} 個" } }
//! ```
//!
//! Nested objects flatten to dotted keys (`commit.button`); `{name}` is
//! replaced from the variables. A key missing in the shown language falls
//! back to English, then to the key itself, so a missing string shows up as
//! its key instead of as nothing. A Rust crate keeps its own catalog the
//! same way and loads it with [`Catalog::from_json`] (`include_str!`).
//!
//! The language follows `general.locale` in Yuzora's settings: `"system"`
//! (the default) asks the OS; any Chinese locale shows zh-TW, everything else
//! English.

use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, RwLock};

pub const EN: &str = "en";
pub const ZH_TW: &str = "zh-TW";
pub const LOCALES: &[&str] = &[EN, ZH_TW];

/// Maps any locale tag (`zh_TW.UTF-8`, `zh-Hant-TW`, `en-US`) onto a shipped one.
pub fn normalize(tag: &str) -> &'static str {
    let t = tag.trim().to_ascii_lowercase().replace('_', "-");
    if t == "zh" || t.starts_with("zh-") { ZH_TW } else { EN }
}

/// The language for a setting (`system`, `en`, `zh-TW`, unset) and the OS's locale.
pub fn resolve(choice: Option<&str>, system: Option<&str>) -> &'static str {
    match choice.map(str::trim).filter(|c| !c.is_empty() && !c.eq_ignore_ascii_case("system")) {
        Some(c) => normalize(c),
        None => system.map(normalize).unwrap_or(EN),
    }
}

pub fn system_locale() -> Option<String> {
    sys_locale::get_locale()
}

static CURRENT: RwLock<Option<&'static str>> = RwLock::new(None);

/// The language shown now (read once, then kept until [`refresh`]).
pub fn current() -> &'static str {
    if let Some(l) = *CURRENT.read().unwrap_or_else(|e| e.into_inner()) {
        return l;
    }
    let l = resolve(crate::settings::locale_choice().as_deref(), system_locale().as_deref());
    *CURRENT.write().unwrap_or_else(|e| e.into_inner()) = Some(l);
    l
}

/// Reads the setting again (after `general.locale` changed) and drops cached catalogs.
pub fn refresh() {
    *CURRENT.write().unwrap_or_else(|e| e.into_inner()) = None;
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// One language's strings, with English beneath it.
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
    /// English, then `locale` over it, from JSON texts (bad JSON reads as empty).
    pub fn from_json(en: &str, localized: Option<&str>) -> Self {
        let mut strings = HashMap::new();
        for text in std::iter::once(en).chain(localized) {
            if let Ok(v) = serde_json::from_str::<Value>(text) {
                flatten("", &v, &mut strings);
            }
        }
        Self { strings }
    }

    /// A mod's `i18n/` folder.
    pub fn load(i18n_dir: &Path, locale: &str) -> Self {
        let read = |l: &str| std::fs::read_to_string(i18n_dir.join(format!("{l}.json"))).ok();
        let en = read(EN).unwrap_or_default();
        let localized = if locale == EN { None } else { read(locale) };
        Self::from_json(&en, localized.as_deref())
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

static CACHE: Mutex<Option<HashMap<(String, &'static str), Arc<Catalog>>>> = Mutex::new(None);

/// The catalog of a mod (by instance name: `yuzora-git`, `frame-settings`) in the shown language.
pub fn catalog(mod_name: &str) -> Arc<Catalog> {
    let locale = current();
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    cache
        .get_or_insert_with(HashMap::new)
        .entry((mod_name.to_owned(), locale))
        .or_insert_with(|| Arc::new(Catalog::load(&crate::paths::mod_dir(mod_name).join("i18n"), locale)))
        .clone()
}

/// Forgets a mod's cached strings (its files changed and it is loading again).
pub fn forget(mod_name: &str) {
    if let Some(map) = CACHE.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        map.retain(|(name, _), _| name != mod_name);
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
    fn locale_tags_map_onto_the_two_shipped_languages() {
        for zh in ["zh-TW", "zh_TW.UTF-8", "zh-Hant-TW", "zh-HK", "zh-CN", "zh"] {
            assert_eq!(normalize(zh), ZH_TW, "{zh}");
        }
        for en in ["en-US", "ja-JP", "", "C"] {
            assert_eq!(normalize(en), EN, "{en}");
        }
    }

    #[test]
    fn the_setting_beats_the_system_and_system_follows_the_os() {
        assert_eq!(resolve(Some("en"), Some("zh-TW")), EN);
        assert_eq!(resolve(Some("zh-TW"), Some("en-US")), ZH_TW);
        assert_eq!(resolve(Some("system"), Some("zh_TW")), ZH_TW);
        assert_eq!(resolve(None, Some("en-GB")), EN);
        assert_eq!(resolve(None, None), EN);
    }

    #[test]
    fn missing_strings_fall_back_to_english_then_the_key() {
        let c = Catalog::from_json(
            r#"{ "title": "Git", "commit": { "button": "Commit {count}" }, "only_en": "English" }"#,
            Some(r#"{ "commit": { "button": "提交 {count} 個" } }"#),
        );
        assert_eq!(c.t("commit.button", &[("count", "3".into())]), "提交 3 個");
        assert_eq!(c.t("only_en", &[]), "English");
        assert_eq!(c.t("nope.missing", &[]), "nope.missing");
        assert_eq!(c.len(), 3);
    }

    #[test]
    fn catalogs_load_from_a_mod_folder() {
        let dir = std::env::temp_dir().join(format!("yuzora-i18n-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("en.json"), r#"{"a":"A","b":"B"}"#).unwrap();
        std::fs::write(dir.join("zh-TW.json"), r#"{"a":"甲"}"#).unwrap();
        let zh = Catalog::load(&dir, ZH_TW);
        assert_eq!((zh.t("a", &[]), zh.t("b", &[])), ("甲".into(), "B".into()));
        assert_eq!(Catalog::load(&dir, EN).t("a", &[]), "A");
        assert!(Catalog::load(&dir.join("missing"), ZH_TW).is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
