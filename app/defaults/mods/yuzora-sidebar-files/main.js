// The sidebar's Files mode: the focused pane's repo (or folder) as a tree with
// git marks, file and content search, and a right-click menu. Owned by lane L5.
// The same mod opens itself as the right-click popup (props().menu).
import { View, div, svg, uniform_list } from "gpui-kit";
import { v_flex, h_flex, InputState, Input, Scrollbar } from "gpui-base";
import { spaces, panes, focused, theme, run, home, props, openOverlay, closeOverlay, shared, setShared, notify, watchFolder, window } from "herdr";
import { t, tokens } from "yuzora:core";
import { listDir, gitStatus, stat, createFile, createFolder, rename, trash, reveal, copyText, findFiles, grep, unsavedCount } from "yuzora:files";
import { openFile, moved, join, parent, base, rel } from "./open.js";
import { fileIcon } from "./icons.js";
import { heading } from "./paths.js";
import { readGit } from "./git-state.js";
import { createStore, panel, info } from "./tree-state.js";

const ME = "yuzora-sidebar-files";
const PER_FOLDER = 2000, ROWS = 20000;
const REQUEST = "yuzora.files.request", REFRESH = "yuzora.files.refresh";
// How long a click waits to learn that the file is still there; after that it opens, and the pane says what is wrong.
const CHECK_MS = 1500;

// What `promise` says, or null when it says nothing within `ms`.
function within(cx, ms, promise) {
  return new Promise(resolve => {
    cx.timer.after(ms, () => resolve(null));
    promise.then(resolve, () => resolve(null));
  });
}

// Where the tree is: the focused pane's folder, on its space's machine. A tool
// pane (no folder of its own) keeps the folder already shown in that space.
function where(prev) {
  const space = spaces().find(s => s.active);
  if (!space) return null;
  const f = focused();
  const pane = f?.pane ? panes().find(p => p.pane === f.pane) : null;
  const on = f && f.pane === null && f.terminal?.includes("/") ? f.terminal.split("/")[0] : space.machine ?? null;
  let cwd = pane?.cwd || (f && f.pane === null ? f.cwd : null);
  if (!cwd && prev && prev.space === space.id && prev.on === on) return prev;
  cwd ||= space.folder || (on ? null : home());
  return cwd ? { space: space.id, cwd, on, key: `${on ?? ""}|${cwd}` } : { space: space.id, cwd: null, on, key: `${on ?? ""}|` };
}

export default class FilesSidebar extends View {
  init(_p, cx) {
    this.menu = props()?.menu ?? null;
    if (this.menu) return;
    this.store = createStore({
      listDir: (root, folder, options) => listDir(folder ? join(root, folder) : root, { ...options, limit: PER_FOLDER }),
      readGit: (cwd, options) => readGit(gitStatus, cwd, options),
      exists: (root, r, on) => stat(r ? join(root, r) : root, { on }).then(found => found !== null),
    });
    this.hidden = localStorage.getItem("hidden") === "1";
    this.rows = []; this.rowsEntry = null; this.rowsVersion = -1; this.version = 0;
    this.searching = false; this.searchMode = "name"; this.results = []; this.searchSeq = 0;
    this.searchInput = InputState.new({ placeholder: t("search.placeholder") });
    this.searchInput.on("change", (_e, cx) => this.search(cx));
    this.editInput = InputState.new({});
    this.editInput.on("submit", (_e, cx) => this.submitEdit(cx));
    this.edit = null;
    this.lastRequest = shared(REQUEST)?.seq ?? 0;
    this.lastRefresh = shared(REFRESH) ?? 0;
    this.loc = null; this.here = null; this.watching = null; this.watchedRoot = null; this.watchSeq = 0;
    // The last file click whose check is not over: files open in the order they were clicked.
    this.opening = null;
    this.relocate(cx);
    this.countUnsaved();
    cx.timer.every(10000, cx => {
      this.reload(cx, true); this.countUnsaved();
      this.store.forget(new Set(spaces().map(s => s.id)));
    });
  }

