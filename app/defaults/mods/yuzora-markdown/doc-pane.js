// One file in a pane: the code editor, or the Markdown preview with its
// source. Shared by yuzora-editor and yuzora-markdown (identical copies; a
// test keeps them so). Props: {path, on?, line?, column?, opened?}.
import { View, div, svg } from "gpui-kit";
import { v_flex, h_flex, InputState, Input } from "gpui-base";
import { theme, settings, panes, focused, agents, watchFolder, notify, setShared, nextCommand, home, props, window as win } from "herdr";
import { t, tokens } from "yuzora:core";
import {
  openDocument, releaseDocument, documentState, documentChanged, saveDocument, reloadDocument, keepMine,
  checkDisk, focusDocument, gotoLine, notePane, find, findStep, endFind, replaceOne, replaceAll, unsavedCount, Editor, Markdown,
} from "yuzora:files";

const PANES = new Set(["yuzora-editor", "yuzora-markdown"]);
const AGENT_WINDOW = 3 * 60 * 1000;
// A line to go to and focus on open apply only this soon after the pane was
// opened, never again later (a restart, a change made by an agent).
const OPEN_WINDOW = 10 * 1000;

const parent = path => path.slice(0, Math.max(0, path.lastIndexOf("/")));

// The agent most likely writing this file: one working in a folder that holds it.
function editingAgent(state) {
  if (!state?.externalAt || Date.now() - state.externalAt > AGENT_WINDOW) return null;
  const all = panes();
  for (const a of agents()) {
    if (a.status !== "working") continue;
    const cwd = all.find(p => p.pane === a.pane)?.cwd;
    if (cwd && (state.path === cwd || state.path.startsWith(cwd.endsWith("/") ? cwd : cwd + "/"))) return a;
  }
  return null;
}

// Unsaved documents across every pane, for the mode bar's badge and the status bar.
function countUnsaved() {
  const n = unsavedCount();
  setShared("yuzora.badge.files", n || null);
  setShared("yuzora.status.files", n ? { text: t("status.unsaved", { count: n }), tone: "blocked" } : null);
}

export class DocPane extends View {
  /** "editor" or "markdown". */
  kind() { return "editor"; }

  init(_p, cx) {
    const p = props() ?? {};
    this.path = p.path ?? null;
    this.on = p.on ?? null;
    this.owner = `${this.kind()}-${Math.random().toString(36).slice(2, 10)}`;
    this.view = this.kind() === "markdown" ? (localStorage.getItem("view") ?? "preview") : "source";
    this.doc = this.path ? openDocument(this.path, { on: this.on }) : null;
    this.state = this.doc ? documentState(this.doc) : null;
    this.wasDirty = false;
    this.until = (p.opened ?? 0) + OPEN_WINDOW;
    this.fresh = Date.now() < this.until;
    this.goto = p.line && this.fresh ? { line: p.line, column: p.column ?? 1 } : null;
    this.handle = cx.focus_handle();
    this.finding = false; this.replacing = false; this.caseSensitive = false;
    this.findInput = InputState.new({ placeholder: t("find.placeholder") });
    this.findInput.on("change", (_e, cx) => { find(this.doc, this.findInput.value(), { caseSensitive: this.caseSensitive }); cx.notify(); });
    this.findInput.on("submit", (e, cx) => { findStep(this.doc, Boolean(e?.shift)); cx.notify(); });
    this.replaceInput = InputState.new({ placeholder: t("find.replace_placeholder") });
    this.replaceInput.on("submit", (_e, cx) => this.replace(false, cx));
    if (!this.doc) return;
    countUnsaved();
    // pane.go onto this pane (mod.json "focusable").
    cx.spawn(async cx => {
      for (;;) {
        if ((await nextCommand()) === "focus") {
          this.notePane();
          if (this.view === "source" && this.state?.owner === this.owner) focusDocument(this.doc); else this.handle.focus();
          cx.notify();
        }
      }
    });
    this.listen(cx);
    this.watch(cx);
    cx.timer.every(this.on ? 5000 : 15000, cx => { checkDisk(this.doc).catch(() => {}); });
  }

