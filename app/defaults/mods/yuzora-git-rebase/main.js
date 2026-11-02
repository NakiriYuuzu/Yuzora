// The interactive rebase pane: the commits from a base to HEAD as a todo list
// to reorder (drag, buttons or keys), give an action (pick, reword, edit,
// squash, fixup, drop) and send to git. Owned by lane L2; git runs in
// yuzora:git (Rust). The Git sidebar and log open it through shared
// "yuzora.git.rebase" { cwd, on, base, at }.
//
// The rows are plain elements in a scroll area (not a v_virtual_list): each
// has several controls and a drag, and a virtual list's item renderer may not
// register handlers.
import { View, div } from "gpui-kit";
import { v_flex, h_flex, TextareaState, Textarea, InputState, Input } from "gpui-base";
import { props, shared, setShared, panes, run, theme, settings } from "herdr";
import { t, tokens } from "yuzora:core";
import { open, status, changed, rebaseTodo, rebaseInteractive, rewordInMemory, sequencer } from "yuzora:git";

const KEY = "yuzora.git.rebase";
const ROW = 30;
const ACTIONS = ["pick", "reword", "edit", "squash", "fixup", "drop"];
const LETTER = { pick: "p", reword: "r", edit: "e", squash: "s", fixup: "f", drop: "d" };
const BY_LETTER = Object.fromEntries(ACTIONS.map((a) => [LETTER[a], a]));
// status().operation.kind is "none" (or empty) when nothing is in progress.
const kindOf = (st) => (st?.operation?.kind && st.operation.kind !== "none" ? st.operation.kind : "");

export default class GitRebase extends View {
  init(_props, cx) {
    this.handle = cx.focus_handle();
    this.focused = new Set(); this.nextEditor = 1;
    this.baseInput = InputState.new({ placeholder: t("base.placeholder") });
    this.baseInput.on("submit", (_e, cx) => this.setBase(this.baseInput.value(), cx));
    this.baseInput.on("focus", (_e, cx) => { this.focused.add(0); cx.notify(); });
    this.baseInput.on("blur", (_e, cx) => { this.focused.delete(0); cx.notify(); });
    this.ctx = 0; this.at = 0; this.spec = null;
    this.repo = null; this.st = null; this.error = null; this.note = null; this.result = null; this.busy = null;
    this.base = ""; this.todo = null; this.rows = []; this.loading = false; this.todoError = null; this.headOid = null; this.opKind = "";
    this.sel = 0; this.drag = null; this.dropAt = null; this.updateRefs = false; this.confirmAbort = false;
    const p = props(), spec = p?.cwd ? p : shared(KEY);
    if (spec?.cwd) this.begin(spec, cx);
  }

  async close() {
    this.ctx++;
    this.releaseEditors();
    this.baseInput.release();
  }