  // The mode bar's badge: unsaved documents. The editor panes keep it current;
  // this catches what they cannot (a restart, a save that ends after its pane closed).
  countUnsaved() {
    const n = unsavedCount();
    if ((shared("yuzora.badge.files") ?? null) === (n || null)) return;
    setShared("yuzora.badge.files", n || null);
    setShared("yuzora.status.files", n ? { text: t("status.unsaved", { count: n }), tone: "blocked" } : null);
  }

  // Context.

  // The tree of a folder seen before is there from the first frame, and read again behind it.
  relocate(cx) {
    const next = where(this.loc);
    if (next?.key === this.loc?.key && next?.space === this.loc?.space) return;
    this.loc = next;
    this.here = next ? this.store.enter(next) : null;
    this.version++;
    this.watching = null;
    // A name being typed is for the folder it was started in.
    this.edit = null;
    // Results belong to the folder they were found in: the search runs again there.
    this.searchSeq++; this.results = []; this.searchNote = null;
    if (this.here?.cwd) this.reload(cx);
  }

  current(here) {
    return this.here === here;
  }

  // `timed`: the 10 s timer's, which a watched folder doesn't need for search. `here`: the folder to read again,
  // which is the one shown unless a change made in another one has just ended.
  reload(cx, timed = false, here = this.here) {
    if (!here?.cwd) return;
    cx.spawn(async cx => {
      const read = await this.store.reload(here, () => { this.version++; cx.notify(); }, { timed });
      if (!read?.ok || !this.current(here)) return;
      // An open search follows the files too (the new root's, after a move).
      if (this.searching && !(read.timed && this.watching === here)) this.search(cx, !read.timed);
      this.watch(here, cx);
    });
  }

  // Watches are recursive on macOS: the home folder (or /) itself is left to the timer.
  watch(here, cx) {
    const root = here.root;
    if (here.on || (this.watching === here && this.watchedRoot === root) || root === "/" || root === home()) return;
    this.watching = here; this.watchedRoot = root;
    const token = ++this.watchSeq;
    const same = () => this.watching === here && this.watchSeq === token;
    cx.spawn(async cx => {
      try {
        while (same()) {
          await watchFolder(root);
          if (same()) this.reload(cx);
        }
      } catch { if (same()) this.watching = null; }
    });
  }

  // `row`: from the tree on screen; the folder is opened under the root that tree was listed from.
  toggle(row, cx) {
    const here = this.here, loc = this.loc, folder = row.rel, root = here.tree.root;
    if (here.open.has(folder)) {
      for (const f of [...here.open]) if (f === folder || f.startsWith(folder + "/")) here.open.delete(f);
      this.version++; cx.notify();
      return;
    }
    this.version++;
    cx.spawn(async cx => {
      const failed = await this.store.expand(here, root, folder, () => { this.version++; cx.notify(); });
      if (!failed || this.loc !== loc) return;
      if (failed.gone) notify({ title: t("error.gone", { name: row.name }), level: "warning" });
      else notify({ title: t("error.list"), body: String(failed.error?.message ?? failed.error), level: "error" });
    });
    cx.notify();
  }

  collapse(cx) {
    this.here.open.clear(); this.version++; cx.notify();
  }

  // The visible rows, rebuilt only when something they show changed.
  visible() {
    const here = this.here;
    if (this.rowsEntry === here && this.rowsVersion === this.version) return this.rows;
    const folders = here.tree?.folders, root = here.tree?.root;
    const out = [];
    let capped = false;
    const walk = (folder, depth) => {
      for (const e of folders?.get(folder)?.list ?? []) {
        if (e.hidden && !this.hidden) continue;
        // One more row than fits: the tree is cut, and says so.
        if (out.length >= ROWS) { capped = true; return; }
        const r = folder ? `${folder}/${e.name}` : e.name;
        const open = e.dir && here.open.has(r);
        out.push({ rel: r, abs: join(root, r), name: e.name, dir: e.dir, link: e.link, depth, open });
        if (open) walk(r, depth + 1);
      }
    };
    walk("", 0);
    this.rows = out; this.rowsCapped = capped; this.rowsEntry = here; this.rowsVersion = this.version;
    return out;
  }

  mark(row) {
    const git = this.here.git;
    const m = git?.marks[row.abs];
    if (m) return m;
    return row.dir && !row.open && git?.dirMarks.has(row.abs) ? "•" : null;
  }

