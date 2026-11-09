//! The right dock's injection (yuzora, lane `dock`; docs/beta2/shell.md): puts
//! Yuzora's four modes (files, git, database, ssh) into every layout without
//! the layout file ever holding them.
//!
//! The rule the hooks in `window.rs` follow (each marked `yuzora(dock)`):
//! a tree read from a layout file is [`inject`]ed, a tree about to be written
//! to a layout file is [`strip`]ped. The injected nodes all have ids starting
//! with [`PREFIX`], reserved for this module, so the file stays exactly what
//! its author wrote and the dock's clicks (width, open, mode, drawer edge)
//! land in `layouts/.state/<name>.json` like any other click.
//!
//! Where the dock goes follows the layout's shape, not a list of names:
//! - [`Placed::Panel`]: a `panel` column (holding a `panel-body` stack) in a
//!   row: the modes join that stack and a 40px mode bar sits after `panel`.
//! - [`Placed::Drawer`]: anything else: a peeking float at the right edge.
//! - [`Placed::Own`]: the layout places Yuzora's modes itself (an old clone of
//!   `yuzora`): nothing is injected.
//! - [`Placed::Off`]: the dock is off (the root's `"yuzora": {"dock": "off"}`,
//!   or [`RIGHT_DOCK`] is false).

use crate::layout::{Align, Axis, Content, Edge, Float, Kind, Node, Size};
use serde_json::{Value, json};

/// The switch: while false, [`inject`] only strips and [`ensure`] does nothing,
/// so the app is what it was. On since the mode bar and drawer ship in
/// `yuzora-modebar` (PR-2).
pub const RIGHT_DOCK: bool = true;

/// Reserved: only the injection makes nodes whose ids start with this. A file
/// that has one has it ignored.
pub const PREFIX: &str = "yuzora-dock";
/// The modes, in the order they join the panel's stack. Each is the mod
/// `yuzora-sidebar-<mode>`, as node `yuzora-dock-<mode>`.
pub const MODES: [&str; 4] = ["files", "git", "database", "ssh"];
/// The mod showing the mode bar (panel) or the drawer, told apart by its `dock` prop.
pub const MODEBAR_MOD: &str = "yuzora-modebar";
/// The mods that already are modes (or their bar): a layout using one places them itself.
const OWN: [&str; 6] = [
    MODEBAR_MOD,
    "yuzora-sidebar-workspace",
    "yuzora-sidebar-files",
    "yuzora-sidebar-git",
    "yuzora-sidebar-database",
    "yuzora-sidebar-ssh",
];
/// The mode bar's width, px (min = max: it never resizes).
const BAR_PX: f32 = 40.0;
/// How much of the drawer shows at the edge until hovered, px.
const PEEK_PX: f32 = 40.0;

/// What [`inject`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placed {
    Panel,
    Drawer,
    Own,
    Off,
}

/// A layout's choice (root `"yuzora": {"dock": …}`); anything else is `auto`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Choice {
    Auto,
    Drawer,
    Off,
}

#[cfg(test)]
static FORCED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether the dock is injected: [`RIGHT_DOCK`], or a test of the window
/// (its own process) that turned it on.
fn enabled() -> bool {
    #[cfg(test)]
    if FORCED.load(std::sync::atomic::Ordering::SeqCst) {
        return true;
    }
    RIGHT_DOCK
}

