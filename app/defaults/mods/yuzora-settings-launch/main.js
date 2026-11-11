// Start an agent from a herdr-core template, into a space and tab of this
// window. A dialog: `mod.open {name: "yuzora-settings-launch"}`, or
// openOverlay({mod: "yuzora-settings-launch", kind: "dialog", width: 560,
// props: {template?, machine?, space?, tab?, task?, closeAlso?}}).
// Owned by lane L6.
//
// runTemplate (yuzora:settings) runs `hc run --bg` with the herdr.space and
// herdr.tab labels; the app then shows the agent in that tab (a new tab when
// "New tab" is picked). The dialog stays until its pane is there, focuses it,
// and closes (with the settings dialog it came from, `closeAlso`).

import { View, div } from "gpui-kit";
import { v_flex, h_flex, InputState, TextareaState, Textarea, Scrollbar } from "gpui-base";
import { theme, spaces, panes, focused, run, props, closeOverlay, notify } from "herdr";
import { t } from "yuzora:core";
import { listTemplates, runTemplate } from "yuzora:settings";
import { title, button, note, spinner, field, input, item, badge } from "./ui.js";

const NEW_TAB = "";

export default class Launch extends View {
  init(_props, cx) {
    const p = props() ?? {};
    const all = spaces();
    // A template is one machine's (props.machine, null: this one): a space on
    // that machine, the active one first; none there, no template picked.
    const asked = "machine" in p;
    const on = (s) => !asked || (s.machine ?? null) === (p.machine ?? null);
    const space = all.find((s) => s.id === p.space) ?? all.find((s) => s.active && on(s)) ?? all.find(on) ?? all.find((s) => s.active) ?? all[0] ?? null;
    this.space = space?.id ?? null;
    this.tab = p.tab ?? space?.tabs.find((tb) => tb.active)?.id ?? NEW_TAB;
    this.template = space && on(space) ? (p.template ?? null) : null;
    this.lists = {}; // machine ("" here) -> TemplateList
    this.state = null; // null | "starting" | "placing"
    this.error = null;
    this.task = TextareaState.new({ placeholder: t("task_placeholder"), rows: 4 });
    this.task.set_value(p.task ?? "");
    this.name = InputState.new({ placeholder: t("name_placeholder") });
    this.name.on("focus", (_e, cx) => { this.nameFocused = true; cx.notify(); });
    this.name.on("blur", (_e, cx) => { this.nameFocused = false; cx.notify(); });
    this.handle = cx.focus_handle();
    this.task.focus();
    this.load(this.machine(), cx);
  }

  current() {
    return spaces().find((s) => s.id === this.space) ?? null;
  }

  machine() {
    return this.current()?.machine ?? null;
  }

  load(machine, cx) {
    const key = machine ?? "";
    if (this.lists[key]) return;
    this.lists[key] = { loading: true, templates: [] };
    cx.spawn(async (cx) => {
      this.lists[key] = await listTemplates(machine);
      cx.notify();
    });
  }

  close() {
    const p = props() ?? {};
    if (p.closeAlso) closeOverlay(p.closeAlso);
    closeOverlay(p.overlay);
  }

  start(cx) {
    const s = this.current();
    if (!s || !this.template || this.state) return;
    const machine = s.machine ?? null;
    const f = focused();
    const cwd = s.folder || (f?.space === s.id ? f.cwd : null) || null;
    this.state = "starting";
    this.error = null;
    cx.spawn(async (cx) => {
      const r = await runTemplate({ template: this.template, task: this.task.value(), machine, space: s.id, tab: this.tab || null, cwd, name: this.name.value().trim() || null });
      if (!r.ok) {
        this.state = null;
        this.error = r.message ? `${t(`errors.${r.code ?? r.kind}`)} ${r.message}` : t(`errors.${r.code ?? r.kind}`);
        cx.notify();
        return;
      }
      if (!r.sent) notify({ title: t("not_sent"), body: r.reason ? String(r.reason) : t("not_sent_about"), level: "warning" });
      // The app shows it once it reads the labels: wait for its pane (a few seconds at most).
      const addr = machine && !String(r.address).includes("/") ? `${machine}/${r.address}` : String(r.address);
      this.state = "placing";
      cx.notify();
      for (let i = 0; i < 25; i++) {
        if (panes().some((p) => p.terminal === addr)) {
          run("pane.focus", { terminal: addr });
          break;
        }
        await cx.sleep(200);
      }
      this.close();
    });
    cx.notify();
  }

  section(th, label, child) {
    return v_flex().gap(6).child(div().text_size(12).text_color(th.muted).child(label)).child(child);
  }

