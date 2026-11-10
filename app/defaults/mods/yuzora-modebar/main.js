// The mode bar. One mod, three forms, told apart by props().dock:
//   "bar"    the right dock's 40px column next to the panel (injected by src/yuzora/dock.rs):
//            picks which of the four modes the panel's stack shows, and opens or closes the panel.
//   "drawer" the dock's peeking launcher in a layout with no panel: its modes open in a window
//            (a floating mod holds one mod and refocuses the terminal on every press, so it can't hold them).
//   "head"   that window's header: the mode tabs and a close button.
//   none     LEGACY (beta.4): the old sidebar's row of five modes, with the workspace mode and its amber count; only an
//            old clone of the yuzora layout still has it (the shipped one gets the "bar" form from the dock).
// Commands (<mod>.<command>: a key in keys.json, `herdr-app do`, the status bar): files, git, database, ssh
// open that mode; toggle shows the last one or closes it.
// A mode's badge is shared("yuzora.badge.<mode>"): a number, or null for none. Each lane sets its own;
// the legacy workspace mode's (agents waiting for a reply, every machine) is amber and set here.
// "When Yuzora opens" (settings > General, general.startup.dock) is applied here, by the first bar or drawer of the run.
// Owned by lane L1 (the dock forms: lane dock).

import { View, div, svg } from "gpui-kit";
import { h_flex, v_flex } from "gpui-base";
import { agents, layout, run, theme, settings, shared, setShared, props, placement, spaces, openOverlay, closeOverlay, nextCommand } from "herdr";
import { t, tokens, session, motion } from "yuzora:core";
import { startup } from "yuzora:settings";

const STACK = "yuzora-sidebar";
const MODES = ["workspace", "files", "git", "database", "ssh"];
const WAITING = "yuzora.badge.workspace";

// The dock's.
const DOCK_MODES = MODES.slice(1);
const PANEL = "panel", BODY = "panel-body";
const NODE = "yuzora-dock-";
const LAST = "yuzora.dock.last"; // the mode the toggle command shows again
const OPEN = "yuzora.dock.open"; // the drawer's window now: { id, mode, session }; the window's header sets it, clears it on closing
const COLORS = "yuzora.space.colors", SPACE_COLORS = "space.colors";
const WINDOW_W = 380;
const BUTTON = 34, GAP = 2, STEP = BUTTON + GAP; // the bar's mode buttons: the selection slides by one STEP
const STARTED = "yuzora.startup.applied"; // session() of the run whose startup mode was applied: once per run
const HONORED = "yuzora.startup.honored"; // settings > General shows its "When Yuzora opens" choice only once something applies it

function find(node, id) {
  if (node?.id === id) return node;
  for (const child of node?.children ?? []) {
    const found = find(child, id);
    if (found) return found;
  }
  return null;
}

// The window open now, or null. Shared state outlives the app but overlay ids start over on every run, so
// what an earlier run left is no window of this one (and its id could be another overlay's).
export function openNow() {
  const open = shared(OPEN);
  return open && open.session === session() ? open : null;
}

export function badgeText(value) {
  const n = typeof value === "number" ? Math.floor(value) : 0;
  return n > 0 ? (n > 99 ? "99+" : String(n)) : null;
}

// Herdr's Spaces (the left side) keep their badge colors in "space.colors"; Yuzora's mods keep theirs in
// "yuzora.space.colors". Where Yuzora has a color for a Space, Herdr gets the same hex unless it already has a
// hex of its own (a pick in its UI stays). Herdr's `spaces` and `picker` mods give a Space without a color a theme
// color name ("ansi6") on every render, and they render before this does, so a name is theirs to be replaced, not a pick.
export function syncSpaceColors() {
  const mine = shared(COLORS);
  if (!mine || typeof mine !== "object") return;
  const theirs = shared(SPACE_COLORS) ?? {};
  const ids = new Set(spaces().map((s) => s.id));
  let next = null;
  for (const [id, hex] of Object.entries(mine)) {
    if (ids.has(id) && typeof hex === "string" && hex.startsWith("#") && !String(theirs[id] ?? "").startsWith("#")) (next ??= { ...theirs })[id] = hex;
  }
  if (next) setShared(SPACE_COLORS, next);
}

