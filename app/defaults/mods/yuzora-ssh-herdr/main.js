// HERDR Session: makes an SSH host a herdr machine (`hc machine add`), or,
// when it already is one, opens a Space there. Props: { host }. Herdr's own
// machine-add dialog is offered as is (not changed). Owned by lane L4.
import { View, div } from "gpui-kit";
import { v_flex, h_flex } from "gpui-base";
import { theme, props, closeOverlay, run, spaces } from "herdr";
import { t } from "yuzora:core";
import { hosts } from "yuzora:ssh";

const MONO = "Menlo";

export default class SshHerdr extends View {
  init(_props, cx) {
    this.handle = cx.focus_handle();
    this.handle.focus();
  }

  dismiss() {
    closeOverlay(props().overlay);
  }

  // On this computer: a local space's new tab (the focused space may be another machine's).
  runHere(argv) {
    const local = spaces().find((s) => !s.machine && s.active) ?? spaces().find((s) => !s.machine);
    if (local) run("tab.new", { space: local.id, argv });
    else run("space.new", { machine: "", argv });
  }

  button(th, id, label, primary, onClick) {
    return div().id(id).h(30).px(14).flex().items_center().rounded(6).text_size(12.5).font_medium().cursor_pointer()
      .when(primary, (el) => el.bg(th.solid).text_color(th.solid_foreground).hover((s) => s.opacity(0.88)))
      .when(!primary, (el) => el.border(1).border_color(th.input).hover((s) => s.bg(th.hover)))
      .on_click(onClick)
      .child(label);
  }

  render() {
    const th = theme();
    const h = hosts().find((x) => x.id === props()?.host);
    const name = h?.name ?? props()?.host ?? "";
    // An ssh config alias says everything ssh needs; anything else goes as user@host:port.
    const target = h?.source === "config" ? h.name : h?.target ?? name;
    const root = v_flex().id("ssh-herdr").track_focus(this.handle)
      .on_key_down((e, cx) => { if (e.key === "escape") { this.dismiss(); cx.stop_propagation(); } })
      .w_full().p(18).gap(12).bg(th.popover).text_color(th.foreground)
      .child(div().text_size(15).font_semibold().child(t("title", { host: name })))
      .child(div().text_size(12.5).line_height("19px").text_color(th.muted).child(t("body", { host: name })));
    if (h?.machine) {
      return root
        .child(div().text_size(12.5).child(t("already", { host: name, machine: h.machine, state: h.machineState ?? "?" })))
        .child(h_flex().gap(8).justify_end()
          .child(this.button(th, "herdr-close", t("close"), false, () => this.dismiss()))
          .child(this.button(th, "herdr-space", t("newSpace", { machine: h.machine }), true, () => { run("space.new", { machine: h.machine }); this.dismiss(); })));
    }
    return root
      .child(div().text_size(12.5).text_color(th.muted).child(t("command")))
      .child(div().px(10).py(7).rounded(6).bg(th.background).border(1).border_color(th.border).text_size(12.5).font_family(MONO).child(`hc machine add ${target}`))
      .child(h_flex().gap(8).justify_end()
        .child(this.button(th, "herdr-dialog", t("dialog"), false, () => { run("mod.open", { name: "machine-add" }); this.dismiss(); }))
        .child(this.button(th, "herdr-run", t("run"), true, () => { this.runHere(["hc", "machine", "add", target]); this.dismiss(); })));
  }
}