  spacesList(th) {
    return v_flex()
      .max_h(150)
      .id("ln-spaces")
      .overflow_y_scrollbar()
      .gap(1)
      .children(
        spaces().map((s) =>
          item(th, `ln-s-${s.id}`, s.id === this.space, (_e, cx) => {
            if (s.id === this.space) return;
            this.space = s.id;
            this.tab = s.tabs.find((tb) => tb.active)?.id ?? NEW_TAB;
            if (this.template && !this.templatesHere().some((e) => e.name === this.template)) this.template = null;
            this.load(s.machine ?? null, cx);
            cx.notify();
          }, h_flex().flex_1().gap(8).items_center()
            .child(div().flex_1().min_w(0).text_size(13).text_color(th.foreground).child(s.name))
            .child(note(th, s.machine ?? t("local")))),
        ),
      );
  }

  tabsRow(th) {
    const s = this.current();
    const chip = (id, label, on, pick) =>
      div()
        .id(`ln-t-${id || "new"}`)
        .px(10)
        .py(3)
        .rounded(7)
        .border_1()
        .border_color(on ? th.accent : th.border)
        .bg(on ? th.raised : th.background)
        .text_size(12.5)
        .text_color(on ? th.foreground : th.muted)
        .cursor_pointer()
        .on_click(pick)
        .child(label);
    return h_flex()
      .flex_wrap()
      .gap(6)
      .children((s?.tabs ?? []).map((tb) => chip(tb.id, tb.name, tb.id === this.tab, (_e, cx) => { this.tab = tb.id; cx.notify(); })))
      .child(chip(NEW_TAB, t("new_tab"), this.tab === NEW_TAB, (_e, cx) => { this.tab = NEW_TAB; cx.notify(); }));
  }

  templatesHere() {
    return this.lists[this.machine() ?? ""]?.templates ?? [];
  }

  templatesList(th) {
    const l = this.lists[this.machine() ?? ""];
    if (!l || l.loading) return h_flex().gap(8).items_center().child(spinner(th)).child(note(th, t("reading")));
    if (l.error) return note(th, `${t("read_failed")} ${l.error}`, "error");
    if (!l.templates.length) return note(th, t("none"));
    return v_flex()
      .max_h(170)
      .id("ln-templates")
      .overflow_y_scrollbar()
      .gap(1)
      .children(
        l.templates.map((e) => {
          const broken = Boolean(e.error);
          return item(th, `ln-tp-${e.name}`, e.name === this.template, (_e, cx) => {
            if (broken) return;
            this.template = e.name;
            cx.notify();
          }, h_flex().flex_1().gap(8).items_center()
            .child(div().text_size(13).font_semibold().text_color(broken ? th.dim : th.foreground).child(e.name))
            .child(div().flex_1().min_w(0).text_size(12).text_color(th.dim).child(e.fields.description ?? ""))
            .when(broken, (el) => el.child(badge(th, t("broken"), th.ansi1))));
        }),
      );
  }

  render(cx) {
    const th = theme();
    const busy = Boolean(this.state);
    const ready = Boolean(this.current() && this.template);
    // As tall as it's designed, shrinking with the window (a dialog stops at
    // 85% of it): the fields scroll, the buttons stay.
    return v_flex()
      .id("launch")
      .track_focus(this.handle)
      .on_key_down((e, cx) => {
        if (e.key === "escape") {
          cx.stop_propagation();
          closeOverlay(props().overlay);
        } else if (e.key === "enter" && (e.modifiers?.platform || e.modifiers?.control)) {
          cx.stop_propagation();
          this.start(cx);
        }
      })
      .w_full()
      .h(600)
      .min_h(0)
      .bg(th.popover)
      .child(div().px(20).pt(18).child(title(th, t("title"), t("about"))))
      .child(
        div()
          .relative()
          .flex_1()
          .min_h(0)
          .child(
            div()
              .id("ln-body")
              .size_full()
              .overflow_y_scroll()
              .child(
                v_flex()
                  .px(20)
                  .pb(8)
                  .gap(14)
                  .child(this.section(th, t("template"), this.templatesList(th)))
                  .child(this.section(th, t("space"), this.spacesList(th)))
                  .child(this.section(th, t("tab"), this.tabsRow(th)))
                  .child(this.section(th, t("task"), Textarea.new(this.task).w_full().px(10).py(8).rounded(7).border_1().border_color(th.input).bg(th.background).text_size(13).text_color(th.foreground)))
                  .child(field(th, t("name"), input(th, this.name, this.nameFocused), t("name_hint"))),
              ),
          )
          .child(Scrollbar.vertical("ln-body").absolute().inset_0()),
      )
      .child(
        v_flex()
          .px(20)
          .py(14)
          .gap(8)
          .border_t(1)
          .border_color(th.border)
          .when(Boolean(this.error), (el) => el.child(note(th, this.error, "error")))
          .child(
            h_flex()
              .gap(8)
              .items_center()
              .when(busy, (el) => el.child(spinner(th)).child(note(th, t(this.state))))
              .child(div().flex_1())
              .child(button(th, "ln-cancel", t("cancel"), () => closeOverlay(props().overlay)))
              .child(button(th, "ln-start", t("start"), (_e, cx) => this.start(cx), "primary", !ready || busy)),
          ),
      );
  }
}
