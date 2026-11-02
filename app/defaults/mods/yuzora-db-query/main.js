// A SQL query pane: an editor on top (⌘↵ runs the selection, else the
// statement under the cursor; ⇧⌘↵ runs everything), results below, one tab
// per statement that returned rows or changed some. Owned by lane L3.
// props: {key, connection, database, sql}. Without a connection it asks for
// one; sql is the editor's first text. Commands (keys, or `herdr-app do
// yuzora-db-query.<command>`): run, run_all, stop.
import { View, div } from "gpui-kit";
import { v_flex, h_flex } from "gpui-base";
import { theme, props, nextCommand } from "herdr";
import { t } from "yuzora:core";
import { connections, databases, query, job, cancel, release, history, clearHistory, DataGrid, SqlEditor, editorText, setEditorText, editorAction } from "yuzora:db";
import { gridColors } from "./ui.js";

export default class DbQuery extends View {
  init(_props, cx) {
    const p = props() ?? {};
    this.key = p?.key ?? `dbq-${Date.now().toString(36)}${Math.floor(Math.random() * 1e4)}`;
    this.connection = p?.connection ?? null;
    this.database = p?.database ?? null;
    this.state = null;
    this.jobId = null;
    this.shown = 0;
    this.menu = null; // "conn" | "db" | "history"
    this.dbs = [];
    this.split = 0.45;
    this.initial = typeof p?.sql === "string" ? p.sql : "";
    this.handle = cx.focus_handle();
    cx.spawn(async (cx) => {
      for (;;) {
        const c = await nextCommand();
        if (c === "focus") this.handle.focus();
        else if (c === "run" || c === "run_all") this.run(cx, c === "run_all");
        else if (c === "stop" && this.state?.running) cancel(this.jobId);
      }
    });
    if (this.connection) this.loadDbs(cx);
    cx.spawn(async (cx) => {
      for (;;) {
        const action = await editorAction(`${this.key}:sql`);
        if (action === "run") this.run(cx, false);
      }
    });
  }

  async close() {
    release(this.key);
  }

  conn() {
    const now = Date.now();
    if (!this.conns || now - this.connsAt > 1000) {
      this.conns = connections();
      this.connsAt = now;
    }
    return this.conns.find((c) => c.id === this.connection) ?? null;
  }

  loadDbs(cx) {
    const id = this.connection;
    cx.spawn(async (cx) => {
      try {
        const list = await databases(id);
        if (this.connection === id) this.dbs = list;
      } catch (_e) {
        this.dbs = [];
      }
      cx.notify();
    });
  }

  /** Runs the selection, else the statement under the cursor (`all`: the whole text). */
  run(cx, all) {
    if (!this.connection || this.state?.running) return;
    const text = editorText(`${this.key}:sql`);
    if (!text) return;
    const sql = (all ? text.text : text.selection.trim() ? text.selection : text.statement || text.text).trim();
    if (!sql) return;
    this.shown = 0;
    this.menu = null;
    this.watch(cx, query(this.connection, this.database, this.key, sql));
  }

  watch(cx, id) {
    this.jobId = id;
    this.state = null;
    cx.notify();
    cx.spawn(async (cx) => {
      let seen = 0;
      for (;;) {
        let st;
        try {
          st = await job(id, seen);
        } catch (_e) {
          return;
        }
        if (this.jobId !== id) return;
        this.state = st;
        seen = st.version;
        // Follow the newest result while the script runs.
        if (st.running) this.shown = Math.max(0, st.results.length - 1);
        cx.notify();
        if (!st.running) return;
      }
    });
  }

  pick(menu, cx) {
    this.menu = this.menu === menu ? null : menu;
    cx.notify();
  }

  menuList(th, items) {
    return v_flex().id(`menu-${this.menu}`).absolute().top(30).left(8).w(300).max_h(320).overflow_y_scrollbar().py(4)
      .rounded(6).border_1().border_color(th.border).bg(th.popover).shadow_md()
      .children(items);
  }

  menuItem(th, id, label, detail, action) {
    return h_flex().id(id).min_h(26).px(10).py(3).gap(8).items_center().text_size(12).cursor_pointer().hover((s) => s.bg(th.hover))
      .on_click((_e, cx) => { action(cx); this.menu = null; cx.notify(); })
      .child(div().flex_1().min_w(0).truncate().child(label))
      .when(Boolean(detail), (el) => el.child(div().flex_none().text_size(11).text_color(th.dim).child(detail)));
  }

  toolbar(th, conn) {
    const st = this.state;
    const running = Boolean(st?.running);
    const chip = (id, text, menu) => h_flex().id(id).h(20).px(8).gap(5).items_center().rounded(4).border_1().border_color(th.input)
      .text_size(11.5).cursor_pointer().hover((s) => s.bg(th.hover)).on_click((_e, cx) => this.pick(menu, cx))
      .child(div().max_w(160).truncate().child(text)).child(div().text_color(th.dim).child("▾"));
    const status = running
      ? t("running", { s: (st.elapsedMs / 1000).toFixed(1) })
      : st?.cancelled ? t("cancelled") : st?.error ? t("failed") : st ? t("finished", { ms: st.elapsedMs }) : "";
    return h_flex().h(28).flex_none().gap(6).pl(8).pr(6).items_center().text_size(12).border_b(1).border_color(th.divider ?? th.border)
      .child(chip("conn", conn?.name ?? t("pick_connection"), "conn"))
      .when(Boolean(conn), (el) => el.child(chip("db", this.database || conn.database || t("default_db"), "db")))
      .child(chip("history", t("history"), "history"))
      .child(div().flex_1().min_w(0).truncate().text_size(11.5).text_color(st?.error ? th.blocked : th.dim).child(status))
      .child(running
        ? div().id("stop").h(20).px(10).flex().flex_none().items_center().rounded(4).border_1().border_color(th.input)
          .text_size(11.5).cursor_pointer().hover((s) => s.opacity(0.88)).on_click(() => cancel(this.jobId)).child(t("stop"))
        : div().id("run").h(20).px(10).flex().flex_none().items_center().rounded(4).bg(th.solid).text_color(th.solid_foreground)
          .font_semibold().text_size(11.5).cursor_pointer().hover((s) => s.opacity(0.88)).tooltip(t("run_tip"))
          .on_click((_e, cx) => this.run(cx, false)).child(t("run")));
  }

