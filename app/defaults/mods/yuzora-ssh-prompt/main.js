// One question an SSH connection is waiting on: trust a new or changed host
// key, a password, a key's passphrase, or keyboard-interactive answers.
// Opened by the SSH sidebar or the SFTP pane with props { id } (a prompt id
// from yuzora:ssh prompts()); closing it without an answer cancels. Owned by lane L4.
import { View, div, svg } from "gpui-kit";
import { v_flex, h_flex, InputState, Input } from "gpui-base";
import { theme, props, closeOverlay } from "herdr";
import { t } from "yuzora:core";
import { prompts, answerPrompt } from "yuzora:ssh";

const I = (name) => `mods/yuzora-sidebar-ssh/icons/${name}.svg`;
const MONO = "Menlo";

export default class SshPrompt extends View {
  init(_props, cx) {
    this.prompt = this.current();
    this.answered = false;
    this.remember = false;
    this.handle = cx.focus_handle();
    const p = this.prompt;
    const questions = p?.kind === "keyboard" ? p.questions : p && ["password", "passphrase"].includes(p.kind) ? [{ text: "", echo: false }] : [];
    this.inputs = questions.map((q) => {
      const s = InputState.new({ placeholder: q.text.trim() });
      s.set_masked(!q.echo);
      s.on("submit", (_e, cx) => this.submit(cx));
      return s;
    });
    if (this.inputs[0]) {
      this.inputs[0].focus();
      cx.timer.after(200, () => this.inputs[0].focus());
    } else this.handle.focus();
    // Answered elsewhere or expired: nothing left to ask.
    cx.timer.every(1000, () => {
      if (!this.answered && !this.current()) this.dismiss();
    });
  }

  current() {
    return prompts().find((p) => p.id === props()?.id) ?? null;
  }

  dismiss() {
    closeOverlay(props().overlay);
  }

  // Called once when the dialog goes (Esc, a click on Cancel, the app quitting).
  async close() {
    if (!this.answered && this.prompt) answerPrompt(this.prompt.id, null);
    this.answered = true;
  }

  reply(answer) {
    if (this.answered || !this.prompt) return;
    this.answered = true;
    answerPrompt(this.prompt.id, answer);
    closeOverlay(props().overlay);
  }

  submit(cx) {
    const p = this.prompt;
    if (!p) return;
    if (p.kind === "keyboard") this.reply({ responses: this.inputs.map((s) => s.value()), remember: this.remember });
    else if (this.inputs[0]) this.reply({ secret: this.inputs[0].value(), remember: this.remember });
    cx.notify();
  }

  button(th, id, label, primary, danger, onClick) {
    return div()
      .id(id)
      .h(30)
      .px(14)
      .flex()
      .items_center()
      .rounded(6)
      .text_size(12.5)
      .font_medium()
      .cursor_pointer()
      .when(primary, (el) => el.bg(th.solid).text_color(th.solid_foreground).hover((s) => s.opacity(0.88)))
      .when(!primary, (el) => el.border(1).border_color(danger ? th.blocked : th.input).text_color(danger ? th.blocked : th.foreground).hover((s) => s.bg(th.hover)))
      .on_click(onClick)
      .child(label);
  }

  fingerprint(th, label, kind, value, strong) {
    return v_flex()
      .gap(3)
      .child(div().text_size(11).text_color(th.dim).child(label))
      .child(
        h_flex()
          .gap(8)
          .px(10)
          .py(7)
          .rounded(6)
          .bg(th.background)
          .border(1)
          .border_color(strong ? th.blocked : th.border)
          .items_center()
          .child(div().flex_none().text_size(11).font_semibold().text_color(th.muted).child(kind ?? ""))
          .child(div().flex_1().min_w(0).text_size(12).font_family(MONO).text_color(th.foreground).child(value ?? "")),
      );
  }

  check(th) {
    return h_flex()
      .id("ssh-remember")
      .gap(8)
      .items_center()
      .cursor_pointer()
      .on_click((_e, cx) => {
        this.remember = !this.remember;
        cx.notify();
      })
      .child(
        div()
          .w(14)
          .h(14)
          .rounded(3)
          .border(1)
          .border_color(this.remember ? th.accent : th.input)
          .bg(this.remember ? th.accent : "#00000000")
          .flex()
          .items_center()
          .justify_center()
          .text_size(10)
          .text_color(th.accent_foreground)
          .child(this.remember ? "✓" : ""),
      )
      .child(div().text_size(12).text_color(th.muted).child(t("remember")));
  }

