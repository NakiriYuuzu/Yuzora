//! The `git` section of Yuzora's settings.json (IntelliJ's Version Control |
//! Git page, the parts that apply). Read where they're used, so a change on
//! the settings page applies to the next command.

use serde_json::{Value, json};
use std::path::PathBuf;

pub const SECTION: &str = "git";

/// Yuzora's own files (settings, commit message history). Tests use a scratch
/// folder so they never read or write the person's app home.
pub fn own_dir() -> PathBuf {
    if cfg!(test) { std::env::temp_dir().join(format!("yuzora-git-tests-{}", std::process::id())) } else { yuzora_core::paths::yuzora_dir() }
}

/// Yuzora's settings.json (see [`own_dir`]).
pub fn file() -> PathBuf {
    own_dir().join("settings.json")
}

/// Every setting and its default.
pub fn defaults() -> Value {
    json!({
        // General
        "executable": "",
        // Commit
        "warnCrlf": true,
        "warnDetachedHead": true,
        "largeFileWarn": true,
        "largeFileMb": 50,
        "cherryPickSuffix": true,
        "signOff": false,
        "skipHooks": false,
        "commitCleanup": "strip",
        // Push
        "forceWithLease": true,
        "autoUpdateOnReject": false,
        "pushTags": "none",
        "showPushDialog": true,
        "protectedBranches": "^(main|master)$",
        // Update
        "updateMethod": "merge",
        // Local changes are stashed around a pull (`--autostash`), IntelliJ's
        // "save changes" on update; beta.2 has no shelf, so stash is the one way.
        "autoStash": true,
        "fetchTags": "auto",
        "fetchPrune": false,
        "autoFetchMinutes": 0,
        // Log
        "logRecentCount": 2000,
        "logShowTags": true,
        "logShowRemotes": true,
        // Diff and annotate
        // A diff opens in a modal (Keep moves it beside the focused pane);
        // an open diff pane takes the next file instead.
        "diffOpenIn": "modal",
        "diffKeepIn": "split",
        "diffFollowPane": true,
        "diffMode": "side",
        "diffIgnoreWhitespace": "none",
        "diffContext": 3,
        "annotateIgnoreSpaces": true,
        "annotateDetectMovement": "none",
        // Conflicts
        "swapSidesInRebase": true,
        // Messages kept per repository
        "messageHistory": 20
    })
}

/// The stored section over the defaults.
pub fn all() -> Value {
    let mut v = defaults();
    if let Value::Object(stored) = yuzora_core::settings::section_from(&file(), SECTION)
        && let Value::Object(m) = &mut v
    {
        for (k, x) in stored {
            m.insert(k, x);
        }
    }
    v
}

pub fn get(key: &str) -> Value {
    all().get(key).cloned().unwrap_or(Value::Null)
}

pub fn flag(key: &str) -> bool {
    get(key).as_bool().unwrap_or(false)
}

pub fn text(key: &str) -> String {
    get(key).as_str().unwrap_or_default().to_owned()
}

pub fn number(key: &str) -> f64 {
    get(key).as_f64().unwrap_or(0.0)
}

/// The git program: the setting, else `git` on PATH.
pub fn executable() -> String {
    let e = text("executable");
    if e.trim().is_empty() { "git".into() } else { e.trim().to_owned() }
}

/// Whether a branch matches the protected-branches pattern (force push refused).
/// A pattern the engine can't read protects every branch: one written for
/// JavaScript (look-around) would otherwise protect none.
pub fn is_protected(branch: &str) -> bool {
    protects(&text("protectedBranches"), branch)
}

fn protects(pattern: &str, branch: &str) -> bool {
    !pattern.trim().is_empty() && regex::Regex::new(pattern.trim()).map_or(true, |r| r.is_match(branch))
}

/// Why the engine can't read a pattern, or `None`. The settings page asks
/// this, not JavaScript's RegExp: that takes more than the engine does.
pub fn pattern_error(pattern: &str) -> Option<String> {
    let pattern = pattern.trim();
    if pattern.is_empty() { None } else { regex::Regex::new(pattern).err().map(|e| e.to_string()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_cover_the_settings_page() {
        let d = defaults();
        assert_eq!(d["forceWithLease"], true);
        assert_eq!(d["updateMethod"], "merge");
        assert_eq!(d["largeFileMb"], 50);
        assert!(regex::Regex::new(d["protectedBranches"].as_str().unwrap()).unwrap().is_match("main"));
    }

    // The settings page (yuzora-git-settings) lists keys this file must
    // have a default for, with their strings in both languages.
    #[test]
    fn every_setting_on_the_page_has_a_default_and_its_strings() {
        const PAGE: &str = include_str!("../../../defaults/mods/yuzora-git-settings/main.js");
        let langs = [include_str!("../../../defaults/mods/yuzora-git-settings/i18n/en.json"), include_str!("../../../defaults/mods/yuzora-git-settings/i18n/zh-TW.json")]
            .map(|s| serde_json::from_str::<Value>(s).unwrap());
        let d = defaults();
        let items: Vec<&str> = PAGE.lines().filter_map(|l| l.trim().strip_prefix("{ key: \"")).collect();
        assert!(items.len() > 20, "{}", items.len());
        for item in items {
            let key = &item[..item.find('"').unwrap()];
            assert!(d.get(key).is_some(), "{key} has no default");
            let options: Vec<&str> = item.split_once("options: [").map(|(_, o)| o[..o.find(']').unwrap()].split(", ").map(|x| x.trim_matches('"')).collect()).unwrap_or_default();
            if !options.is_empty() {
                assert!(d[key].as_str().is_some_and(|v| options.contains(&v)), "{key}'s default {} isn't one of {options:?}", d[key]);
            }
            for l in &langs {
                for s in [format!("s.{key}"), format!("s.{key}.desc")].into_iter().chain(options.iter().map(|o| format!("opt.{key}.{o}"))) {
                    assert!(l[&s].is_string(), "no string {s}");
                }
            }
        }
    }

    #[test]
    fn a_pattern_the_engine_cant_read_protects_every_branch() {
        assert!(protects("^(main|release/.*)$", "main") && !protects("^(main|release/.*)$", "feature/x"));
        assert!(!protects("  ", "main"), "empty protects none");
        // JavaScript takes look-ahead; Rust's regex doesn't. Refusing every
        // force push beats allowing them to the branches it meant.
        assert!(protects("^(?!wip/).*", "main") && protects("^(?!wip/).*", "wip/x"));
    }

    #[test]
    fn tests_stay_out_of_the_app_home() {
        assert!(own_dir().starts_with(std::env::temp_dir()));
        assert_ne!(own_dir(), yuzora_core::paths::yuzora_dir());
    }
}
