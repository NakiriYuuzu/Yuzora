// yuzora(L6): Yuzora's part of the settings frame. A file of its own so
// main.js (Herdr's) only calls in, at lines marked `yuzora(L6)`.
//
// - The "Yuzora" group: every mod whose mod.json declares
//   "yuzora": { "settings": { title, icon, order, section } } (settingsPages()).
//   A page is that mod, so picking one turns the dialog into a row: this frame
//   as the nav only (props { yuzora: <mod> }), then the page's mod
//   (props { section }). Picking one of Herdr's sections goes back to the
//   frame as Herdr draws it (app.settings {section}).
// - Without a section, settings open on Yuzora's General page.
// - Brand and privacy text: Yuzora collects nothing and sends nothing.

import { div, svg } from "gpui-kit";
import { v_flex, h_flex } from "gpui-base";
import { openOverlay, closeOverlay, run, settings, props, shared, setShared } from "herdr";
import { settingsPages, t } from "yuzora:core";
import { session } from "yuzora:settings";

export const APP = "Yuzora";
/** The page settings open on when nobody asked for one. */
export const DEFAULT_PAGE = "yuzora-settings-general";
// The dialog as app.settings opens it (window.rs: width 880; main.js: h(620), nav w(190)).
const WIDTH = 880;
const HEIGHT = 620;
const NAV = 190;
// The Yuzora dialog open now, {id, run}: app.settings only knows its own
// ("settings"). Overlay ids restart with the app, so a kept one counts only
// in the run (session()) that opened it.
const OPEN = "yuzora.settings.overlay";

export function pages() {
  try {
    return settingsPages();
  } catch {
    return [];
  }
}

/** A Yuzora page by mod name or section key (app.settings {section: "git"}). */
export function find(asked) {
  if (!asked) return null;
  return pages().find((p) => p.mod === asked || p.section === asked) ?? null;
}

/** Shows `page` beside the nav, in place of the dialog `overlay`. `height`:
 *  the dialog's now (logical px, zoomed), so the switch doesn't jump: Herdr's
 *  dialog is its content's 620 px, at most 85% of the window. */
export function open(page, overlay, height) {
  const zoom = Number(settings().zoom) || 1;
  if (overlay) closeOverlay(overlay);
  const id = openOverlay({
    kind: "dialog",
    width: WIDTH,
    height: height > 0 ? height / zoom : HEIGHT,
    layout: {
      type: "row",
      children: [
        { type: "block", size: { px: NAV }, content: { mod: "frame-settings", props: { yuzora: page.mod } } },
        { type: "block", content: { mod: page.mod, props: { section: page.section } } },
      ],
    },
  });
  setShared(OPEN, { id, run: session() });
}

/** One settings dialog at a time: app.settings (⌘, the menu) while a Yuzora
 *  page is open closes that page's dialog instead of stacking on it. */
export function closeOthers(own) {
  const open = shared(OPEN);
  if (!open) return;
  const now = open.run === session();
  if (now && open.id === own) return;
  if (now) closeOverlay(open.id);
  setShared(OPEN, null);
}

/** The frame asked for a Yuzora page (`view.leaving`): opens it once, at the
 *  height the dialog came out (or without one, if no size arrives). */
export function leave(view, height) {
  if (view.left || !view.leaving) return;
  view.left = true;
  open(view.leaving, props()?.overlay, height);
}

/** Back to one of Herdr's sections, from the nav beside a Yuzora page. */
export function herdr(section, overlay) {
  if (overlay) closeOverlay(overlay);
  run("app.settings", { section });
}

/** The group in the nav: a heading, then one row per page (nothing without pages). */
export function group(th, current, pick) {
  const list = pages();
  if (!list.length) return div();
  return v_flex()
    .gap(2)
    .pb(10)
    .child(div().px(10).pt(2).pb(6).text_size(11).font_semibold().text_color(th.dim).child(t("group").toUpperCase()))
    .children(
      list.map((p) => {
        const on = p.mod === current;
        return h_flex()
          .id(`st-yz-${p.mod}`)
          .gap(10)
          .px(10)
          .py(7)
          .items_center()
          .rounded(7)
          .cursor_pointer()
          .bg(on ? th.raised : "#00000000")
          .hover((s) => s.bg(th.raised))
          .on_click((_e, cx) => pick(p, cx))
          .child(p.icon ? svg(p.icon).size(15).text_color(on ? th.accent : th.dim) : div().w(15).h(15))
          .child(div().flex_1().min_w(0).text_size(13.5).text_color(on ? th.foreground : th.muted).child(p.label));
      }),
    )
    .child(div().px(10).pt(10).pb(2).text_size(11).font_semibold().text_color(th.dim).child(t("app").toUpperCase()));
}

/** The Privacy section: Yuzora's own words in place of Herdr's usage data. */
export function privacy(th, heading, row) {
  return v_flex()
    .w_full()
    .child(heading(th, t("privacy.heading")))
    .child(row(th, t("privacy.title"), t("privacy.about"), div().text_size(12.5).text_color(th.done ?? th.accent).child("✓")))
    .child(row(th, t("privacy.usage"), t("privacy.usage_about"), div().text_size(12.5).text_color(th.dim).child(t("privacy.off"))))
    .child(row(th, t("privacy.feedback"), t("privacy.feedback_about"), div()))
    .child(div().pt(12).text_size(12.5).line_height("18px").text_color(th.dim).child(t("privacy.network")));
}

/** Updates: Yuzora's line in place of Herdr's (for a build that doesn't update itself). */
export function appUpdates(version) {
  return { title: `${APP} ${version}`, about: t("updates.app_off") };
}

export function updatesFooter() {
  return t("updates.footer");
}

export function defaultSound() {
  return t("alerts.default_sound");
}
