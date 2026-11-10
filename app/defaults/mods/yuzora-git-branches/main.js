// The branches popup (IntelliJ's): update, fetch, push, new branch; recent,
// local, remote branches and tags; a branch's actions. Opened by the Git
// sidebar as a popup with props {cwd, on}. ↑↓ move, Enter picks, Esc goes back.
import { View, div } from "gpui-kit";
import { v_flex, h_flex, InputState, Input, Scrollbar } from "gpui-base";
import { spaces, panes, props, closeOverlay, openOverlay, theme, run, setShared, notify } from "herdr";
import { t, tokens } from "yuzora:core";
import { open, branches, checkout, branchCreate, branchRename, branchDelete, branchTrack, merge, rebase, fetch, pull, stashPush, tagDelete, tagPush, cancel, settings as gitSettings } from "yuzora:git";

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
// ---- end of the shared pane helpers ----

const LIMIT = 30;

export default class GitBranches extends View {
  init(_props, cx) {
    this.data = null; this.error = null; this.busy = null; this.note = null;
    this.picked = null; this.ask = null; this.hi = 0; this.query = "";
    this.search = InputState.new({ placeholder: t("search") });
    this.search.on("change", (_e, cx) => { this.query = this.search.value(); this.hi = 0; cx.notify(); });
    this.search.on("submit", (_e, cx) => this.enter(cx));
    this.name = InputState.new({ placeholder: "" });
    this.name.on("submit", (_e, cx) => this.submitName(cx));
    this.handle = cx.focus_handle();
    this.search.focus();
    cx.timer.after(300, () => this.search.focus());
    this.load(cx);
  }

  close() { closeOverlay(props()?.overlay); }

  load(cx) {
    const p = props() ?? {};
    if (!p.cwd) { this.error = t("noRepo"); return; }
    cx.spawn(async (cx) => {
      try {
        this.repo = await open(p.cwd, p.on ?? null);
        this.data = await branches(this.repo.id);
      } catch (e) { this.error = String(e); }
      cx.notify();
    });
  }

  // A git call; on success the popup closes (most actions) or reloads.
  act(cx, name, work, keepOpen = false) {
    if (this.busy || !this.repo) return;
    this.busy = name; this.note = null;
    const token = `branches-${Date.now()}`;
    this.token = token;
    cx.spawn(async (cx) => {
      try {
        const out = await work(this.repo, token);
        if (out?.blocked) { this.ask = { kind: "blocked", target: out.target }; return; }
        if (out && out.ok === false) { this.note = out.output || t("failed"); return; }
        if (out?.notMerged) { this.ask = { kind: "notMerged", name: this.picked?.name }; return; }
        if (out?.conflicts?.length) {
          openRequested("yuzora.git.merge", "yuzora-git-merge", t("conflicts"), { cwd: this.repo.root, on: this.repo.machine, path: null });
        } else if (out?.output?.trim()) notify({ title: t(`done.${name}`), body: out.output.trim().split("\n").slice(-2).join("\n"), level: "success", timeout: 3000 });
        if (keepOpen) { this.data = await branches(this.repo.id); this.picked = null; this.ask = null; }
        else this.close();
      } catch (e) { this.note = String(e); }
      finally { this.busy = null; this.token = null; cx.notify(); }
    });
    cx.notify();
  }

