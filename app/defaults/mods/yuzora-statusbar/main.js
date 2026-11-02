// The status bar (placeholder): panes and spaces, agents, who needs you, the palette. Owned by lane L1.
import { View, div } from "gpui-kit";
import { h_flex } from "gpui-base";
import { spaces, agents, panes, theme, run } from "herdr";
import { t, version } from "yuzora:core";

export default class Statusbar extends View {
  render() {
    const th = theme();
    const list = agents();
    const needs = list.filter((a) => a.status === "blocked").length;
    return h_flex().id("root").size_full().px(12).gap(14).items_center().bg(th.chrome).border_t(1).border_color(th.divider ?? th.border)
      .text_size(11.5).text_color(th.muted)
      .child(div().child(t("summary", { panes: panes().length, spaces: spaces().length })))
      .child(div().child(t("agents", { count: list.length })))
      .when(needs > 0, (el) => el.child(div().text_color(th.blocked).child(t("needs", { count: needs }))))
      .child(div().flex_1())
      .child(div().text_color(th.dim).child(`Yuzora ${version()}`))
      .child(div().id("palette").text_color(th.dim).hover((s) => s.text_color(th.foreground)).on_click(() => run("app.palette")).child(t("palette")));
  }
}
