//! Lane L1's glue in the app: the pane frame's `blocked` border, and a person's
//! new Space starting in a folder they pick. The rest of the shell is mods
//! (`defaults/mods/yuzora-*`) and `crates/yuzora-shell`.

use crate::layout::{Line, PaneFrame};
use serde_json::{Value, json};

/// A pane's frame line: the layout's `blocked` one while its agent needs a
/// reply (it wins over focus: the person is wanted there), else `line`.
pub fn frame_line<'a>(f: &'a PaneFrame, line: Option<&'a Line>, status: Option<&str>) -> Option<&'a Line> {
    f.blocked.as_ref().filter(|_| status == Some("blocked")).or(line)
}

/// The dialog that asks for a new Space's folder.
pub const NEW_SPACE_MOD: &str = "yuzora-new-space";

/// A person's space.new that says nothing but (maybe) the machine: keys, the
/// palette, menus, "+" buttons. It asks for a folder first, as beta.1 did.
/// One that brings a name, folder, tab, agent or task (worktrees, ssh, the
/// dialog itself) runs as it is.
pub fn asks_folder(args: &Value) -> bool {
    args.as_object().is_none_or(|a| a.iter().all(|(k, v)| k == "machine" || v.is_null()))
}

/// The folder dialog for a new Space on `machine` (None: this computer).
pub fn new_space_dialog(machine: Option<String>) -> Value {
    json!({ "mod": NEW_SPACE_MOD, "kind": "dialog", "width": 440, "props": { "machine": machine } })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{Content, HeaderContent, Node};

    fn shipped() -> Node {
        serde_json::from_str(include_str!("../../defaults/layouts/yuzora.json")).unwrap()
    }

    fn frame(root: &Node) -> PaneFrame {
        root.walk()
            .into_iter()
            .find_map(|n| match n.content() {
                Some(Content::Context { pane, .. }) => pane.clone(),
                _ => None,
            })
            .expect("the yuzora layout shows the active tab")
    }

    #[test]
    fn a_blocked_agent_takes_the_amber_border() {
        let f = frame(&shipped());
        let focus = f.focus.as_ref();
        assert_eq!(f.blocked.as_ref().map(|l| l.color.as_str()), Some("#8a6420"));
        assert_eq!(focus.map(|l| l.color.as_str()), Some("ring"));
        assert_eq!(frame_line(&f, focus, Some("blocked")).map(|l| l.color.as_str()), Some("#8a6420"));
        assert_eq!(frame_line(&f, focus, Some("working")).map(|l| l.color.as_str()), Some("ring"));
        assert_eq!(frame_line(&f, None, None), None);
        // Another layout without `blocked` keeps its own look.
        let plain = PaneFrame { border: f.border.clone(), ..PaneFrame::default() };
        assert_eq!(frame_line(&plain, plain.border.as_ref(), Some("blocked")), plain.border.as_ref());
    }

    #[test]
    fn the_blocked_line_round_trips_and_stays_out_when_unset() {
        let f = frame(&shipped());
        let json = serde_json::to_value(&f).unwrap();
        assert_eq!(json["blocked"]["color"], "#8a6420");
        assert!(serde_json::to_value(PaneFrame::default()).unwrap().get("blocked").is_none());
    }

    #[test]
    fn the_layout_uses_the_shells_mods_and_every_one_ships() {
        let root = shipped();
        let mut mods: Vec<String> = root
            .walk()
            .into_iter()
            .filter_map(|n| match n.content() {
                Some(Content::Mod { name, .. }) => Some(name.clone()),
                _ => None,
            })
            .collect();
        match frame(&root).header.map(|h| h.content) {
            Some(HeaderContent::Mod { name, .. }) => mods.push(name),
            other => panic!("pane header: {other:?}"),
        }
        for want in ["yuzora-shell-tabs", "yuzora-shell-pane", "yuzora-statusbar", "yuzora-modebar", "yuzora-sidebar-workspace"] {
            assert!(mods.iter().any(|m| m == want), "{want} is in the layout");
        }
        let defaults = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("defaults/mods");
        for m in &mods {
            assert!(defaults.join(m).join("main.js").is_file(), "{m} ships in defaults/mods");
        }
    }

    #[test]
    fn only_a_bare_new_space_asks_for_its_folder() {
        // Keys and the palette (nothing or {}), menus and "+" buttons (a machine, "" for this one).
        for bare in [Value::Null, json!({}), json!({ "machine": "" }), json!({ "machine": "lab-01" }), json!({ "machine": null, "name": null })] {
            assert!(asks_folder(&bare), "{bare} asks");
        }
        // Worktrees, ssh sessions, scripts naming it and the dialog itself bring what they want.
        for full in [
            json!({ "folder": "/srv/app", "machine": "lab-01" }),
            json!({ "machine": "", "argv": ["ssh", "box"], "tab": "box" }),
            json!({ "name": "notes" }),
            json!({ "task": "fix it" }),
            json!({ "name": "app", "folder": "/srv/app", "machine": "" }),
        ] {
            assert!(!asks_folder(&full), "{full} runs as it is");
        }
    }

    #[test]
    fn the_folder_dialog_ships_and_takes_the_machine() {
        let spec = new_space_dialog(Some("lab-01".into()));
        assert_eq!(spec["mod"], NEW_SPACE_MOD);
        assert_eq!(spec["kind"], "dialog");
        assert_eq!(spec["props"]["machine"], "lab-01");
        assert_eq!(new_space_dialog(None)["props"]["machine"], Value::Null);
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("defaults/mods").join(NEW_SPACE_MOD);
        assert!(dir.join("main.js").is_file() && dir.join("mod.json").is_file());
    }
}
