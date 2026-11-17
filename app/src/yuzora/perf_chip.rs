//! The resource chip in every layout with a status bar (lane L8, docs/beta2/perf.md): where the window draws the
//! status bar's block, it draws the bar's own mod in the width that is left and a `yuzora-perf` copy at its right end.
//! The chip is no node of the layout, so `layout()`, the layout file, `.state` and every tree operation never see
//! it. A layout with no status bar shows no chip.

use crate::layout::{Content, Kind, Node, Size};
use crate::mods::Instance;
use gpui::{AnyElement, IntoElement, ParentElement, Styled, div};

/// The chip's mod.
pub const MOD: &str = "yuzora-perf";
/// A status bar is at most this tall in its layout file. herdr's own are 28 px (`main`) and 40 px (`win98`'s
/// taskbar); the root's default of 48 still reads as a bar, where a panel docked at the bottom is far taller.
const BAR_MAX: f32 = 48.0;
/// The chip's width at zoom 1: what `yuzora`'s status bar gives its `yuzora-perf` block.
const WIDTH: f32 = 150.0;

#[cfg(test)]
static OFF: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// A window test (in a process of its own) that compares with the window without the chip.
#[cfg(test)]
pub fn set_for_tests(on: bool) {
    OFF.store(!on, std::sync::atomic::Ordering::SeqCst);
}

/// The chip's copy among the window's copies, keyed to the status bar it is drawn in and told that bar's mod (it
/// paints as that bar does). None when `shown` (the tree on screen) has a `yuzora-perf` of its own, as a block or a
/// float, shown or hidden, or no shown status bar. `file` is the same layout as its file has it, without clicks.
pub fn slot(shown: &Node, file: &Node) -> Option<(String, Instance)> {
    #[cfg(test)]
    if OFF.load(std::sync::atomic::Ordering::SeqCst) {
        return None;
    }
    let chip = |c: &Content| matches!(c, Content::Mod { name, .. } if name == MOD);
    if shown.walk().iter().any(|n| n.content().is_some_and(chip) || n.floating.iter().any(|f| chip(&f.content))) {
        return None;
    }
    let bar = bar(shown, file)?;
    let Some(Content::Mod { name, .. }) = bar.content() else { return None };
    let key = key(&bar.id);
    // Node ids are free text: a node named like the slot keeps the slot to itself.
    if shown.find(&key).is_some() {
        return None;
    }
    Some((key, Instance { name: MOD.to_owned(), props: serde_json::json!({ "bar": name }) }))
}

/// The chip's slot at the block `id`, named as upstream names a mod slot that belongs to a node (`<stack>#header`).
pub fn key(id: &str) -> String {
    format!("{id}#{MOD}")
}

/// The status bar of `shown`: the last child of a column that holds the shown tab, below the tab's own branch, when
/// that child is a mod block of a fixed height of at most [`BAR_MAX`]. The nearest such column to the tab wins. The
/// height is the file's, so a click or a mod that resizes the bar (win98 fits its taskbar to the zoom) keeps it the
/// bar. None when the bar is hidden: the chip hides with it, and never moves to another bar.
fn bar<'a>(shown: &'a Node, file: &Node) -> Option<&'a Node> {
    let thin = |n: &Node| matches!(file.find(&n.id).unwrap_or(n).size, Some(Size::Px { px }) if px <= BAR_MAX);
    let path = to_tab(shown)?;
    let bar = path.windows(2).rev().find_map(|w| {
        let Kind::Column { children } = &w[0].kind else { return None };
        let last = children.last()?;
        (!std::ptr::eq(last, w[1]) && matches!(last.content(), Some(Content::Mod { .. })) && thin(last)).then_some(last)
    })?;
    (!bar.hidden).then_some(bar)
}

/// The shown nodes from `n` down to the shown tab block.
fn to_tab(n: &Node) -> Option<Vec<&Node>> {
    if n.hidden {
        return None;
    }
    if matches!(n.content(), Some(Content::Context { .. })) {
        return Some(vec![n]);
    }
    let below = match n.kind {
        Kind::Stack { .. } => n.active_child().and_then(to_tab),
        _ => n.children().iter().find_map(to_tab),
    }?;
    Some([vec![n], below].concat())
}

