//! Every theme has every color name the shipped layouts, mods and app code draw with.
//!
//! A person shows any layout under any theme. `settings::color` paints a name the theme
//! lacks in magenta, and a mod that reads one gets `undefined`. So a name used anywhere
//! below has to be one `settings::resolve` gives every theme, not one only Yuzora's own
//! theme files define. A child of `settings` (it reads the private theme list).

use super::{PRESETS, THEMES, Theme, parse_hex, parse_theme_file, preset, resolve};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// What a person can write in `themes/<name>.json` at the least: no `colors`.
const BASE_ONLY: [(&str, &str, &str); 2] = [("base colors only (dark)", "#101216", "#d8dae0"), ("base colors only (light)", "#fafaf7", "#24262b")];

fn themes() -> Vec<(String, Theme)> {
    let presets = PRESETS.iter().map(|n| (n.to_string(), resolve(preset(n).unwrap(), &BTreeMap::new())));
    let file = |name: &str, text: &str| {
        let (palette, colors) = parse_theme_file(name, text).unwrap();
        (name.to_owned(), resolve(&palette, &colors))
    };
    let base_only = BASE_ONLY.iter().map(|(name, background, foreground)| {
        let ansi = serde_json::to_string(&["#808080"; 16]).unwrap();
        file(name, &format!(r##"{{"background":"{background}","foreground":"{foreground}","selection":"#405060","ansi":{ansi}}}"##))
    });
    presets.chain(THEMES.iter().map(|(n, text)| file(n, text))).chain(base_only).collect()
}

fn is_name(s: &str) -> bool {
    s.starts_with(|c: char| c.is_ascii_lowercase()) && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// The identifier after each `prefix` that starts a word: `th.` finds `th.border`, not `width.x`.
fn after<'a>(text: &'a str, prefix: &'a str) -> impl Iterator<Item = &'a str> {
    text.match_indices(prefix).filter_map(move |(at, _)| {
        let word = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '$' || c == '.';
        if text[..at].chars().next_back().is_some_and(word) {
            return None;
        }
        let rest = &text[at + prefix.len()..];
        Some(&rest[..rest.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).unwrap_or(rest.len())])
    })
}

/// A layout document's colors: every `color` and `background` string, and a gradient's two.
fn layout_colors(v: &serde_json::Value, out: &mut Vec<String>) {
    match v {
        serde_json::Value::Array(items) => items.iter().for_each(|i| layout_colors(i, out)),
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                match (k.as_str(), v) {
                    ("color" | "background", serde_json::Value::String(s)) => out.push(s.clone()),
                    ("gradient", serde_json::Value::Array(pair)) => out.extend(pair.iter().filter_map(|c| c.as_str().map(str::to_owned))),
                    _ => layout_colors(v, out),
                }
            }
        }
        _ => {}
    }
}

/// A mod's reads of the theme: `theme().name`, and `th.name` (what every Yuzora mod calls it).
fn script_colors(text: &str) -> Vec<String> {
    after(text, "th.").chain(after(text, "theme().")).map(str::to_owned).collect()
}

/// The names app code paints by: `c("accent")`, `color("accent")`, `color_or_literal("accent")`, `theme()["accent"]`.
fn code_colors(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (open, close) in [("c(\"", "\")"), ("color(\"", "\")"), ("color_or_literal(\"", "\")"), ("theme()[\"", "\"]")] {
        for (at, _) in text.match_indices(open) {
            if text[..at].chars().next_back().is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.') {
                continue;
            }
            let rest = &text[at + open.len()..];
            if let Some(name) = rest.find('"').map(|end| &rest[..end]).filter(|name| rest[name.len()..].starts_with(close)) {
                out.push(name.to_owned());
            }
        }
    }
    out
}

fn files(dir: &Path, extension: &str, out: &mut Vec<std::path::PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())).map(|e| e.unwrap().path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            files(&path, extension, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some(extension) {
            out.push(path);
        }
    }
}

