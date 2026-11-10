// The diff pane: one file, side by side or unified, with word highlights;
// stage, unstage or discard hunks or lines without touching the rest.
// Follows shared "yuzora.git.diff" ({cwd, on, path, orig, mode, rev?, to?, at}).
import { View, div } from "gpui-kit";
import { v_flex, h_flex, v_virtual_list, Scrollbar, VirtualListScrollHandle } from "gpui-base";
import { theme, props, shared, setShared, settings, run, panes, focused, closeOverlay } from "herdr";
import { t, tokens, session } from "yuzora:core";
import { open, changes, diffOpen, diffView, diffStage, diffUnstage, diffDiscard, settings as gitSettings } from "yuzora:git";

// Which pane an opener reuses. panes() lists the panes of every space and pane.focus selects
// the space of the pane it is given, so a pane is only looked for among the tabs of the shown
// space. The functions down to the end of this block are the same in every Git mod that has
// them (a mod cannot import another mod's files): scripts/yuzora-git-pane-reuse.test.ts keeps
// them equal.
// A diff, merge or rebase pane is asked in shared `<key>.<space id>`, so a space's pane keeps what
// its space asked for while another space asks for its own.
function requestKey(key, space) {
  return `${key}.${space}`;
}

// shared is kept in a file across runs, and a request to the plain `<key>` is an order given during
// one run. The first read of a run drops what an earlier run left there (beta.4 wrote every request
// to it), once for the three kinds, so no pane is shown it. One written after that is followed.
function retirePlain() {
  const run = session();
  if (shared("yuzora.git.run") === run) return;
  setShared("yuzora.git.run", run);
  for (const key of ["yuzora.git.diff", "yuzora.git.merge", "yuzora.git.rebase"]) if (shared(key) != null) setShared(key, null);
}

// What the shown space's pane is asked: its space's request, or the plain `<key>`, which names no
// space (an agent's, written during this run) and is for every pane. The newer one wins.
function requested(key) {
  retirePlain();
  const space = focused()?.space, mine = space ? shared(requestKey(key, space)) : null, any = shared(key);
  return (mine?.at ?? 0) >= (any?.at ?? 0) ? mine ?? any : any;
}
// ---- end of the shared pane helpers ----

const ROW = 20, CHAR = 7, GUTTER = 44, INDENT = 6;
// Side by side, the left side's share of the width: kept in shared state (a diff opens with new props every
// time, so a localStorage keyed by them would start over), dragged between these, 0.5 by default.
const SPLIT = "yuzora.git.diff.split", SPLIT_MIN = 0.15, SPLIT_MAX = 0.85;
// The grab area over the divider, the bar below the lines when they scroll sideways, and the room left
// after the widest line.
const GRAB = 5, BAR = 8, REST = 12;
// How wide a diff's lines are is found out a little at a time, so that nothing a frame or an event does grows with the
// diff. [`scan`] goes through the rows SLICE units of work at a time (a row costs STEP, and reading a line TOLL besides
// its length, and SLOW more for each character that is not ASCII): a frame that finds the rows not all gone through asks,
// out of sight, for the next slice, the first included. It works out what each line can take at most, in cells
// ([`bound`], reading LINE characters of a line at most), and puts the lines in the order of that, widest first (once
// all the rows are ranked, natively, a side at a time, ORDER units a line; a side whose turn comes with the slice used
// up waits for the next one); a line that three cells a character cannot make as wide as the FEW widest read so far is
// not read. Then each frame lays out
// the first lines in that order, out of sight, in the box the rows draw them in ([`GitDiff.probes`]): one a side in the
// first, GROW times as many as in the one before after it, and never more than FRAME in all, counted as [`cost`] says. A
// line that costs more than FRAME by itself is laid out alone, a side to a frame, up to LAYOUT characters or TEXTS
// pieces, past which it is not laid out at all. The range is the widest line laid out, and while lines are left that can
// be wider, the most the widest of them can take at most. A piece of a line is rounded up to a pixel of the window,
// which is at most HALF a cell for each piece but the first.
//
// The engine cuts off what runs past its time (50 ms in a frame, 500 in an event), and a cut leaves nothing behind but the
// mark of what was begun: a slice or a batch is marked when it starts and unmarked when it is back, so the one that starts
// after a mark still there knows the last was cut. A slice is then half what the cut one got done, a batch half its cost,
// and what is cut by itself, a row or a line, is taken at its upper bound. An event asks for the next frame before it
// works; a frame cannot ask, so a timer asks, LATE ms after the event that wanted the frame, if nothing came of it.
const GROW = 4, FRAME = 1 << 15, LAYOUT = 1 << 18, TEXTS = 1 << 12;
const LATE = 120;
const SLICE = 2e6, ORDER = 6, STEP = 100, TOLL = 200, SLOW = 25, LINE = 1 << 20, SORT = 1 << 16, FEW = 4, WORST = 3, HALF = 0.5;
// The width of a run of this over its length is what a cell takes in the font. The layout rounds that width up
// to a pixel, which on a cell is the less the longer the run.
const RULER = "W".repeat(1 << 16);
// Every length this pane writes is multiplied by the zoom, and what comes back, the size an element reports,
// where a pointer is and how far a wheel went, is in window pixels. The pane keeps and works out lengths in
// the unit it writes; this is where what comes back is turned into it, wherever it is read.
const unzoom = (px) => px / (Number(settings().zoom) || 1);
// The measurings begun so far: a measuring's elements are named by it, so one begun again for the same diff (it is read
// again) has its sizes reported though they are what the last reported.
let gauges = 0;

export default class GitDiff extends View {
  init(_props, cx) {
    const s = gitSettings(), p = props() ?? {};
    // Kept from a modal: the view it had there.
    this.layout = (p.layout ?? s.diffMode) === "unified" ? "unified" : "side";
    this.whitespace = p.whitespace ?? s.diffIgnoreWhitespace ?? "none";
    this.context = Number(p.context ?? s.diffContext ?? 3);
    this.view = null; this.error = null; this.busy = false; this.note = null;
    this.sel = {}; this.hunk = 0; this.askDiscard = null;
    this.scrollX = 0; this.rowsPx = 0; this.drag = null; this.fit = null; this.gauge = null; this.unit = null;
    this.scroll = VirtualListScrollHandle.new();
    this.handle = cx.focus_handle();
    // Props without `at` (an agent's tab.new) date from now, not from 1970.
    this.born = Date.now();
    this.follow(cx);
  }

