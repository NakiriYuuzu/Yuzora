// The sidebar's Database mode: saved connections, a database picker and the
// schema tree. Owned by lane L3. Clicking a table opens it in the tab's
// yuzora-db-table pane (one per tab); "SQL" opens a yuzora-db-query pane.
import { View, div, svg, uniform_list } from "gpui-kit";
import { v_flex, h_flex } from "gpui-base";
import { theme, run, shared, setShared, focused, panes, openOverlay } from "herdr";
import { t, tokens } from "yuzora:core";
import { connections, connect, databases, objects } from "yuzora:db";

const ACTIVE = "yuzora.db.active"; // {connection, database}: what the sidebar shows
const OPEN = "yuzora.db.open"; // {key, target, seq}: retarget a table pane
const TAB_PANES = "yuzora.db.tabPanes"; // {tab: key}: each tab's table pane
const STATUS = "yuzora.status.db";
const ROW = 25;
const OTHER_KINDS = ["function", "procedure", "sequence", "trigger", "index", "type", "other"];

/** 2104 → "2.1k", 1200000 → "1.2M". */
function short(n) {
  if (n === null || n === undefined || n < 0) return "";
  if (n < 1000) return String(n);
  if (n < 1e6) return `${(n / 1e3).toFixed(n < 1e4 ? 1 : 0).replace(/\.0$/, "")}k`;
  return `${(n / 1e6).toFixed(n < 1e7 ? 1 : 0).replace(/\.0$/, "")}M`;
}

function major(version) {
  const m = /^(.*?\s\d+)(?:\.\d+)*/.exec(version ?? "");
  return m ? m[1] : version ?? "";
}

function dirname(path) {
  const i = (path ?? "").replace(/[\\/]+$/, "").search(/[\\/][^\\/]*$/);
  return i > 0 ? path.slice(0, i) : path;
}

export default class SidebarDatabase extends View {
  init(_props, cx) {
    this.list = [];
    this.listAt = 0;
    this.dbs = {}; // connection → database names
    this.trees = {}; // `${connection}|${database}` → {loading, error, items}
    this.expanded = new Set();
    this.dbOpen = false;
    this.busy = {};
    const active = shared(ACTIVE);
    if (active?.connection) cx.spawn(async (cx) => { await this.load(active.connection, active.database ?? null, cx); });
  }

  conns() {
    const now = Date.now();
    if (now - this.listAt > 400) {
      this.list = connections();
      this.listAt = now;
    }
    return this.list;
  }

  active() {
    const a = shared(ACTIVE);
    const list = this.conns();
    const conn = list.find((c) => c.id === a?.connection) ?? null;
    return { conn, database: conn ? a?.database ?? null : null };
  }

  treeKey(id, database) {
    return `${id}|${database ?? ""}`;
  }

  async load(id, database, cx, force) {
    const key = this.treeKey(id, database);
    if (!force && this.trees[key]?.items) return;
    this.trees[key] = { loading: true };
    cx.notify();
    try {
      await connect(id, database);
      if (force || !this.dbs[id]) this.dbs[id] = await databases(id).catch(() => []);
      const items = await objects(id, database);
      this.trees[key] = { items };
      const schemas = [...new Set(items.map((o) => o.schema))];
      const first = schemas.find((s) => s === "public" || s === "dbo" || s === "") ?? schemas[0];
      if (first !== undefined && ![...this.expanded].some((e) => e.startsWith(`${key}§s:`))) this.expanded.add(`${key}§s:${first}`);
    } catch (e) {
      this.trees[key] = { error: String(e?.message ?? e) };
    }
    this.listAt = 0;
    cx.notify();
  }

  select(conn, database, cx) {
    this.dbOpen = false;
    setShared(ACTIVE, { connection: conn.id, database });
    cx.spawn(async (cx) => { await this.load(conn.id, database, cx); });
  }

  openDialog(conn) {
    openOverlay({ mod: "yuzora-db-connection", kind: "dialog", modal: true, width: 480, height: 640, props: { id: conn?.id ?? null } });
  }

