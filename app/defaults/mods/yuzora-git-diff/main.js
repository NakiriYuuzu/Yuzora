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

const ROW = 20, CHAR = 7, GUTTER = 44;
// Side by side, the left side's share of the width: kept in shared state (a diff opens with new props every
// time, so a localStorage keyed by them would start over), dragged between these, 0.5 by default.
const SPLIT = "yuzora.git.diff.split", SPLIT_MIN = 0.15, SPLIT_MAX = 0.85;
// The grab area over the divider, the bar below the lines when they scroll sideways, and the room left
// after the widest line: the indent (6) and some more.
const GRAB = 5, BAR = 8, REST = 18;
// How [`widest`] finds the widest lines without laying all of them out: the lines laid out out of sight
// per side (WIDEST) and the longest worth laying out (LAYOUT); the characters it reads in a diff (LOOK)
// and in one line (LINE), what a line costs in them besides its length (TOLL) and a character that is not
// ASCII besides that (SLOW); how many of those a line may have to be sorted by kind (SORT); and the most
// cells a UTF-16 unit takes (WORST).
const WIDEST = 4, LAYOUT = 1 << 18, LOOK = 4e6, LINE = 1 << 20, TOLL = 200, SLOW = 25, SORT = 1 << 16, WORST = 3;
// The width of a run of this over its length is what a cell takes in the font.
const RULER = "W".repeat(32);