  async close() {
    if (!this.doc) return;
    const state = documentState(this.doc);
    if (state?.dirty) notify({ title: t("toast.kept", { name: state.name }), body: t("toast.kept_body"), level: "warning" });
    releaseDocument(this.doc, this.owner);
    countUnsaved();
  }

  listen(cx) {
    cx.spawn(async cx => {
      let seq = 0;
      for (;;) {
        seq = await documentChanged(this.doc, seq);
        if (!seq) return;
        this.state = documentState(this.doc);
        this.settle();
        cx.notify();
      }
    });
  }

  // Follows the disk: a watch on the file's folder here, a poll on another machine.
  // Watches are recursive on macOS, so a file right in the home folder (or /) is polled only.
  watch(cx) {
    if (this.on) return;
    const dir = parent(this.path) || "/";
    if (dir === "/" || dir === home()) return;
    cx.spawn(async () => {
      try {
        for (;;) {
          const changed = await watchFolder(dir);
          if (changed.some(p => p === this.path || p === dir)) await checkDisk(this.doc).catch(() => {});
        }
      } catch {}
    });
  }

  // The pane is ours while one of ours has focus: the editor (or this view's
  // keys and commands), or a pane just opened, which pane.split focuses.
  notePane() {
    const f = focused();
    if (!f?.pane || this.state?.pane === f.pane) return;
    if (PANES.has(panes().find(p => p.pane === f.pane)?.mod)) notePane(this.doc, f.pane);
  }

  // After each change: remember the pane, jump to a requested line, count unsaved files.
  settle() {
    const s = this.state;
    if (!s) return;
    if (s.focused || this.fresh) this.notePane();
    if (Date.now() > this.until) { this.goto = null; this.fresh = false; }
    // The editor exists once this pane owns it (Rust bumps when it is made).
    const ready = s.phase === "ready" && s.owner === this.owner && this.view === "source";
    if (ready && this.goto) {
      gotoLine(this.doc, this.goto.line, this.goto.column);
      this.goto = null; this.fresh = false;
    } else if (ready && this.fresh) {
      focusDocument(this.doc);
      this.fresh = false;
    }
    if (s.dirty !== this.wasDirty) { this.wasDirty = s.dirty; countUnsaved(); }
  }

  save(force, cx) {
    const s = this.state;
    if (!s || s.phase !== "ready" || s.readonly) return;
    // Nothing to write: a clean save would only rewrite line endings.
    if (!force && !s.dirty && !s.deleted) return;
    saveDocument(this.doc, { force }).then(r => {
      if (!r.ok && !r.conflict) notify({ title: t("error.save", { name: s.name }), body: r.error ?? "", level: "error" });
    });
    cx.notify();
  }

  replace(all, cx) {
    const text = this.replaceInput.value();
    // The search engine moves to the next match by itself.
    (all ? replaceAll : replaceOne)(this.doc, text);
    cx.notify();
  }

  openFind(replace, cx) {
    if (this.state?.readonly) replace = false;
    this.finding = true; this.replacing = replace;
    this.findInput.focus();
    if (this.findInput.value()) find(this.doc, this.findInput.value(), { caseSensitive: this.caseSensitive });
    cx.notify();
  }

  closeFind(cx) {
    this.finding = false; endFind(this.doc); focusDocument(this.doc); cx.notify();
  }

  key(e, cx) {
    this.notePane();
    const mod = e.modifiers.platform || (e.modifiers.control && win()?.platform !== "macos");
    if (mod && e.key === "s") { this.save(false, cx); cx.stop_propagation(); }
    else if (mod && e.key === "f" && this.view === "source") { this.openFind(false, cx); cx.stop_propagation(); }
    else if (mod && e.key === "h" && this.view === "source") { this.openFind(true, cx); cx.stop_propagation(); }
    else if (e.key === "escape" && this.finding) { this.closeFind(cx); cx.stop_propagation(); }
  }

  look(th) {
    return {
      font_family: String(settings().font_family ?? ""), font_size: 12.5, theme: "One Dark",
      colors: { foreground: th.foreground, muted: th.ansi8, status: th.muted, background: th.background, active_line: th.hover, selection: th.selection, caret: th.foreground, border: th.border },
    };
  }

