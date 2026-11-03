// The sidebar's Git mode: the branch, staged and unstaged changes, the commit
// message and its options. Owned by lane L2; git runs in yuzora:git (Rust).
// Publishes yuzora.badge.git (changed files) and yuzora.status.git.
import { View, div, svg } from "gpui-kit";
import { v_flex, h_flex, TextareaState, Textarea, InputState, Input, v_virtual_list, Scrollbar } from "gpui-base";
import { spaces, panes, focused, theme, run, openOverlay, closeOverlay, notify, shared, setShared, props } from "herdr";
import { t, tokens } from "yuzora:core";
import { open, status, changed, stage, unstage, discard, commitChecks, commit, messages, push, pushPreview, sequencer, fetch, trust, settings as gitSettings } from "yuzora:git";

// Where the Git mode looks: the focused pane's folder, else the space's.
function location() {
  const space = spaces().find((s) => s.active), f = focused();
  const pane = f?.pane ? panes().find((p) => p.pane === f.pane) : null;
  const on = f?.pane === null ? (f.terminal?.includes("/") ? f.terminal.split("/")[0] : null) : space?.machine ?? null;
  return { cwd: pane?.cwd || f?.cwd || space?.folder || null, on };
}
const key = () => JSON.stringify(location());

const ROW = 24, HEAD = 26, ACTION = 22, ACTION_ALL = 84, LIST_PAD = 6;
const SECTIONS = ["conflicts", "staged", "unstaged"];

export default class SidebarGit extends View {
  init(_props, cx) {
    this.menu = props()?.menu ?? null;
    if (this.menu) { this.handle = cx.focus_handle(); this.handle.focus(); return; }
    this.st = null; this.repo = null; this.error = null; this.busy = null; this.note = null;
    this.closed = {}; this.warnings = null; this.showHistory = false; this.more = false;
    this.amend = false; this.signOff = Boolean(gitSettings().signOff); this.noVerify = Boolean(gitSettings().skipHooks);
    this.history = { history: [], template: null, last: null, authors: [] };
    this.message = TextareaState.new({ placeholder: t("commit.placeholder"), rows: 3 });
    this.message.set_auto_grow(3, 10);
    this.message.on("change", (_e, cx) => { const had = Boolean(this.text?.trim()); this.text = this.message.value(); if (had !== Boolean(this.text.trim())) cx.notify(); });
    this.message.on("submit", (e, cx) => { if (e?.modifiers?.platform || e?.modifiers?.control) this.commit(cx, false); });
    this.author = InputState.new({ placeholder: t("commit.author") });
    this.text = "";
    this.attach(cx);
    cx.timer.every(1000, (cx) => { if (key() !== this.ctx) this.attach(cx); });
    this.fetchedAt = Date.now();
    cx.timer.every(60000, (cx) => this.autoFetch(cx));
  }

  // Settings › Git › auto fetch: a quiet fetch every N minutes; the refs it
  // moves come back through the watcher (↑↓ in the branch button).
  autoFetch(cx) {
    const minutes = Number(gitSettings().autoFetchMinutes ?? 0), repo = this.repo;
    // Only once status came back trusted (the engine refuses otherwise too).
    if (!minutes || !repo || !this.st || this.st.trust || this.busy || Date.now() - this.fetchedAt < minutes * 60000) return;
    this.fetchedAt = Date.now();
    cx.spawn(async () => { try { await fetch(repo.id, { auto: true }); } catch {} });
  }

  // A new place: open its repo and follow it until the place changes.
  attach(cx) {
    this.ctx = key();
    const ctx = this.ctx, { cwd, on } = location();
    this.repo = null; this.st = null; this.error = null; this.warnings = null;
    if (!cwd) { this.error = t("noFolder"); this.publish(); cx.notify(); return; }
    cx.spawn(async (cx) => {
      try {
        const repo = await open(cwd, on);
        if (this.ctx !== ctx) return;
        this.repo = repo;
        await this.reload(cx);
        this.loadMessages(cx);
        // Local repos: the watcher bumps the generation. Remote ones: no
        // watcher, so each ~25 s wait without news reads status again.
        let gen = this.st?.generation ?? 0;
        while (this.ctx === ctx) {
          const next = await changed(repo.id, gen);
          if (this.ctx !== ctx) break;
          if (next > gen || repo.machine) { gen = next; await this.reload(cx, Boolean(repo.machine) && next === gen); }
        }
      } catch (e) {
        if (this.ctx !== ctx) return;
        this.error = String(e).includes("not a git repository") ? t("notRepo") : String(e);
        this.publish(); cx.notify();
      }
    });
    cx.notify();
  }

