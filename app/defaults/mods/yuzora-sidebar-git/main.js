// The sidebar's Git mode: the branch, staged and unstaged changes, the commit
// message and its options. Owned by lane L2; git runs in yuzora:git (Rust).
// Publishes yuzora.badge.git (changed files) and yuzora.status.git.
import { View, div, svg } from "gpui-kit";
import { v_flex, h_flex, TextareaState, Textarea, InputState, Input, v_virtual_list, Scrollbar } from "gpui-base";
import { spaces, panes, focused, theme, run, openOverlay, closeOverlay, notify, shared, setShared, props, nextCommand } from "herdr";
import { t, tokens } from "yuzora:core";
import { open, status, changed, stage, unstage, discard, commitChecks, commit, messages, push, pushPreview, sequencer, fetch, trust, repos, settings as gitSettings } from "yuzora:git";

// Which pane an opener reuses. panes() lists the panes of every space and pane.focus selects
// the space of the pane it is given, so a pane is only looked for among the tabs of the shown
// space. The functions down to the end of this block are the same in every Git mod that has
// them (a mod cannot import another mod's files): scripts/yuzora-git-pane-reuse.test.ts keeps
// them equal.
function shownSpace() {
  return spaces().find((s) => s.active) ?? null;
}

function findPane(mod, space) {
  const tabs = new Set((space?.tabs ?? []).map((x) => x.id));
  return panes().find((p) => p.mod === mod && tabs.has(p.tab)) ?? null;
}

// A diff, merge or rebase pane is asked in shared `<key>.<space id>`, so a space's pane keeps what
// its space asked for while another space asks for its own.
function requestKey(key, space) {
  return `${key}.${space}`;
}

// Merge and rebase: the shown space's pane is asked in its space's request and focused, else a tab
// of its own gets the request as its props.
function openRequested(key, mod, name, fields) {
  const space = shownSpace();
  if (!space) return;
  const spec = { ...fields, at: Date.now() };
  const pane = findPane(mod, space);
  if (pane) {
    setShared(requestKey(key, space.id), spec);
    run("pane.focus", { pane: pane.pane });
  } else run("tab.new", { name, space: space.id, mod, props: spec });
}

// What each graph pane shows, by tab: { [tab id]: { space, root, on } }. A mod cannot read another
// pane's props, so a graph pane says it here once its repo is open (yuzora-git-log) and the Git
// sidebar looks it up (findLog). The place stays when the pane closes: the tab and its graph are
// still there, a quit closes every pane, and after a restart a tab that is not shown has no pane
// running to say it again. Writing drops the places of tabs that are gone, except a machine's: its
// spaces are not listed until it is connected.
const LOGS = "yuzora.git.logs";
function findLog(space, root, on) {
  const logs = shared(LOGS) ?? {}, tabs = new Set(space.tabs.map((x) => x.id));
  return panes().find((p) => p.mod === "yuzora-git-log" && tabs.has(p.tab) && logs[p.tab]?.root === root && (logs[p.tab].on ?? null) === (on ?? null)) ?? null;
}
// ---- end of the shared pane helpers ----

// Where the Git mode looks: the focused pane's folder, else the space's.
function location() {
  const space = spaces().find((s) => s.active), f = focused();
  const pane = f?.pane ? panes().find((p) => p.pane === f.pane) : null;
  const on = f?.pane === null ? (f.terminal?.includes("/") ? f.terminal.split("/")[0] : null) : space?.machine ?? null;
  return { cwd: pane?.cwd || f?.cwd || space?.folder || null, on };
}

// The repo picked for the active space (shared `yuzora.git.repo.<space id>`,
// kept across restarts), else none: the Git mode follows the focus.
const pinKey = (space) => `yuzora.git.repo.${space}`;
function pinned() {
  const space = spaces().find((s) => s.active);
  const pin = space ? shared(pinKey(space.id)) : null;
  return pin?.root ? { space: space.id, root: pin.root, on: pin.on ?? null } : null;
}