  // The newest of the props and the shared request.
  wanted() {
    const p = props() ?? {};
    // In a modal, only what it was opened with.
    if (p.overlay) return p.path ? p : null;
    const s = requested("yuzora.git.diff");
    return s?.path && (!p.path || (s.at ?? 0) >= (p.at ?? this.born ?? 0)) ? s : p.path ? p : null;
  }

  follow(cx) {
    const spec = this.wanted();
    this.spec = spec;
    this.loop = (this.loop ?? 0) + 1;
    const loop = this.loop;
    this.view = null; this.fit = null; this.gauge = null; this.error = null; this.sel = {}; this.hunk = 0; this.note = null; this.scrollX = 0;
    // Called from render too: the state above is what this frame draws, so no notify here.
    if (!spec) return;
    cx.spawn(async (cx) => {
      try {
        const repo = await open(spec.cwd, spec.on ?? null);
        if (this.loop !== loop) return;
        this.repo = repo;
        await this.load(cx, loop);
        // Work tree and index diffs follow the repo, when a change may touch
        // the file ([`touches`]); commits never change.
        if (spec.mode !== "unstaged" && spec.mode !== "staged") return;
        let gen = 0;
        while (this.loop === loop) {
          const next = await changes(repo.id, gen);
          if (this.loop !== loop) break;
          if (gen && next.generation > gen && !this.busy && touches(next.paths, spec)) await this.load(cx, loop);
          gen = next.generation;
        }
      } catch (e) { if (this.loop === loop) { this.error = String(e); cx.notify(); } }
    });
  }

  async load(cx, loop = this.loop) {
    const spec = this.spec;
    try {
      const { id } = await diffOpen(this.repo.id, { path: spec.path, orig: spec.orig ?? null, mode: spec.mode ?? "unstaged", rev: spec.rev, to: spec.to ?? null });
      const view = await diffView(id, { layout: this.layout, context: this.context, whitespace: this.whitespace });
      if (this.loop !== loop) return;
      this.show(view); this.error = null;
      // Selections name lines of the old diff: drop the ones that moved.
      const keep = {};
      for (const [h, s] of Object.entries(this.sel)) if (view.hunks[h]) keep[h] = s;
      this.sel = keep;
      this.hunk = Math.min(this.hunk, Math.max(0, view.hunks.length - 1));
    } catch (e) { if (this.loop === loop) this.error = String(e); }
    cx.notify();
  }

  // Out of the modal into a pane (Git settings: beside the focused pane, or
  // a tab), shown as it is here.
  keep() {
    const { overlay, ...spec } = props() ?? {};
    placeDiff({ ...spec, at: Date.now(), layout: this.layout, whitespace: this.whitespace, context: this.context }, gitSettings().diffKeepIn);
    closeOverlay(overlay);
  }

  relayout(cx) {
    if (!this.view) return;
    cx.spawn(async (cx) => {
      try { this.show(await diffView(this.view.id, { layout: this.layout, context: this.context, whitespace: this.whitespace })); this.sel = {}; this.error = null; }
      catch (e) { this.error = String(e); }
      cx.notify();
    });
  }

  // Lines picked in a hunk: {hunk: {del: [line], add: [line]}} for the Rust side.
  picked() {
    const lines = {};
    for (const [h, s] of Object.entries(this.sel)) if (s.del.size || s.add.size) lines[h] = { del: [...s.del], add: [...s.add] };
    return lines;
  }

  apply(cx, kind, hunks, lines) {
    if (this.busy || !this.view) return;
    const fn = kind === "stage" ? diffStage : kind === "unstage" ? diffUnstage : diffDiscard;
    this.busy = true; this.note = null;
    cx.spawn(async (cx) => {
      try {
        const out = await fn(this.view.id, hunks, lines ?? {});
        if (out && out.ok === false) this.note = out.output || t("failed");
        this.sel = {};
      } catch (e) { this.note = String(e); }
      finally { this.busy = false; await this.load(cx); }
    });
    cx.notify();
  }

  toggleLine(h, side, index, cx) {
    const s = (this.sel[h] ??= { del: new Set(), add: new Set() });
    const set = side === "l" ? s.del : s.add;
    if (set.has(index)) set.delete(index); else set.add(index);
    cx.notify();
  }

  // Rows can't hold handlers (virtual list): a click on a fold opens the
  // whole file; on a changed line's number it picks the line; elsewhere on a
  // change it makes that hunk the current one.
  clickRow(i, cx) {
    const r = this.view?.rows[i];
    if (!r) return;
    if (r.k === "fold") { this.context = -1; this.relayout(cx); return; }
    if (r.h === undefined) return;
    const x = this.downX ?? -1;
    if (this.view.stageable) {
      if (this.layout === "unified") {
        if (x >= 0 && x < GUTTER * 2) return this.toggleLine(r.h, r.k === "-" ? "l" : "r", r.k === "-" ? r.li : r.ri, cx);
      } else {
        // Where the left side ends, as the last frame laid it out: its share of the row, less the divider.
        const left = ((this.rowsWidth ?? 0) - 1) * (this.lay?.ratio ?? 0.5);
        if (x >= 0 && x < GUTTER && r.l !== undefined) return this.toggleLine(r.h, "l", r.li, cx);
        if (x > left && x < left + 1 + GUTTER && r.r !== undefined) return this.toggleLine(r.h, "r", r.ri, cx);
      }
    }
    this.hunk = r.h; cx.notify();
  }

  // The width the rows are laid out in. It is kept as the box reported it, in window pixels: a new zoom changes
  // what it is in the unit of the pane without the box having changed size to report.
  get rowsWidth() {
    return unzoom(this.rowsPx);
  }

  // The left side's share of the width: the one kept, or the one being dragged.
  ratio() {
    return this.drag?.kind === "split" ? this.drag.ratio : ratioOf(shared(SPLIT));
  }

  // A diff's rows come in here: what the pane needs of their lines to draw is worked out from now on, once, not in
  // each frame, a slice at a time ([`ranked`]) from the first frame. None of it is done here: this is a task, and one the
  // engine cuts off would take the loop that follows the file with it. A row that is not what the rows are ends the diff
  // as an error.
  show(view) {
    this.view = view; this.gauge = null;
    this.fit = this.layout === "unified" ? { longest: Math.min(2000, view.rows.reduce((m, r) => Math.max(m, (r.t ?? "").length, (r.lt ?? "").length, (r.rt ?? "").length), 0)) } : ranking(view.rows);
  }

