// The Git merge pane: the files a merge, rebase, cherry-pick or revert left in
// conflict, and a three-way editor (yours | result | theirs) for each one.
// Owned by lane L2; git runs in yuzora:git (Rust). The Git sidebar opens it
// through shared "yuzora.git.merge" { cwd, on, path, at }.
//
// The chunk list is a v_virtual_list, whose item renderer may not register
// handlers: rows are only drawn there, clicks come from on_item_click and the
// chunk's controls live in the bar above the list.
import { View, div } from "gpui-kit";
import { v_flex, h_flex, TextareaState, Textarea, v_virtual_list, Scrollbar, VirtualListScrollHandle } from "gpui-base";
import { props, shared, theme, settings } from "herdr";
import { t, tokens } from "yuzora:core";
import { open, status, changed, sequencer, conflictOpen, conflictResolve, conflictAccept, settings as gitSettings } from "yuzora:git";

const KEY = "yuzora.git.merge";
const LINE = 18;
// [folds above, lines kept first, lines kept last]: long unchanged text folds
// early, a huge change only late.
const FOLD = { equal: [8, 3, 3], change: [200, 150, 20] };
// status().operation.kind is "none" (or empty) when nothing is in progress.
const kindOf = (st) => (st?.operation?.kind && st.operation.kind !== "none" ? st.operation.kind : "");
// Added or deleted on one side only (AU, UA, DU, UD, DD) has nothing to merge: take a side.
const mergeable = (kind) => kind === "UU" || kind === "AA";
const NON_CONFLICT = ["ours", "theirs", "same"];
const sourceSide = (kind) => (kind === "theirs" ? "theirs" : "ours");

export default class GitMerge extends View {
  init(_props, cx) {
    this.handle = cx.focus_handle();
    this.scroll = VirtualListScrollHandle.new();
    this.edit = TextareaState.new({ rows: 5 });
    this.edit.set_auto_grow(4, 12);
    this.edit.on("focus", (_e, cx) => { this.typing = true; cx.notify(); });
    this.edit.on("blur", (_e, cx) => { this.typing = false; cx.notify(); });
    this.ctx = 0; this.at = 0; this.spec = null;
    this.repo = null; this.st = null; this.error = null; this.note = null; this.busy = null;
    this.picked = new Set(); this.confirmAbort = false; this.labels = null; this.labelsKey = null;
    this.resetFile();
    const p = props(), spec = p?.cwd ? p : shared(KEY);
    if (spec?.cwd) this.start(spec, cx);
  }

  async close() {
    this.ctx++;
    this.edit.release();
    this.scroll.release();
  }

  resetFile() {
    this.file = null; this.session = null; this.fileError = null; this.loading = false;
    this.choices = {}; this.sel = -1; this.expanded = new Set(); this.editing = null; this.typing = false; this.colW = 240;
  }

  // A new place: open its repo and follow it until another one replaces it.
  // No notify here: render() calls this too, and the tasks below notify.
  start(spec, cx) {
    this.spec = spec; this.at = spec.at ?? 0;
    const ctx = ++this.ctx;
    this.repo = null; this.st = null; this.error = null; this.note = null; this.picked.clear(); this.labels = null; this.labelsKey = null;
    this.resetFile();
    cx.spawn(async (cx) => {
      try {
        const repo = await open(spec.cwd, spec.on ?? null);
        if (this.ctx !== ctx) return;
        this.repo = repo;
        await this.reload(cx, ctx);
        if (spec.path && this.ctx === ctx) this.openFile(spec.path, cx);
        // Local repos: the watcher bumps the generation. Remote ones: no
        // watcher, so each ~25 s wait without news reads status again.
        let gen = this.st?.generation ?? 0;
        while (this.ctx === ctx) {
          const next = await changed(repo.id, gen);
          if (this.ctx !== ctx) break;
          const grew = next > gen;
          gen = next;
          if (grew || repo.machine) await this.reload(cx, ctx, !grew);
        }
      } catch (e) {
        if (this.ctx !== ctx) return;
        this.error = String(e).includes("not a git repository") ? t("notRepo") : String(e);
        cx.notify();
      }
    });
  }

  async reload(cx, ctx, force = false) {
    const repo = this.repo;
    if (!repo) return;
    try {
      const st = await status(repo.id, { force });
      if (this.ctx !== ctx) return;
      this.st = st; this.error = null;
      const paths = new Set(st.conflicts.map((c) => c.path));
      for (const p of [...this.picked]) if (!paths.has(p)) this.picked.delete(p);
      // Resolved elsewhere (a terminal, Continue): the open file is done.
      if (this.file && !paths.has(this.file) && !this.busy) this.resetFile();
      this.loadLabels(cx);
    } catch (e) { if (this.ctx === ctx) this.error = String(e); }
    cx.notify();
  }

