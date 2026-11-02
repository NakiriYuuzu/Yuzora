// A table pane: Data (the rows, editable when the table has a primary key),
// Structure (columns, indexes, foreign keys, DDL) and SQL (this table's
// query, ⌘↵ runs it). Owned by lane L3.
// props: {key, connection, database, schema, name, kind}. The sidebar
// retargets an open pane through shared "yuzora.db.open" ({key, target, seq}).
import { View, div } from "gpui-kit";
import { v_flex, h_flex } from "gpui-base";
import { theme, props, nextCommand, shared, setShared, focused } from "herdr";
import { t } from "yuzora:core";
import { connections, openTable, query, job, cancel, release, describe, DataGrid, SqlEditor, editorText, setEditorText, editorAction } from "yuzora:db";
import { gridColors, highlight } from "./ui.js";

const OPEN = "yuzora.db.open";
const TAB_PANES = "yuzora.db.tabPanes";
const TABS = ["data", "structure", "sql"];

export default class DbTable extends View {
  init(p, cx) {
    this.key = p?.key ?? `dbt-${Date.now().toString(36)}`;
    this.target = p?.connection && p?.name ? { connection: p.connection, database: p.database ?? null, schema: p.schema ?? "", name: p.name, kind: p.kind ?? "table" } : null;
    this.seq = shared(OPEN)?.seq ?? 0;
    this.tab = "data";
    this.jobId = null;
    this.state = null;
    this.shown = 0; // index into state.results
    this.info = null;
    this.infoError = null;
    this.sql = "";
    this.handle = cx.focus_handle();
    cx.spawn(async () => {
      for (;;) if ((await nextCommand()) === "focus") this.handle.focus();
    });
    const tab = focused()?.tab;
    const map = shared(TAB_PANES) ?? {};
    if (tab && !map[tab]) setShared(TAB_PANES, { ...map, [tab]: this.key });
    if (this.target) this.load(cx);
    cx.spawn(async (cx) => {
      for (;;) {
        const action = await editorAction(`${this.key}:sql`);
        if (action === "run") this.runSql(cx);
      }
    });
  }

  async close() {
    release(this.key);
  }

  conn() {
    return this.target ? connections().find((c) => c.id === this.target.connection) ?? null : null;
  }

