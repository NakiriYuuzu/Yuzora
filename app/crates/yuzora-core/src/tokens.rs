//! Yuzora's design tokens that a Herdr theme has no name for: diff colors,
//! agent brand colors, Space badge colors, the agent states' shapes, sizes.
//! Theme colors (background, border, working, …) come from `theme()` and
//! `themes/yuzora-dark.json`; this file is `app/defaults/yuzora/tokens.json`.

use serde_json::Value;

pub const TOKENS_JSON: &str = include_str!("../../../defaults/yuzora/tokens.json");

pub fn tokens() -> Value {
    serde_json::from_str(TOKENS_JSON).expect("defaults/yuzora/tokens.json is valid JSON")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_hex(v: &Value) -> bool {
        v.as_str().is_some_and(|s| s.len() == 7 && s.starts_with('#') && s[1..].chars().all(|c| c.is_ascii_hexdigit()))
    }

    #[test]
    fn tokens_hold_the_design_colors_and_shapes() {
        let t = tokens();
        for k in ["added", "added_bg", "removed", "removed_bg"] {
            assert!(is_hex(&t["diff"][k]), "diff.{k}");
        }
        for k in ["claude", "codex", "pi", "gemini"] {
            assert!(is_hex(&t["agents"][k]), "agents.{k}");
        }
        assert!(t["space_badges"].as_array().is_some_and(|l| !l.is_empty() && l.iter().all(is_hex)));
        for k in ["working", "blocked", "done", "idle", "unknown"] {
            assert!(is_hex(&t["status"][k]["color"]), "status.{k}");
            assert!(t["status"][k]["shape"].is_string(), "status.{k}.shape");
        }
        assert_eq!(t["metrics"]["sidebar"], 248);
    }
}