  openQuery(conn, database) {
    const props = { key: `dbq-${Date.now().toString(36)}`, connection: conn.id, database };
    if (focused()?.tab) run("pane.split", { direction: "right", mod: "yuzora-db-query", props });
    else run("tab.new", { mod: "yuzora-db-query", props });
  }

  openTable(conn, database, item) {
    const target = { connection: conn.id, database, schema: item.schema, name: item.name, kind: item.kind };
    const tab = focused()?.tab;
    const map = shared(TAB_PANES) ?? {};
    const key = tab ? map[tab] : null;
    if (key && panes().some((p) => p.tab === tab && p.mod === "yuzora-db-table")) {
      setShared(OPEN, { key, target, seq: Date.now() });
      return;
    }
    const fresh = `dbt-${Date.now().toString(36)}`;
    if (tab) {
      run("pane.split", { direction: "right", mod: "yuzora-db-table", props: { key: fresh, ...target } });
      setShared(TAB_PANES, { ...map, [tab]: fresh });
    } else {
      run("tab.new", { mod: "yuzora-db-table", props: { key: fresh, ...target } });
    }
  }

  /** The tree's visible rows for one connection and database. */
  rows(key, items, hasSchemas) {
    const out = [];
    const bySchema = new Map();
    for (const o of items) {
      if (!bySchema.has(o.schema)) bySchema.set(o.schema, []);
      bySchema.get(o.schema).push(o);
    }
    const pushObjects = (list, depth, prefix) => {
      for (const o of list.filter((o) => o.kind === "table")) out.push({ id: `${prefix}o:${o.kind}:${o.name}`, depth, item: o });
      for (const o of list.filter((o) => o.kind === "view" || o.kind === "materialized_view")) out.push({ id: `${prefix}o:${o.kind}:${o.name}`, depth, item: o });
      for (const kind of OTHER_KINDS) {
        const of = list.filter((o) => o.kind === kind);
        if (!of.length) continue;
        const folder = `${prefix}k:${kind}`;
        const open = this.expanded.has(`${key}§${folder}`);
        out.push({ id: folder, depth, folder: t(`kind.${kind}`), count: of.length, open });
        if (open) for (const o of of) out.push({ id: `${folder}|o:${o.name}|${o.detail}`, depth: depth + 1, item: o });
      }
    };
    if (!hasSchemas) {
      pushObjects(items, 0, "");
      return out;
    }
    for (const [schema, list] of bySchema) {
      const id = `s:${schema}`;
      const open = this.expanded.has(`${key}§${id}`);
      out.push({ id, depth: 0, folder: schema, count: list.length, open, schema: true });
      if (open) pushObjects(list, 1, `${id}|`);
    }
    return out;
  }

  row(r, th, tk) {
    const pad = 6 + r.depth * 13;
    const base = h_flex().h(ROW).gap(6).pl(pad).pr(6).items_center().rounded(5).hover((s) => s.bg(th.hover));
    if (r.folder !== undefined) {
      return base
        .child(div().w(10).flex_none().text_size(10).text_color(th.dim).child(r.open ? "▾" : "▸"))
        .child(svg("mods/yuzora-sidebar-database/folder.svg").w(13).h(10).flex_none().text_color("#6f7078"))
        .child(div().flex_1().min_w(0).truncate().text_size(11.5).child(r.folder || "main"))
        .child(div().flex_none().text_size(11).text_color(th.dim).child(String(r.count)));
    }
    const o = r.item;
    const isTable = o.kind === "table";
    const isView = o.kind === "view" || o.kind === "materialized_view";
    const icon = isTable ? "table" : isView ? "view" : o.kind === "function" || o.kind === "procedure" ? "routine" : "other";
    const color = isTable ? tk.database?.table ?? "#7dd3fc" : isView ? tk.database?.view ?? "#a78bfa" : th.dim;
    const meta = isTable ? short(o.rows) : isView ? t(`kind.${o.kind}`) : o.detail;
    return base
      .child(div().w(10).flex_none())
      .child(svg(`mods/yuzora-sidebar-database/${icon}.svg`).size(11).flex_none().text_color(color))
      .child(div().flex_1().min_w(0).truncate().text_size(11.5).child(o.name))
      .when(Boolean(meta), (el) => el.child(div().flex_none().max_w(90).truncate().text_size(11).text_color(th.dim).child(meta)));
  }