  // The next slice of the rows is ranked, when a frame has asked for it out of sight. The frame after it is asked for first,
  // so that a slice the engine cuts off still has one.
  ranked(fit, cx) {
    if (this.fit !== fit || fit.done) return;
    this.again(cx);
    if (fit.trying) cutSlice(fit);
    fit.tries++;
    const meter = fit.trying = { left: fit.slice, at: fit.at, slice: fit.slice };
    try { scan(fit, meter); fit.trying = null; } catch (e) { fit.trying = null; this.error = String(e); }
  }

  // The next frame is asked for, and asked for again if no event has asked since LATE ms: a frame the engine cuts off is not
  // drawn again by anything, and the events that would ask are in the frame it cut. A frame that is cut again is asked for
  // again, a smaller one, for as long as it is cut.
  again(cx) {
    cx.notify();
    this.late?.cancel();
    this.late = cx.timer.after(LATE, (cx) => {
      if (!this.waiting()) return;
      cx.notify();
      if (this.cutOff()) this.again(cx);
    });
  }

  // Whether the last slice or frame that was begun was not finished.
  cutOff() {
    const { fit, gauge: g } = this;
    return Boolean(fit?.trying || g && (g.l.trying || g.r.trying));
  }

  // Whether something is left to find out out of sight, which a frame asks for.
  waiting() {
    const { fit, gauge: g } = this;
    if (!fit?.l) return false;
    return !fit.done || Boolean(g) && (this.unit?.key !== g.key || ["l", "r"].some((name) => g[name].left === undefined || g[name].left >= 0 && !this.settled(name)));
  }

  // What measuring the lines has laid out so far, for the zoom and font they are drawn in now. A line is as wide as
  // the pane draws it, a piece at a time with each piece rounded up to a pixel of the window, so a new zoom or font
  // measures them again; what is left to lay out bounds the range meanwhile.
  gauged() {
    if (!this.fit?.l) return null;
    const key = this.gaugeKey();
    if (this.gauge?.key !== key) {
      const side = () => ({ left: undefined, got: 0, blind: 0, rounds: 0, frame: FRAME, trying: null, cuts: 0 });
      this.gauge = { id: ++gauges, key, l: side(), r: side() };
    }
    const g = this.gauge;
    if (this.fit.done) for (const name of ["l", "r"]) g[name].left ??= this.fit[name].keys.length - 1;
    return g;
  }

  gaugeKey() {
    return `${Number(settings().zoom) || 1}|${settings().font_family ?? "JetBrains Mono"}`;
  }

  // Whether the report of a size is for the measuring now under way: not for one a new view, zoom or font has ended.
  current(g) {
    return this.gauge === g && g.key === this.gaugeKey();
  }

  // The most, in cells, that the lines of a side that were not laid out can take: the next of them in line,
  // which can take the most of those left, and those passed over because they cannot be laid out. While the rows
  // are still being ranked, the most any line ranked so far can take.
  remain(name) {
    const g = this.gauge[name], rank = this.fit[name];
    if (!this.fit.done) return rank.top;
    return Math.max(g.blind, g.left >= 0 ? entry(rank, g.left).cells : 0);
  }

  // How wide the box of the widest line of a side that was not laid out may be, in the unit the pane writes
  // lengths in (a cell is not known till the ruler is laid out; the pieces of a line are in its cells).
  later(name) {
    const cells = this.remain(name);
    return cells ? INDENT + cells * (this.unit?.w ?? 0) : 0;
  }

  // Whether no line of a side that was not laid out can be more than a pixel wider than the widest that was.
  settled(name) {
    return this.unit?.key === this.gauge.key && this.later(name) <= this.gauge[name].got + unzoom(1);
  }

  // What the widest line of a side takes, in the box it is drawn in: as it was laid out, or while some lines are
  // left as much as the widest of them could take at most, if that is more.
  span(name) {
    const g = this.gauge?.[name];
    if (!g) return 0;
    if (this.settled(name)) return g.got;
    return Math.max(g.got, this.later(name));
  }

  // How far the lines can scroll sideways: the most the widest line of either side reaches past its
  // side's room (its cell less the numbers).
  reach(ratio) {
    const w = this.rowsWidth ?? 0, left = (w - 1) * ratio;
    if (!w) return 0;
    return Math.max(0, this.span("l") + REST - (left - GUTTER), this.span("r") + REST - (w - 1 - left - GUTTER));
  }

  // A sideways swipe or wheel moves every line the same distance from where the last one left them, also when it
  // comes before the frame that draws that one; the frame keeps the offset within [`reach`].
  // A swipe is sideways when it goes more that way than up or down: the list takes the rest, and a
  // little sideways in a vertical swipe does not drift the lines.
  wheel(e, cx) {
    if (!this.lay || Math.abs(e.delta.x) <= Math.abs(e.delta.y)) return;
    const at = Math.max(this.scrollX - unzoom(e.delta.x), 0);
    if (at !== this.scrollX) { this.scrollX = at; cx.notify(); }
  }

  // Where a press landed in the rows, across, for [`clickRow`] to tell a line's number from its text.
  pressed(e) {
    const x = e.local_position?.x;
    this.downX = x == null ? null : unzoom(x);
  }

  // The drag in progress follows the pointer: the divider by the share of the width it moved, the bar by
  // where its thumb is, which the offset is worked out from in each frame.
  dragTo(x, cx) {
    const d = this.drag, lay = this.lay;
    if (!d || !lay) return;
    if (d.kind === "split") {
      const ratio = ratioOf(d.from + unzoom(x - d.x) / Math.max(1, lay.width - 1));
      if (ratio !== d.ratio) { d.ratio = ratio; cx.notify(); }
    } else {
      const room = Math.max(1, lay.width - lay.thumb), at = Math.min(Math.max(d.from + unzoom(x - d.x), 0), room);
      if (at !== d.at) { d.at = at; cx.notify(); }
    }
  }

  // The drag is over, wherever it ended: the divider's share is kept.
  drop(cx) {
    const d = this.drag;
    this.drag = null;
    if (d?.kind === "split" && d.ratio !== this.ratio()) setShared(SPLIT, Math.round(d.ratio * 1000) / 1000);
    cx.notify();
  }

