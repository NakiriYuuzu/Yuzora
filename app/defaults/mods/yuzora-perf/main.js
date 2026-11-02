// Resource usage in the window's bottom-right corner, right of the status bar. Owned by lane L8.
// The chip shows this computer's Yuzora (the app, its herdr-core, the tools it starts) as
// "{cpu}% · {memory}MB", with ⚠ while sampling has trouble; a press opens the details above it,
// right edges aligned (also "yuzora-perf.toggle"). yuzora:perf samples on a worker thread; this mod
// polls every 2 s and draws.

import { View, div, svg } from "gpui-kit";
import { h_flex, v_flex, Popover, ToggleGroup, Toggle } from "gpui-base";
import { theme, window, nextCommand } from "herdr";
import { t, tokens } from "yuzora:core";
import { state, tick } from "yuzora:perf";

const ICONS = "mods/yuzora-perf";
const CATEGORIES = [
  { id: "app", icon: `${ICONS}/app-window.svg` },
  { id: "core", icon: "mods/workspace-chrome/terminal.svg" },
  { id: "tools", icon: `${ICONS}/wrench.svg` },
];
const MONO = "JetBrains Mono";

const mb = (bytes) => bytes / 1_000_000;
const one = (n) => (Number.isFinite(n) ? n.toFixed(1) : "—");
const processes = (count) => (count === 1 ? t("process_one") : t("process_other", { count }));

export default class Perf extends View {
  init(_props, cx) {
    this.state = state();
    this.open = false;
    this.sort = "memory";
    this.poll(cx);
    cx.timer.every(2000, (cx) => this.poll(cx));
    // "yuzora-perf.toggle" (a key in keys.json, or herdr-app do) opens or closes the details.
    cx.spawn(async (cx) => {
      for (;;) {
        if ((await nextCommand()) !== "toggle") continue;
        this.open = !this.open;
        cx.notify();
      }
    });
  }

  // Sampled while the window is in front; otherwise the poll counts as skipped.
  poll(cx) {
    if (this.busy) return;
    this.busy = true;
    const focused = Boolean(window()?.active);
    cx.spawn(async (cx) => {
      try { this.state = await tick(focused); } catch { this.state = state(); }
      this.busy = false;
      cx.notify();
    });
  }

  render() {
    const th = theme(), tk = tokens();
    const { snapshot: snap, health: h } = this.state;
    const trouble = h.failures + h.empty;
    const warn = trouble > 0 || (snap !== null && snap.core_pid === null);
    const text = snap
      ? t("chip", { cpu: Math.round(snap.total.cpu), memory: Math.round(mb(snap.total.memory)) })
      : t(trouble > 0 ? "chip_unavailable" : "chip_waiting");
    const chip = h_flex().id("chip").h(20).px(6).gap(5).items_center().rounded(4).cursor_pointer()
      .text_color(this.open ? th.foreground : th.muted)
      .when(this.open, (el) => el.bg(th.selected))
      .hover((s) => s.bg(th.hover).text_color(th.foreground))
      .when(!this.open, (el) => el.tooltip(this.tip(snap, h)))
      .child(svg(`${ICONS}/cpu.svg`).size(12).flex_none())
      .child(div().whitespace_nowrap().child(text))
      .when(warn, (el) => el.child(div().text_color(tk.status.blocked.color).child("⚠")))
      .child(svg(`${ICONS}/chevron-up.svg`).size(10).flex_none());
    return h_flex().id("root").size_full().min_w(0).pr(4).justify_end().items_center()
      .bg(th.chrome).border_t(1).border_color(th.divider ?? th.border)
      .text_size(11.5).text_color(th.muted)
      .child(Popover.new("usage").anchor("bottom_right").open(this.open)
        .on_open_change((open, cx) => { this.open = open; cx.notify(); })
        .trigger(chip)
        .when(this.open, (el) => el.content(this.panel(th, tk))));
  }

  tip(snap, h) {
    const lines = [];
    if (snap) {
      const of = (u) => ({ cpu: Math.round(u.cpu), memory: Math.round(mb(u.memory)), processes: processes(u.count) });
      lines.push(t("tip.total", of(snap.total)), t("tip.app", of(snap.app)));
      lines.push(snap.core_pid === null ? t("tip.core_missing", { error: snap.core_error ?? "—" }) : t("tip.core", of(snap.core)));
      lines.push(t("tip.tools", of(snap.tools)));
    }
    if (h.failures + h.empty > 0) {
      lines.push(t("tip.sampling", { failures: h.failures, empty: h.empty, attempts: h.attempts, skipped: h.skipped, error: h.last_error ?? "—" }));
    }
    lines.push(t("tip.open"));
    return lines.join("\n");
  }

