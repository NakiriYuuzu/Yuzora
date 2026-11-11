// Adds, edits or deletes one of Yuzora's own SSH hosts (settings "ssh"
// profiles; the password goes to the system keychain). Props: { id? } to
// edit, { id, remove: true } to delete. Owned by lane L4.
import { View, div } from "gpui-kit";
import { v_flex, h_flex, InputState, Input } from "gpui-base";
import { theme, props, closeOverlay, shared, setShared } from "herdr";
import { t } from "yuzora:core";
import { hosts, saveProfile, removeProfile } from "yuzora:ssh";

// Errors read "`yuzora:ssh.<fn>`: [code] text"; people see the text.
const message = (e) => String(e?.message ?? e).replace(/^(Error: )?(`[^`]*`: )?\[[a-z-]+\] /, "");
const SELECTED = "yuzora.ssh.selected";
const FIELDS = [
  ["name", "namePlaceholder"],
  ["host", "hostPlaceholder"],
  ["port", null],
  ["user", null],
  ["identityFile", "identityPlaceholder"],
  ["proxyJump", "jumpPlaceholder"],
  ["password", null],
];

export default class SshHost extends View {
  init(_props, cx) {
    const id = props()?.id;
    this.profile = id ? hosts().find((h) => h.profile?.id === id)?.profile ?? null : null;
    this.handle = cx.focus_handle();
    this.inputs = {};
    for (const [key, ph] of FIELDS) {
      const value = key === "port" ? String(this.profile?.port ?? (this.profile ? "" : "22")) : key === "password" ? "" : this.profile?.[key] ?? "";
      const s = InputState.new({ placeholder: ph ? t(ph) : "", value });
      if (key === "password") s.set_masked(true);
      s.on("submit", (_e, cx) => this.save(cx));
      this.inputs[key] = s;
    }
    if (!props()?.remove) {
      this.inputs[this.profile ? "name" : "host"].focus();
      cx.timer.after(200, () => this.inputs[this.profile ? "name" : "host"].focus());
    } else this.handle.focus();
  }

  dismiss() {
    closeOverlay(props().overlay);
  }

  save(cx) {
    const v = (k) => this.inputs[k].value().trim();
    const port = v("port") ? Number(v("port")) : null;
    try {
      const id = saveProfile(
        { id: this.profile?.id ?? "", name: v("name"), host: v("host"), port: Number.isInteger(port) && port > 0 ? port : null, user: v("user") || null, identityFile: v("identityFile") || null, proxyJump: v("proxyJump") || null },
        this.inputs.password.value() || null,
      );
      // A host that now reaches another server comes back under a new id: the sidebar's
      // selection (its key in shared) follows it there.
      const was = this.profile ? `profile:${this.profile.id}` : null;
      if (was && was !== `profile:${id}` && (shared(SELECTED) ?? was) === was) setShared(SELECTED, `profile:${id}`);
      this.dismiss();
    } catch (e) {
      this.error = t("failed", { why: message(e) });
      cx.notify();
    }
  }

  button(th, id, label, primary, onClick, danger) {
    return div().id(id).h(30).px(14).flex().items_center().rounded(6).text_size(12.5).font_medium().cursor_pointer()
      .when(primary && !danger, (el) => el.bg(th.solid).text_color(th.solid_foreground).hover((s) => s.opacity(0.88)))
      .when(Boolean(danger), (el) => el.border(1).border_color(th.blocked).text_color(th.blocked).hover((s) => s.bg(th.hover)))
      .when(!primary && !danger, (el) => el.border(1).border_color(th.input).hover((s) => s.bg(th.hover)))
      .on_click(onClick)
      .child(label);
  }

  field(th, key, label, width) {
    return v_flex().gap(4).when(Boolean(width), (el) => el.w(width).flex_none()).when(!width, (el) => el.flex_1().min_w(0))
      .child(div().text_size(12).text_color(th.muted).child(label))
      .child(Input.new(this.inputs[key]).h(30).px(10).items_center().rounded(6).border(1).border_color(th.input).bg(th.background).text_size(12.5).text_color(th.foreground));
  }

  render() {
    const th = theme();
    const root = v_flex().id("ssh-host").track_focus(this.handle)
      .on_key_down((e, cx) => { if (e.key === "escape") { this.dismiss(); cx.stop_propagation(); } })
      .w_full().p(18).gap(12).bg(th.popover).text_color(th.foreground);
    const name = this.profile?.name ?? "";
    if (props()?.remove) {
      return root
        .child(div().text_size(15).font_semibold().child(t("remove", { name })))
        .child(div().text_size(12.5).text_color(th.muted).child(t("removeBody")))
        .child(h_flex().gap(8).justify_end()
          .child(this.button(th, "ssh-host-cancel", t("cancel"), false, () => this.dismiss()))
          .child(this.button(th, "ssh-host-delete", t("delete"), true, () => { removeProfile(this.profile?.id ?? props().id); this.dismiss(); }, true)));
    }
    return root
      .child(div().text_size(15).font_semibold().child(this.profile ? t("edit", { name }) : t("add")))
      .child(div().text_size(11.5).text_color(th.dim).child(t("hint")))
      .child(this.field(th, "name", t("name")))
      .child(h_flex().gap(8).child(this.field(th, "host", t("host"))).child(this.field(th, "port", t("port"), 72)))
      .child(this.field(th, "user", t("user")))
      .child(this.field(th, "identityFile", t("identity")))
      .child(this.field(th, "proxyJump", t("jump")))
      .child(this.field(th, "password", t("password")))
      .when(Boolean(this.error), (el) => el.child(div().text_size(12).text_color(th.blocked).child(this.error)))
      .child(h_flex().pt(4).gap(8).justify_end()
        .child(this.button(th, "ssh-host-cancel", t("cancel"), false, () => this.dismiss()))
        .child(this.button(th, "ssh-host-save", t("save"), true, (_e, cx) => this.save(cx))));
  }
}
