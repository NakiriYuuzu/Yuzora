//! Mods announce Yuzora tools and settings pages in their own `mod.json`,
//! so the "+ new pane" menu (L1) and the settings frame (L6) find them
//! without anyone editing a shared list:
//!
//! ```json
//! {
//!   "place": "none",
//!   "yuzora": {
//!     "pane": { "title": "pane.title", "icon": "mods/yuzora-git/icon.svg", "order": 20, "props": {} },
//!     "settings": { "title": "settings.title", "icon": "mods/yuzora-git/icon.svg", "order": 20, "section": "git" }
//!   }
//! }
//! ```
//!
//! - `pane`: the mod can open as a tool pane (`pane.split {mod, props}`).
//! - `settings`: the mod draws a settings page; `section` is its key in
//!   Yuzora's settings.json (defaults to the mod's name).
//! - `title` is a key in the mod's i18n strings (shown as is when it has none).
//! - `order` sorts the lists (default 100, then by name).

use serde::Deserialize;
use serde_json::{Value, json};
use std::path::Path;

#[derive(Debug, Default, Deserialize)]
struct Manifest {
    #[serde(default)]
    yuzora: Option<YuzoraBlock>,
}

#[derive(Debug, Default, Deserialize)]
struct YuzoraBlock {
    #[serde(default)]
    pane: Option<Entry>,
    #[serde(default)]
    settings: Option<Entry>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct Entry {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    icon: Option<String>,
    #[serde(default)]
    order: Option<f64>,
    #[serde(default)]
    props: Option<Value>,
    #[serde(default)]
    section: Option<String>,
}

/// What one scan found, each list sorted by `order`, then mod name.
#[derive(Debug, Default, PartialEq)]
pub struct Discovered {
    pub panes: Vec<Value>,
    pub settings: Vec<Value>,
}

/// Reads every `<mods>/<name>/mod.json` that has a `main.js`. `label` turns a
/// mod's title key into text (the shown language); pass `|_, k| k.to_owned()` for keys.
pub fn scan(mods: &Path, label: impl Fn(&str, &str) -> String) -> Discovered {
    let mut found = Discovered::default();
    let Ok(entries) = std::fs::read_dir(mods) else { return found };
    let mut dirs: Vec<_> = entries.flatten().map(|e| e.path()).filter(|p| p.join("main.js").is_file()).collect();
    dirs.sort();
    for dir in dirs {
        let Some(name) = dir.file_name().and_then(|n| n.to_str()).filter(|n| !n.starts_with('.')).map(str::to_owned) else { continue };
        let Ok(text) = std::fs::read_to_string(dir.join("mod.json")) else { continue };
        let Some(block) = serde_json::from_str::<Manifest>(&text).ok().and_then(|m| m.yuzora) else { continue };
        let describe = |e: &Entry, kind: &str| {
            let title = e.title.clone().unwrap_or_else(|| name.clone());
            let mut v = json!({
                "mod": name,
                "title": title,
                "label": label(&name, &title),
                "icon": e.icon,
                "order": e.order.unwrap_or(100.0),
            });
            match kind {
                "pane" => v["props"] = e.props.clone().unwrap_or(Value::Null),
                _ => v["section"] = json!(e.section.clone().unwrap_or_else(|| name.clone())),
            }
            v
        };
        if let Some(p) = &block.pane {
            found.panes.push(describe(p, "pane"));
        }
        if let Some(s) = &block.settings {
            found.settings.push(describe(s, "settings"));
        }
    }
    let key = |v: &Value| (v["order"].as_f64().unwrap_or(100.0), v["mod"].as_str().unwrap_or_default().to_owned());
    for list in [&mut found.panes, &mut found.settings] {
        list.sort_by(|a, b| key(a).partial_cmp(&key(b)).unwrap_or(std::cmp::Ordering::Equal));
    }
    found
}

/// The person's mods, titles in the shown language.
pub fn scan_installed() -> Discovered {
    scan(&crate::paths::mods_dir(), |m, k| crate::i18n::translate(m, k, &[]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, manifest: &str) {
        let d = dir.join(name);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("main.js"), "").unwrap();
        std::fs::write(d.join("mod.json"), manifest).unwrap();
    }

    #[test]
    fn panes_and_settings_pages_are_found_and_sorted() {
        let dir = std::env::temp_dir().join(format!("yuzora-discovery-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        write(&dir, "yuzora-git", r#"{"place":"none","yuzora":{"pane":{"title":"pane.title","order":20},"settings":{"title":"settings.title","section":"git"}}}"#);
        write(&dir, "yuzora-db", r#"{"place":"none","yuzora":{"pane":{"title":"pane.title","order":10,"props":{"view":"grid"}}}}"#);
        write(&dir, "plain", r#"{"place":"right"}"#);
        write(&dir, "broken", "{nope");
        std::fs::create_dir_all(dir.join("no-main")).unwrap();
        std::fs::write(dir.join("no-main/mod.json"), r#"{"yuzora":{"pane":{}}}"#).unwrap();

        let found = scan(&dir, |m, k| format!("{m}/{k}"));
        let mods = |l: &[Value]| l.iter().map(|v| v["mod"].as_str().unwrap().to_owned()).collect::<Vec<_>>();
        assert_eq!(mods(&found.panes), ["yuzora-db", "yuzora-git"]);
        assert_eq!(found.panes[0]["props"]["view"], "grid");
        assert_eq!(found.panes[1]["label"], "yuzora-git/pane.title");
        assert_eq!(mods(&found.settings), ["yuzora-git"]);
        assert_eq!(found.settings[0]["section"], "git");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
