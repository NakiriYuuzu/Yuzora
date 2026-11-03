//! Yuzora's design tokens that a Herdr theme has no name for: diff colors,
//! agent brand colors, Space badge colors, the agent states' shapes, sizes.
//! Theme colors (background, border, working, …) come from `theme()` and
//! `themes/yuzora-dark.json` / `yuzora-light.json`; this file is
//! `app/defaults/yuzora/tokens.json`. Its values are for a dark theme; while
//! the theme showing is light (the app says how to tell: [`set_light`]), its
//! `light` part replaces the ones it names.

use serde_json::Value;
use std::sync::OnceLock;

pub const TOKENS_JSON: &str = include_str!("../../../defaults/yuzora/tokens.json");

static LIGHT: OnceLock<fn() -> bool> = OnceLock::new();

/// How to tell whether the theme showing is light; asked on every [`tokens`] call.
pub fn set_light(light: fn() -> bool) {
    let _ = LIGHT.set(light);
}

/// Whether a `#rrggbb` color is light, the way Herdr tells a light theme by its background.
pub fn is_light(hex: &str) -> bool {
    let h = hex.trim().trim_start_matches('#');
    let Some(v) = h.get(..6).and_then(|s| u32::from_str_radix(s, 16).ok()) else { return false };
    let ch = |s: u32| ((v >> s) & 255) as f32 / 255.0;
    0.2126 * ch(16) + 0.7152 * ch(8) + 0.0722 * ch(0) > 0.5
}

/// Whether the theme showing is light (false until the app says how to tell).
pub fn light() -> bool {
    LIGHT.get().is_some_and(|light| light())
}

/// The tokens for the theme showing.
pub fn tokens() -> Value {
    for_theme(light())
}

/// The tokens for a dark (`false`) or a light (`true`) theme.
pub fn for_theme(light: bool) -> Value {
    let mut t: Value = serde_json::from_str(TOKENS_JSON).expect("defaults/yuzora/tokens.json is valid JSON");
    let over = t.as_object_mut().and_then(|o| o.remove("light"));
    if let (true, Some(over)) = (light, over) {
        merge(&mut t, over);
    }
    t
}

