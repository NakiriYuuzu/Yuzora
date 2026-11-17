// Save, Don't Save or Cancel, before closing a pane, tab or Space, closing the window, quitting
// or restarting into an update loses unsaved changes. The app opens it in place of the command
// (app/src/yuzora/unsaved.rs) with props { docs: [{doc, name, path, on}], command: {name, args} }.
// Save and Don't Save leave the documents clean, then run the command again: it finds nothing
// unsaved this time and goes ahead. Owned by lane L5.
import { View, div } from "gpui-kit";
import { v_flex, h_flex } from "gpui-base";
import { theme, props, closeOverlay, run } from "herdr";
import { t } from "yuzora:core";
import { saveDocument, discardDocument, documentState } from "yuzora:files";

export default class Unsaved extends View {
  init(_props, cx) {
    const p = props() ?? {};
    this.docs = p.docs ?? [];
    this.command = p.command ?? null;
    this.errors = {};
    this.saving = false;
    this.handle = cx.focus_handle();
    this.handle.focus();
  }

  // Closed before the command runs, as the confirm dialog does: the command may end this copy.
  finish() {
    closeOverlay(props().overlay);
    if (this.command) run(this.command.name, this.command.args ?? {});
  }

  // A deleted file is written back where it was (the editor's "Save it again"); one that changed
  // on disk is not overwritten from here. Any failure keeps the dialog, and the command waits.
  save(cx) {
    if (this.saving) return;
    this.saving = true;
    this.errors = {};
    cx.notify();
    cx.spawn(async (cx) => {
      const results = await Promise.all(this.docs.map(async (d) => {
        const state = documentState(d.doc);
        if (!state?.dirty) return [d.doc, null];
        try {
          const r = await saveDocument(d.doc, { force: state.deleted });
          return [d.doc, r.ok ? null : r.conflict ? t("error.conflict") : t("error.save", { error: r.error ?? "" })];
        } catch (e) {
          return [d.doc, t("error.save", { error: String(e?.message ?? e) })];
        }
      }));
      this.saving = false;
      this.errors = Object.fromEntries(results.filter(([, why]) => why));
      if (this.docs.every((d) => !documentState(d.doc)?.dirty)) return this.finish();
      cx.notify();
    });
  }

  discard() {
    if (this.saving) return;
    for (const d of this.docs) discardDocument(d.doc);
    this.finish();
  }

  cancel() {
    closeOverlay(props().overlay);
  }

  key(e, cx) {
    if (e.key === "escape") this.cancel();
    else if (e.key === "enter") this.save(cx);
    else return;
    cx.stop_propagation();
  }

  render() {
    const th = theme();
    const one = this.docs.length === 1;
    const button = (id, text, primary, action) => h_flex().id(id).h(30).px(14).items_center().rounded(7)
      .border(1).border_color(primary ? th.accent : th.input).bg(primary ? th.accent : th.raised)
      .text_color(primary ? th.accent_foreground : th.foreground).text_size(12.5).font_medium()
      .when(!this.saving, (el) => el.cursor_pointer().hover((s) => s.opacity(0.88)).on_click(action))
      .when(this.saving, (el) => el.opacity(0.5))
      .child(text);
    const where = (d) => {
      const dir = d.path.slice(0, d.path.length - d.name.length).replace(/[\\/]$/, "");
      return d.on ? `${dir} · ${t("on", { machine: d.on })}` : dir;
    };
    const row = (d) => v_flex().gap(2).min_w(0)
      .child(h_flex().gap(6).min_w(0)
        .child(div().flex_none().font_medium().text_color(th.foreground).child(d.name))
        .child(div().flex_1().min_w(0).truncate().text_color(th.dim).child(where(d))))
      .when(Boolean(this.errors[d.doc]), (el) => el.child(div().text_size(11.5).text_color(th.ansi1).child(this.errors[d.doc])));
    return v_flex().id("unsaved").track_focus(this.handle).on_key_down((e, cx) => this.key(e, cx))
      .w_full().p(18).gap(12).bg(th.popover).text_color(th.foreground).text_size(12.5)
      .child(h_flex().items_center()
        .child(div().flex_1().text_size(15).font_semibold().child(one ? t("title_one", { name: this.docs[0].name }) : t("title_many", { count: this.docs.length })))
        .child(div().text_size(11).text_color(th.dim).child("Esc")))
      .child(div().text_color(th.muted).child(t("body")))
      .child(v_flex().id("unsaved-files").gap(8).max_h(220).overflow_y_scrollbar().children(this.docs.map(row)))
      .child(h_flex().gap(8).items_center()
        .child(button("us-dont-save", t("dont_save"), false, () => this.discard()))
        .child(div().flex_1())
        .child(button("us-cancel", t("cancel"), false, () => this.cancel()))
        .child(button("us-save", this.saving ? t("saving") : t("save"), true, (_e, cx) => this.save(cx))));
  }
}