  load(cx, order) {
    const tg = this.target;
    this.info = null;
    this.infoError = null;
    this.shown = 0;
    this.watch(cx, openTable(tg.connection, tg.database, this.key, { schema: tg.schema, name: tg.name }, order ?? null));
    cx.spawn(async (cx) => {
      try {
        const info = await describe(tg.connection, tg.database, { schema: tg.schema, name: tg.name });
        if (this.target === tg) {
          this.info = info;
          setEditorText(`${this.key}:ddl`, info.ddl ?? "");
        }
      } catch (e) {
        if (this.target === tg) this.infoError = String(e?.message ?? e);
      }
      cx.notify();
    });
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
        const first = st.results[0];
        if (first && !this.sql) {
          this.sql = first.statement;
          setEditorText(`${this.key}:sql`, this.sql);
        }
        cx.notify();
        if (!st.running) return;
      }
    });
  }

  run(cx) {
    if (this.state?.running) return;
    if (this.tab === "sql") this.runSql(cx);
    else if (this.target) {
      this.sql = "";
      this.load(cx);
    }
  }

  runSql(cx) {
    if (!this.target || this.state?.running) return;
    const text = editorText(`${this.key}:sql`);
    const sql = (text?.selection?.trim() ? text.selection : text?.text ?? "").trim();
    if (!sql) return;
    this.sql = sql;
    this.tab = "data";
    this.shown = 0;
    this.watch(cx, query(this.target.connection, this.target.database, this.key, sql));
  }

  stop() {
    if (this.jobId) cancel(this.jobId);
  }

  /** The sidebar picked another table for this pane. */
  retarget(cx) {
    const o = shared(OPEN);
    if (!o || o.key !== this.key || o.seq === this.seq) return;
    this.seq = o.seq;
    this.target = o.target;
    this.sql = "";
    this.tab = "data";
    this.load(cx);
  }

  toolbar(th, conn) {
    const tg = this.target;
    const running = Boolean(this.state?.running);
    const where = [tg.database ?? conn?.database ?? "", tg.schema ? `${tg.schema}.${tg.name}` : tg.name].filter(Boolean).join(" · ");
    return h_flex().h(28).flex_none().gap(7).pl(8).pr(6).items_center().text_size(12).border_b(1).border_color(th.divider ?? th.border)
      .child(div().flex_none().font_semibold().child(conn?.name ?? tg.connection))
      .child(div().min_w(0).truncate().text_size(11.5).text_color(th.dim).child(where))
      .child(h_flex().flex_none().p(2).gap(2).rounded(5).bg(th.raised)
        .children(TABS.map((id) => div().id(`tab-${id}`).h(18).px(8).flex().items_center().rounded(3).text_size(11).cursor_pointer()
          .bg(this.tab === id ? th.selected : th.raised).text_color(this.tab === id ? th.foreground : th.muted)
          .hover((s) => s.text_color(th.foreground))
          .on_click((_e, cx) => { this.tab = id; cx.notify(); }).child(t(`tab.${id}`)))))
      .child(div().flex_1())
      .child(running
        ? div().id("stop").h(20).px(10).flex().flex_none().items_center().rounded(4).border_1().border_color(th.input)
          .text_size(11.5).cursor_pointer().hover((s) => s.opacity(0.88)).on_click(() => this.stop()).child(t("stop"))
        : div().id("run").h(20).px(10).flex().flex_none().items_center().rounded(4).bg(th.solid).text_color(th.solid_foreground)
          .font_semibold().text_size(11.5).cursor_pointer().hover((s) => s.opacity(0.88)).tooltip(t(this.tab === "sql" ? "run_sql_tip" : "refresh_tip"))
          .on_click((_e, cx) => this.run(cx)).child(t("run")));
  }

  data(th) {
    const st = this.state;
    const results = st?.results ?? [];
    const current = results[Math.min(this.shown, Math.max(0, results.length - 1))];
    const strip = h_flex().h(30).flex_none().px(12).items_center().bg(th.chrome).text_size(12).overflow_hidden()
      .child(h_flex().min_w(0).gap(0).children(highlight(this.sql || current?.statement || "", th)));
    let body;
    if (!st) body = div().p(12).text_size(12).text_color(th.dim).child(t("loading"));
    else if (st.error && !current?.columns) body = div().p(12).text_size(12).text_color(th.blocked).child(st.error);
    else if (current && current.columns === 0) {
      body = v_flex().p(12).gap(6).text_size(12)
        .child(div().text_color(th.foreground).child(current.affected === null ? t("done") : t("affected", { count: current.affected })))
        .when(Boolean(current.error), (el) => el.child(div().text_color(th.blocked).child(current.error)))
        .child(div().text_color(th.dim).child(`${current.elapsedMs} ms`));
    } else {
      body = DataGrid.new(`${this.key}:grid`, { result: current?.id ?? null, colors: gridColors(th) });
    }
    const chips = results.length > 1
      ? h_flex().h(26).flex_none().px(8).gap(4).items_center().border_b(1).border_color(th.divider ?? th.border)
        .children(results.map((r, i) => div().id(`res-${i}`).h(20).px(8).flex().items_center().rounded(4).text_size(11).cursor_pointer()
          .bg(i === this.shown ? th.selected : th.chrome).text_color(i === this.shown ? th.foreground : th.muted)
          .on_click((_e, cx) => { this.shown = i; cx.notify(); })
          .child(r.columns ? t("result_rows", { n: i + 1, count: r.rows }) : t("result_affected", { n: i + 1, count: r.affected ?? 0 }))))
      : null;
    return v_flex().flex_1().min_h(0).child(strip).when(Boolean(chips), (el) => el.child(chips))
      .child(v_flex().flex_1().min_h(0).child(body))
      .when(Boolean(st?.error && current?.columns), (el) => el.child(div().px(12).py(4).text_size(11.5).text_color(th.blocked).child(st.error)));
  }

  structure(th) {
    if (this.infoError) return div().p(12).text_size(12).text_color(th.blocked).child(this.infoError);
    const info = this.info;
    if (!info) return div().p(12).text_size(12).text_color(th.dim).child(t("loading"));
    const head = (cols) => h_flex().h(26).px(12).bg(th.raised).text_size(11.5).font_semibold().text_color(th.muted)
      .children(cols.map(([text, w]) => div().w(w).flex_none().truncate().child(text)));
    const line = (cells, i) => h_flex().id(`l-${i}`).h(26).px(12).items_center().text_size(12).border_b(1).border_color(th.divider ?? th.border)
      .children(cells.map(([text, w, color]) => div().w(w).flex_none().pr(8).truncate().text_color(color ?? th.foreground).child(text)));
    const section = (title) => div().pt(14).pb(6).px(12).text_size(11.5).font_semibold().text_color(th.dim).child(title);
    const colW = [["name", 180], ["type", 170], ["nullable", 80], ["default", 220], ["key", 60]];
    return v_flex().id("structure").flex_1().min_h(0).overflow_y_scrollbar().pb(12)
      .child(section(t("structure.columns", { count: info.columns.length })))
      .child(head(colW.map(([k, w]) => [t(`structure.${k}`), w])))
      .children(info.columns.map((c, i) => line([
        [c.name, 180], [c.typeName, 170, th.muted], [c.nullable ? "NULL" : "NOT NULL", 80, th.dim],
        [c.default ?? "", 220, th.dim], [c.primaryKey ? `PK ${c.primaryKey}` : "", 60, th.blocked],
      ], i)))
      .when(info.indexes.length > 0, (el) => el
        .child(section(t("structure.indexes", { count: info.indexes.length })))
        .children(info.indexes.map((x, i) => line([
          [x.name, 240], [x.columns.join(", "), 260, th.muted], [x.primary ? "PRIMARY" : x.unique ? "UNIQUE" : "", 90, th.dim],
        ], `i${i}`))))
      .when(info.foreignKeys.length > 0, (el) => el
        .child(section(t("structure.foreign_keys", { count: info.foreignKeys.length })))
        .children(info.foreignKeys.map((f, i) => line([
          [f.name, 220], [f.columns.join(", "), 160, th.muted],
          [`→ ${f.referenced.schema ? `${f.referenced.schema}.` : ""}${f.referenced.name} (${f.referencedColumns.join(", ")})`, 320, th.dim],
        ], `f${i}`))))
      .when(Boolean(info.ddl), (el) => el
        .child(section("DDL"))
        .child(div().mx(12).h(Math.min(420, 22 + 18 * (info.ddl.split("\n").length + 1))).rounded(6).border_1().border_color(th.border).overflow_hidden()
          .child(SqlEditor.new(`${this.key}:ddl`, { engine: this.conn()?.engine, value: info.ddl, readonly: true }))));
  }

  sqlTab(th) {
    return v_flex().flex_1().min_h(0)
      .child(div().flex_1().min_h(0).child(SqlEditor.new(`${this.key}:sql`, { engine: this.conn()?.engine, value: this.sql })))
      .child(div().h(24).flex_none().px(12).flex().items_center().text_size(11).text_color(th.dim).border_t(1).border_color(th.divider ?? th.border)
        .child(t("sql_hint")));
  }

  render(cx) {
    this.retarget(cx);
    const th = theme();
    const root = v_flex().id("root").track_focus(this.handle).size_full().bg(th.background).text_color(th.foreground).text_size(12.5)
      .on_key_down((e, cx) => {
        if (e.key === "enter" && (e.modifiers.platform || e.modifiers.control)) { this.run(cx); cx.stop_propagation(); }
      });
    if (!this.target) return root.child(div().p(16).text_size(12).text_color(th.dim).child(t("pick")));
    const conn = this.conn();
    const content = this.tab === "structure" ? this.structure(th) : this.tab === "sql" ? this.sqlTab(th) : this.data(th);
    return root.child(this.toolbar(th, conn)).child(content);
  }
}