export default class GitDiff extends View {
  init(_props, cx) {
    const s = gitSettings(), p = props() ?? {};
    // Kept from a modal: the view it had there.
    this.layout = (p.layout ?? s.diffMode) === "unified" ? "unified" : "side";
    this.whitespace = p.whitespace ?? s.diffIgnoreWhitespace ?? "none";
    this.context = Number(p.context ?? s.diffContext ?? 3);
    this.view = null; this.error = null; this.busy = false; this.note = null;
    this.sel = {}; this.hunk = 0; this.askDiscard = null;
    this.scrollX = 0; this.textW = { l: 0, r: 0 }; this.cellW = 0; this.drag = null; this.fit = null;
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
    this.view = null; this.fit = null; this.error = null; this.sel = {}; this.hunk = 0; this.note = null; this.scrollX = 0;
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
      try { this.show(await diffView(this.view.id, { layout: this.layout, context: this.context, whitespace: this.whitespace })); this.sel = {}; }
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

  // The left side's share of the width: the one kept, or the one being dragged.
  ratio() {
    return this.drag?.kind === "split" ? this.drag.ratio : ratioOf(shared(SPLIT));
  }

  // A diff's rows come in here: what the pane needs of them to draw, which takes reading all of their
  // lines, is worked out now, once, not in each frame. Both are kept only when it is all done, so a
  // failure leaves the diff that is on show as it was.
  show(view) {
    const fit = this.layout === "unified"
      ? { longest: Math.min(2000, view.rows.reduce((m, r) => Math.max(m, (r.t ?? "").length, (r.lt ?? "").length, (r.rt ?? "").length), 0)) }
      : widest(view.rows);
    this.view = view; this.fit = fit;
  }

  // What the widest line of a side takes: those laid out as the font draws them, or what the lines that
  // were not could take at most, whichever is more.
  span(side) {
    return Math.max(this.textW[side], (this.fit?.[side]?.rest ?? 0) * this.cellW);
  }

  // How far the lines can scroll sideways: the most the widest line of either side reaches past its
  // side's room (its cell less the numbers).
  reach(ratio) {
    const w = this.rowsWidth ?? 0, left = (w - 1) * ratio;
    if (!w) return 0;
    return Math.max(0, this.span("l") + REST - (left - GUTTER), this.span("r") + REST - (w - 1 - left - GUTTER));
  }

  // A sideways swipe or wheel moves every line the same distance; the frame keeps it within [`reach`].
  // A swipe is sideways when it goes more that way than up or down: the list takes the rest, and a
  // little sideways in a vertical swipe does not drift the lines.
  wheel(e, cx) {
    const lay = this.lay;
    if (!lay || Math.abs(e.delta.x) <= Math.abs(e.delta.y)) return;
    const at = Math.max(lay.sx - e.delta.x, 0);
    if (at !== this.scrollX) { this.scrollX = at; cx.notify(); }
  }

  // The drag in progress follows the pointer: the divider by the share of the width it moved, the bar
  // by what its thumb's travel stands for.
  dragTo(x, cx) {
    const d = this.drag, lay = this.lay;
    if (!d || !lay) return;
    if (d.kind === "split") {
      const ratio = ratioOf(d.from + (x - d.x) / Math.max(1, lay.width - 1));
      if (ratio !== d.ratio) { d.ratio = ratio; cx.notify(); }
    } else {
      const at = Math.max(d.from + (x - d.x) * d.per, 0);
      if (at !== this.scrollX) { this.scrollX = at; cx.notify(); }
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

  // One side's cell: number (click picks the line), then the text, which `sx` moves under the cell's
  // edge. `grow` is its share of the row, the same for every row.
  cell(r, side, th, d, grow, sx) {
    const n = r[side], text = side === "l" ? r.lt ?? r.t : r.rt ?? r.t;
    const changedRow = r.k === "~" || r.k === "-" || r.k === "+";
    const present = n !== undefined;
    const del = side === "l";
    const bg = !changedRow || !present ? null : del ? d.removed_bg : d.added_bg;
    const strong = del ? `${d.removed ?? "#f08a8a"}55` : `${d.added ?? "#86d17a"}55`;
    const index = del ? r.li : r.ri;
    const picked = changedRow && present && this.sel[r.h]?.[del ? "del" : "add"]?.has(index);
    const canPick = changedRow && present && this.view.stageable;
    return h_flex().w(0).flex_grow(grow).min_w(0).h_full().when(Boolean(bg), (el) => el.bg(bg)).when(changedRow && !present, (el) => el.bg(th.raised))
      .child(div().w(GUTTER).flex_none().h_full().pr(6).text_right().text_color(picked ? th.foreground : th.dim)
        .when(picked, (el) => el.bg(del ? d.removed ?? th.ansi1 : d.added ?? th.ansi2).text_color(th.background))
        .when(canPick, (el) => el.cursor_pointer().hover((s) => s.text_color(th.foreground)))
        .child(present ? String(n) : ""))
      .child(h_flex().flex_1().min_w(0).h_full().overflow_hidden()
        .child(div().flex_none().pl(6).ml(-sx).child(present ? this.segments(r[`${side}s`], text, th.foreground, strong) : "")));
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
    const lay = this.lay, thumb = Math.max(24, Math.round(lay.width * lay.width / (lay.width + lay.reach)));
    return div().absolute().left(0).bottom(0).w_full().h(BAR)
      .child(div().id("diff-thumb").absolute().top(2).left(Math.round((lay.width - thumb) * lay.sx / lay.reach)).w(thumb).h(BAR - 4).rounded(3).bg(th.dim)
        .opacity(this.drag?.kind === "bar" ? 0.9 : 0.5).cursor_pointer().hover((s) => s.opacity(0.9))
        .on_mouse_down("left", (e, cx) => {
          cx.stop_propagation();
          this.drag = { kind: "bar", x: e.position.x, from: lay.sx, per: lay.reach / Math.max(1, lay.width - thumb) };
          cx.notify();
        }));
  }

  // The lines [`widest`] chose, laid out out of sight: how wide the font that draws them makes them is
  // how far the sides scroll. So is a run of ruler, which says what a cell is for the lines that were not.
  probes() {
    const fit = this.fit;
    if (!fit?.l) return div();
    const hidden = () => v_flex().absolute().top(0).left(0).h(0).overflow_hidden();
    const side = (name) => hidden()
      .on_size_change((size, cx) => { if (Math.abs(size.width - this.textW[name]) > 0.5) { this.textW[name] = size.width; cx.notify(); } })
      .children(fit[name].show.map((text) => div().whitespace_nowrap().child(text)));
    const ruler = hidden()
      .on_size_change((size, cx) => { if (Math.abs(size.width / RULER.length - this.cellW) > 0.01) { this.cellW = size.width / RULER.length; cx.notify(); } })
      .child(div().whitespace_nowrap().child(RULER));
    return div().child(side("l")).child(side("r")).child(ruler);
  }

  // Side by side: the sides share the width by one ratio whatever their lines hold, each clipping its
  // own, and the lines scroll sideways together.
  sideBody(v, th, d, mono) {
    const ratio = this.ratio(), width = this.rowsWidth ?? 0, reach = this.reach(ratio);
    // What is kept, a wheel's or a thumb's reach past the end included, is brought within the room there is
    // now: a pane made narrower or a divider moved leaves the lines where they are, and does not give them
    // what was asked for beyond the end.
    this.scrollX = Math.min(this.scrollX, reach);
    this.lay = { ratio, width, reach, sx: this.scrollX, l: ratio * 100, r: (1 - ratio) * 100 };
    return div().flex_1().min_h(0).relative().overflow_hidden().font_family(mono).text_size(11.5)
      .child(div().id("diff-rows-box").size_full().pb(reach > 0 ? BAR : 0)
        .on_size_change((size, cx) => { if (Math.abs(size.width - (this.rowsWidth ?? -1)) > 0.5) { this.rowsWidth = size.width; cx.notify(); } })
        .on_mouse_down("left", (e) => { this.downX = e.local_position?.x ?? null; })
        .on_scroll_wheel((e, cx) => this.wheel(e, cx))
        .child(this.rowList(v, th, d, 0)))
      .child(Scrollbar.vertical("diff-rows").viewport_from_layout().absolute().top(0).left(0).right(0).bottom(reach > 0 ? BAR : 0))
      .child(this.splitter(th))
      .when(reach > 0, (el) => el.child(this.hbar(th)))
      .child(this.probes());
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
            .on_size_change((size) => { this.rowsWidth = size.width; })
            .on_mouse_down("left", (e) => { this.downX = e.local_position?.x ?? null; })
            .child(this.rowList(v, th, d, width))))
        .child(Scrollbar.vertical("diff-rows").viewport_from_layout().absolute().inset_0());
    } else body = this.sideBody(v, th, d, mono);

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

// What a UTF-16 unit of a line takes at most, in cells (the width of an ASCII character), from what
// JetBrains Mono and what the system falls back to for the rest draw (scripts/fixtures/yuzora-git-diff-font-widths.json,
// held to by yuzora-git-diff-layout.test.ts): Latin, Greek, Cyrillic, punctuation, arrows and box drawing take 1;
// Chinese, Japanese, full width 1.67 and Korean 1.44; a symbol drawn as an emoji 2.17, and one past U+FFFF is two
// units. Each is rounded up with some to spare, and what is not listed is taken at [`WORST`].
const ASCII = /[\0-\x7f]+/g;
const NARROW = /[^\x80-\u058f\u2000-\u206f\u2190-\u22ff\u2500-\u259f]+/g;
const WIDE = /[^\u1100-\u115f\u2e80-\ua4cf\uac00-\ud7a3\uf900-\ufaff\ufe30-\ufe6f\uff00-\uff60\uffe0-\uffe6]+/g;
const PAIR = /[^\ud800-\udfff]+/g;

// The cells a line takes at most, reading as much of it as `meter` has left to give: a line it reads
// only the first [`LINE`] characters of has the rest taken at [`WORST`], and one it has nothing left
// for is taken at [`WORST`] whole, as is every character that is not ASCII in a line with more than
// [`SORT`] of them. The reading costs what [`TOLL`] and [`SLOW`] say, taken from the meter.
function bound(s, meter) {
  const n = s.length, read = Math.min(n, LINE, Math.max(meter.left, 0));
  if (!read) return WORST * n;
  const head = read < n ? s.slice(0, read) : s;
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

// For each side, the lines to lay out ([`GitDiff.probes`]: the few that may be widest, by what
// [`bound`] says they take at most) and `rest`, the most that any other line may take, in cells. The
// widest line is among the ones laid out or no wider than `rest`, whatever the font. Each line is read
// once for the whole diff and within what [`LOOK`] allows; tabs are spaces by now.
function widest(rows) {
  const meter = { left: LOOK };
  const side = (text) => {
    const best = [];
    let rest = 0;
    for (const r of rows) {
      const s = text(r);
      if (!s) continue;
      // A line that cannot take more than the last of the few is not read.
      const worst = WORST * s.length;
      if (best.length === WIDEST && worst <= best[WIDEST - 1][0]) { if (worst > rest) rest = worst; continue; }
      const b = bound(s, meter);
      if (best.length < WIDEST || b > best[WIDEST - 1][0]) {
        best.push([b, s]);
        best.sort((x, y) => y[0] - x[0]);
        if (best.length > WIDEST) { const out = best.pop()[0]; if (out > rest) rest = out; }
      } else if (b > rest) rest = b;
    }
    // One too long to lay out is among the lines that are not.
    const show = [];
    for (const [b, s] of best) { if (s.length <= LAYOUT) show.push(s); else if (b > rest) rest = b; }
    return { show, rest };
  };
  return { l: side((r) => r.lt ?? r.t), r: side((r) => r.rt ?? r.t) };
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
