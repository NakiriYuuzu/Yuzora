// The diff pane: one file, side by side or unified, its code coloured as the editor colours it, with word highlights;
// stage, unstage or discard hunks or lines without touching the rest; select lines (by a click or a Shift-click) or
// characters (by dragging across them) and copy them, a hunk or a patch. Follows shared "yuzora.git.diff" ({cwd, on, path, orig, mode, rev?, to?, at}).
// The same class draws the menu a right-click opens (props.menu), as the Git log does.
import { View, div } from "gpui-kit";
import { v_flex, h_flex, v_virtual_list, Scrollbar, VirtualListScrollHandle } from "gpui-base";
import { theme, props, shared, setShared, settings, run, panes, focused, closeOverlay, openOverlay, notify } from "herdr";
import { t, tokens, session } from "yuzora:core";
import { open, changes, diffOpen, diffView, diffText, diffPatch, diffStage, diffUnstage, diffDiscard, DiffLine, diffLook, diffHit, settings as gitSettings } from "yuzora:git";

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
// Dragging across rows: a press is a drag once the pointer has gone SLOP px from where it landed. Then the pointer is looked at every
// TICK ms, and within EDGE of the list's top or bottom (or past it) the list scrolls by a row, and by one more for each ROW further
// out, up to MOST rows.
const TICK = 50, EDGE = ROW, MOST = 8, SLOP = 4;
// The menu a right-click opens: three rows of MENU_ROW in MENU_PAD, so MENU_HEIGHT.
const MENU_W = 210, MENU_ROW = 28, MENU_PAD = 4, MENU_HEIGHT = 3 * MENU_ROW + 2 * MENU_PAD;
// How wide a diff's lines are is found out a little at a time, so that nothing a frame or an event does grows with the
// diff. [`scan`] goes through the rows SLICE units of work at a time (a row costs STEP, and reading a line TOLL besides
// its length, and SLOW more for each character that is not ASCII): a frame that finds the rows not all gone through asks,
// out of sight, for the next slice, the first included. It works out what each line can take at most, in cells
// ([`bound`], reading LINE characters of a line at most), and puts the lines in the order of that, widest first (once
// all the rows are ranked, natively, a side at a time, ORDER units a line; a side whose turn comes with the slice used
// up waits for the next one); a line that WORST cells a character cannot make as wide as the FEW widest read so far is
// not read. Then each frame lays out
// the first lines in that order, out of sight, in the box the rows draw them in ([`GitDiff.probes`]): one a side in the
// first, GROW times as many as in the one before after it, and never more than FRAME in all, counted as [`cost`] says. A
// line that costs more than FRAME by itself is laid out alone, a side to a frame, up to LAYOUT characters, past which it
// is not laid out at all. The range is the widest line laid out, and while lines are left that can be wider, the most the
// widest of them can take at most. A line is one text ([`DiffLine`]), rounded up to a pixel of the window once.
//
// The engine cuts off what runs past its time (50 ms in a frame, 500 in an event), and a cut leaves nothing behind but the
// mark of what was begun: a slice or a batch is marked when it starts and unmarked when it is back, so the one that starts
// after a mark still there knows the last was cut. A slice is then half what the cut one got done, a batch half its cost,
// and what is cut by itself, a row or a line, is tried once more and then taken at its upper bound (a machine that stops for a
// moment cuts whatever it is doing, which says nothing of that). Both grow back, doubling each time one is not cut, but
// never to the size that was cut: half of that is the most they are asked for from then on. When both sides had a batch
// in the frame that was cut, the one whose batch cost more is the cause. An event asks for the next frame before it
// works; a frame cannot ask, so a timer asks, LATE ms after the event that wanted the frame, if nothing came of it.
const GROW = 4, FRAME = 1 << 15, LAYOUT = 1 << 18;
const LATE = 120, RETRIES = 40;
const SLICE = 2e6, ORDER = 6, STEP = 100, TOLL = 200, SLOW = 25, LINE = 1 << 20, SORT = 1 << 16, FEW = 4, WORST = 4;
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
    // The popup of a right-click: what it offers, and nothing of the pane's own.
    this.offer = p.menu ?? null;
    if (this.offer) return;
    // Kept from a modal: the view it had there.
    this.layout = (p.layout ?? s.diffMode) === "unified" ? "unified" : "side";
    this.whitespace = p.whitespace ?? s.diffIgnoreWhitespace ?? "none";
    this.context = Number(p.context ?? s.diffContext ?? 3);
    this.view = null; this.error = null; this.busy = false; this.note = null;
    this.sel = {}; this.hunk = 0; this.askDiscard = null;
    // What is selected to copy: {side: "left" | "right" | "unified", anchor, head}, where it began and where it ends, each a
    // place {row, at}: a row of the view (not a line on screen, so the list's scrolling does not touch it) and a place in
    // that row's line (a byte offset of the line as the file has it; -1 before its start, EOL past its end). Or null.
    this.mark = null;
    this.scrollX = 0; this.rowsPx = 0; this.boxH = 0; this.drag = null; this.sweep = null; this.clock = null; this.fit = null; this.gauge = null; this.unit = null;
    // How wide each side's lines were in the last frame, and the same while a diff read again is ranked: a floor for the range.
    this.shown = null; this.floor = null;
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
    this.shown = null; this.floor = null;
    this.endSweep();
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
    this.again(cx);
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
      this.again(cx);
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

  // The line a press at `x` picks (a change's own number, in a diff that can stage), or null.
  pickAt(r, x) {
    if (!this.view.stageable || r.h === undefined || x < 0) return null;
    if (this.layout === "unified") return x < GUTTER * 2 ? (r.k === "-" ? ["l", r.li] : ["r", r.ri]) : null;
    // Where the left side ends, as the last frame laid it out: its share of the row, less the divider.
    const left = ((this.rowsWidth ?? 0) - 1) * (this.lay?.ratio ?? 0.5);
    if (x < GUTTER && r.l !== undefined) return ["l", r.li];
    if (x > left && x < left + 1 + GUTTER && r.r !== undefined) return ["r", r.ri];
    return null;
  }

  // The side a press at `x` is on: what a selection begun there belongs to.
  sideAt(x) {
    if (this.layout === "unified") return "unified";
    return x > ((this.rowsWidth ?? 0) - 1) * (this.lay?.ratio ?? 0.5) ? "right" : "left";
  }

  // Rows can't hold handlers (virtual list): a click on a fold opens the whole file; on a changed
  // line's number (where lines can be picked) it picks the line; elsewhere it selects the row's line
  // to copy (Shift: from where the selection began, by lines, or to the character under the pointer
  // when characters are selected) and makes its hunk the current one. The release of a drag is no click.
  clickRow(i, cx) {
    const r = this.view?.rows[i];
    if (!r || this.dragged) return;
    if (r.k === "fold") { this.context = -1; this.relayout(cx); return; }
    const x = this.downX ?? -1, pick = this.pickAt(r, x);
    if (pick) return this.toggleLine(r.h, pick[0], pick[1], cx);
    const m = this.mark;
    if (this.downShift && m) {
      this.mark = lines(m) ? reach(m, i) : { ...m, head: { row: i, at: this.placeAt(i, m.side, this.downAt) ?? EOL } };
    } else this.mark = whole(i, this.sideAt(x));
    if (r.h !== undefined) this.hunk = r.h;
    cx.notify();
  }

  // The place in row `i`'s line on `side` that window x `at` is over, by the line as it was last drawn; null when the row
  // has no line there or it was not drawn lately.
  placeAt(i, side, at) {
    const v = this.view, line = v && at != null ? lineOf(v.rows[i], side) : null;
    return line ? diffHit(v.id, line.side, line.k, at) : null;
  }

  // Cmd+C (Ctrl+C): what is selected, as the file has it. False when there is nothing to copy.
  copyLines(cx) {
    const v = this.view, runs = v ? copyRuns(v.rows, this.mark) : [];
    if (!runs.length || empty(this.mark)) return false;
    try { clip(cx, diffText(v.id, runs, cutOf(v.rows, this.mark)), lines(this.mark) ? linesCopied(total(runs)) : t("copied.selection")); }
    catch (e) { this.note = String(e?.message ?? e); cx.notify(); }
    return true;
  }

  // A right-click: the row joins the selection (replaces it, unless it is in it), and a menu of this
  // mod's own offers what to copy: the selection, the hunk under the pointer, the patch of the
  // hunks the selection touches.
  openMenu(i, x, cx) {
    const v = this.view, r = v?.rows[i];
    if (!r || r.k === "fold") return;
    if (!covers(this.mark, i) || empty(this.mark)) this.mark = whole(i, this.sideAt(x));
    const m = this.mark;
    openOverlay({
      mod: "yuzora-git-diff", kind: "popup", at: "pointer", width: MENU_W, height: MENU_HEIGHT,
      props: { menu: { id: v.id, whitespace: this.whitespace, runs: copyRuns(v.rows, m), cut: cutOf(v.rows, m), lines: lines(m), hunk: hunkRuns(v, r.h ?? -1), hunks: touchedHunks(v.rows, m) } },
    });
    cx.notify();
  }

  // What of row `i`'s line on `side` ("l" or "r"; a unified selection is on every row) is selected, as DiffLine's `sel`:
  // [from, to], from -1 when it began on a row above, to EOL when it goes on below. Null for none of it.
  marked(i, side) {
    const m = this.mark;
    if (!covers(m, i) || empty(m) || m.side !== "unified" && m.side !== (side === "l" ? "left" : "right")) return null;
    const [a, b] = ends(m), from = i === a.row ? a.at : -1, to = i === b.row ? b.at : EOL;
    // Ending at a line's start selects the line end before it, and nothing of this one.
    return i === b.row && i !== a.row && to <= 0 ? null : [from, to];
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
  // as an error. The rows are new ones: a selection of rows names rows of the old, so it goes. How wide the old ones were stays
  // as the range's floor till the new ones are ranked: it is not known sooner, and the offset has to stay where it is meanwhile.
  show(view) {
    this.view = view; this.gauge = null; this.mark = null; this.floor = this.shown;
    this.endSweep();
    this.fit = this.layout === "unified" ? { longest: Math.min(2000, view.rows.reduce((m, r) => Math.max(m, cells(r.t, m), cells(r.lt, m), cells(r.rt, m)), 0)) } : ranking(view.rows);
  }

  // The next slice of the rows is ranked, when a frame has asked for it out of sight. The frame after it is asked for first,
  // so that a slice the engine cuts off still has one.
  ranked(fit, cx) {
    if (this.fit !== fit || fit.done) return;
    this.again(cx);
    if (fit.trying) cutSlice(fit);
    fit.tries++;
    const meter = fit.trying = { left: fit.slice, at: fit.at, slice: fit.slice };
    try { scan(fit, meter); fit.trying = null; regrow(fit); } catch (e) { fit.trying = null; this.error = String(e); }
  }

  // The next frame is asked for, and asked for again if no event has asked since LATE ms: a frame the engine cuts off is not
  // drawn again by anything, and the events that would ask are in the frame it cut. A frame that is cut again is asked for
  // again, a smaller one, up to RETRIES times in a row.
  again(cx) {
    cx.notify();
    this.retries = 0;
    this.watch(cx);
  }

  watch(cx) {
    this.late?.cancel();
    this.late = cx.timer.after(LATE, (cx) => {
      if (!this.waiting() && !this.framing) return;
      cx.notify();
      if (this.cutOff() && ++this.retries < RETRIES) this.watch(cx);
    });
  }

  // Whether the last slice, frame or batch that was begun was not finished.
  cutOff() {
    const { fit, gauge: g } = this;
    return Boolean(this.framing || fit?.trying || g && (g.l.trying || g.r.trying));
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
      const side = () => ({ left: undefined, got: 0, blind: 0, rounds: 0, frame: FRAME, top: FRAME, trying: null, cuts: 0, alone: -1 });
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
  // are still being ranked, the most any line ranked so far can take (which is not an upper bound of the rows to come).
  remain(name) {
    const g = this.gauge[name], rank = this.fit[name];
    if (!this.fit.done) return rank.top;
    return Math.max(g.blind, g.left >= 0 ? entry(rank, g.left).cells : 0);
  }

  // How wide the box of the widest line of a side that was not laid out may be, in the unit the pane writes
  // lengths in (a cell is not known till the ruler is laid out).
  later(name) {
    const cells = this.remain(name), bound = cells ? INDENT + cells * (this.unit?.w ?? 0) : 0;
    return this.fit.done ? bound : Math.max(bound, this.floor?.[name] ?? 0);
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

  // Where a press landed in the rows, across (in the pane's unit, and in the window for the place in a line it is over), and
  // whether Shift was down, for [`clickRow`] to tell a line's number from its text and a selection begun from one extended.
  //
  // A press in a row's text is a click that may turn into a drag across characters, and `this.sweep` says which it is now:
  //   down  The button is down, and the pointer has not gone SLOP px from where it landed nor left its row. This is a click not
  //         yet let go of: no clock runs, the list does not scroll, nothing is selected. The release is the click the list reports.
  //   drag  The pointer has gone SLOP px or is over another row. The clock looks at it every TICK ms; at the list's edge the list scrolls
  //         then and only then; the selection runs from the place pressed to the place under the pointer ([`extend`]), and the
  //         release is no click.
  //   over  (`this.sweep` is null) The button is up (a release, or a move that says so), the pointer left the pane, a key reached
  //         the pane, the pane lost the keyboard it had when the drag began, or the diff is read again ([`endSweep`]). The selection stays.
  // No press on a fold (its click opens the file), on the numbers a line is picked by, or on the bar under the lines is any of these.
  pressed(e, cx) {
    const x = e.local_position?.x, y = e.local_position?.y, v = this.view;
    this.downX = x == null ? null : unzoom(x);
    this.downAt = e.position?.x ?? null;
    this.downShift = Boolean(e.modifiers?.shift);
    this.dragged = false;
    this.endSweep();
    if (x == null || y == null || !v || unzoom(y) >= this.listHeight()) return;
    const from = this.rowAt(y), r = v.rows[from];
    if (!r || r.k === "fold" || this.pickAt(r, this.downX)) return;
    // The list's top in the window, where the pointer will be told as a position in the window, not in the rows.
    const side = this.sideAt(this.downX);
    this.sweep = { from, side, at: this.placeAt(from, side, e.position.x) ?? -1, shift: this.downShift, top: e.position.y - y, x0: e.position.x, y0: e.position.y, x: e.position.x, y: e.position.y, drag: false, began: false };
  }

  // How tall the rows' list is: its box less the bar under the lines when they scroll sideways, which the box leaves room for.
  listHeight() {
    return unzoom(this.boxH) - (this.layout === "side" && (this.lay?.reach ?? 0) > 0 ? BAR : 0);
  }

  // The row at `y` window pixels down the list, whatever the zoom and wherever it is scrolled to. The rows of a virtual list hold no
  // handler and the list reports a click only when the press and the release are on one row, so the row under a pointer that is
  // dragged is worked out from how far the list says it is scrolled (the first row drawn is not it: the list draws from
  // a row before the one at the top).
  rowAt(y) {
    const last = (this.view?.rows.length ?? 1) - 1;
    return Math.min(last, Math.max(0, Math.floor((unzoom(y) + unzoom(this.scroll.offset())) / ROW)));
  }

  // The pointer is at `at` in the window: a press that has not moved is still a click, and one that has begins the drag. A move
  // selects and never scrolls: a platform may send the last move of a held drag again and again (macOS does, every 16 ms), and the
  // list's speed at its edge is the clock's, the same everywhere.
  sweepTo(at, cx) {
    const s = this.sweep;
    if (!s) return;
    s.x = at.x; s.y = at.y;
    if (!s.drag) {
      if (Math.hypot(at.x - s.x0, at.y - s.y0) < SLOP && this.rowAt(at.y - s.top) === s.from) return;
      s.drag = true;
      this.dragged = true;
      s.keys = this.handle.is_focused();
      this.clock = cx.timer.after(TICK, (cx) => this.beat(cx));
    }
    this.extend(cx, false);
  }

  // A drag looks at the pointer again, with no move: a list scrolled under it by the wheel or the bar takes the selection with it,
  // and a pointer held at the list's edge scrolls the list. This is the only place the list is scrolled by a drag.
  beat(cx) {
    if (!this.sweep) return;
    // The pane had the keyboard when the drag began and another has it now (a shortcut of the app showed it or opened it): this one
    // may not be shown, and the button's release will not be seen here.
    if (this.sweep.keys && !this.handle.is_focused()) { this.endSweep(); return; }
    this.extend(cx, true);
    this.clock = cx.timer.after(TICK, (cx) => this.beat(cx));
  }

  // The selection runs from the place pressed to the place under the pointer (the row under it, and in that row's line the
  // place nearest it), and, when `scroll` is set (the clock's look), a pointer at the list's edge scrolls the list, by more the
  // further out it is: the rows that come into view are selected at the next look. It begins with the drag (from the selection
  // there is, if Shift was down). Nothing here touches the ranking of the rows or asks for a frame but the one that shows what changed.
  extend(cx, scroll) {
    const s = this.sweep, rows = this.view?.rows;
    if (!s || !rows) return;
    const p = unzoom(s.y - s.top), h = this.listHeight(), off = unzoom(this.scroll.offset());
    const up = p < EDGE, depth = up ? EDGE - p : Math.max(0, p - (h - EDGE));
    let moved = false;
    if (scroll && depth > 0 && (up ? off > 0 : off < rows.length * ROW - h - 0.5)) {
      const step = Math.min(1 + Math.floor(depth / ROW), MOST);
      // The list brings a row into view and no further: the row `step` rows above the first one in view, or the one that ends `step` rows below the last.
      this.scroll.scroll_to_item(Math.max(0, Math.min(rows.length - 1, up ? Math.ceil(off / ROW) - step : Math.floor((off + h) / ROW) + step - 1)), "top");
      moved = true;
    }
    const row = this.rowAt(s.y - s.top);
    if (!s.began) {
      s.began = true;
      const m = this.mark;
      this.mark = s.shift && m ? m : { side: s.side, anchor: { row: s.from, at: s.at }, head: { row: s.from, at: s.at } };
      if (rows[s.from].h !== undefined) this.hunk = rows[s.from].h;
      moved = true;
    }
    const m = this.mark, before = row < m.anchor.row;
    // Over a row whose line was not drawn (or that has none on the side): before or past it, as the pointer goes.
    const head = { row, at: this.placeAt(row, m.side, s.x) ?? (before ? -1 : EOL) };
    if (head.row !== m.head.row || head.at !== m.head.at) { this.mark = { ...m, head }; moved = true; }
    if (moved) cx.notify();
  }

  // The button is up, or the pointer left, or the rows are others: the selection stays, the clock stops. Nothing is drawn
  // differently for it, so no frame is asked for.
  endSweep() {
    this.clock?.cancel();
    this.clock = null;
    this.sweep = null;
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
  // inside the pane. Leaving the pane ends the drag: the release would not be seen out there. The same for a drag across
  // rows (a sweep of the pointer). They are in every frame and look at what is on when they are called, so that a press that
  // begins a sweep, and the release that ends it, ask for no frame.
  dragging(el) {
    return el
      .on_mouse_move((e, cx) => {
        // The release of a button can be missed: a shortcut of the app takes a key from the pane, and after a key the window counts
        // the pointer as over nothing till it moves. A move with the button up says it is up.
        if (!e.dragging) { if (this.drag) this.drop(cx); this.endSweep(); }
        else if (this.drag) this.dragTo(e.position.x, cx);
        else if (this.sweep) this.sweepTo(e.position, cx);
      })
      .on_mouse_up("left", (_e, cx) => { if (this.drag) this.drop(cx); this.endSweep(); })
      .on_mouse_down("left", (_e, cx) => { if (this.drag) this.drop(cx); })
      .on_hover((hovered, cx) => { if (!hovered) { if (this.drag) this.drop(cx); this.endSweep(); } });
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
    // A key ends a drag across rows: after one the window does not count the pointer as over anything until it moves, so
    // the release of a button pressed before the key may never reach the pane.
    this.endSweep();
    if (e.key === "c" && (e.modifiers?.platform || e.modifiers?.control) && !e.modifiers?.alt && !e.modifiers?.shift) {
      if (this.copyLines(cx)) cx.stop_propagation();
      return;
    }
    if (e.key === "f7" || e.key === "n" || e.key === "down" && e.modifiers?.alt) { this.go(cx, e.modifiers?.shift ? -1 : 1); cx.stop_propagation(); }
    else if (e.key === "p" || e.key === "up" && e.modifiers?.alt) { this.go(cx, -1); cx.stop_propagation(); }
    else if (e.key === "s" && this.view?.stageable && this.spec?.mode === "unstaged") { this.apply(cx, "stage", Object.keys(this.picked()).length ? [] : [this.hunk], this.picked()); cx.stop_propagation(); }
    else if (e.key === "u" && this.view?.stageable && this.spec?.mode === "staged") { this.apply(cx, "unstage", Object.keys(this.picked()).length ? [] : [this.hunk], this.picked()); cx.stop_propagation(); }
  }

  // The number the lines know the theme's colours by: code as the editor colours it, the marks of what changed and the
  // selection. Asked for once for each look.
  look(th, d) {
    const key = JSON.stringify([syntax(th), d.removed, d.added, th.accent]);
    if (this.looked?.key !== key) {
      this.looked = { key, id: diffLook({ syntax: syntax(th), removed: `${d.removed ?? "#f08a8a"}55`, added: `${d.added ?? "#86d17a"}55`, selection: wash(th.accent) }) };
    }
    return this.looked.id;
  }

  // A line's text, drawn natively as one text: the line as the file has it, its code coloured by the diff's syntax, the
  // words that changed marked (`m`, or a side's `lm` and `rm`), and `sel` tinted ([from, to] in the line, from below 0
  // and to past its end reaching the row's edges). `side` is the side the line is of ("l" or "r"), `k` its place there.
  text(r, side, k, look, marks, sel) {
    const text = side === "l" ? r.lt ?? r.t : r.rt ?? r.t;
    return DiffLine.new(`${side}${k}`, { text, diff: this.view.id, side, line: k, look, ...(marks ? { marks } : {}), ...(sel ? { sel } : {}) });
  }

  // The box a line's text is drawn in. The cell draws it and the measuring of the lines lays it out ([`probes`]): how
  // wide a line is is how wide this is.
  line(r, side, look, sel) {
    const n = r[side];
    return div().flex_none().h_full().pl(INDENT).when(n !== undefined, (el) => el.child(this.text(r, side, n - 1, look, r[`${side}m`], sel)));
  }

  // One side's cell: number (click picks the line), then the text, which `sx` moves under the cell's
  // edge. `grow` is its share of the row, the same for every row; `selected`, what of its line is selected ([`marked`]).
  cell(r, side, th, d, look, grow, sx, selected) {
    const n = r[side];
    const changedRow = r.k === "~" || r.k === "-" || r.k === "+";
    const present = n !== undefined;
    const del = side === "l";
    // Beside a line the other side has alone, the room is that change's colour, softer: an added line's on the left.
    const bg = !changedRow ? null : present ? (del ? d.removed_bg : d.added_bg) : del ? d.added_filler ?? th.raised : d.removed_filler ?? th.raised;
    const index = del ? r.li : r.ri;
    const picked = changedRow && present && this.sel[r.h]?.[del ? "del" : "add"]?.has(index);
    const canPick = changedRow && present && this.view.stageable;
    return h_flex().w(0).flex_grow(grow).min_w(0).h_full().relative().when(Boolean(bg), (el) => el.bg(bg))
      .child(div().w(GUTTER).flex_none().h_full().pr(6).text_right().text_color(picked ? th.foreground : th.dim)
        .when(picked, (el) => el.bg(del ? d.removed ?? th.ansi1 : d.added ?? th.ansi2).text_color(th.background))
        .when(canPick, (el) => el.cursor_pointer().hover((s) => s.text_color(th.foreground)))
        .child(present ? String(n) : ""))
      .child(h_flex().flex_1().min_w(0).h_full().overflow_hidden().child(this.line(r, side, look, selected).ml(-sx)));
  }

  row(r, i, th, d, look, width) {
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
      // The sign and the text are one clipped box: a selection reaches from its left to its right.
      const on = add ? "r" : "l", n = add ? r.ri : del ? r.li : r.l - 1;
      return base.relative().when(current, (el) => el.border_l(2).border_color(th.accent)).when(Boolean(bg), (el) => el.bg(bg))
        .child(div().w(GUTTER * 2).flex_none().h_full().flex().text_color(picked ? th.background : th.dim)
          .when(picked, (el) => el.bg(del ? d.removed ?? th.ansi1 : d.added ?? th.ansi2))
          .when(canPick, (el) => el.cursor_pointer())
          .child(div().w(GUTTER).pr(6).text_right().child(r.l !== undefined ? String(r.l) : ""))
          .child(div().w(GUTTER).pr(6).text_right().child(r.r !== undefined ? String(r.r) : "")))
        .child(h_flex().flex_1().min_w(14 + width).h_full().overflow_hidden()
          .child(div().w(14).flex_none().text_color(del ? d.removed : add ? d.added : th.dim).child(del ? "−" : add ? "+" : ""))
          .child(this.text(r, on, n, look, r.m, this.marked(i, "l"))));
    }
    // The current hunk's mark sits over the row's edge: a border would move this row's divider off the others'.
    const lay = this.lay;
    return base.relative().child(this.cell(r, "l", th, d, look, lay.l, lay.sx, this.marked(i, "l"))).child(div().w(1).flex_none().h_full().bg(th.border)).child(this.cell(r, "r", th, d, look, lay.r, lay.sx, this.marked(i, "r")))
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
    const g = this.gauge[name], rank = this.fit[name], text = SIDES[name], lines = [];
    let used = 0;
    for (let at = g.left; at >= 0 && lines.length < GROW ** g.rounds && used <= g.frame; at--) {
      const { cells, r } = entry(rank, at), long = (r[text] ?? r.t).length, price = cost(long);
      if (long > LAYOUT) {
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

  // The frame before this one was cut off. Of the sides that had a batch in it the one whose cost more is the cause (both,
  // if they cost the same), and the other is asked for again as it was.
  blame(g) {
    const cut = ["l", "r"].filter((name) => g[name].trying), most = Math.max(0, ...cut.map((name) => g[name].trying.cost));
    for (const name of cut) this.cutBatch(name, g[name].trying.cost >= most);
  }

  // The batch of a side was in a frame that was cut off. If it is the cause, the batch after it is half its cost, and a line
  // that was cut by itself is asked for once more, then passed over, at its upper bound like those that cannot be laid out.
  cutBatch(name, cause) {
    const s = this.gauge[name], batch = s.trying;
    s.trying = null; s.cuts++;
    if (!cause) return;
    if (batch.lines.length > 1) { s.top = Math.min(s.top, Math.max(1, s.frame >> 1)); s.frame = Math.max(1, batch.cost >> 1); return; }
    if (s.alone !== batch.from) { s.alone = batch.from; return; }
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
    s.frame = Math.min(s.top, s.frame * 2);
    this.again(cx);
  }

  // What is done out of sight, in the very box the cells draw lines in ([`line`]), in the font and size of the rows: the
  // next slice of the rows is ranked, if they are not all, as the size of an element that is new each time says; then a
  // batch of each side is laid out while some lines of it may be wider than the widest laid out so far, and a run of
  // ruler, which says what a cell is for the lines that are not. Each reports its width, which is how far the sides
  // scroll; they go away once nothing is left to find out.
  probes(look) {
    const g = this.gauge, fit = this.fit;
    if (!g) return div();
    const hidden = () => v_flex().absolute().top(0).left(0).h(0).overflow_hidden();
    const out = [];
    let heavy = false;
    if (!fit.done) out.push(hidden().id(`rank-${fit.at}-${fit.tries}`).on_size_change((_size, cx) => this.ranked(fit, cx)).child(div().w(1)));
    else {
      this.blame(g);
      for (const name of ["l", "r"]) {
        if (this.settled(name)) continue;
        const batch = this.batch(name);
        if (!batch.lines.length || batch.cost > FRAME && heavy) continue;
        if (batch.cost > FRAME) heavy = true;
        g[name].trying = batch;
        out.push(hidden().id(`gauge-${g.id}-${name}-${batch.from}-${g[name].cuts}`)
          .on_size_change((size, cx) => this.measured(g, name, batch, size, cx))
          .children(batch.lines.map((r) => h_flex().child(this.line(r, name, look, null)))));
      }
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
  sideBody(v, th, d, look, mono) {
    this.gauged();
    const ratio = this.ratio(), width = this.rowsWidth ?? 0, reach = this.reach(ratio);
    // What is kept, a wheel's or a thumb's reach past the end included, is brought within the room there is
    // now: a pane made narrower or a divider moved leaves the lines where they are, and does not give them
    // what was asked for beyond the end.
    // A thumb that is held stays under the pointer when the range changes: the offset follows the thumb. While the rows of a
    // diff read again are ranked the range is at least what it was ([`later`]), so neither the offset nor the thumb goes to a
    // range that is only what was ranked so far; once they are all ranked it is the new diff's, and the offset is brought within it.
    const thumb = Math.max(24, Math.round(width * width / (width + reach))), room = Math.max(1, width - thumb);
    this.scrollX = this.drag?.kind === "bar" ? reach * Math.min(this.drag.at, room) / room : Math.min(this.scrollX, reach);
    this.shown = { l: this.span("l"), r: this.span("r") };
    this.lay = { ratio, width, reach, thumb, sx: this.scrollX, l: ratio * 100, r: (1 - ratio) * 100 };
    return div().flex_1().min_h(0).relative().overflow_hidden().font_family(mono).text_size(11.5)
      .child(div().id("diff-rows-box").size_full().pb(reach > 0 ? BAR : 0)
        .on_size_change((size, cx) => { this.boxH = size.height; if (Math.abs(size.width - this.rowsPx) > 0.5) { this.rowsPx = size.width; cx.notify(); } })
        .on_mouse_down("left", (e, cx) => this.pressed(e, cx))
        .on_scroll_wheel((e, cx) => this.wheel(e, cx))
        .child(this.rowList(v, th, d, look, 0)))
      .child(Scrollbar.vertical("diff-rows").viewport_from_layout().absolute().top(0).left(0).right(0).bottom(reach > 0 ? BAR : 0))
      .child(this.splitter(th))
      .when(reach > 0, (el) => el.child(this.hbar(th)))
      .child(this.probes(look));
  }

  rowList(v, th, d, look, width) {
    return v_virtual_list("diff-rows", v.rows.length, ROW, (i) => String(i),
      (range) => v.rows.slice(range.start, range.end).map((r, k) => this.row(r, range.start + k, th, d, look, width)))
      .on_item_click((key, cx) => this.clickRow(Number(key), cx))
      .on_item_secondary_click((key, e, cx) => this.openMenu(Number(key), e.local_position?.x == null ? -1 : unzoom(e.local_position.x), cx))
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

  // The popup of a right-click. Each row puts its text on the clipboard and closes the popup.
  renderOffer(cx) {
    const th = theme(), m = this.offer, n = total(m.runs), whole = m.lines !== false;
    // A failure is said in a toast and the menu closes: the popup is as tall as its three rows (MENU_HEIGHT), so a
    // line under them would be cut off and the click would seem to do nothing.
    const act = (cx, work) => cx.spawn(async (cx) => {
      try { await work(cx); }
      catch (e) { notify({ title: t("copy.failed"), body: String(e?.message ?? e), level: "error" }); }
      closeOverlay(props().overlay);
    });
    const row = (id, label, enabled, work) => div().id(id).h(MENU_ROW).px(10).flex().items_center().rounded(4).text_size(12)
      .text_color(enabled ? th.foreground : th.dim)
      .when(enabled, (el) => el.cursor_pointer().hover((s) => s.bg(th.hover)).on_click((_e, cx) => act(cx, work)))
      .child(label);
    return v_flex().size_full().p(MENU_PAD)
      .child(row("copy", !whole || !n ? t("menu.copy") : n === 1 ? t("menu.copy_one") : t("menu.copy_other", { count: n }), n > 0,
        (cx) => clip(cx, diffText(m.id, m.runs, m.cut), whole ? linesCopied(n) : t("copied.selection"))))
      .child(row("hunk", t("menu.hunk"), m.hunk.length > 0, (cx) => clip(cx, diffText(m.id, m.hunk), t("copied.hunk"))))
      .child(row("patch", t("menu.patch"), m.hunks.length > 0, async (cx) => clip(cx, await diffPatch(m.id, m.hunks, m.whitespace), t("copied.patch"))));
  }

  // A frame that does not come back, not even by throwing, was cut off by the engine: it stays marked as begun.
  render(cx) {
    if (this.offer) return this.renderOffer(cx);
    this.framing = true;
    try { return this.frame(cx); } finally {
      this.framing = false;
      if (this.gauge) this.gauge.l.trying = this.gauge.r.trying = null;
    }
  }

  frame(cx) {
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
      const width = (this.fit?.longest ?? 0) * CHAR + 24, look = this.look(th, d);
      body = div().flex_1().min_h(0).relative().font_family(mono).text_size(11.5)
        .child(div().id("diff-x").size_full().overflow_x_scrollbar()
          .child(div().id("diff-rows-box").w_full().min_w(GUTTER * 2 + 14 + width).h_full()
            .on_size_change((size) => { this.rowsPx = size.width; this.boxH = size.height; })
            .on_mouse_down("left", (e, cx) => this.pressed(e, cx))
            .child(this.rowList(v, th, d, look, width))))
        .child(Scrollbar.vertical("diff-rows").viewport_from_layout().absolute().inset_0());
    } else body = this.sideBody(v, th, d, this.look(th, d), mono);

    const ask = this.askDiscard && h_flex().px(10).py(6).gap(8).items_center().bg((tokens().attention ?? {}).background ?? th.raised).text_size(11.5)
      .child(div().flex_1().child(t("discard.ask")))
      .child(this.button("discard-yes", t("discard"), th, true, (cx) => { const a = this.askDiscard; this.askDiscard = null; this.apply(cx, "discard", a.hunks, a.lines); }))
      .child(this.button("discard-no", t("cancel"), th, true, (cx) => { this.askDiscard = null; cx.notify(); }));

    return root.child(header)
      .when(Boolean(ask), (el) => el.child(ask))
      .when(Boolean(this.note), (el) => el.child(div().px(10).py(4).text_size(11.5).text_color(th.ansi1).child(this.note)))
      .when(Boolean(v?.modes) && v.rows.length > 0, (el) => el.child(div().px(10).py(3).text_size(11).text_color(th.muted).child(t("modes", { from: v.modes[0], to: v.modes[1] }))))
      .when(Boolean(v) && !v.stageable && (spec.mode === "unstaged" || spec.mode === "staged") && this.whitespace !== "none", (el) => el.child(div().px(10).py(3).text_size(11).text_color(th.dim).child(t("wsNoStage"))))
      .child(body);
  }
}

// A kept or dragged share of the width, in range; anything else (nothing kept, a damaged file) is half.
function ratioOf(v) {
  return typeof v === "number" && Number.isFinite(v) ? Math.min(SPLIT_MAX, Math.max(SPLIT_MIN, v)) : 0.5;
}

// The one place the clipboard is written: the text, and a toast that says what it was.
function clip(cx, text, what) {
  cx.write_to_clipboard(text);
  notify({ title: what, level: "success", timeout: 1500 });
}

const total = (runs) => runs.reduce((n, r) => n + r[2] - r[1], 0);
const linesCopied = (n) => (n === 1 ? t("copied.lines_one") : t("copied.lines_other", { count: n }));

// The accent as a wash over what is under it (a color that is not #rrggbb is used as it is).
const wash = (c) => (/^#[0-9a-f]{6}$/i.test(c ?? "") ? `${c}44` : c);

// A place past a line's end: a selection that reaches it goes on to the line end, and to the next line.
const EOL = 2 ** 30;

// How many cells a line is drawn in, a tab four, when that can be more than `than` (a tab is the widest a unit is drawn).
function cells(s, than) {
  if (!s || s.length * 4 <= than) return 0;
  let n = s.length;
  for (let at = s.indexOf("\t"); at >= 0; at = s.indexOf("\t", at + 1)) n += 3;
  return n;
}

// Code colors by theme color name, so code reads on light and dark themes alike. A name not listed takes its prefix's
// (`keyword.control` is a keyword). The table and [`syntax`] are the editor's (yuzora-editor/doc-pane.js), held equal by
// a test in yuzora-git.
const SYNTAX = {
  keyword: "ansi5",
  function: "ansi12", "function.method": "ansi12", link: "ansi12",
  "function.builtin": "ansi6", operator: "ansi6", "string.escape": "ansi6", "string.special": "ansi6",
  type: "ansi11", "type.builtin": "ansi11", constructor: "ansi11", namespace: "ansi11", module: "ansi11",
  string: "ansi2", literal: "ansi2",
  number: "ansi3", boolean: "ansi3", constant: "ansi3", "constant.builtin": "ansi3", attribute: "ansi3",
  comment: "dim",
  property: "ansi1", tag: "ansi1", label: "ansi1", heading: "ansi1", "variable.builtin": "ansi1", "variable.parameter": "ansi1",
};

// The theme's colors for code (`syntax` of Editor and Markdown). Built with
// `fromEntries`: mods run with frozen built-in prototypes, where
// `out.constructor = …` throws.
function syntax(th) {
  return Object.fromEntries(Object.entries(SYNTAX).filter(([, color]) => th[color]).map(([name, color]) => [name, th[color]]));
}

// Where a selection begins and where it ends, first to last, whichever way it was made.
function ends(m) {
  const { anchor: a, head: h } = m;
  return h.row < a.row || h.row === a.row && h.at < a.at ? [h, a] : [a, h];
}

function covers(m, row) {
  if (!m) return false;
  const [from, to] = ends(m);
  return row >= from.row && row <= to.row;
}

// A selection of nothing: a drag that came back to where it began.
function empty(m) {
  const [a, b] = m ? ends(m) : [];
  return !m || a.row === b.row && a.at >= b.at;
}

// Row `i`'s line on `side`, "l" or "r"; "unified" is the line the row shows (an added line is the right side's, the
// rest the left's): which side it is of and its place there (0-based). None for a row with none there (a fold, the
// blank beside an added line).
function lineOf(r, side) {
  if (!r || r.k === "fold") return null;
  const s = side === "unified" ? (r.k === "+" ? "r" : "l") : side === "left" ? "l" : "r";
  const n = s === "l" ? r.l : r.r;
  return n === undefined ? null : { side: s, k: n - 1 };
}

// The line of row `i`, whole, selected on `side`.
function whole(i, side) {
  return { side, anchor: { row: i, at: -1 }, head: { row: i, at: EOL } };
}

// Whether a selection is of whole lines: from before its first line to past its last.
function lines(m) {
  const [a, b] = ends(m);
  return a.at < 0 && b.at >= EOL;
}

// A selection of whole lines reaches row `i` by lines, from the line it began on.
function reach(m, i) {
  const down = i >= m.anchor.row;
  return { ...m, anchor: { row: m.anchor.row, at: down ? -1 : EOL }, head: { row: i, at: down ? EOL : -1 } };
}

// The lines of the rows a selection covers, in the order they are shown: the row, which side the line
// is on ("l" or "r") and its place in that side (0-based). The side is the one the selection began
// on; unified shows a deleted line from the left and an added one from the right. A row with no
// line on the side (the blank beside an added line) and a fold add none.
function lineOfRows(rows, m) {
  const out = [];
  if (!m) return out;
  const [from, to] = ends(m);
  for (let i = from.row; i <= to.row && i < rows.length; i++) {
    const line = lineOf(rows[i], m.side);
    if (line) out.push({ r: rows[i], i, side: line.side, line: line.k });
  }
  return out;
}

// Where the copy begins in the first line copied and ends in the last, `[from, to]` (what `diffText` cuts them at): where
// the selection does, when it begins or ends on a row that has a line on its side, else that line's start or end.
function cutOf(rows, m) {
  const got = lineOfRows(rows, m), [a, b] = ends(m);
  if (!got.length) return [0, EOL];
  return [got[0].i === a.row ? Math.max(0, a.at) : 0, got[got.length - 1].i === b.row ? b.at : EOL];
}

// Those lines as runs of one side's lines, `[side, start, end)`, neighbours joined: what `diffText` takes.
function copyRuns(rows, m) {
  const runs = [];
  for (const { side, line } of lineOfRows(rows, m)) {
    const last = runs[runs.length - 1];
    if (last && last[0] === side && last[2] === line) last[2] = line + 1;
    else runs.push([side, line, line + 1]);
  }
  return runs;
}

// The hunks that have a selected line, once each (hunks run in the order of the rows).
function touchedHunks(rows, m) {
  return [...new Set(lineOfRows(rows, m).map(({ r }) => r.h).filter((h) => h !== undefined))];
}

// A hunk's lines as they are after the change (the right side's range); none for a hunk that only deletes.
function hunkRuns(view, h) {
  const b = view.hunks[h]?.b;
  return b && b[1] > b[0] ? [["r", b[0], b[1]]] : [];
}

// What a UTF-16 unit of a line takes at most, in cells (the width of an ASCII character), from what
// JetBrains Mono and what the system falls back to for the rest draw (scripts/fixtures/yuzora-git-diff-font-widths.json,
// held to by yuzora-git-diff-layout.test.ts): Latin, Greek, Cyrillic, punctuation, arrows and box drawing take 1;
// Chinese, Japanese, full width 1.67 and Korean 1.44; a symbol drawn as an emoji 2.17, and one past U+FFFF is two
// units; a tab, drawn as four spaces, 4, the most a unit is drawn ([`WORST`]). Each is rounded up with some to spare,
// and what is not listed is taken at [`UNLISTED`], three cells.
// This is never a width. It says in what order the lines are laid out and when to stop (when what is left cannot be
// wider than what was laid out), so a cell too many here costs a line laid out, not a pixel of range. What it must
// not be is too few: a character that takes more than its class says can end the laying out while a line with it in
// is wider than the widest laid out, by that character's excess.
const ASCII = /[\0-\x08\x0a-\x7f]+/g;
const NARROW = /[^\x80-\u058f\u2000-\u206f\u2190-\u22ff\u2500-\u259f]+/g;
const WIDE = /[^\u1100-\u115f\u2e80-\ua4cf\uac00-\ud7a3\uf900-\ufaff\ufe30-\ufe6f\uff00-\uff60\uffe0-\uffe6]+/g;
const PAIR = /[^\ud800-\udfff]+/g;
const TABS = /[^\t]+/g;
const UNLISTED = 3;

// The cells a line takes at most, reading its first [`LINE`] characters (the rest is taken at [`WORST`] a unit, as is
// every character that is not ASCII in a line with more than [`SORT`] of them). A tab is read with the characters that are
// not ASCII, and taken at WORST: four cells, as it is drawn. What the reading costs, [`TOLL`] and
// [`SLOW`], is taken from `meter`.
function bound(s, meter) {
  const n = s.length, read = Math.min(n, LINE), head = read < n ? s.slice(0, read) : s;
  const other = head.replace(ASCII, "");
  meter.left -= TOLL + read + SLOW * other.length;
  let cells = read - other.length + WORST * (n - read);
  if (other.length > SORT) return cells + WORST * other.length;
  if (other.length) {
    const count = (re) => other.replace(re, "").length;
    const narrow = count(NARROW), wide = count(WIDE), pair = count(PAIR), tabs = count(TABS);
    cells += 1.25 * narrow + 2 * wide + 1.5 * pair + WORST * tabs + UNLISTED * (other.length - narrow - wide - pair - tabs);
  }
  return cells;
}

// Where in a row each side's line is (an unchanged row has one, `t`, for both).
const SIDES = { l: "lt", r: "rt" };

// What laying out a line costs, in the characters it has.
function cost(chars) {
  return chars + 80;
}

// Every line of a side in the order they are laid out in, widest first by what each can take at most, in cells:
// [`bound`] of its text. The order is a sort of numbers (a bound in quarters
// of a cell above, the row below, in one number), so what a frame goes through of it is the few lines it takes off the
// end, and nothing depends on how many rows there are. How wide the lines are is found by laying them out in that
// order: what the table of cells says is wrong costs lines to lay out, not the range. A line that is the same on both
// sides (an unchanged row) is read once.
function ranking(rows) {
  const count = rows.length, base = 2 ** Math.ceil(Math.log2(count + 1));
  const side = () => ({ keys: new Float64Array(count), n: 0, few: [], floor: -1, top: 0, base, rows });
  return { rows, count, base, at: 0, done: false, slice: SLICE, top: SLICE, trying: null, tries: 0, hurt: -1, retry: -1, l: side(), r: side() };
}

// Ranks the rows from where `fit` left off until `meter` has nothing left, and puts the lines of a side in order when all
// the rows have been (a side whose turn comes with the meter used up is left for the next call).
// A line is read in full if it can beat the FEW widest read so far, else it is taken at WORST cells a unit.
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
      lc = WORST * lt.length;
      if (!hurt && lc > l.floor) { lc = bound(lt, meter); readL = true; }
      if (rt === lt) same = lc;
    }
    if (rt) {
      rc = same || WORST * rt.length;
      if (same) readR = rc > r.floor;
      else if (!hurt && rc > r.floor) { rc = bound(rt, meter); readR = true; }
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

// The slice before this one was cut off by the engine. If it got any row done, the next is half of what it did, and none will
// be as big as half of this one was; or else the row it began with is tried once more, and if that is cut too it cannot be read
// in the time there is, and is taken at its upper bound.
function cutSlice(fit) {
  const { left, at, slice } = fit.trying;
  fit.trying = null;
  if (fit.at !== at) {
    fit.slice = Math.max(1, Math.min(slice - left, slice) >> 1);
    fit.top = Math.min(fit.top, Math.max(1, slice >> 1));
  } else if (fit.retry === at) fit.hurt = at;
  else fit.retry = at;
}

// A slice that was not cut off: the next is twice as many units, up to the most a slice has been allowed since one was.
function regrow(fit) {
  fit.slice = Math.min(fit.top, fit.slice * 2);
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
