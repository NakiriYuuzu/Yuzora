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
use crate::layout_sync::State;
use serde_json::{Value, json};

/// The switch: while false, [`inject`] and [`ensure`] do nothing (only a save
/// and `layout.set` strip), so the app is what it was. On since the mode bar
/// and drawer ship in `yuzora-modebar` (PR-2).
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
/// The `settings.json` section holding the person's dock choices.
pub const SHELL: &str = "shell";
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
    id == PREFIX || id.starts_with("yuzora-dock-")
}

fn mod_of(c: &Content) -> Option<&str> {
    match c {
        Content::Mod { name, .. } => Some(name),
        _ => None,
    }
}

fn word(s: &str) -> Choice {
    match s {
        "off" => Choice::Off,
        "drawer" => Choice::Drawer,
        _ => Choice::Auto,
    }
}

fn choice(n: &Node) -> Choice {
    word(n.yuzora.as_ref().and_then(|y| y.get("dock")).and_then(Value::as_str).unwrap_or_default())
}

/// What the person chose for the layout `name` in Yuzora's settings
/// (`shell.dock.layouts.<name>`: `auto`, `drawer` or `off`). Nothing, or a
/// word nobody knows, is no choice: the layout's own field decides.
fn personal_in(shell: &Value, name: &str) -> Option<Choice> {
    let word = shell.get("dock")?.get("layouts")?.get(name)?.as_str()?;
    matches!(word, "auto" | "drawer" | "off").then(|| self::word(word))
}

fn personal(name: &str) -> Option<Choice> {
    // A test that did not turn the dock on (so it runs in the shared process, with the developer's real home) never reads their settings.
    #[cfg(test)]
    if !FORCED.load(std::sync::atomic::Ordering::SeqCst) {
        return None;
    }
    personal_in(&yuzora_core::settings::section(SHELL), name)
}

/// The layout already uses one of Yuzora's mode mods, in a block or a float:
/// an old clone of `yuzora`, an author's own. (Called on a stripped tree: the
/// dock's own nodes are never there to count.)
fn provides_own(n: &Node) -> bool {
    n.walk().iter().any(|x| {
        x.content().and_then(mod_of).is_some_and(|m| OWN.contains(&m)) || x.floating.iter().any(|f| mod_of(&f.content).is_some_and(|m| OWN.contains(&m)))
    })
}

/// The first row with a `panel` column of its own (holding a `panel-body`
/// stack): the modes join that stack, the bar goes right after the panel.
/// Found and changed in one walk, by position and never by id, so a tree with
/// repeated or missing ids can't send it somewhere else. False (nothing
/// touched) when no row has one.
fn place_panel(x: &mut Node) -> bool {
    if x.axis() == Some(Axis::Row)
        && let Some(children) = x.children_mut()
        && let Some(at) = children.iter().position(|c| c.id == "panel" && matches!(c.find("panel-body"), Some(Node { kind: Kind::Stack { .. }, .. })))
        && let Some(Node { kind: Kind::Stack { children: modes, .. }, .. }) = children[at].find_mut("panel-body")
    {
        for m in MODES {
            modes.push(Node::block(format!("{PREFIX}-{m}"), Content::Mod { name: format!("yuzora-sidebar-{m}"), props: Value::Null }));
        }
        let mut bar = Node::block(format!("{PREFIX}-bar"), Content::Mod { name: MODEBAR_MOD.into(), props: json!({ "dock": "bar" }) });
        bar.size = Some(Size::Px { px: BAR_PX });
        bar.min = Some(BAR_PX);
        bar.max = Some(BAR_PX);
        children.insert(at + 1, bar);
        return true;
    }
    x.children_mut().is_some_and(|cs| cs.iter_mut().any(place_panel))
}