/// Every color name in use, with the files that use it.
fn used() -> BTreeMap<String, BTreeSet<String>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let layouts = |text: &str| {
        let mut out = Vec::new();
        layout_colors(&serde_json::from_str(text).unwrap(), &mut out);
        out
    };
    let sources: [(&str, &str, &dyn Fn(&str) -> Vec<String>); 5] = [
        ("defaults/layouts", "json", &layouts),
        // Layouts earlier versions shipped, which people who edited them still have.
        ("src/yuzora/dock_fixtures", "json", &layouts),
        ("defaults/mods", "js", &script_colors),
        ("defaults/frame", "js", &script_colors),
        ("src", "rs", &code_colors),
    ];
    let mut used: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (dir, extension, names) in sources {
        let mut paths = Vec::new();
        files(&root.join(dir), extension, &mut paths);
        let mut found = 0;
        for path in paths {
            let text = std::fs::read_to_string(&path).unwrap();
            for name in names(&text).into_iter().filter(|n| is_name(n)) {
                used.entry(name).or_default().insert(path.strip_prefix(root).unwrap().display().to_string());
                found += 1;
            }
        }
        assert!(found > 0, "{dir}: no color name found, so nothing in it was checked");
    }
    used
}

#[test]
fn every_color_name_in_use_is_in_every_theme() {
    let themes = themes();
    let mut missing = Vec::new();
    for (name, places) in used() {
        let without: Vec<&str> = themes.iter().filter(|(_, t)| t.get(&name).and_then(|v| parse_hex(v)).is_none()).map(|(n, _)| n.as_str()).collect();
        if !without.is_empty() {
            let places: Vec<String> = places.into_iter().collect();
            missing.push(format!("  {name}\n    used in: {}\n    not in:  {}", places.join(", "), without.join(", ")));
        }
    }
    assert!(
        missing.is_empty(),
        "color names some themes lack (painted magenta, `undefined` in a mod):\n{}\nGive every theme the name in settings::resolve, or use one it already has.",
        missing.join("\n")
    );
}

#[test]
fn a_theme_that_names_no_divider_draws_its_border() {
    let shipped = |name: &str| parse_theme_file(name, THEMES.iter().find(|(n, _)| *n == name).unwrap().1).unwrap();
    for (name, theme) in themes().into_iter().filter(|(n, _)| !n.starts_with("yuzora-")) {
        assert_eq!(theme.get("divider"), theme.get("border"), "{name}");
    }
    let (palette, mut colors) = shipped("atom-one-dark");
    colors.insert("border".into(), "#102030".into());
    assert_eq!(resolve(&palette, &colors)["divider"], "#102030", "a border a person set is their divider too");
    for name in ["yuzora-dark", "yuzora-light"] {
        let (palette, colors) = shipped(name);
        assert_ne!(colors["divider"], colors["border"], "{name} names a divider of its own");
        assert_eq!(resolve(&palette, &colors)["divider"], colors["divider"], "{name} keeps it");
    }
}

#[test]
fn the_scans_find_the_names() {
    let mut layout = Vec::new();
    layout_colors(
        &serde_json::json!({"background": "canvas", "divider": {"width": 1, "color": "divider"}, "children": [
            {"background": {"gradient": ["chrome", "#123456"]}, "content": {"pane": {"focus": {"color": "ring"}}}},
            {"background": {"image": "bg/sky.jpg"}}]}),
        &mut layout,
    );
    layout.sort();
    assert_eq!(layout, ["#123456", "canvas", "chrome", "divider", "ring"]);
    assert_eq!(script_colors("const th = theme(); div().w(width.px).bg(th.chrome).border_color(th.divider ?? th.border).text_color(theme().dim)"), ["chrome", "divider", "border", "dim"]);
    assert_eq!(
        code_colors(r#"d.bg(c("accent")).text_color(settings::color("muted")); c("cursor", v); color_or_literal("ring"); theme()["done"]; el.text_color("dim")"#),
        ["accent", "muted", "ring", "done"]
    );
    assert!(is_name("ansi12") && is_name("solid_foreground") && !is_name("#123456") && !is_name("headColor") && !is_name(""));
}