  menuFor(th) {
    if (this.menu === "conn") {
      const list = connections();
      return this.menuList(th, list.length
        ? list.map((c) => this.menuItem(th, `c-${c.id}`, c.name, c.label, (cx) => {
          this.connection = c.id;
          this.database = null;
          this.dbs = [];
          this.loadDbs(cx);
        }))
        : [div().px(10).py(4).text_size(12).text_color(th.dim).child(t("no_connections"))]);
    }
    if (this.menu === "db") {
      return this.menuList(th, this.dbs.length
        ? this.dbs.map((d) => this.menuItem(th, `d-${d}`, d, "", () => { this.database = d; }))
        : [div().px(10).py(4).text_size(12).text_color(th.dim).child(t("no_databases"))]);
    }
    if (this.menu === "history") {
      const list = this.connection ? history(this.connection) : [];
      const items = list.slice(0, 50).map((h, i) => this.menuItem(th, `h-${i}`, h.sql.replace(/\s+/g, " "),
        `${h.ok ? "" : "✕ "}${h.elapsedMs} ms`, () => setEditorText(`${this.key}:sql`, h.sql)));
      if (list.length) items.push(this.menuItem(th, "h-clear", t("clear_history"), "", () => clearHistory(this.connection)));
      return this.menuList(th, items.length ? items : [div().px(10).py(4).text_size(12).text_color(th.dim).child(t("no_history"))]);
    }
    return null;
  }

  results(th) {
    const st = this.state;
    const results = st?.results ?? [];
    if (!st) return div().p(12).text_size(12).text_color(th.dim).child(this.connection ? t("hint") : t("pick_connection"));
    const current = results[Math.min(this.shown, Math.max(0, results.length - 1))];
    const tabs = h_flex().h(26).flex_none().px(8).gap(4).items_center().border_b(1).border_color(th.divider ?? th.border)
      .children(results.map((r, i) => div().id(`res-${i}`).h(20).px(8).flex().flex_none().items_center().rounded(4).text_size(11).cursor_pointer()
        .bg(i === this.shown ? th.selected : th.background).text_color(r.error ? th.blocked : i === this.shown ? th.foreground : th.muted)
        .on_click((_e, cx) => { this.shown = i; cx.notify(); })
        .child(r.columns ? t("result_rows", { n: i + 1, count: r.rows }) : t("result_affected", { n: i + 1, count: r.affected ?? 0 }))));
    let body;
    if (!current) {
      body = div().p(12).text_size(12).text_color(st.error ? th.blocked : th.dim).child(st.error ?? (st.running ? t("waiting") : t("no_results")));
    } else if (current.columns === 0) {
      body = v_flex().p(12).gap(6).text_size(12)
        .child(div().child(current.affected === null ? t("done") : t("affected", { count: current.affected })))
        .when(Boolean(current.error), (el) => el.child(div().text_color(th.blocked).child(current.error)))
        .child(div().text_color(th.dim).child(`${current.elapsedMs} ms`));
    } else {
      body = DataGrid.new(`${this.key}:grid`, { result: current.id, colors: gridColors(th) });
    }
    return v_flex().flex_1().min_h(0)
      .when(results.length > 0, (el) => el.child(tabs))
      .child(v_flex().flex_1().min_h(0).child(body))
      .when(Boolean(st.error && current), (el) => el.child(div().px(12).py(4).text_size(11.5).text_color(th.blocked).child(st.error)));
  }

  render() {
    const th = theme();
    const conn = this.conn();
    const menu = this.menuFor(th);
    return v_flex().id("root").track_focus(this.handle).size_full().relative()
      .on_size_change((size, cx) => { if (size.height !== this.height) { this.height = size.height; cx.notify(); } }).bg(th.background).text_color(th.foreground).text_size(12.5)
      .on_key_down((e, cx) => {
        if (e.key === "enter" && (e.modifiers.platform || e.modifiers.control)) { this.run(cx, e.modifiers.shift); cx.stop_propagation(); }
        else if (e.key === "escape" && this.menu) { this.menu = null; cx.notify(); }
      })
      .child(this.toolbar(th, conn))
      .child(v_flex().flex_1().min_h(0)
        .child(div().flex_none().h(Math.max(80, Math.round((this.height ?? 480) * this.split))).border_b(1).border_color(th.divider ?? th.border)
          .child(SqlEditor.new(`${this.key}:sql`, { engine: conn?.engine, value: this.initial })))
        .child(this.results(th)))
      .when(Boolean(menu), (el) => el.child(menu));
  }
}
