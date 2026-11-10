// Pieces every Yuzora settings page draws with, in the settings dialog's look
// (frame/settings): a scrolling page, headings, rows, buttons, notes.
// The same file ships in each yuzora-settings-* mod: a mod imports only its own folder.

import { div, svg } from "gpui-kit";
import { v_flex, h_flex, Scrollbar, Input } from "gpui-base";
import { motion } from "yuzora:core";

const CLEAR = "#00000000";

/** The page: padded, scrolling, with a bar that always shows (as the frame's). */
export function page(th, id, content) {
  return div()
    .relative()
    .size_full()
    .bg(th.popover)
    .child(div().id(id).size_full().overflow_y_scroll().child(div().px(28).py(22).child(content)))
    .child(Scrollbar.vertical(id).mode("always").absolute().inset_0());
}

export function title(th, text, about) {
  return v_flex()
    .pb(10)
    .gap(4)
    .child(div().text_size(17).font_semibold().text_color(th.foreground).child(text))
    .when(Boolean(about), (el) => el.child(div().text_size(12.5).line_height("18px").text_color(th.dim).child(about)));
}

export function heading(th, text) {
  return div().pt(14).pb(6).text_size(12).font_semibold().text_color(th.dim).child(text.toUpperCase());
}

/** One setting: what it is on the left, its control on the right. */
export function row(th, name, about, control) {
  return h_flex()
    .w_full()
    .gap(16)
    .py(11)
    .items_center()
    .border_b(1)
    .border_color(th.border)
    .child(
      v_flex()
        .flex_1()
        .min_w(0)
        .gap(3)
        .child(div().text_size(13.5).text_color(th.foreground).child(name))
        .when(Boolean(about), (el) => el.child(div().text_size(12).line_height("17px").text_color(th.dim).child(about))),
    )
    .child(div().flex_none().child(control ?? div()));
}

/** kind: "plain" (default), "primary", "danger"; disabled draws it dim and ignores clicks. */
export function button(th, id, label, onClick, kind = "plain", disabled = false) {
  const primary = kind === "primary";
  const danger = kind === "danger";
  return div()
    .id(id)
    .px(11)
    .py(4)
    .rounded(7)
    .border_1()
    .border_color(primary ? th.accent : danger ? th.ansi1 : th.border)
    .bg(primary ? th.accent : th.raised)
    .text_size(12.5)
    .text_color(primary ? th.accent_foreground : danger ? th.ansi1 : th.foreground)
    .when(disabled, (el) => el.opacity(0.45))
    .when(!disabled, (el) => el.cursor_pointer().hover((s) => (primary ? s.opacity(0.9) : s.bg(th.hover))).on_click(onClick))
    .child(label);
}

export function toggle(th, id, on, onClick) {
  return div()
    .id(id)
    .relative()
    .w(36)
    .h(20)
    .rounded(10)
    .cursor_pointer()
    .bg(on ? th.accent : th.hover)
    .on_click(onClick)
    .child(
      div()
        .id(`${id}-knob`)
        .absolute()
        .top(2)
        .left(on ? 18 : 2)
        .w(16)
        .h(16)
        .rounded(8)
        .bg(on ? th.accent_foreground : th.muted)
        .transition("left", motion("fast")),
    );
}

/** One of a few choices: [[value, label], …]. */
export function segmented(th, id, choices, current, pick) {
  return h_flex()
    .p(2)
    .gap(2)
    .rounded(8)
    .border_1()
    .border_color(th.border)
    .bg(th.background)
    .children(
      choices.map(([value, label]) =>
        div()
          .id(`${id}-${value}`)
          .px(10)
          .py(3)
          .rounded(6)
          .text_size(12.5)
          .cursor_pointer()
          .when(current === value, (el) => el.bg(th.raised).text_color(th.foreground).border_1().border_color(th.border))
          .when(current !== value, (el) => el.text_color(th.muted).hover((s) => s.text_color(th.foreground)))
          .on_click((_e, cx) => pick(value, cx))
          .child(label),
      ),
    );
}

/** A small label: source, state. */
export function badge(th, text, color) {
  return div().px(6).py(1).rounded(4).border_1().border_color(color ?? th.border).text_size(11).text_color(color ?? th.muted).child(text);
}

/** tone: "error", "warning", "ok", or muted. */
export function note(th, text, tone) {
  const color = tone === "error" ? th.ansi1 : tone === "warning" ? th.blocked : tone === "ok" ? th.ansi2 : th.dim;
  return div().text_size(12).line_height("17px").text_color(color).child(text);
}

export function spinner(th) {
  return svg("mods/workspace-chrome/spinner.svg").size(12).text_color(th.working).spin(900);
}

/** A labeled input: the label over it, a hint (or problem) under it. */
export function field(th, label, control, hint, tone) {
  return v_flex()
    .gap(4)
    .child(div().text_size(12).text_color(th.muted).child(label))
    .child(control)
    .when(Boolean(hint), (el) => el.child(note(th, hint, tone)));
}

/** An Input in the dialog's frame. */
export function input(th, state, focused, mono) {
  return Input.new(state)
    .h(30)
    .px(10)
    .rounded(7)
    .border_1()
    .border_color(focused ? th.accent : th.input)
    .bg(th.background)
    .text_size(13)
    .text_color(th.foreground)
    .when(Boolean(mono), (el) => el.font_family(mono));
}

/** A clickable line in a list. */
export function item(th, id, on, onClick, child) {
  return h_flex()
    .id(id)
    .w_full()
    .gap(10)
    .px(10)
    .py(7)
    .items_center()
    .rounded(7)
    .cursor_pointer()
    .bg(on ? th.raised : CLEAR)
    .hover((s) => s.bg(th.raised))
    .on_click(onClick)
    .child(child);
}
