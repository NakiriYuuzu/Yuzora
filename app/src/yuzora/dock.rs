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
//!
//! The resource chip (`yuzora-perf`) goes in the same way, into every layout
//! that doesn't show it, the dock on or off: beside the status bar under the
//! tab, else at the bottom of the window ([`place_perf`]).

use crate::layout::{Align, Axis, Content, Edge, Float, Kind, Node, Size};
use crate::layout_sync::{State, without_clicks};
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
/// The resource chip's mod, and the node that shows it.
const PERF_MOD: &str = "yuzora-perf";
const PERF: &str = "yuzora-dock-perf";
/// What a node showed before the chip went beside it (see [`beside`]).
const BODY: &str = "yuzora-dock-body";
/// A root's id once its own went to its body (see [`place_perf`]).
const ROOT: &str = "yuzora-dock-root";
/// The chip's width beside a status bar, and its bar's height at the bottom of a window, px.
const PERF_WIDE: f32 = 150.0;
const PERF_TALL: f32 = 26.0;
/// The tallest bar under the tab that counts as a status bar, px.
const STATUS_MAX: f32 = 32.0;

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

/// Whether a block or a float of `n` shows the resource chip.
fn shows_perf(n: &Node) -> bool {
    n.walk().iter().any(|x| x.content().and_then(mod_of) == Some(PERF_MOD) || x.floating.iter().any(|f| mod_of(&f.content) == Some(PERF_MOD)))
}

/// Puts the resource chip into a layout that doesn't show it: beside the
/// status bar (a mod block of at most [`STATUS_MAX`], last in the column that
/// holds the tab), else at the bottom of the root. Found by position, as in
/// [`place_panel`]. The sizes that decide are the layout file's, never a
/// click's: `file`'s (the tree as the file has it, when `n` carries clicks),
/// else `n`'s own (`n` is the file). So the window and the next load of the
/// file always put the chip in the same place.
fn place_perf(n: &mut Node, file: Option<&Node>) {
    if shows_perf(n) {
        return;
    }
    let sized = file.map(|f| without_clicks(f, n));
    let bar = status_bar(sized.as_ref().unwrap_or(n));
    if let Some(bar) = bar.and_then(|at| node_at_mut(n, &at)) {
        beside(bar, Axis::Row, PERF_WIDE);
    } else if let Kind::Column { children } = &mut n.kind {
        children.push(perf_block(PERF_TALL));
    } else {
        beside(n, Axis::Column, PERF_TALL);
        // The root fills the window, so no click on it is a size: its id goes with what it showed, which is what an id
        // finds (a stack to select, a terminal's pane, a mod's copy).
        let id = std::mem::replace(&mut n.id, ROOT.into());
        if let Some(body) = n.children_mut().and_then(|c| c.first_mut()) {
            body.id = if reserved(&id) { String::new() } else { id };
        }
    }
}

/// The path (child indices) to the status bar under the tab, if there is one.
fn status_bar(n: &Node) -> Option<Vec<usize>> {
    fn to_tab(x: &Node) -> Option<Vec<usize>> {
        if matches!(x.content(), Some(Content::Context { .. })) {
            return Some(Vec::new());
        }
        x.children().iter().enumerate().find_map(|(i, c)| to_tab(c).map(|p| [vec![i], p].concat()))
    }
    let tab = to_tab(n)?;
    let above = (0..tab.len()).rev().find(|&k| node_at(n, &tab[..k]).and_then(Node::axis) == Some(Axis::Column))?;
    let column = node_at(n, &tab[..above])?;
    let last = column.children().len().checked_sub(1)?;
    let bar = &column.children()[last];
    let thin = matches!(bar.size, Some(Size::Px { px }) if px <= STATUS_MAX);
    if !thin || !matches!(bar.content(), Some(Content::Mod { .. })) {
        return None;
    }
    let mut at = tab[..above].to_vec();
    at.push(last);
    Some(at)
}

fn node_at<'a>(n: &'a Node, at: &[usize]) -> Option<&'a Node> {
    at.iter().try_fold(n, |x, &i| x.children().get(i))
}

fn node_at_mut<'a>(n: &'a mut Node, at: &[usize]) -> Option<&'a mut Node> {
    at.iter().try_fold(n, |x, &i| x.children_mut()?.get_mut(i))
}

fn perf_block(px: f32) -> Node {
    let mut chip = Node::block(PERF, Content::Mod { name: PERF_MOD.into(), props: Value::Null });
    chip.size = Some(Size::Px { px });
    chip
}

/// Shows the chip beside `x` along `axis`. `x` keeps its place: its id, size
/// and look, so the clicks on it (a status bar hidden or resized) still mean
/// the whole of it, and a root keeps the fields only a root has. What `x`
/// showed, with how it lays out its own children, moves into a child
/// [`BODY`]. [`take_body_back`] undoes it.
fn beside(x: &mut Node, axis: Axis, px: f32) {
    let mut body = Node::new(BODY, std::mem::replace(&mut x.kind, Kind::Row { children: Vec::new() }));
    body.gap = x.gap.take();
    body.divider = x.divider.take();
    body.resizable = x.resizable.replace(false);
    body.window_drag = std::mem::take(&mut x.window_drag);
    let children = vec![body, perf_block(px)];
    x.kind = match axis {
        Axis::Row => Kind::Row { children },
        Axis::Column => Kind::Column { children },
    };
}

/// [`beside`] undone: a node holding a body (a [`BODY`], or the one child of a
/// [`ROOT`] without a reserved id) and nothing else but what [`strip`] drops
/// shows the body itself again. Anything else beside a body (a mod's
/// `layout.set` put it there), a node that has a field [`beside`] moves (a
/// reserved id a person wrote), or a body with a field [`beside`] doesn't give
/// it (a mod's edit) leaves the node as it is, and [`strip`] keeps the body
/// without its reserved id.
fn take_body_back(x: &mut Node) {
    let rooted = x.id == ROOT;
    let left = (x.gap, &x.divider, x.resizable, x.window_drag) == (None, &None, Some(false), false);
    let Some(children) = x.children_mut() else { return };
    let is_body = |c: &Node| left && bare(c) && if rooted { !reserved(&c.id) } else { c.id == BODY };
    if children.iter().filter(|c| is_body(c)).count() != 1 || !children.iter().all(|c| is_body(c) || dropped(c)) {
        return;
    }
    let Some(body) = std::mem::take(children).into_iter().find(is_body) else { return };
    if rooted {
        x.id = body.id;
    }
    x.kind = body.kind;
    x.gap = body.gap;
    x.divider = body.divider;
    x.resizable = body.resizable;
    x.window_drag = body.window_drag;
}