  // While a drag is on, the pane's root sees every move and the release, wherever the pointer is
  // inside the pane. Leaving the pane ends the drag: the release would not be seen out there.
  dragging(el) {
    return el.when(Boolean(this.drag), (root) => root
      .on_mouse_move((e, cx) => this.dragTo(e.position.x, cx))
      .on_mouse_up("left", (_e, cx) => this.drop(cx))
      .on_mouse_down("left", (_e, cx) => this.drop(cx))
      .on_hover((hovered, cx) => { if (!hovered) this.drop(cx); }));
  }

  // The row each hunk starts on, for ↑ ↓.
  starts() {
    const out = [];
    (this.view?.rows ?? []).forEach((r, i) => { if (r.h !== undefined && out[r.h] === undefined) out[r.h] = i; });
    return out;
  }

  go(cx, step) {
    const n = this.view?.hunks.length ?? 0;
    if (!n) return;
    this.hunk = (this.hunk + step + n) % n;
    const at = this.starts()[this.hunk];
    if (at !== undefined) this.scroll.scroll_to_item(Math.max(0, at - 3), "top");
    cx.notify();
  }

  key(e, cx) {
    if (e.key === "f7" || e.key === "n" || e.key === "down" && e.modifiers?.alt) { this.go(cx, e.modifiers?.shift ? -1 : 1); cx.stop_propagation(); }
    else if (e.key === "p" || e.key === "up" && e.modifiers?.alt) { this.go(cx, -1); cx.stop_propagation(); }
    else if (e.key === "s" && this.view?.stageable && this.spec?.mode === "unstaged") { this.apply(cx, "stage", Object.keys(this.picked()).length ? [] : [this.hunk], this.picked()); cx.stop_propagation(); }
    else if (e.key === "u" && this.view?.stageable && this.spec?.mode === "staged") { this.apply(cx, "unstage", Object.keys(this.picked()).length ? [] : [this.hunk], this.picked()); cx.stop_propagation(); }
  }

  segments(segs, text, color, strong) {
    if (!segs || !segs.length) return div().whitespace_nowrap().text_color(color).child(text || " ");
    return h_flex().whitespace_nowrap().text_color(color)
      .children(segs.map((s, i) => div().id(`s${i}`).when(s.h, (el) => el.bg(strong).rounded(2)).child(s.t || "")));
  }

  // The box a line's text is drawn in. The cell draws it and the measuring of the lines lays it out ([`probes`]):
  // how wide a line is is how wide this is, whatever it is cut into.
  line(r, side, th, d) {
    const del = side === "l", text = del ? r.lt ?? r.t : r.rt ?? r.t;
    const strong = del ? `${d.removed ?? "#f08a8a"}55` : `${d.added ?? "#86d17a"}55`;
    return div().flex_none().pl(INDENT).child(r[side] !== undefined ? this.segments(r[`${side}s`], text, th.foreground, strong) : "");
  }

  // One side's cell: number (click picks the line), then the text, which `sx` moves under the cell's
  // edge. `grow` is its share of the row, the same for every row.
  cell(r, side, th, d, grow, sx) {
    const n = r[side];
    const changedRow = r.k === "~" || r.k === "-" || r.k === "+";
    const present = n !== undefined;
    const del = side === "l";
    const bg = !changedRow || !present ? null : del ? d.removed_bg : d.added_bg;
    const index = del ? r.li : r.ri;
    const picked = changedRow && present && this.sel[r.h]?.[del ? "del" : "add"]?.has(index);
    const canPick = changedRow && present && this.view.stageable;
    return h_flex().w(0).flex_grow(grow).min_w(0).h_full().when(Boolean(bg), (el) => el.bg(bg)).when(changedRow && !present, (el) => el.bg(th.raised))
      .child(div().w(GUTTER).flex_none().h_full().pr(6).text_right().text_color(picked ? th.foreground : th.dim)
        .when(picked, (el) => el.bg(del ? d.removed ?? th.ansi1 : d.added ?? th.ansi2).text_color(th.background))
        .when(canPick, (el) => el.cursor_pointer().hover((s) => s.text_color(th.foreground)))
        .child(present ? String(n) : ""))
      .child(h_flex().flex_1().min_w(0).h_full().overflow_hidden().child(this.line(r, side, th, d).ml(-sx)));
  }

  row(r, i, th, d, width) {
    if (r.k === "fold") {
      return h_flex().id(`r${i}`).h(ROW).px(10).items_center().bg(th.raised).text_color(th.dim).cursor_pointer()
        .hover((s) => s.text_color(th.foreground)).child(t("fold", { count: r.n }));
    }
    const current = r.h !== undefined && r.h === this.hunk;
    const base = h_flex().id(`r${i}`).h(ROW).w_full();
    if (this.layout === "unified") {
      const del = r.k === "-", add = r.k === "+";
      const bg = del ? d.removed_bg : add ? d.added_bg : null;
      const index = del ? r.li : r.ri;
      const side = del ? "del" : "add";
      const picked = (del || add) && this.sel[r.h]?.[side]?.has(index);
      const canPick = (del || add) && this.view.stageable;
      return base.when(current, (el) => el.border_l(2).border_color(th.accent)).when(Boolean(bg), (el) => el.bg(bg))
        .child(div().w(GUTTER * 2).flex_none().h_full().flex().text_color(picked ? th.background : th.dim)
          .when(picked, (el) => el.bg(del ? d.removed ?? th.ansi1 : d.added ?? th.ansi2))
          .when(canPick, (el) => el.cursor_pointer())
          .child(div().w(GUTTER).pr(6).text_right().child(r.l !== undefined ? String(r.l) : ""))
          .child(div().w(GUTTER).pr(6).text_right().child(r.r !== undefined ? String(r.r) : "")))
        .child(div().w(14).flex_none().text_color(del ? d.removed : add ? d.added : th.dim).child(del ? "−" : add ? "+" : ""))
        .child(div().flex_1().min_w(width).child(this.segments(r.s, r.t, th.foreground, del ? `${d.removed}55` : `${d.added}55`)));
    }
    // The current hunk's mark sits over the row's edge: a border would move this row's divider off the others'.
    const lay = this.lay;
    return base.relative().child(this.cell(r, "l", th, d, lay.l, lay.sx)).child(div().w(1).flex_none().h_full().bg(th.border)).child(this.cell(r, "r", th, d, lay.r, lay.sx))
      .when(current, (el) => el.child(div().absolute().top(0).left(0).w(2).h_full().bg(th.accent)));
  }