  render(cx) {
    const th = theme(), s = this.state;
    const root = v_flex().id("root").size_full().min_h(0).bg(th.background).text_color(th.foreground).text_size(12.5)
      .track_focus(this.handle).on_key_down((e, cx) => this.key(e, cx));
    if (!this.doc) return root.child(this.message(th, t("message.no_file")));
    return root
      .child(this.header(th, s))
      .children(this.banners(th, s))
      .when(this.finding && this.view === "source", el => el.child(this.findBar(th, s)))
      .child(this.body(th, s));
  }

  header(th, s) {
    const tk = tokens();
    const agent = editingAgent(s);
    const brand = agent ? tk.agents?.[agent.program?.key] : null;
    const toggle = (id, label) => div().id(`view-${id}`).h(18).px(6).rounded(3).flex().items_center().text_size(11).cursor_pointer()
      .bg(this.view === id ? th.input : "#00000000").text_color(this.view === id ? th.foreground : th.muted)
      .on_click((_e, cx) => { this.view = id; localStorage.setItem("view", id); if (id === "source") focusDocument(this.doc); cx.notify(); })
      .child(label);
    const icon = (id, glyph, label, onClick) => div().id(id).w(20).h(20).flex().items_center().justify_center().rounded(4).cursor_pointer()
      .text_color(th.dim).hover(st => st.bg(th.hover).text_color(th.foreground)).tooltip(label).on_click(onClick).child(glyph);
    return h_flex().h(28).flex_none().gap(7).pl(8).pr(6).items_center().text_size(12).border_b(1).border_color(th.border)
      .when(Boolean(s?.badge), el => el.child(div().flex_none().text_size(10).font_bold().text_color(s.badgeColor ?? th.muted).child(s.badge)))
      .child(div().flex_none().max_w(260).truncate().font_semibold().child(s?.name ?? this.path.split("/").pop()))
      .when(Boolean(s?.dirty), el => el.child(div().w(6).h(6).flex_none().rounded_full().bg(th.foreground).tooltip(t("header.unsaved"))))
      .when(this.kind() === "markdown", el => el.child(div().flex_none().text_color(th.dim).child(this.view === "preview" ? t("header.preview") : t("header.source"))))
      .when(this.kind() !== "markdown", el => el.child(div().flex_1().min_w(0).truncate().text_color(th.dim).child(s?.dir ?? "")))
      .when(this.kind() === "markdown", el => el.child(div().flex_1()))
      .when(Boolean(agent), el => el.child(div().flex_none().px(6).py(1).rounded(8).text_size(11)
        .bg(agent.program?.key === "claude" ? "#2a201c" : th.raised).text_color(agent.program?.key === "claude" ? "#e8a27f" : brand ?? th.blocked)
        .child(t("header.agent", { agent: agent.program?.name ?? agent.app ?? "Agent" }))))
      .when(Boolean(s?.readonly) && s?.phase === "ready", el => el.child(div().flex_none().text_size(11).text_color(th.dim).child(t("header.readonly"))))
      .when(this.kind() === "markdown", el => el.child(h_flex().flex_none().p(2).rounded(5).bg(th.raised).child(toggle("source", t("header.source"))).child(toggle("preview", t("header.preview")))))
      .when(this.view === "source" && s?.phase === "ready", el => el.child(icon("find", "⌕", t("find.open"), (_e, cx) => this.finding ? this.closeFind(cx) : this.openFind(false, cx))));
  }

