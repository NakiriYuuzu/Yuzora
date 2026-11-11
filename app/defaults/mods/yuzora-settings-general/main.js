// Settings > Yuzora > General: look, what happens when Yuzora
// opens, Herdr's panels (L1's), the terminal clipboard. Owned by lane L6.
//
// Startup lives in Yuzora's settings (yuzora/settings.json,
// section "general"); look and clipboard are Herdr's settings.json, written
// the way the settings frame writes it (read, change, write if unchanged).
// Themes and fonts themselves are L1's; this page only picks among them.

import { View, div } from "gpui-kit";
import { v_flex, h_flex, InputState, Input } from "gpui-base";
import { run, theme, themes, fonts, settings, layouts, layout, shared, setShared, appHome, readTextFile, writeTextFile } from "herdr";
import { t, getSetting, setSetting } from "yuzora:core";
import { page, title, heading, row, button, toggle, segmented, note, item } from "./ui.js";

// What the app uses when settings.json names no font (settings.rs).
const DEFAULT_FONT = "JetBrains Mono";
const DEFAULT_UI_FONT = "Geist";
const MODES = ["last", "none", "files", "git", "database", "ssh"];

function find(node, id) {
  if (node?.id === id) return node;
  for (const child of node?.children ?? []) {
    const found = find(child, id);
    if (found) return found;
  }
  return null;
}

export default class General extends View {
  init(_props, cx) {
    this.person = {};
    this.raw = null;
    this.picking = null; // "dark" | "light" | "font" | "ui_font" while its list is open
    this.filter = "";
    this.search = InputState.new({ placeholder: t("look.search") });
    this.search.on("change", (_e, cx) => {
      this.filter = this.search.value();
      cx.notify();
    });
    cx.spawn(async (cx) => {
      await this.load();
      cx.notify();
    });
  }

  file() {
    return `${appHome()}/settings.json`;
  }

  async load() {
    this.raw = await readTextFile(this.file());
    try {
      this.person = this.raw ? JSON.parse(this.raw) : {};
    } catch {
      this.person = {};
    }
  }

  // Herdr's settings.json: read, change, write back only if nobody changed it meanwhile.
  change(edit, cx) {
    cx.spawn(async (cx) => {
      for (let attempt = 0; attempt < 3; attempt++) {
        await this.load();
        const next = JSON.parse(JSON.stringify(this.person));
        edit(next);
        try {
          await writeTextFile(this.file(), JSON.stringify(next, null, 2) + "\n", this.raw);
          this.person = next;
          break;
        } catch {}
      }
      cx.notify();
    });
  }

  general() {
    const g = getSetting("general");
    return g && typeof g === "object" ? g : {};
  }

  setGeneral(edit) {
    const next = JSON.parse(JSON.stringify(this.general()));
    edit(next);
    setSetting("general", next);
  }

  // Look ----------------------------------------------------------------

  pickerButton(th, which, label, family) {
    const open = this.picking === which;
    return div()
      .id(`gn-pick-${which}`)
      .w(210)
      .px(10)
      .py(5)
      .rounded(7)
      .border_1()
      .border_color(open ? th.accent : th.border)
      .bg(th.raised)
      .cursor_pointer()
      .hover((s) => s.bg(th.hover))
      .on_click((_e, cx) => {
        this.picking = open ? null : which;
        this.filter = "";
        this.search.set_value("");
        if (this.picking) this.search.focus();
        cx.notify();
      })
      .child(div().when(Boolean(family), (el) => el.font_family(family)).text_size(13).text_color(th.foreground).truncate().child(label));
  }

  // The open picker's list: a search box over at most 200 matches.
  list(th, which, entries, current, choose) {
    const q = this.filter.toLowerCase();
    const matches = entries.filter((e) => e.label.toLowerCase().includes(q)).slice(0, 200);
    return v_flex()
      .w_full()
      .mt(2)
      .mb(8)
      .rounded(8)
      .border_1()
      .border_color(th.border)
      .bg(th.background)
      .on_scroll_wheel((_e, cx) => cx.stop_propagation())
      .child(div().px(10).py(7).border_b(1).border_color(th.border).child(Input.new(this.search).h(24).text_size(13).text_color(th.foreground)))
      .child(
        div()
          .id(`gn-list-${which}`)
          .h(190)
          .p(4)
          .overflow_y_scrollbar()
          .children(
            matches.map((e, i) =>
              item(th, `gn-li-${which}-${i}`, e.value === current, (_e, cx) => {
                choose(e.value, cx);
                this.picking = null;
                cx.notify();
              }, div().when(Boolean(e.family), (el) => el.font_family(e.family)).text_size(13).text_color(th.foreground).child(e.label)),
            ),
          )
          .when(!matches.length, (el) => el.child(div().p(8).child(note(th, t("look.none"))))),
      );
  }