  // The bar over the divider to grab: laid out with the same shares as the rows, so it is where
  // their divider is. A press starts a drag (a double press puts the default back); the pane's
  // root follows the pointer and ends it (see dragging()).
  splitter(th) {
    const lay = this.lay;
    return h_flex().absolute().inset_0()
      .child(div().w(0).flex_grow(lay.l).min_w(0))
      .child(div().w(1).flex_none().h_full().relative()
        .child(div().id("diff-split").absolute().top(0).left(-(GRAB - 1) / 2).w(GRAB).h_full().cursor_col_resize()
          .when(this.drag?.kind === "split", (el) => el.bg(th.accent).opacity(0.5)).hover((s) => s.bg(th.hover))
          .on_mouse_down("left", (e, cx) => {
            cx.stop_propagation();
            if (e.click_count >= 2) { this.drag = null; setShared(SPLIT, null); }
            else this.drag = { kind: "split", x: e.position.x, from: lay.ratio, ratio: lay.ratio };
            cx.notify();
          })))
      .child(div().w(0).flex_grow(lay.r).min_w(0));
  }

  // The bar under the lines when they scroll sideways; its thumb drags.
  hbar(th) {
    const lay = this.lay, thumb = lay.thumb;
    return div().absolute().left(0).bottom(0).w_full().h(BAR)
      .child(div().id("diff-thumb").absolute().top(2).left(Math.round((lay.width - thumb) * lay.sx / lay.reach)).w(thumb).h(BAR - 4).rounded(3).bg(th.dim)
        .opacity(this.drag?.kind === "bar" ? 0.9 : 0.5).cursor_pointer().hover((s) => s.opacity(0.9))
        .on_mouse_down("left", (e, cx) => {
          cx.stop_propagation();
          const from = (lay.width - thumb) * lay.sx / lay.reach;
          this.drag = { kind: "bar", x: e.position.x, from, at: from };
          cx.notify();
        }));
  }

  // The next lines of a side to lay out, widest first by what each can take at most, as many as a frame lays out,
  // and where they start in the order. A line that costs a frame by itself is a batch of its own, unless it is past
  // what can be laid out: those are passed over, and kept in `blind`.
  batch(name) {
    const g = this.gauge[name], rank = this.fit[name], { text, pieces } = SIDES[name], lines = [];
    let used = 0;
    for (let at = g.left; at >= 0 && lines.length < GROW ** g.rounds && used <= g.frame; at--) {
      const { cells, r } = entry(rank, at), long = (r[text] ?? r.t).length, cut = r[pieces]?.length || 1, price = cost(long, cut);
      if (long > LAYOUT || cut > TEXTS) {
        if (lines.length) break;
        g.blind = Math.max(g.blind, cells);
        g.left = at - 1;
        continue;
      }
      if (lines.length && used + price > g.frame) break;
      lines.push(r);
      used += price;
    }
    return { from: g.left, lines, cost: used };
  }

  // The frame that asked for a batch of a side was cut off: the batch after it is half its cost, and a line that was cut
  // by itself is passed over, at its upper bound like those that cannot be laid out.
  cutBatch(name) {
    const s = this.gauge[name], batch = s.trying;
    s.trying = null; s.cuts++;
    if (batch.lines.length > 1) { s.frame = Math.max(1, batch.cost >> 1); return; }
    s.blind = Math.max(s.blind, entry(this.fit[name], batch.from).cells);
    s.left = batch.from - 1;
  }

  // A batch has been laid out: the widest of its lines is the widest so far if it is, and the next frame asks for the next.
  measured(g, name, batch, size, cx) {
    const s = g[name];
    if (!this.current(g) || s.left !== batch.from) return;
    const width = unzoom(size.width);
    if (width > s.got) s.got = width;
    s.left = batch.from - batch.lines.length;
    s.rounds++;
    this.again(cx);
  }

  // What is done out of sight, in the very box the cells draw lines in ([`line`]), in the font and size of the rows: the
  // next slice of the rows is ranked, if they are not all, as the size of an element that is new each time says; then a
  // batch of each side is laid out while some lines of it may be wider than the widest laid out so far, and a run of
  // ruler, which says what a cell is for the lines that are not. Each reports its width, which is how far the sides
  // scroll; they go away once nothing is left to find out.
  probes(th, d) {
    const g = this.gauge, fit = this.fit;
    if (!g) return div();
    const hidden = () => v_flex().absolute().top(0).left(0).h(0).overflow_hidden();
    const out = [];
    let heavy = false;
    if (!fit.done) out.push(hidden().id(`rank-${fit.at}-${fit.tries}`).on_size_change((_size, cx) => this.ranked(fit, cx)).child(div().w(1)));
    else for (const name of ["l", "r"]) {
      if (g[name].trying) this.cutBatch(name);
      if (this.settled(name)) continue;
      const batch = this.batch(name);
      if (!batch.lines.length || batch.cost > FRAME && heavy) continue;
      if (batch.cost > FRAME) heavy = true;
      g[name].trying = batch;
      out.push(hidden().id(`gauge-${g.id}-${name}-${batch.from}-${g[name].cuts}`)
        .on_size_change((size, cx) => this.measured(g, name, batch, size, cx))
        .children(batch.lines.map((r) => h_flex().child(this.line(r, name, th, d)))));
    }
    if (this.unit?.key !== g.key) {
      out.push(hidden().id(`ruler-${g.key}`)
        .on_size_change((size, cx) => {
          const w = unzoom(size.width) / RULER.length;
          if (this.current(g) && w > 0) { this.unit = { key: g.key, w }; cx.notify(); }
        })
        .child(div().whitespace_nowrap().child(RULER)));
    }
    return div().children(out);
  }

