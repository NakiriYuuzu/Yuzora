// Repository tools: stashes, tags, remotes, worktrees, submodules, patches and
// the identity and signing config. Props {cwd, on, tab?}; without cwd it
// follows the focused terminal's repository. Owned by lane L2.
// Lists that can be long are virtual lists, whose rows take no handlers (the
// list's on_item_click does the picking); the short ones are plain columns.
import { View, div } from "gpui-kit";
import { v_flex, h_flex, InputState, Input, TextareaState, Textarea, v_virtual_list, Scrollbar } from "gpui-base";
import { spaces, panes, focused, theme, props, run, openOverlay, setShared, settings } from "herdr";
import { t, tokens } from "yuzora:core";
import {
  open, changed, cancel, branches, stashes, stashPush, stashAction, commitDetails, tagCreate, tagDelete, tagPush, remotes, remoteEdit,
  fetch as gitFetch, worktrees, worktreeAction, submodules, submoduleUpdate, patchCreate, patchSave, patchApply, config, setConfig,
  settings as gitSettings,
} from "yuzora:git";

const TABS = ["stashes", "tags", "remotes", "worktrees", "submodules", "patches", "identity"];
const LOAD = {
  stashes: (r) => stashes(r.id),
  tags: async (r) => ({ tags: (await branches(r.id)).tags ?? [], remotes: await remotes(r.id) }),
  remotes: (r) => remotes(r.id),
  worktrees: (r) => worktrees(r.id),
  submodules: (r) => submodules(r.id),
  identity: (r) => config(r.id),
};
const ROW = 28;
// gpg.format and core.autocrlf: "" is unset.
const FORMATS = ["", "openpgp", "x509", "ssh"], CRLF = ["", "true", "input", "false"];

// Where the pane looks without props: the focused pane's folder, else the space's.
function location() {
  const space = spaces().find((s) => s.active), f = focused();
  const pane = f?.pane ? panes().find((p) => p.pane === f.pane) : null;
  const on = f?.pane === null ? (f.terminal?.includes("/") ? f.terminal.split("/")[0] : null) : space?.machine ?? null;
  return { cwd: pane?.cwd || f?.cwd || space?.folder || null, on };
}