  // Everything the list shows, filtered by the search.
  entries() {
    const d = this.data;
    if (!d) return [];
    const q = this.query.trim().toLowerCase();
    const hit = (s) => !q || s.toLowerCase().includes(q);
    const out = [];
    if (!q) {
      out.push({ kind: "action", id: "update", label: t("update") });
      out.push({ kind: "action", id: "pull", label: t("pull") });
      out.push({ kind: "action", id: "fetch", label: t("fetch") });
      out.push({ kind: "action", id: "push", label: t("push") });
      out.push({ kind: "action", id: "new", label: t("new") });
      out.push({ kind: "action", id: "revision", label: t("revision") });
      out.push({ kind: "action", id: "stash", label: t("stash") });
      out.push({ kind: "action", id: "repo", label: t("repo") });
    }
    const section = (title, list) => {
      const shown = list.filter((x) => hit(x.name));
      if (!shown.length) return;
      out.push({ kind: "title", label: title });
      for (const x of shown.slice(0, q ? 200 : LIMIT)) out.push(x);
      if (!q && shown.length > LIMIT) out.push({ kind: "more", label: t("more", { count: shown.length - LIMIT }) });
    };
    const local = new Map(d.local.map((b) => [b.name, b]));
    section(t("recent"), d.recent.filter((n) => local.has(n)).map((n) => ({ kind: "local", ...local.get(n) })));
    section(t("local"), d.local.map((b) => ({ kind: "local", ...b })));
    section(t("remote"), d.remote.map((b) => ({ kind: "remote", ...b })));
    section(t("tags"), d.tags.map((b) => ({ kind: "tag", ...b })));
    return out;
  }

  actions(b) {
    const cur = this.data?.current;
    const a = [];
    if (b.kind === "local") {
      if (!b.current) a.push(["checkout", t("b.checkout")]);
      a.push(["branchFrom", t("b.newFrom", { name: b.name })]);
      if (!b.current && cur) a.push(["compare", t("b.compare", { cur })], ["merge", t("b.merge", { name: b.name, cur })], ["rebase", t("b.rebase", { cur, name: b.name })]);
      a.push(["rename", t("b.rename")], ["track", b.upstream ? t("b.retrack", { upstream: b.upstream }) : t("b.track")]);
      if (b.current) a.push(["pushBranch", t("push")]);
      if (!b.current && !b.worktree) a.push(["delete", t("b.delete")]);
    } else if (b.kind === "remote") {
      a.push(["checkout", t("b.checkoutTracking")], ["branchFrom", t("b.newFrom", { name: b.name })]);
      if (cur) a.push(["compare", t("b.compare", { cur })], ["merge", t("b.merge", { name: b.name, cur })], ["rebase", t("b.rebase", { cur, name: b.name })], ["pullInto", t("b.pull", { name: b.name, cur })]);
      a.push(["deleteRemote", t("b.deleteRemote")]);
    } else if (b.kind === "tag") {
      a.push(["checkout", t("b.checkoutTag")], ["branchFrom", t("b.newFrom", { name: b.name })]);
      if (cur) a.push(["merge", t("b.merge", { name: b.name, cur })]);
      a.push(["tagPush", t("b.pushTag")], ["tagDelete", t("b.deleteTag")]);
    }
    return a.map(([id, label]) => ({ kind: "do", id, label }));
  }

  run(id, b, cx) {
    const remoteOf = () => (this.data.remote[0]?.remote ?? "origin");
    switch (id) {
      case "update": return this.act(cx, "update", (r, token) => pull(r.id, { token }));
      case "pull": this.ask = { kind: "pull", method: gitSettings().updateMethod ?? "merge", noFf: false, squash: false, noCommit: false }; return cx.notify();
      case "fetch": return this.act(cx, "fetch", (r, token) => fetch(r.id, { token }), true);
      case "push":
      case "pushBranch":
        this.close();
        return openOverlay({ mod: "yuzora-git-push", kind: "dialog", width: 520, height: 460, props: { cwd: this.repo.root, on: this.repo.machine } });
      case "new": return this.prompt(cx, "new", t("ask.new"), "");
      case "revision": return this.prompt(cx, "revision", t("ask.revision"), "");
      case "stash": return this.prompt(cx, "stash", t("ask.stash"), "");
      case "repo":
        this.close();
        return run("tab.new", { name: t("repo"), mod: "yuzora-git-repo", props: { cwd: this.repo.root, on: this.repo.machine } });
      case "checkout": return this.act(cx, "checkout", (r) => checkout(r.id, b.name, { smart: false }).then((o) => ({ ...o, target: b.name })));
      case "branchFrom": return this.prompt(cx, "branchFrom", t("ask.newFrom", { name: b.name }), b.kind === "remote" ? b.branch : "");
      case "compare":
        this.close();
        return run("tab.new", { name: `${b.name} ↔ ${this.data.current}`, mod: "yuzora-git-log", props: { cwd: this.repo.root, on: this.repo.machine, compare: { a: this.data.current, b: b.name } } });
      case "merge": return this.act(cx, "merge", (r) => merge(r.id, b.name, {}));
      case "rebase": return this.act(cx, "rebase", (r) => rebase(r.id, b.name, {}));
      case "pullInto": return this.act(cx, "update", (r, token) => pull(r.id, { remote: b.remote, branch: b.branch, token }));
      case "rename": return this.prompt(cx, "rename", t("ask.rename", { name: b.name }), b.name);
      case "track": return this.prompt(cx, "track", t("ask.track", { name: b.name }), b.upstream ?? `${remoteOf()}/${b.name}`);
      case "delete": this.ask = { kind: "delete", name: b.name }; return cx.notify();
      case "deleteRemote": this.ask = { kind: "deleteRemote", name: b.name }; return cx.notify();
      case "tagPush": return this.act(cx, "push", (r, token) => tagPush(r.id, remoteOf(), b.name, token), true);
      case "tagDelete": this.ask = { kind: "tagDelete", name: b.name }; return cx.notify();
    }
  }

