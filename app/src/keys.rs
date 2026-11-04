//! ~/.config/herdr-app/keys.json: every key is a string action, reloaded live.
//!
//! ```json
//! {
//!   "global":   { "ctrl-shift-t": "tab.new", "ctrl-1": "tab.select:1" },
//!   "terminal": { "ctrl-left": "send:\u001bb", "ctrl-shift-t": "terminal" },
//!   "layout:terminals": { "ctrl-]": "space.next" },
//!   "mod:editor": { "ctrl-s": "editor.save" }
//! }
//! ```
//!
//! An action is a command (`pane.split:right`: the value fills its first
//! argument), `send:<text>` (types into the focused terminal),
//! `<mod>.<command>` (delivered to that mod's `nextCommand()`), or `terminal`
//! (the program gets the key: the binding is switched off where this section
//! applies).

use gpui::{App, KeyBinding, KeyBindingContextPredicate, NoAction};
use std::rc::Rc;
use gpui_shell::action::ShellAction;
use std::collections::BTreeMap;

/// yuzora(L1): where the shipped keys used to bind `layout.switch` (next layout).
#[cfg(target_os = "macos")]
const OLD_NEXT_LAYOUT_KEY: &str = "cmd-shift-l";
#[cfg(not(target_os = "macos"))]
const OLD_NEXT_LAYOUT_KEY: &str = "ctrl-shift-l";

/// yuzora(L1): the binding the shipped keys used to have for next layout, in a keys.json written
/// before it was removed. `parse` and `listing` both leave it out; a named `layout.switch:<name>`,
/// another key, or another section stays.
fn retired_default(section: &str, keys: &str, action: &str) -> bool {
    section == "global" && action == "layout.switch" && keys == OLD_NEXT_LAYOUT_KEY
}

/// The reserved shortcut that restores the default layout and keys.
pub const ESCAPE_HATCH: &str = "ctrl-alt-shift-escape";

/// The shipped keys: prefix-free; plain ctrl+letter always stays with the
/// program. Herdr uses ctrl+shift (defaults/keys.json), or cmd on macOS
/// (defaults/keys.macos.json, with Mac terminal editing keys).
#[cfg(not(target_os = "macos"))]
pub const DEFAULTS: &str = include_str!("../defaults/keys.json");
#[cfg(target_os = "macos")]
pub const DEFAULTS: &str = include_str!("../defaults/keys.macos.json");

fn defaults() -> BTreeMap<String, BTreeMap<String, String>> {
    serde_json::from_str(DEFAULTS).expect("defaults/keys.json is valid")
}

/// Every binding as `(section, keys, action)`, for the keybind help.
pub fn listing() -> Vec<(String, String, String)> {
    listing_of(&std::fs::read_to_string(path()).unwrap_or_default())
}

fn listing_of(text: &str) -> Vec<(String, String, String)> {
    let sections: BTreeMap<String, BTreeMap<String, String>> = serde_json::from_str(text).unwrap_or_else(|_| defaults());
    let mut out = Vec::new();
    for (s, keys) in sections {
        for (k, a) in keys {
            let a = current(&a);
            if !gone_mod(&a) && !retired_default(&s, &k, &a) {
                out.push((s.clone(), k, a));
            }
        }
    }
    out.push(("global".into(), ESCAPE_HATCH.into(), "app.reset".into()));
    out
}

pub fn path() -> std::path::PathBuf {
    crate::mods::app_dir().join("keys.json")
}

/// The GPUI key tab a section applies in. `global` is the window itself,
/// not "no tab": GPUI ranks a binding without one above every other, so
/// it would beat `layout:` (same depth, added earlier) and only lose to
/// `terminal` and `mod:` by order.
fn tab(section: &str) -> Result<String, String> {
    match section {
        "global" => Ok("Herdr".into()),
        "terminal" => Ok("Terminal".into()),
        s => match (s.strip_prefix("mod:"), s.strip_prefix("layout:")) {
            (Some(p), _) if !p.is_empty() => Ok(format!("mod == {p}")),
            // While layouts/<name>.json is shown (the window's tab says which).
            (_, Some(l)) if !l.is_empty() => Ok(format!("layout == {l}")),
            _ => Err(format!("unknown section {s:?}: use \"global\", \"terminal\", \"layout:<name>\" or \"mod:<name>\"")),
        },
    }
}