  // What a click or a right click on `key` is on: a row of the tree on screen, or else a file of the tree that
  // was replaced just before (the frame the click was made on was drawn from it).
  target(key) {
    const here = this.here, row = this.visible().find(x => x.abs === key);
    if (row) return { abs: row.abs, rel: row.rel, name: row.name, dir: row.dir, root: here.tree.root, row };
    const was = this.store.listed(here, key);
    return was ? { abs: key, ...was, row: null } : null;
  }

  // A row is told by its path: whatever the tree is by the time the click is heard, it is that file or none.
  click(key, cx) {
    const at = this.target(key);
    if (!at) return;
    const here = this.here;
    if (at.row) here.selected = at.rel;
    this.version++;
    if (at.dir) { if (at.row) this.toggle(at.row, cx); return; }
    // A tree still being read again, or one that is gone, may show what is not there.
    this.openChecked(at, Boolean(at.row) && panel(here).fresh === "fresh", cx);
  }

  // Opens the file in the Space of the click, once it is known to be there when that is in doubt. Files open in
  // the order they were clicked: a click waits for the checks of those before it.
  openChecked(file, sure, cx) {
    const here = this.here, loc = this.loc, ahead = this.opening;
    if (sure && !ahead) { openFile(file.abs, here.on); cx.notify(); return; }
    const mine = new Promise(done => cx.spawn(async cx => {
      try {
        const there = sure || await within(cx, CHECK_MS, this.store.confirm(here, file.root, file.rel, () => { this.version++; cx.notify(); }));
        if (ahead) await ahead;
        if (this.loc !== loc) return;
        if (there === false) notify({ title: t("error.gone", { name: file.name }), level: "warning" });
        else openFile(file.abs, here.on);
      } finally { done(); }
    }));
    this.opening = mine;
    mine.then(() => { if (this.opening === mine) this.opening = null; });
    cx.notify();
  }

  // Requests from the popup copy (rename, new file …) and refreshes after its actions.
  requests(cx) {
    const req = shared(REQUEST);
    if (req?.seq && req.seq > this.lastRequest) {
      this.lastRequest = req.seq;
      // A frame only looks: the name input and the redraw are for the task that follows it.
      cx.timer.after(0, cx => this.answer(req, cx));
    }
    const refresh = shared(REFRESH) ?? 0;
    if (refresh > this.lastRefresh) { this.lastRefresh = refresh; this.reload(cx); }
  }

  // New file / folder / rename: one bar above the tree.

  // The popup's request, for the machine and the folder this tree is.
  answer(req, cx) {
    const here = this.here;
    if (here?.root && (req.on ?? null) === (here.on ?? null) && this.store.owns(here, req.root)) this.startEdit(req.action, req.path, req.dir, cx);
  }

  startEdit(kind, path, dir, cx) {
    const value = kind === "rename" ? base(path) : "";
    this.edit = { kind, path, dir: dir ?? (path ? parent(path) : this.here.root) };
    this.editInput.set_value(value);
    if (value) {
      const dot = value.lastIndexOf(".");
      this.editInput.set_selected_range({ start: 0, end: dot > 0 ? dot : value.length });
    }
    this.editInput.focus();
    cx?.notify();
  }

  submitEdit(cx) {
    const edit = this.edit, name = this.editInput.value().trim();
    if (!edit) return;
    if (!name || name === "." || name === ".." || /[\\/\0]/.test(name)) { notify({ title: t("error.name"), level: "warning" }); return; }
    this.edit = null;
    // The change is for the machine and the Space it was asked in (the bar is dropped when another Space is shown),
    // whatever is shown by the time the machine answers.
    const here = this.here, loc = this.loc, on = here.on ?? null, target = join(edit.dir, name);
    cx.spawn(async cx => {
      try {
        if (edit.kind === "new-file") { await createFile(target, { on }); if (this.loc === loc) openFile(target, on); }
        else if (edit.kind === "new-folder") await createFolder(target, { on });
        else if (target !== edit.path) { await rename(edit.path, target, { on }); moved(edit.path, target, on); }
        const r = rel(here.root, edit.dir);
        if (r) here.open.add(r);
        here.selected = rel(here.root, target);
      } catch (e) { notify({ title: t(`error.${edit.kind}`), body: String(e?.message ?? e), level: "error" }); }
      finally { this.reload(cx, false, here); cx.notify(); }
    });
    cx.notify();
  }

