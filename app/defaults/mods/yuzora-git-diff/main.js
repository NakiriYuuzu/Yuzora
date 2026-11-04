// The diff pane: one file, side by side or unified, with word highlights;
// stage, unstage or discard hunks or lines without touching the rest.
// Follows shared "yuzora.git.diff" ({cwd, on, path, orig, mode, rev?, to?, at}).
import { View, div } from "gpui-kit";
import { v_flex, h_flex, v_virtual_list, Scrollbar, VirtualListScrollHandle } from "gpui-base";
import { theme, props, shared, settings, run, panes, focused, closeOverlay } from "herdr";
import { t, tokens } from "yuzora:core";
import { open, changed, diffOpen, diffView, diffStage, diffUnstage, diffDiscard, settings as gitSettings } from "yuzora:git";

const ROW = 20, CHAR = 7, GUTTER = 44;

export default class GitDiff extends View {
  init(_props, cx) {
    const s = gitSettings(), p = props() ?? {};
    // Kept from a modal: the view it had there.
    this.layout = (p.layout ?? s.diffMode) === "unified" ? "unified" : "side";
    this.whitespace = p.whitespace ?? s.diffIgnoreWhitespace ?? "none";
    this.context = Number(p.context ?? s.diffContext ?? 3);
    this.view = null; this.error = null; this.busy = false; this.note = null;
    this.sel = {}; this.hunk = 0; this.askDiscard = null;
    this.scroll = VirtualListScrollHandle.new();
    this.handle = cx.focus_handle();
    // Props without `at` (an agent's tab.new) date from now, not from 1970.
    this.born = Date.now();
    this.follow(cx);
  }

  // The newest of the props and the shared request.
  wanted() {
    const p = props() ?? {}, s = shared("yuzora.git.diff");
    // In a modal, only what it was opened with.
    if (p.overlay) return p.path ? p : null;
    return s?.path && (!p.path || (s.at ?? 0) >= (p.at ?? this.born ?? 0)) ? s : p.path ? p : null;
  }