// The window a drawer's mode opens in: a header (the tabs, close) over the mode itself. Opening it again
// with the same key replaces it where it is.
export function openMode(mode, at) {
  setShared(LAST, mode);
  return openOverlay({
    kind: "window", key: "yuzora-dock", keep: true, width: WINDOW_W, height: 0.85, ...(at ? { at } : {}),
    layout: { id: "dock", type: "column", children: [
      { id: "head", type: "block", size: { px: 34 }, content: { mod: "yuzora-modebar", props: { dock: "head", mode } } },
      { id: "body", type: "block", content: { mod: `yuzora-sidebar-${mode}` } },
    ] },
  });
}

const modeIcon = (mode) => svg(`mods/yuzora-modebar/${mode}.svg`).size(16);
const badgeChip = (th, tk, badge) => div().absolute().top(2).right(3).min_w(13).h(13).px(3).rounded(7).bg(tk.status.done.color)
  .flex().items_center().justify_center().text_size(9).font_bold().text_color(th.canvas).child(badge);

export default class Modebar extends View {
  init(_props, cx) {
    this.dock = props()?.dock ?? null;
    if (this.dock === "head") { setShared(OPEN, { id: props()?.overlay ?? null, mode: props()?.mode ?? null, session: session() }); return; }
    if (this.dock === "bar" || this.dock === "drawer") this.start();
    cx.spawn(async (cx) => {
      for (;;) {
        this.command(await nextCommand());
        cx.notify();
      }
    });
  }

  // Settings > General "When Yuzora opens": the dock's mode to open (or "none": closed), applied by the first
  // dock this run shows, once. A layout switch later in the run does not apply it again.
  start() {
    setShared(HONORED, true);
    if (shared(STARTED) === session()) return;
    setShared(STARTED, session());
    const mode = startup().dock;
    if (mode === null) return;
    if (this.dock === "bar") {
      if (mode === "none") run("layout.hide", { id: PANEL });
      else this.showMode(mode);
    } else if (mode !== "none") {
      openMode(mode);
    }
  }

  // The window closes (its ×, a layout switch, the app quitting): it is no longer open. A mode tab's replacement
  // opens with the same key: the window removes the old overlay first (this close() clears OPEN), then the new
  // header's init writes its own id. A close() that finds another id there leaves it.
  close() {
    if (this.dock === "head" && openNow()?.id === props()?.overlay) setShared(OPEN, null);
  }

  // ---- commands -----------------------------------------------------------------

  command(name) {
    if (this.dock === null) {
      if (MODES.includes(name)) run("layout.select", { stack: STACK, child: `${STACK}-${name}` });
      return;
    }
    const mode = DOCK_MODES.includes(name) ? name : name === "toggle" ? (shared(LAST) ?? "files") : null;
    if (!mode) return;
    if (this.dock === "bar") {
      if (name === "toggle" && this.shownMode() !== null) run("layout.hide", { id: PANEL });
      else this.showMode(mode);
    } else if (name === "toggle" && openNow()) {
      const open = openNow();
      setShared(OPEN, null);
      closeOverlay(open.id);
    } else {
      openMode(mode);
    }
  }

  // ---- the bar --------------------------------------------------------------------

  // The dock mode the panel shows now, or null (the panel is closed, or shows Files / Changes).
  shownMode() {
    const panel = find(layout(), PANEL), active = find(layout(), BODY)?.active;
    return panel && !panel.hidden && typeof active === "string" && active.startsWith(NODE) ? active.slice(NODE.length) : null;
  }

  showMode(mode) {
    setShared(LAST, mode);
    run("layout.select", { stack: BODY, child: `${NODE}${mode}` });
    run("layout.show", { id: PANEL });
  }

  // The mode on screen closes the panel; any other opens its mode.
  pickBar(mode) {
    if (this.shownMode() === mode) run("layout.hide", { id: PANEL });
    else this.showMode(mode);
  }