  // Search: file names (fuzzy) or contents (ripgrep).

  // `fresh`: list the files again rather than use the last 15 s listing.
  search(cx, fresh = false) {
    const query = this.searchInput.value(), here = this.here, seq = ++this.searchSeq;
    if (!query.trim() || !here?.root) { this.results = []; this.searchNote = null; cx.notify(); return; }
    cx.timer.after(this.searchMode === "name" ? 60 : 250, cx => {
      if (seq !== this.searchSeq) return;
      cx.spawn(async cx => {
        try {
          if (this.searchMode === "name") {
            const found = await findFiles(here.root, query, { on: here.on, hidden: this.hidden, limit: 200, fresh });
            if (seq !== this.searchSeq) return;
            this.results = found.matches.map(f => ({ key: f.path, root: here.root, on: here.on, path: f.path }));
            const notes = [
              found.tooLong ? t("search.too_long") : found.matches.length ? null : t("search.none"),
              found.more ? t("search.truncated", { count: found.matches.length }) : null,
              found.truncated ? t("search.partial", { count: "200,000" }) : null,
              found.unfiltered ? t("search.unfiltered", { machine: here.on }) : null,
              found.skipped ? t("search.skipped") : null,
            ].filter(Boolean);
            this.searchNote = notes.length ? notes.join(" ") : null;
          } else {
            const found = await grep(here.root, query, { on: here.on, hidden: this.hidden, limit: 500 });
            if (seq !== this.searchSeq) return;
            this.results = found.hits.map((h, i) => ({ key: `${i}:${h.path}:${h.line}`, root: here.root, on: here.on, path: h.path, line: h.line, column: h.column, text: h.text }));
            const notes = !found.available ? [t("search.no_rg")] : [
              found.hits.length ? null : t("search.none"),
              found.truncated ? t("search.truncated", { count: found.hits.length }) : null,
              found.skipped ? t("search.skipped") : null,
            ].filter(Boolean);
            this.searchNote = notes.length ? notes.join(" ") : null;
          }
        } catch (e) { if (seq === this.searchSeq) { this.results = []; this.searchNote = String(e?.message ?? e); } }
        cx.notify();
      });
    });
  }

  openResult(key) {
    const r = this.results.find(x => x.key === key);
    if (!r) return;
    openFile(join(r.root, r.path), r.on, r.line ? { line: r.line, column: r.column } : {});
  }

  toggleSearch(cx) {
    this.searching = !this.searching;
    // Results kept while closed may be from other settings or files: look again.
    if (this.searching) { this.searchInput.focus(); this.search(cx); }
    cx.notify();
  }

  // Badge and status: unsaved documents are counted by the editor mods.