fn merge(into: &mut Value, over: Value) {
    match (into, over) {
        (Value::Object(into), Value::Object(over)) => {
            for (k, v) in over {
                merge(into.entry(k).or_insert(Value::Null), v);
            }
        }
        (into, over) => *into = over,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DARK: &str = include_str!("../../../defaults/themes/yuzora-dark.json");
    const LIGHT_THEME: &str = include_str!("../../../defaults/themes/yuzora-light.json");

    fn is_hex(v: &Value) -> bool {
        v.as_str().is_some_and(|s| s.len() == 7 && s.starts_with('#') && s[1..].chars().all(|c| c.is_ascii_hexdigit()))
    }

    #[test]
    fn tokens_hold_the_design_colors_and_shapes() {
        for light in [false, true] {
            let t = for_theme(light);
            assert!(t.get("light").is_none(), "the light part never shows");
            for k in ["added", "added_bg", "removed", "removed_bg"] {
                assert!(is_hex(&t["diff"][k]), "diff.{k}");
            }
            for k in ["claude", "codex", "pi", "gemini"] {
                assert!(is_hex(&t["agents"][k]), "agents.{k}");
            }
            for k in ["table", "view", "edited_row", "error"] {
                assert!(is_hex(&t["database"][k]), "database.{k}");
            }
            for k in ["keyword", "type", "string", "number", "comment", "function"] {
                assert!(is_hex(&t["sql"][k]), "sql.{k}");
            }
            assert!(t["space_badges"].as_array().is_some_and(|l| !l.is_empty() && l.iter().all(is_hex)));
            for k in ["working", "blocked", "done", "idle", "unknown"] {
                assert!(is_hex(&t["status"][k]["color"]), "status.{k}");
                assert!(t["status"][k]["shape"].is_string(), "status.{k}.shape");
            }
            assert_eq!(t["metrics"]["sidebar"], 248);
        }
    }

    // A typo in `light` would add a token nobody reads instead of replacing one.
    #[test]
    fn the_light_part_only_replaces_colors_that_exist() {
        fn walk(over: &Value, base: &Value, path: &str) {
            match over {
                Value::Object(o) => {
                    for (k, v) in o {
                        assert!(base.get(k).is_some(), "light.{path}{k} is not a token");
                        walk(v, &base[k], &format!("{path}{k}."));
                    }
                }
                // A list (the graph's lanes) replaces one as long, color for color.
                Value::Array(a) => {
                    assert_eq!(base.as_array().map(Vec::len), Some(a.len()), "light.{path}: {over} replaces {base}");
                    for (i, v) in a.iter().enumerate() {
                        walk(v, &base[i], &format!("{path}{i}."));
                    }
                }
                v => assert!(is_hex(v) && is_hex(base), "light.{path}: {v} replaces {base}"),
            }
        }
        let all: Value = serde_json::from_str(TOKENS_JSON).unwrap();
        walk(&all["light"], &for_theme(false), "");
        assert_ne!(for_theme(true)["text"]["strong"], for_theme(false)["text"]["strong"]);
        assert_eq!(for_theme(true)["space_badges"], for_theme(false)["space_badges"]);
    }

    #[test]
    fn tells_light_colors() {
        assert!(is_light("#ffffff") && is_light("#f4f4f6"));
        assert!(!is_light("#17181b") && !is_light("#151619"));
        assert!(!is_light("") && !is_light("#12"));
    }

    fn contrast(a: &str, b: &str) -> f64 {
        fn lum(hex: &str) -> f64 {
            let v = u32::from_str_radix(hex.trim_start_matches('#'), 16).unwrap();
            let ch = |s: u32| {
                let c = ((v >> s) & 255) as f64 / 255.0;
                if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
            };
            0.2126 * ch(16) + 0.7152 * ch(8) + 0.0722 * ch(0)
        }
        let (x, y) = (lum(a), lum(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    // WCAG AA: 4.5 for text on every panel color it's drawn on, 3 for marks and rings.
    #[test]
    fn both_yuzora_themes_read_at_wcag_aa() {
        for (file, light) in [(DARK, false), (LIGHT_THEME, true)] {
            let theme: Value = serde_json::from_str(file).unwrap();
            let c = |k: &str| theme["colors"][k].as_str().unwrap_or_else(|| panic!("colors.{k}")).to_owned();
            let t = for_theme(light);
            let panels = ["background", "canvas", "chrome", "popover", "raised", "hover", "selected"];
            for fg in ["foreground", "muted", "dim", "accent", "working", "blocked", "done", "idle", "unknown"] {
                for bg in panels {
                    let r = contrast(&c(fg), &c(bg));
                    assert!(r >= 4.5, "light={light}: {fg} on {bg} is {r:.2}");
                }
                if ["working", "blocked", "done", "idle", "unknown"].contains(&fg) {
                    assert_eq!(t["status"][fg]["color"].as_str(), Some(c(fg).as_str()), "light={light}: status.{fg}.color");
                }
            }
            assert!(contrast(&c("accent_foreground"), &c("accent")) >= 4.5, "light={light}: accent_foreground");
            assert!(contrast(&c("solid_foreground"), &c("solid")) >= 4.5, "light={light}: solid_foreground");
            for bg in ["background", "chrome", "popover"] {
                assert!(contrast(&c("ring"), &c(bg)) >= 3.0, "light={light}: ring on {bg}");
            }
            for (k, s) in t["status"].as_object().unwrap() {
                for bg in ["background", "chrome", "selected"] {
                    let r = contrast(s["mark"].as_str().unwrap(), &c(bg));
                    assert!(r >= 3.0, "light={light}: status.{k}.mark on {bg} is {r:.2}");
                }
            }
            for k in ["strong", "soft"] {
                assert!(contrast(t["text"][k].as_str().unwrap(), &c("selected")) >= 4.5, "light={light}: text.{k}");
            }
            let s = |a: &str, b: &str| t.pointer(&format!("/{a}/{b}")).and_then(Value::as_str).unwrap().to_owned();
            for (fg, bg) in [(s("diff", "added"), s("diff", "added_bg")), (s("diff", "removed"), s("diff", "removed_bg")), (s("attention", "text"), s("attention", "background"))] {
                assert!(contrast(&fg, &bg) >= 4.5, "light={light}: {fg} on {bg}");
            }
            // Databases: SQL on the editor and grid, an edited row, an error, the tree's icons.
            let edited = s("database", "edited_row");
            for (k, v) in t["sql"].as_object().unwrap() {
                for bg in [c("background"), c("raised"), c("chrome"), c("popover"), edited.clone()] {
                    let r = contrast(v.as_str().unwrap(), &bg);
                    assert!(r >= 4.5, "light={light}: sql.{k} on {bg} is {r:.2}");
                }
            }
            assert!(contrast(&c("foreground"), &edited) >= 4.5, "light={light}: an edited row's text");
            for bg in [c("background"), c("raised"), edited.clone()] {
                assert!(contrast(&s("database", "error"), &bg) >= 4.5, "light={light}: database.error on {bg}");
            }
            for k in ["table", "view"] {
                for bg in ["chrome", "selected"] {
                    assert!(contrast(&s("database", k), &c(bg)) >= 3.0, "light={light}: database.{k} on {bg}");
                }
            }
            for b in t["space_badges"].as_array().unwrap() {
                assert!(contrast(t["text"]["on_badge"].as_str().unwrap(), b.as_str().unwrap()) >= 4.5, "badge {b}");
            }
            // Git's log graph: a lane for each of the graph's eight colors and
            // HEAD's dot, marks on a row as it is plain, hovered and selected.
            let lanes = t["git_graph"]["lanes"].as_array().unwrap();
            assert_eq!(lanes.len(), 8, "light={light}: git_graph.lanes");
            for v in lanes.iter().chain([&t["git_graph"]["head"]]) {
                for bg in ["background", "hover", "selected"] {
                    let r = contrast(v.as_str().unwrap(), &c(bg));
                    assert!(r >= 3.0, "light={light}: git_graph {v} on {bg} is {r:.2}");
                }
            }
            // The terminal: every color but black on the terminal's background.
            let bg = theme["background"].as_str().unwrap();
            assert!(contrast(theme["foreground"].as_str().unwrap(), bg) >= 4.5);
            for (i, a) in theme["ansi"].as_array().unwrap().iter().enumerate().skip(1) {
                let r = contrast(a.as_str().unwrap(), bg);
                assert!(r >= 4.5, "light={light}: ansi{i} is {r:.2}");
            }
        }
    }
}
