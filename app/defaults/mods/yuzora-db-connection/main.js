// Add or edit a database connection (a dialog). Owned by lane L3.
// props: {id: string | null, overlay}. The password goes to the system's
// credential store; everything else to Yuzora settings' `db` section.
import { View, div } from "gpui-kit";
import { v_flex, h_flex, InputState, Input } from "gpui-base";
import { props, closeOverlay, theme } from "herdr";
import { t } from "yuzora:core";
import { connections, engines, saveConnection, deleteConnection, testConnection } from "yuzora:db";

const SSL_MODES = ["disable", "prefer", "require", "verify-ca", "verify-full"]; // weakest to strictest
const TEXT_FIELDS = ["name", "host", "port", "user", "database", "instance", "path", "caFile"];

export default class ConnectionDialog extends View {
  init(_props, cx) {
    const id = props()?.id ?? null;
    const existing = id ? connections().find((c) => c.id === id) : null;
    this.engines = engines();
    this.form = existing
      ? { ...existing }
      : { id: "", name: "", engine: "postgres", host: "127.0.0.1", port: null, user: "", database: "", ssl: "prefer", caFile: "", instance: "", path: "", readOnly: false, savePassword: true, ssh: null };
    this.editing = Boolean(existing);
    this.original = existing;
    this.inputs = {};
    for (const key of TEXT_FIELDS) {
      const value = this.form[key] === null || this.form[key] === undefined ? "" : String(this.form[key]);
      const input = InputState.new({ value, placeholder: this.placeholder(key) });
      input.on("change", (_e, cx) => { this.form[key] = input.value(); this.test = null; cx.notify(); });
      input.on("submit", (_e, cx) => this.save(cx));
      this.inputs[key] = input;
    }
    this.password = InputState.new({ placeholder: this.editing ? t("field.password_keep") : "" });
    this.password.set_masked(true);
    this.password.on("change", (_e, cx) => { this.passwordTouched = true; this.test = null; cx.notify(); });
    this.password.on("submit", (_e, cx) => this.save(cx));
    this.inputs.name.focus();
    this.handle = cx.focus_handle();
    this.test = null; // {ok, text}
    this.busy = null; // "test" | "save" | "delete"
    this.error = null;
    this.confirmDelete = false;
  }

  placeholder(key) {
    if (key === "port") return String(this.engines?.find((e) => e.id === this.form?.engine)?.defaultPort ?? "");
    if (key === "path") return "~/data/app.sqlite";
    if (key === "host") return "127.0.0.1";
    return "";
  }

  config() {
    const f = this.form;
    const port = String(f.port ?? "").trim();
    return { ...f, port: port === "" ? null : Number(port), name: String(f.name ?? "").trim() };
  }

  pwd() {
    const value = this.password.value();
    return this.passwordTouched || !this.editing ? value : null;
  }

  /** Editing pointed the connection at another server or login, or weakened its TLS: the saved password stays behind (config.rs `password_may_follow`). */
  moved() {
    const a = this.original;
    if (!a) return false;
    const b = this.config();
    const port = (c) => c.port ?? this.engines.find((e) => e.id === c.engine)?.defaultPort ?? null;
    const same = (x, y) => String(x ?? "").trim().toLowerCase() === String(y ?? "").trim().toLowerCase();
    const verifies = a.ssl === "verify-ca" || a.ssl === "verify-full";
    return a.engine !== b.engine || !same(a.host, b.host) || port(a) !== port(b) || (a.user ?? "") !== (b.user ?? "") || !same(a.instance, b.instance)
      || SSL_MODES.indexOf(b.ssl) < SSL_MODES.indexOf(a.ssl) || (verifies && String(a.caFile ?? "").trim() !== String(b.caFile ?? "").trim());
  }

  close() {
    closeOverlay(props().overlay);
  }

  save(cx) {
    if (this.busy) return;
    this.busy = "save";
    this.error = null;
    cx.notify();
    cx.spawn(async (cx) => {
      try {
        await saveConnection(this.config(), this.pwd());
        this.close();
      } catch (e) {
        this.error = String(e?.message ?? e);
      }
      this.busy = null;
      cx.notify();
    });
  }

  runTest(cx) {
    if (this.busy) return;
    this.busy = "test";
    this.test = null;
    cx.notify();
    cx.spawn(async (cx) => {
      try {
        const r = await testConnection(this.config(), this.pwd());
        this.test = { ok: true, text: t("test.ok", { version: r.version, ms: r.elapsedMs }) };
      } catch (e) {
        this.test = { ok: false, text: String(e?.message ?? e) };
      }
      this.busy = null;
      cx.notify();
    });
  }

  remove(cx) {
    if (!this.confirmDelete) {
      this.confirmDelete = true;
      cx.notify();
      return;
    }
    this.busy = "delete";
    cx.spawn(async (cx) => {
      try {
        await deleteConnection(this.form.id);
        this.close();
      } catch (e) {
        this.error = String(e?.message ?? e);
      }
      this.busy = null;
      cx.notify();
    });
  }

  setEngine(id, cx) {
    if (this.form.engine === id) return;
    this.form.engine = id;
    this.inputs.port.set_value("");
    this.form.port = null;
    if (id === "sqlite") this.form.ssl = "disable";
    else if (this.form.ssl === "disable") this.form.ssl = "prefer";
    this.test = null;
    cx.notify();
  }