  opKey() {
    const op = this.st?.operation;
    return `${kindOf(this.st)}|${op?.step?.join("/") ?? ""}|${op?.onto ?? ""}`;
  }

  // The names of the two sides come with a conflict view; the list reads one
  // file's (once per operation step) just to have them.
  loadLabels(cx) {
    const key = this.opKey(), repo = this.repo, ctx = this.ctx;
    const c = this.st?.conflicts.find((x) => mergeable(x.kind));
    if (!c || this.labelsKey === key) return;
    this.labelsKey = key;
    cx.spawn(async (cx) => {
      try {
        const v = await conflictOpen(repo.id, c.path);
        if (this.ctx === ctx) { this.labels = v.labels; cx.notify(); }
      } catch {}
    });
  }

  // One git call at a time; its failure shows in the note. `after` runs once the
  // call worked and the status is fresh.
  act(cx, name, work, after) {
    if (this.busy || !this.repo) return;
    const ctx = this.ctx;
    this.busy = name; this.note = null; this.confirmAbort = false;
    cx.spawn(async (cx) => {
      let ok = false;
      try {
        const out = await work(this.repo);
        if (out?.ok === false) this.note = { tone: "error", text: out.output || t("failed") };
        else {
          ok = true;
          if (out?.conflicts?.length) this.note = { tone: "warning", text: t("conflicts", { count: out.conflicts.length }) };
        }
      } catch (e) { this.note = { tone: "error", text: String(e) }; }
      finally { this.busy = null; await this.reload(cx, ctx); }
      if (ok && after && this.ctx === ctx) after(cx);
    });
    cx.notify();
  }

  // ---- one file ----

  openFile(path, cx) {
    const c = this.st?.conflicts.find((x) => x.path === path), repo = this.repo, ctx = this.ctx;
    this.resetFile(); this.note = null;
    if (!c || !repo) { this.note = { tone: "warning", text: t("file.notConflicted", { path }) }; cx.notify(); return; }
    this.file = path;
    cx.notify();
    if (!mergeable(c.kind)) return;
    this.loading = true;
    cx.spawn(async (cx) => {
      try {
        const v = await conflictOpen(repo.id, path);
        if (this.ctx !== ctx || this.file !== path) return;
        this.labels = v.labels; this.labelsKey = this.opKey();
        if (v.binary) this.fileError = t("file.binary");
        else { this.session = v; this.prepare(); }
      } catch (e) { if (this.ctx === ctx && this.file === path) this.fileError = String(e); }
      finally { if (this.ctx === ctx && this.file === path) this.loading = false; cx.notify(); }
      if (this.session && this.file === path && this.sel >= 0) {
        await cx.sleep(60);
        try { this.scroll.scroll_to_item(this.sel, "center"); } catch {}
        cx.notify();
      }
    });
  }

  prepare() {
    let longest = 0;
    for (const c of this.session.chunks) for (const side of [c.ours, c.theirs, c.base]) for (const l of side) longest = Math.max(longest, l.length);
    // Monospace: about 7.3 px a character at 12 px.
    this.colW = Math.min(900, Math.max(240, Math.min(longest, 200) * 7.3 + 24));
    this.sel = this.pool()[0] ?? -1;
  }

  // The chunks prev/next walk: open conflicts, else every conflict, else every change.
  pool() {
    const ch = this.session?.chunks ?? [];
    const conflicts = ch.filter((c) => c.kind === "conflict");
    const open = conflicts.filter((c) => this.choices[c.index] === undefined);
    return (open.length ? open : conflicts.length ? conflicts : ch.filter((c) => c.kind !== "equal")).map((c) => c.index);
  }

  remaining() {
    return (this.session?.chunks ?? []).filter((c) => c.kind === "conflict" && this.choices[c.index] === undefined).length;
  }

  // What the Result column shows for a chunk: the lines it will write, or null while a conflict is open.
  resultOf(c) {
    // A final "\n" ends the last line; it doesn't start another.
    const ch = this.choices[c.index], lines = (s) => (s === "" ? [] : s.replace(/\n$/, "").split("\n"));
    if (ch === undefined) return c.kind === "conflict" ? null : c.kind === "theirs" ? c.theirs : c.ours;
    if (typeof ch === "object") return lines(ch.text);
    switch (ch) {
      case "ours": return c.ours;
      case "theirs": return c.theirs;
      case "base": return c.base;
      case "both": return [...c.ours, ...c.theirs];
      case "both-theirs-first": return [...c.theirs, ...c.ours];
      default: return lines(c.auto ?? "");
    }
  }

