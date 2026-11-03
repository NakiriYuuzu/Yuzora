// Git Log: the commit graph with its filters, a commit's details, a branch
// compare, and the right-click menu. Owned by lane L2; git runs in yuzora:git.
// A tool pane with props { cwd, on, filter?, compare?, title? }. The same class
// draws the context menu's popup (props.menu), so a right-click needs no mod of its own.
// Rows of a virtual list take no handlers (the list's on_item_click does), and
// the graph is a native cell per visible row, so render touches visible rows only.
import { View, div } from "gpui-kit";
import { v_flex, h_flex, InputState, Input, TextareaState, Textarea, v_virtual_list, VirtualListScrollHandle, Scrollbar } from "gpui-base";
import { spaces, panes, focused, theme, settings, run, props, openOverlay, closeOverlay, notify, setShared } from "herdr";
import { t, tokens, getSetting } from "yuzora:core";
import { open, branches, logOpen, logInfo, logRows, logWait, logFind, logRefresh, logClose, commitDetails, compare, GraphCell, checkout, branchCreate, tagCreate, cherryPick, revert, reset, undoCommit, rewordInMemory, patchCreate, settings as gitSettings } from "yuzora:git";

const ROW = 26, LANE = 16, FILE = 24, MENU = 28, MENU_HEAD = 34;
// Filter boxes after the branch one: [field, width, applied while typing].
const FILTERS = [["user", 110, true], ["since", 96, false], ["until", 96, false], ["path", 130, true], ["hash", 84, false], ["range", 110, false]];
const SIGNATURE = { G: "ok", U: "warn", X: "warn", Y: "warn", R: "bad", B: "bad", E: "warn", N: "dim" };
const RESET = [{ id: "soft" }, { id: "mixed" }, { id: "hard", danger: true }, { id: "keep" }];

