// Settings > Yuzora > Mods: the mods and layouts in Yuzora's home, shipped
// (the app installs and replaces them) or the person's own. Clone, open the
// folder, check, put a shipped one back, switch layouts, install a folder.
// Owned by lane L6.
//
// Shipped ones can't be edited for good: the next start moves edits to
// <name>-edited and puts the shipped one back. Clone first (mod.clone /
// layout.clone), then change the clone.

import { View, div } from "gpui-kit";
import { v_flex, h_flex, InputState } from "gpui-base";
import { theme, run, shared, pickFolder, props } from "herdr";
import { t, panes } from "yuzora:core";
import { listMods, resetMod, appStatus, checkMod, planInstall, installFolder, reveal } from "yuzora:settings";
import { page, title, heading, button, segmented, note, badge, spinner, input } from "./ui.js";

const MONO = "JetBrains Mono";

export default class Mods extends View {
  init(_props, cx) {
    this.tab = props()?.tab === "layouts" ? "layouts" : "mods";
    this.data = listMods();
    this.errors = [];
    this.statusError = null;
    this.busy = {}; // name -> what's running
    this.results = {}; // name -> { text, tone, output? }
    this.cloning = null; // { kind, name }
    this.install = null; // { path, plan | error, busy, done }
    this.cloneInput = InputState.new();
    this.cloneInput.on("focus", (_e, cx) => { this.cloneFocused = true; cx.notify(); });
    this.cloneInput.on("blur", (_e, cx) => { this.cloneFocused = false; cx.notify(); });
    this.cloneInput.on("submit", (_e, cx) => this.clone(cx));
    this.refresh(cx);
    // New folders (a clone, an agent's mod) show up on their own.
    cx.timer.every(5000, (cx) => {
      this.data = listMods();
      cx.notify();
    });
  }

  refresh(cx) {
    this.data = listMods();
    cx.spawn(async (cx) => {
      const s = await appStatus();
      this.errors = s.ok ? s.errors ?? [] : [];
      this.statusError = s.ok ? null : s.message ?? s.code;
      cx.notify();
    });
    cx.notify();
  }

  // `herdr-app status` lists "mod <copy name>: <error>": a copy is its folder's name.
  errorsOf(name) {
    return this.errors.filter((e) => e.startsWith(`mod ${name}: `)).map((e) => e.slice(`mod ${name}: `.length));
  }

  // Actions -------------------------------------------------------------

  startClone(kind, name, cx) {
    this.cloning = { kind, name };
    const taken = (n) => (kind === "mod" ? this.data.mods : this.data.layouts).some((m) => m.name === n);
    let as = `${name}-copy`;
    for (let i = 2; taken(as); i++) as = `${name}-copy-${i}`;
    this.cloneInput.set_value(as);
    this.cloneInput.focus();
    cx.notify();
  }

  clone(cx) {
    const c = this.cloning;
    if (!c) return;
    const as = this.cloneInput.value().trim();
    if (!/^[A-Za-z0-9_-]+$/.test(as)) {
      this.results[c.name] = { text: t("clone_name"), tone: "error" };
      return cx.notify();
    }
    run(c.kind === "mod" ? "mod.clone" : "layout.clone", { name: c.name, as });
    this.results[c.name] = { text: t(c.kind === "mod" ? "cloned" : "cloned_layout", { name: as }), tone: "ok" };
    this.cloning = null;
    this.refresh(cx);
  }

  check(name, cx) {
    this.busy[name] = "check";
    delete this.results[name];
    cx.spawn(async (cx) => {
      const r = await checkMod(name);
      delete this.busy[name];
      this.results[name] = r.ok ? { text: t("check_ok"), tone: "ok", output: r.output } : { text: t("check_failed"), tone: "error", output: r.output ?? r.message };
      this.refresh(cx);
    });
    cx.notify();
  }

  reset(m, cx) {
    try {
      const kept = resetMod(m.name);
      this.results[m.name] = { text: kept ? t("reset_kept", { name: kept }) : t("reset_done"), tone: "ok" };
    } catch (e) {
      this.results[m.name] = { text: String(e), tone: "error" };
    }
    this.refresh(cx);
  }

  open(path, name, cx) {
    try {
      reveal(path);
    } catch (e) {
      this.results[name] = { text: String(e), tone: "error" };
      cx.notify();
    }
  }

  // An editor pane (L5) opens a mod's main.js, when one is installed.
  editor() {
    try {
      return panes().find((p) => p.mod === "yuzora-editor" || p.mod.startsWith("yuzora-editor-")) ?? null;
    } catch {
      return null;
    }
  }

  pickInstall(cx) {
    cx.spawn(async (cx) => {
      const path = await pickFolder({ prompt: t("install_pick") });
      if (!path) return;
      this.install = { path, busy: true };
      cx.notify();
      const r = await planInstall(path);
      this.install = r.ok ? { path, plan: r.plan } : { path, error: r.code };
      cx.notify();
    });
  }