  follow(cx) {
    const spec = this.wanted();
    this.spec = spec;
    this.loop = (this.loop ?? 0) + 1;
    const loop = this.loop;
    this.view = null; this.error = null; this.sel = {}; this.hunk = 0; this.note = null;
    // Called from render too: the state above is what this frame draws, so no notify here.
    if (!spec) return;
    cx.spawn(async (cx) => {
      try {
        const repo = await open(spec.cwd, spec.on ?? null);
        if (this.loop !== loop) return;
        this.repo = repo;
        await this.load(cx, loop);
        // Work tree and index diffs follow the repo; commits never change.
        if (spec.mode !== "unstaged" && spec.mode !== "staged") return;
        let gen = 0;
        while (this.loop === loop) {
          const next = await changed(repo.id, gen);
          if (this.loop !== loop) break;
          if (gen && next > gen && !this.busy) await this.load(cx, loop);
          gen = next;
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
      this.view = view; this.error = null;
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
      try { this.view = await diffView(this.view.id, { layout: this.layout, context: this.context, whitespace: this.whitespace }); this.sel = {}; }
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
        const half = ((this.rowsWidth ?? 0) - 1) / 2;
        if (x >= 0 && x < GUTTER && r.l !== undefined) return this.toggleLine(r.h, "l", r.li, cx);
        if (x > half && x < half + 1 + GUTTER && r.r !== undefined) return this.toggleLine(r.h, "r", r.ri, cx);
      }
    }
    this.hunk = r.h; cx.notify();
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

  // One side's cell: number (click picks the line), then the text.
  cell(r, side, th, d, width) {
    const n = r[side], text = side === "l" ? r.lt ?? r.t : r.rt ?? r.t;
    const changedRow = r.k === "~" || r.k === "-" || r.k === "+";
    const present = n !== undefined;
    const del = side === "l";
    const bg = !changedRow || !present ? null : del ? d.removed_bg : d.added_bg;
    const strong = del ? `${d.removed ?? "#f08a8a"}55` : `${d.added ?? "#86d17a"}55`;
    const index = del ? r.li : r.ri;
    const picked = changedRow && present && this.sel[r.h]?.[del ? "del" : "add"]?.has(index);
    const canPick = changedRow && present && this.view.stageable;
    return h_flex().flex_1().min_w(width).h_full().when(Boolean(bg), (el) => el.bg(bg)).when(changedRow && !present, (el) => el.bg(th.raised))
      .child(div().w(GUTTER).flex_none().h_full().pr(6).text_right().text_color(picked ? th.foreground : th.dim)
        .when(picked, (el) => el.bg(del ? d.removed ?? th.ansi1 : d.added ?? th.ansi2).text_color(th.background))
        .when(canPick, (el) => el.cursor_pointer().hover((s) => s.text_color(th.foreground)))
        .child(present ? String(n) : ""))
      .child(div().pl(6).flex_1().child(present ? this.segments(r[`${side}s`], text, th.foreground, strong) : ""));
  }

  row(r, i, th, d, width) {
    if (r.k === "fold") {
      return h_flex().id(`r${i}`).h(ROW).px(10).items_center().bg(th.raised).text_color(th.dim).cursor_pointer()
        .hover((s) => s.text_color(th.foreground)).child(t("fold", { count: r.n }));
    }
    const current = r.h !== undefined && r.h === this.hunk;
    const base = h_flex().id(`r${i}`).h(ROW).w_full().when(current, (el) => el.border_l(2).border_color(th.accent));
    if (this.layout === "unified") {
      const del = r.k === "-", add = r.k === "+";
      const bg = del ? d.removed_bg : add ? d.added_bg : null;
      const index = del ? r.li : r.ri;
      const side = del ? "del" : "add";
      const picked = (del || add) && this.sel[r.h]?.[side]?.has(index);
      const canPick = (del || add) && this.view.stageable;
      return base.when(Boolean(bg), (el) => el.bg(bg))
        .child(div().w(GUTTER * 2).flex_none().h_full().flex().text_color(picked ? th.background : th.dim)
          .when(picked, (el) => el.bg(del ? d.removed ?? th.ansi1 : d.added ?? th.ansi2))
          .when(canPick, (el) => el.cursor_pointer())
          .child(div().w(GUTTER).pr(6).text_right().child(r.l !== undefined ? String(r.l) : ""))
          .child(div().w(GUTTER).pr(6).text_right().child(r.r !== undefined ? String(r.r) : "")))
        .child(div().w(14).flex_none().text_color(del ? d.removed : add ? d.added : th.dim).child(del ? "−" : add ? "+" : ""))
        .child(div().flex_1().min_w(width).child(this.segments(r.s, r.t, th.foreground, del ? `${d.removed}55` : `${d.added}55`)));
    }
    return base.child(this.cell(r, "l", th, d, width)).child(div().w(1).h_full().bg(th.border)).child(this.cell(r, "r", th, d, width));
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
    const root = v_flex().id("git-diff").size_full().min_h(0).bg(th.background).text_color(th.foreground).text_size(12)
      .track_focus(this.handle).on_key_down((e, cx) => this.key(e, cx)).on_mouse_down("left", () => this.handle.focus());
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
    else {
      const longest = Math.min(2000, v.rows.reduce((m, r) => Math.max(m, (r.t ?? "").length, (r.lt ?? "").length, (r.rt ?? "").length), 0));
      const width = longest * CHAR + 24;
      const total = this.layout === "unified" ? GUTTER * 2 + 14 + width : 2 * (GUTTER + width) + 1;
      body = div().flex_1().min_h(0).relative().font_family(mono).text_size(11.5)
        .child(div().id("diff-x").size_full().overflow_x_scrollbar()
          .child(div().id("diff-rows-box").w_full().min_w(total).h_full()
            .on_size_change((size) => { this.rowsWidth = size.width; })
            .on_mouse_down("left", (e) => { this.downX = e.local_position?.x ?? null; })
            .child(v_virtual_list("diff-rows", v.rows.length, ROW, (i) => String(i),
              (range) => v.rows.slice(range.start, range.end).map((r, k) => this.row(r, range.start + k, th, d, width)))
              .on_item_click((key, cx) => this.clickRow(Number(key), cx))
              .track_scroll(this.scroll).size_full())))
        .child(Scrollbar.vertical("diff-rows").viewport_from_layout().absolute().inset_0());
    }

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

// A diff pane beside the focused pane, or in a tab of its own (Git settings).
function placeDiff(spec, where) {
  const pane = where !== "tab" && focused()?.pane;
  if (pane) run("pane.split", { direction: "right", pane, mod: "yuzora-git-diff", props: spec });
  else run("tab.new", { name: t("tab.diff"), mod: "yuzora-git-diff", props: spec });
}