const pad = (n) => String(n).padStart(2, "0");
const stamp = (s) => { const d = new Date(s * 1000); return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`; };

export default class GitRepo extends View {
  init(_props, cx) {
    const p = props() ?? {};
    this.pinned = Boolean(p.cwd);
    this.tab = TABS.includes(p.tab) ? p.tab : "stashes";
    this.repo = null; this.error = null; this.note = null; this.ask = null; this.busy = null; this.token = null; this.cancelled = false;
    this.data = {}; this.details = {}; this.sel = { stash: null, tag: null }; this.loop = 0;
    this.keepIndex = false; this.untracked = false; this.restoreIndex = false; this.branching = false;
    this.remote = null; this.edit = null;
    this.patch = { threeWay: false, cached: false, reverse: false }; this.patchStaged = true;
    this.global = false; this.draft = { gpgSign: false, gpgFormat: "", autocrlf: "" }; this.idDirty = false;
    // Buttons depend on what is typed, so every field redraws on change.
    const input = (placeholder, value = "", edited = null) => {
      const st = InputState.new({ placeholder, value });
      st.on("change", (_e, cx) => { edited?.(); cx.notify(); });
      return st;
    };
    const dirty = () => { this.idDirty = true; };
    this.stashMsg = input(t("stash.message")); this.stashBranch = input(t("stash.branchName"));
    this.tagName = input(t("tag.name")); this.tagRev = input(t("tag.rev")); this.tagMsg = input(t("tag.message")); this.tagFilter = input(t("tag.filter"));
    this.remName = input(t("remote.name")); this.remUrl = input(t("remote.url")); this.editVal = input("");
    this.patchText = TextareaState.new({ placeholder: t("patch.placeholder"), rows: 8 }); this.patchText.set_auto_grow(8, 16);
    this.patchText.on("change", (_e, cx) => cx.notify());
    this.patchFile = input(t("patch.file"), "changes.patch");
    this.idName = input("Name", "", dirty); this.idEmail = input("name@example.com", "", dirty); this.idKey = input(t("identity.keyPlaceholder"), "", dirty);
    this.attach(cx);
    if (!this.pinned) cx.timer.every(1000, (cx) => { if (JSON.stringify(this.where()) !== this.ctx) this.attach(cx); });
  }

  where() {
    const p = props() ?? {};
    return p.cwd ? { cwd: p.cwd, on: p.on ?? null } : location();
  }

  // A new place: open its repo, and follow it while it stays the shown one.
  attach(cx) {
    const where = this.where();
    this.ctx = JSON.stringify(where);
    const ctx = this.ctx;
    if (!where.cwd) { this.repo = null; this.error = t("noFolder"); cx.notify(); return; }
    cx.spawn(async (cx) => {
      try {
        const repo = await open(where.cwd, where.on);
        if (this.ctx !== ctx) return;
        this.error = null;
        // The same repo (a pane's cwd moving inside it) keeps what is on screen.
        if (this.repo?.id !== repo.id) {
          this.repo = repo; this.data = {}; this.details = {}; this.sel = { stash: null, tag: null }; this.note = null; this.ask = null; this.edit = null;
          this.watch(cx, repo, ++this.loop);
        }
        await this.pull();
        cx.notify();
      } catch (e) {
        if (this.ctx !== ctx) return;
        this.repo = null; this.loop++;
        this.error = String(e).includes("not a git repository") ? t("notRepo") : String(e);
        cx.notify();
      }
    });
    cx.notify();
  }

  // Local repos: the watcher bumps the generation. Remote ones have no
  // watcher, so each ~25 s wait without news reads again.
  watch(cx, repo, loop) {
    cx.spawn(async (cx) => {
      let gen = 0;
      try {
        while (this.loop === loop) {
          const next = await changed(repo.id, gen);
          if (this.loop !== loop) break;
          if (gen && (next > gen || repo.machine) && !this.busy) { await this.pull(); cx.notify(); }
          gen = next;
        }
      } catch {}
    });
  }

  // Reads one tab's data. Its caller redraws: a click handler's cx ends with the
  // call, so the waiting happens in a task (reload) and tasks call this directly.
  async pull(tab = this.tab) {
    const repo = this.repo, load = LOAD[tab];
    if (!repo || !load) return;
    try {
      const v = await load(repo);
      if (this.repo?.id !== repo.id) return;
      this.data[tab] = v;
      if (tab === "stashes" && !v.some((s) => s.hash === this.sel.stash)) this.sel.stash = null;
      if (tab === "tags") {
        if (!v.tags.some((x) => x.name === this.sel.tag)) this.sel.tag = null;
        if (!v.remotes.some((x) => x.name === this.remote)) this.remote = (v.remotes.find((x) => x.name === "origin") ?? v.remotes[0])?.name ?? null;
      }
      if (tab === "identity" && !this.idDirty) this.fillIdentity(v);
    } catch (e) { this.note = { tone: "error", text: String(e) }; }
  }

  reload(cx, tab = this.tab) {
    cx.spawn(async (cx) => { await this.pull(tab); cx.notify(); });
  }

  go(tab, cx) {
    if (this.tab === tab) return;
    this.tab = tab; this.note = null; this.ask = null; this.edit = null; this.branching = false;
    this.reload(cx, tab);
    cx.notify();
  }

  // One git call at a time from here; its answer shows under the tabs.
  // opts.cancellable: the Cancel button stops it through the token it is given.
  act(cx, name, work, opts = {}) {
    if (this.busy || !this.repo) return;
    const token = opts.cancellable ? `yz-repo-${Date.now()}` : null;
    this.busy = name; this.token = token; this.cancelled = false; this.note = null;
    cx.spawn(async (cx) => {
      try {
        const out = await work(this.repo, token, cx);
        if (this.cancelled) this.note = { tone: "warning", text: t("cancelled") };
        else if (out && out.ok === false) this.note = { tone: "error", text: out.output || t("failed") };
        else if (out?.conflicts?.length) this.note = { tone: "warning", text: t("conflicts", { count: out.conflicts.length }) };
        else if (out?.output || opts.ok) this.note = { tone: "ok", text: out?.output || opts.ok };
      } catch (e) { this.note = this.cancelled ? { tone: "warning", text: t("cancelled") } : { tone: "error", text: String(e) }; }
      finally { this.busy = null; this.token = null; await this.pull(); cx.notify(); }
    });
    cx.notify();
  }

  cancelWork(cx) {
    this.cancelled = true;
    if (this.token) cancel(this.token);
    cx.notify();
  }

  confirm(cx, text, label, action, check = null) {
    this.ask = { text, label, run: action, check };
    cx.notify();
  }

  openDiff(path, orig, rev) {
    const repo = this.repo;
    const spec = { cwd: repo.root, on: repo.machine, path, orig: orig ?? null, mode: "commit", rev, at: Date.now() };
    showDiff(spec);
  }

  // ---- widgets ----

  checkbox(id, on, label, th, toggle) {
    return h_flex().id(id).gap(5).items_center().cursor_pointer().text_size(11.5).text_color(on ? th.foreground : th.muted)
      .on_click((_e, cx) => toggle(cx))
      .child(div().w(12).h(12).flex().items_center().justify_center().rounded(3).border_1().border_color(on ? th.accent : th.input ?? th.border)
        .bg(on ? th.accent : th.background).text_size(9).text_color(th.background).child(on ? "✓" : ""))
      .child(label);
  }

  // kind: "primary" (ink on paper) or "danger".
  button(id, label, th, enabled, action, kind = "") {
    return div().id(id).h(24).px(10).flex().flex_none().items_center().rounded(5).text_size(11.5)
      .when(kind === "primary" && enabled, (el) => el.bg(th.solid ?? th.foreground).text_color(th.solid_foreground ?? th.background).font_semibold())
      .when(kind !== "primary" || !enabled, (el) => el.border_1().border_color(th.border).text_color(!enabled ? th.dim : kind === "danger" ? th.ansi1 : th.foreground))
      .when(enabled, (el) => el.cursor_pointer().hover((s) => s.opacity(0.85)).on_click((_e, cx) => action(cx)))
      .child(label);
  }

  pill(id, label, on, th, action) {
    return div().id(id).h(20).px(8).flex().items_center().rounded(4).text_size(11.5).cursor_pointer()
      .bg(on ? th.selected : th.raised).text_color(on ? th.foreground : th.muted).hover((s) => s.text_color(th.foreground))
      .on_click((_e, cx) => action(cx)).child(label);
  }

  field(id, state, th, width) {
    return div().id(id).h(26).px(8).flex().items_center().rounded(6).border_1().border_color(th.border).bg(th.background).text_size(12)
      .when(Boolean(width), (el) => el.w(width).flex_none()).when(!width, (el) => el.flex_1().min_w(0))
      .on_mouse_down("left", (_e, cx) => cx.stop_propagation()).child(Input.new(state).w_full());
  }

  label(text, th) {
    return div().text_size(11).text_color(th.dim).child(text);
  }

  // A list of rows that may be long. `pick` gets the key of the row clicked.
  vlist(id, items, key, row, pick, th, empty) {
    if (!items.length) return div().flex_1().min_h(0).p(16).text_color(th.dim).child(empty);
    return div().flex_1().min_h(0).relative().px(6)
      .child(v_virtual_list(id, items.length, ROW, (i) => key(items[i]), (range) => items.slice(range.start, range.end).map(row)).on_item_click(pick).size_full())
      .child(Scrollbar.vertical(id).viewport_from_layout().absolute().inset_0());
  }

  fileColor(kind, th) {
    const fs = tokens().file_status ?? {};
    if (kind === "A") return fs.added ?? th.ansi2;
    if (kind === "D") return fs.deleted ?? th.ansi1;
    return fs.modified ?? th.blocked;
  }

  // ---- stashes ----

  pickStash(hash, cx) {
    this.sel.stash = hash; this.branching = false;
    if (!this.details[hash]) {
      const repo = this.repo;
      cx.spawn(async (cx) => {
        try { this.details[hash] = await commitDetails(repo.id, hash); } catch (e) { this.details[hash] = { files: [], error: String(e) }; }
        cx.notify();
      });
    }
    cx.notify();
  }

  stashesTab(th, mono) {
    const list = this.data.stashes ?? [], sel = list.find((s) => s.hash === this.sel.stash), idle = !this.busy;
    const form = v_flex().flex_none().p(10).gap(8).border_b(1).border_color(th.divider ?? th.border)
      .child(this.field("stash-message", this.stashMsg, th))
      .child(h_flex().gap(10).flex_wrap().items_center()
        .child(this.checkbox("stash-keep", this.keepIndex, t("stash.keepIndex"), th, (cx) => { this.keepIndex = !this.keepIndex; cx.notify(); }))
        .child(this.checkbox("stash-untracked", this.untracked, t("stash.untracked"), th, (cx) => { this.untracked = !this.untracked; cx.notify(); }))
        .child(div().flex_1())
        .child(this.button("stash-clear", t("stash.clear"), th, idle && list.length > 0, (cx) => this.confirm(cx, t("stash.clearAsk", { count: list.length }), t("stash.clear"),
          (cx) => this.act(cx, "stash", (r) => stashAction(r.id, "clear", "stash@{0}")), null), "danger"))
        .child(this.button("stash-push", t("stash.push"), th, idle, (cx) => this.act(cx, "stash", async (r) => {
          const out = await stashPush(r.id, { message: this.stashMsg.value(), keepIndex: this.keepIndex, untracked: this.untracked });
          if (out.ok) this.stashMsg.set_value("");
          return out;
        }), "primary")));

    const list_ = this.vlist("stashes", list, (s) => s.hash,
      (s) => h_flex().h(ROW).px(8).gap(8).items_center().rounded(4).cursor_pointer().when(this.sel.stash === s.hash, (el) => el.bg(th.selected)).hover((st) => st.bg(th.hover))
        .child(div().w(78).flex_none().font_family(mono).text_size(11).text_color(th.dim).child(s.ref))
        .child(div().flex_1().min_w(0).truncate().child(s.message))
        .child(div().flex_none().text_size(11).text_color(th.dim).child(stamp(s.time))),
      (key, cx) => this.pickStash(key, cx), th, t("stash.empty"));

    let detail = null;
    if (sel) {
      const det = this.details[sel.hash], files = det?.files ?? [];
      detail = v_flex().flex_none().p(10).gap(8).border_t(1).border_color(th.border).bg(th.chrome)
        .child(h_flex().gap(8).items_center()
          .child(div().flex_none().font_family(mono).text_size(11).text_color(th.dim).child(sel.ref))
          .child(div().flex_1().min_w(0).truncate().font_semibold().child(sel.message)))
        .child(h_flex().gap(8).flex_wrap().items_center()
          .child(this.button("stash-apply", t("stash.apply"), th, idle, (cx) => this.act(cx, "stash", (r) => stashAction(r.id, "apply", sel.ref, { index: this.restoreIndex }))))
          .child(this.button("stash-pop", t("stash.pop"), th, idle, (cx) => this.act(cx, "stash", (r) => stashAction(r.id, "pop", sel.ref, { index: this.restoreIndex }))))
          .child(this.checkbox("stash-index", this.restoreIndex, t("stash.restoreIndex"), th, (cx) => { this.restoreIndex = !this.restoreIndex; cx.notify(); }))
          .child(div().flex_1())
          .child(this.button("stash-branch", t("stash.branch"), th, idle, (cx) => { this.branching = !this.branching; cx.notify(); }))
          .child(this.button("stash-drop", t("stash.drop"), th, idle, (cx) => this.confirm(cx, t("stash.dropAsk", { ref: sel.ref }), t("stash.drop"),
            (cx) => this.act(cx, "stash", (r) => stashAction(r.id, "drop", sel.ref)), null), "danger")))
        .when(this.branching, (el) => el.child(h_flex().gap(6)
          .child(this.field("stash-branch-name", this.stashBranch, th))
          .child(this.button("stash-branch-go", t("stash.branchCreate"), th, idle && Boolean(this.stashBranch.value().trim()), (cx) => {
            this.branching = false;
            this.act(cx, "stash", (r) => stashAction(r.id, "branch", sel.ref, { branch: this.stashBranch.value().trim() }));
          }, "primary"))))
        .child(v_flex().id("stash-files").max_h(150).overflow_y_scrollbar()
          .when(!det, (el) => el.child(div().text_size(11.5).text_color(th.dim).child(t("loading"))))
          .when(Boolean(det?.error), (el) => el.child(div().text_size(11.5).text_color(th.ansi1).child(det.error)))
          .children(files.map((f, i) => h_flex().id(`sf-${i}`).h(22).px(6).gap(8).items_center().rounded(4).cursor_pointer().hover((s) => s.bg(th.hover))
            .on_click(() => this.openDiff(f.path, f.orig, sel.hash))
            .child(div().w(12).flex_none().text_size(11).font_semibold().text_color(this.fileColor(f.kind, th)).child(f.kind))
            .child(div().flex_1().min_w(0).truncate().text_size(11.5).child(f.orig ? `${f.orig} → ${f.path}` : f.path)))));
    }
    return v_flex().flex_1().min_h(0).child(form).child(list_).when(Boolean(detail), (el) => el.child(detail));
  }

  // ---- tags ----

  tagsTab(th, mono) {
    const d = this.data.tags ?? { tags: [], remotes: [] }, idle = !this.busy, remote = this.remote;
    const q = this.tagFilter.value().trim().toLowerCase();
    const tags = q ? d.tags.filter((x) => x.name.toLowerCase().includes(q)) : d.tags;
    const sel = d.tags.find((x) => x.name === this.sel.tag);
    const form = v_flex().flex_none().p(10).gap(8).border_b(1).border_color(th.divider ?? th.border)
      .child(h_flex().gap(6).child(this.field("tag-name", this.tagName, th, 200)).child(this.field("tag-rev", this.tagRev, th, 170)).child(this.field("tag-message", this.tagMsg, th)))
      .child(h_flex().gap(8).items_center().flex_wrap()
        .child(this.label(t("tag.hint"), th))
        .child(div().flex_1())
        .child(this.button("tag-create", t("tag.create"), th, idle && Boolean(this.tagName.value().trim()), (cx) => this.act(cx, "tag", async (r) => {
          const out = await tagCreate(r.id, this.tagName.value().trim(), this.tagRev.value().trim() || null, this.tagMsg.value().trim() || null);
          if (out.ok) { this.tagName.set_value(""); this.tagRev.set_value(""); this.tagMsg.set_value(""); }
          return out;
        }), "primary")));
    const bar = h_flex().flex_none().px(10).py(6).gap(8).items_center().flex_wrap().border_b(1).border_color(th.divider ?? th.border)
      .child(this.label(t("tag.remote"), th))
      .children(d.remotes.map((r) => this.pill(`tag-remote-${r.name}`, r.name, r.name === remote, th, (cx) => { this.remote = r.name; cx.notify(); })))
      .when(d.remotes.length === 0, (el) => el.child(this.label(t("tag.noRemotes"), th)))
      .child(div().flex_1())
      .child(this.button("tag-push-all", t("tag.pushAll"), th, idle && Boolean(remote), (cx) => this.act(cx, "tag", (r, token) => tagPush(r.id, remote, null, token), { cancellable: true, ok: t("tag.pushed") })))
      .child(this.field("tag-filter", this.tagFilter, th, 160));

    const list = this.vlist("tags", tags, (x) => x.name,
      (x) => h_flex().h(ROW).px(8).gap(8).items_center().rounded(4).cursor_pointer().when(this.sel.tag === x.name, (el) => el.bg(th.selected)).hover((s) => s.bg(th.hover))
        .child(div().w(200).flex_none().truncate().child(x.name))
        .child(div().w(66).flex_none().font_family(mono).text_size(11).text_color(th.dim).child(x.hash.slice(0, 8)))
        .child(div().flex_1().min_w(0).truncate().text_size(11.5).text_color(th.muted).child(x.subject ?? ""))
        .child(div().flex_none().text_size(11).text_color(th.dim).child(x.time ? stamp(x.time) : "")),
      (key, cx) => { this.sel.tag = key; cx.notify(); }, th, q ? t("tag.noMatch") : t("tag.empty"));

    const detail = sel && h_flex().flex_none().p(10).gap(8).items_center().flex_wrap().border_t(1).border_color(th.border).bg(th.chrome)
      .child(div().flex_1().min_w(0).truncate().font_semibold().child(sel.name))
      .child(this.button("tag-push", remote ? t("tag.push", { remote }) : t("tag.pushOne"), th, idle && Boolean(remote),
        (cx) => this.act(cx, "tag", (r, token) => tagPush(r.id, remote, sel.name, token), { cancellable: true, ok: t("tag.pushed") })))
      .child(this.button("tag-delete", t("tag.delete"), th, idle, (cx) => this.confirm(cx, t("tag.deleteAsk", { name: sel.name }), t("tag.delete"),
        (cx, alsoRemote) => this.act(cx, "tag", async (r, token) => {
          // The remote first: if it refuses, the local tag is still there to try again.
          if (alsoRemote) {
            const out = await tagDelete(r.id, sel.name, remote, token);
            if (out.ok === false) return out;
          }
          return tagDelete(r.id, sel.name);
        }, { cancellable: true }), remote ? { label: t("tag.alsoRemote", { remote }), on: false } : null), "danger"));
    return v_flex().flex_1().min_h(0).child(form).child(bar).child(list).when(Boolean(detail), (el) => el.child(detail));
  }

  // ---- remotes ----

  startEdit(r, mode, cx) {
    this.edit = { name: r.name, mode };
    this.editVal.set_value(mode === "url" ? r.url : r.name);
    this.editVal.focus();
    cx.notify();
  }

  saveEdit(cx) {
    const e = this.edit, value = this.editVal.value().trim();
    if (!e || !value) return;
    this.edit = null;
    this.act(cx, "remote", (r) => remoteEdit(r.id, e.mode === "url" ? "set-url" : "rename", e.name, value));
  }

  fetch(cx, name) {
    this.act(cx, `fetch:${name ?? ""}`, (r, token) => gitFetch(r.id, name ? { remote: name, token } : { token }), { cancellable: true, ok: t("remote.fetched") });
  }

  remotesTab(th, mono) {
    const list = this.data.remotes ?? [], idle = !this.busy;
    const rows = list.map((r) => {
      const editing = this.edit?.name === r.name, fetching = this.busy === `fetch:${r.name}`;
      if (editing) {
        return h_flex().id(`rm-${r.name}`).px(10).py(8).gap(8).items_center().border_b(1).border_color(th.divider ?? th.border)
          .child(div().w(90).flex_none().truncate().font_semibold().child(r.name))
          .child(this.field(`rm-edit-${r.name}`, this.editVal, th))
          .child(this.button(`rm-save-${r.name}`, t("save"), th, idle && Boolean(this.editVal.value().trim()), (cx) => this.saveEdit(cx), "primary"))
          .child(this.button(`rm-cancel-${r.name}`, t("cancel"), th, true, (cx) => { this.edit = null; cx.notify(); }));
      }
      return h_flex().id(`rm-${r.name}`).px(10).py(8).gap(8).items_center().border_b(1).border_color(th.divider ?? th.border)
        .child(v_flex().flex_1().min_w(0).gap(2)
          .child(div().font_semibold().child(r.name))
          .child(div().truncate().font_family(mono).text_size(11).text_color(th.muted).child(r.url))
          .when(Boolean(r.pushUrl) && r.pushUrl !== r.url, (el) => el.child(div().truncate().font_family(mono).text_size(11).text_color(th.dim).child(`${t("remote.push")}: ${r.pushUrl}`))))
        .child(fetching
          ? this.button(`rm-fetch-${r.name}`, t("cancel"), th, !this.cancelled, (cx) => this.cancelWork(cx))
          : this.button(`rm-fetch-${r.name}`, t("remote.fetch"), th, idle, (cx) => this.fetch(cx, r.name)))
        .child(this.button(`rm-url-${r.name}`, t("remote.editUrl"), th, idle, (cx) => this.startEdit(r, "url", cx)))
        .child(this.button(`rm-rename-${r.name}`, t("remote.rename"), th, idle, (cx) => this.startEdit(r, "rename", cx)))
        .child(this.button(`rm-remove-${r.name}`, t("remote.remove"), th, idle, (cx) => this.confirm(cx, t("remote.removeAsk", { name: r.name }), t("remote.remove"),
          (cx) => this.act(cx, "remote", (rp) => remoteEdit(rp.id, "remove", r.name)), null), "danger"));
    });
    const canAdd = idle && Boolean(this.remName.value().trim()) && Boolean(this.remUrl.value().trim());
    return v_flex().id("remotes-body").flex_1().min_h(0).overflow_y_scrollbar()
      .child(h_flex().px(10).py(8).gap(8).items_center().border_b(1).border_color(th.divider ?? th.border)
        .child(div().flex_1().text_color(th.muted).child(t("remote.count", { count: list.length })))
        .child(this.button("rm-fetch-all", this.busy === "fetch:" ? t("cancel") : t("remote.fetchAll"), th, idle || this.busy === "fetch:",
          (cx) => { if (this.busy === "fetch:") this.cancelWork(cx); else this.fetch(cx, null); })))
      .children(rows)
      .when(list.length === 0, (el) => el.child(div().p(16).text_color(th.dim).child(t("remote.empty"))))
      .child(v_flex().p(10).gap(8)
        .child(this.label(t("remote.add"), th))
        .child(h_flex().gap(6).child(this.field("rm-new-name", this.remName, th, 140)).child(this.field("rm-new-url", this.remUrl, th))
          .child(this.button("rm-add", t("remote.addButton"), th, canAdd, (cx) => this.act(cx, "remote", async (r) => {
            const out = await remoteEdit(r.id, "add", this.remName.value().trim(), this.remUrl.value().trim());
            if (out.ok) { this.remName.set_value(""); this.remUrl.set_value(""); }
            return out;
          }), "primary"))));
  }

  // ---- worktrees ----

  worktreesTab(th, mono) {
    const list = this.data.worktrees ?? [], idle = !this.busy, space = spaces().find((s) => s.active);
    const rows = list.map((w, i) => {
      const main = i === 0, locked = Boolean(w.locked), flag = (text, color) => div().flex_none().px(5).rounded(3).bg(th.raised).text_size(10.5).text_color(color ?? th.muted).child(text);
      return h_flex().id(`wt-${i}`).px(10).py(8).gap(8).items_center().border_b(1).border_color(th.divider ?? th.border)
        .child(v_flex().flex_1().min_w(0).gap(3)
          .child(h_flex().gap(6).items_center()
            .child(div().min_w(0).truncate().font_semibold().child(w.bare ? t("worktree.bare") : w.branch ?? `${t("worktree.detached")} ${(w.head ?? "").slice(0, 8)}`))
            .when(main, (el) => el.child(flag(t("worktree.main"))))
            .when(w.current, (el) => el.child(flag(t("worktree.current"), th.accent)))
            .when(locked, (el) => el.child(flag(typeof w.locked === "string" ? `${t("worktree.locked")}: ${w.locked}` : t("worktree.locked"), th.blocked)))
            .when(Boolean(w.prunable), (el) => el.child(flag(typeof w.prunable === "string" ? `${t("worktree.prunable")}: ${w.prunable}` : t("worktree.prunable"), th.blocked))))
          .child(div().truncate().font_family(mono).text_size(11).text_color(th.muted).child(w.path)))
        .child(this.button(`wt-lock-${i}`, locked ? t("worktree.unlock") : t("worktree.lock"), th, idle && !main, (cx) => this.act(cx, "worktree", (r) => worktreeAction(r.id, locked ? "unlock" : "lock", w.path))))
        .child(this.button(`wt-remove-${i}`, t("worktree.remove"), th, idle && !main && !w.current, (cx) => this.confirm(cx, t("worktree.removeAsk", { path: w.path }), t("worktree.remove"),
          (cx, force) => this.act(cx, "worktree", (r) => worktreeAction(r.id, "remove", w.path, force)), { label: t("worktree.force"), on: false }), "danger"));
    });
    return v_flex().id("worktrees-body").flex_1().min_h(0).overflow_y_scrollbar()
      .child(h_flex().px(10).py(8).gap(8).items_center().border_b(1).border_color(th.divider ?? th.border)
        .child(div().flex_1().text_color(th.muted).child(t("worktree.count", { count: list.length })))
        .child(this.button("wt-prune", t("worktree.prune"), th, idle, (cx) => this.act(cx, "worktree", (r) => worktreeAction(r.id, "prune"), { ok: t("worktree.pruned") })))
        // The upstream worktree dialog does the work (branch names, where to put it, which space opens it).
        .child(this.button("wt-new", t("worktree.new"), th, Boolean(space), () => openOverlay({ mod: "worktree", kind: "dialog", width: 460, height: 280, props: { space: space.id, folder: this.repo.root, action: "new" } }), "primary")))
      .children(rows)
      .when(list.length === 0, (el) => el.child(div().p(16).text_color(th.dim).child(t("worktree.empty"))));
  }

  // ---- submodules ----

  submodulesTab(th, mono) {
    const list = this.data.submodules ?? [], idle = !this.busy;
    const tone = { ok: th.muted, modified: th.blocked, uninitialized: th.dim, conflict: th.ansi1 };
    return v_flex().id("submodules-body").flex_1().min_h(0).overflow_y_scrollbar()
      .child(h_flex().px(10).py(8).gap(8).items_center().border_b(1).border_color(th.divider ?? th.border)
        .child(div().flex_1().text_color(th.muted).child(t("submodule.count", { count: list.length })))
        .child(this.button("sm-update", t("submodule.update"), th, idle && list.length > 0, (cx) => this.act(cx, "submodule", (r, token) => submoduleUpdate(r.id, { token }), { cancellable: true, ok: t("submodule.updated") }), "primary")))
      .children(list.map((s, i) => h_flex().id(`sm-${i}`).px(10).py(8).gap(8).items_center().border_b(1).border_color(th.divider ?? th.border)
        .child(v_flex().flex_1().min_w(0).gap(2)
          .child(div().truncate().font_semibold().child(s.path))
          .child(div().truncate().font_family(mono).text_size(11).text_color(th.dim).child(`${s.hash.slice(0, 10)}${s.describe ? ` · ${s.describe}` : ""}`)))
        .child(div().flex_none().text_size(11.5).text_color(tone[s.state] ?? th.muted).child(t(`submodule.${s.state}`)))))
      .when(list.length === 0, (el) => el.child(div().p(16).text_color(th.dim).child(t("submodule.empty"))));
  }

  // ---- patches ----

  applyPatch(cx, check) {
    const text = this.patchText.value();
    this.act(cx, "patch", (r) => patchApply(r.id, text, { check, ...this.patch }), { ok: check ? t("patch.clean") : t("patch.applied") });
  }

  makePatch(cx, save) {
    const staged = this.patchStaged, name = this.patchFile.value().trim() || "changes.patch";
    this.act(cx, "patch", async (r, _token, acx) => {
      const text = await patchCreate(r.id, { staged });
      if (!text.trim()) return { ok: false, output: t("patch.nothing") };
      if (!save) { acx.write_to_clipboard(text); return { ok: true, output: t("patch.copied", { count: text.split("\n").length - 1 }) }; }
      return { ok: true, output: t("patch.saved", { path: await patchSave(r.id, { staged }, r.root, name) }) };
    });
  }

  patchesTab(th) {
    const idle = !this.busy, has = Boolean(this.patchText.value().trim()), remote = Boolean(this.repo.machine);
    const box = (id, el) => div().id(id).rounded(6).border_1().border_color(th.border).bg(th.background).px(8).py(6).text_size(12).cursor_text()
      .on_mouse_down("left", (_e, cx) => cx.stop_propagation()).child(el);
    const opt = (key, label) => this.checkbox(`patch-${key}`, this.patch[key], label, th, (cx) => { this.patch[key] = !this.patch[key]; cx.notify(); });
    return v_flex().id("patches-body").flex_1().min_h(0).overflow_y_scrollbar()
      .child(v_flex().p(10).gap(8).border_b(1).border_color(th.divider ?? th.border)
        .child(div().font_semibold().child(t("patch.apply")))
        .child(box("patch-text", Textarea.new(this.patchText).w_full()))
        .child(h_flex().gap(10).flex_wrap().items_center()
          .child(opt("threeWay", t("patch.threeWay"))).child(opt("cached", t("patch.cached"))).child(opt("reverse", t("patch.reverse")))
          .child(div().flex_1())
          .child(this.button("patch-paste", t("patch.paste"), th, true, (cx) => { const x = cx.read_from_clipboard(); if (x) { this.patchText.set_value(x); cx.notify(); } }))
          .child(this.button("patch-check", t("patch.check"), th, idle && has, (cx) => this.applyPatch(cx, true)))
          .child(this.button("patch-apply", t("patch.applyButton"), th, idle && has, (cx) => this.applyPatch(cx, false), "primary"))))
      .child(v_flex().p(10).gap(8)
        .child(div().font_semibold().child(t("patch.create")))
        .child(h_flex().gap(2).p(2).rounded(6).bg(th.raised).w(260)
          .child(this.pill("patch-staged", t("patch.staged"), this.patchStaged, th, (cx) => { this.patchStaged = true; cx.notify(); }))
          .child(this.pill("patch-unstaged", t("patch.unstaged"), !this.patchStaged, th, (cx) => { this.patchStaged = false; cx.notify(); })))
        .child(h_flex().gap(8).items_center().flex_wrap()
          .child(this.button("patch-copy", t("patch.copy"), th, idle, (cx) => this.makePatch(cx, false)))
          .when(!remote, (el) => el.child(this.field("patch-file", this.patchFile, th, 180)).child(this.button("patch-save", t("patch.save"), th, idle, (cx) => this.makePatch(cx, true))))
          .when(remote, (el) => el.child(this.label(t("patch.remoteHint"), th)))));
  }

  // ---- identity and signing ----

  // The file being edited (repo or user): its own values, not the merged ones.
  layer(c) { return (this.global ? c?.global : c?.local) ?? c ?? {}; }

  fillIdentity(full) {
    const c = this.layer(full);
    this.idName.set_value(c.userName ?? ""); this.idEmail.set_value(c.userEmail ?? ""); this.idKey.set_value(c.signingKey ?? "");
    this.draft = { gpgSign: Boolean(c.gpgSign), gpgFormat: c.gpgFormat ?? "", autocrlf: c.autocrlf ?? "" };
    this.idDirty = false;
  }

  saveIdentity(cx) {
    const c = this.layer(this.data.identity), d = this.draft, global = this.global, changes = [];
    const text = (key, field, value) => { const v = value.trim(); if (v !== (c[field] ?? "")) changes.push([key, v || null]); };
    text("user.name", "userName", this.idName.value());
    text("user.email", "userEmail", this.idEmail.value());
    text("user.signingKey", "signingKey", this.idKey.value());
    if (d.gpgSign !== Boolean(c.gpgSign)) changes.push(["commit.gpgSign", d.gpgSign ? "true" : "false"]);
    if (d.gpgFormat !== (c.gpgFormat ?? "")) changes.push(["gpg.format", d.gpgFormat || null]);
    if (d.autocrlf !== (c.autocrlf ?? "")) changes.push(["core.autocrlf", d.autocrlf || null]);
    if (!changes.length) { this.note = { tone: "ok", text: t("identity.unchanged") }; cx.notify(); return; }
    this.act(cx, "identity", async (r) => {
      for (const [key, value] of changes) {
        const out = await setConfig(r.id, key, value, global);
        if (out.ok === false) return out;
      }
      this.idDirty = false;
      return { ok: true, output: t("identity.saved", { count: changes.length }) };
    });
  }

  identityTab(th, mono) {
    const c = this.data.identity, d = this.draft, idle = !this.busy;
    if (!c) return div().p(16).text_color(th.dim).child(t("loading"));
    const row = (label, control) => h_flex().gap(12).items_center().child(div().w(130).flex_none().text_color(th.muted).child(label)).child(control);
    const choice = (id, values, current, key) => h_flex().gap(2).p(2).rounded(6).bg(th.raised).flex_wrap()
      .children(values.map((v) => this.pill(`${id}-${v || "unset"}`, v || t("identity.unset"), current === v, th, (cx) => { this.draft[key] = v; this.idDirty = true; cx.notify(); })));
    return v_flex().id("identity-body").flex_1().min_h(0).overflow_y_scrollbar()
      .child(v_flex().p(10).gap(10)
        .child(h_flex().gap(10).items_center().flex_wrap()
          .child(div().text_color(th.muted).child(t("identity.scope")))
          .child(h_flex().gap(2).p(2).rounded(6).bg(th.raised)
            .child(this.pill("scope-local", t("identity.local"), !this.global, th, (cx) => { this.global = false; if (c) this.fillIdentity(c); cx.notify(); }))
            .child(this.pill("scope-global", t("identity.global"), this.global, th, (cx) => { this.global = true; if (c) this.fillIdentity(c); cx.notify(); }))))
        .child(this.label(this.global ? t("identity.globalHint") : t("identity.localHint"), th))
        .child(row("user.name", this.field("id-name", this.idName, th, 280)))
        .child(row("user.email", this.field("id-email", this.idEmail, th, 280)))
        .child(row("commit.gpgSign", this.checkbox("id-sign", d.gpgSign, t("identity.sign"), th, (cx) => { this.draft.gpgSign = !this.draft.gpgSign; this.idDirty = true; cx.notify(); })))
        .child(row("user.signingKey", this.field("id-key", this.idKey, th, 280)))
        .child(row("gpg.format", choice("id-format", FORMATS, d.gpgFormat, "gpgFormat")))
        .child(row("core.autocrlf", choice("id-crlf", CRLF, d.autocrlf, "autocrlf")))
        .child(h_flex().gap(8).child(this.button("id-save", t("save"), th, idle, (cx) => this.saveIdentity(cx), "primary")))
        .child(v_flex().gap(3).pt(6).border_t(1).border_color(th.divider ?? th.border)
          .child(this.label(`${t("identity.version")}: ${c.version || "?"}`, th))
          .child(this.label(`${t("identity.executable")}: ${c.executable || "git"}`, th))
          .when(Boolean(c.hooksPath), (el) => el.child(this.label(`core.hooksPath: ${c.hooksPath}`, th)))));
  }

  render(cx) {
    const th = theme(), mono = settings().font_family ?? "JetBrains Mono";
    const root = v_flex().id("git-repo").size_full().min_h(0).bg(th.background).text_color(th.foreground).text_size(12);
    if (!this.repo) return root.child(div().p(16).text_color(this.error ? th.ansi1 : th.dim).child(this.error ?? t("loading")));

    const tabs = h_flex().flex_none().flex_wrap().px(8).py(6).gap(2).items_center().border_b(1).border_color(th.border)
      .children(TABS.map((id) => div().id(`tab-${id}`).h(26).px(10).flex().items_center().rounded(5).text_size(12).cursor_pointer()
        .bg(this.tab === id ? th.selected : th.background).text_color(this.tab === id ? th.foreground : th.muted).hover((s) => s.text_color(th.foreground))
        .on_click((_e, cx) => this.go(id, cx)).child(t(`tab.${id}`))))
      .child(div().flex_1())
      .child(div().min_w(0).truncate().text_size(11).text_color(th.dim).child(this.repo.root.split(/[\\/]/).pop()));

    const tone = { error: th.ansi1, warning: th.blocked, ok: th.muted };
    const status = this.busy && h_flex().flex_none().px(10).py(5).gap(8).items_center().text_size(11.5).text_color(th.muted)
      .child(div().child(t("working")))
      .when(Boolean(this.token), (el) => el.child(this.button("cancel-work", t("cancel"), th, !this.cancelled, (cx) => this.cancelWork(cx))));
    const note = this.note && !this.busy && div().id("note").flex_none().max_h(110).overflow_y_scrollbar().px(10).py(5).font_family(mono).text_size(11).text_color(tone[this.note.tone] ?? th.muted).child(this.note.text);
    const ask = this.ask && v_flex().flex_none().px(10).py(8).gap(6).bg((tokens().attention ?? {}).background ?? th.raised).text_size(11.5)
      .child(div().child(this.ask.text))
      .when(Boolean(this.ask.check), (el) => el.child(this.checkbox("ask-check", this.ask.check.on, this.ask.check.label, th, (cx) => { this.ask.check.on = !this.ask.check.on; cx.notify(); })))
      .child(h_flex().gap(6)
        .child(this.button("ask-yes", this.ask.label, th, true, (cx) => { const a = this.ask; this.ask = null; a.run(cx, Boolean(a.check?.on)); }, "danger"))
        .child(this.button("ask-no", t("cancel"), th, true, (cx) => { this.ask = null; cx.notify(); })));

    const body = {
      stashes: () => this.stashesTab(th, mono), tags: () => this.tagsTab(th, mono), remotes: () => this.remotesTab(th, mono), worktrees: () => this.worktreesTab(th, mono),
      submodules: () => this.submodulesTab(th, mono), patches: () => this.patchesTab(th), identity: () => this.identityTab(th, mono),
    }[this.tab]();
    return root.child(tabs).when(Boolean(ask), (el) => el.child(ask)).when(Boolean(status), (el) => el.child(status)).when(Boolean(note), (el) => el.child(note)).child(body);
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
  const s = gitSettings();
  const pane = s.diffFollowPane !== false && panes().find((p) => p.mod === "yuzora-git-diff");
  if (pane) {
    setShared("yuzora.git.diff", spec);
    return run("pane.focus", { pane: pane.pane });
  }
  if (s.diffOpenIn === "pane") return placeDiff(spec, s.diffKeepIn);
  openOverlay({ mod: "yuzora-git-diff", kind: "dialog", width: 0.9, height: 0.86, props: spec });
}