  // Whether the chunk's result ends its last line: only a file's last line may not.
  endsLine(c) {
    const ch = this.choices[c.index], e = c.eol ?? {};
    const side = (k) => e[k] !== false;
    if (ch === undefined) return side(c.kind === "theirs" ? "theirs" : "ours");
    if (typeof ch === "object") return ch.text.endsWith("\n");
    switch (ch) {
      case "theirs": case "base": return side(ch);
      case "both": return side(c.theirs.length ? "theirs" : "ours");
      case "both-theirs-first": return side(c.ours.length ? "ours" : "theirs");
      case "auto": return side("ours") && side("theirs");
      default: return side("ours");
    }
  }

  choose(i, value, cx) {
    this.choices = { ...this.choices, [i]: value };
    this.editing = null; this.typing = false;
    cx.notify();
  }

  reset(i, cx) {
    const { [i]: _gone, ...rest } = this.choices;
    this.choices = rest;
    cx.notify();
  }

  // Non-conflicting changes already count in the result; this puts back the ones a click moved off their side.
  deviating() {
    return (this.session?.chunks ?? []).filter((c) => NON_CONFLICT.includes(c.kind) && this.choices[c.index] !== undefined && this.choices[c.index] !== sourceSide(c.kind));
  }

  applyNonConflicting(cx) {
    const next = { ...this.choices };
    for (const c of this.session.chunks) if (NON_CONFLICT.includes(c.kind)) next[c.index] = sourceSide(c.kind);
    this.choices = next; this.editing = null;
    cx.notify();
  }

  simple() {
    return (this.session?.chunks ?? []).filter((c) => c.kind === "conflict" && c.auto != null && this.choices[c.index] === undefined);
  }

  resolveSimple(cx) {
    const next = { ...this.choices };
    for (const c of this.simple()) next[c.index] = "auto";
    this.choices = next;
    cx.notify();
  }

  go(dir, cx) {
    const at = this.pool();
    if (!at.length) return;
    const next = dir > 0 ? at.find((i) => i > this.sel) ?? at[0] : [...at].reverse().find((i) => i < this.sel) ?? at[at.length - 1];
    this.sel = next;
    this.scroll.scroll_to_item(next, "center");
    cx.notify();
  }

  startEdit(i, cx) {
    const c = this.session.chunks[i];
    this.editing = i;
    this.editEnds = this.endsLine(c);
    // Each line with its end, as written: no lines is "", one blank line "\n",
    // so the box reads back the lines it showed (a file's last line may have no end).
    const lines = this.resultOf(c) ?? c.ours;
    this.edit.set_value(lines.map((l, k) => (k < lines.length - 1 || this.editEnds ? `${l}\n` : l)).join(""));
    cx.spawn(async (cx) => { await cx.sleep(30); this.edit.focus(); });
    cx.notify();
  }

  // The text sent ends its last line as the source did, even if the box lost
  // its final end. An emptied box drops the chunk ("" is no line; one blank line is "\n").
  useEdit(cx) {
    const value = this.edit.value();
    this.choose(this.editing, { text: value !== "" && this.editEnds && !value.endsWith("\n") ? `${value}\n` : value }, cx);
  }

  stopEdit(cx) {
    this.editing = null; this.typing = false;
    this.handle.focus();
    cx.notify();
  }

  // The file after this one in the list that can still be merged, else the first.
  nextAfter(order, path) {
    const at = order.indexOf(path), left = this.st?.conflicts.filter((c) => mergeable(c.kind)) ?? [];
    return (left.find((c) => order.indexOf(c.path) > at) ?? left[0])?.path ?? null;
  }

  afterFile(order, path) {
    return (cx) => {
      const next = this.nextAfter(order, path);
      if (next) this.openFile(next, cx);
      else cx.notify();
    };
  }

  applyFile(cx) {
    const s = this.session, path = this.file, order = this.st.conflicts.map((c) => c.path);
    if (!s || this.remaining() > 0) return;
    this.act(cx, "apply", async (repo) => {
      try { return await conflictResolve(s.id, { ...this.choices }); }
      catch (e) {
        // The session is kept for a while only: open it again, same chunks.
        if (!String(e).includes("gone")) throw e;
        const v = await conflictOpen(repo.id, path);
        return conflictResolve(v.id, { ...this.choices });
      }
    }, this.afterFile(order, path));
  }