/// 0.1.0's words in a keys.json from then (installed once, so still theirs):
/// projects became spaces, contexts tabs, the start popup the new tab.
pub(crate) fn current(action: &str) -> String {
    if action == "mod.open:start" {
        return "tab.new".into();
    }
    let command = |a: &str| crate::commands::find(a.split_once(':').map_or(a, |(n, _)| n)).is_some();
    for (old, new) in [("project.", "space."), ("context.", "tab.")] {
        if let Some(rest) = action.strip_prefix(old) {
            let renamed = format!("{new}{rest}");
            // Only a command that was renamed: a mod named `project` keeps its own.
            if command(&renamed) && !command(action) {
                return renamed;
            }
        }
    }
    action.to_owned()
}

/// A mod's command whose mod is gone (0.1.x's `ribbon.next`): bound to nothing.
fn gone_mod(action: &str) -> bool {
    let name = action.split_once(':').map_or(action, |(n, _)| n);
    crate::commands::find(name).is_none()
        && name.split_once('.').is_some_and(|(m, _)| !m.starts_with("frame-") && !crate::mods::mods_dir().join(m).is_dir())
}

fn valid_action(action: &str) -> bool {
    let name = action.split_once(':').map_or(action, |(n, _)| n);
    crate::commands::find(name).is_some()
        || action == "terminal"
        || action.starts_with("send:")
        || name.split_once('.').is_some_and(|(m, c)| !m.is_empty() && !c.is_empty())
}

/// Reads keys.json (writing the defaults if it's missing) into bindings, or says what's wrong.
pub fn load() -> Result<Vec<KeyBinding>, String> {
    let p = path();
    if !p.exists() {
        let _ = std::fs::write(&p, DEFAULTS);
    }
    let text = std::fs::read_to_string(&p).map_err(|e| e.to_string())?;
    parse(&text)
}

pub fn parse(text: &str) -> Result<Vec<KeyBinding>, String> {
    let sections: BTreeMap<String, BTreeMap<String, String>> = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    let mut problems = Vec::new();
    for (section, keys) in &sections {
        let ctx = match tab(section) {
            Ok(c) => c,
            Err(e) => {
                problems.push(e);
                continue;
            }
        };
        for (keys, action) in keys {
            let action = &current(action);
            if gone_mod(action) {
                continue;
            }
            // yuzora(L1): the old default "next layout" key is dropped from keys.json files written
            // before it was removed (keys.json is only written once); another key for it stays.
            if retired_default(section, keys, action) {
                continue;
            }
            if !valid_action(action) {
                problems.push(format!("{section}.{keys}: unknown action {action:?}"));
                continue;
            }
            if keys == ESCAPE_HATCH {
                problems.push(format!("{ESCAPE_HATCH} is reserved for resetting Herdr"));
                continue;
            }
            let predicate = match KeyBindingContextPredicate::parse(&ctx) {
                Ok(p) => Some(Rc::new(p)),
                Err(e) => {
                    problems.push(format!("{section}: {e}"));
                    continue;
                }
            };
            let action: Box<dyn gpui::Action> = if action == "terminal" {
                // Off where this section applies; globally, simply not bound.
                if section == "global" {
                    continue;
                }
                Box::new(NoAction)
            } else {
                #[cfg(target_os = "macos")]
                let native = crate::macos::action(action);
                #[cfg(not(target_os = "macos"))]
                let native: Option<Box<dyn gpui::Action>> = None;
                native.unwrap_or_else(|| Box::new(ShellAction::new(action.clone())))
            };
            let binding = KeyBinding::load(keys, action, predicate, false, None, &gpui::DummyKeyboardMapper);
            match binding {
                Ok(b) => out.push(b),
                Err(e) => problems.push(format!("{section}.{keys}: {e}")),
            }
        }
    }
    if problems.is_empty() { Ok(out) } else { Err(problems.join("; ")) }
}