  async reload(cx, force = false) {
    const repo = this.repo;
    if (!repo) return;
    try {
      const st = await status(repo.id, { force });
      if (this.repo?.id !== repo.id) return;
      this.st = st; this.error = null;
    } catch (e) { this.error = String(e); }
    this.publish(); cx.notify();
  }

  loadMessages(cx) {
    const repo = this.repo;
    cx.spawn(async (cx) => {
      try {
        const m = await messages(repo.id);
        if (this.repo?.id !== repo.id) return;
        this.history = m;
        if (!this.text.trim() && m.template) { this.message.set_value(m.template); this.text = m.template; }
        cx.notify();
      } catch {}
    });
  }

  changedFiles() {
    const st = this.st;
    if (!st) return 0;
    return new Set([...st.staged, ...st.unstaged, ...st.untracked, ...st.conflicts].map((f) => f.path)).size;
  }

  // The mode bar's badge and the status bar's Git part.
  publish() {
    const st = this.st, n = this.changedFiles();
    const badge = st && !st.trust ? (n || null) : null;
    let status = null;
    if (st?.trust) {
      status = { text: `⎇ ${t("trust.status")}`, tone: "blocked" };
    } else if (st) {
      const b = st.branch;
      const name = b.detached ? (b.oid ?? "").slice(0, 8) : b.head ?? "";
      const sync = `${b.ahead ? ` ↑${b.ahead}` : ""}${b.behind ? ` ↓${b.behind}` : ""}`;
      const op = st.operation.kind && st.operation.kind !== "none" ? ` · ${t(`op.${st.operation.kind}`)}` : "";
      status = {
        text: `⎇ ${name}${sync}${op} · ${n ? t("status.changes", { count: n }) : t("status.clean")}`,
        tone: st.conflicts.length ? "blocked" : this.busy || op ? "working" : n ? "accent" : "muted",
      };
    }
    if (JSON.stringify(shared("yuzora.badge.git") ?? null) !== JSON.stringify(badge)) setShared("yuzora.badge.git", badge);
    if (JSON.stringify(shared("yuzora.status.git") ?? null) !== JSON.stringify(status)) setShared("yuzora.status.git", status);
  }

  // One git call at a time from here; its error shows under the message box.
  act(cx, name, work) {
    if (this.busy || !this.repo) return;
    this.busy = name; this.note = null; this.publish();
    cx.spawn(async (cx) => {
      try {
        const out = await work(this.repo);
        if (out && out.ok === false) this.note = { tone: "error", text: out.output || t("failed") };
        else if (out?.conflicts?.length) this.note = { tone: "warning", text: t("conflicts", { count: out.conflicts.length }) };
      } catch (e) { this.note = { tone: "error", text: String(e) }; }
      finally { this.busy = null; await this.reload(cx); }
    });
    cx.notify();
  }

  openDiff(f, section) {
    const repo = this.repo;
    if (!repo) return;
    const mode = section === "staged" ? "staged" : "unstaged";
    if (section === "conflicts") return this.openMerge(f.path);
    const spec = { cwd: repo.root, on: repo.machine, path: f.path, orig: f.orig ?? null, mode, at: Date.now() };
    setShared("yuzora.git.diff", spec);
    const pane = panes().find((p) => p.mod === "yuzora-git-diff");
    if (pane) run("pane.focus", { pane: pane.pane });
    else run("tab.new", { name: "Diff", mod: "yuzora-git-diff", props: spec });
  }