/// Tests of a window turn the dock on for their process.
#[cfg(test)]
pub fn force_for_tests() {
    FORCED.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// Whether `id` is one the injection owns.
pub fn reserved(id: &str) -> bool {
    id.starts_with(PREFIX)
}

fn mod_of(c: &Content) -> Option<&str> {
    match c {
        Content::Mod { name, .. } => Some(name),
        _ => None,
    }
}

fn choice(n: &Node) -> Choice {
    match n.yuzora.as_ref().and_then(|y| y.get("dock")).and_then(Value::as_str) {
        Some("off") => Choice::Off,
        Some("drawer") => Choice::Drawer,
        _ => Choice::Auto,
    }
}

/// The layout (outside the reserved ids) already uses one of Yuzora's mode
/// mods, in a block or a float: an old clone of `yuzora`, an author's own.
fn provides_own(n: &Node) -> bool {
    n.walk().iter().any(|x| {
        (!reserved(&x.id) && x.content().and_then(mod_of).is_some_and(|m| OWN.contains(&m)))
            || x.floating.iter().any(|f| !reserved(&f.id) && mod_of(&f.content).is_some_and(|m| OWN.contains(&m)))
    })
}

/// A `panel` holding a `panel-body` stack, in a row: the modes go there, the bar after it.
fn panel_slot(n: &Node) -> bool {
    n.find("panel").is_some_and(|p| p.find("panel-body").is_some_and(|b| matches!(b.kind, Kind::Stack { .. })))
        && n.parent_of("panel").is_some_and(|p| p.axis() == Some(Axis::Row))
}

/// `n` without anything the injection put there. A stack that showed a
/// removed child shows what `fallback` (the tree as the file has it) chose.
pub fn strip(n: &Node, fallback: Option<&Node>) -> Node {
    fn go(x: &mut Node, fallback: Option<&Node>) {
        x.floating.retain(|f| !reserved(&f.id));
        let id = x.id.clone();
        if let Kind::Stack { children, active, .. } = &mut x.kind {
            children.retain(|c| !reserved(&c.id));
            if active.as_deref().is_some_and(reserved) {
                *active = fallback.and_then(|f| f.find(&id)).and_then(|f| match &f.kind {
                    Kind::Stack { active, .. } => active.clone(),
                    _ => None,
                });
            }
        }
        if let Some(children) = x.children_mut() {
            children.retain(|c| !reserved(&c.id));
            for c in children.iter_mut() {
                go(c, fallback);
            }
        }
    }
    let mut out = n.clone();
    go(&mut out, fallback);
    out
}

/// A tree read from the layout `_layout`'s file (or its shipped default):
/// puts the dock in. Off (see [`RIGHT_DOCK`]) it only strips. `_layout` is
/// there for the person's own choice per layout (a later step); today only
/// the layout's own `yuzora.dock` decides.
pub fn inject(n: &mut Node, _layout: &str) -> Placed {
    inject_with(n, enabled())
}

fn inject_with(n: &mut Node, on: bool) -> Placed {
    // A reserved id in a file is ignored: the dock is rebuilt, never doubled.
    *n = strip(n, None);
    let choice = choice(n);
    if !on || choice == Choice::Off {
        return Placed::Off;
    }
    if provides_own(n) {
        return Placed::Own;
    }
    if choice != Choice::Drawer && panel_slot(n) {
        if let Some(Node { kind: Kind::Stack { children, .. }, .. }) = n.find_mut("panel-body") {
            for m in MODES {
                children.push(Node::block(format!("{PREFIX}-{m}"), Content::Mod { name: format!("yuzora-sidebar-{m}"), props: Value::Null }));
            }
        }
        let parent = n.parent_of("panel").map(|p| p.id.clone()).unwrap_or_default();
        let row = n.find_mut(&parent).and_then(Node::children_mut).expect("panel_slot found a row holding panel");
        let at = row.iter().position(|c| c.id == "panel").expect("panel_slot found panel in that row");
        let mut bar = Node::block(format!("{PREFIX}-bar"), Content::Mod { name: MODEBAR_MOD.into(), props: json!({ "dock": "bar" }) });
        bar.size = Some(Size::Px { px: BAR_PX });
        bar.min = Some(BAR_PX);
        bar.max = Some(BAR_PX);
        row.insert(at + 1, bar);
        return Placed::Panel;
    }
    n.floating.push(Float {
        id: PREFIX.into(),
        content: Content::Mod { name: MODEBAR_MOD.into(), props: json!({ "dock": "drawer" }) },
        edge: Edge::Right,
        align: Align::Center,
        peek: Some(PEEK_PX),
        hidden: false,
    });
    Placed::Drawer
}

/// A tree a mod made (`layout.set`): the dock is already in it when the mod
/// started from `layout()`; a tree built from scratch gets it added.
/// Whatever is there is kept as it is, with the clicks it carries.
pub fn ensure(n: &mut Node, layout: &str) {
    ensure_with(n, layout, enabled());
}

fn ensure_with(n: &mut Node, _layout: &str, on: bool) {
    if !on {
        return;
    }
    let mut want = n.clone();
    inject_with(&mut want, true);
    if reserved_ids(n) != reserved_ids(&want) {
        *n = want;
    }
}

/// Every reserved node and float id in `n`, in tree order.
fn reserved_ids(n: &Node) -> Vec<String> {
    n.walk().iter().flat_map(|x| x.floating.iter().map(|f| f.id.clone()).chain([x.id.clone()])).filter(|id| reserved(id)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout_sync::{State, text_of, without_clicks};

    const MAIN: &str = include_str!("../../defaults/layouts/main.json");
    const TERMINALS: &str = include_str!("../../defaults/layouts/terminals.json");
    const WIN98: &str = include_str!("../../defaults/layouts/win98.json");
    const YUZORA: &str = include_str!("../../defaults/layouts/yuzora.json");
    /// `yuzora.json` as beta.3 shipped it: the modes in its own side bar.
    const YUZORA_BETA3: &str = include_str!("dock_fixtures/yuzora-beta3.json");
    /// The shape `yuzora.json` takes once its left side is the upstream's (spec appendix B).
    const YUZORA_NEXT: &str = include_str!("dock_fixtures/yuzora-pr3-draft.json");

    fn parse(text: &str) -> Node {
        serde_json::from_str(text).unwrap()
    }

    fn injected(text: &str) -> (Node, Node, Placed) {
        let file = parse(text);
        let mut n = file.clone();
        let placed = inject_with(&mut n, true);
        (file, n, placed)
    }

    fn active(n: &Node, stack: &str) -> Option<String> {
        match &n.find(stack)?.kind {
            Kind::Stack { active, .. } => active.clone(),
            _ => None,
        }
    }

    fn floats(n: &Node) -> Vec<&Float> {
        n.walk().into_iter().flat_map(|x| x.floating.iter()).collect()
    }

    /// What a person does to the dock: opens it on Git, widens it and hides the bar (panel), or drags the drawer to the other edge.
    fn click(chrome: &mut Node, placed: Placed) {
        match placed {
            Placed::Panel => {
                chrome.find_mut("panel").unwrap().hidden = false;
                chrome.find_mut("panel").unwrap().size = Some(Size::Px { px: 420.0 });
                assert!(chrome.select("panel-body", "yuzora-dock-git"));
                chrome.find_mut("yuzora-dock-bar").unwrap().hidden = true;
            }
            Placed::Drawer => {
                let f = chrome.find_float_mut(PREFIX).unwrap();
                f.edge = Edge::Left;
                f.align = Align::End;
            }
            _ => unreachable!("nothing to click"),
        }
    }

    #[test]
    fn every_shipped_layout_gets_the_dock() {
        // A shipped layout without a line here fails: say where its dock goes.
        let mut names: Vec<&str> = crate::window::shipped_layouts();
        names.sort();
        assert_eq!(names, ["main", "terminals", "win98", "yuzora"]);

        let (_, main, placed) = injected(MAIN);
        assert_eq!(placed, Placed::Panel);
        let body = main.find("panel-body").unwrap();
        let ids: Vec<&str> = body.children().iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["files", "changes", "yuzora-dock-files", "yuzora-dock-git", "yuzora-dock-database", "yuzora-dock-ssh"]);
        for m in MODES {
            let node = main.find(&format!("yuzora-dock-{m}")).unwrap();
            assert!(matches!(node.content(), Some(Content::Mod { name, .. }) if *name == format!("yuzora-sidebar-{m}")));
        }
        let row = main.parent_of("panel").unwrap();
        let ids: Vec<&str> = row.children().iter().map(|c| c.id.as_str()).collect();
        let at = ids.iter().position(|i| *i == "panel").unwrap();
        assert_eq!(ids[at + 1], "yuzora-dock-bar", "the bar sits right after the panel: {ids:?}");
        let bar = main.find("yuzora-dock-bar").unwrap();
        assert_eq!((bar.size, bar.min, bar.max), (Some(Size::Px { px: 40.0 }), Some(40.0), Some(40.0)));
        assert!(matches!(bar.content(), Some(Content::Mod { name, props }) if name == MODEBAR_MOD && props["dock"] == "bar"));
        assert!(floats(&main).is_empty());

        for text in [TERMINALS, WIN98] {
            let (_, n, placed) = injected(text);
            assert_eq!(placed, Placed::Drawer);
            let drawer = floats(&n).into_iter().find(|f| f.id == PREFIX).expect("the drawer");
            assert_eq!((drawer.edge, drawer.align, drawer.peek), (Edge::Right, Align::Center, Some(40.0)));
            assert!(n.floating.iter().any(|f| f.id == PREFIX), "the drawer floats over the root");
            assert!(matches!(&drawer.content, Content::Mod { name, props } if name == MODEBAR_MOD && props["dock"] == "drawer"));
            assert!(n.walk().iter().all(|x| !reserved(&x.id)), "no node, only the float");
        }

        // yuzora.json today places the modes in its own side bar: left alone (its rework makes it a Panel).
        assert_eq!(injected(YUZORA).2, Placed::Own);
        assert_eq!(injected(YUZORA_NEXT).2, Placed::Panel);
    }

    #[test]
    fn a_panel_that_is_not_in_a_row_gets_the_drawer() {
        let column = json!({ "id": "root", "type": "column", "children": [
            { "id": "main", "type": "block", "content": { "tab": "active" } },
            { "id": "panel", "type": "column", "size": { "px": 300 }, "children": [
                { "id": "panel-body", "type": "stack", "children": [{ "id": "files", "type": "block", "content": { "mod": "files" } }] } ] } ] });
        let (_, n, placed) = injected(&column.to_string());
        assert_eq!(placed, Placed::Drawer);
        assert!(n.find("yuzora-dock-bar").is_none() && n.find("yuzora-dock-git").is_none());
        // A panel that isn't a stack holder is no slot either.
        let plain = json!({ "id": "root", "type": "row", "children": [
            { "id": "main", "type": "block", "content": { "tab": "active" } },
            { "id": "panel", "type": "column", "children": [{ "id": "panel-body", "type": "block", "content": { "mod": "files" } }] } ] });
        assert_eq!(injected(&plain.to_string()).2, Placed::Drawer);
    }

    #[test]
    fn strip_undoes_inject_and_inject_is_idempotent() {
        for text in [MAIN, TERMINALS, WIN98, YUZORA, YUZORA_NEXT] {
            let (file, n, placed) = injected(text);
            assert_eq!(strip(&n, None), file);
            let mut again = n.clone();
            assert_eq!(inject_with(&mut again, true), placed);
            assert_eq!(again, n, "a tree read back from the injected one is the same");
        }
    }

    #[test]
    fn off_changes_nothing() {
        for text in [MAIN, TERMINALS, WIN98, YUZORA, YUZORA_BETA3] {
            let file = parse(text);
            let mut n = file.clone();
            assert_eq!(inject_with(&mut n, false), Placed::Off);
            assert_eq!(n, file, "inject(F) == F");
            assert_eq!(strip(&file, Some(&file)), file, "strip(F) == F");
            // What a save would write for it (nothing edited) is the file's text.
            assert_eq!(text_of(&strip(&without_clicks(&file, &n), Some(&file))), text_of(&file));
            let mut shown = file.clone();
            ensure_with(&mut shown, "x", false);
            assert_eq!(shown, file, "ensure does nothing while off");
        }
    }

    #[test]
    fn the_switch_the_app_runs_with_is_on() {
        // main and a layout with no panel get the dock through the public entry the hooks call; yuzora keeps its own modes.
        for (text, want) in [(MAIN, Placed::Panel), (TERMINALS, Placed::Drawer), (WIN98, Placed::Drawer), (YUZORA, Placed::Own)] {
            let mut n = parse(text);
            assert_eq!(inject(&mut n, "x"), want);
            let mut shown = n.clone();
            ensure(&mut shown, "x");
            assert_eq!(shown, n, "a tree that has the dock keeps it");
        }
        let mut bare = parse(MAIN);
        ensure(&mut bare, "main");
        assert!(bare.find("yuzora-dock-bar").is_some(), "a tree built without the dock gets it");
    }

    #[test]
    fn dock_clicks_are_state_not_edits() {
        for text in [MAIN, TERMINALS] {
            let (_, file_chrome, placed) = injected(text);
            let mut chrome = file_chrome.clone();
            click(&mut chrome, placed);
            let state = State::between(&file_chrome, &chrome);
            let on_dock = state.nodes.keys().chain(state.floats.keys()).filter(|id| reserved(id)).count();
            assert_eq!(on_dock, 1, "the dock's own node (the bar, the drawer) keeps its click: {state:?}");
            assert_eq!(without_clicks(&file_chrome, &chrome), file_chrome, "so saving writes no layout file");
        }
        // Injecting only into the shown tree (the file's tree staying as the file has it) would lose them.
        let (file, chrome_only, placed) = injected(MAIN);
        let mut chrome = chrome_only.clone();
        click(&mut chrome, placed);
        let lost = State::between(&file, &chrome);
        assert!(!lost.nodes.keys().chain(lost.floats.keys()).any(|id| reserved(id)), "{lost:?}");
        assert_ne!(without_clicks(&file, &chrome), file, "and every save would count as an edit");
    }

    #[test]
    fn a_saved_edit_leaves_the_dock_out_of_the_file() {
        for text in [MAIN, TERMINALS] {
            let (_, file_chrome, placed) = injected(text);
            let mut chrome = file_chrome.clone();
            click(&mut chrome, placed);
            let state = State::between(&file_chrome, &chrome);
            // A mod docked a block: a real edit.
            let mut shown = chrome.clone();
            shown.children_mut().unwrap().push(Node::block("docked", Content::Terminal { terminal: "t9".into() }));
            let edited = without_clicks(&file_chrome, &shown);
            assert_ne!(edited, file_chrome);
            assert!(text_of(&edited).contains("yuzora-dock"), "unstripped, the dock would reach the file");
            let written = text_of(&strip(&edited, Some(&file_chrome)));
            assert!(!written.contains("yuzora-dock") && written.contains("docked"));
            // Restart: the file is read and injected again; the saved clicks still land.
            let mut saved = state.clone();
            saved.file = serde_json::to_value(&edited).ok();
            let saved: State = serde_json::from_str(&serde_json::to_string(&saved).unwrap()).unwrap();
            let mut reread = parse(&written);
            inject_with(&mut reread, true);
            let mut s = saved;
            s.forget_since_saved(&reread);
            let mut again = reread.clone();
            s.apply(&mut again);
            match placed {
                Placed::Panel => {
                    assert!(!again.find("panel").unwrap().hidden);
                    assert_eq!(again.find("panel").unwrap().size, Some(Size::Px { px: 420.0 }));
                    assert_eq!(active(&again, "panel-body").as_deref(), Some("yuzora-dock-git"));
                    assert!(again.find("yuzora-dock-bar").unwrap().hidden);
                }
                _ => {
                    let f = floats(&again).into_iter().find(|f| f.id == PREFIX).unwrap();
                    assert_eq!((f.edge, f.align), (Edge::Left, Align::End));
                }
            }
        }
    }

    #[test]
    fn layout_set_strips_and_puts_the_stack_back() {
        let (file, mut chrome, placed) = injected(MAIN);
        click(&mut chrome, placed);
        assert_eq!(active(&chrome, "panel-body").as_deref(), Some("yuzora-dock-git"));
        let stripped = strip(&chrome, Some(&file));
        assert_eq!(active(&stripped, "panel-body"), active(&file, "panel-body"));
        assert_eq!(active(&stripped, "panel-body").as_deref(), Some("files"));
        assert!(!text_of(&stripped).contains("yuzora-dock"));
        // Without the file's tree to ask the stack has no choice (it shows its first child).
        assert_eq!(active(&strip(&chrome, None), "panel-body"), None);
    }

    #[test]
    fn ensure_keeps_what_is_there_and_adds_what_is_missing() {
        let (file, mut chrome, placed) = injected(MAIN);
        click(&mut chrome, placed);
        let mut kept = chrome.clone();
        ensure_with(&mut kept, "main", true);
        assert_eq!(kept, chrome, "a tree that has the dock keeps it, clicks and all");
        let mut bare = file.clone();
        ensure_with(&mut bare, "main", true);
        assert_eq!(strip(&bare, None), file);
        assert!(bare.find("yuzora-dock-bar").is_some());
        // The drawer too, and a dock the tree opted out of stays out.
        let mut win = parse(WIN98);
        ensure_with(&mut win, "win98", true);
        assert!(win.floating.iter().any(|f| f.id == PREFIX));
        let mut off = parse(&MAIN.replacen("\"id\": \"root\",", "\"id\": \"root\", \"yuzora\": { \"dock\": \"off\" },", 1));
        let before = off.clone();
        ensure_with(&mut off, "main", true);
        assert_eq!(off, before);
    }

    #[test]
    fn the_opt_out_round_trips_and_is_honored() {
        let with = |dock: &str| MAIN.replacen("\"id\": \"root\",", &format!("\"id\": \"root\", \"yuzora\": {{ \"dock\": \"{dock}\" }},"), 1);
        for dock in ["off", "drawer", "auto"] {
            let n = parse(&with(dock));
            assert!(text_of(&n).contains("\"yuzora\""), "a rewrite keeps the field ({dock})");
            assert_eq!(parse(&text_of(&n)), n);
        }
        let (_, n, placed) = injected(&with("off"));
        assert_eq!(placed, Placed::Off);
        assert!(reserved_ids(&n).is_empty());
        let (file, n, placed) = injected(&with("drawer"));
        assert_eq!(placed, Placed::Drawer, "main, with a panel, takes the drawer when asked");
        assert!(n.find("yuzora-dock-bar").is_none() && n.floating.iter().any(|f| f.id == PREFIX));
        assert_eq!(strip(&n, None), file);
        // `auto`, or a word nobody knows, is the default.
        assert_eq!(injected(&with("auto")).2, Placed::Panel);
        assert_eq!(injected(&with("sideways")).2, Placed::Panel);
        // No field at all: the shipped files don't carry one.
        assert!(parse(MAIN).yuzora.is_none());
        assert!(!text_of(&parse(MAIN)).contains("\"yuzora\""));
    }

    #[test]
    fn a_layout_that_places_the_modes_itself_is_left_alone() {
        let (file, n, placed) = injected(YUZORA_BETA3);
        assert_eq!(placed, Placed::Own);
        assert_eq!(n, file);
        // A mode (or the bar) in a float counts too, and so does a bare bar.
        let floaty = json!({ "id": "root", "type": "block", "content": { "tab": "active" },
            "floating": [{ "id": "mine", "content": { "mod": "yuzora-sidebar-git" } }] });
        assert_eq!(injected(&floaty.to_string()).2, Placed::Own);
        let bar = json!({ "id": "root", "type": "row", "children": [{ "id": "bar", "type": "block", "content": { "mod": "yuzora-modebar" } }] });
        assert_eq!(injected(&bar.to_string()).2, Placed::Own);
        // The dock's own nodes don't count as the layout's: injecting twice stays a Panel.
        let (_, once, _) = injected(MAIN);
        let mut twice = once.clone();
        assert_eq!(inject_with(&mut twice, true), Placed::Panel);
    }

    #[test]
    fn reserved_ids_in_a_file_are_injected_once() {
        let mut file = parse(MAIN);
        file.find_mut("panel-body").unwrap().children_mut().unwrap().push(Node::block("yuzora-dock-git", Content::Mod { name: "mine".into(), props: Value::Null }));
        file.floating.push(Float {
            id: PREFIX.into(),
            content: Content::Mod { name: "mine".into(), props: Value::Null },
            edge: Edge::Left,
            align: Align::Start,
            peek: None,
            hidden: false,
        });
        let mut n = file.clone();
        assert_eq!(inject_with(&mut n, true), Placed::Panel);
        let ids = reserved_ids(&n);
        assert_eq!(ids.iter().filter(|i| *i == "yuzora-dock-git").count(), 1, "{ids:?}");
        assert!(!ids.iter().any(|i| i == PREFIX), "the hand-written drawer is gone");
        assert!(matches!(n.find("yuzora-dock-git").unwrap().content(), Some(Content::Mod { name, .. }) if name == "yuzora-sidebar-git"));
    }
}