// Where the Git mode looks now: the pinned repo, else location().
function place() {
  const pin = pinned();
  return pin ? { cwd: pin.root, on: pin.on, pin: pin.space } : location();
}
const key = () => JSON.stringify(place());
const baseName = (path) => path.replace(/[\\/]+$/, "").split(/[\\/]/).pop() || path;

const ROW = 24, HEAD = 26, ACTION = 22, ACTION_ALL = 84, LIST_PAD = 6, PICK_ROW = 26, PICK_MAX = 358;
// Line counts of six digits would not fit the narrowest sidebar next to the buttons: 150000 reads 150k.
const count = (n) => (n < 100000 ? String(n) : n < 1000000 ? `${Math.round(n / 1000)}k` : "999k+");
const SECTIONS = ["conflicts", "staged", "unstaged"];

export default class SidebarGit extends View {
  init(_props, cx) {
    this.menu = props()?.menu ?? null;
    if (this.menu) { this.handle = cx.focus_handle(); this.handle.focus(); return; }
    this.picker = props()?.picker ?? null;
    if (this.picker) { this.handle = cx.focus_handle(); this.handle.focus(); this.findRepos(cx); return; }
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
    // "yuzora-sidebar-git.pick-repo" (a key in keys.json, or herdr-app do) opens the repo picker.
    cx.spawn(async () => { for (;;) if ((await nextCommand()) === "pick-repo") this.openPicker("dialog"); });
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
    const ctx = this.ctx, here = place(), { cwd, on } = here;
    this.place = here;
    this.repo = null; this.st = null; this.error = null; this.warnings = null;
    if (!cwd) { this.error = t("noFolder"); this.publish(); cx.notify(); return; }
    cx.spawn(async (cx) => {
      try {
        const repo = await open(cwd, on);
        if (this.ctx !== ctx) return;
        // A pinned repo opens as itself (git may spell it another way: a subst
        // drive on Windows, say; the file system tells). Without its `.git`
        // git opens the repo above it instead: unpinned. With one git won't
        // take now (it can't be read, say), the pin stays, and so does the
        // error: not the repo above.
        if (here.pin && repo.root !== cwd && !(await this.sameFolder(repo.root, here))) {
          if (await this.unpinIfGone(here)) { if (this.ctx === ctx) this.attach(cx); return; }
          if (this.ctx !== ctx) return;
          this.error = t("repo.opensAbove", { root: repo.root });
          this.publish(); cx.notify();
          return;
        }
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
        if (here.pin && await this.unpinIfGone(here)) { if (this.ctx === ctx) this.attach(cx); return; }
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
    } catch (e) {
      this.error = String(e);
      const here = this.place;
      if (here?.pin && this.repo?.id === repo.id && await this.unpinIfGone(here)) { this.attach(cx); return; }
    }
    this.publish(); cx.notify();
  }

  // A pinned repo that's gone (its folder, or the `.git` in it: the file
  // system says, no git runs) is unpinned, with a note: the Git mode follows
  // the focus again. One that can't be looked at now (a machine away, no
  // permission to its folder: `repos` fails) stays.
  async unpinIfGone(here) {
    try {
      const found = await repos(here.cwd, here.on, []);
      if (found.repos.some((r) => r.rel === ".")) return false;
    } catch { return false; }
    return this.unpin(here);
  }

  // Whether `root` (as git names it) is `here`'s pinned folder: their real
  // paths, as `repos` gives them, are the same.
  async sameFolder(root, here) {
    try { return (await repos(root, here.on, [])).repos.some((r) => r.rel === "." && r.root === here.cwd); }
    catch { return false; }
  }

  // Unpins `here`, with a note (unless it was pinned again meanwhile).
  unpin(here) {
    if (shared(pinKey(here.pin))?.root !== here.cwd) return false;
    setShared(pinKey(here.pin), null);
    notify({ title: t("repo.gone", { name: baseName(here.cwd) }), body: here.cwd, level: "warning", timeout: 5000 });
    return true;
  }

  // The repo picker: follow the focus, or pin one of the space's repos (the
  // space's folder, its panes' folders: no git runs to list them).
  // At the pointer (a click on the header), or centered (the command).
  openPicker(kind = "popup") {
    const space = spaces().find((s) => s.active);
    if (!space) return;
    const tabs = new Set((space.tabs ?? []).map((tab) => tab.id));
    const cwds = [...new Set(panes().filter((p) => tabs.has(p.tab) && p.cwd).map((p) => p.cwd))];
    const folder = space.folder || cwds[0];
    if (!folder) return;
    const pin = pinned();
    openOverlay({ mod: "yuzora-sidebar-git", kind, ...(kind === "popup" ? { at: "pointer", height: 360 } : {}), width: 340,
      props: { picker: { space: space.id, folder, on: space.machine ?? null, cwds, current: pin?.root ?? this.repo?.root ?? null, pinned: Boolean(pin) } } });
  }

  // The picker popup (props.picker) lists the repos itself.
  findRepos(cx) {
    const p = this.picker;
    this.found = null;
    cx.spawn(async (cx) => {
      try { this.found = await repos(p.folder, p.on, p.cwds); }
      catch (e) { this.found = { repos: [], truncated: false, error: String(e) }; }
      cx.notify();
    });
  }

  // Pins `root` for the picker's space (none: follow the focus).
  pick(root) {
    const p = this.picker;
    setShared(pinKey(p.space), root ? { root, on: p.on } : null);
    closeOverlay(props()?.overlay);
  }

  // The picker's row the keys are on: 0 "follow the focus", then the repos;
  // at first the one picked now.
  pickAt(list) {
    const p = this.picker;
    return this.hi ?? (p.pinned ? list.findIndex((r) => r.root === p.current) + 1 : 0);
  }

  pickKey(e, cx, list) {
    const n = list.length + 1, at = this.pickAt(list);
    if (e.key === "escape") closeOverlay(props()?.overlay);
    else if (e.key === "down") this.hi = (at + 1) % n;
    else if (e.key === "up") this.hi = (at - 1 + n) % n;
    else if (e.key === "enter") { if (this.found) this.pick(at ? list[at - 1].root : null); }
    else return;
    cx.stop_propagation(); cx.notify();
  }

  renderPicker() {
    const th = theme(), p = this.picker, found = this.found, list = found?.repos ?? [], at = this.pickAt(list);
    const row = (id, title, detail, on, extra, action, i) => h_flex().id(id).h(PICK_ROW).px(8).gap(6).flex_none().items_center().rounded(5).cursor_pointer()
      .when(on, (el) => el.bg(th.selected)).when(!on && i === at, (el) => el.bg(th.hover)).hover((s) => s.bg(th.hover)).on_click(() => action())
      .child(div().w(10).flex_none().text_size(10).text_color(th.accent).child(on ? "✓" : ""))
      .child(div().flex_none().max_w(150).truncate().child(title))
      .child(div().flex_1().min_w(0).truncate().text_size(10.5).text_color(th.dim).child(detail ?? ""))
      .when(Boolean(extra), (el) => el.child(div().flex_none().text_size(10.5).text_color(th.muted).child(extra)));
    // As a popup the overlay is as tall as its content: the column takes its rows' height (PICK_MAX at most: 360, the popup's, less its border) and only
    // then does the list shrink and scroll. Nothing here waits for a height to fill: not size_full or flex_1, and not
    // overflow_y_scrollbar, whose scroll area fills its parent (the list was 0 high).
    return v_flex().id("picker").track_focus(this.handle).w_full().max_h(PICK_MAX).min_h(0).p(4).bg(th.popover ?? th.raised).text_color(th.foreground).text_size(12)
      .on_key_down((e, cx) => this.pickKey(e, cx, list))
      .child(row("pick-follow", t("repo.follow"), !p.pinned && p.current ? baseName(p.current) : null, !p.pinned, null, () => this.pick(null), 0))
      .child(div().h(1).mx(4).my(2).flex_none().bg(th.divider ?? th.border))
      .child(v_flex().id("pick-list").min_h(0).overflow_y_scroll()
        .children(list.map((r, i) => row(`pick-${i}`, r.name, r.rel === "." ? t("repo.here") : r.rel, p.pinned && r.root === p.current, r.trusted ? t("repo.trusted") : null, () => this.pick(r.root), i + 1)))
        .when(!found, (el) => el.child(div().px(8).py(4).text_size(11).text_color(th.dim).child(t("repo.loading"))))
        .when(Boolean(found) && !list.length, (el) => el.child(div().px(8).py(4).text_size(11).text_color(found.error ? th.ansi1 : th.dim).child(found.error ?? t("repo.empty"))))
        .when(Boolean(found?.truncated), (el) => el.child(div().px(8).py(4).text_size(10.5).text_color(th.dim).child(t("repo.truncated", { count: list.length })))));
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
    showDiff(spec);
  }

  openMerge(path) {
    const repo = this.repo;
    openRequested("yuzora.git.merge", "yuzora-git-merge", t("merge.tab"), { cwd: repo.root, on: repo.machine, path: path ?? null });
  }

  openLog() {
    const repo = this.repo, space = shownSpace();
    if (!repo || !space) return;
    const pane = findLog(space, repo.root, repo.machine);
    if (pane) run("pane.focus", { pane: pane.pane });
    else run("tab.new", { name: t("log.tab"), space: space.id, mod: "yuzora-git-log", props: { cwd: repo.root, on: repo.machine } });
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
      .child(div().w(10).flex_none().text_size(10).child(this.closed[it.section] ? "▸" : "▾"))
      .child(div().flex_1().min_w(0).truncate().child(`${titles[it.section]} · ${it.count}`))
      .when(it.count > 0, (el) => el.child(div().w(ACTION_ALL).flex_none().text_right().text_size(11).hover((s) => s.text_color(th.foreground)).child(action)));
  }

  // The buttons are flex_none at the right edge, where clickRow counts slots from, so nothing may
  // push them: the folder (flex_1, no basis) gives way first, then the name, and last the +N −M,
  // which can shrink to a "…" in a layout narrower than the 180px minimum.
  // `probe(slot, "move")` (tests only) returns a handler for a part of the row: its size, or where it sits.
  fileRow(it, th, probe) {
    const f = it.f, d = tokens().diff ?? {};
    const parts = f.path.split("/"), name = parts.pop(), dir = parts.join("/");
    const id = `${it.section}:${f.path}`;
    const seen = (el, slot) => (probe ? el.on_size_change(probe(slot)).on_mouse_move(probe(slot, "move")) : el);
    const glyph = (label) => seen(div().id(`g-${label}`).w(ACTION).h(18).flex().flex_none().items_center().justify_center().rounded(4)
      .text_size(12).text_color(th.dim).hover((s) => s.bg(th.hover).text_color(th.foreground)).child(label), `action:${label}`);
    return seen(h_flex().id(`f-${id}`).h(ROW).pl(6).gap(6).items_center().rounded(5).cursor_pointer()
      .when(this.selected === id, (el) => el.bg(th.selected)).hover((s) => s.bg(th.hover))
      .child(div().w(12).flex_none().text_size(11).font_semibold().text_color(this.fileColor(f.kind, th)).child(f.kind === "?" ? "U" : f.kind))
      .child(seen(h_flex().id("text").flex_1().min_w(0).gap(4), "text").child(seen(div().id("name").flex_shrink(1).min_w(0).max_w(140).truncate().child(name), "name"))
        .when(Boolean(dir), (el) => el.child(seen(div().id("dir").flex_1().min_w(0).truncate().text_size(10.5).text_color(th.dim).child(dir), "dir"))))
      .when(f.add > 0, (el) => el.child(seen(div().id("add").flex_shrink(1).min_w(0).truncate().text_size(10.5).text_color(d.added ?? th.ansi2).child(`+${count(f.add)}`), "add")))
      .when(f.del > 0, (el) => el.child(seen(div().id("del").flex_shrink(1).min_w(0).truncate().text_size(10.5).text_color(d.removed ?? th.ansi1).child(`−${count(f.del)}`), "del")))
      .child(h_flex().flex_none()
        .when(it.section === "unstaged", (el) => el.child(glyph("↺")).child(glyph("+")))
        .when(it.section === "staged", (el) => el.child(glyph("−")))
        .when(it.section === "conflicts", (el) => el.child(glyph("⇄")))), "row");
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
    if (this.picker) return this.renderPicker();
    const th = theme();
    const st = this.st;
    const b = st?.branch;
    const items = this.items();
    const sizes = items.map((it) => (it.head ? HEAD : ROW));
    const ahead = b?.ahead ?? 0, behind = b?.behind ?? 0;
    const canCommit = Boolean(st && !this.busy && (this.text.trim() || this.amend));
    const branchName = !b ? "…" : b.detached ? `${t("detached")} ${(b.oid ?? "").slice(0, 8)}` : b.head ?? "";
    const tone = this.note?.tone === "error" ? th.ansi1 : th.blocked;

    // Which repo: the one the focus is in, or the one pinned for this space.
    const here = this.place, pin = Boolean(here?.pin);
    const repoName = this.repo ? baseName(this.repo.root) : pin ? baseName(here.cwd) : t("repo.none");
    const header = h_flex().h(32).px(12).gap(6).flex_none().items_center()
      .child(div().font_semibold().flex_none().child(t("title")))
      .child(h_flex().id("repo-pick").flex_1().min_w(0).h(22).px(6).gap(4).items_center().rounded(4).text_size(11).text_color(th.muted)
        .cursor_pointer().hover((s) => s.bg(th.hover).text_color(th.foreground)).tooltip(pin ? t("repo.pinnedTip") : t("repo.followTip"))
        .on_click(() => this.openPicker())
        .when(pin, (el) => el.child(div().flex_none().w(6).h(6).rounded(3).bg(th.accent)))
        .child(div().min_w(0).truncate().child(repoName))
        .child(div().flex_none().text_size(9).text_color(th.dim).child("▾")))
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
            .bg(th.accent).text_color(th.accent_foreground).hover((s) => s.opacity(0.88))
            .on_click((_e, cx) => cx.spawn(async (cx) => {
              try { await trust(repo.id); } catch (e) { this.error = String(e); }
              await this.reload(cx, true);
            }))
            .child(t("trust.button"))));
    }

