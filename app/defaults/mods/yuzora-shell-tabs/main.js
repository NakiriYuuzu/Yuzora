// The tab bar. Owned by lane L1.
// Every Space's tabs, each with its Space's badge, its most urgent agent state and (when
// shown) a close button; + opens Herdr's new-tab menu. "New pane ▾" mixes a tool pane
// (yuzora:core panes(): mods whose mod.json says "yuzora": {"pane": …}), a terminal or an
// agent into the shown tab, right of or below the focused pane. Window buttons where
// the system draws none. Opened as a popup with props { menu: "pane" } it draws that menu;
// the command "yuzora-shell-tabs.new-pane" opens it.

import { View, div, svg, image } from "gpui-kit";
import { h_flex, v_flex } from "gpui-base";
import { spaces, panes, focused, knownAgents, run, openOverlay, closeOverlay, props, theme, window, layout, settings, shared, setShared, nextCommand } from "herdr";
import { t, tokens, panes as toolPanes } from "yuzora:core";
import { spaceBadges } from "yuzora:shell";

const SELF = "yuzora-shell-tabs";
const STATUS = "mods/yuzora-sidebar-workspace/status";
const AGENTS = "yuzora.shell.agents"; // machine -> the agents knownAgents() last listed
const SPLIT = "yuzora.shell.split"; // "right" | "down": where the new-pane menu puts things

// Spaces' badges (initial and kept color). The colors live in shared state; a Space seen the
// first time gets one and it's written back, so every mod and every restart agree.
const COLORS = "yuzora.space.colors";
function badgesOf(list) {
  const r = spaceBadges(list.map((s) => ({ id: s.id, name: s.name })), shared(COLORS));
  if (r.changed) setShared(COLORS, r.kept);
  return r.badges;
}

// The agents the new-pane menu offers: the installed ones, then two to install.
export function choicesOf(agents) {
  return [...agents.filter((a) => a.installed), ...agents.filter((a) => !a.installed).slice(0, 2)];
}

// The new-pane menu is as tall as its rows, at most this: past it the list under the place
// pills scrolls. (A popup sizes to its content, so nothing in it may wait for a height to fill.)
export const MENU_MAX = 520;

function find(node, id) {
  if (node?.id === id) return node;
  for (const child of node?.children ?? []) {
    const found = find(child, id);
    if (found) return found;
  }
  return null;
}

// A tab's name: the one someone gave it, else its agent's or program's.
export function tabName(tab, mine) {
  if (tab.named) return tab.name;
  const lead = mine.find((p) => p.program?.agent) ?? mine.find((p) => p.terminal) ?? mine[0];
  const p = lead?.program;
  return ((p?.agent || p?.shell) && p.name) || lead?.title || p?.name || tab.name;
}

// What the new-pane menu does with the focused pane: split it, or start a tab when nothing
// is focused (no space yet, or a layout terminal block has the keyboard).
export function placeIn(f, space, dir) {
  return f?.pane ? { split: { direction: dir, pane: f.pane } } : { tab: space ? { space: space.id } : {} };
}

// The command a picked row runs, [name, args]: a tool pane ({kind: "tool", mod, props}), a
// terminal ({kind: "terminal"}) or an agent ({kind: "agent", name, run}), at `at` (placeIn).
export function paneCommand(item, at, dir) {
  if (item.kind === "agent") {
    return at.split ? ["terminal.open", { argv: item.run, place: dir }] : ["tab.new", { ...at.tab, name: item.name, argv: item.run }];
  }
  const extra = item.kind === "tool" ? { mod: item.mod, props: item.props ?? {} } : {};
  return at.split ? ["pane.split", { ...at.split, ...extra }] : ["tab.new", { ...at.tab, ...extra }];
}

export default class Tabs extends View {
  init(_props, cx) {
    this.pressed = null;
    this.panels = JSON.parse(localStorage.getItem("panels") ?? "{}"); // space id -> "files" | "changes" | null
    if (props()?.menu !== "pane") {
      // "yuzora-shell-tabs.new-pane" (a key in keys.json, or herdr-app do) opens the new-pane menu.
      cx.spawn(async () => {
        for (;;) if ((await nextCommand()) === "new-pane") this.openPaneMenu("dialog");
      });
      return;
    }
    const machine = spaces().find((s) => s.active)?.machine ?? null;
    const where = machine ?? "";
    this.agents = this.cachedAgents();
    this.dir = shared(SPLIT) === "down" ? "down" : "right";
    cx.spawn(async (cx) => {
      try {
        this.agents = await knownAgents({ on: machine });
        setShared(AGENTS, { ...(shared(AGENTS) ?? {}), [where]: this.agents });
      } catch { this.failed = true; }
      this.listed = true;
      cx.notify();
    });
  }