/// A mod block's element, with the chip's at its right end when the window draws one there. The bar's mod gets the
/// width that is left and lays itself out as in a narrower window; what it cannot fit is cut at the chip.
pub fn beside(block: AnyElement, chip: Option<AnyElement>) -> AnyElement {
    let Some(chip) = chip else { return block };
    div()
        .size_full()
        .flex()
        .child(div().flex_1().min_w_0().h_full().overflow_hidden().flex().child(block))
        .child(div().flex_none().w(crate::settings::z(WIDTH)).h_full().flex().child(chip))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn shown(text: &str, name: &str) -> Node {
        let mut n: Node = serde_json::from_str(text).unwrap();
        crate::yuzora::dock::inject(&mut n, name);
        n
    }

    fn tree(v: Value) -> Node {
        serde_json::from_value(v).unwrap()
    }

    /// The id of the block the chip is drawn in, for a layout with no clicks.
    fn found(n: &Node) -> Option<String> {
        slot(n, n).map(|(key, _)| key.strip_suffix(&format!("#{MOD}")).unwrap().to_owned())
    }

    fn tab() -> Value {
        json!({ "id": "main", "type": "block", "content": { "tab": "active" } })
    }

    /// A mod block `px` high.
    fn thin(id: &str, px: f32) -> Value {
        json!({ "id": id, "type": "block", "size": { "px": px }, "content": { "mod": "m" } })
    }

    fn column(children: Vec<Value>) -> Node {
        tree(json!({ "id": "root", "type": "column", "children": children }))
    }

    #[test]
    fn the_status_bar_of_each_shipped_layout() {
        let mut names = crate::window::shipped_layouts();
        names.sort();
        assert_eq!(names, ["main", "terminals", "win98", "yuzora"], "a new shipped layout says here where its chip goes");
        for (name, text, bar) in [
            ("main", include_str!("../../defaults/layouts/main.json"), Some("status")),
            ("terminals", include_str!("../../defaults/layouts/terminals.json"), None),
            ("win98", include_str!("../../defaults/layouts/win98.json"), Some("taskbar")),
            ("yuzora", include_str!("../../defaults/layouts/yuzora.json"), None),
        ] {
            assert_eq!(found(&shown(text, name)).as_deref(), bar, "{name}");
        }
        let main = shown(include_str!("../../defaults/layouts/main.json"), "main");
        let (key, copy) = slot(&main, &main).unwrap();
        assert_eq!((key.as_str(), copy.name.as_str(), &copy.props), ("status#yuzora-perf", MOD, &json!({ "bar": "workspace-status" })));
        let win98 = shown(include_str!("../../defaults/layouts/win98.json"), "win98");
        assert_eq!(slot(&win98, &win98).unwrap().1.props, json!({ "bar": "win98" }));
    }

    #[test]
    fn a_status_bar_is_a_thin_mod_block_last_in_a_column_that_holds_the_tab() {
        assert_eq!(found(&column(vec![tab(), thin("s", 28.0)])).as_deref(), Some("s"));
        assert_eq!(found(&column(vec![tab(), thin("s", 48.0)])).as_deref(), Some("s"), "48 px");
        assert_eq!(found(&column(vec![tab(), thin("s", 49.0)])), None, "taller is a panel");
        let weighted = json!({ "id": "s", "type": "block", "content": { "mod": "m" } });
        assert_eq!(found(&column(vec![tab(), weighted])), None, "no fixed height");
        let terminal = json!({ "id": "s", "type": "block", "size": { "px": 28 }, "content": { "terminal": "t1" } });
        assert_eq!(found(&column(vec![tab(), terminal])), None, "a terminal, not a mod");
        let stack = json!({ "id": "s", "type": "stack", "size": { "px": 28 }, "active": "a", "children": [thin("a", 28.0)] });
        assert_eq!(found(&column(vec![tab(), stack])), None, "a stack holds alternatives, not a bar");
        let row = json!({ "id": "s", "type": "row", "size": { "px": 28 }, "children": [thin("a", 28.0), thin("b", 28.0)] });
        assert_eq!(found(&column(vec![tab(), row])), None, "a row of blocks");
        assert_eq!(found(&column(vec![thin("title", 28.0), thin("toolbar", 32.0), tab()])), None, "bars at the top are not last");
        assert_eq!(found(&column(vec![tab(), thin("upper", 28.0), thin("lower", 24.0)])).as_deref(), Some("lower"), "two thin bars");
        let term = json!({ "id": "t", "type": "block", "size": { "px": 200 }, "content": { "terminal": "t1" } });
        assert_eq!(found(&column(vec![tab(), thin("s", 28.0), term])), None, "a terminal block below the bar is last");
        assert_eq!(found(&tree(json!({ "id": "root", "type": "row", "children": [tab(), thin("s", 28.0)] }))), None, "a row's end");
        assert_eq!(found(&column(vec![thin("a", 300.0), thin("b", 28.0)])), None, "no tab");
    }

    #[test]
    fn the_column_is_the_nearest_to_the_tab_with_a_bar_below_it() {
        let side = tree(json!({ "id": "root", "type": "row", "children": [
            { "id": "side", "type": "column", "size": { "px": 200 }, "children": [thin("list", 300.0), thin("footer", 28.0)] },
            tab()] }));
        assert_eq!(found(&side), None, "a sidebar's footer is in a column without the tab");
        let nested = json!({ "id": "root", "type": "column", "children": [
            { "id": "screen", "type": "row", "children": [{ "id": "work", "type": "column", "children": [tab(), thin("bar", 28.0)] }] },
            thin("taskbar", 40.0)] });
        assert_eq!(found(&tree(nested)).as_deref(), Some("bar"), "the column nearest the tab has its own bar");
        let thin_term = json!({ "id": "t", "type": "block", "size": { "px": 28 }, "content": { "terminal": "t1" } });
        for below in [None, Some(thin("panel", 300.0)), Some(thin_term)] {
            let mut work = vec![thin("tabs", 38.0), tab()];
            work.extend(below.clone());
            let n = tree(json!({ "id": "root", "type": "column", "children": [{ "id": "work", "type": "column", "children": work }, thin("status", 28.0)] }));
            assert_eq!(found(&n).as_deref(), Some("status"), "the nearest column ends with the tab, a panel or a terminal: {below:?}");
        }
        let stacked = |active: &str| tree(json!({ "id": "root", "type": "stack", "active": active, "children": [
            { "id": "a", "type": "column", "children": [tab(), thin("status", 28.0)] },
            { "id": "b", "type": "block", "content": { "mod": "m" } }] }));
        assert_eq!(found(&stacked("a")).as_deref(), Some("status"), "a stack showing the tab");
        assert_eq!(found(&stacked("b")), None, "a stack showing something else");
    }

    #[test]
    fn a_hidden_status_bar_hides_the_chip_and_a_resized_one_keeps_it() {
        let file = column(vec![tab(), thin("upper", 28.0), thin("status", 28.0)]);
        let mut hidden = file.clone();
        hidden.find_mut("status").unwrap().hidden = true;
        assert_eq!(slot(&hidden, &file), None, "hidden: no chip, and none in the bar above it");
        let mut resized = file.clone();
        resized.find_mut("status").unwrap().size = Some(Size::Px { px: 120.0 });
        assert_eq!(slot(&resized, &file).map(|s| s.0).as_deref(), Some("status#yuzora-perf"), "a click's size is not the file's");
        let mut no_tab = file.clone();
        no_tab.find_mut("main").unwrap().hidden = true;
        assert_eq!(slot(&no_tab, &file), None, "no shown tab");
    }

    #[test]
    fn a_layout_that_places_its_own_chip_gets_none() {
        let with = |chip: Value, floating: bool| {
            let mut n = json!({ "id": "root", "type": "column", "children": [tab(), thin("status", 28.0)] });
            if floating {
                n["floating"] = json!([chip]);
            } else {
                n["children"].as_array_mut().unwrap().insert(0, chip);
            }
            tree(n)
        };
        assert_eq!(found(&with(json!({ "id": "c", "type": "block", "content": { "mod": MOD } }), false)), None, "a block");
        assert_eq!(found(&with(json!({ "id": "c", "type": "block", "hidden": true, "content": { "mod": MOD } }), false)), None, "a hidden block");
        let float = json!({ "id": "c", "content": { "mod": MOD }, "edge": "bottom", "align": "end", "hidden": true });
        assert_eq!(found(&with(float, true)), None, "a hidden float");
        let other = json!({ "id": "c", "content": { "mod": "yuzora-perf-notes" } });
        assert_eq!(found(&with(other, true)).as_deref(), Some("status"), "only the chip's own mod");
    }

    #[test]
    fn a_node_named_like_the_chips_slot_keeps_it() {
        let with = |id: &str| column(vec![thin(id, 20.0), tab(), thin("status", 28.0)]);
        assert_eq!(found(&with("status#yuzora-perf")), None);
        assert_eq!(found(&with("status#other")).as_deref(), Some("status"));
    }

    #[test]
    fn the_dock_choice_does_not_decide_the_chip() {
        for choice in ["off", "drawer", "auto"] {
            let text = json!({ "id": "root", "type": "column", "yuzora": { "dock": choice }, "children": [tab(), thin("status", 28.0)] });
            assert_eq!(found(&shown(&text.to_string(), "mine")).as_deref(), Some("status"), "{choice}");
        }
    }
}