    const branch = h_flex().id("branch").mx(8).mb(6).h(28).px(8).gap(6).flex_none().items_center().rounded(6).border_1().border_color(th.border)
      .bg(th.raised).text_size(11.5).cursor_pointer().hover((s) => s.bg(th.hover)).on_click(() => this.openBranches())
      .child(div().flex_none().text_color(th.muted).child("⎇"))
      .child(div().flex_1().min_w(0).truncate().child(branchName))
      .when(Boolean(b.upstream) || ahead || behind, (el) => el.child(div().flex_none().text_size(11).text_color(ahead || behind ? (tokens().diff?.added ?? th.ansi2) : th.dim).child(`↑${ahead} ↓${behind}`)))
      .child(div().flex_none().text_size(9).text_color(th.dim).child("▾"));

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

// A diff pane beside the focused pane, or in a tab of its own (Git settings).
function placeDiff(spec, where) {
  const pane = where !== "tab" && focused()?.pane;
  if (pane) run("pane.split", { direction: "right", pane, mod: "yuzora-git-diff", props: spec });
  else run("tab.new", { name: t("tab.diff"), mod: "yuzora-git-diff", props: spec });
}

// Where a diff opens (Git settings): into the open diff pane, which follows
// the shared request; else a modal, or straight into a pane as Keep would.
function showDiff(spec) {
  const s = gitSettings(), space = shownSpace();
  const pane = s.diffFollowPane !== false && findPane("yuzora-git-diff", space);
  if (pane) {
    setShared(requestKey("yuzora.git.diff", space.id), spec);
    return run("pane.focus", { pane: pane.pane });
  }
  if (s.diffOpenIn === "pane") return placeDiff(spec, s.diffKeepIn);
  openOverlay({ mod: "yuzora-git-diff", kind: "dialog", width: 0.9, height: 0.86, props: spec });
}
