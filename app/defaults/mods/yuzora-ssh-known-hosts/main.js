// Known hosts: Yuzora's own known_hosts (entries can be removed) and the
// ones under ~/.ssh and /etc/ssh, read only. Props: { host? } fills the
// filter with that host's name. Owned by lane L4.
import { View, div } from "gpui-kit";
import { v_flex, h_flex, InputState, Input } from "gpui-base";
import { theme, props, closeOverlay } from "herdr";
import { t } from "yuzora:core";
import { hosts, knownHosts, removeKnownHost } from "yuzora:ssh";

const MONO = "Menlo";
const kind = (a) => (/ed25519/i.test(a) ? "ED25519" : /ecdsa/i.test(a) ? "ECDSA" : /rsa/i.test(a) ? "RSA" : a.toUpperCase());

export default class KnownHosts extends View {
  init(_props, cx) {
    const h = props()?.host ? hosts().find((x) => x.id === props().host) : null;
    this.filter = InputState.new({ placeholder: t("filter"), value: h?.hostname ?? "" });
    this.filter.on("change", (_e, cx) => cx.notify());
    this.lines = knownHosts();
    this.confirming = null;
    this.handle = cx.focus_handle();
    this.filter.focus();
  }

  dismiss() {
    closeOverlay(props().overlay);
  }

  matches(l, q) {
    if (!q) return true;
    q = q.toLowerCase();
    return l.patterns.toLowerCase().includes(q) || l.fingerprint.toLowerCase().includes(q) || (l.hashed && false);
  }

  row(th, l) {
    const key = `${l.file}:${l.line}`;
    const asking = this.confirming === key;
    return h_flex()
      .id(`kh-${key}`)
      .min_h(40)
      .px(10)
      .gap(10)
      .items_center()
      .rounded(6)
      .hover((s) => s.bg(th.hover))
      .child(
        v_flex()
          .flex_1()
          .min_w(0)
          .child(h_flex().gap(6).items_center()
            .child(div().truncate().text_size(12.5).text_color(l.hashed ? th.muted : th.foreground).child(l.hashed ? t("hashed") : l.patterns))
            .when(l.marker === "revoked", (el) => el.child(div().text_size(10.5).text_color(th.blocked).child(t("revoked"))))
            .when(l.marker === "cert-authority", (el) => el.child(div().text_size(10.5).text_color(th.done).child(t("ca")))))
          .child(div().truncate().text_size(11).font_family(MONO).text_color(th.dim).child(l.fingerprint)),
      )
      .child(div().w(64).flex_none().text_size(11).font_semibold().text_color(th.muted).child(kind(l.algorithm)))
      .when(l.editable, (el) =>
        el.child(
          div()
            .id(`kh-rm-${key}`)
            .h(24)
            .px(9)
            .flex()
            .flex_none()
            .items_center()
            .rounded(5)
            .border(1)
            .border_color(asking ? th.blocked : th.input)
            .text_size(11.5)
            .text_color(asking ? th.blocked : th.muted)
            .cursor_pointer()
            .hover((s) => s.bg(th.raised))
            .on_click((_e, cx) => {
              if (!asking) this.confirming = key;
              else {
                removeKnownHost(l.file, l.line, l.raw);
                this.lines = knownHosts();
                this.confirming = null;
              }
              cx.notify();
            })
            .child(asking ? t("confirm") : t("remove")),
        ),
      );
  }

  render() {
    const th = theme();
    const q = this.filter.value().trim();
    const files = [...new Set(this.lines.map((l) => l.file))];
    const groups = files.map((f) => ({ file: f, editable: this.lines.find((l) => l.file === f)?.editable, lines: this.lines.filter((l) => l.file === f && this.matches(l, q)) }));
    return v_flex()
      .id("known-hosts")
      .track_focus(this.handle)
      .on_key_down((e, cx) => { if (e.key === "escape") { this.dismiss(); cx.stop_propagation(); } })
      .size_full()
      .p(18)
      .gap(10)
      .bg(th.popover)
      .text_color(th.foreground)
      .child(h_flex().items_center().child(div().flex_1().text_size(15).font_semibold().child(t("title")))
        .child(div().id("kh-close").text_size(12).text_color(th.dim).cursor_pointer().on_click(() => this.dismiss()).child(t("close"))))
      .child(div().text_size(12).line_height("18px").text_color(th.muted).child(t("body")))
      .child(Input.new(this.filter).h(30).px(10).items_center().rounded(6).border(1).border_color(th.input).bg(th.background).text_size(12.5).text_color(th.foreground))
      .child(
        v_flex()
          .id("kh-list")
          .flex_1()
          .min_h(0)
          .gap(2)
          .overflow_y_scrollbar()
          .when(this.lines.length === 0, (el) => el.child(div().p(10).text_size(12).text_color(th.dim).child(t("none"))))
          .children(groups.flatMap((g) => [
            div().pt(8).pb(2).px(10).text_size(11).font_semibold().text_color(th.dim).child(t(g.editable ? "own" : "system", { path: g.file })),
            ...(g.lines.length ? g.lines.map((l) => this.row(th, l)) : [div().px(10).py(4).text_size(11.5).text_color(th.dim).child(t("none"))]),
            ...(g.editable ? [] : [div().px(10).pb(4).text_size(11).text_color(th.dim).child(t("readonly"))]),
          ])),
      );
  }
}
