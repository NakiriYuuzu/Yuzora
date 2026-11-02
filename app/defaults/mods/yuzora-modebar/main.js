// The sidebar's mode bar: switches the yuzora-sidebar stack, shows each mode's badge,
// opens settings. Owned by lane L1.
// A mode's badge is shared("yuzora.badge.<mode>"): a number, or null for none. Each lane
// sets its own; the workspace mode's (agents waiting for a reply) is amber.

import { View, div, svg } from "gpui-kit";
import { h_flex } from "gpui-base";
import { layout, run, theme, shared } from "herdr";
import { t, tokens } from "yuzora:core";

const STACK = "yuzora-sidebar";
const MODES = ["workspace", "files", "git", "database", "ssh"];

function find(node, id) {
  if (node?.id === id) return node;
  for (const child of node?.children ?? []) {
    const found = find(child, id);
    if (found) return found;
  }
  return null;
}

export function badgeText(value) {
  const n = typeof value === "number" ? Math.floor(value) : 0;
  return n > 0 ? (n > 99 ? "99+" : String(n)) : null;
}

export default class Modebar extends View {
  render() {
    const th = theme(), tk = tokens();
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
}