/// `n` without anything the injection put there. A stack that showed a
/// removed child shows what `fallback` (the tree as the file has it) chose.
pub fn strip(n: &Node, fallback: Option<&Node>) -> Node {
    fn go(x: &mut Node, fallback: Option<&Node>) {
        x.floating.retain(|f| !reserved(&f.id));
        let id = x.id.clone();
        if let Kind::Stack { active, .. } = &mut x.kind
            && active.as_deref().is_some_and(reserved)
        {
            *active = fallback
                .and_then(|f| f.find(&id))
                .and_then(|f| match &f.kind {
                    Kind::Stack { active, .. } => active.clone(),
                    _ => None,
                })
                .filter(|a| !reserved(a));
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
/// puts the dock in. Off (see [`RIGHT_DOCK`]) it leaves the tree alone. The
/// person's choice for the layout (settings, `shell.dock.layouts.<layout>`)
/// beats the layout's own `yuzora.dock`, which beats `auto`.
pub fn inject(n: &mut Node, layout: &str) -> Placed {
    inject_with(n, enabled(), personal(layout))
}

fn inject_with(n: &mut Node, on: bool, personal: Option<Choice>) -> Placed {
    // Off, the tree is the file's own, whatever ids it uses.
    if !on {
        return Placed::Off;
    }
    // A reserved id in a file is ignored: the dock is rebuilt, never doubled.
    *n = strip(n, None);
    let choice = personal.unwrap_or_else(|| choice(n));
    if choice == Choice::Off {
        return Placed::Off;
    }
    if provides_own(n) {
        return Placed::Own;
    }
    if choice != Choice::Drawer && place_panel(n) {
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

/// What a tree written to a layout file reads back as, shown: [`strip`]ped
/// (as the file gets it), then [`inject`]ed (as a load shows it). What
/// `file_chrome` must hold after a write, so the dock's clicks stay clicks.
pub fn as_read(n: &Node, layout: &str, fallback: Option<&Node>) -> Node {
    as_read_with(n, fallback, enabled(), personal(layout))
}

fn as_read_with(n: &Node, fallback: Option<&Node>, on: bool, personal: Option<Choice>) -> Node {
    let mut read = strip(n, fallback);
    inject_with(&mut read, on, personal);
    read
}

/// A tree a mod made (`layout.set`): the dock is already in it when the mod
/// started from `layout()`; a tree built from scratch gets it added.
/// Whatever is there is kept as it is, with the clicks it carries.
/// `fallback` (the tree as the file has it) is what a stack that showed a
/// removed dock node goes back to when the dock is rebuilt.
pub fn ensure(n: &mut Node, layout: &str, fallback: Option<&Node>) {
    ensure_with(n, enabled(), personal(layout), fallback);
}

fn ensure_with(n: &mut Node, on: bool, personal: Option<Choice>, fallback: Option<&Node>) {
    if !on {
        return;
    }
    let mut want = strip(n, fallback);
    inject_with(&mut want, true, personal);
    if reserved_ids(n) != reserved_ids(&want) {
        // The stacks keep what they showed, where that child is still there.
        for x in n.walk() {
            if let Kind::Stack { active: Some(a), .. } = &x.kind {
                want.select(&x.id, a);
            }
        }
        *n = want;
    }
}

/// The clicks of `before` (the layout's `.state` as written) that belong to a
/// dock part `chrome` doesn't show now: the person chose `off` or `drawer` for
/// this layout since, and the clicks on the part that isn't shown (the panel's
/// mode, the bar, the drawer's place) must outlive that until it is back. A
/// click `state` already has (made on what shows now) is not replaced. A panel
/// the person opened themselves meanwhile (`state` shows it open, and no dock
/// mode is in the chrome to have opened it) is theirs: the mode doesn't come
/// back over their Files.
pub fn keep_clicks(state: &mut State, before: &State, chrome: &Node) {
    let shown = reserved_ids(chrome);
    let own_panel = state.nodes.get("panel").and_then(|p| p.hidden) == Some(false);
    let gone = |id: &str| reserved(id) && !shown.iter().any(|s| s == id);
    for (id, click) in &before.nodes {
        if gone(id) {
            state.nodes.entry(id.clone()).or_insert_with(|| click.clone());
        } else if let Some(mode) = click.active.as_ref().filter(|a| gone(a))
            && chrome.find(id).is_some()
            && !own_panel
        {
            state.nodes.entry(id.clone()).or_default().active.get_or_insert_with(|| mode.clone());
            // The panel was opened for that mode: its visibility goes with it (see `without_panel_click`).
            if let Some(hidden) = before.nodes.get("panel").and_then(|p| p.hidden) {
                state.nodes.entry("panel".into()).or_default().hidden.get_or_insert(hidden);
            }
        }
    }
    for (id, click) in &before.floats {
        if gone(id) {
            state.floats.entry(id.clone()).or_insert_with(|| click.clone());
        }
    }
}

/// `state` as it shows on `chrome`: with the dock not in the panel (off, a
/// drawer, a layout's own), the panel's visibility click that was made for a
/// dock mode (the stack still names one) is not shown, so the layout's own panel
/// doesn't appear beside the drawer or under `off`. The click stays in the state
/// (written as it is) for the dock's return.
pub fn without_panel_click(state: &State, chrome: &Node) -> State {
    let mut s = state.clone();
    if chrome.find(&format!("{PREFIX}-bar")).is_none()
        && s.nodes.values().any(|c| c.active.as_deref().is_some_and(reserved))
        && let Some(p) = s.nodes.get_mut("panel")
    {
        p.hidden = None;
    }
    s
}

/// Whether a kept window `spec` is the drawer's (key [`PREFIX`]) while `chrome`
/// has no drawer: the person chose `off` or `auto` since, so it doesn't reopen.
pub fn stale_window(spec: &Value, chrome: &Node) -> bool {
    spec.get("key").and_then(Value::as_str) == Some(PREFIX) && !chrome.walk().iter().any(|n| n.floating.iter().any(|f| f.id == PREFIX))
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

    fn parse(text: &str) -> Node {
        serde_json::from_str(text).unwrap()
    }

    fn injected(text: &str) -> (Node, Node, Placed) {
        let file = parse(text);
        let mut n = file.clone();
        let placed = inject_with(&mut n, true, None);
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

        // yuzora.json's left side is the upstream's, so the modes go to the right: a Panel, like main.
        let (_, yuzora, placed) = injected(YUZORA);
        assert_eq!(placed, Placed::Panel);
        assert!(yuzora.find("yuzora-dock-bar").is_some() && yuzora.find("yuzora-dock-git").is_some());
        // The beta.3 file places the modes itself: left alone.
        assert_eq!(injected(YUZORA_BETA3).2, Placed::Own);
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
        for text in [MAIN, TERMINALS, WIN98, YUZORA] {
            let (file, n, placed) = injected(text);
            assert_eq!(strip(&n, None), file);
            let mut again = n.clone();
            assert_eq!(inject_with(&mut again, true, None), placed);
            assert_eq!(again, n, "a tree read back from the injected one is the same");
        }
    }

    #[test]
    fn off_changes_nothing() {
        for text in [MAIN, TERMINALS, WIN98, YUZORA, YUZORA_BETA3] {
            let file = parse(text);
            let mut n = file.clone();
            assert_eq!(inject_with(&mut n, false, None), Placed::Off);
            assert_eq!(n, file, "inject(F) == F");
            assert_eq!(strip(&file, Some(&file)), file, "strip(F) == F");
            // What a save would write for it (nothing edited) is the file's text.
            assert_eq!(text_of(&strip(&without_clicks(&file, &n), Some(&file))), text_of(&file));
            let mut shown = file.clone();
            ensure_with(&mut shown, false, None, None);
            assert_eq!(shown, file, "ensure does nothing while off");
        }
    }

    #[test]
    fn the_switch_the_app_runs_with_is_on() {
        // main and a layout with no panel get the dock with the switch the hooks run under (no personal choice: tests never read the real settings); yuzora (the new one) is a Panel and a beta.3 clone keeps its own modes.
        for (text, want) in [(MAIN, Placed::Panel), (TERMINALS, Placed::Drawer), (WIN98, Placed::Drawer), (YUZORA, Placed::Panel), (YUZORA_BETA3, Placed::Own)] {
            let mut n = parse(text);
            assert_eq!(inject_with(&mut n, enabled(), None), want);
            let mut shown = n.clone();
            ensure_with(&mut shown, enabled(), None, None);
            assert_eq!(shown, n, "a tree that has the dock keeps it");
        }
        let mut bare = parse(MAIN);
        ensure_with(&mut bare, enabled(), None, None);
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
            inject_with(&mut reread, true, None);
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
        ensure_with(&mut kept, true, None, None);
        assert_eq!(kept, chrome, "a tree that has the dock keeps it, clicks and all");
        let mut bare = file.clone();
        ensure_with(&mut bare, true, None, None);
        assert_eq!(strip(&bare, None), file);
        assert!(bare.find("yuzora-dock-bar").is_some());
        // The drawer too, and a dock the tree opted out of stays out.
        let mut win = parse(WIN98);
        ensure_with(&mut win, true, None, None);
        assert!(win.floating.iter().any(|f| f.id == PREFIX));
        let mut off = parse(&MAIN.replacen("\"id\": \"root\",", "\"id\": \"root\", \"yuzora\": { \"dock\": \"off\" },", 1));
        let before = off.clone();
        ensure_with(&mut off, true, None, None);
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

    // T9: the person's choice (settings) beats the layout's field, which beats auto.
    #[test]
    fn a_persons_choice_beats_the_layout_field() {
        let with = |dock: &str| MAIN.replacen("\"id\": \"root\",", &format!("\"id\": \"root\", \"yuzora\": {{ \"dock\": \"{dock}\" }},"), 1);
        let placed = |text: &str, personal: Option<Choice>| {
            let mut n = parse(text);
            inject_with(&mut n, true, personal)
        };
        // Neither says anything: auto.
        assert_eq!(placed(MAIN, None), Placed::Panel);
        // The person says off or drawer over a layout that says auto (or nothing).
        assert_eq!(placed(MAIN, Some(Choice::Off)), Placed::Off);
        assert_eq!(placed(&with("auto"), Some(Choice::Off)), Placed::Off);
        assert_eq!(placed(MAIN, Some(Choice::Drawer)), Placed::Drawer);
        // The person says auto over a layout that says off or drawer.
        assert_eq!(placed(&with("off"), None), Placed::Off);
        assert_eq!(placed(&with("off"), Some(Choice::Auto)), Placed::Panel);
        assert_eq!(placed(&with("drawer"), Some(Choice::Auto)), Placed::Panel);
        // A layout with no panel stays a drawer under auto, and off still wins.
        assert_eq!(placed(WIN98, Some(Choice::Auto)), Placed::Drawer);
        assert_eq!(placed(WIN98, Some(Choice::Off)), Placed::Off);
        // The choice reaches what a mod's layout.set and a save's read-back make, not only a load.
        let mut bare = parse(MAIN);
        ensure_with(&mut bare, true, Some(Choice::Off), None);
        assert!(reserved_ids(&bare).is_empty(), "off: a tree built without the dock stays without");
        let mut shown = parse(MAIN);
        inject_with(&mut shown, true, None);
        ensure_with(&mut shown, true, Some(Choice::Off), None);
        assert!(reserved_ids(&shown).is_empty(), "off: a tree that had the dock loses it, as a load would");
        let file = parse(MAIN);
        assert!(reserved_ids(&as_read_with(&shown, Some(&file), true, Some(Choice::Off))).is_empty());
        assert!(as_read_with(&file, Some(&file), true, Some(Choice::Drawer)).floating.iter().any(|f| f.id == PREFIX));
    }

    // The clicks on the dock outlive the time the person has it off (or as a drawer): a save then keeps them.
    #[test]
    fn clicks_on_a_dock_part_that_is_not_shown_are_kept() {
        for (text, placed) in [(MAIN, Placed::Panel), (TERMINALS, Placed::Drawer)] {
            let (file, shown, _) = injected(text);
            let mut chrome = shown.clone();
            click(&mut chrome, placed);
            let before = State::between(&shown, &chrome);
            assert!(!before.nodes.is_empty() || !before.floats.is_empty());
            // Off: what shows is the file; a save recomputes the clicks and would lose them.
            let mut state = State::between(&file, &file);
            assert!(state.nodes.is_empty() && state.floats.is_empty());
            keep_clicks(&mut state, &before, &file);
            // The panel's own width is the file's node: the save finds it again by itself. Its visibility goes with the mode it was opened for.
            let mut dock_parts = before.nodes.clone();
            dock_parts.remove("panel");
            if let Some(hidden) = before.nodes.get("panel").and_then(|p| p.hidden) {
                dock_parts.entry("panel".into()).or_default().hidden = Some(hidden);
            }
            assert_eq!(state.nodes, dock_parts, "{text:.20}");
            assert_eq!(state.floats, before.floats);
            // Shown, nothing is carried: the tree's own clicks are the whole truth.
            let mut state = State::between(&shown, &chrome);
            let again = state.clone();
            keep_clicks(&mut state, &State::default(), &chrome);
            assert_eq!(state, again);
            keep_clicks(&mut state, &before, &chrome);
            assert_eq!(state, again);
        }
        // A click made on what shows now (the file's own Changes) is not replaced by the old one on the dock's Git.
        let (file, shown, _) = injected(MAIN);
        let mut chrome = shown.clone();
        click(&mut chrome, Placed::Panel);
        let before = State::between(&shown, &chrome);
        let mut now = file.clone();
        now.select("panel-body", "changes");
        let mut state = State::between(&file, &now);
        keep_clicks(&mut state, &before, &now);
        assert_eq!(state.nodes["panel-body"].active.as_deref(), Some("changes"));
        // Known limit: the file's own default child (Files) picked meanwhile is no click (the stack is as the
        // file has it), so it can't be told from a stack nobody touched: the dock's mode comes back.
        let now = file.clone();
        let mut state = State::between(&file, &now);
        keep_clicks(&mut state, &before, &now);
        assert_eq!(state.nodes["panel-body"].active.as_deref(), Some("yuzora-dock-git"));
    }

    // The panel opened for a dock mode does not show without the dock in it; with the bar there it does.
    #[test]
    fn a_panel_opened_for_the_dock_is_not_shown_without_it() {
        let (file, shown, _) = injected(MAIN);
        let mut chrome = shown.clone();
        click(&mut chrome, Placed::Panel);
        let before = State::between(&shown, &chrome);
        assert_eq!(before.nodes["panel"].hidden, Some(false));
        assert_eq!(without_panel_click(&before, &shown), before, "the bar is there");
        let plain = without_panel_click(&before, &file);
        assert_eq!(plain.nodes["panel"].hidden, None);
        assert_eq!(plain.nodes["panel-body"], before.nodes["panel-body"]);
        // The person's own Files panel (the stack names no dock mode) stays (a save keeps it too: see below).
        let mut files = State::default();
        files.nodes.entry("panel".into()).or_default().hidden = Some(false);
        assert_eq!(without_panel_click(&files, &file), files);
        // Carried with the mode while the dock is off, so its return finds the panel open.
        let mut state = State::between(&file, &file);
        keep_clicks(&mut state, &before, &file);
        assert_eq!(state.nodes["panel"].hidden, Some(false));
        // But not over a panel the person opened themselves meanwhile (their Files): the mode stays out of it.
        let mut state = files.clone();
        keep_clicks(&mut state, &before, &file);
        assert_eq!(state, files);
    }

    #[test]
    fn the_settings_name_a_choice_per_layout() {
        let shell = json!({ "dock": { "layouts": { "win98": "off", "main": "drawer", "odd": "sideways", "num": 5, "auto": "auto" } } });
        assert_eq!(personal_in(&shell, "win98"), Some(Choice::Off));
        assert_eq!(personal_in(&shell, "main"), Some(Choice::Drawer));
        assert_eq!(personal_in(&shell, "auto"), Some(Choice::Auto));
        // A word nobody knows, a non-word, a layout not named, no section: no choice (the layout's field decides).
        for (shell, name) in [(&shell, "odd"), (&shell, "num"), (&shell, "terminals"), (&Value::Null, "main"), (&json!({ "dock": 3 }), "main"), (&json!({ "dock": { "layouts": [] } }), "main")] {
            assert_eq!(personal_in(shell, name), None, "{name}");
        }
        // A name is a key, never a path: a slash or a tilde in it is just part of the name.
        let odd = json!({ "dock": { "layouts": { "a/b": "off" } } });
        assert_eq!(personal_in(&odd, "a/b"), Some(Choice::Off));
        assert_eq!(personal_in(&odd, "a"), None);
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
        // Strip runs first, so the dock's own mode nodes never count as the layout's: injecting twice stays a Panel.
        let (_, once, _) = injected(MAIN);
        let mut twice = once.clone();
        assert_eq!(inject_with(&mut twice, true, None), Placed::Panel);
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
        assert_eq!(inject_with(&mut n, true, None), Placed::Panel);
        let ids = reserved_ids(&n);
        assert_eq!(ids.iter().filter(|i| *i == "yuzora-dock-git").count(), 1, "{ids:?}");
        assert!(!ids.iter().any(|i| i == PREFIX), "the hand-written drawer is gone");
        assert!(matches!(n.find("yuzora-dock-git").unwrap().content(), Some(Content::Mod { name, .. }) if name == "yuzora-sidebar-git"));
    }

    #[test]
    fn a_beta3_state_file_meets_the_new_yuzora_layout_without_harm() {
        // What a beta.3 user's .state/yuzora.json holds: the old side bar's stack on Git, a dragged
        // side bar, the panel opened. The ids that are gone are skipped; the ones that remain apply.
        let state: State = serde_json::from_value(json!({ "nodes": {
            "yuzora-sidebar": { "active": "yuzora-sidebar-git" },
            "yuzora-modebar": { "hidden": true },
            "sidebar": { "size": { "px": 300.0 } },
            "panel": { "hidden": false },
        } })).unwrap();
        let (file, mut n, placed) = injected(YUZORA);
        assert_eq!(placed, Placed::Panel);
        let before = n.clone();
        state.apply(&mut n);
        assert!(n.find("yuzora-sidebar").is_none() && n.find("yuzora-modebar").is_none());
        assert!(!n.find("panel").unwrap().hidden, "a click on a node that is still there applies");
        assert_eq!(n.find("sidebar").unwrap().size, Some(Size::Px { px: 300.0 }));
        // Nothing else moved, and the file stays free of the dock when it is written back.
        n.find_mut("panel").unwrap().hidden = true;
        n.find_mut("sidebar").unwrap().size = before.find("sidebar").unwrap().size;
        assert_eq!(n, before);
        assert_eq!(strip(&n, None), file);
    }

    // S1: the reserved ids are `yuzora-dock` and `yuzora-dock-…`, nothing else.
    #[test]
    fn a_persons_node_that_only_starts_like_the_dock_is_theirs() {
        assert!(reserved(PREFIX) && reserved("yuzora-dock-git") && reserved("yuzora-dock-bar"));
        assert!(!reserved("yuzora-docker") && !reserved("yuzora-dockview") && !reserved("yuzora-doc"));
        let mut file = parse(MAIN);
        file.children_mut().unwrap().push(Node::block("yuzora-docker", Content::Terminal { terminal: "t1".into() }));
        file.floating.push(Float { id: "yuzora-dockview".into(), content: Content::Terminal { terminal: "t2".into() }, edge: Edge::Left, align: Align::Start, peek: None, hidden: false });
        // Off: the tree is the file's, to the byte, and a save writes it back whole.
        let mut off = file.clone();
        assert_eq!(inject_with(&mut off, false, None), Placed::Off);
        assert_eq!(off, file);
        assert_eq!(strip(&off, Some(&file)), file);
        // Even a hand-written reserved id is left alone while off (nothing is injected, so nothing is the dock's to take out).
        let mut hand = file.clone();
        hand.find_mut("panel-body").unwrap().children_mut().unwrap().push(Node::block("yuzora-dock-git", Content::Terminal { terminal: "t3".into() }));
        let mut off_hand = hand.clone();
        assert_eq!(inject_with(&mut off_hand, false, None), Placed::Off);
        assert_eq!(off_hand, hand);
        // On: still theirs, next to the dock.
        let mut on = file.clone();
        assert_eq!(inject_with(&mut on, true, None), Placed::Panel);
        assert!(on.find("yuzora-docker").is_some() && on.floating.iter().any(|f| f.id == "yuzora-dockview"));
        assert_eq!(strip(&on, None), file);
    }

    // B1: a stack never keeps the dock's mode as its choice, even from the fallback.
    #[test]
    fn strip_never_falls_back_to_a_reserved_choice() {
        let (_, mut chrome, placed) = injected(MAIN);
        click(&mut chrome, placed);
        // The fallback tree itself shows the dock's mode (what file_chrome held before the fix).
        let stripped = strip(&chrome, Some(&chrome));
        assert_eq!(active(&stripped, "panel-body"), None);
        assert!(!text_of(&stripped).contains("yuzora-dock"));
    }

    #[test]
    fn as_read_is_what_a_load_of_the_written_file_shows() {
        let (file, mut chrome, placed) = injected(MAIN);
        click(&mut chrome, placed);
        let read = as_read_with(&chrome, Some(&file), true, None);
        assert_eq!(read, injected(&text_of(&strip(&chrome, Some(&file)))).1);
        assert_eq!(active(&read, "panel-body").as_deref(), Some("files"));
        assert!(read.find("yuzora-dock-bar").is_some());
        // As what it is called with again, it stays put.
        assert_eq!(as_read_with(&read, Some(&read), true, None), read);
    }

    fn row_with_panel(id: &str, tail: Vec<Value>) -> Value {
        let mut kids = vec![json!({ "id": "main", "type": "block", "content": { "tab": "active" } }), json!({ "id": "panel", "type": "column", "children": [
            { "id": "panel-body", "type": "stack", "children": [{ "id": "files", "type": "block", "content": { "mod": "files" } }] }] })];
        kids.extend(tail);
        json!({ "id": id, "type": "row", "children": kids })
    }

    // B2: the slot is found by position, never by looking an id up again.
    #[test]
    fn a_tree_with_repeated_or_missing_ids_is_never_a_panic() {
        // The row holding the panel shares its id with an earlier row.
        let dup = json!({ "id": "root", "type": "row", "children": [
            { "id": "r", "type": "row", "children": [{ "id": "a", "type": "block", "content": { "tab": "active" } }] },
            { "id": "c", "type": "column", "children": [row_with_panel("r", vec![])] }] });
        let (_, n, placed) = injected(&dup.to_string());
        assert_eq!(placed, Placed::Panel);
        let row = n.walk().into_iter().filter(|x| x.id == "r").nth(1).unwrap();
        let ids: Vec<&str> = row.children().iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["main", "panel", "yuzora-dock-bar"], "the bar is in the row that holds the panel");
        assert!(n.walk().into_iter().find(|x| x.id == "r").unwrap().children().iter().all(|c| c.id != "yuzora-dock-bar"));
        // No ids at all but the panel's.
        let bare = json!({ "type": "row", "children": [{ "type": "column", "children": [row_with_panel("", vec![])] }] });
        let mut n: Node = serde_json::from_value(bare).unwrap();
        assert_eq!(inject_with(&mut n, true, None), Placed::Panel);
        let mut again: Node = serde_json::from_value(serde_json::to_value(strip(&n, None)).unwrap()).unwrap();
        ensure_with(&mut again, true, None, None);
        assert!(again.find("yuzora-dock-bar").is_some());
        // Several panel-bodies: the modes join the one inside the panel.
        let many = json!({ "id": "root", "type": "row", "children": [
            { "id": "early", "type": "column", "children": [{ "id": "panel-body", "type": "stack", "children": [] }] },
            { "id": "main", "type": "block", "content": { "tab": "active" } },
            { "id": "panel", "type": "column", "children": [{ "id": "panel-body", "type": "stack", "children": [] }] }] });
        let (_, n, placed) = injected(&many.to_string());
        assert_eq!(placed, Placed::Panel);
        assert!(n.find("early").unwrap().find("panel-body").unwrap().children().is_empty(), "the other stack is untouched");
        assert_eq!(n.find("panel").unwrap().find("panel-body").unwrap().children().len(), MODES.len());
        // A first panel-body that is a block (not in the panel) changes nothing about the panel.
        let odd = json!({ "id": "root", "type": "row", "children": [
            { "id": "panel-body", "type": "block", "content": { "mod": "x" } },
            { "id": "panel", "type": "column", "children": [{ "id": "panel-body", "type": "stack", "children": [] }] }] });
        let (_, n, placed) = injected(&odd.to_string());
        assert_eq!(placed, Placed::Panel);
        assert_eq!(n.find("panel").unwrap().children()[0].children().len(), MODES.len());
        // The panel in a column, not in any row: the drawer.
        let col = json!({ "id": "root", "type": "column", "children": [row_with_panel("r", vec![]).pointer("/children/1").cloned().unwrap()] });
        assert_eq!(injected(&col.to_string()).2, Placed::Drawer);
    }

    /// Every input the injection, strip and ensure get, a malformed tree included, is answered without a panic.
    #[test]
    fn nothing_the_dock_is_given_makes_it_panic() {
        let mut seed: u64 = 0x2545_F491_4F6C_DD1D;
        let mut next = move |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % n
        };
        // Half the trees draw from ids with the dock's among them, half from ids without (those are what the invariants below run on).
        const CLEAN: [&str; 6] = ["", "root", "r", "panel", "panel-body", "x"];
        const IDS: [&str; 9] = ["", "root", "r", "panel", "panel-body", "yuzora-dock", "yuzora-dock-git", "yuzora-dock-bar", "x"];
        fn rand_tree(next: &mut impl FnMut(u64) -> u64, ids: &[&str], depth: u32) -> Value {
            let id = ids[next(ids.len() as u64) as usize];
            let mut node = match next(if depth == 0 { 1 } else { 4 }) {
                0 => json!({ "type": "block", "content": { "mod": if next(3) == 0 { "yuzora-sidebar-git" } else { "m" } } }),
                k => {
                    let kids: Vec<Value> = (0..next(4)).map(|_| rand_tree(next, ids, depth - 1)).collect();
                    match k {
                        // A third of the rows hold a panel, so the panel placement is what gets run.
                        1 if next(3) == 0 => {
                            let body = json!({ "id": "panel-body", "type": "stack", "children": kids, "active": ids[next(ids.len() as u64) as usize] });
                            json!({ "type": "row", "children": [{ "type": "block", "content": { "tab": "active" } }, { "id": "panel", "type": "column", "children": [body] }] })
                        }
                        1 => json!({ "type": "row", "children": kids }),
                        2 => json!({ "type": "column", "children": kids }),
                        _ => json!({ "type": "stack", "children": kids, "active": ids[next(ids.len() as u64) as usize] }),
                    }
                }
            };
            if !id.is_empty() || next(2) == 0 {
                node["id"] = json!(id);
            }
            if next(5) == 0 {
                node["floating"] = json!([{ "id": ids[next(ids.len() as u64) as usize], "content": { "mod": "m" } }]);
            }
            if next(6) == 0 {
                let dock = [json!("off"), json!("drawer"), json!("auto"), json!(5)][next(4) as usize].clone();
                node["yuzora"] = json!({ "dock": dock });
            }
            node
        }
        let (mut parsed, mut panels, mut checked) = (0, 0, 0);
        for i in 0..3000 {
            let tree = rand_tree(&mut next, if i % 2 == 0 { &IDS } else { &CLEAN }, 4);
            let Ok(node) = serde_json::from_value::<Node>(tree) else { continue };
            parsed += 1;
            let shown = |n: &Node| serde_json::to_string(n).unwrap();
            // What the dock promises a file that never held it (no reserved id, no stack pointing at one), repeated or missing ids included.
            let points_at_dock = node.walk().iter().any(|x| matches!(&x.kind, Kind::Stack { active: Some(a), .. } if reserved(a)));
            // A reserved id in the file is ignored: what is shown is what a file without it shows.
            let mut once = node.clone();
            inject_with(&mut once, true, None);
            let mut clean = strip(&node, None);
            inject_with(&mut clean, true, None);
            assert_eq!(once, clean, "inject(F) == inject(strip(F)): seed 0x2545_F491_4F6C_DD1D, tree {}", shown(&node));
            let mut twice = once.clone();
            inject_with(&mut twice, true, None);
            assert_eq!(twice, once, "inject is idempotent: seed 0x2545_F491_4F6C_DD1D, tree {}", shown(&node));
            if reserved_ids(&node).is_empty() && !points_at_dock {
                checked += 1;
                let mut shown_tree = node.clone();
                let placed = inject_with(&mut shown_tree, true, None);
                panels += usize::from(placed == Placed::Panel);
                let ctx = |what: &str| format!("{what}: seed 0x2545_F491_4F6C_DD1D, tree {}", shown(&node));
                assert_eq!(strip(&shown_tree, None), node, "{}", ctx("strip(inject(F)) == F"));
                let mut ensured = shown_tree.clone();
                ensure_with(&mut ensured, true, None, None);
                assert_eq!(ensured, shown_tree, "{}", ctx("ensure(inject(F)) == inject(F)"));
            }
            for (on, personal) in [(true, None), (false, None), (true, Some(Choice::Auto)), (true, Some(Choice::Drawer)), (true, Some(Choice::Off))] {
                let mut a = node.clone();
                inject_with(&mut a, on, personal);
                inject_with(&mut a, on, personal);
                let mut b = node.clone();
                ensure_with(&mut b, on, personal, None);
                let _ = strip(&a, Some(&b));
                let _ = strip(&b, None);
                let _ = as_read_with(&a, Some(&node), on, personal);
            }
        }
        assert!(parsed > 1000, "the generator makes trees: {parsed}");
        assert!(checked > 200 && panels > 50, "the invariants run on a good share of them, panels too: {checked} checked, {panels} panels");
    }
}