  field(th, s, label) {
    return v_flex()
      .gap(4)
      .when(Boolean(label), (el) => el.child(div().text_size(12).text_color(th.muted).child(label)))
      .child(Input.new(s).h(32).px(10).items_center().rounded(6).border(1).border_color(th.input).bg(th.background).text_size(13).text_color(th.foreground));
  }

  body(th, p) {
    const kind = p.algorithm ? p.algorithm.replace(/^ssh-/, "").replace(/^ecdsa-sha2-.*/, "ECDSA").toUpperCase() : "";
    switch (p.kind) {
      case "hostKey":
        return v_flex()
          .gap(12)
          .child(div().text_size(12.5).line_height("19px").text_color(th.muted).child(t("hostKey.body", { endpoint: p.endpoint ?? p.label })))
          .child(this.fingerprint(th, p.endpoint, kind, p.fingerprint, false));
      case "hostKeyChanged":
        return v_flex()
          .gap(12)
          .child(div().text_size(12.5).line_height("19px").text_color(th.muted).child(t("changed.body")))
          .child(this.fingerprint(th, t("changed.now"), kind, p.fingerprint, true))
          .children(p.previous.map((f) => this.fingerprint(th, `${t("changed.before")} · ${p.previousSource ?? ""}`, kind, f, false)));
      case "passphrase":
        return v_flex()
          .gap(10)
          .child(div().text_size(12.5).text_color(th.muted).child(t("passphrase.body", { path: p.keyPath ?? "" })))
          .when(p.retry, (el) => el.child(div().text_size(12).text_color(th.blocked).child(t("passphrase.retry"))))
          .child(this.field(th, this.inputs[0]))
          .child(this.check(th));
      case "password":
        return v_flex()
          .gap(10)
          .when(p.retry, (el) => el.child(div().text_size(12).text_color(th.blocked).child(t("password.retry"))))
          .child(this.field(th, this.inputs[0]))
          .child(this.check(th));
      default:
        return v_flex()
          .gap(10)
          .when(Boolean(p.instructions), (el) => el.child(div().text_size(12.5).text_color(th.muted).child(p.instructions)))
          .children(p.questions.map((q, i) => this.field(th, this.inputs[i], q.text.trim())))
          .when(p.canRemember, (el) => el.child(this.check(th)));
    }
  }

  title(p) {
    switch (p.kind) {
      case "hostKey": return t("hostKey.title", { host: p.label });
      case "hostKeyChanged": return t("changed.title", { host: p.label });
      case "password": return t("password.title", { host: p.label });
      case "passphrase": return t("passphrase.title");
      default: return p.name || t("keyboard.title", { host: p.label });
    }
  }

  render() {
    const th = theme();
    const p = this.prompt;
    const root = v_flex()
      .id("ssh-prompt")
      .track_focus(this.handle)
      .on_key_down((e, cx) => {
        if (e.key === "escape") {
          this.dismiss();
          cx.stop_propagation();
        }
      })
      .w_full()
      .p(18)
      .gap(14)
      .bg(th.popover)
      .text_color(th.foreground);
    if (!p) return root.child(div().text_size(12.5).text_color(th.muted).child(t("gone")));
    const changed = p.kind === "hostKeyChanged";
    const trust = p.kind === "hostKey" || changed;
    return root
      .child(
        h_flex()
          .gap(8)
          .items_center()
          .child(svg(I(changed ? "alert" : trust ? "shield" : "key")).size(16).text_color(changed ? th.blocked : th.muted))
          .child(div().flex_1().text_size(15).font_semibold().text_color(changed ? th.blocked : th.foreground).child(this.title(p))),
      )
      .child(this.body(th, p))
      .child(
        h_flex()
          .pt(4)
          .gap(8)
          .justify_end()
          .when(changed, (el) =>
            el.child(this.button(th, "ssh-trust", t("changed.trust"), false, true, () => this.reply({ trust: true }))).child(this.button(th, "ssh-cancel", t("cancel"), true, false, () => this.reply(null))),
          )
          .when(!changed, (el) =>
            el
              .child(this.button(th, "ssh-cancel", t("cancel"), false, false, () => this.reply(trust ? { trust: false } : null)))
              .child(this.button(th, "ssh-ok", trust ? t("hostKey.trust") : t("submit"), true, false, (_e, cx) => (trust ? this.reply({ trust: true }) : this.submit(cx)))),
          ),
      );
  }
}
