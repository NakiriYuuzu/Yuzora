// Annotate (git blame): every line with the commit that last changed it.
// Props {cwd, on, path, rev?}: rev null is the work tree. Owned by lane L2.
// A click on the gutter shows the commit; "Annotate previous revision" blames
// the file as that commit's parent had it (renames followed by git itself).
import { View, div } from "gpui-kit";
import { v_flex, h_flex, v_virtual_list, Scrollbar } from "gpui-base";
import { theme, props, settings, run, panes, setShared } from "herdr";
import { t } from "yuzora:core";
import { open, blame, commitDetails, settings as gitSettings } from "yuzora:git";

const ROW = 18, CHAR = 7, NUM = 44, GUTTER = 250;
// Rust's names for -M and -C: git blame prints the same for both (see ops.rs).
const MOVES = ["none", "inner", "outer"];

const pad = (n) => String(n).padStart(2, "0");
const day = (s) => { const d = new Date(s * 1000); return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`; };
const stamp = (s) => { const d = new Date(s * 1000); return `${day(s)} ${pad(d.getHours())}:${pad(d.getMinutes())}`; };
// Lines of work tree changes blame to the all-zero hash.
const uncommitted = (hash) => /^0+$/.test(hash);
// Theme colors are #rgb or #rrggbb: a tint of one is the same plus an alpha byte.
const tint = (color, a) => {
  const m = /^#([0-9a-f]{3}|[0-9a-f]{6})/i.exec(color ?? "");
  if (!m) return color;
  const hex = m[1].length === 3 ? [...m[1]].map((x) => x + x).join("") : m[1];
  return `#${hex}${Math.round(a * 255).toString(16).padStart(2, "0")}`;
};

export default class GitBlame extends View {
  init(_props, cx) {
    const g = gitSettings(), p = props() ?? {};
    this.ws = Boolean(g.annotateIgnoreSpaces);
    this.move = MOVES.includes(g.annotateDetectMovement) ? g.annotateDetectMovement : "none";
    this.spec = p.cwd && p.path ? { cwd: p.cwd, on: p.on ?? null } : null;
    this.cur = this.spec ? { path: p.path, rev: p.rev || null } : null;
    this.stack = []; this.repo = null; this.data = null; this.error = null; this.pick = null; this.details = {}; this.seq = 0;
    if (!this.spec) return;
    cx.spawn(async (cx) => {
      try {
        this.repo = await open(this.spec.cwd, this.spec.on);
        this.load(cx);
      } catch (e) { this.error = String(e); cx.notify(); }
    });
  }

  // Called from click handlers, whose cx ends with the call: the waiting is in a task.
  load(cx) {
    const seq = ++this.seq, { path, rev } = this.cur;
    this.data = null; this.error = null; this.pick = null;
    cx.spawn(async (cx) => {
      try {
        const d = await blame(this.repo.id, path, rev, { ignoreWhitespace: this.ws, movement: this.move });
        if (this.seq !== seq) return;
        this.data = this.shape(d);
      } catch (e) { if (this.seq === seq) this.error = String(e); }
      cx.notify();
    });
    cx.notify();
  }

  // Per commit, how new it is (0 oldest, 1 newest) for the gutter's shade.
  shape(d) {
    const commits = d.commits ?? {}, age = {};
    const times = Object.entries(commits).filter(([h]) => !uncommitted(h)).map(([, c]) => c.time ?? 0);
    const lo = times.reduce((m, x) => Math.min(m, x), Infinity), hi = times.reduce((m, x) => Math.max(m, x), 0);
    for (const [h, c] of Object.entries(commits)) age[h] = uncommitted(h) ? 1 : hi === lo ? 0.5 : ((c.time ?? 0) - lo) / (hi - lo);
    const longest = d.lines.reduce((m, l) => Math.max(m, l.text.length), 0);
    return { lines: d.lines, commits, age, longest: Math.min(longest, 2000) };
  }