  bar() {
    const th = theme(), tk = tokens();
    syncSpaceColors();
    const shown = this.shownMode();
    const button = (mode) => {
      const on = shown === mode, badge = badgeText(shared(`yuzora.badge.${mode}`));
      return div().id(`dock-${mode}`).absolute().left(0).top(DOCK_MODES.indexOf(mode) * STEP).w(BUTTON).h(BUTTON).flex().flex_none().items_center().justify_center().rounded(6)
        .text_color(on ? tk.text.strong : th.muted)
        .when(!on, (el) => el.hover((s) => s.bg(th.hover).text_color(th.foreground)))
        .tooltip(badge ? `${t(`mode.${mode}`)} · ${badge}` : t(`mode.${mode}`))
        .on_click(() => this.pickBar(mode))
        .child(modeIcon(mode))
        .when(Boolean(badge), (el) => el.child(badgeChip(th, tk, badge)));
    };
    // One selection under the buttons (which have no background of their own), sliding between them. It is there
    // while a mode is shown and not at all when the panel is closed, so opening or closing the panel is not a slide.
    // The layout rounds every length a script writes, so the selection is on its button only if both are placed by
    // the same length: each button's `top` is `index * STEP`, as the selection's. Its id has the zoom in it: a new zoom
    // is a new element, which starts in its place instead of sliding there.
    const at = DOCK_MODES.indexOf(shown);
    const selection = div().id(`dock-selection-${settings().zoom}`).absolute().left(0).top(at * STEP).w(BUTTON).h(BUTTON).rounded(6)
      .bg(th.selected).border_r(2).border_color(th.foreground).transition("top", motion("base"));
    return v_flex().id("root").size_full().py(6).items_center().bg(th.chrome).border_l(1).border_color(th.border)
      .child(div().relative().w(BUTTON).h(DOCK_MODES.length * STEP - GAP)
        .when(at >= 0, (el) => el.child(selection))
        .children(DOCK_MODES.map(button)));
  }

  // ---- the drawer -----------------------------------------------------------------

  // Opens the mode's window beside the drawer: at the pointer, away from the edge the drawer is on.
  pickDrawer(mode, e) {
    const p = placement(), x = e?.position?.x;
    const at = typeof x !== "number" ? undefined
      : p?.vertical === false ? { x: Math.max(0, Math.round(x - WINDOW_W / 2)), y: 48 }
      : p?.edge === "left" ? { x: Math.round(x + 24), y: 48 }
      : { x: Math.max(0, Math.round(x - WINDOW_W - 24)), y: 48 };
    openMode(mode, at);
  }

  drawer() {
    const th = theme(), tk = tokens();
    syncSpaceColors();
    const p = placement(), vertical = p?.vertical !== false, left = p?.edge === "left";
    const open = openNow()?.mode ?? null;
    const item = (mode) => {
      const on = open === mode, badge = badgeText(shared(`yuzora.badge.${mode}`));
      const icon = div().relative().w(40).h(36).flex().flex_none().items_center().justify_center().child(modeIcon(mode))
        .when(Boolean(badge), (el) => el.child(badgeChip(th, tk, badge)));
      // The edge side shows while the drawer peeks, so the icon goes there and the name on the inner side.
      const name = div().w(96).flex_none().truncate().text_size(12.5).child(t(`mode.${mode}`));
      return h_flex().id(`dock-${mode}`).h(36).flex_none().items_center().rounded(6).cursor_pointer()
        .bg(on ? th.selected : th.popover).text_color(on ? th.foreground : th.muted)
        .hover((s) => s.bg(th.hover).text_color(th.foreground))
        .tooltip(t(`mode.${mode}`))
        .on_click((e) => this.pickDrawer(mode, e))
        .children(vertical ? (left ? [icon, name] : [name, icon]) : [icon]);
    };
    return (vertical ? v_flex() : h_flex()).id("root").gap(2).p(2).children(DOCK_MODES.map(item));
  }