  doInstall(cx) {
    const i = this.install;
    if (!i?.plan || i.busy) return;
    i.busy = true;
    cx.spawn(async (cx) => {
      const r = await installFolder(i.path);
      this.install = r.ok ? { path: i.path, plan: r.plan, done: true } : { path: i.path, error: r.code, message: r.message };
      this.refresh(cx);
    });
    cx.notify();
  }

  // Drawing -------------------------------------------------------------

  sourceBadge(th, x) {
    const colors = { shipped: th.dim, edited: th.blocked, clone: th.done, personal: th.ansi2 };
    return badge(th, t(`source.${x.source}`), colors[x.source]);
  }

  cloneRow(th, name) {
    return h_flex()
      .gap(6)
      .items_center()
      .child(div().text_size(12).text_color(th.dim).child(t("clone_as")))
      .child(div().flex_1().child(input(th, this.cloneInput, this.cloneFocused, MONO)))
      .child(button(th, `md-cancel-${name}`, t("cancel"), (_e, cx) => { this.cloning = null; cx.notify(); }))
      .child(button(th, `md-clone-go-${name}`, t("clone"), (_e, cx) => this.clone(cx), "primary"));
  }

  result(th, name) {
    const r = this.results[name];
    if (!r) return null;
    return v_flex()
      .gap(4)
      .child(note(th, r.text, r.tone))
      .when(Boolean(r.output), (el) =>
        el.child(
          v_flex()
            .p(8)
            .rounded(6)
            .border_1()
            .border_color(th.border)
            .bg(th.background)
            .children(String(r.output).split("\n").slice(0, 24).map((l) => div().font_family(MONO).text_size(11).line_height("15px").text_color(th.muted).child(l || " "))),
        ),
      );
  }

  modRow(th, m) {
    const errors = this.errorsOf(m.name);
    const busy = this.busy[m.name];
    const editor = this.editor();
    const actions = h_flex()
      .gap(6)
      .flex_none()
      .when(Boolean(busy), (el) => el.child(spinner(th)))
      .child(button(th, `md-clone-${m.name}`, t("clone"), (_e, cx) => this.startClone("mod", m.name, cx)))
      .child(button(th, `md-open-${m.name}`, t("open_folder"), (_e, cx) => this.open(m.path, m.name, cx)))
      .when(Boolean(editor) && m.main, (el) =>
        el.child(button(th, `md-edit-${m.name}`, t("open_editor"), () => run("tab.new", { mod: editor.mod, props: { path: `${m.path}/main.js` } }))),
      )
      .child(button(th, `md-check-${m.name}`, t("check"), (_e, cx) => this.check(m.name, cx), "plain", Boolean(busy) || !m.main))
      .when(m.source === "edited" && m.resettable, (el) => el.child(button(th, `md-reset-${m.name}`, t("reset"), (_e, cx) => this.reset(m, cx), "danger")));
    return v_flex()
      .w_full()
      .py(9)
      .gap(5)
      .border_b(1)
      .border_color(th.border)
      .child(
        h_flex()
          .gap(10)
          .items_center()
          .child(
            v_flex()
              .flex_1()
              .min_w(0)
              .gap(3)
              .child(
                h_flex()
                  .gap(8)
                  .items_center()
                  .child(div().text_size(13).font_semibold().text_color(th.foreground).child(m.name))
                  .child(this.sourceBadge(th, m))
                  .when(Boolean(errors.length), (el) => el.child(badge(th, t("has_error"), th.ansi1))),
              )
              .child(
                note(
                  th,
                  [m.title, m.cloned_from ? t("cloned_from", { name: m.cloned_from.name, version: m.cloned_from.version ?? "?" }) : null, m.settings ? t("is_settings") : null, m.pane ? t("is_pane") : null]
                    .filter(Boolean)
                    .join(" · ") || " ",
                ),
              ),
          )
          .child(actions),
      )
      .when(m.source === "edited", (el) => el.child(note(th, m.resettable ? t("edited_about", { name: m.name }) : t("edited_later", { name: m.name }), "warning")))
      .children(errors.map((e) => note(th, e, "error")))
      .when(this.cloning?.kind === "mod" && this.cloning.name === m.name, (el) => el.child(this.cloneRow(th, m.name)))
      .child(this.result(th, m.name) ?? div());
  }