const msg = (e) => String(e?.message ?? e).replace(/^Error: /, "");
const list = (v) => (Array.isArray(v) ? v : v ? [v] : []);
// Colors reach the native graph cell as strings it parses itself.
const hex = (c) => (typeof c === "string" && /^#[0-9a-f]{6}([0-9a-f]{2})?$/i.test(c) ? c : undefined);
const tint = (c, a) => (/^#[0-9a-f]{6}$/i.test(c ?? "") ? c + a : c);
const pad = (n) => String(n).padStart(2, "0");

function stamp(s) {
  const d = new Date(s * 1000);
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

function ago(s) {
  const d = Math.max(0, Date.now() / 1000 - s);
  if (d < 60) return t("ago.now");
  if (d < 3600) return t("ago.m", { n: Math.round(d / 60) });
  if (d < 86400) return t("ago.h", { n: Math.round(d / 3600) });
  if (d < 86400 * 30) return t("ago.d", { n: Math.round(d / 86400) });
  if (d < 86400 * 365) return t("ago.mo", { n: Math.round(d / 86400 / 30) });
  return t("ago.y", { n: Math.round(d / 86400 / 365) });
}

// A pane another mod draws: its spec goes in shared state, then the pane is
// focused if open, else made.
function openPane(key, mod, name, fields) {
  const spec = { ...fields, at: Date.now() };
  setShared(key, spec);
  const pane = panes().find((p) => p.mod === mod);
  if (pane) run("pane.focus", { pane: pane.pane });
  else run("tab.new", { name, mod, props: spec });
}

// A closed log's waiting loop would sit until the next ref change, then re-read the
// whole log for nobody; refreshing first wakes it, and the close cancels that read.
function dropLog(id) {
  try { logRefresh(id); } catch {}
  logClose(id);
}

function openDiff(repo, file, how) {
  showDiff({ cwd: repo.root, on: repo.machine, path: file.path, orig: file.orig ?? null, ...how, at: Date.now() });
}

function openMerge(repo) {
  openPane("yuzora.git.merge", "yuzora-git-merge", t("tab.merge"), { cwd: repo.root, on: repo.machine, path: null });
}

// The context menu's rows, in groups (a rule between groups). Also sizes the popup.
function menuItems(m) {
  const there = list(m.refs).filter((x) => x.kind === "local" || x.kind === "remote").slice(0, 3).map((x) => ({ id: `co:${x.name}`, label: t("menu.checkoutRef", { name: x.name }) }));
  const groups = [
    [{ id: "copy" }, ...there, { id: "checkout" }, { id: "branch" }, { id: "tag" }],
    [{ id: "cherry" }, { id: "revert" }, { id: "reset" }, ...(m.head ? [{ id: "undo" }] : []), { id: "reword" }, { id: "rebase" }],
    [{ id: "compare" }, { id: "patch" }],
  ];
  const items = groups.flatMap((g, gi) => g.map((it, i) => ({ ...it, label: it.label ?? t(`menu.${it.id}`), sep: gi > 0 && i === 0 })));
  return { items, height: MENU_HEAD + items.length * MENU + (groups.length - 1) * 9 + 14 };
}

export default class GitLog extends View {
  init(_props, cx) {
    this.p = props() ?? _props ?? {};
    this.handle = cx.focus_handle();
    if (this.p.menu) this.initMenu();
    else this.initLog(cx);
  }

  render(cx) {
    return this.p.menu ? this.renderMenu(cx) : this.renderLog(cx);
  }

  async close() {
    if (this.id != null) dropLog(this.id);
  }

  // ---------------------------------------------------------------- the pane

  initLog(cx) {
    const f = this.p.filter ?? {};
    this.repo = null; this.id = null; this.error = null; this.sel = null; this.detail = null; this.showDetail = false;
    this.w = 900; this.vis = { start: 0, end: 0 }; this.rows0 = -1; this.applied = "{}";
    this.typing = new Set(); this.dd = false; this.ddQuery = ""; this.bl = null;
    this.regex = Boolean(f.regex); this.matchCase = Boolean(f.matchCase); this.noMerges = Boolean(f.noMerges); this.follow = Boolean(f.follow);
    this.showTags = getSetting("git")?.logShowTags !== false;
    this.showRemotes = getSetting("git")?.logShowRemotes !== false;
    this.scroll = VirtualListScrollHandle.new();
    this.cmp = null;
    if (!this.p.compare) {
      const v = { branch: list(f.branches).join(" "), text: f.text ?? "", user: list(f.users)[0] ?? "", since: f.since ?? "", until: f.until ?? "", path: list(f.paths).join(", "), hash: list(f.hashes).join(" "), range: f.range ?? "" };
      this.path0 = v.path;
      this.fields = {};
      for (const k of Object.keys(v)) this.fields[k] = this.field(k, v[k]);
    }
    const space = spaces().find((s) => s.active);
    const cwd = this.p.cwd || space?.folder || null, on = this.p.cwd ? this.p.on ?? null : space?.machine ?? null;
    if (!cwd) return;
    cx.spawn(async (cx) => {
      try {
        this.repo = await open(cwd, on);
        if (this.p.compare) await this.loadCompare();
        else this.openLog(cx);
      } catch (e) {
        this.error = msg(e).includes("not a git repository") ? t("notRepo") : msg(e);
      }
      cx.notify();
    });
  }

  // One filter box. Typing in some applies after a pause; Enter and leaving apply all.
  field(name, value) {
    const s = InputState.new({ placeholder: t(`filter.${name}`), value });
    s.on("change", (_e, cx) => {
      if (name === "branch") {
        if (s.value() === this.picked) return;
        this.ddQuery = s.value();
        if (!this.dd) { this.dd = true; this.loadBranches(cx); }
        cx.notify();
      } else if (name === "text" || FILTERS.some(([k, , live]) => k === name && live)) {
        this.pending?.cancel();
        this.pending = cx.timer.after(450, (cx) => this.apply(cx));
      }
    });
    s.on("submit", (_e, cx) => { this.dd = false; this.handle.focus(); this.apply(cx); cx.notify(); });
    s.on("focus", () => this.typing.add(name));
    s.on("blur", (_e, cx) => { this.typing.delete(name); this.apply(cx); });
    return s;
  }

  filterObject() {
    const v = (k) => this.fields[k].value().trim(), words = (s) => s.split(/[\s,]+/).filter(Boolean);
    const o = {};
    if (v("branch")) o.branches = words(v("branch"));
    if (v("user")) o.users = [v("user")];
    if (v("since")) o.since = v("since");
    if (v("until")) o.until = v("until");
    if (v("path")) {
      o.paths = v("path").split(/\s*,\s*/).filter(Boolean);
      if (this.follow && v("path") === this.path0) o.follow = true;
    }
    if (v("text")) {
      o.text = v("text");
      if (this.regex) o.regex = true;
      if (this.matchCase) o.matchCase = true;
    }
    if (v("hash")) o.hashes = words(v("hash"));
    if (v("range")) o.range = v("range");
    if (this.noMerges) o.noMerges = true;
    return o;
  }

  // A changed filter closes the log and opens another.
  apply(cx) {
    this.pending?.cancel(); this.pending = null;
    if (!this.repo || this.p.compare || JSON.stringify(this.filterObject()) === this.applied) return;
    this.openLog(cx);
  }

  openLog(cx) {
    const filter = this.filterObject();
    let next;
    try { next = logOpen(this.repo.id, filter); } catch (e) { this.error = msg(e); cx.notify(); return; }
    if (this.id != null) dropLog(this.id);
    this.id = next; this.applied = JSON.stringify(filter); this.sel = null; this.detail = null; this.rows0 = -1; this.error = null;
    try { this.scroll.scroll_to_item(0); } catch {}
    cx.spawn(async (cx) => {
      let gen = 0;
      while (this.id === next) {
        try { gen = await logWait(next, gen); } catch { return; }
        if (this.id !== next) return;
        this.keepSelection(next);
        cx.notify();
      }
    });
    cx.notify();
  }

  // When the rows change under a refresh, the selected commit may sit elsewhere.
  keepSelection(id) {
    const rows = logInfo(id).rows;
    if (rows === this.rows0) return;
    this.rows0 = rows;
    if (!this.sel) return;
    const row = logFind(id, this.sel.hash);
    this.sel = row == null ? null : { ...this.sel, row };
  }

  async loadCompare() {
    const { a, b } = this.p.compare;
    this.cmp = await compare(this.repo.id, a, b);
  }

  loadBranches(cx) {
    if (this.bl || !this.repo) return;
    this.bl = [];
    const repo = this.repo;
    cx.spawn(async (cx) => {
      try {
        const b = await branches(repo.id);
        this.bl = [{ name: "HEAD", kind: "head" }, ...b.local.map((x) => ({ name: x.name, kind: x.current ? "head" : "local" })), ...b.remote.map((x) => ({ name: x.name, kind: "remote" })), ...b.tags.map((x) => ({ name: x.name, kind: "tag" }))];
      } catch { this.bl = null; }
      cx.notify();
    });
  }

  ddItems() {
    const q = this.ddQuery.trim().toLowerCase();
    const all = [{ name: "", kind: "all" }, ...(this.bl ?? [])];
    return q ? all.filter((x) => x.name && x.name.toLowerCase().includes(q)) : all;
  }

  pickBranch(name, cx) {
    this.picked = name;
    this.fields.branch.set_value(name);
    this.dd = false;
    this.apply(cx);
    cx.notify();
  }

  clearFilters(cx) {
    this.picked = "";
    for (const f of Object.values(this.fields)) f.set_value("");
    this.regex = false; this.matchCase = false; this.noMerges = false; this.follow = false; this.dd = false;
    this.apply(cx);
    cx.notify();
  }

  // ------------------------------------------------------------- selection

  select(i, cx, reveal) {
    const rows = logInfo(this.id).rows;
    i = Math.max(0, Math.min(rows - 1, i));
    const r = rows ? logRows(this.id, i, i + 1)[0] : null;
    if (!r) return;
    this.sel = { row: i, hash: r.hash };
    if (reveal) this.reveal(i);
    if (this.showDetail) this.loadDetail(r.hash, cx);
    cx.notify();
  }

  // Scrolls by as little as will do: the list's own "top" would jump on every key.
  reveal(i) {
    const { start, end } = this.vis, span = Math.max(1, end - start);
    if (i < start) this.scroll.scroll_to_item(i);
    else if (i >= end - 1) this.scroll.scroll_to_item(Math.max(0, i - span + 2));
  }

  pick(hash, cx) {
    this.sel = { ...(this.sel ?? {}), hash };
    this.showDetail = true;
    this.loadDetail(hash, cx);
    cx.notify();
  }

  loadDetail(hash, cx) {
    if (this.detail?.hash === hash) return;
    this.detail = { hash, data: this.detail?.data ?? null, error: null };
    const repo = this.repo;
    cx.spawn(async (cx) => {
      let data = null, error = null;
      try { data = await commitDetails(repo.id, hash); } catch (e) { error = msg(e); }
      if (this.detail?.hash === hash) { this.detail = { hash, data, error }; cx.notify(); }
    });
  }

  goto(hash, cx) {
    const row = this.id == null ? null : logFind(this.id, hash);
    if (row != null) this.select(row, cx, true);
  }

  key(e, cx) {
    const mod = e.modifiers.platform || e.modifiers.control;
    if (mod && e.key === "f" && this.fields) { this.fields.text.focus(); cx.stop_propagation(); return; }
    if (this.typing.size) {
      if (e.key === "escape") { this.dd = false; this.handle.focus(); cx.stop_propagation(); cx.notify(); }
      return;
    }
    if (this.id == null) return;
    const at = this.sel?.row ?? -1, span = Math.max(1, this.vis.end - this.vis.start - 1);
    const rows = logInfo(this.id).rows;
    if (e.key === "down") this.select(at + 1, cx, true);
    else if (e.key === "up") this.select(at < 0 ? 0 : at - 1, cx, true);
    else if (e.key === "pagedown") this.select(at + span, cx, true);
    else if (e.key === "pageup") this.select(Math.max(0, at - span), cx, true);
    else if (e.key === "home") this.select(0, cx, true);
    else if (e.key === "end") this.select(rows - 1, cx, true);
    else if (e.key === "enter" && this.sel) { this.showDetail = true; this.loadDetail(this.sel.hash, cx); cx.notify(); }
    else return;
    cx.stop_propagation();
  }

  contextMenu(i, cx) {
    const r = logRows(this.id, i, i + 1)[0];
    if (!r) return;
    this.select(i, cx, false);
    const menu = { cwd: this.repo.root, on: this.repo.machine, hash: r.hash, subject: r.subject, refs: r.refs, head: r.head };
    openOverlay({ mod: "yuzora-git-log", kind: "popup", at: "pointer", width: 320, height: menuItems(menu).height, props: { menu } });
  }

  // ----------------------------------------------------------------- pieces

  tag(x, th) {
    const c = x.kind === "head" ? th.accent : x.kind === "remote" ? th.ansi4 : x.kind === "tag" ? th.blocked : th.ansi2;
    return div().flex_none().max_w(150).min_w(0).h(18).px(6).flex().items_center().rounded(4).text_size(10.5)
      .when(x.kind === "head", (el) => el.bg(th.accent).text_color(th.accent_foreground ?? "#ffffff"))
      .when(x.kind !== "head", (el) => el.bg(tint(c, "26")).text_color(c).border_1().border_color(tint(c, "55")))
      .child(div().min_w(0).truncate().child(x.name));
  }

  refsOf(r) {
    const refs = r.refs.filter((x) => (this.showTags || x.kind !== "tag") && (this.showRemotes || x.kind !== "remote"));
    // A detached HEAD has no branch label of its own.
    return r.head && !refs.some((x) => x.kind === "head") ? [{ name: "HEAD", kind: "head" }, ...refs] : refs;
  }

  logRow(r, info, th, mono) {
    const on = this.sel?.row === r.row, bg = on ? th.selected : th.background;
    const refs = this.refsOf(r), narrow = this.w < 520 ? 8 : 20;
    return h_flex().w_full().h(ROW).px(6).gap(6).items_center().bg(bg).text_size(12)
      .when(info.graph, (el) => el.child(div().flex_none().w(Math.min(info.width, narrow) * LANE).h(ROW).overflow_hidden()
        .child(GraphCell.new(`g-${r.row}`, { log: info.id, row: r.row, height: ROW, width: info.width * LANE, head: r.head, ...(hex(bg) ? { background: hex(bg) } : {}) }))))
      .child(h_flex().flex_1().min_w(0).gap(5).items_center()
        .children(refs.slice(0, 3).map((x) => this.tag(x, th)))
        .when(refs.length > 3, (el) => el.child(div().flex_none().text_size(10.5).text_color(th.dim).child(`+${refs.length - 3}`)))
        .child(r.subject == null
          ? div().h(8).w(80 + (r.row % 7) * 28).flex_none().rounded(3).bg(th.raised)
          : div().flex_1().min_w(0).truncate().child(r.subject)))
      .when(this.w >= 640, (el) => el.child(div().w(110).flex_none().truncate().text_size(11.5).text_color(th.muted).child(r.author ?? "")))
      .when(this.w >= 480, (el) => el.child(div().w(72).flex_none().text_right().text_size(11).text_color(th.dim).child(r.time == null ? "" : ago(r.time))))
      .child(div().w(60).flex_none().text_size(11).text_color(th.dim).font_family(mono).child(r.short));
  }

  field_box(name, w, th, ...more) {
    return h_flex().id(`f-${name}`).h(26).w(w).flex_none().px(8).gap(4).items_center().rounded(6).border_1().text_size(11.5).cursor_text()
      .border_color(this.typing.has(name) ? th.accent : th.border).bg(th.background)
      .on_mouse_down("left", (_e, cx) => cx.stop_propagation())
      .child(Input.new(this.fields[name]).flex_1().min_w(0))
      .children(more);
  }

  toggle(id, label, on, th, tip, flip) {
    return div().id(id).h(26).px(8).flex().flex_none().items_center().rounded(6).border_1().text_size(11).cursor_pointer()
      .border_color(on ? th.accent : th.border).bg(on ? tint(th.accent, "26") : th.background).text_color(on ? th.foreground : th.muted)
      .hover((s) => s.text_color(th.foreground)).tooltip(tip)
      .on_click((_e, cx) => { flip(); this.apply(cx); cx.notify(); }).child(label);
  }

  toolbar(th) {
    const down = div().id("dd-toggle").flex_none().w(14).h(20).flex().items_center().justify_center().text_size(9).text_color(th.dim).cursor_pointer()
      .hover((s) => s.text_color(th.foreground))
      .on_click((_e, cx) => { this.dd = !this.dd; this.ddQuery = ""; if (this.dd) { this.bl = null; this.loadBranches(cx); } cx.notify(); })
      .child(this.dd ? "▴" : "▾");
    return h_flex().flex_none().flex_wrap().gap(6).px(8).py(6).items_center().border_b(1).border_color(th.divider ?? th.border)
      .child(this.field_box("branch", 170, th, down))
      .children(FILTERS.map(([name, w]) => this.field_box(name, w, th)))
      .child(this.field_box("text", 190, th))
      .child(this.toggle("t-regex", ".*", this.regex, th, t("filter.regex"), () => { this.regex = !this.regex; }))
      .child(this.toggle("t-case", "Aa", this.matchCase, th, t("filter.case"), () => { this.matchCase = !this.matchCase; }))
      .child(this.toggle("t-merges", t("filter.noMerges"), this.noMerges, th, t("filter.noMergesTip"), () => { this.noMerges = !this.noMerges; }))
      .child(div().id("clear").h(26).px(8).flex().items_center().rounded(6).text_size(11).text_color(th.muted).cursor_pointer().hover((s) => s.text_color(th.foreground).bg(th.hover))
        .on_click((_e, cx) => this.clearFilters(cx)).child(t("filter.clear")))
      .child(div().id("refresh").h(26).w(26).flex().items_center().justify_center().rounded(6).text_size(13).text_color(th.muted).cursor_pointer().hover((s) => s.text_color(th.foreground).bg(th.hover))
        .tooltip(t("refresh")).on_click((_e, cx) => { if (this.id != null) logRefresh(this.id); cx.notify(); }).child("↻"));
  }

  dropdown(th) {
    const items = this.ddItems();
    const color = (k) => (k === "head" ? th.accent : k === "remote" ? th.ansi4 : k === "tag" ? th.blocked : th.ansi2);
    const key = (x) => `${x.kind}:${x.name}`;
    return div().flex_none().h(Math.min(220, items.length * FILE + 8)).mx(8).mb(6).relative().rounded(6).border_1().border_color(th.border).bg(th.popover ?? th.raised)
      .child(v_virtual_list("branch-dd", items.length, FILE, (i) => key(items[i]), (range) => items.slice(range.start, range.end).map((x) =>
        h_flex().w_full().h(FILE).px(8).gap(6).items_center().text_size(11.5).hover((s) => s.bg(th.hover))
          .child(div().flex_1().min_w(0).truncate().text_color(x.kind === "all" ? th.muted : th.foreground).child(x.kind === "all" ? t("filter.allBranches") : x.name))
          .when(x.kind !== "all", (el) => el.child(div().flex_none().text_size(10).text_color(color(x.kind)).child(t(`ref.${x.kind}`)))))).size_full()
        .on_item_click((k, cx) => { const x = this.ddItems().find((i) => key(i) === k); if (x) this.pickBranch(x.name, cx); }))
      .child(Scrollbar.vertical("branch-dd").viewport_from_layout().absolute().inset_0());
  }

  footer(th, parts, note) {
    return h_flex().h(24).flex_none().px(10).gap(10).items_center().border_t(1).border_color(th.divider ?? th.border).text_size(11).text_color(th.dim)
      .child(div().flex_1().min_w(0).truncate().child(parts.join(" · ")))
      .when(Boolean(note), (el) => el.child(div().flex_none().text_color(th.blocked).child(note)));
  }

  // One row of a changed-files list: kind, name, folder (or the old name of a rename).
  fileRow(f, th, fs, on) {
    const parts = f.path.split("/"), name = parts.pop(), dir = f.orig ? `← ${f.orig}` : parts.join("/");
    const c = f.kind === "A" ? fs.added ?? th.ansi2 : f.kind === "D" ? fs.deleted ?? th.ansi1 : fs.modified ?? th.blocked;
    return h_flex().w_full().h(FILE).px(8).gap(6).items_center().text_size(11.5).rounded(5).hover((s) => s.bg(th.hover)).when(on, (el) => el.bg(th.selected))
      .child(div().w(12).flex_none().text_size(11).font_semibold().text_color(c).child(f.kind))
      .child(h_flex().flex_1().min_w(0).gap(6).child(div().flex_none().max_w(180).truncate().child(name))
        .when(Boolean(dir), (el) => el.child(div().flex_1().min_w(0).truncate().text_size(10.5).text_color(th.dim).child(dir))));
  }

  filesList(id, files, th, fs, open_) {
    return div().flex_1().min_h(60).relative().px(4)
      .child(v_virtual_list(id, files.length, FILE, (i) => String(i), (range) => files.slice(range.start, range.end).map((f) => this.fileRow(f, th, fs, false))).size_full()
        .on_item_click((key, cx) => { const f = files[Number(key)]; if (f) open_(f); }))
      .child(Scrollbar.vertical(id).viewport_from_layout().absolute().inset_0());
  }

  meta(label, th, ...value) {
    return h_flex().gap(8).items_start().child(div().w(64).flex_none().text_color(th.dim).child(label)).child(h_flex().flex_1().min_w(0).flex_wrap().gap(6).children(value));
  }

  detailPanel(th, side, mono, fs) {
    const d = this.detail?.data, shell = (body) => v_flex().flex_none().bg(th.chrome ?? th.background).border_color(th.divider ?? th.border)
      .when(side, (el) => el.w(340).h_full().border_l(1)).when(!side, (el) => el.h(260).w_full().border_t(1)).text_size(11.5)
      .child(h_flex().h(28).flex_none().px(10).items_center().child(div().flex_1().font_semibold().child(t("detail.title")))
        .child(div().id("detail-close").w(20).h(20).flex().items_center().justify_center().rounded(4).text_color(th.dim).cursor_pointer().hover((s) => s.bg(th.hover).text_color(th.foreground))
          .on_click((_e, cx) => { this.showDetail = false; cx.notify(); }).child("×")))
      .child(body);
    if (!d) return shell(div().px(10).text_color(this.detail?.error ? th.ansi1 : th.dim).child(this.detail?.error ?? t("loading")));
    const lines = d.body ? d.body.split("\n").slice(0, 80) : [];
    const sig = SIGNATURE[d.signature] ?? "warn", sigColor = sig === "ok" ? tokens().diff?.added ?? th.ansi2 : sig === "bad" ? th.ansi1 : sig === "warn" ? th.blocked : th.dim;
    const person = (name, email, time) => [div().child(`${name} <${email}>`), div().text_color(th.dim).child(stamp(time))];
    const link = d.parents.length > 0 && this.id != null;
    const info = v_flex().id("detail-info").flex_none().max_h(side ? 300 : 110).overflow_y_scrollbar().px(10).pb(6).gap(6)
      .child(div().font_semibold().text_size(12.5).child(d.subject))
      .when(lines.length > 0, (el) => el.child(v_flex().text_color(th.muted).children(lines.map((l) => (l.trim() ? div().child(l) : div().h(8))))))
      .child(this.meta(t("detail.hash"), th, div().id("detail-hash").font_family(mono).cursor_pointer().hover((s) => s.text_color(th.accent)).tooltip(t("detail.copy"))
        .on_click((_e, cx) => cx.write_to_clipboard(d.hash)).child(d.hash)))
      .child(this.meta(t("detail.author"), th, ...person(d.author, d.email, d.authorTime)))
      .when(d.committer !== d.author || Math.abs(d.commitTime - d.authorTime) > 60, (el) => el.child(this.meta(t("detail.committer"), th, ...person(d.committer, d.committerEmail, d.commitTime))))
      .child(this.meta(t("detail.parents"), th, ...(d.parents.length ? d.parents.map((p) => div().id(`parent-${p}`).font_family(mono)
        .text_color(link ? th.accent : th.muted).when(link, (el) => el.cursor_pointer().hover((s) => s.opacity(0.8)).on_click((_e, cx) => this.goto(p, cx))).child(p.slice(0, 8))) : [div().text_color(th.dim).child(t("detail.root"))])))
      .child(this.meta(t("detail.signature"), th, div().text_color(sigColor).child(SIGNATURE[d.signature] ? t(`sig.${d.signature}`) : d.signature || t("sig.N")),
        ...(d.signer ? [div().text_color(th.dim).child(d.signer)] : [])));
    const how = { mode: "commit", rev: d.hash };
    return shell(v_flex().flex_1().min_h(0)
      .child(info)
      .child(div().h(24).flex_none().px(10).flex().items_center().text_color(th.dim).child(t("detail.files", { count: d.files.length })))
      .child(this.filesList("detail-files", d.files, th, fs, (f) => openDiff(this.repo, f, how))));
  }

  // ---------------------------------------------------------------- the view

  renderLog(cx) {
    const th = theme();
    if (!this.repo) {
      return v_flex().id("git-log").size_full().bg(th.background).text_color(th.foreground).text_size(12.5).p(16)
        .child(div().text_color(this.error ? th.ansi1 : th.dim).child(this.error ?? (this.p.cwd || spaces().some((s) => s.active && s.folder) ? t("loading") : t("hint"))));
    }
    if (this.p.compare) return this.renderCompare(th);
    const info = this.id == null ? null : logInfo(this.id), mono = settings().font_family ?? "JetBrains Mono", fs = tokens().file_status ?? {};
    const side = this.w >= 760, detail = this.showDetail && this.sel;
    const parts = info ? [t("footer.commits", { count: info.rows })] : [];
    if (info?.firstMs != null) parts.push(t("footer.first", { ms: info.firstMs }));
    if (info?.fullMs != null) parts.push(t("footer.full", { ms: info.fullMs }));
    const rows = !info ? div().flex_1().p(12).text_color(this.error ? th.ansi1 : th.dim).child(this.error ?? t("loading")) : div().flex_1().min_w(0).min_h(0).relative()
      .child(v_virtual_list("log-rows", info.rows, ROW, (i) => String(i), (range) => {
        this.vis = range;
        return logRows(info.id, range.start, range.end).map((r) => this.logRow(r, info, th, mono));
      }).size_full().track_scroll(this.scroll)
        .on_item_click((key, cx) => { this.handle.focus(); this.showDetail = true; this.select(Number(key), cx, false); })
        .on_item_secondary_click((key, _e, cx) => this.contextMenu(Number(key), cx)))
      .child(Scrollbar.vertical("log-rows").viewport_from_layout().absolute().inset_0())
      .when(info.rows === 0, (el) => el.child(div().absolute().top(16).left(0).right(0).px(12).text_center().text_size(11.5).text_color(info.error ? th.ansi1 : th.dim)
        .child(info.error ?? (info.loading ? t("loading") : t("empty")))));
    return v_flex().id("git-log").size_full().min_h(0).bg(th.background).text_color(th.foreground).text_size(12).track_focus(this.handle)
      .on_key_down((e, cx) => this.key(e, cx))
      .on_size_change((size, cx) => { const cls = (w) => (w < 480) + (w < 640) + (w < 760); if (cls(size.width) !== cls(this.w)) { this.w = size.width; cx.notify(); } })
      .when(Boolean(this.p.title), (el) => el.child(div().h(28).flex_none().px(10).flex().items_center().font_semibold().truncate().child(this.p.title)))
      .child(this.toolbar(th))
      .when(this.dd, (el) => el.child(this.dropdown(th)))
      .child((side ? h_flex().items_stretch() : v_flex()).flex_1().min_h(0)
        .child(rows)
        .when(Boolean(detail), (el) => el.child(this.detailPanel(th, side, mono, fs))))
      .child(this.footer(th, parts, info?.loading ? t(`stage.${info.stage || "start"}`) : null));
  }

  cmpRow(c, th, mono) {
    const on = this.sel?.hash === c.hash;
    return h_flex().w_full().h(FILE).px(8).gap(8).items_center().text_size(11.5).rounded(5).hover((s) => s.bg(th.hover)).when(on, (el) => el.bg(th.selected))
      .child(div().w(58).flex_none().text_size(11).text_color(th.dim).font_family(mono).child(c.short))
      .child(div().flex_1().min_w(0).truncate().child(c.subject))
      .child(div().flex_none().text_size(11).text_color(th.dim).child(ago(c.time)));
  }

  renderCompare(th) {
    const { a, b } = this.p.compare, c = this.cmp, mono = settings().font_family ?? "JetBrains Mono", fs = tokens().file_status ?? {};
    const side = this.w >= 760, detail = this.showDetail && this.sel;
    if (!c) return v_flex().id("git-log").size_full().bg(th.background).text_color(th.foreground).text_size(12.5).p(16).child(div().text_color(this.error ? th.ansi1 : th.dim).child(this.error ?? t("loading")));
    const column = (id, rev, commits) => v_flex().flex_1().min_w(0).min_h(0)
      .child(div().h(26).flex_none().px(10).flex().items_center().text_size(11.5).text_color(th.muted).child(t("compare.only", { rev, count: commits.length >= 500 ? "500+" : commits.length })))
      .child(div().flex_1().min_h(0).relative().px(4)
        .child(v_virtual_list(id, commits.length, FILE, (i) => commits[i].hash, (range) => commits.slice(range.start, range.end).map((x) => this.cmpRow(x, th, mono))).size_full()
          .on_item_click((key, cx) => { if (commits.some((x) => x.hash === key)) this.pick(key, cx); }))
        .child(Scrollbar.vertical(id).viewport_from_layout().absolute().inset_0())
        .when(commits.length === 0, (el) => el.child(div().absolute().top(8).left(0).right(0).text_center().text_size(11.5).text_color(th.dim).child(t("compare.none")))));
    // The file list is b's changes since the merge base, so diff from there.
    const how = { mode: "compare", rev: c.base ?? a, to: b };
    return v_flex().id("git-log").size_full().min_h(0).bg(th.background).text_color(th.foreground).text_size(12).track_focus(this.handle)
      .on_size_change((size, cx) => { if ((size.width >= 760) !== side) { this.w = size.width; cx.notify(); } })
      .child(h_flex().h(34).flex_none().px(10).gap(8).items_center().border_b(1).border_color(th.divider ?? th.border)
        .child(div().px(8).h(20).flex().items_center().rounded(4).bg(th.raised).font_family(mono).text_size(11).child(a))
        .child(div().text_color(th.dim).child("↔"))
        .child(div().px(8).h(20).flex().items_center().rounded(4).bg(th.raised).font_family(mono).text_size(11).child(b)))
      .child((side ? h_flex().items_stretch() : v_flex()).flex_1().min_h(0)
        .child(v_flex().flex_1().min_w(0).min_h(0)
          .child(h_flex().flex_1().min_h(0).child(column("cmp-a", a, c.onlyA)).child(div().w(1).h_full().flex_none().bg(th.divider ?? th.border)).child(column("cmp-b", b, c.onlyB)))
          .child(div().h(26).flex_none().px(10).flex().items_center().border_t(1).border_color(th.divider ?? th.border).text_size(11.5).text_color(th.muted).child(t("detail.files", { count: c.files.length })))
          .child(div().h(150).flex_none().flex().child(this.filesList("cmp-files", c.files, th, fs, (f) => openDiff(this.repo, f, how)))))
        .when(Boolean(detail), (el) => el.child(this.detailPanel(th, side, mono, fs))))
      .child(this.footer(th, [t("compare.footer", { a: c.onlyA.length, b: c.onlyB.length, files: c.files.length })], null));
  }

  // ------------------------------------------------------- the context menu

  initMenu() {
    this.m = this.p.menu; this.screen = "menu"; this.at = 0; this.busy = false; this.err = null; this.coBranch = true;
    this.handle.focus();
  }

  closeMenu() {
    closeOverlay(this.p.overlay);
  }

  // One git call; its failure stays in the popup, its success closes it.
  act(cx, id, work) {
    if (this.busy) return;
    this.busy = true; this.err = null;
    const m = this.m;
    cx.spawn(async (cx) => {
      try {
        const repo = await open(m.cwd, m.on ?? null);
        const out = await work(repo, cx);
        if (out?.conflicts?.length) {
          openMerge(repo);
          notify({ title: t("conflicts", { count: out.conflicts.length }), level: "warning", timeout: 4000 });
          return this.closeMenu();
        }
        if (out && out.ok === false) throw new Error(out.output || t("failed"));
        notify({ title: t(`done.${id}`), body: out?.wasPushed ? t("undo.pushed") : m.hash.slice(0, 8), level: "success", timeout: 2500 });
        return this.closeMenu();
      } catch (e) { this.err = msg(e); }
      this.busy = false;
      cx.notify();
    });
    cx.notify();
  }

  // An in-place form: a name, a name and a message, or the commit's own message.
  form(cx, kind) {
    this.screen = kind; this.err = null;
    const input = (placeholder) => { const s = InputState.new({ placeholder }); s.on("submit", (_e, cx) => this.submit(cx)); return s; };
    if (kind === "branch") this.first = this.name = input(t("form.branchName"));
    else if (kind === "tag") { this.first = this.name = input(t("form.tagName")); this.note = input(t("form.tagMessage")); }
    else {
      this.first = this.text = TextareaState.new({ rows: 6 });
      this.text.on("submit", (e, cx) => { if (e?.secondary) this.submit(cx); });
      const m = this.m;
      cx.spawn(async (cx) => {
        try {
          const r = await open(m.cwd, m.on ?? null), d = await commitDetails(r.id, m.hash);
          if (this.screen === "reword" && !this.text.value()) this.text.set_value(d.body ? `${d.subject}\n\n${d.body}` : d.subject);
        } catch (e) { this.err = msg(e); }
        cx.notify();
      });
    }
    this.first.focus();
    cx.timer.after(250, () => this.first?.focus());
    cx.notify();
  }

  submit(cx) {
    const m = this.m;
    if (this.screen === "branch") {
      const name = this.name.value().trim();
      if (name) this.act(cx, "branch", (r) => branchCreate(r.id, name, m.hash, this.coBranch));
    } else if (this.screen === "tag") {
      const name = this.name.value().trim(), message = this.note.value().trim();
      if (name) this.act(cx, "tag", (r) => tagCreate(r.id, name, m.hash, message || null));
    } else if (this.screen === "reword") {
      const text = this.text.value();
      if (text.trim()) this.act(cx, "reword", (r) => rewordInMemory(r.id, m.hash, text));
    }
  }

  choose(it, cx) {
    const m = this.m;
    if (this.busy || !it) return;
    if (this.screen === "reset") {
      if (it.id === "hard") { this.screen = "confirm"; cx.notify(); return; }
      return this.act(cx, "reset", (r) => reset(r.id, m.hash, it.id));
    }
    if (it.id.startsWith("co:")) return this.act(cx, "checkout", (r) => checkout(r.id, it.id.slice(3), { smart: true }));
    switch (it.id) {
      case "copy":
        cx.write_to_clipboard(m.hash);
        notify({ title: t("done.copy"), body: m.hash.slice(0, 8), level: "success", timeout: 1500 });
        return this.closeMenu();
      case "checkout": return this.act(cx, "checkout", (r) => checkout(r.id, m.hash, { smart: true }));
      case "branch": case "tag": case "reword": return this.form(cx, it.id);
      case "cherry": return this.act(cx, "cherry", (r) => cherryPick(r.id, [m.hash]));
      case "revert": return this.act(cx, "revert", (r) => revert(r.id, [m.hash]));
      case "reset": this.screen = "reset"; this.at = 1; break;
      case "undo": return this.act(cx, "undo", (r) => undoCommit(r.id));
      case "rebase":
        openPane("yuzora.git.rebase", "yuzora-git-rebase", t("tab.rebase"), { cwd: m.cwd, on: m.on ?? null, base: `${m.hash}^` });
        return this.closeMenu();
      case "compare": {
        const short = m.hash.slice(0, 8);
        run("tab.new", { name: t("tab.compare", { a: "HEAD", b: short }), mod: "yuzora-git-log", props: { cwd: m.cwd, on: m.on ?? null, compare: { a: "HEAD", b: m.hash } } });
        return this.closeMenu();
      }
      case "patch": return this.act(cx, "patch", async (r, cx) => { cx.write_to_clipboard(await patchCreate(r.id, { hashes: [m.hash] })); });
    }
    this.err = null;
    cx.notify();
  }

  menuKey(e, cx) {
    const items = this.screen === "reset" ? RESET : this.screen === "menu" ? menuItems(this.m).items : null;
    if (e.key === "escape") {
      if (this.screen === "menu") { this.closeMenu(); cx.stop_propagation(); return; }
      this.screen = "menu"; this.err = null; this.at = 0;
    } else if (!items) return;
    else if (e.key === "down") this.at = (this.at + 1) % items.length;
    else if (e.key === "up") this.at = (this.at + items.length - 1) % items.length;
    else if (e.key === "enter") this.choose(items[this.at], cx);
    else return;
    cx.stop_propagation();
    cx.notify();
  }

  button(id, label, th, kind, action) {
    return h_flex().id(id).h(28).px(12).items_center().rounded(6).text_size(11.5).cursor_pointer().hover((s) => s.opacity(0.88)).on_click((_e, cx) => action(cx))
      .when(kind === "primary", (el) => el.bg(th.solid).text_color(th.solid_foreground).font_semibold())
      .when(kind === "danger", (el) => el.bg(th.ansi1).text_color(th.background).font_semibold())
      .when(kind === "plain", (el) => el.border_1().border_color(th.border).text_color(th.foreground))
      .child(label);
  }

  inputBox(id, state, th) {
    return div().id(id).px(8).py(5).flex().items_center().rounded(6).border_1().border_color(th.border).bg(th.background).text_size(12).cursor_text()
      .on_mouse_down("left", (_e, cx) => cx.stop_propagation()).child(Input.new(state).w_full());
  }

  menuRow(it, i, th) {
    const tall = this.screen === "reset";
    return v_flex().when(it.sep, (el) => el.child(div().mx(7).my(4).h(1).bg(th.border)))
      .child(h_flex().id(`mi-${i}`).h(tall ? 40 : MENU).px(8).gap(9).rounded(7).items_center().justify_between().cursor_pointer()
        .text_color(it.danger ? th.ansi1 : th.foreground).when(i === this.at, (el) => el.bg(th.selected)).hover((s) => s.bg(th.hover))
        .on_click((_e, cx) => this.choose(it, cx))
        .child(tall
          ? v_flex().child(div().child(t(`reset.${it.id}`))).child(div().text_size(10.5).text_color(th.dim).child(t(`reset.${it.id}.tip`)))
          : div().flex_1().min_w(0).truncate().child(it.label)));
  }

  renderMenu(cx) {
    const th = theme(), m = this.m, s = this.screen, back = (cx) => { this.screen = "menu"; this.err = null; cx.notify(); };
    const title = (text) => div().text_size(12.5).font_semibold().child(text);
    const actions = (label, kind) => h_flex().justify_end().gap(6).child(this.button("m-back", t("cancel"), th, "plain", back)).child(this.button("m-ok", label, th, kind, (cx) => this.submit(cx)));
    let body;
    if (s === "menu" || s === "reset") {
      body = v_flex().px(5).children((s === "menu" ? menuItems(m).items : RESET).map((it, i) => this.menuRow(it, i, th)));
    } else if (s === "confirm") {
      body = v_flex().px(12).gap(10).child(title(t("reset.hard.title")))
        .child(div().text_color(th.muted).child(t("reset.hard.ask", { hash: m.hash.slice(0, 8) })))
        .child(h_flex().justify_end().gap(6).child(this.button("m-back", t("cancel"), th, "plain", back))
          .child(this.button("m-ok", t("reset.hard.button"), th, "danger", (cx) => this.act(cx, "reset", (r) => reset(r.id, m.hash, "hard")))));
    } else if (s === "branch") {
      body = v_flex().px(12).gap(8).child(title(t("menu.branch"))).child(this.inputBox("m-name", this.name, th))
        .child(h_flex().id("m-checkout").gap(6).items_center().text_size(11.5).cursor_pointer().on_click((_e, cx) => { this.coBranch = !this.coBranch; cx.notify(); })
          .child(div().w(12).h(12).flex().items_center().justify_center().rounded(3).border_1().border_color(this.coBranch ? th.accent : th.input ?? th.border)
            .bg(this.coBranch ? th.accent : th.background).text_size(9).text_color(th.background).child(this.coBranch ? "✓" : ""))
          .child(t("form.checkout")))
        .child(actions(t("form.create"), "primary"));
    } else if (s === "tag") {
      body = v_flex().px(12).gap(8).child(title(t("menu.tag"))).child(this.inputBox("m-name", this.name, th)).child(this.inputBox("m-note", this.note, th)).child(actions(t("form.create"), "primary"));
    } else {
      body = v_flex().px(12).gap(8).child(title(t("menu.reword")))
        .child(div().id("m-text").px(8).py(5).rounded(6).border_1().border_color(th.border).bg(th.background).text_size(12).cursor_text()
          .on_mouse_down("left", (_e, cx) => cx.stop_propagation()).child(Textarea.new(this.text).w_full()))
        .child(div().text_size(10.5).text_color(th.dim).child(t("form.rewordTip")))
        .child(actions(t("form.save"), "primary"));
    }
    return v_flex().id("git-log-menu").track_focus(this.handle).on_key_down((e, cx) => this.menuKey(e, cx)).size_full().min_h(0).bg(th.popover).text_color(th.foreground).text_size(12.5)
      .child(h_flex().h(MENU_HEAD).flex_none().px(12).gap(8).items_center()
        .child(div().flex_none().text_size(11).text_color(th.dim).font_family(settings().font_family ?? "JetBrains Mono").child(m.hash.slice(0, 8)))
        .child(div().flex_1().min_w(0).truncate().text_color(th.muted).child(m.subject ?? "")))
      .child(body)
      .when(Boolean(this.err), (el) => el.child(div().id("m-err").max_h(90).overflow_y_scrollbar().mx(12).mt(8).text_size(11).text_color(th.ansi1).child(this.err)))
      .when(this.busy, (el) => el.child(div().px(12).pt(6).text_size(11).text_color(th.dim).child(t("working"))));
  }
}

// A diff pane beside the focused pane, or in a tab of its own (Git settings).
function placeDiff(spec, where) {
  const pane = where !== "tab" && focused()?.pane;
  if (pane) run("pane.split", { direction: "right", pane, mod: "yuzora-git-diff", props: spec });
  else run("tab.new", { name: t("tab.diff"), mod: "yuzora-git-diff", props: spec });
}

// Where a diff opens (Git settings): into the open diff pane, which follows
// the shared request; else a modal, or straight into a pane as Keep would.
function showDiff(spec) {
  const s = gitSettings();
  const pane = s.diffFollowPane !== false && panes().find((p) => p.mod === "yuzora-git-diff");
  if (pane) {
    setShared("yuzora.git.diff", spec);
    return run("pane.focus", { pane: pane.pane });
  }
  if (s.diffOpenIn === "pane") return placeDiff(spec, s.diffKeepIn);
  openOverlay({ mod: "yuzora-git-diff", kind: "dialog", width: 0.9, height: 0.86, props: spec });
}