  accept(paths, side, cx) {
    const order = this.st.conflicts.map((c) => c.path), file = this.file;
    this.act(cx, "accept", (repo) => conflictAccept(repo.id, paths, side), file ? this.afterFile(order, file) : undefined);
  }

  togglePick(path, cx) {
    if (!this.picked.delete(path)) this.picked.add(path);
    cx.notify();
  }

  key(e, cx) {
    if (this.typing) {
      if (e.key === "escape") { this.stopEdit(cx); cx.stop_propagation(); }
      return;
    }
    if (!this.file) return;
    const c = this.session?.chunks[this.sel], change = c && c.kind !== "equal";
    if (e.key === "escape") this.resetFile();
    else if (!this.session) return;
    else if (e.key === "down") this.go(1, cx);
    else if (e.key === "up") this.go(-1, cx);
    else if (change && e.key === "left") this.choose(c.index, "ours", cx);
    else if (change && e.key === "right") this.choose(c.index, "theirs", cx);
    else if (change && e.key === "b") this.choose(c.index, "both", cx);
    else if (change && e.key === "a" && c.auto != null) this.choose(c.index, "auto", cx);
    else if (change && e.key === "e") this.startEdit(c.index, cx);
    else return;
    cx.stop_propagation();
    cx.notify();
  }

  // ---- text ----

  sideName(side) {
    return t(side === "ours" ? "side.yours" : "side.theirs");
  }

  // "Yours: abc1234 my change" once the names are known.
  sideLabel(side) {
    const l = this.labels?.[side];
    return l ? `${this.sideName(side)} · ${l}` : this.sideName(side);
  }

  kindLabel(kind) {
    if (kind === "UU" || kind === "AA" || kind === "DD") return t(`kind.${kind}`);
    // git's "us" is yours, except in a rebase, where the sides are swapped.
    const swapped = kindOf(this.st) === "rebasing" && gitSettings().swapSidesInRebase !== false;
    const us = kind === "AU" || kind === "DU";
    const who = us !== swapped ? "yours" : "theirs";
    return t(`kind.${kind[0] === "A" || kind[1] === "A" ? "added" : "deleted"}.${who}`);
  }

  // ---- drawing ----

  button(id, label, enabled, th, action, primary = false, tip = null) {
    return h_flex().id(id).h(24).px(8).flex_none().items_center().justify_center().rounded(5).text_size(11.5)
      .when(primary, (el) => el.bg(enabled ? th.solid : th.raised).text_color(enabled ? th.solid_foreground : th.dim).font_semibold())
      .when(!primary, (el) => el.border_1().border_color(th.border).text_color(enabled ? th.foreground : th.dim))
      .when(Boolean(tip), (el) => el.tooltip(tip))
      .when(enabled, (el) => el.cursor_pointer().hover((s) => (primary ? s.opacity(0.88) : s.bg(th.hover))).on_click((_e, cx) => action(cx)))
      .child(label);
  }

  checkbox(id, on, th, toggle) {
    return div().id(id).w(14).h(14).flex().flex_none().items_center().justify_center().rounded(3).border_1().cursor_pointer()
      .border_color(on ? th.accent : th.input ?? th.border).bg(on ? th.accent : th.background).text_size(9).text_color(th.background)
      .on_mouse_down("left", (_e, cx) => cx.stop_propagation())
      .on_click((_e, cx) => { cx.stop_propagation(); toggle(cx); }).child(on ? "✓" : "");
  }

  noteBar(th) {
    if (!this.note && !this.busy) return null;
    const tone = this.note?.tone === "error" ? th.ansi1 : th.blocked;
    return div().px(12).py(6).flex_none().text_size(11).text_color(this.note ? tone : th.dim).child(this.note ? this.note.text : "…");
  }

  header(th) {
    const kind = kindOf(this.st), step = this.st?.operation?.step;
    return h_flex().h(36).flex_none().px(12).gap(8).items_center().border_b(1).border_color(th.divider ?? th.border)
      .when(Boolean(this.file), (el) => el.child(this.button("back", `‹ ${t("title")}`, true, th, (cx) => { this.resetFile(); cx.notify(); })))
      .child(div().font_semibold().flex_1().min_w(0).truncate().child(this.file ?? t("title")))
      .when(Boolean(kind), (el) => el.child(div().flex_none().px(6).h(20).flex().items_center().rounded(4).border_1().border_color(th.border).text_size(10.5).text_color(th.muted)
        .child(`${t(`op.${kind}`)}${step ? ` ${step[0]}/${step[1]}` : ""}`)));
  }