  prompt(cx, kind, label, value) {
    this.ask = { kind: "name", what: kind, label, from: this.picked };
    this.name.set_value(value);
    this.name.focus();
    this.note = null;
    cx.notify();
  }

  submitName(cx) {
    const a = this.ask, v = this.name.value().trim();
    if (!a || a.kind !== "name" || (!v && a.what !== "track")) return;
    const b = a.from;
    switch (a.what) {
      case "new": return this.act(cx, "branch", (r) => branchCreate(r.id, v, null, true));
      case "branchFrom": return this.act(cx, "branch", (r) => branchCreate(r.id, v, b.name, true));
      case "revision": return this.act(cx, "checkout", (r) => checkout(r.id, v, { smart: false }).then((o) => ({ ...o, target: v })));
      case "stash": return this.act(cx, "stash", (r) => stashPush(r.id, { message: v, untracked: true }));
      case "rename": return this.act(cx, "branch", (r) => branchRename(r.id, b.name, v), true);
      case "track": return this.act(cx, "branch", (r) => branchTrack(r.id, b.name, v || null), true);
    }
  }

  pick(e, cx) {
    if (!e) return;
    if (e.kind === "action") return this.run(e.id, null, cx);
    if (e.kind === "do") return this.run(e.id, this.picked, cx);
    if (e.kind === "local" || e.kind === "remote" || e.kind === "tag") { this.picked = e; this.hi = 0; cx.notify(); }
  }

  shown() {
    return this.picked ? this.actions(this.picked) : this.entries();
  }

  enter(cx) {
    const list = this.shown().filter((e) => e.kind !== "title" && e.kind !== "more");
    this.pick(list[this.hi], cx);
  }

  key(e, cx) {
    const n = this.shown().filter((e) => e.kind !== "title" && e.kind !== "more").length;
    if (e.key === "escape") {
      if (this.ask) this.ask = null; else if (this.picked) this.picked = null; else this.close();
      this.search.focus();
      cx.stop_propagation(); cx.notify();
    } else if (e.key === "down" && n) { this.hi = (this.hi + 1) % n; cx.stop_propagation(); cx.notify(); }
    else if (e.key === "up" && n) { this.hi = (this.hi - 1 + n) % n; cx.stop_propagation(); cx.notify(); }
    else if (e.key === "enter" && !this.ask) { this.enter(cx); cx.stop_propagation(); }
  }

  button(id, label, th, primary, action) {
    return div().id(id).h(24).px(10).flex().items_center().rounded(5).text_size(11.5).cursor_pointer()
      .when(primary, (el) => el.bg(th.foreground).text_color(th.background).font_semibold())
      .when(!primary, (el) => el.border_1().border_color(th.border).hover((s) => s.bg(th.hover)))
      .on_click((_e, cx) => action(cx)).child(label);
  }