  click(key, conn, database, rows, rowKey, cx) {
    const r = rows.find((x) => x.id === rowKey);
    if (!r) return;
    if (r.folder !== undefined) {
      const k = `${key}§${r.id}`;
      if (this.expanded.has(k)) this.expanded.delete(k);
      else this.expanded.add(k);
      cx.notify();
      return;
    }
    if (["table", "view", "materialized_view"].includes(r.item.kind)) this.openTable(conn, database, r.item);
  }

  /** The status bar's text for the shown connection. */
  status(conn) {
    let next = null;
    if (conn?.status?.state === "connected") next = { text: `${major(conn.status.version) || conn.label} · ${conn.name}`, tone: "muted" };
    else if (conn?.status?.state === "connecting") next = { text: t("status.connecting", { name: conn.name }), tone: "working" };
    else if (conn?.status?.state === "error") next = { text: t("status.failed", { name: conn.name }), tone: "blocked" };
    const now = shared(STATUS) ?? null;
    if (JSON.stringify(now) !== JSON.stringify(next)) setShared(STATUS, next);
  }

  connRow(c, selected, th) {
    const st = c.status?.state;
    const led = st === "connected"
      ? div().size(7).flex_none().rounded_full().bg(th.working)
      : st === "error"
        ? div().size(7).flex_none().rounded_full().bg(th.blocked)
        : div().size(7).flex_none().rounded_full().border_1().border_color(th.dim);
    const where = c.engine === "sqlite" ? dirname(c.path) : c.instance ? `${c.host}\\${c.instance}` : c.host;
    const line = st === "connected"
      ? `${major(c.status.version) || c.label} · ${t("conn.connected")}`
      : st === "connecting"
        ? `${c.label} · ${t("conn.connecting")}`
        : st === "error"
          ? `${c.label} · ${t("conn.failed")}`
          : `${c.label} · ${where}`;
    const icon = (id, path, tip, action) => div().id(id).size(20).flex().flex_none().items_center().justify_center().rounded(4)
      .text_color(th.dim).hover((s) => s.bg(th.selected).text_color(th.foreground)).tooltip(tip)
      .on_click((_e, cx) => { cx.stop_propagation(); action(); })
      .child(typeof path === "string" && path.endsWith(".svg") ? svg(path).size(12) : div().text_size(10.5).font_semibold().child(path));
    return h_flex().id(`conn-${c.id}`).min_h(38).px(8).py(4).gap(8).items_center().rounded(6)
      .bg(selected ? th.hover : th.chrome).hover((s) => s.bg(th.hover))
      .on_hover((hovered, cx) => { this.hovered = hovered ? c.id : this.hovered === c.id ? null : this.hovered; cx.notify(); })
      .when(st === "error" && Boolean(c.status?.error), (el) => el.tooltip(c.status.error))
      .on_click((_e, cx) => this.select(c, selected ? shared(ACTIVE)?.database ?? null : null, cx))
      .child(led)
      .child(v_flex().flex_1().min_w(0)
        .child(div().font_medium().truncate().child(c.name))
        .child(div().text_size(11).truncate().text_color(st === "error" ? th.blocked : th.dim).child(line)))
      .when(this.hovered === c.id || selected, (el) => el.child(h_flex().gap(2).flex_none()
        .child(icon(`sql-${c.id}`, "SQL", t("conn.query"), () => this.openQuery(c, selected ? shared(ACTIVE)?.database ?? null : null)))
        .child(icon(`edit-${c.id}`, "mods/yuzora-sidebar-database/edit.svg", t("conn.edit"), () => this.openDialog(c)))));
  }