  panel(th, tk) {
    const { snapshot: snap, health: h, cost } = this.state;
    const amber = tk.status.blocked.color;
    const key = this.sort;
    const rows = snap ? CATEGORIES.map((c) => ({ ...c, ...snap[c.id] })).sort((a, b) => b[key] - a[key]) : [];
    const total = snap ? snap.total[key] : 0;
    const muted = (el) => el.text_size(12).text_color(th.muted);
    const amount = (label, icon, value, unit) => v_flex().flex_1().gap(4)
      .child(muted(h_flex().gap(6).items_center()).child(svg(icon).size(14).flex_none()).child(label))
      .child(h_flex().gap(4).items_end()
        .child(div().font_family(MONO).text_size(26).font_medium().text_color(th.foreground).child(value))
        .child(muted(div().pb(4)).text_size(13).child(unit)));
    const sortBy = (id) => Toggle.new(`sort-${id}`).pressed(key === id).accessibility_label(t(id))
      .on_change((pressed, cx) => { if (pressed) { this.sort = id; cx.notify(); } })
      .h(24).px(9).rounded(6).flex().items_center().cursor_pointer().text_size(12)
      .text_color(key === id ? th.foreground : th.muted)
      .when(key === id, (el) => el.bg(th.selected))
      .hover((s) => s.text_color(th.foreground))
      .child(t(id));
    const row = (r) => {
      const share = total > 0 ? (r[key] / total) * 100 : null;
      const missing = r.id === "core" && snap.core_pid === null;
      return v_flex().id(`row-${r.id}`).px(4).pt(9).pb(10).gap(6).border_b(1).border_color(th.divider)
        .child(h_flex().gap(9).items_start()
          .child(svg(r.icon).size(16).flex_none().mt(2).text_color(th.muted))
          .child(v_flex().flex_1().min_w(0).gap(3)
            .child(div().text_size(13).font_medium().child(t(`category.${r.id}`)))
            .child(muted(div()).child(`${processes(r.count)} · ${t(`detail.${r.id}`)}`)))
          .child(v_flex().flex_none().items_end().gap(3)
            .child(div().font_family(MONO).text_size(13).font_medium().text_color(missing ? amber : th.foreground)
              .child(key === "memory" ? `${one(mb(r.memory))} MB` : `${one(r.cpu)} %`))
            .child(muted(div()).child(share === null ? t("no_share") : t("share", { value: one(share) })))))
        .child(div().h(3).w_full().rounded(2).bg(th.divider).overflow_hidden()
          .child(div().h_full().rounded(2).bg(th.dim).w(`${Math.min(100, Math.max(0, share ?? 0))}%`)));
    };
    const close = h_flex().id("close").size(22).flex_none().rounded(5).items_center().justify_center().cursor_pointer()
      .text_color(th.muted).hover((s) => s.bg(th.hover).text_color(th.foreground))
      .accessibility_label(t("close")).tooltip(t("close"))
      .on_click((_event, cx) => { this.open = false; cx.notify(); })
      .child(svg("mods/workspace-chrome/x.svg").size(14));
    const notes = [t("measurement"), t("cadence")];
    if (snap?.core_pid != null) notes.push(t("core_pid", { pid: snap.core_pid }));
    if (h.attempts > 0) notes.push(t("sampling", { attempts: h.attempts, failures: h.failures, empty: h.empty, skipped: h.skipped }));
    if (cost) notes.push(t("cost", { median: one(cost.median_ms), p95: one(cost.p95_ms), samples: cost.samples }));
    const alerts = [];
    if (snap && snap.core_pid === null) alerts.push(t("core_missing", { error: snap.core_error ?? "—" }));
    if (h.failures > 0 && h.last_error) alerts.push(t("last_error", { error: h.last_error }));

    // The bottom padding is the gap above the status bar (Popover has no offset).
    return div().pb(6).child(v_flex().id("panel").w(420).bg(th.popover).border(1).border_color(th.border).rounded(10)
      .shadow_lg().overflow_hidden().text_size(12.5).text_color(th.foreground)
      .child(h_flex().px(16).pt(14).gap(8).items_center()
        .child(div().flex_1().text_size(15).font_semibold().child(t("title")))
        .child(close))
      .child(muted(div().px(16).pt(4).pb(12)).child(t("scope")))
      .child(h_flex().px(16).pb(12).gap(32)
        .child(amount(t("memory"), `${ICONS}/memory-stick.svg`, snap ? one(mb(snap.total.memory)) : "—", "MB"))
        .child(amount(t("cpu"), `${ICONS}/cpu.svg`, snap ? one(snap.total.cpu) : "—", "%")))
      .child(h_flex().id("summary").px(16).py(10).gap(12).bg(th.raised).border_t(1).border_b(1).border_color(th.divider)
        .accessibility_label(t("summary_label"))
        .children(CATEGORIES.map((c) => v_flex().flex_1().min_w(0).gap(3)
          .child(muted(div().truncate()).child(t(`category.${c.id}`)))
          .child(div().font_family(MONO).text_size(13).font_medium()
            .child(snap ? `${one(mb(snap[c.id].memory))} MB` : "—")))))
      .child(h_flex().pl(16).pr(12).pt(10).pb(6).items_center().justify_between()
        .child(muted(div()).child(t("largest_first")))
        .child(ToggleGroup.new("sort").axis("horizontal").accessibility_label(t("sort_label"))
          .flex().gap(2).p(2).border(1).border_color(th.divider).rounded(8)
          .child(sortBy("memory")).child(sortBy("cpu"))))
      .child(v_flex().id("list").px(12).accessibility_label(t("list_label"))
        .when(!snap, (el) => el.child(muted(div().px(4).py(10)).child(t("no_sample"))))
        .children(rows.map(row)))
      .child(v_flex().id("notes").px(16).py(10).gap(3)
        .children(notes.map((n) => muted(div()).child(n)))
        .children(alerts.map((n) => div().text_size(12).text_color(amber).child(n)))));
  }
}