  // Continue and Abort for the operation in progress, once the files are done.
  operationBar(th) {
    const st = this.st, kind = kindOf(st);
    if (!kind) return null;
    const op = st.operation, att = tokens().attention ?? {}, verb = t(`op.verb.${kind}`), left = st.conflicts.length;
    const onto = op.onto ? (/^[0-9a-f]{20,}$/.test(op.onto) ? op.onto.slice(0, 8) : op.onto) : null;
    const seq = (action) => (cx) => this.act(cx, "sequencer", (r) => sequencer(r.id, action));
    return v_flex().mx(10).mt(8).p(8).gap(6).flex_none().rounded(6).bg(att.background ?? th.raised).border_1().border_color(att.border ?? th.border)
      .child(div().text_size(11.5).text_color(att.text ?? th.blocked).child([t(`op.${kind}`), onto && t("op.onto", { onto }), op.head].filter(Boolean).join(" · ")))
      .when(kind !== "bisecting", (el) => el.child(h_flex().gap(6).flex_wrap()
        .child(this.button("op-continue", t("op.continue", { op: verb }), left === 0 && !this.busy, th, seq("continue"), true, left ? t("op.resolveFirst") : null))
        .when(kind !== "merging", (e) => e.child(this.button("op-skip", t("op.skip"), !this.busy, th, seq("skip"))))
        .child(this.button("op-abort", t("op.abort", { op: verb }), !this.busy, th, (cx) => { this.confirmAbort = true; cx.notify(); }))))
      .when(this.confirmAbort, (el) => el.child(h_flex().gap(6).items_center().flex_wrap().text_size(11)
        .child(div().text_color(th.foreground).child(t("op.abort.ask", { op: verb })))
        .child(this.button("op-abort-yes", t("op.abort", { op: verb }), !this.busy, th, seq("abort")))
        .child(this.button("op-abort-no", t("cancel"), true, th, (cx) => { this.confirmAbort = false; cx.notify(); }))));
  }

  conflictRow(c, th) {
    const on = this.picked.has(c.path), parts = c.path.split("/"), name = parts.pop(), dir = parts.join("/"), can = mergeable(c.kind);
    return h_flex().id(`c-${c.path}`).h(30).px(8).gap(8).items_center().rounded(6).cursor_pointer()
      .when(on, (el) => el.bg(th.selected)).hover((s) => s.bg(th.hover))
      .on_click((e, cx) => { if (e.click_count > 1 && can) this.openFile(c.path, cx); else this.togglePick(c.path, cx); })
      .child(this.checkbox(`cb-${c.path}`, on, th, (cx) => this.togglePick(c.path, cx)))
      .child(div().w(12).flex_none().text_size(11).font_semibold().text_color((tokens().file_status ?? {}).deleted ?? th.ansi1).child("U"))
      .child(h_flex().flex_1().min_w(0).gap(6)
        .child(div().flex_none().max_w(220).truncate().child(name))
        .when(Boolean(dir), (el) => el.child(div().flex_1().min_w(0).truncate().text_size(10.5).text_color(th.dim).child(dir))))
      .child(div().flex_none().text_size(10.5).text_color(th.muted).child(this.kindLabel(c.kind)))
      .when(can, (el) => el.child(this.button(`m-${c.path}`, t("merge"), !this.busy, th, (cx) => { cx.stop_propagation(); this.openFile(c.path, cx); })));
  }

