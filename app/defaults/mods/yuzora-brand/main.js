// The sidebar's top row: Yuzora's mark, clear of the macOS window buttons. Owned by lane L1.
import { View, div } from "gpui-kit";
import { h_flex } from "gpui-base";
import { theme, layout, window, settings } from "herdr";
import { t } from "yuzora:core";

export default class Brand extends View {
  render() {
    const th = theme(), w = window(), root = layout();
    // The traffic lights sit over this row when the layout draws its own title bar.
    const lights = w?.platform === "macos" && w.titlebar === "none" && !w.fullscreen && root?.traffic_lights;
    const left = lights ? Math.max(12, (root.traffic_lights[0] + 64) / (settings().zoom || 1)) : 12;
    return h_flex().id("root").size_full().pl(left).pr(8).gap(8).items_center().bg(th.chrome)
      .child(div().w(18).h(18).flex_none().rounded(5).bg(th.foreground).text_color(th.chrome)
        .text_size(11).font_bold().flex().items_center().justify_center().child("Y"))
      .child(div().flex_1().text_size(13.5).font_bold().text_color(th.foreground).child(t("name")));
  }
}
