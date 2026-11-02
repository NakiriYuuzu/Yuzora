//! The `git` section of Yuzora's settings.json (IntelliJ's Version Control |
//! Git page, the parts that apply). Read where they're used, so a change on
//! the settings page applies to the next command.

use serde_json::{Value, json};

pub const SECTION: &str = "git";

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
    if let Value::Object(stored) = yuzora_core::settings::section(SECTION)
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
pub fn is_protected(branch: &str) -> bool {
    let pattern = text("protectedBranches");
    !pattern.trim().is_empty() && regex::Regex::new(pattern.trim()).is_ok_and(|r| r.is_match(branch))
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
}