  layoutRow(th, l) {
    const current = shared("layout") === l.name;
    const key = `layout:${l.name}`;
    return v_flex()
      .w_full()
      .py(9)
      .gap(5)
      .border_b(1)
      .border_color(th.border)
      .child(
        h_flex()
          .gap(10)
          .items_center()
          .child(
            v_flex()
              .flex_1()
              .min_w(0)
              .gap(3)
              .child(
                h_flex()
                  .gap(8)
                  .items_center()
                  .child(div().text_size(13).font_semibold().text_color(th.foreground).child(l.name))
                  .child(this.sourceBadge(th, l))
                  .when(current, (el) => el.child(badge(th, t("in_use"), th.accent)))
                  .when(!l.valid, (el) => el.child(badge(th, t("has_error"), th.ansi1))),
              )
              .child(note(th, l.cloned_from ? t("cloned_from", { name: l.cloned_from.name, version: l.cloned_from.version ?? "?" }) : " ")),
          )
          .child(
            h_flex()
              .gap(6)
              .flex_none()
              .child(button(th, `ly-switch-${l.name}`, t("switch"), () => run("layout.switch", { name: l.name }), "primary", current || !l.valid))
              .child(button(th, `ly-clone-${l.name}`, t("clone"), (_e, cx) => this.startClone("layout", l.name, cx)))
              .child(button(th, `ly-open-${l.name}`, t("open_folder"), (_e, cx) => this.open(l.path, key, cx)))
              .when(l.shipped, (el) =>
                el.child(button(th, `ly-reset-${l.name}`, t("reset"), (_e, cx) => {
                  run("layout.reset", { name: l.name });
                  this.results[key] = { text: t("layout_reset"), tone: "ok" };
                  cx.notify();
                }, "danger")),
              ),
          ),
      )
      .when(this.cloning?.kind === "layout" && this.cloning.name === l.name, (el) => el.child(this.cloneRow(th, l.name)))
      .child(this.result(th, l.name) ?? this.result(th, key) ?? div());
  }

  installPanel(th) {
    const i = this.install;
    if (!i) return null;
    const box = v_flex().p(12).gap(8).rounded(8).border_1().border_color(i.done ? th.border : th.blocked).bg(th.background);
    if (i.busy && !i.plan) return box.child(h_flex().gap(8).child(spinner(th)).child(note(th, t("install_reading"))));
    if (i.error) {
      return box
        .child(note(th, t(`install_errors.${i.error}`), "error"))
        .when(Boolean(i.message), (el) => el.child(note(th, i.message)))
        .child(h_flex().child(button(th, "in-close", t("close"), (_e, cx) => { this.install = null; cx.notify(); })));
    }
    const p = i.plan;
    const pairs = (list) => list.map((x) => (x.from === x.as ? x.as : t("renamed", { from: x.from, as: x.as }))).join(", ");
    box
      .when(!i.done, (el) => el.child(div().text_size(13).font_semibold().text_color(th.blocked).child(t("trust_title"))).child(note(th, t("trust_about"))))
      .when(Boolean(i.done), (el) => el.child(note(th, t("installed"), "ok")))
      .child(note(th, `${t("install_from")} ${p.source}`))
      .when(p.mods.length > 0, (el) => el.child(note(th, t("install_mods", { list: pairs(p.mods) }))))
      .when(p.layouts.length > 0, (el) => el.child(note(th, t("install_layouts", { list: pairs(p.layouts) }))))
      .when(p.reaches.length > 0, (el) => el.child(note(th, t("install_reaches", { list: p.reaches.map((r) => t(`reaches.${r}`)).join(t("list_sep")) }), "warning")))
      .when(p.keys, (el) => el.child(note(th, t("install_keys"), "warning")));
    if (i.done) {
      return box.child(
        h_flex()
          .gap(6)
          .children(p.layouts.map((l) => button(th, `in-switch-${l.as}`, t("switch_to", { name: l.as }), () => run("layout.switch", { name: l.as }), "primary")))
          .child(button(th, "in-close", t("close"), (_e, cx) => { this.install = null; cx.notify(); })),
      );
    }
    return box.child(
      h_flex()
        .gap(6)
        .child(div().flex_1())
        .when(Boolean(i.busy), (el) => el.child(spinner(th)))
        .child(button(th, "in-cancel", t("cancel"), (_e, cx) => { this.install = null; cx.notify(); }))
        .child(button(th, "in-go", t("install_go"), (_e, cx) => this.doInstall(cx), "danger", Boolean(i.busy))),
    );
  }

  render(cx) {
    const th = theme();
    const list = this.tab === "mods" ? this.data.mods : this.data.layouts;
    return page(
      th,
      "md-body",
      v_flex()
        .w_full()
        .gap(6)
        .child(title(th, t("page.title"), t("page.about")))
        .child(
          h_flex()
            .gap(8)
            .items_center()
            .child(segmented(th, "md-tab", [["mods", t("tab_mods", { n: this.data.mods.length })], ["layouts", t("tab_layouts", { n: this.data.layouts.length })]], this.tab, (v, cx) => { this.tab = v; this.cloning = null; cx.notify(); }))
            .child(div().flex_1())
            .child(button(th, "md-refresh", t("refresh"), (_e, cx) => this.refresh(cx)))
            .child(button(th, "md-install", t("install"), (_e, cx) => this.pickInstall(cx), "primary")),
        )
        .when(Boolean(this.statusError), (el) => el.child(note(th, t("status_failed", { message: this.statusError }), "warning")))
        .child(this.installPanel(th) ?? div())
        .child(heading(th, this.tab === "mods" ? t("heading_mods") : t("heading_layouts")))
        .child(note(th, this.tab === "mods" ? t("about_mods") : t("about_layouts")))
        .children(list.map((x) => (this.tab === "mods" ? this.modRow(th, x) : this.layoutRow(th, x)))),
    );
  }
}