  // Side by side: the sides share the width by one ratio whatever their lines hold, each clipping its
  // own, and the lines scroll sideways together.
  sideBody(v, th, d, mono) {
    this.gauged();
    const ratio = this.ratio(), width = this.rowsWidth ?? 0, reach = this.reach(ratio);
    // What is kept, a wheel's or a thumb's reach past the end included, is brought within the room there is
    // now: a pane made narrower or a divider moved leaves the lines where they are, and does not give them
    // what was asked for beyond the end.
    // A thumb that is held stays under the pointer when the range changes: the offset follows the thumb. A diff read again
    // keeps the offset till its first slice is ranked, when there is a range to bring it within.
    const thumb = Math.max(24, Math.round(width * width / (width + reach))), room = Math.max(1, width - thumb);
    const known = this.fit.at > 0 || this.fit.done;
    this.scrollX = this.drag?.kind === "bar" ? reach * Math.min(this.drag.at, room) / room : known ? Math.min(this.scrollX, reach) : this.scrollX;
    this.lay = { ratio, width, reach, thumb, sx: this.scrollX, l: ratio * 100, r: (1 - ratio) * 100 };
    return div().flex_1().min_h(0).relative().overflow_hidden().font_family(mono).text_size(11.5)
      .child(div().id("diff-rows-box").size_full().pb(reach > 0 ? BAR : 0)
        .on_size_change((size, cx) => { if (Math.abs(size.width - this.rowsPx) > 0.5) { this.rowsPx = size.width; cx.notify(); } })
        .on_mouse_down("left", (e) => this.pressed(e))
        .on_scroll_wheel((e, cx) => this.wheel(e, cx))
        .child(this.rowList(v, th, d, 0)))
      .child(Scrollbar.vertical("diff-rows").viewport_from_layout().absolute().top(0).left(0).right(0).bottom(reach > 0 ? BAR : 0))
      .child(this.splitter(th))
      .when(reach > 0, (el) => el.child(this.hbar(th)))
      .child(this.probes(th, d));
  }

  rowList(v, th, d, width) {
    return v_virtual_list("diff-rows", v.rows.length, ROW, (i) => String(i),
      (range) => v.rows.slice(range.start, range.end).map((r, k) => this.row(r, range.start + k, th, d, width)))
      .on_item_click((key, cx) => this.clickRow(Number(key), cx))
      .track_scroll(this.scroll).size_full();
  }

  pill(id, label, on, th, action) {
    return div().id(id).h(18).px(6).flex().items_center().rounded(3).text_size(11).cursor_pointer()
      .bg(on ? th.selected : th.raised).text_color(on ? th.foreground : th.muted).hover((s) => s.text_color(th.foreground))
      .on_click((_e, cx) => action(cx)).child(label);
  }

  button(id, label, th, enabled, action) {
    return div().id(id).h(20).px(8).flex().items_center().rounded(4).border_1().border_color(th.border).text_size(11)
      .text_color(enabled ? th.foreground : th.dim).when(enabled, (el) => el.cursor_pointer().hover((s) => s.bg(th.hover)).on_click((_e, cx) => action(cx)))
      .child(label);
  }

  render(cx) {
    const th = theme(), d = tokens().diff ?? {};
    const want = this.wanted();
    if (JSON.stringify(want ?? null) !== JSON.stringify(this.spec ?? null)) this.follow(cx);
    const spec = this.spec, v = this.view;
    const mono = settings().font_family ?? "JetBrains Mono", overlay = props()?.overlay;
    const root = this.dragging(v_flex().id("git-diff").size_full().min_h(0).bg(th.background).text_color(th.foreground).text_size(12)
      .track_focus(this.handle).on_key_down((e, cx) => this.key(e, cx)).on_mouse_down("left", () => this.handle.focus()));
    if (!spec) return root.child(div().p(16).text_color(th.dim).child(t("empty")));

    const modeLabel = spec.mode === "staged" ? t("mode.staged") : spec.mode === "commit" ? t("mode.commit", { rev: (spec.rev ?? "").slice(0, 8) })
      : spec.mode === "compare" ? t("mode.compare", { a: spec.rev ?? "", b: spec.to ?? t("worktree") }) : t("mode.unstaged");
    const picked = this.picked(), npicked = Object.values(picked).reduce((n, s) => n + s.del.length + s.add.length, 0);
    const stageable = Boolean(v?.stageable) && !this.busy;
    const hunks = v?.hunks.length ?? 0;
    // A change of mode alone (a file made a program): the whole file goes.
    const modeOnly = Boolean(v?.modes) && hunks === 0;
    const label = (act) => npicked ? t(`${act}.lines`, { count: npicked }) : t(modeOnly ? `${act}.file` : `${act}.hunk`);
    const header = h_flex().h(28).px(8).gap(7).flex_none().items_center().border_b(1).border_color(th.border).text_size(12)
      .child(div().font_semibold().child(t("title")))
      .child(div().min_w(0).truncate().text_color(th.muted).child(spec.orig && spec.orig !== spec.path ? `${spec.orig} → ${spec.path}` : spec.path))
      .when(Boolean(v), (el) => el.child(div().text_size(11.5).text_color(d.added ?? th.ansi2).child(`+${v.add}`)).child(div().text_size(11.5).text_color(d.removed ?? th.ansi1).child(`−${v.del}`)))
      .child(div().flex_none().text_size(11.5).text_color(th.dim).child(modeLabel))
      .child(div().flex_1())
      .child(h_flex().p(2).gap(2).rounded(5).bg(th.raised)
        .child(this.pill("side", t("layout.side"), this.layout === "side", th, (cx) => { this.layout = "side"; this.relayout(cx); }))
        .child(this.pill("unified", t("layout.unified"), this.layout === "unified", th, (cx) => { this.layout = "unified"; this.relayout(cx); })))
      .child(this.pill("ws", t(`ws.${this.whitespace}`), this.whitespace !== "none", th, (cx) => {
        this.whitespace = { none: "trim", trim: "all", all: "none" }[this.whitespace] ?? "none"; this.relayout(cx);
      }))
      .child(this.pill("ctx", this.context < 0 ? t("context.all") : t("context.n", { n: this.context }), this.context < 0, th, (cx) => {
        this.context = this.context < 0 ? 3 : -1; this.relayout(cx);
      }))
      .child(this.pill("prev", "↑", false, th, (cx) => this.go(cx, -1)))
      .child(div().text_size(11).text_color(th.dim).child(hunks ? `${this.hunk + 1}/${hunks}` : "0/0"))
      .child(this.pill("next", "↓", false, th, (cx) => this.go(cx, 1)))
      .when(spec.mode === "unstaged", (el) => el
        .child(this.button("stage", label("stage"), th, stageable && (hunks > 0 || modeOnly), (cx) => this.apply(cx, "stage", npicked ? [] : [this.hunk], picked)))
        .child(this.button("discard", label("discard"), th, stageable && (hunks > 0 || modeOnly), (cx) => { this.askDiscard = { hunks: npicked ? [] : [this.hunk], lines: picked }; cx.notify(); })))
      .when(spec.mode === "staged", (el) => el
        .child(this.button("unstage", label("unstage"), th, stageable && (hunks > 0 || modeOnly), (cx) => this.apply(cx, "unstage", npicked ? [] : [this.hunk], picked))))
      .child(this.button("blame", t("annotate"), th, Boolean(this.repo), () => { run("tab.new", { name: `${t("annotate")} · ${spec.path.split("/").pop()}`, mod: "yuzora-git-blame", props: { cwd: spec.cwd, on: spec.on ?? null, path: spec.path, rev: spec.mode === "commit" ? spec.rev : null } }); if (overlay) closeOverlay(overlay); }))
      .child(this.button("history", t("history"), th, Boolean(this.repo), () => { run("tab.new", { name: `${t("history")} · ${spec.path.split("/").pop()}`, mod: "yuzora-git-log", props: { cwd: spec.cwd, on: spec.on ?? null, filter: { paths: [spec.path], follow: true } } }); if (overlay) closeOverlay(overlay); }))
      // In a modal: Keep moves it into a pane; ✕ (or Esc, or a click beside it) closes it.
      .when(Boolean(overlay), (el) => el.child(this.button("keep", t("keep"), th, true, () => this.keep())).child(this.button("close", "✕", th, true, () => closeOverlay(overlay))));

    let body;
    if (this.error) body = div().p(16).text_color(th.ansi1).child(this.error);
    else if (!v) body = div().p(16).text_color(th.dim).child(t("loading"));
    else if (v.tooLarge) body = div().p(16).text_color(th.dim).child(t("tooLarge"));
    else if (v.binary) body = div().p(16).text_color(th.dim).child(t("binary"));
    else if (v.identical || !v.rows.length) body = div().p(16).text_color(th.dim).child(!v.leftExists && !v.rightExists ? t("missing") : v.modes ? t("modes", { from: v.modes[0], to: v.modes[1] }) : t("identical"));
    else if (this.layout === "unified") {
      const width = (this.fit?.longest ?? 0) * CHAR + 24;
      body = div().flex_1().min_h(0).relative().font_family(mono).text_size(11.5)
        .child(div().id("diff-x").size_full().overflow_x_scrollbar()
          .child(div().id("diff-rows-box").w_full().min_w(GUTTER * 2 + 14 + width).h_full()
            .on_size_change((size) => { this.rowsPx = size.width; })
            .on_mouse_down("left", (e) => this.pressed(e))
            .child(this.rowList(v, th, d, width))))
        .child(Scrollbar.vertical("diff-rows").viewport_from_layout().absolute().inset_0());
    } else body = this.sideBody(v, th, d, mono);

    const ask = this.askDiscard && h_flex().px(10).py(6).gap(8).items_center().bg((tokens().attention ?? {}).background ?? th.raised).text_size(11.5)
      .child(div().flex_1().child(t("discard.ask")))
      .child(this.button("discard-yes", t("discard"), th, true, (cx) => { const a = this.askDiscard; this.askDiscard = null; this.apply(cx, "discard", a.hunks, a.lines); }))
      .child(this.button("discard-no", t("cancel"), th, true, (cx) => { this.askDiscard = null; cx.notify(); }));

    const out = root.child(header)
      .when(Boolean(ask), (el) => el.child(ask))
      .when(Boolean(this.note), (el) => el.child(div().px(10).py(4).text_size(11.5).text_color(th.ansi1).child(this.note)))
      .when(Boolean(v?.modes) && v.rows.length > 0, (el) => el.child(div().px(10).py(3).text_size(11).text_color(th.muted).child(t("modes", { from: v.modes[0], to: v.modes[1] }))))
      .when(Boolean(v) && !v.stageable && (spec.mode === "unstaged" || spec.mode === "staged") && this.whitespace !== "none", (el) => el.child(div().px(10).py(3).text_size(11).text_color(th.dim).child(t("wsNoStage"))))
      .child(body);
    // This frame was not cut off: the batches it asked for are not marked any more.
    if (this.gauge) this.gauge.l.trying = this.gauge.r.trying = null;
    return out;
  }
}

