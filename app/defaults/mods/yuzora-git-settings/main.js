// The Git settings page: one control per key of the git section, grouped like
// IntelliJ's Settings | Version Control | Git. Owned by lane L2.
// Keys and options mirror crates/yuzora-git/src/settings.rs; setSettings only
// takes known keys, and git reads them where it uses them (no restart).
import { View, div } from "gpui-kit";
import { v_flex, h_flex, InputState, Input } from "gpui-base";
import { theme } from "herdr";
import { t } from "yuzora:core";
import { settings as read, setSettings, checkPattern } from "yuzora:git";

const GROUPS = [
  { id: "general", items: [
    { key: "executable", type: "text", wide: true },
    { key: "warnCrlf", type: "bool" },
    { key: "warnDetachedHead", type: "bool" },
    { key: "largeFileWarn", type: "bool" },
    { key: "largeFileMb", type: "number", min: 1, max: 100000, needs: "largeFileWarn" },
  ] },
  { id: "commit", items: [
    { key: "signOff", type: "bool" },
    { key: "skipHooks", type: "bool" },
    { key: "commitCleanup", type: "select", options: ["strip", "whitespace", "verbatim"] },
    { key: "cherryPickSuffix", type: "bool" },
    { key: "messageHistory", type: "number", min: 1, max: 100 },
  ] },
  { id: "push", items: [
    { key: "forceWithLease", type: "bool" },
    { key: "autoUpdateOnReject", type: "bool" },
    { key: "pushTags", type: "select", options: ["none", "follow", "all"] },
    { key: "showPushDialog", type: "bool" },
    { key: "protectedBranches", type: "text", wide: true, regex: true },
  ] },
  { id: "update", items: [
    { key: "updateMethod", type: "select", options: ["merge", "rebase", "ff-only"] },
    { key: "autoStash", type: "bool" },
    { key: "fetchTags", type: "select", options: ["auto", "always", "never"] },
    { key: "fetchPrune", type: "bool" },
    { key: "autoFetchMinutes", type: "number", min: 0, max: 1440 },
  ] },
  { id: "log", items: [
    { key: "logRecentCount", type: "number", min: 100, max: 100000 },
    { key: "logShowTags", type: "bool" },
    { key: "logShowRemotes", type: "bool" },
  ] },
  { id: "diff", items: [
    { key: "diffMode", type: "select", options: ["side", "unified"] },
    { key: "diffIgnoreWhitespace", type: "select", options: ["none", "trim", "all"] },
    { key: "diffContext", type: "number", min: -1, max: 1000 },
    { key: "annotateIgnoreSpaces", type: "bool" },
    { key: "annotateDetectMovement", type: "select", options: ["none", "inner", "outer"] },
  ] },
  { id: "merge", items: [
    { key: "swapSidesInRebase", type: "bool" },
  ] },
];

export default class GitSettings extends View {
  init(_props, cx) {
    this.s = read();
    this.error = null;
    // Text and number fields keep their own state; they are saved on Enter or when they lose focus.
    this.inputs = {};
    for (const { items } of GROUPS) {
      for (const it of items) {
        if (it.type !== "text" && it.type !== "number") continue;
        const st = InputState.new({ value: String(this.s[it.key] ?? "") });
        st.on("submit", (_e, cx) => this.commitField(it, cx));
        st.on("blur", (_e, cx) => this.commitField(it, cx));
        this.inputs[it.key] = st;
      }
    }
  }

  set(cx, values) {
    try {
      setSettings(values);
      this.s = { ...this.s, ...values };
      this.error = null;
    } catch (e) { this.error = { key: Object.keys(values)[0], text: t("error", { error: String(e) }) }; }
    cx.notify();
  }

  commitField(it, cx) {
    const st = this.inputs[it.key], raw = st.value().trim();
    if (it.type === "number") {
      const n = Math.round(Number(raw));
      if (raw === "" || !Number.isFinite(n)) { st.set_value(String(this.s[it.key] ?? "")); return; }
      const v = Math.min(it.max, Math.max(it.min, n));
      st.set_value(String(v));
      if (v !== this.s[it.key]) this.set(cx, { [it.key]: v });
      return;
    }
    if (it.regex) {
      // Checked by the engine: JavaScript's RegExp also takes look-around, which the engine can't read.
      if (checkPattern(raw) !== null) { this.error = { key: it.key, text: t("regex.invalid") }; cx.notify(); return; }
    }
    if (raw !== (this.s[it.key] ?? "")) this.set(cx, { [it.key]: raw });
    else if (this.error) { this.error = null; cx.notify(); }
  }

  control(it, th) {
    const v = this.s[it.key];
    if (it.type === "bool") {
      const on = Boolean(v);
      return div().id(`c-${it.key}`).w(16).h(16).flex().flex_none().items_center().justify_center().rounded(4).border_1().cursor_pointer()
        .border_color(on ? th.accent : th.input ?? th.border).bg(on ? th.accent : th.background).text_size(11).text_color(th.background)
        .on_click((_e, cx) => this.set(cx, { [it.key]: !on })).child(on ? "✓" : "");
    }
    if (it.type === "select") {
      return h_flex().gap(2).p(2).rounded(6).bg(th.raised).flex_wrap()
        .children(it.options.map((o) => div().id(`c-${it.key}-${o}`).h(22).px(8).flex().items_center().rounded(4).text_size(11.5).cursor_pointer()
          .bg(v === o ? th.selected : th.raised).text_color(v === o ? th.foreground : th.muted).hover((s) => s.text_color(th.foreground))
          .on_click((_e, cx) => { if (v !== o) this.set(cx, { [it.key]: o }); }).child(t(`opt.${it.key}.${o}`))));
    }
    return div().id(`c-${it.key}`).h(26).px(8).w(it.wide ? 260 : 96).flex().items_center().rounded(6).border_1().border_color(th.border).bg(th.background)
      .text_size(12).on_mouse_down("left", (_e, cx) => cx.stop_propagation()).child(Input.new(this.inputs[it.key]).w_full());
  }

  row(it, th) {
    const off = it.needs && !this.s[it.needs];
    return h_flex().id(`row-${it.key}`).py(9).gap(16).items_center().border_b(1).border_color(th.divider ?? th.border).when(off, (el) => el.opacity(0.5))
      .child(v_flex().flex_1().min_w(0).gap(2)
        .child(div().text_size(12.5).child(t(`s.${it.key}`)))
        .child(div().text_size(11).text_color(th.dim).child(t(`s.${it.key}.desc`)))
        .when(this.error?.key === it.key, (el) => el.child(div().text_size(11).text_color(th.ansi1).child(this.error.text))))
      .child(div().flex_none().child(this.control(it, th)));
  }

  render(_cx) {
    const th = theme();
    return v_flex().id("git-settings").size_full().min_h(0).overflow_y_scrollbar().bg(th.background).text_color(th.foreground).text_size(12.5)
      .child(v_flex().w_full().max_w(760).px(24).py(12).gap(14)
        .children(GROUPS.map(({ id, items }) => v_flex().id(`group-${id}`).gap(0)
          .child(div().pb(2).text_size(11.5).font_semibold().text_color(th.muted).child(t(`group.${id}`)))
          .children(items.map((it) => this.row(it, th))))));
  }
}
