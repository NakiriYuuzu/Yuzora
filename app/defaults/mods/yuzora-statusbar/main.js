// The status bar. Owned by lane L1. Left to right: panes · Spaces, agents, who needs a
// reply (amber), the focused pane's branch ↑ahead ↓behind, each lane's
// shared("yuzora.status.<lane>") ({ text, tone? }), then herdr-core's version · machines and
// the command palette's key.

import { View, div, svg } from "gpui-kit";
import { h_flex } from "gpui-base";
import { spaces, agents, panes, machines, focused, updates, keys, window, theme, run, shared, setShared, layout } from "herdr";
import { t, tokens } from "yuzora:core";
import { spaceBadges, gitStatus } from "yuzora:shell";

// The lanes' status keys, in lane order (yuzora.status.<lane>).
export const LANES = ["shell", "git", "db", "database", "ssh", "files", "settings", "release"];
const TONES = ["muted", "working", "blocked", "done", "accent"];
const STATUS = "mods/yuzora-sidebar-workspace/status";

// Spaces' badges (initial and kept color). The colors live in shared state; a Space seen the
// first time gets one and it's written back, so every mod and every restart agree.
const COLORS = "yuzora.space.colors";
function badgesOf(list) {
  const r = spaceBadges(list.map((s) => ({ id: s.id, name: s.name })), shared(COLORS));
  if (r.changed) setShared(COLORS, r.kept);
  return r.badges;
}

// Whether a mode bar is on screen: a block (or a float) of it that no hidden node holds.
export function modebarShown(node) {
  if (!node || node.hidden) return false;
  if (node.content?.mod === "yuzora-modebar") return true;
  if ((node.floating ?? []).some((f) => !f.hidden && f.content?.mod === "yuzora-modebar")) return true;
  return (node.children ?? []).some(modebarShown);
}

function hasNode(node, id) {
  return node?.id === id || (node?.children ?? []).some((c) => hasNode(c, id));
}

function shortcut(action) {
  const k = keys().find((b) => b.section === "global" && b.action === action);
  if (!k?.keys.trim()) return "";
  const mac = window()?.platform === "macos";
  const names = mac ? { ctrl: "⌃", alt: "⌥", shift: "⇧", cmd: "⌘" } : { ctrl: "Ctrl", alt: "Alt", shift: "Shift", cmd: "Super" };
  const alias = { secondary: mac ? "cmd" : "ctrl", super: "cmd", win: "cmd" };
  const stroke = k.keys.trim().split(/\s+/)[0].split("-");
  let key = stroke.pop();
  if (key === "") key = "-";
  const held = new Set(stroke.map((m) => alias[m.toLowerCase()] ?? m.toLowerCase()));
  const label = key.length === 1 ? key.toUpperCase() : key[0].toUpperCase() + key.slice(1);
  return [...Object.keys(names).filter((m) => held.has(m)).map((m) => names[m]), label].join(mac ? "" : "+");
}

export default class Statusbar extends View {
  init(_props, cx) {
    this.git = {}; // JSON [machine, cwd] -> gitStatus() (null: not a repo)
    this.asked = {}; // the same key -> when it was last asked
    this.asking = new Set(); // the keys being asked now
    cx.timer.every(2000, (cx) => this.refresh(cx));
  }

  // The focused pane's folder: asked in the frame it is first drawn in (render), then every 15 s while
  // the window is in front.
  refresh(cx) {
    const key = this.key();
    if (!key) return;
    if (key in this.git && (!window()?.active || Date.now() - (this.asked[key] ?? 0) < 15000)) return;
    this.ask(cx, key);
  }

  // One ask at a time for a folder, but not one at a time for all: a slow answer for the folder the focus
  // left does not hold up the one it moved to.
  ask(cx, key) {
    if (this.asking.has(key)) return;
    this.asking.add(key);
    this.asked[key] = Date.now();
    const [on, cwd] = JSON.parse(key);
    cx.spawn(async (cx) => {
      try { this.git[key] = await gitStatus(cwd, { on }); } catch { this.git[key] ??= null; }
      this.asking.delete(key);
      cx.notify();
    });
  }

  // The mode bar opens Git where it is on screen: the right dock's panel or window, or an old clone's
  // sidebar stack (the legacy bar takes the command too). With the bar hidden the command would fail
  // with a toast: an old clone's stack is selected as before; a dock layout has nothing to open.
  openGit() {
    const tree = layout();
    if (modebarShown(tree)) run("yuzora-modebar.git");
    else if (hasNode(tree, "yuzora-sidebar")) run("layout.select", { stack: "yuzora-sidebar", child: "yuzora-sidebar-git" });
  }