  openMerge(path) {
    const repo = this.repo;
    const spec = { cwd: repo.root, on: repo.machine, path: path ?? null, at: Date.now() };
    setShared("yuzora.git.merge", spec);
    const pane = panes().find((p) => p.mod === "yuzora-git-merge");
    if (pane) run("pane.focus", { pane: pane.pane });
    else run("tab.new", { name: t("merge.tab"), mod: "yuzora-git-merge", props: spec });
  }

  openLog() {
    const repo = this.repo;
    if (!repo) return;
    const spec = { cwd: repo.root, on: repo.machine };
    const pane = panes().find((p) => p.mod === "yuzora-git-log" && !p.title?.includes("·"));
    if (pane) run("pane.focus", { pane: pane.pane });
    else run("tab.new", { name: t("log.tab"), mod: "yuzora-git-log", props: spec });
  }

  openBranches() {
    const repo = this.repo;
    if (!repo) return;
    openOverlay({ mod: "yuzora-git-branches", kind: "popup", at: "pointer", width: 340, height: 460, props: { cwd: repo.root, on: repo.machine } });
  }

  // Checks first (IntelliJ's commit checks): blocking ones stop, the rest ask.
  commit(cx, andPush, confirmed = false) {
    const message = this.message.value();
    if (!this.repo || this.busy) return;
    if (!message.trim() && !this.amend) { this.note = { tone: "warning", text: t("commit.empty") }; cx.notify(); return; }
    this.busy = "commit"; this.note = null; this.warnings = null; this.publish();
    cx.spawn(async (cx) => {
      const repo = this.repo;
      try {
        if (!confirmed) {
          const checks = await commitChecks(repo.id, this.amend);
          const blocking = checks.filter((c) => ["identity", "conflicts", "nothing", "rebasing"].includes(c.kind));
          if (blocking.length) { this.note = { tone: "error", text: blocking.map((c) => t(`check.${c.kind}`, { count: c.count ?? c.files?.length ?? 0 })).join("\n") }; return; }
          if (checks.length) { this.warnings = { checks, andPush }; return; }
        }
        const author = this.author.value().trim();
        const out = await commit(repo.id, { message, amend: this.amend, signOff: this.signOff, noVerify: this.noVerify, author: author || undefined });
        if (!out.ok) { this.note = { tone: "error", text: out.output || t("failed") }; return; }
        this.message.set_value(""); this.text = ""; this.amend = false; this.warnings = null;
        notify({ title: t("commit.done"), body: message.split("\n")[0], level: "success", timeout: 3000 });
        this.loadMessages(cx);
        if (andPush) { this.busy = null; await this.reload(cx); this.push(cx); return; }
      } catch (e) { this.note = { tone: "error", text: String(e) }; }
      finally { if (this.busy === "commit") this.busy = null; await this.reload(cx); }
    });
    cx.notify();
  }

  // The push dialog (setting showPushDialog), else straight to the upstream.
  push(cx) {
    const repo = this.repo;
    if (!repo || this.busy) return;
    if (gitSettings().showPushDialog !== false) {
      openOverlay({ mod: "yuzora-git-push", kind: "dialog", width: 520, height: 460, props: { cwd: repo.root, on: repo.machine } });
      return;
    }
    this.act(cx, "push", async (r) => {
      const preview = await pushPreview(r.id);
      return push(r.id, { setUpstream: !preview.upstream, autoUpdate: Boolean(gitSettings().autoUpdateOnReject) });
    });
  }

  toggleAmend(cx) {
    this.amend = !this.amend;
    if (this.amend && !this.message.value().trim() && this.history.last) { this.message.set_value(this.history.last); this.text = this.history.last; }
    if (!this.amend && this.message.value() === this.history.last) { this.message.set_value(""); this.text = ""; }
    cx.notify();
  }

  items() {
    const st = this.st;
    if (!st) return [];
    const lists = { conflicts: st.conflicts.map((c) => ({ ...c, kind: "U" })), staged: st.staged, unstaged: [...st.unstaged, ...st.untracked] };
    const out = [];
    for (const s of SECTIONS) {
      const files = lists[s];
      if (s === "conflicts" && !files.length) continue;
      out.push({ head: true, section: s, count: files.length });
      if (!this.closed[s]) for (const f of files) out.push({ section: s, f });
    }
    return out;
  }