/// The UI kit's own keys (text editing in inputs, lists): bound at its init,
/// kept across every reload of keys.json.
struct KitKeys(Vec<KeyBinding>);
impl gpui::Global for KitKeys {}

/// Restores kit bindings, installs user bindings, then adds reserved bindings.
pub fn install(bindings: Vec<KeyBinding>, cx: &mut App) {
    if !cx.has_global::<KitKeys>() {
        let kit = cx.key_bindings().borrow().bindings().cloned().collect();
        cx.set_global(KitKeys(kit));
    }
    let kit = cx.global::<KitKeys>().0.clone();
    cx.clear_key_bindings();
    cx.bind_keys(kit);
    cx.bind_keys(bindings);
    cx.bind_keys([
        KeyBinding::new(ESCAPE_HATCH, ShellAction::new("app.reset"), None),
        KeyBinding::new("escape", ShellAction::new("overlay.close"), Some("Overlay")),
    ]);
}

#[cfg(test)]
mod tests {
    #[test]
    fn keys_from_0_1_0_mean_the_same_now() {
        assert_eq!(super::current("project.select:2"), "space.select:2");
        assert_eq!(super::current("context.new"), "tab.new");
        assert_eq!(super::current("mod.open:start"), "tab.new");
        assert_eq!(super::current("pane.zoom"), "pane.zoom");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mac_shifted_punctuation_matches_existing_keys_files() {
        use gpui::{KeybindingKeystroke, Keystroke};
        let bindings = super::parse(include_str!("../defaults/keys.macos.json")).unwrap();
        for (base, shifted) in [("]", "}"), ("[", "{")] {
            let event = Keystroke {
                unshifted_key: Some(base.into()),
                ..Keystroke::parse(&format!("cmd-{shifted}")).unwrap()
            };
            assert!(bindings.iter().any(|binding| binding.match_keystrokes(&[event.clone()]) == Some(false)));
        }
        // Layout-derived aliases, not a US punctuation lookup table.
        for (base, shifted) in [("=", "+"), ("-", "_"), ("ö", "Ö"), ("č", "Č"), ("7", "/")] {
            let event = Keystroke {
                unshifted_key: Some(base.into()),
                ..Keystroke::parse(&format!("cmd-{shifted}")).unwrap()
            };
            for keys in [format!("cmd-shift-{base}"), format!("cmd-{shifted}")] {
                let target = KeybindingKeystroke::from_keystroke(Keystroke::parse(&keys).unwrap());
                assert!(event.should_match(&target), "{keys}");
            }
        }
        assert!(Keystroke::parse("cmd-shift-]").unwrap().unshifted_key.is_none());
        assert!(Keystroke::default().unshifted_key.is_none());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_help_binding_matches_shifted_slash() {
        // Linux reports Ctrl+Shift+/ as Ctrl+? without the Shift modifier.
        let event = gpui::Keystroke::parse("ctrl-?").unwrap();
        let defaults: serde_json::Value = serde_json::from_str(super::DEFAULTS).unwrap();
        let help = defaults["global"].as_object().unwrap().iter()
            .filter(|(_, action)| action.as_str() == Some("app.help"))
            .map(|(keys, action)| (keys.clone(), action.clone()))
            .collect::<serde_json::Map<_, _>>();
        let bindings = super::parse(&serde_json::json!({ "global": help }).to_string()).unwrap();
        assert!(bindings.iter().any(|binding| binding.match_keystrokes(&[event.clone()]) == Some(false)));
    }

    // yuzora(L1): no default key switches layout (from the yuzora layout it lands in Herdr's own),
    // and a keys.json written earlier loses that one default binding but keeps any other.
    #[test]
    fn next_layout_has_no_default_key_and_the_old_one_is_dropped() {
        for keys in [include_str!("../defaults/keys.json"), include_str!("../defaults/keys.macos.json")] {
            let value: serde_json::Value = serde_json::from_str(keys).unwrap();
            for (section, bound) in value.as_object().unwrap() {
                for (key, action) in bound.as_object().unwrap() {
                    assert_ne!(action.as_str().unwrap().split(':').next(), Some("layout.switch"), "{section}.{key}");
                }
            }
        }
        let old = super::OLD_NEXT_LAYOUT_KEY;
        let count = |text: String| super::parse(&text).unwrap().len();
        let with = |bound: serde_json::Value| serde_json::json!({ "global": bound }).to_string();
        assert_eq!(count(with(serde_json::json!({ old: "layout.switch" }))), 0, "the old default is dropped");
        assert_eq!(count(with(serde_json::json!({ "ctrl-alt-l": "layout.switch" }))), 1, "another key stays");
        assert_eq!(count(with(serde_json::json!({ old: "layout.switch:main" }))), 1, "a named layout stays");
        let in_layout = serde_json::json!({ "layout:x": { old: "layout.switch" } }).to_string();
        assert_eq!(count(in_layout.clone()), 1, "only the global section is dropped");
        // The help page, the palette's key hints and the welcome list the same bindings.
        let listed = |text: &str| super::listing_of(text).into_iter().filter(|(_, _, a)| a == "layout.switch").count();
        assert_eq!(listed(&with(serde_json::json!({ old: "layout.switch" }))), 0, "not listed either");
        assert_eq!(listed(&in_layout), 1);
    }

    // yuzora(L1): the keys.json examples in the docs and in the guide users' agents read must load:
    // one wrong action (a `:` where the mod's command has a `.`) makes the whole file fail.
    // Every `"<modifier>-…": "<action>"` is matched on its own, so a stray quote or a missing
    // space elsewhere in the file can't hide one.
    #[test]
    fn keys_examples_in_the_docs_load() {
        let pair = fancy_regex::Regex::new(r#""((?:ctrl|alt|cmd|shift)-[^"\n]*)"\s*:\s*"([^"\n]*)""#).unwrap();
        // (file, text, examples that must be found in it)
        let docs = [
            ("defaults/AGENTS.md", include_str!("../defaults/AGENTS.md"), &[("ctrl-shift-t", "tab.new"), ("alt-n", "yuzora-shell-tabs.new-pane")][..]),
            ("docs/beta2/shell.md", include_str!("../../docs/beta2/shell.md"), &[("alt-n", "yuzora-shell-tabs.new-pane")][..]),
            ("defaults/herdr.d.ts", include_str!("../defaults/herdr.d.ts"), &[("alt-s", "<mod>.<command>")][..]),
        ];
        for (name, text, expected) in docs {
            let mut found = Vec::new();
            for caps in pair.captures_iter(text) {
                let caps = caps.unwrap();
                let (key, action) = (&caps[1], caps[2].replace("\\u001b", "\u{1b}"));
                let one = serde_json::json!({ "global": { key: action.clone() } });
                assert_eq!(super::parse(&one.to_string()).err(), None, "{name}: {key} -> {action}");
                found.push((key.to_owned(), action));
            }
            for (key, action) in expected {
                assert!(found.iter().any(|(k, a)| k == key && a == action), "{name}: example {key} -> {action} not found in {found:?}");
            }
        }
    }

    #[test]
    fn shipped_keys_load() {
        for keys in [include_str!("../defaults/keys.json"), include_str!("../defaults/keys.macos.json")] {
            assert_eq!(super::parse(keys).err(), None);
        }
    }
}
