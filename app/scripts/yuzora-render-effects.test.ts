// yuzora(v0.1.0 db-retarget): what Yuzora's mods do in a frame (scripts/yuzora-render-effects.ts).
// Run: bun test scripts/yuzora-render-effects.test.ts
//
// A frame may only look. The engine throws on cx.notify(), creating state and changing an input's selection
// there, and a request that arrives through shared state is only ever seen from a frame (a mod gets no event
// for a change of `shared`): the Database table pane and the Files sidebar both started work in that frame,
// threw half way, and stayed half done. The list below is every call the scanner sees a Yuzora render reach that is
// not a read, each with why it is safe in a frame. A new one fails the test until it has been looked at.
// A call written in a way the scanner cannot follow is not in that list: it is in `unsupported`, which the
// last tests keep empty except for what has been read by hand. What the scanner does not see at all is
// listed at the top of yuzora-render-effects.ts.
import { expect, test } from "bun:test";
import { readdirSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { loadMod, modFrom, scanAll, scanMod, yuzoraMods } from "./yuzora-render-effects";

const root = resolve(dirname(new URL(import.meta.url).pathname), "..");

const scan = (files: Record<string, string>) => scanMod(modFrom("m", files));
const keys = (files: Record<string, string>) => scan(files).findings.map((f) => `${f.where}:${f.detail}`).sort();

// The shapes of the two bugs, and of what is fine, as a mod would write them.
const HEAD = `import { View, div } from "gpui-kit"; import { shared } from "herdr"; import { job } from "yuzora:db";`;

test("a render that starts a load which notifies is found, with the way it got there", () => {
  const src = `${HEAD}
export default class P extends View {
  guard(cx, action) { if (this.edits) this.confirm = { action }; else action(cx); }
  retarget(cx) { const o = shared("k"); this.guard(cx, (cx) => { this.target = o; this.load(cx); }); }
  load(cx) { this.watch(cx, 1); }
  watch(cx, id) { this.id = id; cx.notify(); }
  render(cx) { this.retarget(cx); return div(); }
}`;
  const found = scan({ "main.js": src }).findings;
  expect(found.map((f) => [f.detail, f.effect])).toEqual([["cx.notify", "notify"]]);
  expect(found[0].chain).toEqual(["P.render", "P.retarget", "P.load", "P.watch", "cx.notify"]);
});

test("what a handler, a timer or a stored closure does waits for its own phase", () => {
  const src = `${HEAD}
const button = (text, onClick) => div().id(text).on_click(onClick).child(text);
export default class P extends View {
  retarget(cx) { this.go(cx); }
  go(cx) { this.target = shared("k"); cx.notify(); }
  render(cx) {
    const leave = () => { this.confirm = null; cx.notify(); };
    const o = shared("k");
    if (o) cx.timer.after(0, (cx) => this.retarget(cx));
    return div().on_click((_e, cx) => cx.notify()).on_key_down((e, cx) => this.go(cx))
      .child(button("keep", (_e, cx) => { leave(); cx.notify(); }));
  }
}`;
  expect(keys({ "main.js": src })).toEqual([]);
});

test("a task runs in the frame up to its first await, and not after it", () => {
  const before = `${HEAD}
export default class P extends View {
  render(cx) { cx.spawn(async (cx) => { cx.notify(); await job(1); }); return div(); }
}`;
  const after = `${HEAD}
export default class P extends View {
  render(cx) { this.go(cx); return div(); }
  go(cx) { cx.spawn(async (cx) => { const st = await job(1); this.st = st; cx.notify(); }); }
  async later(cx) { await job(1); cx.notify(); }
  other(cx) { this.later(cx); }
}`;
  expect(keys({ "main.js": before })).toEqual(["P.render:cx.notify", "P.render:spawn"]);
  expect(keys({ "main.js": after })).toEqual(["P.go:spawn"]);
});

test("a closure given to a callee that calls it runs in the frame: map, when, a local guard, a virtual list", () => {
  const src = `${HEAD}
import { v_virtual_list } from "gpui-base";
export default class P extends View {
  render(cx) {
    return div().children([1, 2].map((x) => { cx.notify(); return div(); }))
      .when(this.on, (el) => { this.input.set_selected_range({ start: 0, end: 1 }); return el; })
      .child(v_virtual_list("l", 3, (range) => { cx.bind_keys([]); return []; }));
  }
}`;
  expect(keys({ "main.js": src })).toEqual(["P.render:bind_keys", "P.render:cx.notify", "P.render:set_selected_range"]);
});

test("calls through a helper file and a parent class are followed", () => {
  const files = {
    "doc.js": `import { View } from "gpui-kit"; export class Doc extends View { render(cx) { return this.bar(cx); } bar(cx) { return open(cx); } }
function open(cx) { cx.notify(); }`,
    "main.js": `import { Doc } from "./doc.js"; import { extra } from "./extra.js";
export default class P extends Doc { render(cx) { extra(cx); return super.render(cx); } }`,
    "extra.js": `import { InputState } from "gpui-base"; export function extra(cx) { return InputState.new(); }`,
  };
  expect(keys(files)).toEqual(["doc.js#open:cx.notify", "extra.js#extra:InputState.new"]);
});

test("a call through a namespace import is followed into that file", () => {
  const files = {
    "ui.js": `export function head(cx) { cx.notify(); return 1; } export function pick(th, then) { return div().on_click(then); }`,
    "main.js": `import * as ui from "./ui.js";
export default class P extends View { render(cx) { ui.pick(1, (e, cx) => cx.notify()); return ui.head(cx); } }`,
  };
  expect(keys(files)).toEqual(["ui.js#head:cx.notify"]);
});

test("a super call, a subclass that inherits its render, and the window, theme and dock calls the engine refuses", () => {
  const files = {
    "base.js": `import { View } from "gpui-kit"; import { set_theme } from "gpui-kit";
export class Base extends View { render(cx) { set_theme("x"); return this.body(cx); } body(cx) { window.refresh(); return 1; } }`,
    "main.js": `import { Base } from "./base.js";
export default class P extends Base { body(cx) { cx.new(Q, {}); return super.body(cx); } }`,
  };
  expect(keys(files)).toEqual(["P.body:cx.new", "base.js#Base.body:refresh", "base.js#Base.render:set_theme"]);
});

test("template literals, regular expressions and comments do not hide or invent calls", () => {
  const src = `${HEAD}
export default class P extends View {
  render(cx) {
    const re = /\\/\\*[a-z]+\\/(?:x)/g; // cx.notify() in a comment
    /* cx.notify() in another */
    const text = \`a \${this.name(cx)} b \${"cx.notify()"}\`;
    return div().child(text).child(re.test("x") ? "y" : "n");
  }
  name(cx) { cx.focus_handle(); return "n"; }
}`;
  expect(keys({ "main.js": src })).toEqual(["P.name:focus_handle"]);
});

// A call written in a way that is one token off the plain one is still a call.
test("an optional call is a call: this.go?.(cx), cx.notify?.() and a local closure", () => {
  const head = `${HEAD}
export default class P extends View {`;
  const viaThis = `${head}
  go(cx) { cx.notify(); }
  render(cx) { this.go?.(cx); return div(); }
}`;
  const onCx = `${head}
  render(cx) { cx.notify?.(); return div(); }
}`;
  const local = `${head}
  render(cx) { const f = (cx) => cx.notify(); f?.(cx); return div(); }
}`;
  expect(keys({ "main.js": viaThis })).toEqual(["P.go:cx.notify"]);
  expect(keys({ "main.js": onCx })).toEqual(["P.render:cx.notify"]);
  expect(keys({ "main.js": local })).toEqual(["P.render/f:cx.notify"]);
});

test("reading a getter runs it", () => {
  const src = `${HEAD}
import { setShared } from "herdr";
export default class P extends View {
  get pick() { setShared("k", 1); return 1; }
  set pick(v) { this.v = v; }
  render(cx) { const n = this.pick; return div().child(String(n)); }
}`;
  expect(keys({ "main.js": src })).toEqual(["P.pick:setShared"]);
});

test("an object of arrows indexed and called calls them all: the way a pane picks its tab", () => {
  const src = `${HEAD}
export default class P extends View {
  one() { return 1; }
  two(cx) { cx.notify(); return 2; }
  render(cx) { return div().child(String({ a: () => this.one(), b: () => this.two(cx) }[this.tab]())); }
}`;
  expect(keys({ "main.js": src })).toEqual(["P.two:cx.notify"]);
  expect(scan({ "main.js": src }).unsupported).toEqual([]);
});

// What this reader does not follow must not read as "nothing": it is reported, with its place.
const UNFOLLOWED: [string, string, string, number][] = [
  ["bind", `const go = this.go.bind(this); go(cx);`, ".bind()", 3],
  ["call", `const { go } = this; go.call(this, cx);`, ".call()", 3],
  ["apply", `this.go.apply(this, [cx]);`, ".apply()", 3],
  ["an alias of this", `const self = this; self.go(cx);`, "this as a value", 3],
  ["destructuring this", `const { go } = this; go(cx);`, "this as a value", 3],
  ["a computed member", `this["go"](cx);`, "a call of a computed callee", 3],
  ["a computed callee", `({ a: () => cx.notify() })["a"]();`, "a call of a computed callee", 3],
  ["a parenthesized callee", `(this.mode ? this.go : this.stay)(cx);`, "a call of a computed callee", 3],
  ["a closure chosen by a condition", `const f = this.mode ? (cx) => cx.notify() : null; f?.(cx);`, "a call through a variable: f", 3],
  ["a method that is not the class's", `this.cb(cx);`, "this.cb() is not a method of the class", 3],
];

for (const [name, body, form, line] of UNFOLLOWED) {
  test(`${name} in a render is reported, not read as clean`, () => {
    const src = `${HEAD}
export default class P extends View {
  render(cx) { ${body} return div(); }
  go(cx) { cx.notify(); }
  stay(cx) { return 1; }
}`;
    const report = scan({ "main.js": src });
    expect(report.unsupported.map((u) => [u.where, u.form, u.line])).toContainEqual(["P.render", form, line]);
  });
}

// Only an object whose every value is an arrow is read as called by its key; any other is not known.
test("an object indexed and called is followed when it is arrows, and reported when it holds anything else", () => {
  const unsupported = (literal: string) => {
    const src = `${HEAD}
function go(cx) { cx.notify(); }
export default class P extends View {
  one() { return 1; }
  render(cx) { return div().child(String(${literal}[this.tab](cx))); }
}`;
    return scan({ "main.js": src }).unsupported.map((u) => [u.where, u.form]);
  };
  expect(unsupported("{ a: go }")).toEqual([["P.render", "a call of a computed callee"]]);
  expect(unsupported("{ a(cx) { cx.notify(); } }")).toEqual([["P.render", "a call of a computed callee"]]);
  expect(unsupported("{ a: () => this.one(), b: go }")).toEqual([["P.render", "a call of a computed callee"]]);
  expect(unsupported("{ a: () => this.one(), b: (cx) => this.one() }")).toEqual([]);
});

test("a class of the mod that has a constructor is not followed by new, and one that has none is plain", () => {
  const src = (cls: string) => `${HEAD}
${cls}
export default class P extends View {
  render(cx) { const l = new Local(cx); return div().child(String(l)); }
}`;
  expect(scan({ "main.js": src("class Local { constructor(cx) { cx.notify(); } }") }).unsupported.map((u) => [u.where, u.form])).toEqual([["P.render", "new Local()"]]);
  expect(scan({ "main.js": src("class Local { tag() { return 1; } }") }).unsupported).toEqual([]);
});

test("a await that is not always reached is reported, and one that always is is not", () => {
  const sources = (await_: string) => `${HEAD}
export default class P extends View {
  render(cx) { this.go(cx, false); return div(); }
  async go(cx, w) { ${await_} cx.notify(); }
}`;
  for (const cond of ["if (w) await job(1);", "if (w) { await job(1); }", "w && await job(1);", "w ? await job(1) : 0;", "while (w) { await job(1); }", "try { if (w) { await job(1); } } catch (e) {}", "switch (w) { case 1: await job(1); }"]) {
    expect(scan({ "main.js": sources(cond) }).unsupported.map((u) => u.form)).toEqual(["a conditional await"]);
  }
  for (const always of ["await job(1);", "const x = await job(1);", "try { await job(1); } catch (e) {}", "for (;;) { await job(1); break; }"]) {
    expect(scan({ "main.js": sources(always) }).unsupported).toEqual([]);
    expect(keys({ "main.js": sources(always) })).toEqual([]);
  }
});

test("a method of the class that is named apply, and a template with two holes, are plain calls", () => {
  const src = `${HEAD}
export default class P extends View {
  apply(cx, a) { return a; }
  render(cx) {
    const text = \`a \${this.name()} b \${this.name()}\`;
    this.apply(cx, 1);
    return div().child(text);
  }
  name() { return "n"; }
}`;
  expect(scan({ "main.js": src }).unsupported).toEqual([]);
});

// The defect this pane had, put back in each way the scanner is able to see it.
test("the Database table pane's defect, put back in any way a call can be written, is never read as clean", () => {
  const real = readFileSync(join(root, "defaults/mods/yuzora-db-table/main.js"), "utf8");
  expect(real).toContain("this.picked(cx);");
  const clean = scanMod(modFrom("yuzora-db-table", { "main.js": real }));
  expect(clean.findings).toEqual([]);
  expect(clean.unsupported).toEqual([]);
  const writings = [
    "this.retarget(cx);",
    "this.retarget?.(cx);",
    "const self = this; self.retarget(cx);",
    "const go = this.retarget.bind(this); go(cx);",
    "const { retarget } = this; retarget.call(this, cx);",
    'this["retarget"](cx);',
    "(this.picked ? this.retarget : this.picked).call(this, cx);",
    "const f = this.seq ? (cx) => this.retarget(cx) : null; f?.(cx);",
  ];
  for (const writing of writings) {
    const report = scanMod(modFrom("yuzora-db-table", { "main.js": real.replace("this.picked(cx);", writing) }));
    expect([writing, report.findings.length + report.unsupported.length > 0]).toEqual([writing, true]);
  }
});

test("every Yuzora mod file is read whole, and every render is found", () => {
  const mods = yuzoraMods(root);
  expect(mods.length).toBeGreaterThan(40);
  const { renders } = scanAll(root);
  const missing: string[] = [];
  for (const m of mods) {
    const files = readdirSync(m.dir).filter((f) => f.endsWith(".js"));
    expect(loadMod(m.name, m.dir).units.size).toBe(files.length);
    const hasRender = files.some((f) => /\n {2}render\(/.test(readFileSync(join(m.dir, f), "utf8")));
    if (hasRender && !renders.some((r) => r.startsWith(`${m.name}/`))) missing.push(m.name);
  }
  expect(missing).toEqual([]);
});

// Every call a Yuzora render can reach that is not a read, and why it is safe there. `cx.notify`, `refused` and
// `set_selected_range` are what the engine throws on in a frame: none stands here for a mod that is shipped.
// A `pending` one is a fix that is in another branch: it need not be found.
const REVIEWED: Record<string, { why: string; pending?: boolean }> = {
  "yuzora-sidebar-git/SidebarGit.attach:spawn": { why: "the place is looked at in the frame it changes in (lifecycle_tests: yuzora_git_sidebar_follows_its_place_in_the_frame_it_changes); the task notifies only after an await" },
  "yuzora-sidebar-git/SidebarGit.show:set_value": { why: "set_value of a text is not refused in a frame (set_selected_range, replace and the token calls are); it runs from attach, in the same test" },
  "yuzora-sidebar-git/SidebarGit.publish:setShared": { why: "writes the badge and the status only when they differ from what is shared" },
  "yuzora-shell-tabs/badgesOf:setShared": { why: "written only when spaceBadges says the kept colors changed" },
  "yuzora-shell-tabs/syncSpaceColors:setShared": { why: "written only when a Space has a hex of ours and Herdr's is not one" },
  "yuzora-shell-tabs/Tabs.syncPanel:run": { why: "layout.select and layout.show are queued once per Space switch, and `pending` holds the frame that asks again" },
  "yuzora-shell-pane/badgesOf:setShared": { why: "written only when spaceBadges says the kept colors changed" },
  "yuzora-statusbar/badgesOf:setShared": { why: "written only when spaceBadges says the kept colors changed" },
  "yuzora-statusbar/Statusbar.ask:spawn": { why: "one ask at a time for a folder; the task notifies after its await" },
  "yuzora-modebar/Modebar.legacy:setShared": { why: "written only when the count of blocked agents changed" },
  "yuzora-modebar/syncSpaceColors:setShared": { why: "written only when a Space has a hex of ours and Herdr's is not one" },
  "yuzora-sidebar-database/SidebarDatabase.status:setShared": { why: "compares with the shared value first, for the status and the badge" },
  "yuzora-sidebar-workspace/SidebarWorkspace.render:setAgentOrder": { why: "sets a list the next-agent command reads, and asks for no redraw (mods.rs setAgentOrder)" },
  "yuzora-git-merge/GitMerge.start:spawn": { why: "start() sets the state the frame draws and says so: no notify before the first await" },
  "yuzora-git-merge/retirePlain:setShared": { why: "once per run: it is guarded by yuzora.git.run" },
  "yuzora-git-diff/GitDiff.follow:spawn": { why: "follow() sets the state the frame draws and says so: no notify before the first await" },
  "yuzora-git-diff/retirePlain:setShared": { why: "once per run: it is guarded by yuzora.git.run" },
  "yuzora-git-diff/GitDiff.look:diffLook": { why: "numbers the theme's colours in yuzora-git's table of looks (the same colours, the same number, kept once) and notifies nothing; asked only when the colours change" },
  "yuzora-git-rebase/GitRebase.begin:spawn": { why: "begin() sets the state the frame draws and says so: no notify before the first await" },
  "yuzora-git-rebase/GitRebase.begin:set_value": { why: "set_value of a text is not refused in a frame" },
  "yuzora-git-rebase/retirePlain:setShared": { why: "once per run: it is guarded by yuzora.git.run" },
  "yuzora-git-log/GitLog.onScreen:spawn": { why: "the claim is a task with nothing to notify" },
  "yuzora-git-log/GitLog.begin:spawn": { why: "the task notifies after its awaits" },
  "yuzora-git-log/claimLog:setShared": { why: "written only when the tab's place differs from the one shared" },
  "frame/settings/settings/main.js#Settings.loadPreviews:spawn": { why: "asked only when the layout names changed; the task notifies after its awaits" },
  "yuzora-sidebar-files/FilesSidebar.reload:spawn": { why: "the task notifies after its awaits" },
  "yuzora-sidebar-files/FilesSidebar.startEdit:toast": { why: "a toast is a queued request", pending: true },
  "yuzora-sidebar-files/FilesSidebar.startEdit:set_value": { why: "set_value of a text is not refused in a frame", pending: true },
  "yuzora-sidebar-files/FilesSidebar.startEdit:set_selected_range": { why: "REFUSED in a frame: the right-click popup's Rename; fixed in claude/v010-files-open (the edit starts from a timer)", pending: true },
  "yuzora-sidebar-files/FilesSidebar.startEdit:focus": { why: "focusing the input is not refused in a frame", pending: true },
  "yuzora-sidebar-files/FilesSidebar.startEdit:cx.notify": { why: "REFUSED in a frame: the right-click popup's New file and New folder; fixed in claude/v010-files-open (the edit starts from a timer)", pending: true },
};

test("what a Yuzora render reaches that is not a read has been looked at", () => {
  const found = scanAll(root).findings.map((f) => `${f.mod}/${f.where}:${f.detail}`).sort();
  const unreviewed = found.filter((k) => !(k in REVIEWED));
  expect(unreviewed).toEqual([]);
  const gone = Object.entries(REVIEWED).filter(([k, v]) => !v.pending && !found.includes(k)).map(([k]) => k);
  expect(gone).toEqual([]);
});

// Written as a call the scanner cannot follow, and read by hand: what each one reaches, and why that is safe in a frame.
const UNSUPPORTED_REVIEWED: Record<string, string> = {
  "frame/settings/settings/main.js#Settings.loadPreviews:a conditional await": "the loop awaits for each name, and loadPreviews is only asked for at least one: an empty list joins to the '' that previewsOf starts as, so nothing is asked (main.js:263)",
  "frame/welcome/welcome/main.js#Welcome.render:.call()": "the page is one of four methods read by index; the next test follows all four",
  "frame/welcome/welcome/main.js#Welcome.render:this as a value": "the same expression: this is the receiver of the .call()",
};

test("a call the scanner cannot follow, found in a Yuzora render, has been read by hand", () => {
  const found = scanAll(root).unsupported.map((u) => `${u.mod}/${u.where}:${u.form}`).sort();
  expect(found.filter((k) => !(k in UNSUPPORTED_REVIEWED))).toEqual([]);
  expect(Object.keys(UNSUPPORTED_REVIEWED).filter((k) => !found.includes(k))).toEqual([]);
});

test("the four pages of the welcome, which its render calls through .call(), reach one write, and a guarded one", () => {
  const dir = join(root, "defaults/frame/welcome");
  const files = Object.fromEntries(readdirSync(dir).filter((f) => f.endsWith(".js")).map((f) => [f, readFileSync(join(dir, f), "utf8")]));
  const dispatch = "[this.pageOne, this.pageTwo, this.pageThree, this.pageFour][this.page - 1].call(this, t)";
  expect(files["main.js"]).toContain(dispatch);
  files["main.js"] = files["main.js"].replace(dispatch, "[this.pageOne(t), this.pageTwo(t), this.pageThree(t), this.pageFour(t)][this.page - 1]");
  const report = scanMod(modFrom("frame/welcome", files));
  expect(report.unsupported).toEqual([]);
  expect(report.findings.map((f) => `${f.where}:${f.detail}`)).toEqual(["welcome/main.js#Welcome.pageThree:setShared"]);
  expect(files["main.js"]).toMatch(/if \(shared\("workspace\.panels"\) === undefined \|\| shared\("workspace\.panels"\) === null\) setShared\("workspace\.panels", \[\]\)/);
});

test("every closure handed to a callee is known to run in the frame or to wait", () => {
  // Reported as waiting; a new name here is a callee whose closure may run in the frame: say which in CALLS_NOW or KEEPS.
  expect(scanAll(root).unclassified).toEqual([]);
});