  fileColor(kind, th) {
    const fs = tokens().file_status ?? {};
    if (kind === "A") return fs.added ?? th.ansi2;
    if (kind === "D" || kind === "U") return fs.deleted ?? th.ansi1;
    if (kind === "?") return fs.untracked ?? th.dim;
    return fs.modified ?? th.blocked;
  }

  // Rows are drawn inside the virtual list, where handlers can't be added:
  // the list's on_item_click gets the row, and where the press was (downX,
  // from the list's own mouse down) picks the part of the row.
  headRow(it, th) {
    const titles = { conflicts: t("section.conflicts"), staged: t("section.staged"), unstaged: t("section.unstaged") };
    const action = it.section === "staged" ? t("unstageAll") : it.section === "unstaged" ? t("stageAll") : t("resolve");
    return h_flex().id(`h-${it.section}`).h(HEAD).px(6).gap(4).items_center().text_size(11.5).text_color(th.muted).cursor_pointer()
      .child(div().w(10).text_size(10).child(this.closed[it.section] ? "▸" : "▾"))
      .child(div().flex_1().min_w(0).truncate().child(`${titles[it.section]} · ${it.count}`))
      .when(it.count > 0, (el) => el.child(div().w(ACTION_ALL).flex_none().text_right().text_size(11).hover((s) => s.text_color(th.foreground)).child(action)));
  }

  fileRow(it, th) {
    const f = it.f, d = tokens().diff ?? {};
    const parts = f.path.split("/"), name = parts.pop(), dir = parts.join("/");
    const id = `${it.section}:${f.path}`;
    const glyph = (label) => div().w(ACTION).h(18).flex().flex_none().items_center().justify_center().rounded(4)
      .text_size(12).text_color(th.dim).hover((s) => s.bg(th.hover).text_color(th.foreground)).child(label);
    return h_flex().id(`f-${id}`).h(ROW).pl(6).gap(6).items_center().rounded(5).cursor_pointer()
      .when(this.selected === id, (el) => el.bg(th.selected)).hover((s) => s.bg(th.hover))
      .child(div().w(12).flex_none().text_size(11).font_semibold().text_color(this.fileColor(f.kind, th)).child(f.kind === "?" ? "U" : f.kind))
      .child(h_flex().flex_1().min_w(0).gap(4).child(div().flex_none().max_w(140).truncate().child(name))
        .when(Boolean(dir), (el) => el.child(div().flex_1().min_w(0).truncate().text_size(10.5).text_color(th.dim).child(dir))))
      .when(f.add > 0, (el) => el.child(div().flex_none().text_size(10.5).text_color(d.added ?? th.ansi2).child(`+${f.add}`)))
      .when(f.del > 0, (el) => el.child(div().flex_none().text_size(10.5).text_color(d.removed ?? th.ansi1).child(`−${f.del}`)))
      .child(h_flex().flex_none()
        .when(it.section === "unstaged", (el) => el.child(glyph("↺")).child(glyph("+")))
        .when(it.section === "staged", (el) => el.child(glyph("−")))
        .when(it.section === "conflicts", (el) => el.child(glyph("⇄"))));
  }

  // A click on a row: the action under the pointer, else open its diff.
  clickRow(key, cx) {
    const it = this.items().find((x) => (x.head ? `h:${x.section}` : `${x.section}:${x.f.path}`) === key);
    if (!it) return;
    const fromRight = (this.listWidth ?? 0) - (this.downX ?? 0) - LIST_PAD;
    if (it.head) {
      if (it.count > 0 && fromRight >= 0 && fromRight < ACTION_ALL + 6) {
        if (it.section === "staged") this.act(cx, "stage", (r) => unstage(r.id, []));
        else if (it.section === "unstaged") this.act(cx, "stage", (r) => stage(r.id, []));
        else this.openMerge(null);
      } else { this.closed[it.section] = !this.closed[it.section]; cx.notify(); }
      return;
    }
    const f = it.f, slot = fromRight >= 0 ? Math.floor(fromRight / ACTION) : -1;
    this.selected = key;
    if (it.section === "unstaged" && slot === 0) this.act(cx, "stage", (r) => stage(r.id, [f.path]));
    else if (it.section === "unstaged" && slot === 1) { this.confirmDiscard = f; cx.notify(); }
    else if (it.section === "staged" && slot === 0) this.act(cx, "stage", (r) => unstage(r.id, [f.path]));
    else { this.openDiff(f, it.section); cx.notify(); }
  }

