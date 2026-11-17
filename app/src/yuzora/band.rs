//! The resource chip in every layout (lane L8, docs/beta2/perf.md): under the layout's root the window draws a
//! band holding one `yuzora-perf` copy, unless the layout places the chip itself. The band is no node of the
//! layout, so `layout()`, the layout file, `.state` and every tree operation never see it.

use crate::layout::{Content, Node};
use crate::mods::Instance;
use gpui::{AnyElement, IntoElement, ParentElement, Styled, div};

/// The chip's mod.
pub const MOD: &str = "yuzora-perf";
/// The band's copy among the window's copies, which node ids also name (`:` keeps it apart, as `float:` does).
pub const SLOT: &str = "yuzora-band:perf";
/// The band's height at zoom 1: `yuzora`'s status bar.
const PX: f32 = 26.0;

#[cfg(test)]
static OFF: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Tests of a window (each in a process of its own) that count copies without the band, or compare with it.
#[cfg(test)]
pub fn set_for_tests(on: bool) {
    OFF.store(!on, std::sync::atomic::Ordering::SeqCst);
}

/// The band's slot and copy while `shown` (the tree on screen) has no chip of its own: no block or float, shown or
/// hidden, holds `yuzora-perf`. The copy is told it is in the band.
pub fn slot(shown: &Node) -> Option<(String, Instance)> {
    #[cfg(test)]
    if OFF.load(std::sync::atomic::Ordering::SeqCst) {
        return None;
    }
    let chip = |c: &Content| matches!(c, Content::Mod { name, .. } if name == MOD);
    let placed = shown.walk().iter().any(|n| n.content().is_some_and(chip) || n.floating.iter().any(|f| chip(&f.content)));
    (!placed).then(|| (SLOT.to_owned(), Instance { name: MOD.to_owned(), props: serde_json::json!({ "band": true }) }))
}

/// The window's layout with `band`, the band copy's element when there is one, under it.
pub fn under(layout: AnyElement, band: Option<AnyElement>) -> AnyElement {
    let Some(band) = band else { return layout };
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(div().flex_1().min_h_0().flex().child(layout))
        .child(div().flex_none().h(crate::settings::z(PX)).w_full().flex().child(band))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shown(text: &str, name: &str) -> Node {
        let mut n: Node = serde_json::from_str(text).unwrap();
        crate::yuzora::dock::inject(&mut n, name);
        n
    }

    fn tree(v: serde_json::Value) -> Node {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn every_shipped_layout_but_yuzora_gets_the_band() {
        let mut names = crate::window::shipped_layouts();
        names.sort();
        assert_eq!(names, ["main", "terminals", "win98", "yuzora"], "a new shipped layout says here whether it gets the band");
        for (name, text, band) in [
            ("main", include_str!("../../defaults/layouts/main.json"), true),
            ("terminals", include_str!("../../defaults/layouts/terminals.json"), true),
            ("win98", include_str!("../../defaults/layouts/win98.json"), true),
            ("yuzora", include_str!("../../defaults/layouts/yuzora.json"), false),
        ] {
            assert_eq!(slot(&shown(text, name)).is_some(), band, "{name}");
        }
        let (key, copy) = slot(&shown(include_str!("../../defaults/layouts/main.json"), "main")).unwrap();
        assert_eq!((key.as_str(), copy.name.as_str(), &copy.props), (SLOT, MOD, &serde_json::json!({ "band": true })));
    }

    #[test]
    fn a_chip_the_layout_places_itself_drops_the_band() {
        let block = tree(serde_json::json!({ "id": "root", "type": "column", "children": [
            { "id": "main", "type": "block", "content": { "tab": "active" } },
            { "id": "mine", "type": "block", "hidden": true, "content": { "mod": "yuzora-perf" } } ] }));
        assert!(slot(&block).is_none(), "a block, hidden too");
        let stacked = tree(serde_json::json!({ "id": "root", "type": "stack", "active": "main", "children": [
            { "id": "main", "type": "block", "content": { "tab": "active" } },
            { "id": "mine", "type": "block", "content": { "mod": "yuzora-perf", "props": { "any": 1 } } } ] }));
        assert!(slot(&stacked).is_none(), "a stack child not shown, with props of its own");
        let float = tree(serde_json::json!({ "id": "root", "type": "block", "content": { "terminal": "t1" },
            "floating": [{ "id": "corner", "content": { "mod": "yuzora-perf" }, "edge": "bottom", "align": "end", "hidden": true }] }));
        assert!(slot(&float).is_none(), "a float, hidden too");
        let other = tree(serde_json::json!({ "id": "root", "type": "block", "content": { "mod": "yuzora-perf-notes" } }));
        assert!(slot(&other).is_some(), "only the chip's own mod");
    }

    #[test]
    fn the_band_does_not_follow_the_dock_choice() {
        for choice in ["off", "drawer", "auto"] {
            let n = shown(&serde_json::json!({ "id": "root", "type": "block", "yuzora": { "dock": choice }, "content": { "terminal": "t1" } }).to_string(), "mine");
            assert!(slot(&n).is_some(), "{choice}");
        }
    }
}