  banners(th, s) {
    if (!s || s.phase !== "ready") return [];
    const button = (id, label, onClick, primary) => div().id(id).h(22).px(8).rounded(5).flex().items_center().cursor_pointer().text_size(11.5)
      .bg(primary ? th.solid : "#00000000").text_color(primary ? th.solid_foreground : th.foreground).border_1().border_color(primary ? th.solid : th.input)
      .on_click(onClick).child(label);
    const banner = (text, tone, buttons = []) => h_flex().flex_none().gap(8).px(10).py(6).items_center().text_size(12)
      .bg(tone === "warn" ? "#1d1a14" : th.raised).border_b(1).border_color(tone === "warn" ? "#8a6420" : th.border)
      .child(div().flex_1().min_w(0).text_color(tone === "warn" ? th.blocked : th.muted).child(text)).children(buttons);
    const out = [];
    if (s.conflict) out.push(banner(t("banner.conflict"), "warn", [
      button("reload", t("banner.reload"), (_e, cx) => { reloadDocument(this.doc); cx.notify(); }),
      button("keep", t("banner.keep"), (_e, cx) => { keepMine(this.doc); cx.notify(); }),
      button("overwrite", t("banner.overwrite"), (_e, cx) => this.save(true, cx), true),
    ]));
    else if (s.deleted) out.push(banner(t("banner.deleted"), "warn", s.readonly ? [] : [button("restore", t("banner.restore"), (_e, cx) => this.save(true, cx), true)]));
    if (s.remote) out.push(banner(t("banner.remote"), "info"));
    else if (s.notUtf8) out.push(banner(t("banner.not_utf8"), "info"));
    else if (s.large && !s.binary && !s.tooLarge) out.push(banner(t("banner.large"), "info"));
    return out;
  }

  findBar(th, s) {
    const count = !this.findInput.value() ? "" : s?.matches ? t("find.count", { current: s.match ?? 0, total: s.matches }) : t("find.none");
    const btn = (id, glyph, label, onClick, on) => div().id(id).h(22).min_w(22).px(4).rounded(4).flex().items_center().justify_center().cursor_pointer()
      .bg(on ? th.selected : "#00000000").text_color(on ? th.foreground : th.muted).hover(st => st.bg(th.hover)).tooltip(label).on_click(onClick).child(glyph);
    const field = input => Input.new(input).h(24).px(6).flex_1().min_w(0).rounded(4).bg(th.canvas).border_1().border_color(th.input).text_size(12);
    return v_flex().flex_none().gap(4).px(8).py(6).border_b(1).border_color(th.border).bg(th.chrome)
      .child(h_flex().gap(4).items_center()
        .child(btn("find-replace", this.replacing ? "▾" : "▸", t("find.toggle_replace"), (_e, cx) => { this.replacing = !this.replacing && !s?.readonly; cx.notify(); }))
        .child(field(this.findInput))
        .child(div().w(64).flex_none().text_size(11).text_color(th.dim).child(count))
        .child(btn("find-case", "Aa", t("find.case"), (_e, cx) => { this.caseSensitive = !this.caseSensitive; find(this.doc, this.findInput.value(), { caseSensitive: this.caseSensitive }); cx.notify(); }, this.caseSensitive))
        .child(btn("find-prev", "↑", t("find.prev"), (_e, cx) => { findStep(this.doc, true); cx.notify(); }))
        .child(btn("find-next", "↓", t("find.next"), (_e, cx) => { findStep(this.doc, false); cx.notify(); }))
        .child(btn("find-close", "×", t("find.close"), (_e, cx) => this.closeFind(cx))))
      .when(this.replacing, el => el.child(h_flex().gap(4).items_center().pl(26)
        .child(field(this.replaceInput))
        .child(btn("replace-one", t("find.replace"), t("find.replace"), (_e, cx) => this.replace(false, cx)))
        .child(btn("replace-all", t("find.replace_all"), t("find.replace_all"), (_e, cx) => this.replace(true, cx)))));
  }

  message(th, text) {
    return div().flex_1().min_h(0).flex().items_center().justify_center().p(24).text_size(12.5).text_color(th.dim).child(text);
  }

  body(th, s) {
    if (!s || s.phase === "loading") return this.message(th, t("message.loading"));
    if (s.phase === "failed") return this.message(th, t("message.failed", { error: s.error ?? "" }));
    if (s.binary) return this.message(th, t("message.binary"));
    if (s.tooLarge) return this.message(th, t("message.too_large"));
    if (this.view === "preview") {
      return div().flex_1().min_h(0).child(Markdown.new(`md-${this.owner}`, { doc: this.doc,
        colors: { foreground: "#f4f4f5", body: "#c9c9cf", muted: th.muted, link: "#61afef", code: th.raised, border: th.border } }));
    }
    return div().flex_1().min_h(0).child(Editor.new(`ed-${this.owner}`, { doc: this.doc, owner: this.owner, status: true, ...this.look(th) }));
  }
}