  listView(th) {
    const st = this.st, conflicts = st.conflicts, picked = conflicts.filter((c) => this.picked.has(c.path));
    const all = conflicts.length > 0 && picked.length === conflicts.length, idle = !this.busy;
    const first = picked.find((c) => mergeable(c.kind)) ?? conflicts.find((c) => mergeable(c.kind));
    const paths = picked.map((c) => c.path);
    const toolbar = h_flex().h(34).flex_none().px(10).gap(8).items_center()
      .child(h_flex().gap(6).items_center().text_size(11).text_color(th.muted)
        .child(this.checkbox("all", all, th, (cx) => { if (all) this.picked.clear(); else for (const c of conflicts) this.picked.add(c.path); cx.notify(); }))
        .child(picked.length ? t("selected", { count: picked.length }) : t("conflicts", { count: conflicts.length })))
      .child(div().flex_1())
      .child(this.button("yours", t("accept.yours"), idle && paths.length > 0, th, (cx) => this.accept(paths, "ours", cx), false, this.sideLabel("ours")))
      .child(this.button("theirs", t("accept.theirs"), idle && paths.length > 0, th, (cx) => this.accept(paths, "theirs", cx), false, this.sideLabel("theirs")))
      .child(this.button("merge", t("merge"), idle && Boolean(first), th, (cx) => this.openFile(first.path, cx), true));
    const bar = this.operationBar(th);
    return v_flex().flex_1().min_h(0)
      .when(Boolean(bar), (el) => el.child(bar))
      .when(conflicts.length > 0, (el) => el.child(toolbar)
        .when(Boolean(this.labels?.ours || this.labels?.theirs), (e) => e.child(div().px(12).pb(4).flex_none().text_size(10.5).text_color(th.dim).truncate()
          .child(`${this.sideLabel("ours")}   ${this.sideLabel("theirs")}`))))
      .child(v_flex().id("conflicts").flex_1().min_h(0).overflow_y_scrollbar().px(6)
        .children(conflicts.map((c) => this.conflictRow(c, th)))
        .when(conflicts.length === 0, (el) => el.child(div().px(8).pt(16).text_size(11.5).text_color(th.dim)
          .child(kindOf(st) ? t("allResolved") : t("noConflicts")))));
  }

  // ---- the three columns ----

  heightOf(c) {
    const kind = c.kind === "equal" ? "equal" : "change", [limit, head, tail] = FOLD[kind], open = this.expanded.has(c.index);
    const shown = (n) => (open || n <= limit ? n : head + 1 + tail);
    const res = this.resultOf(c);
    return Math.max(shown(c.ours.length), shown(c.theirs.length), res ? shown(res.length) : 1, 1) * LINE + (c.kind === "equal" ? 0 : 6);
  }

  foldable(c) {
    const limit = FOLD[c.kind === "equal" ? "equal" : "change"][0];
    return Math.max(c.ours.length, c.theirs.length, this.resultOf(c)?.length ?? 0) > limit;
  }

  // One column's lines of a chunk, folded when long.
  cells(lines, c, th) {
    const [limit, head, tail] = FOLD[c.kind === "equal" ? "equal" : "change"], open = this.expanded.has(c.index);
    const line = (text) => div().h(LINE).px(8).truncate().child(text || " ");
    if (open || lines.length <= limit) return lines.map((l) => line(l));
    return [...lines.slice(0, head).map((l) => line(l)),
      div().h(LINE).px(8).text_size(11).text_color(th.dim).child(t("fold", { count: lines.length - head - tail })),
      ...lines.slice(lines.length - tail).map((l) => line(l))];
  }

  // Drawn inside the list's renderer: no handlers, no state, no notify.
  chunkRow(c, th) {
    const d = tokens().diff ?? {}, att = tokens().attention ?? {};
    const change = d.added_bg ?? th.raised, conflict = att.background ?? th.raised;
    const equal = c.kind === "equal", res = this.resultOf(c);
    const yoursBg = equal || c.kind === "theirs" ? null : c.kind === "conflict" ? conflict : change;
    const theirsBg = equal || c.kind === "ours" ? null : c.kind === "conflict" ? conflict : change;
    const resultBg = res === null ? conflict : equal ? null : change;
    const column = (lines, bg, first) => v_flex().flex_1().min_w(this.colW).h_full().overflow_hidden().py(equal ? 0 : 3)
      .when(Boolean(bg), (el) => el.bg(bg)).when(!first, (el) => el.border_l(1).border_color(th.divider ?? th.border)).children(lines);
    const unresolved = [div().h(LINE).px(8).text_color(att.text ?? th.blocked).truncate().child(`⚠ ${t("unresolved")}`)];
    return h_flex().w_full().h(this.heightOf(c))
      .child(div().w(3).h_full().flex_none().when(this.sel === c.index, (el) => el.bg(th.accent)))
      .child(column(this.cells(c.ours, c, th), yoursBg, true))
      .child(column(res === null ? unresolved : this.cells(res, c, th), resultBg, false))
      .child(column(this.cells(c.theirs, c, th), theirsBg, false));
  }

  pill(id, label, active, th, action, tip = null) {
    return h_flex().id(id).h(22).px(7).flex_none().items_center().rounded(5).text_size(11).cursor_pointer()
      .border_1().border_color(active ? th.accent : th.border).text_color(active ? th.foreground : th.muted)
      .when(active, (el) => el.bg(th.selected)).hover((s) => s.bg(th.hover))
      .when(Boolean(tip), (el) => el.tooltip(tip))
      .on_click((_e, cx) => action(cx)).child(label);
  }