  render() {
    const th = theme();
    const f = this.form;
    const network = f.engine !== "sqlite";
    const label = (text) => div().pt(12).pb(6).text_size(11.5).text_color(th.dim).child(text);
    const field = (key, extra) => Input.new(this.inputs[key]).h(32).px(10).rounded(7).border_1().border_color(th.input)
      .bg(th.background).text_size(12.5).text_color(th.foreground).when(Boolean(extra), extra);
    const segmented = (id, options, value, pick) => h_flex().id(id).p(2).gap(2).rounded(7).bg(th.raised)
      .children(options.map((o) => div().id(`${id}-${o.id}`).h(26).px(10).flex().flex_1().items_center().justify_center()
        .rounded(5).text_size(11.5).cursor_pointer()
        .bg(o.id === value ? th.selected : th.raised).text_color(o.id === value ? th.foreground : th.muted)
        .hover((s) => s.text_color(th.foreground))
        .on_click((_e, cx) => pick(o.id, cx)).child(o.label)));
    const toggle = (id, on, text, flip) => h_flex().id(id).gap(8).pt(10).items_center().cursor_pointer()
      .on_click((_e, cx) => { flip(); this.test = null; cx.notify(); })
      .child(div().size(14).flex_none().rounded(3).border_1().border_color(on ? th.solid : th.input).bg(on ? th.solid : th.background)
        .flex().items_center().justify_center().text_size(10).text_color(th.solid_foreground).child(on ? "✓" : ""))
      .child(div().text_size(12).text_color(th.muted).child(text));
    const button = (id, text, kind, action) => h_flex().id(id).h(32).px(14).items_center().rounded(8).text_size(12.5).font_medium()
      .cursor_pointer().hover((s) => s.opacity(0.88))
      .when(kind === "primary", (el) => el.bg(th.solid).text_color(th.solid_foreground))
      .when(kind === "plain", (el) => el.border_1().border_color(th.input).bg(th.raised).text_color(th.foreground))
      .when(kind === "danger", (el) => el.border_1().border_color(th.input).text_color(th.blocked))
      .on_click((_e, cx) => action(cx)).child(text);

    const body = v_flex().id("form").flex_1().min_h(0).overflow_y_scrollbar().px(18).pb(8)
      .child(label(t("field.name"))).child(field("name"))
      .child(label(t("field.engine")))
      .child(segmented("engine", this.engines.map((e) => ({ id: e.id, label: e.label })), f.engine, (id, cx) => this.setEngine(id, cx)))
      .when(network, (el) => el
        .child(h_flex().gap(8)
          .child(v_flex().flex_1().child(label(t("field.host"))).child(field("host")))
          .child(v_flex().w(96).child(label(t("field.port"))).child(field("port"))))
        .when(f.engine === "mssql", (el) => el.child(label(t("field.instance"))).child(field("instance")))
        .child(label(t("field.user"))).child(field("user"))
        .child(label(t("field.password")))
        .child(Input.new(this.password).h(32).px(10).rounded(7).border_1().border_color(th.input).bg(th.background).text_size(12.5).text_color(th.foreground))
        .when(this.editing && !this.passwordTouched && this.moved(), (el) => el.child(div().pt(6).text_size(11).text_color(th.blocked).child(t("field.password_retype"))))
        .child(toggle("save-password", f.savePassword, t("field.save_password"), () => { f.savePassword = !f.savePassword; }))
        .child(label(t("field.database"))).child(field("database"))
        .child(label(t("field.ssl")))
        .child(segmented("ssl", SSL_MODES.map((m) => ({ id: m, label: t(`ssl.${m}`) })), f.ssl, (id, cx) => { f.ssl = id; this.test = null; cx.notify(); }))
        .child(div().pt(6).text_size(11).text_color(th.dim).child(t(`ssl_hint.${f.ssl}`)))
        .when(f.ssl === "verify-ca" || f.ssl === "verify-full", (el) => el.child(label(t("field.ca_file"))).child(field("caFile")))
        .child(label(t("field.ssh")))
        .child(div().text_size(11.5).text_color(th.dim).child(t("ssh.pending"))))
      .when(!network, (el) => el
        .child(label(t("field.path"))).child(field("path"))
        .child(toggle("read-only", f.readOnly, t("field.read_only"), () => { f.readOnly = !f.readOnly; })));

    const result = this.test
      ? div().px(18).pt(8).text_size(11.5).text_color(this.test.ok ? th.working : th.blocked).child(this.test.text)
      : this.error
        ? div().px(18).pt(8).text_size(11.5).text_color(th.blocked).child(this.error)
        : null;

    return v_flex().id("dialog").track_focus(this.handle)
      .on_key_down((e, cx) => { if (e.key === "escape") { this.close(); cx.stop_propagation(); } })
      .size_full().bg(th.popover).text_color(th.foreground)
      .child(h_flex().px(18).pt(16).pb(4).items_center()
        .child(div().flex_1().text_size(15).font_semibold().child(t(this.editing ? "title.edit" : "title.add")))
        .child(div().text_size(11).text_color(th.dim).child("Esc")))
      .child(body)
      .when(Boolean(result), (el) => el.child(result))
      .child(h_flex().px(18).py(14).gap(8).items_center()
        .when(this.editing, (el) => el.child(button("delete", t(this.confirmDelete ? "button.confirm_delete" : "button.delete"), "danger", (cx) => this.remove(cx))))
        .child(div().flex_1())
        .child(button("test", t(this.busy === "test" ? "button.testing" : "button.test"), "plain", (cx) => this.runTest(cx)))
        .child(button("cancel", t("button.cancel"), "plain", () => this.close()))
        .child(button("save", t(this.busy === "save" ? "button.saving" : "button.save"), "primary", (cx) => this.save(cx))));
  }
}