  // ---- the window's header ----------------------------------------------------------

  // A tab of another mode replaces the window (same key) with that mode.
  pickTab(mode) {
    if (mode !== props()?.mode) openMode(mode);
  }

  closeWindow() {
    setShared(OPEN, null);
    closeOverlay(props()?.overlay);
  }

  head() {
    const th = theme();
    const stop = (_e, cx) => cx.stop_propagation();
    const mode = props()?.mode;
    const tab = (m) => {
      const on = m === mode;
      return h_flex().id(`head-${m}`).h(26).px(8).gap(6).rounded(6).items_center().flex_none().cursor_pointer()
        .bg(on ? th.selected : th.popover).text_color(on ? th.foreground : th.muted)
        .when(!on, (el) => el.hover((s) => s.bg(th.hover).text_color(th.foreground)))
        .on_mouse_down("left", stop)
        .on_click(() => this.pickTab(m))
        .child(modeIcon(m)).child(on ? div().text_size(12.5).child(t(`mode.${m}`)) : div());
    };
    return h_flex().id("root").size_full().px(6).gap(2).items_center().bg(th.popover).border_b(1).border_color(th.border)
      .children(DOCK_MODES.map(tab))
      .child(div().flex_1())
      .child(div().id("close").size(26).flex().flex_none().items_center().justify_center().rounded(6).text_color(th.muted)
        .hover((s) => s.bg(th.hover).text_color(th.foreground)).tooltip(t("close"))
        .on_mouse_down("left", stop)
        .on_click(() => this.closeWindow())
        .child("×"));
  }

  // ---- legacy: the old sidebar's row --------------------------------------------------

  legacy() {
    const th = theme(), tk = tokens();
    const waiting = agents().filter((a) => a.status === "blocked").length || null;
    if (shared(WAITING) !== waiting) setShared(WAITING, waiting);
    const active = find(layout(), STACK)?.active ?? `${STACK}-workspace`;
    const button = (mode) => {
      const child = `${STACK}-${mode}`, on = active === child;
      const badge = badgeText(shared(`yuzora.badge.${mode}`));
      const tone = mode === "workspace" ? tk.status.blocked.color : tk.status.done.color;
      return div().id(`mode-${mode}`).relative().w(36).h(32).flex().flex_none().items_center().justify_center().rounded(6)
        .bg(on ? th.selected : th.chrome).text_color(on ? tk.text.strong : th.muted)
        .when(on, (el) => el.border_b(2).border_color(th.foreground))
        .when(!on, (el) => el.hover((s) => s.bg(th.hover).text_color(th.foreground)))
        .tooltip(badge ? `${t(`mode.${mode}`)} · ${badge}` : t(`mode.${mode}`))
        .on_click(() => run("layout.select", { stack: STACK, child }))
        .child(svg(`mods/yuzora-modebar/${mode}.svg`).size(16))
        .when(Boolean(badge), (el) => el.child(div().absolute().top(2).right(3).min_w(13).h(13).px(3).rounded(7).bg(tone)
          .flex().items_center().justify_center().text_size(9).font_bold().text_color(th.canvas).child(badge)));
    };
    return h_flex().id("root").size_full().px(6).gap(2).items_center().bg(th.chrome).border_t(1).border_color(th.divider ?? th.border)
      .children(MODES.map(button))
      .child(div().flex_1())
      .child(div().w(1).h(18).flex_none().bg(th.border))
      // Herdr's settings until lane L6's settings frame lands (see docs/beta2/shell.md).
      .child(div().id("settings").w(32).h(32).flex().flex_none().items_center().justify_center().rounded(6).text_color(th.muted)
        .hover((s) => s.bg(th.hover).text_color(th.foreground)).tooltip(t("settings"))
        .on_click(() => run("app.settings"))
        .child(svg("mods/yuzora-modebar/settings.svg").size(16)));
  }

  render() {
    return this.dock === "bar" ? this.bar() : this.dock === "drawer" ? this.drawer() : this.dock === "head" ? this.head() : this.legacy();
  }
}