  // ---- the new-pane menu ------------------------------------------------------

  // The active Space's machine's agents, as knownAgents() last listed them.
  cachedAgents() {
    return shared(AGENTS)?.[spaces().find((s) => s.active)?.machine ?? ""] ?? [];
  }

  // Under the pointer from the button; centered from a key or script (nothing to point at).
  // No height: it's as tall as its rows (MENU_MAX at most).
  openPaneMenu(kind = "popup") {
    openOverlay({ mod: SELF, kind, width: 280, props: { menu: "pane" } });
  }

  // The tool panes offered (yuzora:core panes()).
  tools() {
    return toolPanes();
  }

  // A row was picked: close the menu and put it where the pills say.
  choose(item) {
    closeOverlay(props().overlay);
    const [name, args] = paneCommand(item, placeIn(focused(), spaces().find((s) => s.active), this.dir), this.dir);
    run(name, args);
  }

  // The pills: shown at once (this copy), and remembered for the next menu (shared).
  place(dir, cx) {
    this.dir = dir;
    setShared(SPLIT, dir);
    cx.notify();
  }

  menuRow(th, id, icon, label, item, hint = "", enabled = true) {
    return h_flex().id(id).h(30).px(9).gap(10).rounded(6).items_center().flex_none()
      .text_color(enabled ? th.foreground : th.dim).when(enabled, (el) => el.cursor_pointer().hover((s) => s.bg(th.selected)))
      .on_click(() => { if (enabled) this.choose(item); })
      .child(h_flex().w(14).flex_none().items_center().justify_center().child(icon))
      .child(div().flex_1().min_w(0).truncate().child(label))
      .when(Boolean(hint), (el) => el.child(div().flex_none().text_size(11).text_color(th.dim).child(hint)));
  }

  menu(th) {
    const dir = this.dir;
    const space = spaces().find((s) => s.active);
    const at = placeIn(focused(), space, dir);
    const icon = (path, tint, color) => (path ? (tint ? svg(path).size(14).text_color(color) : image(path).size(14)) : div());
    const label = (text) => div().px(9).pt(8).pb(3).text_size(11).text_color(th.dim).child(text);
    const note = (text) => div().px(9).py(5).text_size(12).text_color(th.muted).child(text);
    // The chosen place is the accent; the other a plain outline.
    const pill = (id, value) => div().id(`dir-${id}`).h(22).px(10).flex().items_center().rounded(11).text_size(11.5)
      .border(1).border_color(dir === value ? th.accent : th.input).bg(dir === value ? th.accent : th.popover)
      .text_color(dir === value ? th.accent_foreground : th.muted).cursor_pointer()
      .when(dir !== value, (el) => el.hover((s) => s.bg(th.hover).text_color(th.foreground)))
      .on_click((_e, cx) => this.place(value, cx)).child(t(`menu.${id}`));
    const tools = this.tools();
    const choices = choicesOf(this.agents);
    const agentsNote = choices.length ? null : this.failed ? "menu.failed" : this.listed ? "menu.no_agents" : "menu.checking";
    // A popup is as tall as what's in it: the column takes its rows' height (MENU_MAX at most)
    // and only then does the list shrink and scroll. Nothing here waits for a height to fill:
    // not flex_1, and not overflow_y_scrollbar, whose scroll area fills its parent.
    return v_flex().id("pane-menu").w_full().max_h(MENU_MAX).min_h(0).p(5).bg(th.popover).text_size(12.5)
      .child(h_flex().h(32).flex_none().px(9).gap(6).items_center().text_size(11.5).text_color(th.dim)
        .child(div().flex_1().child(t("menu.where"))).child(pill("right", "right")).child(pill("down", "down")))
      .child(v_flex().id("pane-menu-list").min_h(0).overflow_y_scroll()
        .child(label(t("menu.tools")))
        // A tool pane needs a Space to go in (tab.new can start one for a terminal, not for a mod).
        .children(tools.map((p) => this.menuRow(th, `tool-${p.mod}`, icon(p.icon, true, th.muted), p.label,
          { kind: "tool", mod: p.mod, props: p.props }, at.split || space ? "" : t("menu.needs_space"), Boolean(at.split || space))))
        .when(tools.length === 0, (el) => el.child(note(t("menu.no_tools"))))
        .child(div().h(1).mx(4).my(5).flex_none().bg(th.border))
        .child(this.menuRow(th, "terminal", icon("icons/shell.svg", true, th.muted), t("menu.terminal"), { kind: "terminal" }))
        .child(label(t("menu.agents")))
        .children(choices.map((a) => this.menuRow(th, `agent-${a.key}`, icon(a.icon, a.tint, th.foreground), a.name,
          { kind: "agent", name: a.name, run: a.run }, a.installed ? "" : t("menu.not_installed"), a.installed)))
        .when(Boolean(agentsNote), (el) => el.child(note(t(agentsNote)))));
  }