  // The focused terminal's machine: its address is "<machine>/<id>" off this computer. A layout
  // terminal block can show another machine's terminal than the Space's; a mod pane has none.
  key() {
    const f = focused();
    if (!f?.cwd) return null;
    const machine = f.terminal
      ? (f.terminal.includes("/") ? f.terminal.slice(0, f.terminal.indexOf("/")) : null)
      : spaces().find((s) => s.id === f.space)?.machine ?? null;
    return JSON.stringify([machine, f.cwd]);
  }

  render(cx) {
    const th = theme(), tk = tokens();
    const sp = spaces(), list = agents(), ps = panes(), badges = badgesOf(sp);
    const waiting = list.filter((a) => a.status === "blocked");
    const working = list.some((a) => a.status === "working");
    const f = focused();
    const space = sp.find((s) => s.id === f?.space);
    const key = this.key();
    if (key && !(key in this.git)) this.ask(cx, key);
    const git = key ? this.git[key] : null;
    const shown = f?.pane ? ps.find((p) => p.pane === f.pane) : ps.find((p) => f?.terminal && p.terminal === f.terminal);
    const branch = git?.branch ?? shown?.branch ?? null;
    const badge = space ? badges[space.id] : null;
    const core = updates()?.core?.version;
    const where = machines().map((m) => m.name ?? t("local")).join(" / ");
    const lanes = LANES.map((lane) => [lane, shared(`yuzora.status.${lane}`)]).filter(([, s]) => s && typeof s.text === "string" && s.text);
    const tone = (name) => {
      const n = TONES.includes(name) ? name : "muted";
      return n === "muted" ? th.muted : n === "accent" ? th.accent : tk.status[n].color;
    };
    const item = (id) => h_flex().id(id).flex_none().gap(5).items_center();
    const palette = shortcut("app.palette");
    return h_flex().id("root").size_full().min_w(0).px(10).gap(14).items_center().bg(th.chrome).border_t(1).border_color(th.divider ?? th.border)
      .text_size(11.5).text_color(th.muted)
      .child(item("panes").child(t("summary", { panes: ps.length, spaces: sp.length })))
      .child(item("agents").child(svg(`${STATUS}/${working ? "working" : "idle"}.svg`).size(8).text_color(working ? tk.status.working.mark : th.dim))
        .child(t("agents", { count: list.length })))
      .when(waiting.length > 0, (el) => el.child(item("needs").cursor_pointer().text_color(tk.status.blocked.color)
        .on_click(() => run("pane.focus", { pane: waiting[0].pane }))
        .child(svg(`${STATUS}/blocked.svg`).size(9).text_color(tk.status.blocked.mark))
        .child(t("needs", { count: waiting.length }))))
      .when(Boolean(branch), (el) => el.child(item("branch").min_w(0).cursor_pointer().hover((s) => s.text_color(th.foreground))
        .tooltip(t("branch.open"))
        .on_click(() => this.openGit())
        .when(Boolean(badge), (el) => el.child(div().size(14).flex_none().rounded(3).bg(badge.color).flex().items_center().justify_center()
          .text_size(9).font_bold().text_color(tk.text.on_badge).child(badge.initial)))
        .child(svg("mods/workspace-chrome/branch.svg").size(12).flex_none())
        .child(div().max_w(180).truncate().child(branch))
        .when(git?.ahead > 0, (el) => el.child(div().text_color(tk.diff.added).child(`↑${git.ahead}`)))
        .when(git?.behind > 0, (el) => el.child(div().text_color(tk.diff.removed).child(`↓${git.behind}`)))))
      // The lanes' texts share what's left and shorten (then clip), so HERDR and the palette key stay in view.
      .child(h_flex().id("lanes").flex_1().min_w(0).gap(14).items_center().overflow_x_hidden()
        .children(lanes.map(([lane, s]) => h_flex().id(`lane-${lane}`).min_w(0).max_w(240).flex_shrink(1).items_center()
          .text_color(tone(s.tone)).tooltip(s.text).child(div().min_w(0).truncate().child(s.text)))))
      .child(item("core").max_w(320).child(div().truncate().child(core ? t("core", { version: core, machines: where }) : where)))
      .child(item("palette").cursor_pointer().text_color(th.dim).hover((s) => s.text_color(th.foreground))
        .on_click(() => run("app.palette"))
        .child(palette ? t("palette_key", { key: palette }) : t("palette")));
  }
}