  // Right click: the file's menu, as a popup of this mod (props.menu).
  menuRow(key, cx) {
    const it = this.items().find((x) => !x.head && `${x.section}:${x.f.path}` === key);
    if (!it || !this.repo) return;
    this.selected = key; cx.notify();
    openOverlay({ mod: "yuzora-sidebar-git", kind: "popup", at: "pointer", width: 230, height: 230,
      props: { menu: { cwd: this.repo.root, on: this.repo.machine, section: it.section, path: it.f.path, orig: it.f.orig ?? null } } });
  }

  checkbox(id, on, label, th, toggle) {
    return h_flex().id(id).gap(5).items_center().cursor_pointer().text_size(11).text_color(on ? th.foreground : th.muted)
      .on_click((_e, cx) => toggle(cx))
      .child(div().w(12).h(12).flex().items_center().justify_center().rounded(3).border_1().border_color(on ? th.accent : th.input ?? th.border)
        .bg(on ? th.accent : th.background).text_size(9).text_color(th.background).child(on ? "✓" : ""))
      .child(label);
  }

  button(id, label, primary, enabled, th, action) {
    return h_flex().id(id).h(28).px(10).justify_center().items_center().rounded(6).text_size(11.5)
      .when(primary, (el) => el.flex_1().bg(enabled ? th.foreground : th.raised).text_color(enabled ? th.background : th.dim).font_semibold())
      .when(!primary, (el) => el.border_1().border_color(th.border).text_color(enabled ? th.foreground : th.dim))
      .when(enabled, (el) => el.cursor_pointer().hover((s) => s.opacity(0.88)).on_click((_e, cx) => action(cx)))
      .child(label);
  }

  operationBar(th) {
    const op = this.st?.operation;
    if (!op || !op.kind || op.kind === "none") return null;
    const step = op.step ? ` ${op.step[0]}/${op.step[1]}` : "";
    const att = tokens().attention ?? {};
    const act = (id, label, action) => div().id(id).px(6).h(20).flex().items_center().rounded(4).border_1().border_color(att.border ?? th.border)
      .cursor_pointer().hover((s) => s.bg(th.hover)).on_click((_e, cx) => this.act(cx, "sequencer", (r) => sequencer(r.id, action))).child(label);
    return v_flex().mx(8).mb(6).p(8).gap(6).rounded(6).bg(att.background ?? th.raised).border_1().border_color(att.border ?? th.border)
      .child(div().text_size(11.5).text_color(att.text ?? th.blocked).child(`${t(`op.${op.kind}`)}${step}${op.onto ? ` · ${op.onto}` : ""}`))
      .child(h_flex().gap(4).text_size(11)
        .when(this.st.conflicts.length > 0, (el) => el.child(div().id("op-resolve").px(6).h(20).flex().items_center().rounded(4).bg(th.foreground).text_color(th.background)
          .cursor_pointer().on_click(() => this.openMerge(null)).child(t("resolve"))))
        .when(op.kind !== "bisecting", (el) => el.child(act("op-continue", t("op.continue"), "continue")).child(act("op-abort", t("op.abort"), "abort")))
        .when(op.kind !== "merging" && op.kind !== "bisecting", (el) => el.child(act("op-skip", t("op.skip"), "skip"))));
  }