  askBox(th) {
    const a = this.ask;
    if (!a) return null;
    const box = v_flex().mx(8).my(6).p(8).gap(6).rounded(6).bg(th.raised).text_size(11.5);
    if (a.kind === "name") {
      return box.child(div().child(a.label))
        .child(div().id("name").h(26).px(8).flex().items_center().rounded(5).border_1().border_color(th.accent).bg(th.background)
          .on_mouse_down("left", (_e, cx) => cx.stop_propagation()).child(Input.new(this.name).w_full()))
        .child(h_flex().gap(6).child(this.button("ok", t("ok"), th, true, (cx) => this.submitName(cx))).child(this.button("no", t("cancel"), th, false, (cx) => { this.ask = null; cx.notify(); })));
    }
    if (a.kind === "pull") {
      // IntelliJ's pull dialog: the method and the merge options.
      const opt = (id, label, on, flip) => div().id(id).h(22).px(8).flex().items_center().rounded(4).cursor_pointer()
        .bg(on ? th.selected : th.background).text_color(on ? th.foreground : th.muted).on_click((_e, cx) => { flip(); cx.notify(); }).child(label);
      return box.child(div().child(t("ask.pull", { branch: this.data?.current ?? "" })))
        .child(h_flex().gap(4).flex_wrap().children(["merge", "rebase", "ff-only"].map((m) => opt(`pm-${m}`, t(`pull.${m}`), a.method === m, () => { a.method = m; }))))
        .when(a.method === "merge", (el) => el.child(h_flex().gap(4).flex_wrap()
          .child(opt("p-noff", "--no-ff", a.noFf, () => { a.noFf = !a.noFf; }))
          .child(opt("p-squash", "--squash", a.squash, () => { a.squash = !a.squash; }))
          .child(opt("p-nocommit", "--no-commit", a.noCommit, () => { a.noCommit = !a.noCommit; }))))
        .child(h_flex().gap(6)
          .child(this.button("pull-go", t("pull.go"), th, true, (cx) => {
            this.ask = null;
            const merging = a.method === "merge";
            this.act(cx, "update", (r, token) => pull(r.id, { method: a.method, noFf: merging && a.noFf, squash: merging && a.squash, noCommit: merging && a.noCommit, token }));
          }))
          .child(this.button("no", t("cancel"), th, false, (cx) => { this.ask = null; cx.notify(); })));
    }
    if (a.kind === "blocked") {
      return box.child(div().child(t("ask.blocked", { name: a.target })))
        .child(h_flex().gap(6)
          .child(this.button("smart", t("smart"), th, true, (cx) => { this.ask = null; this.act(cx, "checkout", (r) => checkout(r.id, a.target, { smart: true })); }))
          .child(this.button("force", t("force"), th, false, (cx) => { this.ask = null; this.act(cx, "checkout", (r) => checkout(r.id, a.target, { force: true })); }))
          .child(this.button("no", t("cancel"), th, false, (cx) => { this.ask = null; cx.notify(); })));
    }
    const text = { delete: t("ask.delete", { name: a.name }), notMerged: t("ask.notMerged", { name: a.name }), deleteRemote: t("ask.deleteRemote", { name: a.name }), tagDelete: t("ask.tagDelete", { name: a.name }) }[a.kind];
    const go = (cx) => {
      this.ask = null;
      if (a.kind === "delete") this.act(cx, "delete", (r) => branchDelete(r.id, a.name, {}), true);
      else if (a.kind === "notMerged") this.act(cx, "delete", (r) => branchDelete(r.id, a.name, { force: true }), true);
      else if (a.kind === "deleteRemote") this.act(cx, "delete", (r, token) => branchDelete(r.id, a.name, { remote: true, token }), true);
      else if (a.kind === "tagDelete") this.act(cx, "delete", (r) => tagDelete(r.id, a.name, null), true);
    };
    return box.child(div().child(text))
      .child(h_flex().gap(6).child(this.button("yes", t("delete"), th, true, go)).child(this.button("no", t("cancel"), th, false, (cx) => { this.ask = null; cx.notify(); })));
  }

