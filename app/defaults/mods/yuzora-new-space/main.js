// A new Space in a folder someone picks (beta.1's "open a folder as a Space"). Opened with
// props { machine } (null: this computer) by the workspace sidebar, and for a bare space.new
// from the person (keys, the palette, menus: app/src/yuzora/shell.rs). Here it's the system's
// folder picker; another machine gets a path, checked there. A Space already rooted in that
// folder is shown instead of a second one, and one Space is made per machine at a time.
// Owned by lane L1.
import { View, div } from "gpui-kit";
import { v_flex, h_flex, InputState, Input } from "gpui-base";
import { theme, props, closeOverlay, run, spaces, machines, pickFolder, notify } from "herdr";
import { t } from "yuzora:core";
import { planSpace, beginSpace, endSpace } from "yuzora:shell";

export default class NewSpace extends View {
  init(_props, cx) {
    this.machine = props()?.machine || null;
    this.handle = cx.focus_handle();
    this.state = "picking"; // picking | input | checking | error
    this.error = null;
    this.held = beginSpace(this.machine);
    if (!this.held) {
      notify({ title: this.say("busy"), level: "info", timeout: 4000 });
      closeOverlay(props().overlay);
      return;
    }
    if (this.machine) {
      this.state = "input";
      this.input = InputState.new({ value: "~/", placeholder: t("path.placeholder") });
      this.input.on("submit", (_e, cx) => this.submit(cx));
      this.input.focus();
      cx.timer.after(200, () => this.input.focus());
    } else {
      this.handle.focus();
      this.pick(cx);
    }
  }

  // Esc, the close button or done: the machine can take another new Space.
  async close() {
    this.release();
  }

  release() {
    if (this.held) endSpace(this.machine);
    this.held = false;
  }

  dismiss() {
    this.release();
    closeOverlay(props().overlay);
  }

  // This computer has its own sentences (no machine name to put in).
  say(key, vars = {}) {
    const here = !this.machine && ["busy", "error.not_a_folder"].includes(key);
    return t(here ? `${key}_here` : key, { machine: this.machine ?? "", ...vars });
  }

  pick(cx) {
    this.state = "picking";
    this.error = null;
    cx.notify();
    cx.spawn(async (cx) => {
      let folder;
      try {
        folder = await pickFolder({ prompt: t("pick.prompt") });
      } catch (e) {
        return this.fail("pick_failed", String(e?.message ?? e), cx);
      }
      if (!folder) return this.dismiss();
      await this.create(folder, cx);
    });
  }

  submit(cx) {
    const folder = this.input.value().trim();
    if (!folder || this.state === "checking") return;
    cx.spawn(async (cx) => this.create(folder, cx));
  }

  async create(folder, cx) {
    this.state = "checking";
    this.error = null;
    cx.notify();
    const here = spaces().filter((s) => (s.machine ?? null) === this.machine).map((s) => ({ id: s.id, folder: s.folder }));
    let plan;
    try {
      plan = await planSpace(folder, here, { on: this.machine });
    } catch (e) {
      plan = { error: "unreachable", detail: String(e?.message ?? e) };
    }
    if (!this.held) return; // closed meanwhile
    if (plan.error) return this.fail(plan.error, plan.detail, cx);
    // The machine left while its folder was chosen: don't start a Space nobody can reach.
    if (this.machine && machines().find((m) => m.name === this.machine)?.layout !== "ready") return this.fail("machine_gone", null, cx);
    if (plan.existing) {
      run("space.select", { id: plan.existing });
      notify({ title: t("existing", { name: spaces().find((s) => s.id === plan.existing)?.name ?? plan.name }), level: "info", timeout: 4000 });
    } else {
      run("space.new", { name: plan.name, folder: plan.folder, machine: this.machine ?? "" });
    }
    this.dismiss();
  }

  fail(code, detail, cx) {
    this.state = "error";
    this.error = this.say(`error.${code}`, { detail: detail || "?" });
    cx.notify();
    if (this.input) this.input.focus();
  }

  key(e, cx) {
    if (e.key === "escape") {
      this.dismiss();
      cx.stop_propagation();
    }
  }

  render() {
    const th = theme();
    if (!this.held) return div();
    const checking = this.state === "checking";
    const button = (id, text, primary, action, enabled = true) => h_flex().id(id).h(30).px(14).items_center().rounded(7)
      .border(1).border_color(primary ? th.accent : th.input).bg(primary ? th.accent : th.raised)
      .text_color(primary ? th.accent_foreground : th.foreground).text_size(12.5).font_medium()
      .when(enabled, (el) => el.cursor_pointer().hover((s) => s.opacity(0.88)).on_click(action))
      .when(!enabled, (el) => el.opacity(0.5))
      .child(text);
    const note = (text, color) => div().text_size(12).text_color(color).child(text);
    const body = this.machine
      ? v_flex().gap(6)
        .child(div().text_size(11.5).text_color(th.dim).child(t("path.label", { machine: this.machine })))
        .child(Input.new(this.input).h(34).px(10).rounded(8).border(1).border_color(th.input).bg(th.background)
          .text_size(12.8).text_color(th.foreground))
        .child(div().text_size(11.5).text_color(th.dim).child(t("path.hint", { machine: this.machine })))
      : note(t(this.state === "error" ? "pick.again" : "pick.waiting"), th.muted);
    return v_flex().id("new-space").track_focus(this.handle).on_key_down((e, cx) => this.key(e, cx))
      .w_full().p(18).gap(12).bg(th.popover).text_color(th.foreground)
      .child(h_flex().items_center()
        .child(div().flex_1().text_size(15).font_semibold().child(this.machine ? t("title_on", { machine: this.machine }) : t("title")))
        .child(div().text_size(11).text_color(th.dim).child("Esc")))
      .child(body)
      .when(checking, (el) => el.child(note(t("checking"), th.muted)))
      .when(Boolean(this.error), (el) => el.child(note(this.error, th.ansi1)))
      .child(h_flex().gap(8).justify_end()
        .child(button("ns-cancel", t("cancel"), false, () => this.dismiss()))
        .when(Boolean(this.machine), (el) => el.child(button("ns-create", t("create"), true, (_e, cx) => this.submit(cx), !checking)))
        .when(!this.machine && this.state === "error", (el) => el.child(button("ns-again", t("pick.choose_again"), true, (_e, cx) => this.pick(cx)))));
  }
}