// A kept or dragged share of the width, in range; anything else (nothing kept, a damaged file) is half.
function ratioOf(v) {
  return typeof v === "number" && Number.isFinite(v) ? Math.min(SPLIT_MAX, Math.max(SPLIT_MIN, v)) : 0.5;
}

// What a UTF-16 unit of a line takes at most, in cells (the width of an ASCII character), from what
// JetBrains Mono and what the system falls back to for the rest draw (scripts/fixtures/yuzora-git-diff-font-widths.json,
// held to by yuzora-git-diff-layout.test.ts): Latin, Greek, Cyrillic, punctuation, arrows and box drawing take 1;
// Chinese, Japanese, full width 1.67 and Korean 1.44; a symbol drawn as an emoji 2.17, and one past U+FFFF is two
// units. Each is rounded up with some to spare, and what is not listed is taken at [`WORST`].
// This is never a width. It says in what order the lines are laid out and when to stop (when what is left cannot be
// wider than what was laid out), so a cell too many here costs a line laid out, not a pixel of range. What it must
// not be is too few: a character that takes more than its class says can end the laying out while a line with it in
// is wider than the widest laid out, by that character's excess.
const ASCII = /[\0-\x7f]+/g;
const NARROW = /[^\x80-\u058f\u2000-\u206f\u2190-\u22ff\u2500-\u259f]+/g;
const WIDE = /[^\u1100-\u115f\u2e80-\ua4cf\uac00-\ud7a3\uf900-\ufaff\ufe30-\ufe6f\uff00-\uff60\uffe0-\uffe6]+/g;
const PAIR = /[^\ud800-\udfff]+/g;

// The cells a line takes at most, reading its first [`LINE`] characters (the rest is taken at [`WORST`] a unit, as is
// every character that is not ASCII in a line with more than [`SORT`] of them). What the reading costs, [`TOLL`] and
// [`SLOW`], is taken from `meter`.
function bound(s, meter) {
  const n = s.length, read = Math.min(n, LINE), head = read < n ? s.slice(0, read) : s;
  const other = head.replace(ASCII, "");
  meter.left -= TOLL + read + SLOW * other.length;
  let cells = read - other.length + WORST * (n - read);
  if (other.length > SORT) return cells + WORST * other.length;
  if (other.length) {
    const count = (re) => other.replace(re, "").length;
    const narrow = count(NARROW), wide = count(WIDE), pair = count(PAIR);
    cells += 1.25 * narrow + 2 * wide + 1.5 * pair + WORST * (other.length - narrow - wide - pair);
  }
  return cells;
}