  row(e, i, sel, th) {
    if (e.kind === "title") return div().id(`t${i}`).px(10).pt(8).pb(3).text_size(10.5).font_semibold().text_color(th.dim).child(e.label);
    if (e.kind === "more") return div().id(`m${i}`).px(14).py(3).text_size(11).text_color(th.dim).child(e.label);
    const isRef = e.kind === "local" || e.kind === "remote" || e.kind === "tag";
    return h_flex().id(`e${i}`).h(26).mx(4).px(8).gap(6).items_center().rounded(5).cursor_pointer().text_size(12)
      .when(sel, (el) => el.bg(th.selected)).hover((s) => s.bg(th.hover))
      .on_click((_e, cx) => this.pick(e, cx))
      .when(isRef, (el) => el.child(div().w(12).text_color(e.current ? th.accent : th.dim).child(e.kind === "tag" ? "◆" : e.current ? "●" : "⎇")))
      .child(div().flex_1().min_w(0).truncate().child(isRef ? e.name : e.label))
      .when(e.kind === "local" && (e.ahead || e.behind), (el) => el.child(div().text_size(10.5).text_color(th.muted).child(`↑${e.ahead} ↓${e.behind}`)))
      .when(e.kind === "local" && e.gone, (el) => el.child(div().text_size(10.5).text_color(tokens().file_status?.deleted ?? th.ansi1).child(t("gone"))))
      .when(Boolean(e.worktree), (el) => el.child(div().text_size(10.5).text_color(th.dim).child(t("worktree"))))
      .when(isRef, (el) => el.child(div().text_size(10).text_color(th.dim).child("›")));
  }

  render() {
    const th = theme();
    const list = this.shown();
    let k = -1;
    const rows = list.map((e, i) => {
      const selectable = e.kind !== "title" && e.kind !== "more";
      if (selectable) k++;
      return this.row(e, i, selectable && k === this.hi, th);
    });
    const ask = this.askBox(th);
    return v_flex().id("branches").track_focus(this.handle).on_key_down((e, cx) => this.key(e, cx))
      .size_full().bg(th.popover ?? th.raised).text_color(th.foreground).text_size(12)
      .child(h_flex().h(34).px(10).gap(6).flex_none().items_center().border_b(1).border_color(th.border)
        .when(Boolean(this.picked), (el) => el.child(div().id("back").text_color(th.muted).cursor_pointer().hover((s) => s.text_color(th.foreground))
          .on_click((_e, cx) => { this.picked = null; this.ask = null; this.hi = 0; cx.notify(); }).child("‹")))
        .child(this.picked ? div().flex_1().min_w(0).truncate().font_semibold().child(this.picked.name)
          : div().id("search").flex_1().on_mouse_down("left", (_e, cx) => cx.stop_propagation()).child(Input.new(this.search).w_full())))
      .when(Boolean(this.busy), (el) => el.child(h_flex().px(10).py(4).gap(8).text_size(11).text_color(th.muted)
        .child(div().flex_1().child(t(`busy.${this.busy}`)))
        .when(Boolean(this.token), (el) => el.child(div().id("stop").cursor_pointer().text_color(th.ansi1).on_click(() => cancel(this.token)).child(t("cancel"))))))
      .when(Boolean(this.note), (el) => el.child(div().px(10).py(4).text_size(11).text_color(th.ansi1).child(this.note)))
      .when(Boolean(ask), (el) => el.child(ask))
      .child(div().flex_1().min_h(0).relative()
        .child(v_flex().id("branch-list").size_full().overflow_y_scrollbar().py(4).children(rows))
        .when(!this.data && !this.error, (el) => el.child(div().p(12).text_color(th.dim).child(t("loading"))))
        .when(Boolean(this.error), (el) => el.child(div().p(12).text_color(th.ansi1).child(this.error))));
  }
}