  // The right-click menu of one file (this mod opened as a popup).
  menuAction(cx, id) {
    const m = this.menu, close = () => closeOverlay(props()?.overlay);
    const spec = { cwd: m.cwd, on: m.on, path: m.path, orig: m.orig };
    if (id === "diff") { this.repo = { root: m.cwd, machine: m.on }; this.openDiff({ path: m.path, orig: m.orig }, m.section); return close(); }
    if (id === "annotate") { run("tab.new", { name: `${t("annotate")} · ${m.path.split("/").pop()}`, mod: "yuzora-git-blame", props: spec }); return close(); }
    if (id === "history") { run("tab.new", { name: `${t("history")} · ${m.path.split("/").pop()}`, mod: "yuzora-git-log", props: { cwd: m.cwd, on: m.on, filter: { paths: [m.path], follow: true } } }); return close(); }
    if (id === "copy") { cx.write_to_clipboard(m.path); return close(); }
    if (id === "discard" && !this.confirm) { this.confirm = true; cx.notify(); return; }
    cx.spawn(async (cx) => {
      try {
        const repo = await open(m.cwd, m.on ?? null);
        const out = id === "stage" ? await stage(repo.id, [m.path]) : id === "unstage" ? await unstage(repo.id, [m.path]) : await discard(repo.id, [m.path]);
        if (out && out.ok === false) { this.note = out.output || t("failed"); cx.notify(); return; }
        close();
      } catch (e) { this.note = String(e); cx.notify(); }
    });
  }

  renderMenu() {
    const th = theme(), m = this.menu;
    const items = [["diff", t("menu.diff")]];
    if (m.section === "unstaged") items.push(["stage", t("stage")], ["discard", this.confirm ? t("menu.discardSure") : t("discard")]);
    if (m.section === "staged") items.push(["unstage", t("unstage")]);
    items.push(["annotate", t("annotate")], ["history", t("history")], ["copy", t("menu.copy")]);
    return v_flex().id("menu").track_focus(this.handle).size_full().p(4).bg(th.popover ?? th.raised).text_color(th.foreground).text_size(12)
      .on_key_down((e, cx) => { if (e.key === "escape") { closeOverlay(props()?.overlay); cx.stop_propagation(); } })
      .child(div().px(8).py(4).text_size(11).text_color(th.dim).truncate().child(m.path))
      .children(items.map(([id, label]) => div().id(`mi-${id}`).h(26).px(8).flex().items_center().rounded(5).cursor_pointer()
        .when(id === "discard" && this.confirm, (el) => el.text_color(th.ansi1))
        .hover((s) => s.bg(th.hover)).on_click((_e, cx) => this.menuAction(cx, id)).child(label)))
      .when(Boolean(this.note), (el) => el.child(div().px(8).py(4).text_size(11).text_color(th.ansi1).child(this.note)));
  }