  render(cx) {
    const th = theme();
    const tk = tokens();
    const list = this.conns();
    const { conn, database } = this.active();
    this.status(conn);
    const head = h_flex().h(32).px(12).flex_none().items_center()
      .child(div().flex_1().font_semibold().child(t("connections")))
      .child(div().id("add").size(22).flex().items_center().justify_center().rounded(5).text_color(th.muted)
        .hover((s) => s.bg(th.hover).text_color(th.foreground)).tooltip(t("conn.add"))
        .on_click(() => this.openDialog(null)).child(svg("mods/yuzora-sidebar-database/plus.svg").size(14)));
    const connList = v_flex().gap(2).px(6).pb(8).flex_none()
      .when(list.length === 0, (el) => el.child(v_flex().px(8).py(10).gap(6).text_size(11.5).text_color(th.dim)
        .child(div().child(t("empty")))
        .child(div().id("empty-add").text_color(th.foreground).cursor_pointer().hover((s) => s.underline())
          .on_click(() => this.openDialog(null)).child(t("conn.add")))))
      .children(list.map((c) => this.connRow(c, conn?.id === c.id, th)));
    const root = v_flex().id("root").size_full().bg(th.chrome).text_color(th.foreground).text_size(12.5)
      .child(head).child(connList);
    if (!conn) return root;

    const key = this.treeKey(conn.id, database);
    const tree = this.trees[key];
    const dbs = this.dbs[conn.id] ?? [];
    const dbLabel = database ?? conn.database ?? (dbs.length === 1 ? dbs[0] : "") ?? "";
    const picker = v_flex().px(8).pt(8).pb(4).flex_none().gap(4)
      .child(h_flex().gap(4)
        .child(h_flex().id("db-picker").flex_1().min_w(0).h(28).px(8).gap(6).items_center().rounded(6).border_1().border_color(th.border)
          .bg(th.popover).text_size(11.5).cursor_pointer().hover((s) => s.border_color(th.input))
          .on_click((_e, cx) => { this.dbOpen = !this.dbOpen; cx.notify(); })
          .child(div().flex_1().min_w(0).truncate().child(dbLabel || t("db.default")))
          .child(div().text_color(th.dim).child("▾")))
        .child(div().id("refresh").size(28).flex().flex_none().items_center().justify_center().rounded(6).text_color(th.dim)
          .hover((s) => s.bg(th.hover).text_color(th.foreground)).tooltip(t("refresh"))
          .on_click((_e, cx) => cx.spawn(async (cx) => { await this.load(conn.id, database, cx, true); }))
          .child(svg("mods/yuzora-sidebar-database/refresh.svg").size(12))))
      .when(this.dbOpen, (el) => el.child(v_flex().id("db-list").max_h(220).overflow_y_scrollbar().py(4).rounded(6).border_1().border_color(th.border).bg(th.popover)
        .when(dbs.length === 0, (el) => el.child(div().px(10).py(4).text_size(11.5).text_color(th.dim).child(t("db.none"))))
        .children(dbs.map((d) => h_flex().id(`db-${d}`).h(24).px(10).items_center().text_size(11.5).cursor_pointer()
          .when(d === dbLabel, (el) => el.bg(th.selected))
          .hover((s) => s.bg(th.hover)).on_click((_e, cx) => this.select(conn, d, cx)).child(d)))));

    let body;
    if (!tree || tree.loading) body = div().px(14).py(8).text_size(11.5).text_color(th.dim).child(t("loading"));
    else if (tree.error) body = div().px(14).py(8).text_size(11.5).text_color(th.blocked).child(tree.error);
    else if (!tree.items.length) body = div().px(14).py(8).text_size(11.5).text_color(th.dim).child(t("tree.empty"));
    else {
      const hasSchemas = tree.items.some((o) => o.schema !== "");
      const rows = this.rows(key, tree.items, hasSchemas);
      body = uniform_list("tree", rows.length, (i) => rows[i].id, (range) => {
        const out = [];
        for (let i = range.start; i < range.end; i++) out.push(this.row(rows[i], th, tk));
        return out;
      }).flex_1().min_h(0).px(6).on_item_click((rowKey, cx) => this.click(key, conn, database, rows, rowKey, cx));
    }
    return root
      .child(div().h(1).flex_none().bg(th.divider ?? th.border))
      .child(picker)
      .child(v_flex().flex_1().min_h(0).py(2).child(body));
  }
}