  render(cx) {
    if (this.menu) return this.renderMenu(cx);
    this.requests(cx);
    const now = where(this.loc);
    if (now?.key !== this.loc?.key || now?.space !== this.loc?.space) this.relocate(cx);
    const th = theme(), tk = tokens(), here = this.here;
    const local = !here?.on, note = here ? info(here) : null;
    // Ghost icon buttons, as on the other sidebars' title rows; a toggle that's on is selected.
    const action = (id, icon, label, onClick, on = false, enabled = true) =>
      div().id(id).size(22).flex().flex_none().items_center().justify_center().rounded(5)
        .bg(on ? th.selected : "#00000000").text_color(enabled ? (on ? th.foreground : th.muted) : th.dim)
        .when(enabled, el => el.cursor_pointer().hover(s => s.bg(on ? th.selected : th.hover).text_color(th.foreground)).on_click(onClick))
        .tooltip(label)
        .child(svg(`mods/${ME}/icons/${icon}.svg`).size(14));
    const canAdd = Boolean(here?.root);
    // The folder above (shrinks first) and the folder's own name; "~" only on this machine.
    const head = here?.root ? heading(here.root, local ? home() : null, note.branch) : null;
    return v_flex().id("root").size_full().min_h(0).bg(th.chrome).text_color(th.foreground).text_size(12.5)
      .child(h_flex().h(32).px(12).flex_none().items_center()
        .child(div().flex_1().min_w(0).truncate().font_semibold().child(t("title")))
        .child(h_flex().flex_none().gap(2)
          .child(action("new-file", "new-file", t("action.new_file"), (_e, cx) => this.startEdit("new-file", null, this.selectedDir(), cx), false, canAdd))
          .child(action("new-folder", "new-folder", t("action.new_folder"), (_e, cx) => this.startEdit("new-folder", null, this.selectedDir(), cx), false, canAdd))
          .child(action("refresh", "refresh", t("action.refresh"), (_e, cx) => { this.reload(cx); cx.notify(); }))
          .child(action("collapse", "collapse", t("action.collapse"), (_e, cx) => this.collapse(cx)))
          .child(action("search", "search", t("action.search"), (_e, cx) => this.toggleSearch(cx), this.searching))
          .child(action("hidden", "eye", t("action.hidden"), (_e, cx) => {
            this.hidden = !this.hidden; localStorage.setItem("hidden", this.hidden ? "1" : "0"); this.version++;
            if (this.searching) this.search(cx);
            cx.notify();
          }, this.hidden))))
      .when(Boolean(head), el => el.child(v_flex().flex_none().mt(4).mx(10).mb(6).py(6).px(8).gap(4).rounded(7)
        .bg(th.raised).border_1().border_color(th.border)
        .child(h_flex().gap(6).items_center().min_w(0)
          .child(svg(`mods/${ME}/icons/files/folder.svg`).size(14).flex_none().text_color(th.muted))
          .child(h_flex().flex_1().min_w(0).items_center()
            .when(Boolean(head.dir), row => row.child(div().min_w(0).flex_shrink(100).truncate().text_color(th.dim).child(head.dir)))
            .child(div().min_w(0).truncate().font_semibold().child(head.name)))
          .child(div().flex_none().px(6).py(1).rounded(4).bg(th.hover).text_size(11).text_color(th.muted)
            .child(local ? t("local") : here.on)))
        .when(Boolean(head.branch), card => card.child(h_flex().gap(6).items_center().min_w(0).text_size(11.5).text_color(th.accent)
          .child(svg("mods/workspace-chrome/git-branch.svg").size(13).flex_none())
          .child(div().min_w(0).truncate().child(head.branch))))))
      .when(!head, el => el.child(div().h(6).flex_none()))
      .when(Boolean(note?.untrusted), el => el.child(div().flex_none().px(12).pb(6).text_size(11).text_color(th.dim).child(t("git.untrusted"))))
      .when(Boolean(this.edit), el => el.child(this.renderEdit(th)))
      .child(this.searching ? this.renderSearch(th) : this.renderTree(th, tk, note));
  }

  selectedDir() {
    const row = this.here.selected ? this.visible().find(r => r.rel === this.here.selected) : null;
    if (!row) return this.here.root;
    return row.dir ? row.abs : parent(row.abs);
  }

  renderEdit(th) {
    const label = t(`edit.${this.edit.kind}`);
    return v_flex().flex_none().mx(8).mb(6).p(6).gap(4).rounded(6).bg(th.popover).border_1().border_color(th.input)
      .on_key_down((e, cx) => { if (e.key === "escape") { this.edit = null; cx.notify(); cx.stop_propagation(); } })
      .child(div().text_size(11).text_color(th.dim).child(label))
      .child(Input.new(this.editInput).h(24).px(6).rounded(4).bg(th.canvas).border_1().border_color(th.border).text_size(12))
      .child(div().text_size(10.5).text_color(th.dim).child(t("edit.hint")));
  }

