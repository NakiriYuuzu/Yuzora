// The sidebar's mode bar: switches the yuzora-sidebar stack, opens settings. Owned by lane L1.
import { View, div, svg } from "gpui-kit";
import { h_flex } from "gpui-base";
import { layout, run, theme } from "herdr";
import { t } from "yuzora:core";

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

export default class Modebar extends View {
  render() {
    const th = theme();
    const active = find(layout(), STACK)?.active ?? `${STACK}-workspace`;
    const button = (mode) => {
      const child = `${STACK}-${mode}`, on = active === child;
      return div().id(`mode-${mode}`).w(36).h(32).flex().flex_none().items_center().justify_center().rounded(6)
        .bg(on ? th.selected : th.chrome).text_color(on ? th.foreground : th.muted)
        .when(on, (el) => el.border_b(2).border_color(th.foreground))
        .hover((s) => s.bg(th.hover))
        .tooltip(t(`mode.${mode}`))
        .on_click(() => run("layout.select", { stack: STACK, child }))
        .child(svg(`mods/yuzora-modebar/${mode}.svg`).size(16));
    };
    return h_flex().id("root").size_full().px(6).gap(2).items_center().bg(th.chrome).border_t(1).border_color(th.divider ?? th.border)
      .children(MODES.map(button))
      .child(div().flex_1())
      .child(div().id("settings").w(32).h(32).flex().flex_none().items_center().justify_center().rounded(6).text_color(th.muted)
        .hover((s) => s.bg(th.hover)).tooltip(t("settings"))
        .on_click(() => run("app.settings"))
        .child(svg("mods/yuzora-modebar/settings.svg").size(16)));
  }
}