// Where in a row each side's line is: its text (an unchanged row has one, `t`, for both), and the pieces it is drawn in
// (one text if there are none).
const SIDES = {
  l: { text: "lt", pieces: "ls" },
  r: { text: "rt", pieces: "rs" },
};

// What laying out a line costs, in the characters it has and the texts it is drawn in.
function cost(chars, pieces) {
  return chars + 16 * pieces + 64;
}

// Every line of a side in the order they are laid out in, widest first by what each can take at most, in cells:
// [`bound`] of its text, and what its pieces may be rounded up by. The order is a sort of numbers (a bound in quarters
// of a cell above, the row below, in one number), so what a frame goes through of it is the few lines it takes off the
// end, and nothing depends on how many rows there are. How wide the lines are is found by laying them out in that
// order: what the table of cells says is wrong costs lines to lay out, not the range. A line that is the same on both
// sides (an unchanged row) is read once.
function ranking(rows) {
  const count = rows.length, base = 2 ** Math.ceil(Math.log2(count + 1));
  const side = () => ({ keys: new Float64Array(count), n: 0, few: [], floor: -1, top: 0, base, rows });
  return { rows, count, base, at: 0, done: false, slice: SLICE, trying: null, tries: 0, hurt: -1, l: side(), r: side() };
}

// Ranks the rows from where `fit` left off until `meter` has nothing left, and puts the lines of a side in order when all
// the rows have been (a side whose turn comes with the meter used up is left for the next call).
// A line is read in full if it can beat the FEW widest read so far, else it is taken at three cells a unit.
// This can be cut off anywhere, and the next call goes on from the last row it finished. The engine looks at the clock where
// the script calls or branches, so a row is written in places that doing it again writes the same in, and the counts that
// make it count, `fit.at` last, are set together with nothing in between to call or branch.
function scan(fit, meter) {
  const { rows, count, base, l, r } = fit;
  while (fit.at < count && meter.left > 0) {
    const i = fit.at, row = rows[i], t = row.t, lt = row.lt ?? t, rt = row.rt ?? t, hurt = i === fit.hurt;
    meter.left -= STEP;
    let same = 0, lc = 0, rc = 0, readL = false, readR = false;
    if (lt) {
      const cut = row.ls?.length;
      lc = WORST * lt.length + (cut > 1 ? HALF * (cut - 1) : 0);
      if (!hurt && lc > l.floor) { lc = bound(lt, meter) + (cut > 1 ? HALF * (cut - 1) : 0); readL = true; }
      if (rt === lt && !row.ls && !row.rs) same = lc;
    }
    if (rt) {
      const cut = row.rs?.length;
      rc = same || WORST * rt.length + (cut > 1 ? HALF * (cut - 1) : 0);
      if (same) readR = rc > r.floor;
      else if (!hurt && rc > r.floor) { rc = bound(rt, meter) + (cut > 1 ? HALF * (cut - 1) : 0); readR = true; }
    }
    if (readL) keep(l, lc);
    if (readR) keep(r, rc);
    const ln = l.n, rn = r.n, dl = lt ? 1 : 0, dr = rt ? 1 : 0;
    l.keys[ln] = lc * 4 * base + i;
    r.keys[rn] = rc * 4 * base + i;
    if (lc > l.top) l.top = lc;
    if (rc > r.top) r.top = rc;
    l.n = ln + dl; r.n = rn + dr; fit.at = i + 1;
  }
  if (fit.at === count) {
    for (const side of [l, r]) {
      if (side.sorted || meter.left <= 0) continue;
      side.keys = side.keys.subarray(0, side.n).sort();
      side.sorted = true;
      meter.left -= ORDER * side.n;
    }
    fit.done = l.sorted && r.sorted;
  }
}

// The slice before this one was cut off by the engine. If it got any row done, the next is half of what it did, or else the
// row it began with cannot be read in the time there is, and is taken at its upper bound.
function cutSlice(fit) {
  const { left, at, slice } = fit.trying;
  fit.trying = null;
  if (fit.at === at) fit.hurt = at;
  else fit.slice = Math.max(1, Math.min(slice - left, slice) >> 1);
}

// The few widest bounds a side has read, which a line has to beat to be read.
function keep(side, cells) {
  side.few.push(cells);
  side.few.sort((a, b) => b - a);
  while (side.few.length > FEW) side.few.pop();
  if (side.few.length === FEW) side.floor = side.few[FEW - 1];
}

// The line at a place in a rank: its row, and the most it can take, in cells.
function entry(rank, at) {
  const key = rank.keys[at], q = Math.floor(key / rank.base);
  return { cells: q / 4, r: rank.rows[key - q * rank.base] };
}

// Whether changed paths may touch the diff's file: the file, the one it was renamed from, a
// folder above either (the top too: `.`), anything inside one (a submodule), or attributes
// (how git diffs it). Compared as [`plain`] writes them. A part with `~` and a digit may be
// a Windows short name for any file, and paths null (the index or refs git wrote, a rescan)
// may be anything: it can't be told, so they may.
function touches(paths, spec) {
  if (!paths) return true;
  const mine = [spec.path, spec.orig].filter(Boolean).map(plain);
  return paths.some((p) => {
    const q = plain(p);
    return q === "" || q === "." || /~\d/.test(q) || q === ".gitattributes" || q.endsWith("/.gitattributes") || mine.some((m) => m === q || m.startsWith(`${q}/`) || q.startsWith(`${m}/`));
  });
}

// A path as the watcher, git and a pathspec may each write it, one way: `\` as `/` (the
// watcher's), without case (a file system that ignores it), in NFC (macOS hands names as
// they are on disk, often NFD; git keeps them NFC), no `./` before or `/` after.
function plain(p) {
  let q = p.replaceAll("\\", "/").toLowerCase().normalize("NFC");
  while (q.startsWith("./")) q = q.slice(2);
  while (q.endsWith("/")) q = q.slice(0, -1);
  return q;
}

// A diff pane beside the focused pane, or in a tab of its own (Git settings).
function placeDiff(spec, where) {
  const pane = where !== "tab" && focused()?.pane;
  if (pane) run("pane.split", { direction: "right", pane, mod: "yuzora-git-diff", props: spec });
  else run("tab.new", { name: t("tab.diff"), mod: "yuzora-git-diff", props: spec });
}