  renderTree(th, tk, note) {
    const here = this.here, shown = panel(here);
    if (shown.kind === "none") return div().p(12).text_size(12).text_color(th.dim).child(t("empty.no_folder"));
    // Nothing listed yet: loading, or why it could not be.
    if (shown.kind === "empty") return div().p(12).text_size(12).text_color(shown.error ? th.blocked : th.dim).child(shown.error ?? t("empty.loading"));
    const rows = this.visible();
    // The folder was cut, and every entry that made the cut is hidden: there may be more.
    const empty = note.rootCut ? t("empty.hidden_only", { count: PER_FOLDER })
      : note.skipped ? t("empty.skipped") : t("empty.no_files");
    if (!rows.length) return div().p(12).text_size(12).text_color(th.dim).child(empty);
    const fs = tk.file_status ?? {};
    const markColor = m => m === "A" ? fs.added : m === "D" || m === "U" ? fs.deleted : m === "?" ? fs.untracked : fs.modified;
    // One list per folder: a list keeps its scroll position under its id, and another folder's is not this one's.
    const list = `tree:${here.key}`;
    const tree = v_flex().flex_1().min_h(0).relative()
      .child(uniform_list(list, rows.length, i => rows[i].abs, range => rows.slice(range.start, range.end).map(row => {
        const m = this.mark(row), sel = row.rel === here.selected;
        return h_flex().h(25).pl(6 + row.depth * 13 + 6).pr(12).mx(6).gap(6).items_center().rounded(5)
          .bg(sel ? th.selected : "#00000000").hover(s => s.bg(sel ? th.selected : th.hover))
          .child(row.dir ? svg(`mods/${ME}/icons/chevron-${row.open ? "down" : "right"}.svg`).size(10).flex_none().text_color(th.dim) : div().w(10).flex_none())
          .child(this.icon(th, row.name, row.dir, row.open))
          .child(div().flex_1().min_w(0).truncate().text_color(row.link ? th.muted : th.foreground).child(row.name))
          .when(Boolean(m), el => el.child(div().flex_none().text_size(11).font_semibold().text_color(markColor(m) ?? th.blocked).child(m)));
      }))
        .flex_1().min_h(0)
        .on_item_click((key, cx) => this.click(key, cx))
        .on_item_secondary_click((key, e, cx) => this.openMenu(key, e, cx)))
      .child(Scrollbar.vertical(list).absolute().inset_0())
      .when(note.limited, el => el.child(div().flex_none().px(12).py(6).text_size(11).text_color(th.dim).child(t("empty.limited", { count: PER_FOLDER }))))
      .when(note.skipped, el => el.child(div().flex_none().px(12).py(6).text_size(11).text_color(th.dim).child(t("empty.skipped"))))
      .when(Boolean(note.gitFailed), el => el.child(div().flex_none().px(12).py(6).text_size(11).text_color(th.dim).child(t("git.failed"))))
      .when(note.marksCut, el => el.child(div().flex_none().px(12).py(6).text_size(11).text_color(th.dim).child(t("git.truncated"))))
      .when(Boolean(this.rowsCapped), el => el.child(div().flex_none().px(12).py(6).text_size(11).text_color(th.dim).child(t("empty.rows", { count: ROWS }))));
    // The folder could not be listed again: the rows are from before.
    if (typeof shown.fresh !== "object") return tree;
    return v_flex().flex_1().min_h(0)
      .child(div().flex_none().px(12).pb(6).text_size(11.5).text_color(th.blocked).child(shown.fresh.stale))
      .child(tree);
  }

  icon(th, name, dir = false, open = false) {
    const { icon, color } = fileIcon(name, dir, open);
    return svg(`mods/${ME}/icons/files/${icon}.svg`).size(16).flex_none().text_color(th[color] ?? th.dim);
  }

  renderSearch(th) {
    const tab = (id, label) => div().id(`mode-${id}`).px(8).h(22).flex().items_center().rounded(4).cursor_pointer()
      .bg(this.searchMode === id ? th.selected : "#00000000").text_color(this.searchMode === id ? th.foreground : th.muted)
      .on_click((_e, cx) => { this.searchMode = id; this.search(cx); cx.notify(); }).child(label);
    const results = this.results;
    return v_flex().flex_1().min_h(0)
      .child(v_flex().flex_none().px(8).pb(6).gap(6)
        .child(Input.new(this.searchInput).h(26).px(8).rounded(6).bg(th.canvas).border_1().border_color(th.input).text_size(12))
        .child(h_flex().gap(4).text_size(11.5).child(tab("name", t("search.names"))).child(tab("content", t("search.content")))))
      .when(Boolean(this.searchNote), el => el.child(div().px(12).py(4).text_size(11.5).text_color(th.dim).child(this.searchNote)))
      .child(v_flex().flex_1().min_h(0).relative()
        .child(uniform_list("results", results.length, i => results[i].key, range => results.slice(range.start, range.end).map(r =>
          h_flex().h(r.line ? 38 : 25).mx(6).px(8).gap(6).items_center().rounded(5).hover(s => s.bg(th.hover))
            .child(this.icon(th, base(r.path)))
            .child(v_flex().flex_1().min_w(0)
              .child(h_flex().gap(6).min_w(0)
                .child(div().flex_none().max_w(150).truncate().child(base(r.path)))
                .child(div().flex_1().min_w(0).truncate().text_size(11).text_color(th.dim).child(r.line ? `${parent(r.path) || "."}:${r.line}` : parent(r.path))))
              .when(Boolean(r.line), el => el.child(div().truncate().text_size(11).text_color(th.muted).child(r.text.trim()))))))
          .flex_1().min_h(0)
          .on_item_click((key, cx) => this.openResult(key)))
        .child(Scrollbar.vertical("results").absolute().inset_0()));
  }

