// The Database settings page: the row limit of a result (db.maxRows). Owned by lane L3.
// The value goes through yuzora:db, which leaves the saved connections of the same
// settings section alone; a query reads it when it starts (no restart).
import { View, div } from "gpui-kit";
import { v_flex, h_flex, InputState, Input } from "gpui-base";
import { theme } from "herdr";
import { t } from "yuzora:core";
import { rowLimit, setRowLimit } from "yuzora:db";

export default class DbSettings extends View {
  init(_props, _cx) {
    this.limit = rowLimit();
    this.error = null;
    // Saved on Enter or when the field loses focus, like the Git page's fields.
    this.input = InputState.new({ value: String(this.limit.value) });
    this.input.on("submit", (_e, cx) => this.commit(cx));
    this.input.on("blur", (_e, cx) => this.commit(cx));
  }

  store(cx, rows) {
    try {
      this.limit = { ...this.limit, value: setRowLimit(rows) };
      this.error = null;
    } catch (e) { this.error = t("error", { error: String(e?.message ?? e) }); }
    this.input.set_value(String(this.limit.value));
    cx.notify();
  }

  commit(cx) {
    const raw = this.input.value().trim(), n = Math.round(Number(raw));
    if (raw === "" || !Number.isFinite(n) || n === this.limit.value) { this.input.set_value(String(this.limit.value)); return; }
    this.store(cx, n);
  }

  render(_cx) {
    const th = theme(), l = this.limit;
    const changed = l.value !== l.default;
    return v_flex().id("db-settings").size_full().min_h(0).overflow_y_scrollbar().bg(th.background).text_color(th.foreground).text_size(12.5)
      .child(v_flex().w_full().max_w(760).px(24).py(12).gap(14)
        .child(v_flex().id("group-results").gap(0)
          .child(div().pb(2).text_size(11.5).font_semibold().text_color(th.muted).child(t("group.results")))
          .child(h_flex().id("row-maxRows").py(9).gap(16).items_center().border_b(1).border_color(th.divider ?? th.border)
            .child(v_flex().flex_1().min_w(0).gap(2)
              .child(div().text_size(12.5).child(t("s.maxRows")))
              .child(div().text_size(11).text_color(th.dim).child(t("s.maxRows.desc", { min: l.min, max: l.max, default: l.default })))
              .when(Boolean(this.error), (el) => el.child(div().text_size(11).text_color(th.ansi1).child(this.error))))
            .child(h_flex().flex_none().gap(8).items_center()
              .when(changed, (el) => el.child(div().id("reset-maxRows").h(22).px(8).flex().items_center().rounded(4).text_size(11.5).cursor_pointer()
                .bg(th.raised).text_color(th.muted).hover((s) => s.text_color(th.foreground))
                .on_click((_e, cx) => this.store(cx, l.default)).child(t("reset"))))
              .child(div().id("c-maxRows").h(26).px(8).w(96).flex().items_center().rounded(6).border_1().border_color(th.border).bg(th.background)
                .text_size(12).on_mouse_down("left", (_e, cx) => cx.stop_propagation()).child(Input.new(this.input).w_full()))))));
  }
}