  // A new place: open its repo, list the commits and follow the repo until another place replaces it.
  // No notify here: render() calls this too, and the tasks below notify.
  begin(spec, cx) {
    this.spec = spec; this.at = spec.at ?? 0;
    const ctx = ++this.ctx;
    this.repo = null; this.st = null; this.error = null; this.note = null; this.result = null; this.todoError = null;
    const old = this.rows;
    this.todo = null; this.rows = []; this.headOid = null; this.opKind = "";
    this.base = (spec.base ?? "").trim();
    cx.spawn(async (cx) => {
      try {
        // Text state is made and freed in tasks and handlers, never while drawing.
        for (const r of old) this.dropEditor(r);
        this.baseInput.set_value(this.base);
        const repo = await open(spec.cwd, spec.on ?? null);
        if (this.ctx !== ctx) return;
        this.repo = repo;
        await this.reload(cx, ctx);
        if (this.base && !this.opKind && !this.todo) await this.fetchTodo(cx, ctx);
        cx.notify();
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
      const was = this.opKind, now = kindOf(st);
      this.st = st; this.opKind = now; this.error = null;
      if (was && !now) this.result = { tone: "ok", text: t("finished") };
      // The history moved (a rebase ended, a commit landed): the list is stale.
      if (this.base && !now && (was || (this.headOid !== null && st.branch.oid !== this.headOid))) await this.fetchTodo(cx, ctx);
    } catch (e) { if (this.ctx === ctx) this.error = String(e); }
    cx.notify();
  }

  // base..HEAD, oldest first. Every row starts as a pick.
  async fetchTodo(cx, ctx) {
    const repo = this.repo, base = this.base;
    this.loading = true; this.todoError = null; this.headOid = this.st?.branch.oid ?? null;
    cx.notify();
    try {
      const todo = await rebaseTodo(repo.id, base);
      if (this.ctx !== ctx || this.base !== base) return;
      this.releaseEditors();
      this.todo = todo;
      this.rows = todo.commits.map((c, i) => ({ ...c, at: i, action: "pick", ed: null, edKind: null, edSig: null }));
      this.sel = 0;
      if (!todo.refs.length) this.updateRefs = false;
    } catch (e) {
      if (this.ctx === ctx && this.base === base) { this.todo = null; this.rows = []; this.todoError = String(e); }
    }
    if (this.ctx === ctx) this.loading = false;
    cx.notify();
  }

  setBase(value, cx) {
    const base = value.trim(), ctx = this.ctx;
    if (!base || !this.repo || this.opKind) return;
    this.base = base; this.result = null; this.note = null;
    cx.spawn(async (cx) => { await this.fetchTodo(cx, ctx); });
    cx.notify();
  }

  // One git call at a time; its failure shows in the note.
  act(cx, name, work) {
    if (this.busy || !this.repo) return;
    const ctx = this.ctx;
    this.busy = name; this.note = null; this.confirmAbort = false;
    cx.spawn(async (cx) => {
      try {
        const out = await work(this.repo);
        if (out?.ok === false) this.note = { tone: "error", text: out.output || t("failed") };
        else if (out?.conflicts?.length) this.note = { tone: "warning", text: t("conflicts", { count: out.conflicts.length }) };
      } catch (e) { this.note = { tone: "error", text: String(e) }; }
      finally { this.busy = null; await this.reload(cx, ctx, true); }
    });
    cx.notify();
  }

  openMerge(path) {
    const repo = this.repo;
    if (!repo) return;
    const spec = { cwd: repo.root, on: repo.machine, path: path ?? null, at: Date.now() };
    setShared("yuzora.git.merge", spec);
    const pane = panes().find((p) => p.mod === "yuzora-git-merge");
    if (pane) run("pane.focus", { pane: pane.pane });
    else run("tab.new", { name: t("merge.tab"), mod: "yuzora-git-merge", props: spec });
  }

  // ---- the todo list ----

  // The squash and fixup rows after a pick, reword or edit join it (drops are skipped, as git does):
  // last member's index -> { head, members, squash }. Squash opens one message editor at the last member.
  chains() {
    const out = new Map();
    let head = null, members = [];
    const close = () => {
      if (head !== null && members.length) out.set(members[members.length - 1], { head, members, squash: members.some((i) => this.rows[i].action === "squash") });
    };
    this.rows.forEach((r, i) => {
      if (r.action === "drop") return;
      if (r.action === "squash" || r.action === "fixup") { if (head !== null) members.push(i); return; }
      close();
      head = i; members = [];
    });
    close();
    return out;
  }

  makeEditor(value) {
    const state = TextareaState.new({ value, rows: 3 }), ed = { state, id: this.nextEditor++, text: value };
    state.set_auto_grow(2, 8);
    state.on("change", (_e, cx) => { const was = Boolean(ed.text.trim()); ed.text = state.value(); if (was !== Boolean(ed.text.trim())) cx.notify(); });
    state.on("focus", (_e, cx) => { this.focused.add(ed.id); cx.notify(); });
    state.on("blur", (_e, cx) => { this.focused.delete(ed.id); cx.notify(); });
    return ed;
  }

  dropEditor(r) {
    if (!r.ed) return;
    this.focused.delete(r.ed.id);
    r.ed.state.release();
    r.ed = null; r.edKind = null; r.edSig = null;
  }

  releaseEditors() {
    for (const r of this.rows) this.dropEditor(r);
  }

  // Message editors follow the actions: a reword has its own, a squash chain one for the combined
  // message (git's default: the first message, then each squashed one; fixups add nothing).
  // Runs in handlers only: it makes text state.
  sync() {
    const chains = this.chains(), heads = new Set([...chains.values()].filter((c) => c.squash).map((c) => c.head));
    this.rows.forEach((r, i) => {
      const chain = chains.get(i);
      let kind = null, sig = null, value = null;
      if (chain?.squash) {
        const all = [chain.head, ...chain.members];
        kind = "chain"; sig = all.map((j) => this.rows[j].hash + this.rows[j].action).join();
        value = all.filter((j) => j === chain.head || this.rows[j].action === "squash").map((j) => this.rows[j].message).join("\n\n");
      } else if (r.action === "reword" && !heads.has(i)) { kind = "reword"; sig = r.hash; value = r.message; }
      if (!kind) { this.dropEditor(r); return; }
      if (r.ed && r.edKind === kind && r.edSig === sig) return;
      if (r.ed) { r.ed.state.set_value(value); r.ed.text = value; } else r.ed = this.makeEditor(value);
      r.edKind = kind; r.edSig = sig;
    });
  }

  setAction(i, action, cx) {
    if (!this.rows[i]) return;
    this.rows[i].action = action;
    this.sync();
    cx.notify();
  }

  // Moves row `from` to just before row `at` (as numbered before the move).
  moveTo(from, at, cx) {
    const [r] = this.rows.splice(from, 1), to = at > from ? at - 1 : at;
    this.rows.splice(to, 0, r);
    this.sel = to;
    this.sync();
    cx.notify();
  }

  step(i, d, cx) {
    if (i + d < 0 || i + d >= this.rows.length) return;
    this.moveTo(i, d > 0 ? i + 2 : i - 1, cx);
  }

  drop(cx) {
    const d = this.drag, at = this.dropAt;
    this.drag = null; this.dropAt = null;
    if (d && at !== null && at !== d.from && at !== d.from + 1) this.moveTo(d.from, at, cx);
    else cx.notify();
  }

  refsAt(hash) {
    return (this.todo?.refs ?? []).filter((x) => x.hash === hash);
  }

  problems() {
    const out = [], first = this.rows.find((r) => r.action !== "drop");
    if (first && (first.action === "squash" || first.action === "fixup")) out.push(t("err.first", { hash: first.short }));
    for (const r of this.rows) if (r.ed && !r.ed.text.trim()) out.push(t("err.empty", { hash: r.short }));
    return out;
  }

  entries() {
    const out = [];
    for (const r of this.rows) {
      const e = { action: r.action, hash: r.hash, subject: r.subject };
      if (r.ed && r.ed.text.trim()) e.message = r.ed.text;
      out.push(e);
      if (this.updateRefs) for (const x of this.refsAt(r.hash)) out.push({ action: "update-ref", ref: x.ref });
    }
    return out;
  }

  startRebase(cx) {
    if (this.busy || !this.repo || !this.rows.length || this.problems().length) return;
    const repo = this.repo, ctx = this.ctx, base = this.base, entries = this.entries();
    const updateRefs = this.updateRefs && Boolean(this.todo?.refs.length);
    this.busy = "start"; this.note = null; this.result = null;
    cx.spawn(async (cx) => {
      try {
        const out = await rebaseInteractive(repo.id, base, entries, { updateRefs });
        if (out.stopped) {
          // An edit stop, or a conflict: the operation bar below takes over.
          const n = out.conflicts?.length ?? 0;
          this.result = { tone: "warning", text: n ? t("stopped.conflicts", { count: n }) : t("stopped.edit") };
          if (n) this.openMerge(null);
        } else if (out.ok) this.result = { tone: "ok", text: t("done") };
        else this.note = { tone: "error", text: out.output || t("failed") };
      } catch (e) { this.note = { tone: "error", text: String(e) }; }
      finally { this.busy = null; await this.reload(cx, ctx, true); }
    });
    cx.notify();
  }

  // In-memory editing (rewordInMemory) rewrites the commit and every later one with commit-tree, so
  // only a straight, unreordered pick/reword list qualifies. It goes newest first: a reword changes
  // the hashes of that commit and the newer ones, never the older ones still waiting in the list.
  memOk() {
    const st = this.st;
    return Boolean(st && this.todo && this.rows.length && !this.todo.hasMerges && !this.opKind && !st.branch.detached
      && this.rows.every((r, i) => r.at === i && (r.action === "pick" || r.action === "reword")) && this.rows.some((r) => r.action === "reword"));
  }

  memJobs() {
    return this.rows.filter((r) => r.action === "reword" && r.ed && r.ed.text.trim() && r.ed.text.trim() !== r.message.trim()).reverse()
      .map((r) => ({ hash: r.hash, message: r.ed.text }));
  }

  memReword(cx) {
    const jobs = this.memJobs();
    if (this.busy || !this.repo || !this.memOk() || !jobs.length) return;
    const repo = this.repo, ctx = this.ctx;
    this.busy = "mem"; this.note = null; this.result = null;
    cx.spawn(async (cx) => {
      let done = 0;
      try {
        for (const j of jobs) {
          const out = await rewordInMemory(repo.id, j.hash, j.message);
          if (out?.ok === false) throw new Error(out.output || t("failed"));
          done++;
        }
        this.result = { tone: "ok", text: t("mem.done", { count: done }) };
      } catch (e) {
        this.note = { tone: "error", text: `${t("mem.partial", { done, total: jobs.length })} ${String(e).replace(/^Error: /, "")}` };
      } finally {
        // HEAD moved, so reload() reads the list again with the new hashes.
        this.busy = null; await this.reload(cx, ctx, true);
      }
    });
    cx.notify();
  }

  key(e, cx) {
    if (this.focused.size) {
      if (e.key === "escape") { this.handle.focus(); cx.stop_propagation(); }
      return;
    }
    if (this.drag && e.key === "escape") { this.drag = null; this.dropAt = null; cx.stop_propagation(); cx.notify(); return; }
    const n = this.rows.length, i = this.sel, m = e.modifiers;
    if (!n || this.opKind || i < 0) return;
    if (e.key === "up" || e.key === "down") {
      const d = e.key === "up" ? -1 : 1;
      if (m.alt) this.step(i, d, cx);
      else { this.sel = Math.max(0, Math.min(n - 1, i + d)); cx.notify(); }
    } else if (BY_LETTER[e.key] && !m.alt && !m.control && !m.platform) this.setAction(i, BY_LETTER[e.key], cx);
    else return;
    cx.stop_propagation();
  }

  // ---- drawing ----

  tone(action, th) {
    return { pick: th.muted, reword: th.accent, edit: th.blocked, squash: th.ansi5 ?? th.accent, fixup: th.ansi6 ?? th.muted, drop: th.ansi1 }[action];
  }

  button(id, label, enabled, th, action, primary = false, tip = null) {
    return h_flex().id(id).h(24).px(8).flex_none().items_center().justify_center().rounded(5).text_size(11.5)
      .when(primary, (el) => el.bg(enabled ? th.solid : th.raised).text_color(enabled ? th.solid_foreground : th.dim).font_semibold())
      .when(!primary, (el) => el.border_1().border_color(th.border).text_color(enabled ? th.foreground : th.dim))
      .when(Boolean(tip), (el) => el.tooltip(tip))
      .when(enabled, (el) => el.cursor_pointer().hover((s) => (primary ? s.opacity(0.88) : s.bg(th.hover))).on_click((_e, cx) => action(cx)))
      .child(label);
  }

  checkbox(id, on, label, th, toggle) {
    return h_flex().id(id).gap(5).items_center().cursor_pointer().text_size(11).text_color(on ? th.foreground : th.muted)
      .on_click((_e, cx) => toggle(cx))
      .child(div().w(12).h(12).flex().items_center().justify_center().rounded(3).border_1().border_color(on ? th.accent : th.input ?? th.border)
        .bg(on ? th.accent : th.background).text_size(9).text_color(th.background).child(on ? "✓" : ""))
      .child(label);
  }

  banner(th, lines) {
    const att = tokens().attention ?? {};
    return v_flex().mx(10).mt(6).px(8).py(5).gap(2).flex_none().rounded(6).text_size(11).bg(att.background ?? th.raised).border_1().border_color(att.border ?? th.border)
      .text_color(att.text ?? th.blocked).children([].concat(lines).map((l) => div().child(l)));
  }

  header(th) {
    return h_flex().h(40).flex_none().px(12).gap(8).items_center().border_b(1).border_color(th.divider ?? th.border)
      .child(div().font_semibold().flex_none().child(t("title")))
      .child(div().id("base").flex_1().min_w(0).h(26).px(8).flex().items_center().rounded(6).border_1().border_color(th.border).bg(th.background).text_size(11.5)
        .on_mouse_down("left", (_e, cx) => cx.stop_propagation()).child(Input.new(this.baseInput).w_full()))
      .child(this.button("load", t("load"), Boolean(this.repo) && !this.opKind && !this.busy, th, (cx) => this.setBase(this.baseInput.value(), cx)));
  }

  row(i, th, ins) {
    const r = this.rows[i], sel = this.sel === i, last = this.rows.length - 1;
    const chip = (a) => div().id(`a-${r.hash}-${a}`).w(18).h(18).flex().flex_none().items_center().justify_center().rounded(4).text_size(10.5).cursor_pointer()
      .tooltip(t(`action.${a}`))
      .when(r.action === a, (el) => el.bg(this.tone(a, th)).text_color(th.background).font_semibold())
      .when(r.action !== a, (el) => el.text_color(th.dim).hover((s) => s.bg(th.hover).text_color(th.foreground)))
      .on_mouse_down("left", (_e, cx) => cx.stop_propagation())
      .on_click((_e, cx) => { cx.stop_propagation(); this.setAction(i, a, cx); })
      .child(LETTER[a]);
    const arrow = (id, label, d, tip) => div().id(`${id}-${r.hash}`).w(18).h(18).flex().flex_none().items_center().justify_center().rounded(4).text_size(11)
      .text_color(th.muted).tooltip(tip).cursor_pointer().hover((s) => s.bg(th.hover).text_color(th.foreground))
      .on_mouse_down("left", (_e, cx) => cx.stop_propagation())
      .on_click((_e, cx) => { cx.stop_propagation(); this.step(i, d, cx); }).child(label);
    const joined = r.action === "squash" || r.action === "fixup", dropped = r.action === "drop";
    return h_flex().id(`r-${r.hash}`).h(ROW).px(6).gap(6).flex_none().items_center().rounded(5)
      .when(sel, (el) => el.bg(th.selected)).when(!sel, (el) => el.hover((s) => s.bg(th.hover)))
      .when(ins === i, (el) => el.border_t(2).border_color(th.accent))
      .when(ins === this.rows.length && i === last, (el) => el.border_b(2).border_color(th.accent))
      .when(ins !== null && this.drag?.from === i, (el) => el.opacity(0.5))
      .on_mouse_down("left", (_e, cx) => { this.drag = { from: i }; this.dropAt = null; this.sel = i; this.handle.focus(); cx.notify(); })
      .on_mouse_move((e, cx) => {
        if (!this.drag) return;
        const at = i + (e.local_position.y > ROW / 2 ? 1 : 0);
        if (at !== this.dropAt) { this.dropAt = at; cx.notify(); }
      })
      .child(div().w(12).flex_none().text_size(11).text_color(th.dim).cursor_grab().tooltip(t("drag")).child("⋮⋮"))
      .child(h_flex().gap(1).flex_none().children(ACTIONS.map(chip)))
      .child(div().w(58).flex_none().text_size(11).text_color(th.dim).font_family(settings().font_family ?? "JetBrains Mono").child(r.short))
      .when(joined, (el) => el.child(div().w(10).flex_none().text_color(th.dim).child("↳")))
      .child(div().flex_1().min_w(0).truncate().when(dropped, (el) => el.line_through().text_color(th.dim)).child(r.subject || t("noSubject")))
      .when(r.merge, (el) => el.child(div().flex_none().px(4).rounded(3).text_size(10).text_color(th.blocked).border_1().border_color(th.border).child("merge")))
      .child(div().w(90).flex_none().truncate().text_size(11).text_color(th.dim).child(r.author))
      .child(div().w(66).flex_none().text_size(11).text_color(th.dim).child(r.time ? new Date(r.time * 1000).toISOString().slice(0, 10) : ""))
      .child(arrow("up", "↑", -1, t("moveUp")))
      .child(arrow("down", "↓", 1, t("moveDown")));
  }

  // Under a row: its message editor, and the branches update-ref moves with it.
  extras(i, th) {
    const r = this.rows[i], out = [];
    if (r.ed) {
      out.push(v_flex().id(`m-${r.hash}`).ml(30).mr(8).mb(6).gap(2).flex_none()
        .child(div().text_size(10.5).text_color(th.dim).child(t(r.edKind === "chain" ? "msg.chain" : "msg.reword")))
        .child(div().rounded(5).border_1().border_color(th.border).bg(th.background).px(8).py(4).text_size(12).cursor_text()
          .font_family(settings().font_family ?? "JetBrains Mono").on_mouse_down("left", (_e, cx) => cx.stop_propagation()).child(Textarea.new(r.ed.state).w_full())));
    }
    // Shown always, so you see which branches the option would move; dim while it is off.
    for (const x of this.refsAt(r.hash)) {
      out.push(div().id(`u-${r.hash}-${x.ref}`).ml(30).h(20).flex_none().flex().items_center().text_size(10.5)
        .text_color(this.updateRefs ? th.muted : th.dim).when(!this.updateRefs, (el) => el.opacity(0.6))
        .child(`⤷ ${t("updateRef")} ${x.ref.replace(/^refs\/heads\//, "")}`));
    }
    return out;
  }

  editorView(th) {
    const rows = this.rows, problems = this.problems(), todo = this.todo, jobs = this.memJobs(), idle = !this.busy;
    const ins = this.drag && this.dropAt !== null && this.dropAt !== this.drag.from && this.dropAt !== this.drag.from + 1 ? this.dropAt : null;
    const counts = ACTIONS.filter((a) => a !== "pick").map((a) => [a, rows.filter((r) => r.action === a).length]).filter(([, n]) => n > 0);
    const summary = [t("summary", { count: rows.length }), ...counts.map(([a, n]) => `${n} ${t(`action.${a}`)}`)].join(" · ");
    const list = v_flex().id("todo").flex_1().min_h(0).overflow_y_scrollbar().px(6).py(4)
      .on_hover((h, cx) => { if (!h && this.drag && this.dropAt !== null) { this.dropAt = null; cx.notify(); } })
      .children(rows.flatMap((_r, i) => [this.row(i, th, ins), ...this.extras(i, th)]))
      .when(!rows.length, (el) => el.child(div().px(8).pt(16).text_size(11.5).text_color(th.dim).child(this.loading ? t("loading") : t("noCommits", { base: this.base }))));
    return v_flex().flex_1().min_h(0)
      .when(Boolean(todo?.hasMerges), (el) => el.child(this.banner(th, t("warn.merges"))))
      .when(problems.length > 0, (el) => el.child(this.banner(th, problems)))
      .child(h_flex().h(30).flex_none().px(12).gap(10).items_center().text_size(10.5).text_color(th.dim)
        .child(div().flex_1().min_w(0).truncate().child(`${t("order")} · ${t("keys")}`))
        .when(Boolean(todo?.refs.length), (el) => el.child(this.checkbox("update-refs", this.updateRefs, t("updateRefs"), th, (cx) => { this.updateRefs = !this.updateRefs; cx.notify(); }))))
      .child(list)
      .child(h_flex().h(40).flex_none().px(10).gap(8).items_center().border_t(1).border_color(th.divider ?? th.border)
        .child(div().flex_1().min_w(0).truncate().text_size(11).text_color(th.muted).child(summary))
        .when(this.memOk(), (el) => el.child(this.button("mem", this.busy === "mem" ? t("mem.running") : t("mem.button"), idle && jobs.length > 0, th, (cx) => this.memReword(cx), false, t("mem.tip"))))
        .child(this.button("start", this.busy === "start" ? t("start.running") : t("start"), idle && rows.length > 0 && problems.length === 0, th, (cx) => this.startRebase(cx), true)));
  }

  // A rebase is running (it stopped for an edit or a conflict, or someone started it elsewhere).
  operationView(th) {
    const st = this.st, kind = this.opKind, op = st.operation, left = st.conflicts.length, idle = !this.busy, att = tokens().attention ?? {};
    const seq = (action) => (cx) => this.act(cx, "sequencer", async (r) => {
      const out = await sequencer(r.id, action);
      if (out?.conflicts?.length) this.openMerge(null);
      return out;
    });
    const step = op.step ? ` ${op.step[0]}/${op.step[1]}` : "";
    const onto = op.onto ? (/^[0-9a-f]{20,}$/.test(op.onto) ? op.onto.slice(0, 8) : op.onto) : null;
    return v_flex().flex_1().min_h(0).p(12).gap(10)
      .child(div().text_size(12).text_color(att.text ?? th.blocked).child([`${t(`op.${kind}`)}${step}`, onto && t("op.onto", { onto }), op.head].filter(Boolean).join(" · ")))
      .child(div().text_size(11.5).text_color(th.muted).child(left ? t("op.conflicts", { count: left }) : t("op.ready")))
      .when(left > 0, (el) => el.child(v_flex().gap(2).text_size(11).text_color(th.dim).children(st.conflicts.slice(0, 8).map((c) => div().truncate().child(c.path)))))
      .when(kind !== "bisecting", (el) => el.child(h_flex().gap(6).flex_wrap()
        .when(left > 0, (e) => e.child(this.button("op-resolve", t("op.resolve"), true, th, () => this.openMerge(null), true)))
        .child(this.button("op-continue", t("op.continue"), left === 0 && idle, th, seq("continue"), left === 0, left ? t("op.resolveFirst") : null))
        .when(kind !== "merging", (e) => e.child(this.button("op-skip", t("op.skip"), idle, th, seq("skip"))))
        .child(this.button("op-abort", t("op.abort"), idle, th, (cx) => { this.confirmAbort = true; cx.notify(); }))))
      .when(this.confirmAbort, (el) => el.child(h_flex().gap(6).items_center().flex_wrap().text_size(11)
        .child(div().child(t("op.abort.ask")))
        .child(this.button("op-abort-yes", t("op.abort"), idle, th, seq("abort")))
        .child(this.button("op-abort-no", t("cancel"), true, th, (cx) => { this.confirmAbort = false; cx.notify(); }))));
  }

  statusBar(th) {
    const d = tokens().diff ?? {}, r = this.result, n = this.note;
    if (!r && !n && !this.busy) return null;
    const tone = (x) => (x.tone === "error" ? th.ansi1 : x.tone === "ok" ? d.added ?? th.ansi2 : th.blocked);
    return v_flex().flex_none().px(12).py(6).gap(2).text_size(11)
      .when(Boolean(r), (el) => el.child(div().text_color(tone(r)).child(r.text)))
      .when(Boolean(n), (el) => el.child(div().id("note").max_h(120).overflow_y_scrollbar().text_color(tone(n)).child(n.text)))
      .when(Boolean(this.busy), (el) => el.child(div().text_color(th.dim).child("…")));
  }

  render(cx) {
    // The sidebar or the log opened this pane again (maybe on another base or repo).
    const sh = shared(KEY);
    if (sh?.cwd && (sh.at ?? 0) > this.at) this.begin(sh, cx);
    const th = theme(), radius = (tokens().metrics ?? {}).pane_radius ?? 6;
    const frame = (...kids) => v_flex().id("rebase").size_full().min_h(0).rounded(radius).overflow_hidden().bg(th.background).border_1().border_color(th.border)
      .text_color(th.foreground).text_size(12.5).track_focus(this.handle)
      .on_key_down((e, cx) => this.key(e, cx)).on_mouse_up("left", (_e, cx) => this.drop(cx)).children(kids.filter(Boolean));
    const hint = (text) => div().p(16).text_size(11.5).text_color(th.dim).child(text);
    if (!this.spec) return frame(div().h(40).flex_none().px(12).flex().items_center().border_b(1).border_color(th.divider ?? th.border).font_semibold().child(t("title")), hint(t("empty")));
    if (!this.st) return frame(this.header(th), hint(this.error ?? t("loading")));
    const body = this.opKind ? this.operationView(th)
      : !this.base ? hint(t("noBase"))
      : this.todoError ? hint(this.todoError)
      : this.editorView(th);
    return frame(this.header(th), body, this.statusBar(th));
  }
}