  // The right-click popup.

  openMenu(key, e, cx) {
    const at = this.target(key);
    if (!at) return;
    const here = this.here, menu = { abs: at.abs, rel: at.rel, dir: at.dir, root: at.root, on: here.on };
    if (at.row) here.selected = at.rel;
    this.version++; cx.notify();
    const items = menuItems(menu);
    openOverlay({ mod: ME, kind: "popup", width: 236, height: 12 + items.filter(i => i).length * 28 + items.filter(i => !i).length * 9, props: { menu } });
  }

  renderMenu(cx) {
    const th = theme(), m = this.menu, items = menuItems(m);
    return v_flex().id("menu").size_full().p(6).gap(0).bg(th.popover).text_color(th.foreground).text_size(12.5)
      .children(items.map((it, i) => it
        ? h_flex().id(`item-${i}`).h(28).px(8).rounded(5).items_center().gap(8)
          .text_color(it.enabled === false ? th.dim : it.danger ? th.ansi1 : th.foreground)
          .when(it.enabled !== false, el => el.cursor_pointer().hover(s => s.bg(th.hover)).on_click((_e, cx) => { closeOverlay(props().overlay); it.run(); }))
          .child(div().flex_1().child(it.label))
        : div().h(1).my(4).mx(4).bg(th.border)));
  }
}

function request(action, m, extra) {
  setShared(REQUEST, { action, root: m.root, on: m.on ?? null, seq: Date.now(), ...extra });
}

function revealLabel() {
  const p = window()?.platform;
  return t(p === "macos" ? "menu.reveal_mac" : p === "windows" ? "menu.reveal_windows" : "menu.reveal");
}

// The popup's items for a row: { label, run, enabled?, danger? }, null a divider.
function menuItems(m) {
  const local = !m.on, folder = m.dir ? m.abs : parent(m.abs);
  const relPath = rel(m.root, m.abs) ?? m.abs;
  return [
    m.dir ? null : { label: t("menu.open"), run: () => openFile(m.abs, m.on) },
    m.dir ? null : { label: t("menu.open_side"), run: () => openFile(m.abs, m.on, { place: "side" }) },
    { label: t("menu.terminal"), run: () => run("terminal.open", { folder, place: "right" }) },
    null,
    { label: t("menu.new_file"), run: () => request("new-file", m, { dir: folder }) },
    { label: t("menu.new_folder"), run: () => request("new-folder", m, { dir: folder }) },
    { label: t("menu.rename"), run: () => request("rename", m, { path: m.abs }) },
    { label: t("menu.trash"), enabled: local, danger: true, run: () => {
      trash(m.abs).then(() => { setShared(REFRESH, Date.now()); notify({ title: t("toast.trashed", { name: base(m.abs) }), level: "success", timeout: 3000 }); })
        .catch(e => notify({ title: t("error.trash"), body: String(e?.message ?? e), level: "error" }));
    } },
    null,
    { label: t("menu.copy_path"), run: () => copyText(m.abs) },
    { label: t("menu.copy_relative"), run: () => copyText(relPath) },
    { label: revealLabel(), enabled: local, run: () => { try { reveal(m.abs); } catch (e) { notify({ title: t("error.reveal"), body: String(e?.message ?? e), level: "error" }); } } },
  ].filter((it, i, all) => it || (i > 0 && all[i - 1]));
}