  // ---- the bar --------------------------------------------------------------------

  // The side panel (Files / Changes) is remembered per Space, as Herdr's tab bar does:
  // switching shows that Space's choice (closed if it never had one), opening or closing records it.
  syncPanel(space, panel, body, enabled) {
    if (!space || !panel) return;
    const now = panel.hidden ? null : body?.active ?? null;
    if (space.id !== this.space) {
      const first = this.space === undefined;
      this.space = space.id;
      this.pending = null;
      if (!(first && !(space.id in this.panels))) {
        const stored = this.panels[space.id] ?? null, want = enabled.includes(stored) ? stored : null;
        if (want !== now) {
          this.pending = { want, until: Date.now() + 1000 };
          if (want) { run("layout.select", { stack: "panel-body", child: want }); run("layout.show", { id: "panel" }); }
          else run("layout.hide", { id: "panel" });
          return;
        }
      }
    }
    // Until the restore lands (or something else won), the old state isn't a choice.
    if (this.pending) {
      if (this.pending.want !== now && Date.now() < this.pending.until) return;
      this.pending = null;
    }
    if (this.panels[space.id] === now) return;
    this.panels[space.id] = now;
    localStorage.setItem("panels", JSON.stringify(this.panels));
  }

  render() {
    const th = theme(), tk = tokens();
    if (props()?.menu === "pane") return this.menu(th);
    const sp = spaces(), ps = panes(), badges = badgesOf(sp);
    const stop = (_e, cx) => cx.stop_propagation();
    const button = (id, child, tip, action, w = 28) => div().id(id).w(w).h(26).flex_none().flex().items_center().justify_center().rounded(6)
      .text_color(th.muted).hover((s) => s.bg(th.hover).text_color(th.foreground)).tooltip(tip)
      .on_mouse_down("left", stop).on_click(action).child(child);
    const shown = sp.flatMap((s) => s.tabs.filter((c) => !c.settled || (s.active && c.active)).map((c) => ({ s, c })));
    const item = ({ s, c }, i) => {
      const on = s.active && c.active;
      const mine = ps.filter((p) => p.tab === c.id);
      const name = tabName(c, mine);
      const badge = badges[s.id];
      const first = i > 0 && shown[i - 1].s.id !== s.id;
      return h_flex().id(`tab-${c.id}`).h(26).px(8).gap(6).rounded(6).items_center().flex_none().text_size(12.5)
        .when(first, (el) => el.ml(6))
        .border(1).border_color(on ? th.input : th.chrome).bg(on ? th.raised : th.chrome).text_color(on ? th.foreground : th.muted)
        .when(!on, (el) => el.hover((st) => st.bg(th.hover)))
        .tooltip(`${s.name} · ${name}`)
        .on_mouse_down("left", (e, cx) => { cx.stop_propagation(); this.pressed = c.id; if (e.click_count === 2) run("tab.rename", { id: c.id }); })
        .on_mouse_up("left", () => {
          const from = this.pressed;
          this.pressed = null;
          // Dropped on another tab of its own space: moved there.
          if (from && from !== c.id && s.tabs.some((x) => x.id === from)) run("tab.move", { id: from, index: c.index });
          else run("tab.select", { id: c.id });
        })
        .on_mouse_down("right", (_e, cx) => { cx.stop_propagation(); openOverlay({ mod: "menu", kind: "popup", width: 200, props: { tab: "tab", target: { id: c.id, space: s.id } } }); })
        .child(div().size(14).flex_none().rounded(3).bg(badge.color).flex().items_center().justify_center()
          .text_size(9).font_bold().text_color(tk.text.on_badge).child(badge.initial))
        .child(div().max_w(160).truncate().child(c.zoomed ? `${name} ⤢` : name))
        .when(Boolean(c.status) && c.status !== "idle", (el) => el.child(svg(`${STATUS}/${c.status}.svg`).size(8).flex_none().text_color(tk.status[c.status]?.mark ?? th.dim)))
        .when(on, (el) => el.child(div().id(`close-${c.id}`).size(16).flex_none().flex().items_center().justify_center().rounded(4).text_color(th.dim)
          .hover((st) => st.bg(th.hover).text_color(th.foreground)).tooltip(t("tab.close"))
          .on_mouse_down("left", stop).on_click((_e, cx) => { cx.stop_propagation(); run("tab.close", { id: c.id }); })
          .child(svg("mods/pane/close.svg").size(11))));
    };
    const root = layout(), sidebar = find(root, "sidebar"), w = window();
    // The macOS window buttons sit over the sidebar; with it hidden or narrow, leave them room.
    const lights = w?.platform === "macos" && w.titlebar === "none" && !w.fullscreen && root?.traffic_lights
      ? Math.max(6, (root.traffic_lights[0] + 64) / (settings().zoom || 1) - (sidebar?.hidden ? 0 : sidebar?.size?.px ?? 0))
      : 6;
    const winIcon = (name, glyph) => (w.platform === "windows" ? svg(`mods/tabs/win-${name}.svg`).size(16) : glyph);
    const panel = find(root, "panel"), body = find(root, "panel-body");
    const enabled = shared("workspace.panels") ?? ["files", "changes"];
    this.syncPanel(sp.find((s) => s.active), panel, body, enabled);
    const panelButton = (id, icon) => {
      const open = panel && !panel.hidden && body?.active === id;
      return h_flex().id(`panel-${id}`).h(26).px(8).gap(5).flex_none().items_center().rounded(6).text_size(12.5)
        .bg(open ? th.selected : th.chrome).text_color(open ? th.foreground : th.muted).hover((s) => s.bg(th.hover))
        .on_mouse_down("left", stop)
        .on_click(() => {
          if (open) run("layout.hide", { id: "panel" });
          else { run("layout.select", { stack: "panel-body", child: id }); run("layout.show", { id: "panel" }); }
        })
        .child(svg(icon).size(14)).child(t(`panel.${id}`));
    };
    // Split the focused pane; a layout terminal block (or nothing) has the keyboard: nothing to split.
    const target = focused()?.pane;
    const split = (id, direction) => target
      ? button(id, svg(`mods/pane/${id}.svg`).size(14), t(`pane.${id.replace("-", "_")}`), () => run("pane.split", { direction, pane: target }))
      : div().id(id).w(28).h(26).flex_none().flex().items_center().justify_center().text_color(th.dim).opacity(0.45)
        .tooltip(t("pane.no_pane")).child(svg(`mods/pane/${id}.svg`).size(14));
    return h_flex().id("root").size_full().min_w(0).pl(lights).pr(6).gap(6).items_center().bg(th.chrome).border_b(1).border_color(th.divider ?? th.border)
      .on_mouse_up("left", () => { this.pressed = null; })
      .on_scroll_wheel((e) => { if (Math.abs(e.delta.y) >= 1) run(e.delta.y > 0 ? "tab.prev" : "tab.next"); })
      .child(h_flex().id("tab-scroll").min_w(0).h_full().gap(4).items_center().overflow_x_scrollbar().children(shown.map(item)))
      .child(button("tab-new", svg("mods/workspace-chrome/plus.svg").size(14), t("tab.new"),
        () => openOverlay({ mod: "new-menu", kind: "popup", width: 250, height: 512 }), 26))
      .child(div().flex_1())
      .child(h_flex().id("pane-add").h(26).px(8).gap(6).flex_none().items_center().rounded(6).border(1).border_color(th.input)
        .text_size(12.5).text_color(th.foreground).hover((s) => s.bg(th.hover))
        .on_mouse_down("left", stop)
        .on_click(() => this.openPaneMenu())
        .child(svg(`mods/${SELF}/pane-add.svg`).size(14).text_color(th.muted))
        .child(t("pane.add"))
        .child(svg("mods/workspace-chrome/chevron-down.svg").size(11).text_color(th.dim)))
      .when(enabled.includes("files") && Boolean(panel), (el) => el.child(panelButton("files", "mods/files/file.svg")))
      .when(enabled.includes("changes") && Boolean(panel), (el) => el.child(panelButton("changes", "mods/changes/changes.svg")))
      .child(split("split-right", "right"))
      .child(split("split-down", "down"))
      .when(Boolean(w?.buttons), (el) => el.child(div().w(1).h(16).flex_none().bg(th.border))
        .child(button("minimize", winIcon("min", "—"), t("window.minimize"), () => run("window.minimize"), 30))
        .child(button("maximize", winIcon(w.maximized ? "restore" : "max", "□"), t("window.maximize"), () => run("window.maximize"), 30))
        .child(button("close-window", winIcon("close", svg("mods/pane/close.svg").size(13)), t("window.close"), () => run("window.close"), 30)));
  }
}
