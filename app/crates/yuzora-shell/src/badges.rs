//! A Space's badge: its initial on a color it keeps. Colors come from
//! `tokens().space_badges`, the least used one for a Space seen the first
//! time, and are kept in Yuzora's `settings.json` (`shell.space_colors`), so
//! the sidebar, the tab bar, pane headers and other lanes' headers agree, and
//! the person can pick another by editing it.

use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The `settings.json` section the shell keeps its choices in.
pub const SECTION: &str = "shell";
const KEY: &str = "space_colors";

/// The palette: `tokens().space_badges`.
pub fn palette() -> Vec<String> {
    yuzora_core::tokens::tokens()["space_badges"]
        .as_array()
        .map(|l| l.iter().filter_map(Value::as_str).map(str::to_owned).collect())
        .unwrap_or_default()
}

/// What a Space's badge says: the first letter or digit of its name, upper case
/// (`~` and other marks only when the name has nothing else).
pub fn initial(name: &str) -> String {
    let first = name.chars().find(|c| c.is_alphanumeric()).or_else(|| name.chars().find(|c| !c.is_whitespace()));
    first.map_or_else(|| "?".to_owned(), |c| c.to_uppercase().collect())
}

/// The color kept for `id`, or the palette's least used one (the first of the
/// ties) given to it now. Returns the color and whether `kept` changed.
pub fn assign(kept: &mut BTreeMap<String, String>, id: &str, palette: &[String]) -> (String, bool) {
    if let Some(c) = kept.get(id) {
        return (c.clone(), false);
    }
    let Some(first) = palette.first() else {
        return ("#4f6bed".to_owned(), false);
    };
    let uses = |c: &String| kept.values().filter(|v| *v == c).count();
    let color = palette.iter().min_by_key(|c| uses(c)).unwrap_or(first).clone();
    kept.insert(id.to_owned(), color.clone());
    (color, true)
}

/// The kept colors, read once from `settings.json`, written when one is added.
pub struct Store {
    file: PathBuf,
    kept: Option<BTreeMap<String, String>>,
}

impl Store {
    pub const fn at(file: PathBuf) -> Self {
        Self { file, kept: None }
    }

    fn load(file: &Path) -> BTreeMap<String, String> {
        yuzora_core::settings::section_from(file, SECTION)
            .get(KEY)
            .and_then(Value::as_object)
            .map(|m| m.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned()))).collect())
            .unwrap_or_default()
    }

    /// `id`'s badge: initial and color.
    pub fn badge(&mut self, id: &str, name: &str, palette: &[String]) -> (String, String) {
        let kept = self.kept.get_or_insert_with(|| Self::load(&self.file));
        let (color, changed) = assign(kept, id, palette);
        if changed {
            // Keep the section's other keys (the person's, or later ones).
            let mut section = match yuzora_core::settings::section_from(&self.file, SECTION) {
                Value::Object(m) => m,
                _ => Map::new(),
            };
            section.insert(KEY.into(), json!(kept));
            // A failed write only means the color is chosen again next start.
            let _ = yuzora_core::settings::set_section_at(&self.file, SECTION, Value::Object(section));
        }
        (initial(name), color)
    }
}

static STORE: Mutex<Option<Store>> = Mutex::new(None);

/// `id`'s badge from the app home's `settings.json`.
pub fn badge(id: &str, name: &str) -> (String, String) {
    let mut store = STORE.lock().unwrap_or_else(|e| e.into_inner());
    let palette = palette();
    store.get_or_insert_with(|| Store::at(yuzora_core::settings::path())).badge(id, name, &palette)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn colors() -> Vec<String> {
        ["#a", "#b", "#c"].map(String::from).to_vec()
    }

    #[test]
    fn spaces_get_the_least_used_color_and_keep_it() {
        let mut kept = BTreeMap::new();
        let p = colors();
        assert_eq!(assign(&mut kept, "s1", &p), ("#a".into(), true));
        assert_eq!(assign(&mut kept, "s2", &p), ("#b".into(), true));
        assert_eq!(assign(&mut kept, "s1", &p), ("#a".into(), false));
        assert_eq!(assign(&mut kept, "s3", &p).0, "#c");
        assert_eq!(assign(&mut kept, "s4", &p).0, "#a");
        kept.remove("s2");
        assert_eq!(assign(&mut kept, "s5", &p).0, "#b");
        assert_eq!(assign(&mut BTreeMap::new(), "s1", &[]), ("#4f6bed".into(), false));
    }

    #[test]
    fn initials_skip_marks_and_follow_case() {
        assert_eq!(initial("yuzora"), "Y");
        assert_eq!(initial(".yuuzu"), "Y");
        assert_eq!(initial("~"), "~");
        assert_eq!(initial("中文"), "中");
        assert_eq!(initial("  "), "?");
        assert_eq!(initial("2fa"), "2");
    }

    #[test]
    fn the_store_survives_a_restart_and_keeps_other_keys() {
        let dir = std::env::temp_dir().join(format!("yuzora-shell-badges-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = dir.join("yuzora/settings.json");
        yuzora_core::settings::set_section_at(&file, SECTION, json!({ "mine": 1, KEY: { "s9": "#c" } })).unwrap();
        let p = colors();
        let mut store = Store::at(file.clone());
        assert_eq!(store.badge("s9", "infra", &p), ("I".into(), "#c".into()));
        assert_eq!(store.badge("s1", "yuzora", &p), ("Y".into(), "#a".into()));
        let mut again = Store::at(file.clone());
        assert_eq!(again.badge("s1", "yuzora", &p).1, "#a");
        let section = yuzora_core::settings::section_from(&file, SECTION);
        assert_eq!(section["mine"], 1);
        assert_eq!(section[KEY], json!({ "s1": "#a", "s9": "#c" }));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_palette_is_the_tokens() {
        let p = palette();
        assert!(p.len() >= 4);
        assert_eq!(p[0], "#4f6bed");
    }
}