  look(th) {
    const s = settings();
    const p = this.person;
    const layout = shared("layout");
    const themeList = (light) =>
      themes()
        .filter((x) => Boolean(x.light) === light && (x.name === layout || !layouts().includes(x.name)))
        .map((x) => ({ value: x.name, label: x.name.replace(/-/g, " ") }));
    const fontList = (def) => [{ value: null, label: t("look.default_font", { font: def }), family: def }, ...fonts().map((f) => ({ value: f, label: f, family: f }))];
    const setFont = (key) => (name, cx) => this.change((q) => (name === null ? delete q[key] : (q[key] = name)), cx);
    const size = (d, cx) => this.change((q) => (q.font_size = Math.max(8, Math.min(32, (q.font_size ?? s.font_size) + d))), cx);
    const stepper = (id, value, minus, plus) =>
      h_flex()
        .items_center()
        .gap(6)
        .child(button(th, `${id}-minus`, "−", minus))
        .child(div().w(46).flex().justify_center().text_size(13).text_color(th.foreground).child(value))
        .child(button(th, `${id}-plus`, "+", plus));
    const picker = (which, name, about, label, family, entries, current, choose) =>
      v_flex()
        .w_full()
        .child(row(th, name, about, this.pickerButton(th, which, label, family)))
        .when(this.picking === which, (el) => el.child(this.list(th, which, entries, current, choose)));
    return v_flex()
      .child(heading(th, t("look.heading")))
      .child(note(th, t("look.about")))
      .child(
        row(
          th,
          t("look.appearance"),
          t("look.appearance_about"),
          segmented(th, "gn-ap", [["system", t("look.system")], ["light", t("look.light")], ["dark", t("look.dark")]], p.appearance ?? "system", (v, cx) =>
            this.change((q) => (v === "system" ? delete q.appearance : (q.appearance = v)), cx),
          ),
        ),
      )
      .child(picker("dark", t("look.theme_dark"), null, String(s.theme_dark ?? "").replace(/-/g, " "), null, themeList(false), s.theme_dark, (name) => run("app.theme", { name })))
      .child(picker("light", t("look.theme_light"), null, String(s.theme_light ?? "").replace(/-/g, " "), null, themeList(true), s.theme_light, (name) => run("app.theme", { name })))
      .child(picker("ui_font", t("look.ui_font"), (p.ui_font ?? null) !== null && p.ui_font !== s.ui_font_family ? t("look.layout_font") : null, s.ui_font_family, s.ui_font_family, fontList(DEFAULT_UI_FONT), p.ui_font ?? null, setFont("ui_font")))
      .child(picker("font", t("look.font"), (p.font ?? null) !== null && p.font !== s.font_family ? t("look.layout_font") : null, s.font_family, s.font_family, fontList(DEFAULT_FONT), p.font ?? null, setFont("font")))
      .child(row(th, t("look.font_size"), null, stepper("gn-fs", `${Math.round(s.font_size)}`, (_e, cx) => size(-1, cx), (_e, cx) => size(1, cx))))
      .child(
        row(
          th,
          t("look.zoom"),
          t("look.zoom_about"),
          h_flex()
            .gap(8)
            .items_center()
            .child(stepper("gn-zoom", `${Math.round(s.zoom * 100)}%`, () => run("app.zoom_out"), () => run("app.zoom_in")))
            .child(button(th, "gn-zoom-reset", t("look.reset"), () => run("app.zoom_reset"))),
        ),
      );
  }

  // Startup -------------------------------------------------------------

  startup(th) {
    const now = this.general().startup?.dock ?? "last";
    return v_flex()
      .child(heading(th, t("startup.heading")))
      .child(row(th, t("startup.dock"), t("startup.dock_about"), div()))
      .child(
        h_flex()
          .flex_wrap()
          .gap(6)
          .pt(8)
          .children(
            MODES.map((m) =>
              div()
                .id(`gn-start-${m}`)
                .px(10)
                .py(4)
                .rounded(7)
                .border_1()
                .border_color(m === now ? th.accent : th.border)
                .bg(m === now ? th.raised : th.background)
                .text_size(12.5)
                .text_color(m === now ? th.foreground : th.muted)
                .cursor_pointer()
                .hover((s) => s.text_color(th.foreground))
                .on_click(() => this.setGeneral((g) => (g.startup = { ...(g.startup ?? {}), dock: m })))
                .child(t(`startup.modes.${m}`)),
            ),
          ),
      );
  }

  // Panels (lane L1) -----------------------------------------------------

  // Herdr's Files and Changes panels, as the welcome's third page turns them on and off
  // (shared "workspace.panels"; unset: none, see yuzora-shell-tabs). The tab bar shows a
  // button for each one on; turning off the one showing closes it too.
  setPanel(id, on) {
    const was = shared("workspace.panels");
    const now = (Array.isArray(was) ? was : []).filter((x) => x !== id);
    setShared("workspace.panels", on ? [...now, id] : now);
    const L = layout(), panel = find(L, "panel");
    if (!on && panel && !panel.hidden && find(L, "panel-body")?.active === id) run("layout.hide", { id: "panel" });
  }

  panels(th) {
    const saved = shared("workspace.panels"), on = Array.isArray(saved) ? saved : [];
    const line = (id) => row(th, t(`panels.${id}`), null, toggle(th, `gn-panel-${id}`, on.includes(id), (_e, cx) => {
      this.setPanel(id, !on.includes(id));
      cx.notify();
    }));
    return v_flex()
      .child(heading(th, t("panels.heading")))
      .child(note(th, t("panels.about")))
      .child(line("files"))
      .child(line("changes"));
  }

  // Terminal clipboard ---------------------------------------------------

  security(th, cx) {
    const on = this.person.clipboard_write !== false;
    return v_flex()
      .child(heading(th, t("security.heading")))
      .child(
        row(
          th,
          t("security.clipboard"),
          t("security.clipboard_about"),
          toggle(th, "gn-clip", on, (_e, cx) => this.change((q) => (on ? (q.clipboard_write = false) : delete q.clipboard_write), cx)),
        ),
      );
  }

  render(cx) {
    const th = theme();
    return page(
      th,
      "gn-body",
      v_flex()
        .w_full()
        .child(title(th, t("page.title"), t("page.about")))
        .child(this.look(th))
        // Only once something applies it (the dock's mode bar or drawer sets this flag).
        .when(Boolean(shared("yuzora.startup.honored")), (el) => el.child(this.startup(th)))
        .child(this.panels(th))
        .child(this.security(th, cx)),
    );
  }
}