  // The selected chunk's controls.
  chunkBar(th) {
    const s = this.session, c = s.chunks[this.sel], row = h_flex().h(32).flex_none().px(10).gap(6).items_center().border_b(1).border_color(th.divider ?? th.border);
    if (!c || c.kind === "equal") return row.child(div().text_size(11).text_color(th.dim).child(t("chunk.hint")));
    const changes = s.chunks.filter((x) => x.kind !== "equal"), n = changes.findIndex((x) => x.index === c.index) + 1;
    const cur = this.choices[c.index];
    const opt = (id, label, value, tip) => this.pill(`o-${id}`, label, cur === value, th, (cx) => this.choose(c.index, value, cx), tip);
    return row
      .child(div().flex_none().text_size(11).text_color(c.kind === "conflict" ? (tokens().attention ?? {}).text ?? th.blocked : th.muted)
        .child(`${t("chunk.n", { n, total: changes.length })} · ${t(`chunk.${c.kind}`)}`))
      .child(div().flex_1())
      .child(opt("ours", `‹ ${t("side.yours")}`, "ours", t("chunk.tip.ours")))
      .child(opt("theirs", `${t("side.theirs")} ›`, "theirs", t("chunk.tip.theirs")))
      .child(opt("both", t("chunk.both"), "both", t("chunk.tip.both")))
      .child(opt("both-theirs-first", t("chunk.bothReverse"), "both-theirs-first", t("chunk.tip.bothReverse")))
      .child(opt("base", t("chunk.base"), "base", t("chunk.tip.base")))
      .when(c.auto != null, (el) => el.child(opt("auto", t("chunk.auto"), "auto", t("chunk.tip.auto"))))
      .child(this.pill("o-edit", t("chunk.edit"), typeof cur === "object" || this.editing === c.index, th, (cx) => this.startEdit(c.index, cx)))
      .when(cur !== undefined, (el) => el.child(this.pill("o-reset", t("chunk.reset"), false, th, (cx) => this.reset(c.index, cx), t("chunk.tip.reset"))));
  }

  fileToolbar(th) {
    const s = this.session, left = this.remaining(), idle = !this.busy, deviating = this.deviating().length, simple = this.simple().length;
    return h_flex().h(34).flex_none().px(10).gap(6).items_center().border_b(1).border_color(th.divider ?? th.border)
      .child(this.button("non-conflicting", deviating ? t("apply.nonConflicting.n", { count: deviating }) : t("apply.nonConflicting"), idle && deviating > 0, th,
        (cx) => this.applyNonConflicting(cx), false, t("apply.nonConflicting.tip")))
      .child(this.button("simple", simple ? t("apply.simple.n", { count: simple }) : t("apply.simple"), idle && simple > 0, th, (cx) => this.resolveSimple(cx)))
      .child(this.button("prev", "↑", this.pool().length > 0, th, (cx) => this.go(-1, cx), false, t("chunk.prev")))
      .child(this.button("next", "↓", this.pool().length > 0, th, (cx) => this.go(1, cx), false, t("chunk.next")))
      .child(div().flex_none().text_size(11).text_color(left ? (tokens().attention ?? {}).text ?? th.blocked : (tokens().diff ?? {}).added ?? th.ansi2)
        .child(left ? t("left", { count: left }) : `✓ ${t("left.none")}`))
      .child(div().flex_1())
      .child(this.button("file-yours", t("accept.yours"), idle, th, (cx) => this.accept([this.file], "ours", cx), false, this.sideLabel("ours")))
      .child(this.button("file-theirs", t("accept.theirs"), idle, th, (cx) => this.accept([this.file], "theirs", cx), false, this.sideLabel("theirs")))
      .child(this.button("apply", this.busy === "apply" ? t("apply.running") : t("apply"), idle && left === 0 && s.chunks.length > 0, th, (cx) => this.applyFile(cx), true,
        left ? t("apply.blocked", { count: left }) : null));
  }

  // No merge to show: binary, unreadable, or added/deleted on one side. Whole-file only.
  wholeFile(th, text) {
    const idle = !this.busy, kind = this.st.conflicts.find((c) => c.path === this.file)?.kind;
    return v_flex().flex_1().min_h(0).p(16).gap(10)
      .child(div().text_size(12).text_color(th.muted).child(text))
      .when(Boolean(kind), (el) => el.child(div().text_size(11).text_color(th.dim).child(this.kindLabel(kind))))
      .child(h_flex().gap(8)
        .child(this.button("w-yours", t("accept.yours"), idle, th, (cx) => this.accept([this.file], "ours", cx), false, this.sideLabel("ours")))
        .child(this.button("w-theirs", t("accept.theirs"), idle, th, (cx) => this.accept([this.file], "theirs", cx), false, this.sideLabel("theirs"))))
      .when(Boolean(this.labels), (el) => el.child(div().text_size(10.5).text_color(th.dim).child(`${this.sideLabel("ours")}   ${this.sideLabel("theirs")}`)));
  }