/// Whether `c` has only what [`beside`] gives a body, so taking it back loses nothing.
fn bare(c: &Node) -> bool {
    let mut plain = Node::new(c.id.clone(), c.kind.clone());
    (plain.gap, plain.divider, plain.resizable, plain.window_drag) = (c.gap, c.divider.clone(), c.resizable, c.window_drag);
    plain == *c
}

/// Whether [`strip`] drops `c`: a reserved node that holds nothing (the chip,
/// the bar, a mode, a block a person gave a reserved id). A body, a root and
/// any reserved node with children hold what a person or a mod put there.
fn dropped(c: &Node) -> bool {
    reserved(&c.id) && c.id != BODY && c.id != ROOT && c.children().is_empty()
}

/// `n` without anything the injection put there. A stack that showed a
/// removed child shows what `fallback` (the tree as the file has it) chose.
/// Every node without a reserved id stays, wherever a mod moved a wrapper
/// (`yuzora-dock-root` under a row it was wrapped in included): a wrapper is
/// undone where it is, and a reserved node that holds something stays, its id
/// left empty.
pub fn strip(n: &Node, fallback: Option<&Node>) -> Node {
    fn go(x: &mut Node, fallback: Option<&Node>) {
        x.floating.retain(|f| !reserved(&f.id));
        take_body_back(x);
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
            children.retain(|c| !dropped(c));
            for c in children.iter_mut() {
                go(c, fallback);
            }
        }
        if reserved(&x.id) {
            x.id.clear();
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
    inject_sized(n, on, personal, None)
}

/// [`inject_with`], the chip placed by the sizes `file` has for the nodes it has (see [`place_perf`]).
fn inject_sized(n: &mut Node, on: bool, personal: Option<Choice>, file: Option<&Node>) -> Placed {
    // Off, the tree is the file's own, whatever ids it uses.
    if !on {
        return Placed::Off;
    }
    // A reserved id in a file is ignored: the dock is rebuilt, never doubled.
    *n = strip(n, None);
    place_perf(n, file);
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
    // The chip goes where the layout's file, as it is after this set, puts it: a shipped layout's file stays (what the
    // tree changes are clicks); a person's becomes the tree (written, then read back by `as_read`).
    let file = if crate::window::shipped_layouts().contains(&layout) { fallback } else { None };
    ensure_with(n, enabled(), personal(layout), fallback, file);
}

/// [`ensure`], the chip placed by `file`'s sizes, or by `n`'s own when `n` becomes the file.
fn ensure_with(n: &mut Node, on: bool, personal: Option<Choice>, fallback: Option<&Node>, file: Option<&Node>) {
    if !on {
        return;
    }
    let mut want = strip(n, fallback);
    inject_sized(&mut want, true, personal, file);
    // Kept only when it is the placed tree with clicks on it, so the window shows what a load of the file does.
    if without_clicks(&want, n) != want {
        keep_shown(n, &mut want);
        *n = want;
    }
}

/// The app changes the layout's structure itself (a mod seen for the first
/// time docked at a side): the change is made to the person's tree, as the
/// file will have it, never to the placed one, and the dock and the chip are
/// placed again by the sizes of `file` (the tree as the file has it), with the
/// clicks kept. Placed on a wrapped root, the change would wrap the wrapper.
pub fn edit(n: &mut Node, layout: &str, file: &Node, change: impl FnOnce(&mut Node)) {
    edit_with(n, enabled(), personal(layout), file, change);
}

fn edit_with(n: &mut Node, on: bool, personal: Option<Choice>, file: &Node, change: impl FnOnce(&mut Node)) {
    if !on {
        change(n);
        return;
    }
    let mut want = strip(n, Some(file));
    change(&mut want);
    inject_sized(&mut want, true, personal, Some(file));
    keep_shown(n, &mut want);
    *n = want;
}

/// `to`, the layout placed again, with what `from` shows on the nodes both
/// have: a node keeps its hidden flag, a stack the child it shows, a float its
/// edge, alignment and hidden flag, and a node its size where that still means
/// the same (it is a width in a row, a height in a column).
fn keep_shown(from: &Node, to: &mut Node) {
    for x in from.walk() {
        // A float is found by its own id, on whatever holds it (a root whose reserved id strip left empty included).
        for f in &x.floating {
            if let Some(g) = to.find_float_mut(&f.id) {
                (g.edge, g.align, g.hidden) = (f.edge, f.align, f.hidden);
            }
        }
        if x.id.is_empty() {
            continue;
        }
        if let Kind::Stack { active: Some(a), .. } = &x.kind {
            to.select(&x.id, a);
        }
        let same_axis = holder_axis(from, &x.id) == holder_axis(to, &x.id);
        if let Some(y) = to.find_mut(&x.id) {
            y.hidden = x.hidden;
            if same_axis {
                y.size = x.size;
            }
        }
    }
}

/// The axis the first node `id` in tree order is laid out along, its parent's (none in a stack); none for the root.
fn holder_axis(n: &Node, id: &str) -> Option<Option<Axis>> {
    n.children().iter().find_map(|c| if c.id == id { Some(n.axis()) } else { holder_axis(c, id) })
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
    use crate::layout_sync::text_of;

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

    /// The reserved ids of a status bar with the chip beside it, in tree order.
    const CHIP: [&str; 2] = ["yuzora-dock-body", "yuzora-dock-perf"];

    /// How many blocks and floats of `n` show the resource chip.
    fn chips(n: &Node) -> usize {
        let blocks = n.walk().iter().filter(|x| x.content().and_then(mod_of) == Some("yuzora-perf")).count();
        blocks + floats(n).iter().filter(|f| mod_of(&f.content) == Some("yuzora-perf")).count()
    }

    fn ids(n: &Node) -> Vec<&str> {
        n.children().iter().map(|c| c.id.as_str()).collect()
    }

    #[test]
    fn every_layout_shows_one_chip_whatever_the_dock() {
        let off = MAIN.replacen("\"id\": \"root\",", "\"id\": \"root\", \"yuzora\": { \"dock\": \"off\" },", 1);
        for text in [MAIN, TERMINALS, WIN98, YUZORA, YUZORA_BETA3, off.as_str()] {
            for personal in [None, Some(Choice::Auto), Some(Choice::Drawer), Some(Choice::Off)] {
                let mut n = parse(text);
                inject_with(&mut n, true, personal);
                assert_eq!(chips(&n), 1, "{personal:?}: {text:.60}");
            }
        }
    }

    #[test]
    fn the_status_bar_keeps_its_place_and_its_clicks() {
        let (file, shown, _) = injected(MAIN);
        assert_eq!(shown.parent_of("status").unwrap().id, "work", "where the file has it: last in the column above the tab");
        let status = shown.find("status").unwrap();
        assert_eq!((status.axis(), status.size, status.resizable), (Some(Axis::Row), Some(Size::Px { px: 28.0 }), Some(false)));
        assert_eq!(ids(status), CHIP);
        assert!(matches!(status.children()[0].content(), Some(Content::Mod { name, .. }) if name == "workspace-status"));
        let chip = &status.children()[1];
        assert!(matches!(chip.content(), Some(Content::Mod { name, .. }) if name == "yuzora-perf"));
        assert_eq!(chip.size, Some(Size::Px { px: 150.0 }));
        // A click on the status bar (hidden, another height) is the whole bar's, the chip's too.
        let state: State = serde_json::from_value(json!({ "nodes": { "status": { "hidden": true, "size": { "px": 40 } } } })).unwrap();
        let mut clicked = shown.clone();
        state.apply(&mut clicked);
        let status = clicked.find("status").unwrap();
        assert_eq!((status.hidden, status.size, status.axis()), (true, Some(Size::Px { px: 40.0 }), Some(Axis::Row)));
        assert!(clicked.visible_blocks().iter().all(|b| !CHIP.contains(&b.id.as_str())), "hidden with the bar");
        assert_eq!(State::between(&shown, &clicked), state, "saved as the same click");
        assert_eq!(strip(&without_clicks(&shown, &clicked), Some(&shown)), file, "and the file is as it was");
    }

    #[test]
    fn without_a_status_bar_the_chip_goes_to_the_bottom() {
        for (text, rows) in [(TERMINALS, ["tabs", "main", "yuzora-dock-perf"]), (WIN98, ["screen", "taskbar", "yuzora-dock-perf"])] {
            let (_, n, _) = injected(text);
            assert_eq!(ids(&n), rows);
            assert_eq!(n.children()[2].size, Some(Size::Px { px: 26.0 }));
        }
        // A root that isn't a column keeps what only a root has and becomes one: what it showed (with the root's id,
        // which finds it), then the chip.
        let row = json!({ "id": "root", "type": "row", "titlebar": "none", "traffic_lights": [12, 16], "theme": "graphite", "background": "canvas",
            "gap": 4, "divider": { "width": 1, "color": "border" }, "yuzora": { "dock": "drawer" }, "children": [
            { "id": "side", "type": "block", "size": { "px": 200 }, "content": { "mod": "spaces" } },
            { "id": "main", "type": "block", "content": { "tab": "active" } }] });
        let block = json!({ "id": "root", "type": "block", "window_drag": true, "content": { "tab": "active" } });
        for tree in [row, block] {
            let (file, n, _) = injected(&tree.to_string());
            assert_eq!((n.id.as_str(), n.axis(), n.resizable), ("yuzora-dock-root", Some(Axis::Column), Some(false)));
            assert_eq!((n.titlebar, n.traffic_lights, &n.look.theme, &n.background, &n.yuzora), (file.titlebar, file.traffic_lights, &file.look.theme, &file.background, &file.yuzora));
            assert_eq!(ids(&n), ["root", "yuzora-dock-perf"]);
            let body = &n.children()[0];
            assert_eq!((&body.kind, body.gap, &body.divider, body.window_drag), (&file.kind, file.gap, &file.divider, file.window_drag));
            assert_eq!((n.gap, &n.divider, n.window_drag), (None, &None, false), "the root's own children are the body and the chip");
            assert_eq!(n.children()[1].size, Some(Size::Px { px: 26.0 }));
            assert_eq!(strip(&n, None), file);
        }
    }

    #[test]
    fn a_status_bar_is_the_last_mod_block_of_32px_or_less_in_the_column_nearest_the_tab() {
        let tab = json!({ "id": "main", "type": "block", "content": { "tab": "active" } });
        let bar = |px: f32| json!({ "id": "bar", "type": "block", "size": { "px": px }, "content": { "mod": "m" } });
        let under_tab = |last: Value| json!({ "id": "root", "type": "column", "children": [tab.clone(), last] }).to_string();
        let (_, n, _) = injected(&under_tab(bar(32.0)));
        assert_eq!(ids(n.find("bar").unwrap()), CHIP, "32px is a status bar");
        let row = json!({ "id": "bar", "type": "row", "size": { "px": 28 }, "children": [{ "id": "m", "type": "block", "content": { "mod": "m" } }] });
        let other_tab = json!({ "id": "bar", "type": "block", "size": { "px": 28 }, "content": { "tab": "active" } });
        for last in [bar(33.0), row, other_tab] {
            let (_, n, _) = injected(&under_tab(last.clone()));
            assert_eq!(ids(&n), ["main", "bar", "yuzora-dock-perf"], "no status bar: {last}");
        }
        // An outer column's 40px bar doesn't hide the status bar of the column that holds the tab.
        let nested = json!({ "id": "root", "type": "column", "children": [
            { "id": "screen", "type": "row", "children": [{ "id": "work", "type": "column", "children": [tab.clone(), bar(28.0)] }] },
            { "id": "taskbar", "type": "block", "size": { "px": 40 }, "content": { "mod": "m" } }] });
        let (_, n, _) = injected(&nested.to_string());
        assert_eq!(ids(n.find("bar").unwrap()), CHIP);
    }

    #[test]
    fn a_layout_that_shows_the_chip_itself_gets_no_second() {
        let float = json!({ "id": "root", "type": "column", "children": [{ "id": "main", "type": "block", "content": { "tab": "active" } }],
            "floating": [{ "id": "mine", "content": { "mod": "yuzora-perf" } }] }).to_string();
        let tucked = json!({ "id": "root", "type": "column", "children": [
            { "id": "main", "type": "block", "content": { "tab": "active" } },
            { "id": "s", "type": "stack", "active": "a", "children": [
                { "id": "a", "type": "block", "content": { "mod": "m" } },
                { "id": "b", "type": "block", "size": { "px": 20 }, "content": { "mod": "yuzora-perf" } }] }] }).to_string();
        for text in [YUZORA, YUZORA_BETA3, float.as_str(), tucked.as_str()] {
            let (_, n, _) = injected(text);
            assert!(n.walk().iter().all(|x| ![BODY, PERF, ROOT].contains(&x.id.as_str())), "{text:.60}");
            assert_eq!(chips(&n), 1, "{text:.60}");
        }
    }

    #[test]
    fn strip_keeps_what_a_mod_put_beside_the_chip() {
        // A mod docked a block at the bottom of what it read (a row root shown as a column of the row and the chip):
        // the file gets that column without the chip, and what the root showed stays, with its id; the root has none.
        let row = json!({ "id": "root", "type": "row", "gap": 4, "children": [{ "id": "main", "type": "block", "content": { "tab": "active" } }] });
        let (_, mut n, _) = injected(&row.to_string());
        n.children_mut().unwrap().push(Node::block("docked", Content::Mod { name: "m".into(), props: Value::Null }));
        let out = strip(&n, None);
        assert!(!text_of(&out).contains("yuzora-dock"), "{}", text_of(&out));
        assert_eq!((out.id.as_str(), ids(&out)), ("", vec!["root", "docked"]));
        assert_eq!((out.children()[0].gap, out.find("main").is_some()), (Some(4.0), true));
        let mut again = out.clone();
        inject_with(&mut again, true, None);
        assert_eq!(chips(&again), 1);
        // The same beside main's status bar: the bar keeps what it showed and what the mod put there.
        let (_, mut n, _) = injected(MAIN);
        n.find_mut("status").unwrap().children_mut().unwrap().push(Node::block("docked", Content::Mod { name: "m".into(), props: Value::Null }));
        let out = strip(&n, None);
        assert!(!text_of(&out).contains("yuzora-dock"), "{}", text_of(&out));
        let status = out.find("status").unwrap();
        assert_eq!(ids(status), ["", "docked"]);
        assert!(matches!(status.children()[0].content(), Some(Content::Mod { name, .. }) if name == "workspace-status"));
    }

    #[test]
    fn a_root_a_person_named_like_the_chips_keeps_what_it_holds() {
        // Not what `beside` leaves: a root holding one column with a look of its own, a root with a gap of its own.
        let one = json!({ "id": "yuzora-dock-root", "type": "column", "titlebar": "none", "children": [
            { "id": "inner", "type": "column", "padding": [40, 40], "background": "#203040", "gap": 12, "children": [
                { "id": "main", "type": "block", "content": { "tab": "active" } }] }] });
        let gap = json!({ "id": "yuzora-dock-root", "type": "column", "gap": 8, "children": [
            { "id": "main", "type": "block", "content": { "tab": "active" } },
            { "id": "yuzora-dock-perf", "type": "block", "size": { "px": 26 }, "content": { "mod": "m" } }] });
        let (file, shown, _) = injected(&one.to_string());
        for n in [&shown, &strip(&shown, Some(&file))] {
            let inner = n.find("inner").unwrap();
            assert_eq!((&inner.padding, &inner.background, inner.gap), (&file.children()[0].padding, &file.children()[0].background, Some(12.0)), "shown and saved");
        }
        let (_, shown, _) = injected(&gap.to_string());
        for n in [&shown, &strip(&shown, None)] {
            assert_eq!((n.axis(), n.gap, n.children()[0].id.as_str()), (Some(Axis::Column), Some(8.0), "main"), "shown and saved");
        }
    }

    #[test]
    fn a_tree_a_mod_sends_back_has_the_chip_where_the_file_puts_it() {
        // win98's taskbar fits itself to 28px with a click, then win98 sets layout() back (or a tree built without the
        // dock): a status bar's height now.
        let (_, file, _) = injected(WIN98);
        let mut clicked = file.clone();
        clicked.find_mut("taskbar").unwrap().size = Some(Size::Px { px: 28.0 });
        let mut chipless = clicked.clone();
        chipless.children_mut().unwrap().retain(|c| c.id != PERF);
        for sent in [clicked.clone(), strip(&clicked, None), chipless] {
            // win98 itself: its file stays, and its 40px say the chip is below the taskbar, as the window shows it.
            // Beside it would be a change of structure, which a shipped layout refuses.
            let mut shipped = sent.clone();
            ensure_with(&mut shipped, true, None, Some(&file), Some(&file));
            assert_eq!(shipped, clicked);
            // A copy of win98: its file becomes the tree, a 28px taskbar is a status bar there, and the chip goes beside
            // it at once, as the written file reads back.
            let mut copy = sent;
            ensure_with(&mut copy, true, None, Some(&file), None);
            assert_eq!(ids(copy.find("taskbar").unwrap()), CHIP);
            assert_eq!(copy, as_read_with(&clicked, Some(&file), true, None));
        }
    }

    #[test]
    fn a_set_that_adds_a_status_bar_below_the_old_one_moves_the_chip_to_it() {
        let file = parse(r#"{ "id": "root", "type": "column", "children": [
            { "id": "main", "type": "block", "content": { "tab": "active" } },
            { "id": "old", "type": "block", "size": { "px": 28 }, "content": { "mod": "m" } }] }"#);
        let mut sent = file.clone();
        inject_with(&mut sent, true, None);
        let mut new = Node::block("new", Content::Mod { name: "m".into(), props: Value::Null });
        new.size = Some(Size::Px { px: 28.0 });
        sent.children_mut().unwrap().push(new);
        // A person's layout: the tree becomes the file, and the file's last 28px bar is the status bar.
        ensure_with(&mut sent, true, None, Some(&file), None);
        assert_eq!((ids(sent.find("old").unwrap()), ids(sent.find("new").unwrap())), (vec![], CHIP.to_vec()));
        assert_eq!(sent, as_read_with(&sent, Some(&file), true, None), "the window shows what the written file reads back as");
    }

    #[test]
    fn a_mods_edit_of_what_the_chip_wraps_reaches_the_file() {
        // A row root with no status bar (on screen the root's id is the row inside the chip's column), and a status bar
        // (what it shows is yuzora-dock-body beside the chip).
        let row = r#"{ "id": "root", "type": "row", "children": [
            { "id": "side", "type": "block", "size": { "px": 200 }, "content": { "mod": "m" } },
            { "id": "main", "type": "block", "content": { "tab": "active" } }] }"#;
        let bar = r#"{ "id": "root", "type": "column", "children": [
            { "id": "main", "type": "block", "content": { "tab": "active" } },
            { "id": "status", "type": "block", "size": { "px": 28 }, "content": { "mod": "m" } }] }"#;
        let padding: Option<crate::layout::Padding> = serde_json::from_value(json!([8, 8])).unwrap();
        for (text, wrapped) in [(row, "root"), (bar, BODY)] {
            let file = parse(text);
            let mut sent = file.clone();
            inject_with(&mut sent, true, None);
            sent.find_mut(wrapped).unwrap().padding = padding.clone();
            ensure_with(&mut sent, true, None, Some(&file), None);
            let written = strip(&sent, Some(&file));
            assert!(written.walk().iter().any(|x| x.padding == padding), "the padding is written: {written:?}");
            assert_eq!(sent, as_read_with(&sent, Some(&file), true, None), "the window shows what the written file reads back as");
        }
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
        // And the resource chip, at the end of the status bar.
        let ids: Vec<&str> = main.find("status").unwrap().children().iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["yuzora-dock-body", "yuzora-dock-perf"]);

        for text in [TERMINALS, WIN98] {
            let (_, n, placed) = injected(text);
            assert_eq!(placed, Placed::Drawer);
            let drawer = floats(&n).into_iter().find(|f| f.id == PREFIX).expect("the drawer");
            assert_eq!((drawer.edge, drawer.align, drawer.peek), (Edge::Right, Align::Center, Some(40.0)));
            assert!(n.floating.iter().any(|f| f.id == PREFIX), "the drawer floats over the root");
            assert!(matches!(&drawer.content, Content::Mod { name, props } if name == MODEBAR_MOD && props["dock"] == "drawer"));
            let nodes: Vec<&str> = n.walk().iter().map(|x| x.id.as_str()).filter(|id| reserved(id)).collect();
            assert_eq!(nodes, ["yuzora-dock-perf"], "no node of the dock, only the float (and the chip)");
        }

        // yuzora.json's left side is the upstream's, so the modes go to the right: a Panel, like main.
        let (_, yuzora, placed) = injected(YUZORA);
        assert_eq!(placed, Placed::Panel);
        assert!(yuzora.find("yuzora-dock-bar").is_some() && yuzora.find("yuzora-dock-git").is_some());
        assert!(yuzora.find("yuzora-dock-perf").is_none(), "it has a chip of its own");
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
            ensure_with(&mut shown, false, None, None, None);
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
            ensure_with(&mut shown, enabled(), None, None, None);
            assert_eq!(shown, n, "a tree that has the dock keeps it");
        }
        let mut bare = parse(MAIN);
        ensure_with(&mut bare, enabled(), None, None, None);
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
        ensure_with(&mut kept, true, None, None, None);
        assert_eq!(kept, chrome, "a tree that has the dock keeps it, clicks and all");
        let mut bare = file.clone();
        ensure_with(&mut bare, true, None, None, None);
        assert_eq!(strip(&bare, None), file);
        assert!(bare.find("yuzora-dock-bar").is_some());
        // The drawer too, and a dock the tree opted out of stays out.
        let mut win = parse(WIN98);
        ensure_with(&mut win, true, None, None, None);
        assert!(win.floating.iter().any(|f| f.id == PREFIX));
        let mut off = parse(&MAIN.replacen("\"id\": \"root\",", "\"id\": \"root\", \"yuzora\": { \"dock\": \"off\" },", 1));
        let before = off.clone();
        ensure_with(&mut off, true, None, None, None);
        assert_eq!(reserved_ids(&off), CHIP, "the dock stays out; the chip is no part of it");
        assert_eq!(strip(&off, None), before);
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
        assert_eq!(reserved_ids(&n), CHIP, "no dock, only the chip");
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
        ensure_with(&mut bare, true, Some(Choice::Off), None, None);
        assert_eq!(reserved_ids(&bare), CHIP, "off: a tree built without the dock stays without (the chip is no part of it)");
        let mut shown = parse(MAIN);
        inject_with(&mut shown, true, None);
        ensure_with(&mut shown, true, Some(Choice::Off), None, None);
        assert_eq!(reserved_ids(&shown), CHIP, "off: a tree that had the dock loses it, as a load would");
        let file = parse(MAIN);
        assert_eq!(reserved_ids(&as_read_with(&shown, Some(&file), true, Some(Choice::Off))), CHIP);
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
        assert_eq!(state.nodes["panel"], files.nodes["panel"]);
        assert!(!state.nodes.contains_key("panel-body"), "the dock's mode stays out");
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
        ensure_with(&mut again, true, None, None, None);
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
        const IDS: [&str; 12] = ["", "root", "r", "panel", "panel-body", "yuzora-dock", "yuzora-dock-git", "yuzora-dock-bar", "yuzora-dock-body", "yuzora-dock-perf", "yuzora-dock-root", "x"];
        fn rand_tree(next: &mut impl FnMut(u64) -> u64, ids: &[&str], depth: u32) -> Value {
            let id = ids[next(ids.len() as u64) as usize];
            let mut node = match next(if depth == 0 { 1 } else { 4 }) {
                // Blocks: the tab, a status bar (thin or not), a mod of the dock's, now and then the chip itself.
                0 => match next(8) {
                    0 | 1 => json!({ "type": "block", "content": { "tab": "active" } }),
                    2 | 3 => {
                        let px = [20, 28, 40][next(3) as usize];
                        json!({ "type": "block", "size": { "px": px }, "content": { "mod": "m" } })
                    }
                    4 if next(4) == 0 => json!({ "type": "block", "content": { "mod": "yuzora-perf" } }),
                    _ => json!({ "type": "block", "content": { "mod": if next(3) == 0 { "yuzora-sidebar-git" } else { "m" } } }),
                },
                k => {
                    let kids: Vec<Value> = (0..next(4)).map(|_| rand_tree(next, ids, depth - 1)).collect();
                    match k {
                        // A third of the rows hold a panel, so the panel placement is what gets run.
                        1 if next(3) == 0 => {
                            let body = json!({ "id": "panel-body", "type": "stack", "children": kids, "active": ids[next(ids.len() as u64) as usize] });
                            json!({ "type": "row", "children": [{ "type": "block", "content": { "tab": "active" } }, { "id": "panel", "type": "column", "children": [body] }] })
                        }
                        1 => json!({ "type": "row", "children": kids }),
                        // A third of the columns end in the tab and a bar under it.
                        2 if next(3) == 0 => {
                            let mut kids = kids;
                            kids.push(json!({ "type": "block", "content": { "tab": "active" } }));
                            kids.push(json!({ "type": "block", "size": { "px": 28 }, "content": { "mod": "m" } }));
                            json!({ "type": "column", "children": kids })
                        }
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
        let (mut parsed, mut panels, mut checked, mut bars) = (0, 0, 0, 0);
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
                bars += usize::from(shown_tree.walk().iter().any(|x| x.axis() == Some(Axis::Row) && ids(x).first() == Some(&BODY)));
                let ctx = |what: &str| format!("{what}: seed 0x2545_F491_4F6C_DD1D, tree {}", shown(&node));
                assert_eq!(chips(&shown_tree), chips(&node).max(1), "{}", ctx("one chip, or the file's own"));
                assert_eq!(strip(&shown_tree, None), node, "{}", ctx("strip(inject(F)) == F"));
                let mut ensured = shown_tree.clone();
                ensure_with(&mut ensured, true, None, None, None);
                assert_eq!(ensured, shown_tree, "{}", ctx("ensure(inject(F)) == inject(F)"));
            }
            for (on, personal) in [(true, None), (false, None), (true, Some(Choice::Auto)), (true, Some(Choice::Drawer)), (true, Some(Choice::Off))] {
                let mut a = node.clone();
                inject_with(&mut a, on, personal);
                inject_with(&mut a, on, personal);
                let mut b = node.clone();
                ensure_with(&mut b, on, personal, None, None);
                let _ = strip(&a, Some(&b));
                let _ = strip(&b, None);
                let _ = as_read_with(&a, Some(&node), on, personal);
            }
            // I1 on any tree: reserved ids at any depth, holding anything; and on the shown tree wrapped in a row with a
            // block beside it, as a mod or a dock does. I2: ensure keeps what shows on every node it keeps.
            let ctx = |what: &str| format!("{what}: seed 0x2545_F491_4F6C_DD1D, tree {}", shown(&node));
            if let Err(e) = holds_i1(&node, &strip(&node, None)) {
                panic!("{}: {e}", ctx("I1"));
            }
            let mut wrapped = once.clone();
            wrap(&mut wrapped, Axis::Row);
            if let Err(e) = holds_i1(&wrapped, &strip(&wrapped, None)) {
                panic!("{}: {e}", ctx("I1, wrapped"));
            }
            let mut ensured = node.clone();
            ensure_with(&mut ensured, true, None, None, None);
            if let Err(e) = holds_i2(&node, &ensured) {
                panic!("{}: {e}", ctx("I2"));
            }
        }
        assert!(parsed > 1000, "the generator makes trees: {parsed}");
        assert!(checked > 200 && panels > 50 && bars > 20, "the invariants run on a good share of them, panels and status bars too: {checked} checked, {panels} panels, {bars} status bars");
    }

    // The three invariants, as properties over trees.
    // I1: `strip` never deletes a node a person or a mod owns. Every node without a reserved id in a tree is in what
    //     `strip` gives back, with its fields; a block's content is never lost, a body's included.
    // I2: whatever places or re-places the chip never loses a click. A node with the same id before and after keeps
    //     its hidden flag, its size where it means the same, a stack its child, the drawer its edge and alignment.
    // I3: the tree shown and the app's copy of the file are placed by one rule from the file's sizes, after a load, a
    //     set, an app edit and a save: the two differ only by clicks, and a restart shows what the window showed.

    /// How `beside` leaves the node it wraps: what it moved into the body, gone; `resizable: false`.
    fn beside_left(x: &Node) -> bool {
        (x.gap, &x.divider, x.resizable, x.window_drag) == (None, &None, Some(false), false)
    }

    /// `x`'s own fields: everything but what it holds, what `beside` moves, and the dock's floats.
    fn own_fields(x: &Node) -> serde_json::Map<String, Value> {
        let mut v = serde_json::to_value(x).unwrap();
        let o = v.as_object_mut().unwrap();
        for k in ["type", "children", "content", "active", "header", "gap", "divider", "resizable", "window_drag", "floating"] {
            o.remove(k);
        }
        let floats: Vec<&Float> = x.floating.iter().filter(|f| !reserved(&f.id)).collect();
        if !floats.is_empty() {
            o.insert("floating".into(), json!(floats));
        }
        std::mem::take(o)
    }

    /// I1 for `s`, what `strip` gave back for `t`.
    fn holds_i1(t: &Node, s: &Node) -> Result<(), String> {
        let left: Vec<&str> = s
            .walk()
            .into_iter()
            .flat_map(|x| {
                let active = match &x.kind {
                    Kind::Stack { active: Some(a), .. } => Some(a.as_str()),
                    _ => None,
                };
                std::iter::once(x.id.as_str()).chain(active).chain(x.floating.iter().map(|f| f.id.as_str()))
            })
            .filter(|id| reserved(id))
            .collect();
        if !left.is_empty() {
            return Err(format!("reserved ids are left: {left:?}"));
        }
        let count = |n: &Node, id: &str| n.walk().iter().filter(|x| x.id == id).count();
        let shape = |n: &Node| {
            let v = serde_json::to_value(n).unwrap();
            ["type", "content", "gap", "divider", "resizable", "window_drag"].map(|k| v.get(k).cloned())
        };
        for x in t.walk().into_iter().filter(|x| !reserved(&x.id)) {
            if count(s, &x.id) < count(t, &x.id) {
                return Err(format!("{:?} is deleted", x.id));
            }
            if x.id.is_empty() || count(t, &x.id) > 1 {
                continue;
            }
            let y = s.find(&x.id).unwrap();
            let kept = own_fields(y);
            if let Some((k, v)) = own_fields(x).into_iter().find(|(k, v)| kept.get(k) != Some(v)) {
                return Err(format!("{:?} lost its {k} {v}", x.id));
            }
            // Only a node `beside` left may change what it is and how it lays out its children (it gets its body back).
            if !beside_left(x) && shape(x) != shape(y) {
                return Err(format!("{:?} changed from {:?} to {:?}", x.id, shape(x), shape(y)));
            }
        }
        let contents = |n: &Node, all: bool| -> Vec<String> {
            let theirs = |x: &&&Node| all || !reserved(&x.id) || x.id == BODY || x.id == ROOT;
            n.walk().iter().filter(theirs).filter_map(|x| x.content()).map(|c| serde_json::to_string(c).unwrap()).collect()
        };
        let mut have = contents(s, true);
        for c in contents(t, false) {
            let Some(i) = have.iter().position(|h| *h == c) else { return Err(format!("the content {c} is lost")) };
            have.swap_remove(i);
        }
        Ok(())
    }

    /// The axis of the node that holds `id` (none for a stack's child), or none when `id` is the root or not there.
    fn laid_out_along(n: &Node, id: &str) -> Option<Option<Axis>> {
        n.children().iter().find_map(|c| if c.id == id { Some(n.axis()) } else { laid_out_along(c, id) })
    }

    /// I2 for `after`, `before` placed again: each node and float with an id `before` and `after` have once. A size
    /// is a width in a row and a height in a column: it is kept where the node is laid out along the same axis.
    fn holds_i2(before: &Node, after: &Node) -> Result<(), String> {
        let once = |n: &Node, id: &str| n.walk().iter().filter(|x| x.id == id).count() == 1;
        for x in before.walk().into_iter().filter(|x| !x.id.is_empty() && once(before, &x.id) && once(after, &x.id)) {
            let y = after.find(&x.id).unwrap();
            if y.hidden != x.hidden {
                return Err(format!("{:?} was hidden {}, is {}", x.id, x.hidden, y.hidden));
            }
            if laid_out_along(before, &x.id) == laid_out_along(after, &x.id) && y.size != x.size {
                return Err(format!("{:?} was {:?}, is {:?}", x.id, x.size, y.size));
            }
            if let (Kind::Stack { active: Some(a), .. }, Kind::Stack { active: b, .. }) = (&x.kind, &y.kind)
                && y.children().iter().any(|c| c.id == *a)
                && b.as_ref() != Some(a)
            {
                return Err(format!("{:?} showed {a}, shows {b:?}", x.id));
            }
        }
        let after_floats = floats(after);
        for f in floats(before) {
            if let Some(g) = after_floats.iter().find(|g| g.id == f.id)
                && (g.edge, g.align, g.hidden) != (f.edge, f.align, f.hidden)
            {
                return Err(format!("the float {:?} moved from {:?} {:?} to {:?} {:?}", f.id, f.edge, f.align, g.edge, g.align));
            }
        }
        Ok(())
    }

    /// A window over a person's layout, as `window.rs` keeps it: the tree shown, the app's copy of the file
    /// (`file_chrome`), and the file.
    struct Window {
        chrome: Node,
        copy: Node,
        file: Node,
        personal: Option<Choice>,
    }

    impl Window {
        fn load(file: &Node, personal: Option<Choice>) -> Window {
            let mut chrome = file.clone();
            inject_with(&mut chrome, true, personal);
            Window { copy: chrome.clone(), chrome, file: file.clone(), personal }
        }

        /// A mod's `layout.set` on a person's layout: the file becomes the tree, the clicks start over.
        fn set(&mut self, mut n: Node) -> Result<(), String> {
            holds_i1(&n, &strip(&n, Some(&self.copy))).map_err(|e| format!("I1 on the tree sent: {e}"))?;
            let sent = n.clone();
            ensure_with(&mut n, true, self.personal, Some(&self.copy), None);
            holds_i2(&sent, &n).map_err(|e| format!("I2 on the set: {e}"))?;
            self.file = strip(&n, Some(&self.copy));
            holds_i1(&n, &self.file).map_err(|e| format!("I1 on the file written: {e}"))?;
            self.copy = as_read_with(&n, Some(&self.copy), true, self.personal);
            self.chrome = n;
            Ok(())
        }

        /// `save()`: an edit (anything but a click) to the file, the clicks to `.state`.
        fn save(&mut self) -> Result<State, String> {
            let edited = without_clicks(&self.copy, &self.chrome);
            if edited != self.copy {
                self.file = strip(&edited, Some(&self.copy));
                holds_i1(&edited, &self.file).map_err(|e| format!("I1 on the file saved: {e}"))?;
                self.copy = as_read_with(&edited, Some(&self.copy), true, self.personal);
            }
            if without_clicks(&self.copy, &self.chrome) != self.copy {
                return Err("I3: the window and the app's copy of the file differ by more than clicks".into());
            }
            Ok(State::between(&self.copy, &self.chrome))
        }

        /// A restart: the file read and placed, the saved clicks put on it.
        fn restart(&self, state: &State) -> Node {
            let mut shown: Node = serde_json::from_str(&text_of(&self.file)).unwrap();
            inject_with(&mut shown, true, self.personal);
            let mut saved = state.clone();
            saved.file = serde_json::to_value(&self.copy).ok();
            saved.forget_since_saved(&shown);
            without_panel_click(&saved, &shown).apply(&mut shown);
            shown
        }
    }

    fn merge(v: &mut Value, with: Value) {
        for (k, x) in with.as_object().unwrap() {
            v[k] = x.clone();
        }
    }

    /// The person's layouts the properties run on: a root of each type, with no status bar or one on either side of
    /// 32px, the fields `beside` moves set on the bar or on the root, and reserved ids the person wrote.
    fn person_layouts() -> Vec<(String, Node)> {
        let tab = json!({ "id": "main", "type": "block", "content": { "tab": "active" } });
        let side = json!({ "id": "side", "type": "block", "size": { "px": 200 }, "content": { "mod": "side" } });
        let panel = json!({ "id": "panel", "type": "column", "hidden": true, "size": { "px": 368 }, "children": [
            { "id": "panel-body", "type": "stack", "active": "files", "children": [
                { "id": "files", "type": "block", "content": { "mod": "files" } },
                { "id": "changes", "type": "block", "content": { "mod": "changes" } }] }] });
        let moved = |resizable: bool| json!({ "gap": 3, "divider": { "width": 1, "color": "border" }, "resizable": resizable, "window_drag": true });
        let mut out = Vec::new();
        for px in [None, Some(32), Some(33)] {
            for fields_on in ["nothing", "the bar", "the root"] {
                let mut bar = px.map(|px| json!({ "id": "status", "type": "block", "size": { "px": px }, "content": { "mod": "status" } }));
                match (fields_on, bar.as_mut()) {
                    ("the bar", Some(bar)) => merge(bar, moved(true)),
                    ("the bar", None) => continue,
                    _ => {}
                }
                let held: Vec<Value> = [tab.clone()].into_iter().chain(bar.clone()).collect();
                let column = |id: &str| json!({ "id": id, "type": "column", "children": held.clone() });
                let mut roots = vec![
                    ("column", column("root")),
                    ("row", json!({ "id": "root", "type": "row", "children": [side.clone(), column("work")] })),
                    ("row with a panel", json!({ "id": "root", "type": "row", "children": [column("work"), panel.clone()] })),
                    ("stack", json!({ "id": "root", "type": "stack", "active": "work", "children": [column("work"), side.clone()] })),
                ];
                if px.is_none() {
                    roots.push(("block", json!({ "id": "root", "type": "block", "content": { "tab": "active" } })));
                }
                for (root_kind, mut root) in roots {
                    merge(&mut root, json!({ "titlebar": "none", "traffic_lights": [12, 16], "background": "canvas" }));
                    if fields_on == "the root" {
                        merge(&mut root, moved(false));
                    }
                    out.push((format!("{root_kind} root, status bar {px:?}, beside's fields on {fields_on}"), serde_json::from_value(root).unwrap()));
                }
            }
        }
        // Reserved ids a person wrote: the root named like the chip's, the column holding the tab and the status bar
        // named like the dock's.
        let reserved_named = [
            json!({ "id": "yuzora-dock-root", "type": "row", "children": [side.clone(), { "id": "work", "type": "column", "children": [tab.clone()] }] }),
            json!({ "id": "yuzora-dock-root", "type": "column", "padding": [6, 6], "children": [tab.clone()] }),
            json!({ "id": "root", "type": "row", "children": [side.clone(), { "id": "yuzora-dock-work", "type": "column", "children": [tab.clone(),
                { "id": "status", "type": "block", "size": { "px": 28 }, "content": { "mod": "status" } }] }] }),
            json!({ "id": "root", "type": "column", "children": [tab.clone(), { "id": "yuzora-dock-body", "type": "block", "size": { "px": 28 }, "content": { "mod": "status" } }] }),
        ];
        out.extend(reserved_named.into_iter().map(|root| (format!("reserved ids in {root}"), serde_json::from_value(root).unwrap())));
        out
    }

    fn added() -> Node {
        let mut b = Node::block("added", Content::Mod { name: "added".into(), props: Value::Null });
        b.size = Some(Size::Px { px: 300.0 });
        b
    }

    fn wrap(n: &mut Node, axis: Axis) {
        let inner = std::mem::replace(n, Node::new("", Kind::Row { children: Vec::new() }));
        *n = Node::container("wrap", axis, vec![inner, added()]);
    }

    fn path_of(n: &Node, id: &str) -> Option<Vec<usize>> {
        if n.id == id {
            return Some(Vec::new());
        }
        n.children().iter().enumerate().find_map(|(i, c)| path_of(c, id).map(|p| [vec![i], p].concat()))
    }

    /// What holds the chip: the status bar, the root, a column root.
    fn chip_holder(n: &mut Node) -> Option<&mut Node> {
        let at = path_of(n, PERF)?;
        node_at_mut(n, &at[..at.len() - 1])
    }

    /// What a mod's `layout.set` does to the tree it read from `layout()`, and the same change to the person's file
    /// where the file has what it changes.
    type Change = fn(&mut Node);

    fn nothing(_: &mut Node) {}

    fn in_a_row(n: &mut Node) {
        wrap(n, Axis::Row);
    }

    fn in_a_column(n: &mut Node) {
        wrap(n, Axis::Column);
    }

    /// The node the chip wraps (a status bar's body, what a root showed) in a row of its own, or the chip itself.
    fn placed_in_a_row(n: &mut Node) {
        let at = path_of(n, BODY).or_else(|| path_of(n, ROOT).map(|_| vec![0])).or_else(|| path_of(n, PERF));
        if let Some(x) = at.and_then(|at| node_at_mut(n, &at)) {
            let inner = std::mem::replace(x, Node::new("", Kind::Row { children: Vec::new() }));
            *x = Node::container("wrap", Axis::Row, vec![inner]);
        }
    }

    fn added_beside_the_chip(n: &mut Node) {
        if let Some(c) = chip_holder(n).and_then(Node::children_mut) {
            c.push(added());
        }
    }

    fn status_bar_above_the_tab(n: &mut Node) {
        if let Some(at) = path_of(n, "status")
            && let Some((last, up)) = at.split_last()
            && let Some(c) = node_at_mut(n, up).and_then(Node::children_mut)
        {
            let bar = c.remove(*last);
            c.insert(0, bar);
        }
    }

    fn resizable_on_the_chips_holder(n: &mut Node) {
        if let Some(x) = chip_holder(n) {
            x.resizable = Some(true);
        }
    }

    const SETS: [(&str, Change, Option<Change>); 7] = [
        ("nothing", nothing, Some(nothing)),
        ("the root wrapped in a row", in_a_row, Some(in_a_row)),
        ("the root wrapped in a column", in_a_column, Some(in_a_column)),
        ("the placed node wrapped", placed_in_a_row, None),
        ("a node added beside the chip", added_beside_the_chip, None),
        ("the status bar moved above the tab", status_bar_above_the_tab, Some(status_bar_above_the_tab)),
        ("resizable set on what holds the chip", resizable_on_the_chips_holder, None),
    ];

    /// What a person does first: hides the dock's bar, shows Git in the panel, drags the drawer to the left edge,
    /// widens their side bar.
    fn person_clicks(chrome: &mut Node) -> State {
        let before = chrome.clone();
        if let Some(bar) = chrome.find_mut("yuzora-dock-bar") {
            bar.hidden = true;
        }
        chrome.select("panel-body", "yuzora-dock-git");
        if let Some(f) = chrome.find_float_mut(PREFIX) {
            (f.edge, f.align) = (Edge::Left, Align::End);
        }
        if let Some(side) = chrome.find_mut("side") {
            side.size = Some(Size::Px { px: 260.0 });
        }
        State::between(&before, chrome)
    }

    /// A person's layout loaded, clicked, then a mod's `layout.set` of what it read with one change, then a save and
    /// a restart.
    fn after_a_set(file: &Node, personal: Option<Choice>, change: Change, on_file: Option<Change>) -> Result<(), String> {
        let mut w = Window::load(file, personal);
        holds_i1(&w.chrome, &strip(&w.chrome, None)).map_err(|e| format!("I1 on the tree shown: {e}"))?;
        let clicks = person_clicks(&mut w.chrome);
        let mut sent = w.chrome.clone();
        change(&mut sent);
        w.set(sent)?;
        let state = w.save()?;
        // The person's own clicks went into the file with the tree; the ones on the dock stay clicks; none is made up.
        let dock = |(id, c): &(&String, &crate::layout_sync::NodeState)| reserved(id) || c.active.as_deref().is_some_and(reserved);
        let want = State { nodes: clicks.nodes.iter().filter(dock).map(|(id, c)| (id.clone(), c.clone())).collect(), floats: clicks.floats.clone(), ..State::default() };
        if state != want {
            return Err(format!("I2: the clicks are {state:?}, not {want:?}"));
        }
        if w.restart(&state) != w.chrome {
            return Err("I3: a restart shows another tree than the window".into());
        }
        if let Some(on_file) = on_file
            && reserved_ids(file).is_empty()
        {
            let mut person = file.clone();
            person_clicks(&mut person);
            on_file(&mut person);
            if w.file != person {
                return Err(format!("the file is\n{}not\n{}", text_of(&w.file), text_of(&person)));
            }
        }
        Ok(())
    }

    #[test]
    fn a_mods_set_on_any_layout_holds_the_three_invariants() {
        let (mut runs, mut failures) = (0, Vec::new());
        for (layout, file) in person_layouts() {
            for personal in [None, Some(Choice::Drawer), Some(Choice::Off)] {
                for (what, change, on_file) in SETS {
                    runs += 1;
                    if let Err(e) = after_a_set(&file, personal, change, on_file) {
                        failures.push(format!("{layout}, dock {personal:?}, {what}: {e}"));
                    }
                }
            }
        }
        assert!(failures.is_empty(), "{} of {runs} fail:\n{}", failures.len(), failures.join("\n"));
        assert_eq!(runs, 798, "38 layouts, 3 dock choices, 7 changes");
    }
}
