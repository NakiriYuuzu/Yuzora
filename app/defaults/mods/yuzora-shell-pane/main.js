// A pane's header, for the yuzora layout's pane frames. Owned by lane L1.
//   { "tab": "active", "pane": { "header": { "content": { "mod": "yuzora-shell-pane" } } } }
// 28px: the Space's badge, the program's icon, its name, "folder · branch", the agent's
// state (shape and word), then split / zoom on hover and close. A pane whose agent needs a
// reply gets an amber header (the frame's `blocked` border goes with it). Tool panes
// (mod panes) show the name and icon their mod.json gives the new-pane menu.
// Props from the app: { pane, terminal }. Click: focus the pane.

import { View, div, svg, image } from "gpui-kit";
import { h_flex } from "gpui-base";
import { panes, spaces, props, run, theme } from "herdr";
import { t, tokens, panes as toolPanes } from "yuzora:core";
import { spaceBadge } from "yuzora:shell";

const STATUS = "mods/yuzora-sidebar-workspace/status";

const base = (path) => (path ?? "").replace(/[/\\]+$/, "").split(/[/\\]/).pop() || path;

// What the header says about a pane: name, icon and where it is.
export function describe(p, tools) {
  if (p.mod) {
    const tool = tools.find((x) => x.mod === p.mod);
    return { name: tool?.label ?? p.title ?? p.mod, icon: tool?.icon ?? null, tint: true, where: "" };
  }
  const prog = p.program;
  const name = ((prog?.agent || prog?.shell) && prog.name) || p.title || prog?.name || "";
  const where = [base(p.cwd), p.branch].filter(Boolean).join(" · ");
  return { name, icon: prog?.icon ?? null, tint: Boolean(prog?.tint), where };
}

export default class PaneHeader extends View {
  init() { this.hovered = false; }

  button(th, id, icon, tip, command, args) {
    return div().id(id).size(20).flex_none().flex().items_center().justify_center().rounded(4).text_color(th.dim)
      .hover((s) => s.bg(th.hover).text_color(th.foreground)).tooltip(tip)
      .on_click((_e, cx) => { cx.stop_propagation(); run(command, args); })
      .child(svg(icon).size(13));
  }

  render() {
    const th = theme(), tk = tokens();
    const p = panes().find((x) => x.pane === props()?.pane);
    if (!p) return div();
    const space = spaces().find((s) => s.tabs.some((c) => c.id === p.tab));
    const badge = space ? spaceBadge(space.id, space.name) : null;
    const d = describe(p, p.mod ? toolPanes() : []);
    const st = p.mod ? null : p.status;
    const blocked = st === "blocked";
    const look = st ? tk.status[st] ?? tk.status.unknown : null;
    const pane = p.pane;
    return h_flex().id(`pane-head-${pane}`).size_full().min_w(0).pl(8).pr(6).gap(7).items_center()
      .bg(blocked ? tk.status.blocked.header_bg : th.background).border_b(1).border_color(th.divider ?? th.border)
      .text_size(12).text_color(th.foreground)
      .on_hover((hovered, cx) => { this.hovered = hovered; cx.notify(); })
      .on_click(() => run("pane.focus", { pane }))
      .when(Boolean(badge), (el) => el.child(div().size(16).flex_none().rounded(4).bg(badge.color).flex().items_center().justify_center()
        .text_size(9.5).font_bold().text_color(tk.text.on_badge).child(badge.initial)))
      .when(Boolean(d.icon), (el) => el.child(d.tint ? svg(d.icon).size(14).flex_none().text_color(th.muted) : image(d.icon).size(14).flex_none()))
      .child(div().flex_none().max_w(220).truncate().font_semibold().child(d.name || t("shell")))
      .child(div().flex_1().min_w(0).truncate().text_color(th.dim).child(d.where))
      .when(Boolean(look), (el) => el.child(h_flex().flex_none().gap(5).items_center().text_size(11.5).text_color(look.color)
        .child(svg(`${STATUS}/${tk.status[st] ? st : "unknown"}.svg`).size(8).text_color(look.mark))
        .child(t(`status.${tk.status[st] ? st : "unknown"}`))))
      .when(this.hovered, (el) => el
        .child(this.button(th, `ph-r-${pane}`, "mods/pane/split-right.svg", t("split_right"), "pane.split", { direction: "right", pane }))
        .child(this.button(th, `ph-d-${pane}`, "mods/pane/split-down.svg", t("split_down"), "pane.split", { direction: "down", pane }))
        .child(this.button(th, `ph-z-${pane}`, `mods/pane/${p.zoomed ? "unzoom" : "zoom"}.svg`, t(p.zoomed ? "unzoom" : "zoom"), "pane.zoom", { pane })))
      .child(this.button(th, `ph-x-${pane}`, "mods/pane/close.svg", t("close"), "pane.close", { pane }));
  }
}