  fileView(th) {
    const s = this.session, c = this.st.conflicts.find((x) => x.path === this.file);
    if (this.loading) return div().p(16).text_size(11.5).text_color(th.dim).child(t("loading"));
    if (this.fileError) return this.wholeFile(th, this.fileError);
    if (!s) return this.wholeFile(th, c && !mergeable(c.kind) ? t("file.oneSide") : t("loading"));
    const chunks = s.chunks, sizes = chunks.map((c) => this.heightOf(c));
    const minW = this.colW * 3 + 4, heads = [this.sideLabel("ours"), t("result"), this.sideLabel("theirs")];
    const colHead = h_flex().w_full().h(22).flex_none().border_b(1).border_color(th.divider ?? th.border).text_size(11).text_color(th.muted)
      .child(div().w(3).flex_none())
      .children(heads.map((h, i) => div().flex_1().min_w(this.colW).px(8).h_full().flex().items_center().truncate()
        .when(i > 0, (el) => el.border_l(1).border_color(th.divider ?? th.border)).child(h)));
    const list = div().relative().flex_1().min_h(0).font_family(settings().font_family ?? "JetBrains Mono").text_size(12)
      .child(div().id("merge-x").size_full().overflow_x_scrollbar()
        .child(v_flex().w_full().min_w(minW).h_full()
          .child(colHead)
          .child(div().flex_1().min_h(0).child(v_virtual_list("merge-chunks", chunks.length, sizes, (i) => String(i),
            (range) => chunks.slice(range.start, range.end).map((ch) => this.chunkRow(ch, th)))
            .size_full().track_scroll(this.scroll)
            .on_item_click((key, cx) => {
              const ch = chunks[Number(key)];
              if (!ch) return;
              if (ch.kind !== "equal") this.sel = ch.index;
              if (this.foldable(ch)) { if (!this.expanded.delete(ch.index)) this.expanded.add(ch.index); }
              this.handle.focus();
              cx.notify();
            })))))
      .child(Scrollbar.vertical("merge-chunks").viewport_from_layout().absolute().inset_0());
    const editor = this.editing !== null && v_flex().flex_none().p(8).gap(6).border_t(1).border_color(th.divider ?? th.border)
      .child(div().text_size(11).text_color(th.muted).child(t("edit.title")))
      .child(div().id("edit-box").rounded(6).border_1().border_color(th.border).bg(th.background).px(8).py(6).text_size(12).cursor_text()
        .font_family(settings().font_family ?? "JetBrains Mono").on_mouse_down("left", (_e, cx) => cx.stop_propagation()).child(Textarea.new(this.edit).w_full()))
      .child(h_flex().gap(6)
        .child(this.button("edit-use", t("edit.use"), true, th, (cx) => this.useEdit(cx), true))
        .child(this.button("edit-cancel", t("cancel"), true, th, (cx) => this.stopEdit(cx))));
    return v_flex().flex_1().min_h(0)
      .child(this.fileToolbar(th))
      .child(this.chunkBar(th))
      .child(list)
      .when(Boolean(editor), (el) => el.child(editor));
  }

  render(cx) {
    // The sidebar opened this pane again (maybe on another file or repo).
    const sh = shared(KEY);
    if (sh?.cwd && (sh.at ?? 0) > this.at) this.start(sh, cx);
    const th = theme(), radius = (tokens().metrics ?? {}).pane_radius ?? 6;
    const frame = (...kids) => v_flex().id("merge").size_full().min_h(0).rounded(radius).overflow_hidden().bg(th.background).border_1().border_color(th.border)
      .text_color(th.foreground).text_size(12.5).track_focus(this.handle).on_key_down((e, cx) => this.key(e, cx)).children(kids.filter(Boolean));
    if (!this.spec) return frame(this.header(th), div().p(16).text_size(11.5).text_color(th.dim).child(t("empty")));
    if (!this.st) return frame(this.header(th), div().p(16).text_size(11.5).text_color(th.dim).child(this.error ?? t("loading")));
    return frame(this.header(th), this.file ? this.fileView(th) : this.listView(th), this.noteBar(th));
  }
}