  choose(hash, cx) {
    this.pick = this.pick === hash ? null : hash;
    if (this.pick && !uncommitted(hash) && !this.details[hash]) {
      const repo = this.repo;
      cx.spawn(async (cx) => {
        try { this.details[hash] = await commitDetails(repo.id, hash); } catch {}
        cx.notify();
      });
    }
    cx.notify();
  }

  // The commit's parent and the file's name there: git's `previous` line.
  previous(c) {
    if (!c.previous) return null;
    const at = c.previous.indexOf(" ");
    return at < 0 ? null : { rev: c.previous.slice(0, at), path: c.previous.slice(at + 1) };
  }

  annotate(prev, cx) {
    this.stack.push(this.cur);
    this.cur = { path: prev.path, rev: prev.rev };
    this.load(cx);
  }

  back(cx) {
    this.cur = this.stack.pop();
    this.load(cx);
  }

  showLog(hash) {
    const repo = this.repo;
    run("tab.new", { name: `${t("log.tab")} · ${hash.slice(0, 8)}`, mod: "yuzora-git-log", props: { cwd: repo.root, on: repo.machine, filter: { hashes: [hash] } } });
  }

  fileHistory() {
    const repo = this.repo, path = this.cur.path;
    run("tab.new", { name: `${t("history")} · ${path.split("/").pop()}`, mod: "yuzora-git-log", props: { cwd: repo.root, on: repo.machine, filter: { paths: [path], follow: true } } });
  }