  render(cx) {
    if (this.menu) return this.renderMenu();
    const th = theme();
    const st = this.st;
    const b = st?.branch;
    const items = this.items();
    const sizes = items.map((it) => (it.head ? HEAD : ROW));
    const ahead = b?.ahead ?? 0, behind = b?.behind ?? 0;
    const canCommit = Boolean(st && !this.busy && (this.text.trim() || this.amend));
    const branchName = !b ? "…" : b.detached ? `${t("detached")} ${(b.oid ?? "").slice(0, 8)}` : b.head ?? "";
    const tone = this.note?.tone === "error" ? th.ansi1 : th.blocked;

    const header = h_flex().h(32).px(12).gap(6).flex_none().items_center()
      .child(div().font_semibold().flex_1().child(t("title")))
      .child(div().id("graph").h(22).px(6).flex().items_center().rounded(4).bg(th.raised).text_size(11).text_color(th.muted)
        .when(Boolean(this.repo), (el) => el.cursor_pointer().hover((s) => s.text_color(th.foreground)).on_click(() => this.openLog()))
        .child(t("graph")));

    if (!st) {
      return v_flex().id("root").size_full().bg(th.chrome).text_color(th.foreground).text_size(12.5)
        .child(header)
        .child(div().px(12).pt(4).text_size(11.5).text_color(th.dim).child(this.error ?? t("loading")));
    }

    // The repo's own config names programs git would run on its own: nothing
    // reads it until the person trusts the folder.
    if (st.trust) {
      const repo = this.repo;
      return v_flex().id("root").size_full().bg(th.chrome).text_color(th.foreground).text_size(12.5)
        .child(header)
        .child(v_flex().mx(8).p(10).gap(8).rounded(6).border_1().border_color(th.blocked).bg(th.raised)
          .child(div().font_semibold().child(t("trust.title")))
          .child(div().text_size(11.5).text_color(th.muted).child(t("trust.body", { keys: st.trust.join(", ") })))
          .child(div().id("trust").h(24).px(10).flex().items_center().justify_center().rounded(4).cursor_pointer()
            .bg(th.accent).text_color(th.background).hover((s) => s.text_color(th.foreground))
            .on_click((_e, cx) => cx.spawn(async (cx) => {
              try { await trust(repo.id); } catch (e) { this.error = String(e); }
              await this.reload(cx, true);
            }))
            .child(t("trust.button"))));
    }

    const branch = h_flex().id("branch").mx(8).mb(6).h(28).px(8).gap(6).flex_none().items_center().rounded(6).border_1().border_color(th.border)
      .bg(th.raised).text_size(11.5).cursor_pointer().hover((s) => s.bg(th.hover)).on_click(() => this.openBranches())
      .child(div().text_color(th.muted).child("⎇"))
      .child(div().flex_1().min_w(0).truncate().child(branchName))
      .when(Boolean(b.upstream) || ahead || behind, (el) => el.child(div().flex_none().text_size(11).text_color(ahead || behind ? (tokens().diff?.added ?? th.ansi2) : th.dim).child(`↑${ahead} ↓${behind}`)))
      .child(div().text_size(9).text_color(th.dim).child("▾"));

    const list = div().id("git-list").flex_1().min_h(0).relative().px(LIST_PAD)
      .on_size_change((size) => { this.listWidth = size.width; })
      .on_mouse_down("left", (e) => { this.downX = e.local_position?.x ?? null; })
      .child(v_virtual_list("git-files", items.length, sizes, (i) => (items[i].head ? `h:${items[i].section}` : `${items[i].section}:${items[i].f.path}`),
        (range) => items.slice(range.start, range.end).map((it) => (it.head ? this.headRow(it, th) : this.fileRow(it, th))))
        .on_item_click((key, cx) => this.clickRow(key, cx))
        .on_item_secondary_click((key, _e, cx) => this.menuRow(key, cx))
        .size_full())
      .child(Scrollbar.vertical("git-files").viewport_from_layout().absolute().inset_0())
      .when(items.length === 0 || (st.counts.total === 0 && !st.conflicts.length), (el) => el.child(div().absolute().top(56).left(0).right(0).text_center().text_size(11.5).text_color(th.dim).child(t("clean"))))
      .when(st.truncated, (el) => el.child(div().absolute().bottom(4).left(8).text_size(10.5).text_color(th.dim).child(t("truncated"))));

    const discardAsk = this.confirmDiscard && v_flex().mx(8).mb(6).p(8).gap(6).rounded(6).bg(th.raised).border_1().border_color(th.border).text_size(11.5)
      .child(div().child(t("discard.ask", { path: this.confirmDiscard.path })))
      .child(h_flex().gap(6)
        .child(this.button("discard-yes", t("discard"), false, true, th, (cx) => { const f = this.confirmDiscard; this.confirmDiscard = null; this.act(cx, "discard", (r) => discard(r.id, [f.path])); }))
        .child(this.button("discard-no", t("cancel"), false, true, th, (cx) => { this.confirmDiscard = null; cx.notify(); })));

    const warnings = this.warnings && v_flex().p(8).gap(6).rounded(6).bg((tokens().attention ?? {}).background ?? th.raised).text_size(11)
      .children(this.warnings.checks.map((c, i) => div().id(`w-${i}`).text_color(th.blocked).child(t(`check.${c.kind}`, { count: c.count ?? c.files?.length ?? 0, files: (c.files ?? []).slice(0, 3).join(", ") }))))
      .child(h_flex().gap(6)
        .child(this.button("w-yes", t("commit.anyway"), false, true, th, (cx) => { const p = this.warnings.andPush; this.warnings = null; this.commit(cx, p, true); }))
        .child(this.button("w-no", t("cancel"), false, true, th, (cx) => { this.warnings = null; cx.notify(); })));

    const history = this.showHistory && v_flex().id("history").max_h(180).overflow_y_scrollbar().rounded(6).border_1().border_color(th.border).bg(th.popover ?? th.raised)
      .children((this.history.history.length ? this.history.history : []).map((m, i) => div().id(`m-${i}`).px(8).py(4).text_size(11).truncate().cursor_pointer()
        .hover((s) => s.bg(th.hover)).on_click((_e, cx) => { this.message.set_value(m); this.text = m; this.showHistory = false; cx.notify(); }).child(m.split("\n")[0])))
      .when(!this.history.history.length, (el) => el.child(div().p(8).text_size(11).text_color(th.dim).child(t("history.empty"))));

    const commitBox = v_flex().flex_none().p(8).gap(6).border_t(1).border_color(th.divider ?? th.border)
      .child(div().id("message").rounded(6).border_1().border_color(th.border).bg(th.background).px(8).py(6).text_size(12).cursor_text()
        .on_mouse_down("left", (_e, cx) => cx.stop_propagation()).child(Textarea.new(this.message).w_full()))
      .child(h_flex().gap(10).flex_wrap()
        .child(this.checkbox("amend", this.amend, t("commit.amend"), th, (cx) => this.toggleAmend(cx)))
        .child(this.checkbox("signoff", this.signOff, t("commit.signOff"), th, (cx) => { this.signOff = !this.signOff; cx.notify(); }))
        .child(this.checkbox("noverify", this.noVerify, t("commit.noVerify"), th, (cx) => { this.noVerify = !this.noVerify; cx.notify(); }))
        .child(div().flex_1())
        .child(div().id("history-toggle").text_size(11).text_color(th.muted).cursor_pointer().hover((s) => s.text_color(th.foreground)).tooltip(t("history"))
          .on_click((_e, cx) => { this.showHistory = !this.showHistory; cx.notify(); }).child(t("history.short")))
        .child(div().id("more").text_size(11).text_color(th.muted).cursor_pointer().hover((s) => s.text_color(th.foreground)).tooltip(t("commit.more"))
          .on_click((_e, cx) => { this.more = !this.more; cx.notify(); }).child("⋯")))
      .when(Boolean(history), (el) => el.child(history))
      .when(this.more, (el) => el.child(div().id("author").h(26).px(8).flex().items_center().rounded(6).border_1().border_color(th.border).bg(th.background).text_size(11.5)
        .on_mouse_down("left", (_e, cx) => cx.stop_propagation()).child(Input.new(this.author).w_full())))
      .when(Boolean(warnings), (el) => el.child(warnings))
      .when(Boolean(this.note), (el) => el.child(div().text_size(11).text_color(tone).child(this.note.text)))
      .child(h_flex().gap(6)
        .child(this.button("commit", this.busy === "commit" ? t("commit.running") : this.amend ? t("commit.amendButton") : t("commit.button"), true, canCommit, th, (cx) => this.commit(cx, false)))
        .child(this.button("commit-push", t("commit.andPush"), false, canCommit, th, (cx) => this.commit(cx, true)))
        .child(this.button("push", this.busy === "push" ? t("push.running") : ahead ? `${t("push")} ↑${ahead}` : t("push"), false, Boolean(!this.busy && (ahead || !b.upstream) && !b.detached), th, (cx) => this.push(cx))));

    return v_flex().id("root").size_full().min_h(0).bg(th.chrome).text_color(th.foreground).text_size(12.5)
      .child(header)
      .child(branch)
      .when(Boolean(this.operationBar(th)), (el) => el.child(this.operationBar(th)))
      .when(Boolean(discardAsk), (el) => el.child(discardAsk))
      .child(list)
      .child(commitBox);
  }
}
