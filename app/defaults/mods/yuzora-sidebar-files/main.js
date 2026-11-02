// The sidebar's Files mode (placeholder). Owned by lane L5.
import { View, div } from "gpui-kit";
import { v_flex, h_flex } from "gpui-base";
import { theme } from "herdr";
import { t } from "yuzora:core";

export default class SidebarFiles extends View {
  render() {
    const th = theme();
    return v_flex().id("root").size_full().bg(th.chrome).text_color(th.foreground).text_size(12.5)
      .child(h_flex().h(32).px(12).flex_none().items_center().child(div().font_semibold().child(t("title"))))
      .child(div().px(12).pt(4).text_size(11.5).text_color(th.dim).child(t("placeholder", { lane: "L5" })));
  }
}