  compare(hash, c, prev) {
    const repo = this.repo, path = c.filename ?? this.cur.path;
    const spec = { cwd: repo.root, on: repo.machine, path, orig: prev && prev.path !== path ? prev.path : null, mode: "commit", rev: hash, at: Date.now() };
    setShared("yuzora.git.diff", spec);
    const pane = panes().find((p) => p.mod === "yuzora-git-diff");
    if (pane) run("pane.focus", { pane: pane.pane });
    else run("tab.new", { name: "Diff", mod: "yuzora-git-diff", props: spec });
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

  row(i, th) {
    const { lines, commits, age } = this.data, l = lines[i], c = commits[l.hash] ?? {};
    // The gutter names the commit once per run of lines.
    const first = i === 0 || lines[i - 1].hash !== l.hash;
    const fresh = uncommitted(l.hash);
    // No handlers in here: a list's rows are drawn during layout (see on_item_click below).
    return h_flex().h(ROW).w_full().items_center().cursor_pointer().when(this.pick === l.hash, (el) => el.bg(tint(th.accent, 0.12)))
      .child(h_flex().w(GUTTER).h_full().flex_none().relative().overflow_hidden().items_center().gap(6).pl(8).pr(6)
        .child(div().absolute().inset_0().bg(tint(th.accent, 0.04 + 0.2 * (age[l.hash] ?? 0))))
        .when(first, (el) => el
          .child(div().w(58).flex_none().text_size(11).text_color(th.muted).child(fresh ? "" : l.hash.slice(0, 8)))
          .child(div().flex_1().min_w(0).truncate().text_size(11).child(fresh ? t("uncommitted") : c.author ?? ""))
          .child(div().flex_none().text_size(10.5).text_color(th.dim).child(!fresh && c.time ? day(c.time) : ""))))
      .child(div().w(NUM).flex_none().pr(8).text_right().text_color(th.dim).child(String(l.line)))
      .child(div().flex_1().whitespace_nowrap().child(l.text));
  }

  panel(th, mono) {
    const hash = this.pick, c = this.data.commits[hash] ?? {}, det = this.details[hash], fresh = uncommitted(hash);
    const prev = this.previous(c);
    return v_flex().flex_none().p(10).gap(6).border_t(1).border_color(th.border).bg(th.chrome).text_size(12)
      .child(h_flex().gap(8).items_center()
        .child(div().flex_none().font_family(mono).text_size(11.5).text_color(th.muted).child(fresh ? "" : hash.slice(0, 10)))
        .child(div().flex_1().min_w(0).truncate().font_semibold().child(fresh ? t("uncommitted") : det?.subject ?? c.summary ?? ""))
        .child(div().id("panel-close").px(4).text_color(th.muted).cursor_pointer().hover((s) => s.text_color(th.foreground)).tooltip(t("close"))
          .on_click((_e, cx) => { this.pick = null; cx.notify(); }).child("✕")))
      .when(!fresh, (el) => el.child(div().text_size(11.5).text_color(th.muted).child(`${c.author ?? ""} <${c.email ?? ""}>${c.time ? ` · ${stamp(c.time)}` : ""}`)))
      .when(Boolean(det?.body), (el) => el.child(div().id("panel-body").max_h(84).overflow_y_scrollbar().text_size(11.5).text_color(th.muted).child(det.body)))
      .when(!fresh, (el) => el.child(h_flex().gap(6).flex_wrap()
        .child(this.button("p-log", t("commit.log"), th, true, () => this.showLog(hash)))
        .child(this.button("p-prev", t("commit.previous"), th, Boolean(prev), (cx) => this.annotate(prev, cx)))
        .child(this.button("p-diff", t("commit.diff"), th, true, () => this.compare(hash, c, prev)))));
  }

  render(cx) {
    const th = theme();
    const root = v_flex().id("git-blame").size_full().min_h(0).bg(th.background).text_color(th.foreground).text_size(12);
    if (!this.spec) return root.child(div().p(16).text_color(th.dim).child(t("empty")));
    const mono = settings().font_family ?? "JetBrains Mono";
    const { path, rev } = this.cur, d = this.data, ready = Boolean(this.repo);
    const where = rev ? `${path} @ ${rev.length > 12 ? rev.slice(0, 8) : rev}` : path;

    const toolbar = h_flex().h(30).px(8).gap(7).flex_none().items_center().border_b(1).border_color(th.border).text_size(12)
      .when(this.stack.length > 0, (el) => el.child(this.button("back", t("back"), th, true, (cx) => this.back(cx))))
      .child(div().font_semibold().child(t("title")))
      .child(div().flex_1().min_w(0).truncate().text_color(th.muted).child(where))
      .child(this.pill("ws", t("ignoreSpaces"), this.ws, th, (cx) => { this.ws = !this.ws; this.load(cx); }))
      .child(div().flex_none().text_size(11).text_color(th.dim).child(t("moves")))
      .child(h_flex().flex_none().p(2).gap(2).rounded(5).bg(th.raised)
        .children(MOVES.map((m) => this.pill(`mv-${m}`, t(`moves.${m}`), this.move === m, th, (cx) => { if (this.move !== m) { this.move = m; this.load(cx); } }))))
      .child(this.button("history", t("history"), th, ready, () => this.fileHistory()))
      .child(this.button("refresh", t("refresh"), th, ready, (cx) => this.load(cx)));

    let body;
    if (this.error) body = div().p(16).text_color(th.ansi1).child(this.error);
    else if (!d) body = div().p(16).text_color(th.dim).child(t("loading"));
    else if (!d.lines.length) body = div().p(16).text_color(th.dim).child(t("emptyFile"));
    else {
      // A new blame is a new list, so it starts at the top.
      const list = `blame-${this.seq}`, width = d.longest * CHAR + 24;
      body = div().flex_1().min_h(0).relative().font_family(mono).text_size(11.5)
        .child(div().id("blame-x").size_full().overflow_x_scrollbar()
          .child(div().w_full().min_w(GUTTER + NUM + width).h_full()
            .child(v_virtual_list(list, d.lines.length, ROW, (i) => String(i),
              (range) => d.lines.slice(range.start, range.end).map((_, k) => this.row(range.start + k, th)))
              .on_item_click((key, cx) => this.choose(d.lines[Number(key)].hash, cx)).size_full())))
        .child(Scrollbar.vertical(list).viewport_from_layout().absolute().inset_0());
    }

    return root.child(toolbar).child(body).when(Boolean(d && this.pick), (el) => el.child(this.panel(th, mono)));
  }
}
