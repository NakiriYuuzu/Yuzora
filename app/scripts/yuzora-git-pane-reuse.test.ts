// yuzora(L2): which Git pane an opener reuses, and which requests a pane follows
// (defaults/mods/yuzora-git-*, yuzora-sidebar-git).
// Run: bun test scripts/yuzora-git-pane-reuse.test.ts
import { test, expect } from "bun:test";
import { readFile, readdir } from "node:fs/promises";

type Tab = { id: string; active?: boolean };
type Space = { id: string; active?: boolean; folder?: string; machine?: string | null; tabs: Tab[] };
type Pane = { pane: string; mod: string; tab: string };

const MODS = new URL("../defaults/mods/", import.meta.url);
const read = (mod: string, file = "main.js") => readFile(new URL(`${mod}/${file}`, MODS), "utf8").catch(() => null);
const strip = (src: string) => src.replace(/^import [\s\S]*?;\n/gm, "").replace(/^export default class/m, "class").replace(/^export (?=function|const|class)/gm, "");

// A mod's own source, run with herdr and the host modules stubbed.
async function load(mod: string, scope: Record<string, unknown>, take: string[]) {
  return new Function(...Object.keys(scope), `${strip((await read(mod))!)}\nreturn { ${take.join(", ")} };`)(...Object.values(scope));
}

// Anything gpui-kit hands a view: every call and every property is the same inert builder.
const chain: any = new Proxy(function () {}, { get: (_t, k) => (k === Symbol.toPrimitive ? () => "" : chain), apply: () => chain });

// The run the app is in, and the run an earlier app left its shared.json from.
const SESSION = "4242-2000";
const EARLIER = "4100-1000";
const RUN = "yuzora.git.run";
const PLAIN_KEYS = ["yuzora.git.diff", "yuzora.git.merge", "yuzora.git.rebase"];

function world(spaces: Space[], panes: Pane[], settings: object = {}) {
  const calls: unknown[][] = [];
  const store = new Map<string, unknown>();
  const state = { spaces, panes };
  const scope = {
    View: class {},
    spaces: () => state.spaces.map((s) => ({ folder: "", machine: null, active: false, ...s })),
    panes: () => state.panes,
    focused: () => {
      const here = state.spaces.find((x) => x.active);
      return here ? { space: here.id, tab: here.tabs.find((x) => x.active)?.id ?? null, pane: null } : null;
    },
    shared: (k: string) => store.get(k) ?? null,
    setShared: (k: string, v: unknown) => { store.set(k, v); calls.push(["setShared", k, v]); },
    run: (name: string, args: unknown) => calls.push(["run", name, args]),
    openOverlay: (spec: unknown) => calls.push(["openOverlay", spec]),
    closeOverlay: (id: unknown) => calls.push(["closeOverlay", id]),
    gitSettings: () => ({ diffOpenIn: "modal", diffKeepIn: "split", diffFollowPane: true, ...settings }),
    t: (k: string) => k,
    session: () => SESSION,
  };
  const ran = (name: string) => calls.filter((c) => c[0] === "run" && c[1] === name).map((c) => c[2]);
  const show = (id: string) => state.spaces.forEach((s) => { s.active = s.id === id; });
  return { scope, calls, store, state, ran, show };
}

// Two spaces, the second on screen: "one" has tab t1, "two" has tab t2.
const two = (): Space[] => [
  { id: "s1", folder: "/projects/one", tabs: [{ id: "t1", active: true }] },
  { id: "s2", folder: "/projects/two", active: true, tabs: [{ id: "t2", active: true }, { id: "t3" }] },
];
const LOG = "yuzora-git-log";
const LOGS = "yuzora.git.logs";

// A pane of every Git mod in a tab of the space that is not shown: an opener that looks one up
// without checking its tab focuses it, whatever mod it is after.
const PANE_MODS = [LOG, "yuzora-git-diff", "yuzora-git-merge", "yuzora-git-rebase", "yuzora-git-blame", "yuzora-git-repo"];
const elsewhere = (): Pane[] => PANE_MODS.map((mod, i) => ({ pane: `x${i}`, mod, tab: "t1" }));

// The graph pane the app would have: the real GitLog, drawn once.
// `gate` holds `open` back until it resolves: a repo takes a while to open, so the first draw can come first.
async function logPane(w: ReturnType<typeof world>, p: object, { rootOf = (cwd: string) => cwd, gate = Promise.resolve() }: { rootOf?: (cwd: string) => string; gate?: Promise<unknown> } = {}) {
  const scope = {
    ...w.scope,
    props: () => p,
    open: async (cwd: string, on: string | null) => (await gate, { id: 1, root: rootOf(cwd), machine: on }),
    InputState: { new: () => ({ on() {} }) },
    VirtualListScrollHandle: { new: () => ({}) },
    getSetting: () => null,
  };
  const { GitLog } = await load(LOG, scope, ["GitLog"]);
  const later: Promise<unknown>[] = [];
  const cx: any = { focus_handle: () => ({}), notify() {}, spawn: (f: (cx: unknown) => Promise<unknown>) => { const r = f(cx); later.push(r); return r; } };
  const view = new GitLog();
  view.openLog = () => {};
  view.renderLog = () => null;
  view.init(p, cx);
  return { view, cx, settle: () => Promise.all(later) };
}

async function sidebar(w: ReturnType<typeof world>, repo: { root: string; machine: string | null }) {
  const { SidebarGit, repoView } = await load("yuzora-sidebar-git", w.scope, ["SidebarGit", "repoView"]);
  const side = new SidebarGit();
  side.v = repoView(repo);
  return side;
}

const graph = { cwd: "/projects/two", on: null };
const two2 = { root: "/projects/two", machine: null };

test("the graph button in another space opens that space's own log, not the first space's", async () => {
  const w = world(two(), elsewhere());
  (await sidebar(w, two2)).openLog();
  expect(w.ran("pane.focus")).toEqual([]);
  expect(w.ran("tab.new")).toEqual([{ name: "log.tab", space: "s2", mod: LOG, props: graph }]);
});

test("pressed again in the same space, the graph button focuses the log open there", async () => {
  const w = world(two(), [...elsewhere(), { pane: "p7", mod: LOG, tab: "t3" }]);
  const pane = await logPane(w, graph);
  w.state.spaces[1].tabs = [{ id: "t2" }, { id: "t3", active: true }];
  pane.view.render(pane.cx);
  await pane.settle();
  (await sidebar(w, two2)).openLog();
  expect(w.ran("pane.focus")).toEqual([{ pane: "p7" }]);
  expect(w.ran("tab.new")).toEqual([]);
});

test("each space keeps its own graph, even for the same repo", async () => {
  const w = world(two(), [{ pane: "p1", mod: LOG, tab: "t1" }, { pane: "p2", mod: LOG, tab: "t2" }]);
  w.show("s1");
  const first = await logPane(w, graph);
  first.view.render(first.cx);
  w.show("s2");
  const second = await logPane(w, graph);
  second.view.render(second.cx);
  await Promise.all([first.settle(), second.settle()]);
  const side = await sidebar(w, two2);
  side.openLog();
  expect(w.ran("pane.focus")).toEqual([{ pane: "p2" }]);
  w.show("s1");
  side.openLog();
  expect(w.ran("pane.focus")).toEqual([{ pane: "p2" }, { pane: "p1" }]);
});

test("when the space's repo changes, the graph button opens that repo's log, not the old one", async () => {
  const w = world(two(), [{ pane: "p7", mod: LOG, tab: "t2" }]);
  const pane = await logPane(w, graph);
  pane.view.render(pane.cx);
  await pane.settle();
  (await sidebar(w, { root: "/projects/two/vendor/lib", machine: null })).openLog();
  expect(w.ran("pane.focus")).toEqual([]);
  expect(w.ran("tab.new")).toEqual([{ name: "log.tab", space: "s2", mod: LOG, props: { cwd: "/projects/two/vendor/lib", on: null } }]);
});

test("the same folder on another machine is another repo", async () => {
  const w = world(two(), [{ pane: "p7", mod: LOG, tab: "t2" }]);
  const pane = await logPane(w, { cwd: "/srv/app", on: "box" });
  pane.view.render(pane.cx);
  await pane.settle();
  (await sidebar(w, { root: "/srv/app", machine: null })).openLog();
  expect(w.ran("pane.focus")).toEqual([]);
  (await sidebar(w, { root: "/srv/app", machine: "box" })).openLog();
  expect(w.ran("pane.focus")).toEqual([{ pane: "p7" }]);
  (await sidebar(w, { root: "/srv/app", machine: "other" })).openLog();
  expect(w.ran("pane.focus")).toEqual([{ pane: "p7" }]);
});

test("a file history or a compare is never the branch graph", async () => {
  const w = world(two(), [{ pane: "p7", mod: LOG, tab: "t2" }, { pane: "p8", mod: LOG, tab: "t3" }]);
  for (const extra of [{ filter: { paths: ["a.txt"], follow: true } }, { compare: { a: "main", b: "dev" } }, { filter: { hashes: ["abc"] } }]) {
    const pane = await logPane(w, { ...graph, ...extra });
    pane.view.render(pane.cx);
    await pane.settle();
  }
  (await sidebar(w, two2)).openLog();
  expect(w.ran("pane.focus")).toEqual([]);
  expect(w.ran("tab.new")).toHaveLength(1);
});

test("a log that is gone is not focused", async () => {
  const w = world(two(), [{ pane: "p7", mod: LOG, tab: "t2" }]);
  const pane = await logPane(w, graph);
  pane.view.render(pane.cx);
  await pane.settle();
  w.state.panes = [];
  (await sidebar(w, two2)).openLog();
  expect(w.ran("pane.focus")).toEqual([]);
  expect(w.ran("tab.new")).toHaveLength(1);
});

test("a graph tab that is not shown after a quit and a restart is still the one the graph button focuses", async () => {
  const w = world(two(), [{ pane: "p7", mod: LOG, tab: "t3" }]);
  w.state.spaces[1].tabs = [{ id: "t2" }, { id: "t3", active: true }];
  const before = await logPane(w, graph);
  before.view.render(before.cx);
  await before.settle();
  // Quitting closes every pane that is loaded.
  await before.view.close();
  // Back again: the layout brings t3 and its pane, but t2 is the tab shown, so that pane is not loaded and says nothing.
  w.state.spaces[1].tabs = [{ id: "t2", active: true }, { id: "t3" }];
  (await sidebar(w, two2)).openLog();
  expect(w.ran("pane.focus")).toEqual([{ pane: "p7" }]);
  expect(w.ran("tab.new")).toEqual([]);
});

test("two graphs of one repo: when one is closed the other is still found", async () => {
  const w = world(two(), [{ pane: "p7", mod: LOG, tab: "t2" }, { pane: "p8", mod: LOG, tab: "t3" }]);
  const first = await logPane(w, graph);
  first.view.render(first.cx);
  await first.settle();
  w.state.spaces[1].tabs = [{ id: "t2" }, { id: "t3", active: true }];
  const second = await logPane(w, graph);
  second.view.render(second.cx);
  await second.settle();
  await second.view.close();
  w.state.panes = [{ pane: "p7", mod: LOG, tab: "t2" }];
  (await sidebar(w, two2)).openLog();
  expect(w.ran("pane.focus")).toEqual([{ pane: "p7" }]);
  expect(w.ran("tab.new")).toEqual([]);
});

test("a graph opened on a subfolder, or with a trailing slash, is the graph of the repo root", async () => {
  const w = world(two(), [{ pane: "p7", mod: LOG, tab: "t2" }]);
  const pane = await logPane(w, { cwd: "/projects/two/src/", on: null }, { rootOf: () => "/projects/two" });
  pane.view.render(pane.cx);
  await pane.settle();
  (await sidebar(w, two2)).openLog();
  expect(w.ran("pane.focus")).toEqual([{ pane: "p7" }]);
  expect(w.ran("tab.new")).toEqual([]);
});

test("a graph says which repo it shows when its repo opens before it is first drawn", async () => {
  const w = world(two(), [{ pane: "p7", mod: LOG, tab: "t2" }]);
  const early = await logPane(w, graph);
  await early.settle();
  expect(w.store.has(LOGS)).toBe(false);
  early.view.render(early.cx);
  await early.settle();
  expect(w.store.get(LOGS)).toEqual({ t2: { space: "s2", root: "/projects/two", on: null } });
});

test("a graph says which repo it shows when it is first drawn before its repo opens", async () => {
  const w = world(two(), [{ pane: "p7", mod: LOG, tab: "t2" }]);
  let opened = () => {};
  const late = await logPane(w, graph, { gate: new Promise<void>((resolve) => { opened = resolve; }) });
  late.view.render(late.cx);
  await new Promise((resolve) => setTimeout(resolve, 0));
  expect(w.store.has(LOGS)).toBe(false);
  opened();
  await late.settle();
  expect(w.store.get(LOGS)).toEqual({ t2: { space: "s2", root: "/projects/two", on: null } });
});

test("a place is written once, and a write drops the places of tabs that are gone but not of a machine not listed yet", async () => {
  const w = world(two(), [{ pane: "p7", mod: LOG, tab: "t2" }]);
  const kept = { space: "s2", root: "/projects/other", on: null };
  w.store.set(LOGS, {
    old: { space: "s2", root: "/closed", on: null },
    older: { space: "s9", root: "/space/removed", on: null },
    t3: kept,
    "box/t7": { space: "box/s1", root: "/srv/app", on: "box" },
  });
  const pane = await logPane(w, graph);
  pane.view.render(pane.cx);
  await pane.settle();
  expect(w.store.get(LOGS)).toEqual({ t3: kept, "box/t7": { space: "box/s1", root: "/srv/app", on: "box" }, t2: { space: "s2", root: "/projects/two", on: null } });
  const writes = w.calls.length;
  pane.view.claim();
  expect(w.calls).toHaveLength(writes);
});

test("a log with no repo in its props takes the folder of the space it is first drawn in", async () => {
  const w = world(two(), []);
  // The app switches to space one, runs init, and only then publishes it.
  w.show("s1");
  const pane = await logPane(w, {});
  const asked: unknown[][] = [];
  pane.view.begin = (_cx: unknown, cwd: string) => asked.push([cwd, pane.view.on]);
  w.show("s2");
  pane.view.render(pane.cx);
  expect(asked).toEqual([["/projects/two", null]]);
});

test("a log with no repo asks the shown space's machine, and says which repo it opened", async () => {
  const w = world([{ id: "m/s1", folder: "/srv/app", machine: "m", active: true, tabs: [{ id: "m/t1", active: true }] }], [{ pane: "m/p1", mod: LOG, tab: "m/t1" }]);
  const pane = await logPane(w, {});
  pane.view.render(pane.cx);
  await pane.settle();
  pane.view.render(pane.cx);
  await pane.settle();
  (await sidebar(w, { root: "/srv/app", machine: "m" })).openLog();
  expect(w.ran("pane.focus")).toEqual([{ pane: "m/p1" }]);
});

// merge and rebase: one pane per space, which follows its space's request.
test("merge: another space's pane is left alone, the shown space's is asked and focused", async () => {
  const w = world(two(), elsewhere());
  const side = await sidebar(w, two2);
  side.openMerge("a.txt");
  expect(w.ran("pane.focus")).toEqual([]);
  expect(w.store.size).toBe(0);
  expect(w.ran("tab.new")).toEqual([{ name: "merge.tab", space: "s2", mod: "yuzora-git-merge", props: expect.objectContaining({ cwd: "/projects/two", path: "a.txt" }) }]);
  w.state.panes = [...elsewhere(), { pane: "p2", mod: "yuzora-git-merge", tab: "t3" }];
  side.openMerge("b.txt");
  expect(w.ran("pane.focus")).toEqual([{ pane: "p2" }]);
  expect([...w.store.keys()]).toEqual(["yuzora.git.merge.s2"]);
  expect(w.store.get("yuzora.git.merge.s2")).toMatchObject({ cwd: "/projects/two", path: "b.txt" });
});

test("the log, the branch list and the rebase editor open merge in the shown space too", async () => {
  const tabs = (mod: string) => [...elsewhere(), { pane: "p3", mod, tab: "t3" }];
  const repo = two2;

  const log = world(two(), elsewhere());
  const { openMerge } = await load(LOG, log.scope, ["openMerge"]);
  openMerge(repo);
  expect(log.ran("pane.focus")).toEqual([]);
  expect(log.ran("tab.new")).toHaveLength(1);
  log.state.panes = tabs("yuzora-git-merge");
  openMerge(repo);
  expect(log.ran("pane.focus")).toEqual([{ pane: "p3" }]);

  const menu = world(two(), elsewhere());
  const { GitLog } = await load(LOG, { ...menu.scope, props: () => ({ overlay: "o1" }) }, ["GitLog"]);
  const popup = new GitLog();
  popup.p = { overlay: "o1" };
  popup.m = { cwd: "/projects/two", on: null, hash: "abc" };
  popup.choose({ id: "rebase" }, {});
  expect(menu.ran("pane.focus")).toEqual([]);
  expect(menu.ran("tab.new")).toEqual([{ name: "tab.rebase", space: "s2", mod: "yuzora-git-rebase", props: expect.objectContaining({ base: "abc^" }) }]);
  menu.state.panes = tabs("yuzora-git-rebase");
  popup.choose({ id: "rebase" }, {});
  expect(menu.ran("pane.focus")).toEqual([{ pane: "p3" }]);
  expect([...menu.store.keys()]).toEqual(["yuzora.git.rebase.s2"]);

  const list = world(two(), elsewhere());
  const { GitBranches } = await load("yuzora-git-branches", list.scope, ["GitBranches"]);
  const branches = new GitBranches();
  branches.repo = repo;
  branches.close = () => {};
  const later: Promise<unknown>[] = [];
  const cx: any = { notify() {}, spawn: (f: (cx: unknown) => Promise<unknown>) => { const r = f(cx); later.push(r); return r; } };
  branches.act(cx, "merge", async () => ({ conflicts: [{ path: "a" }] }));
  await Promise.all(later);
  expect(list.ran("pane.focus")).toEqual([]);
  expect(list.ran("tab.new")).toEqual([{ name: "conflicts", space: "s2", mod: "yuzora-git-merge", props: expect.objectContaining({ cwd: "/projects/two" }) }]);
  list.state.panes = tabs("yuzora-git-merge");
  branches.act(cx, "merge", async () => ({ conflicts: [{ path: "a" }] }));
  await Promise.all(later);
  expect(list.ran("pane.focus")).toEqual([{ pane: "p3" }]);
  expect([...list.store.keys()]).toEqual(["yuzora.git.merge.s2"]);

  const todo = world(two(), elsewhere());
  const { GitRebase } = await load("yuzora-git-rebase", todo.scope, ["GitRebase"]);
  const editor = new GitRebase();
  editor.repo = repo;
  editor.openMerge(null);
  expect(todo.ran("pane.focus")).toEqual([]);
  expect(todo.ran("tab.new")).toEqual([{ name: "merge.tab", space: "s2", mod: "yuzora-git-merge", props: expect.objectContaining({ cwd: "/projects/two" }) }]);
  todo.state.panes = tabs("yuzora-git-merge");
  editor.openMerge(null);
  expect(todo.ran("pane.focus")).toEqual([{ pane: "p3" }]);
  expect([...todo.store.keys()]).toEqual(["yuzora.git.merge.s2"]);
});

// diff: four openers carry the same showDiff.
const DIFF_OPENERS = ["yuzora-sidebar-git", "yuzora-git-log", "yuzora-git-blame", "yuzora-git-repo"];
const DIFF = "yuzora-git-diff";
const spec = { cwd: "/projects/two", on: null, path: "a.txt", orig: null, mode: "unstaged", at: 1 };

for (const mod of DIFF_OPENERS) {
  test(`${mod}: a diff pane in another space is left alone, the shown space's follows the request`, async () => {
    const w = world(two(), elsewhere());
    const { showDiff } = await load(mod, w.scope, ["showDiff"]);
    showDiff(spec);
    expect(w.ran("pane.focus")).toEqual([]);
    expect(w.store.size).toBe(0);
    expect(w.calls).toEqual([["openOverlay", expect.objectContaining({ mod: DIFF, kind: "dialog", props: spec })]]);
    w.state.panes = [...elsewhere(), { pane: "p2", mod: DIFF, tab: "t3" }];
    showDiff(spec);
    expect(w.ran("pane.focus")).toEqual([{ pane: "p2" }]);
    expect([...w.store.keys()]).toEqual(["yuzora.git.diff.s2"]);
    expect(w.store.get("yuzora.git.diff.s2")).toEqual(spec);
  });
}

// What a diff pane shows: the real GitDiff, drawn with the view builders inert.
async function diffPane(w: ReturnType<typeof world>, p: object) {
  const scope = {
    ...w.scope,
    props: () => p,
    VirtualListScrollHandle: { new: () => ({}) },
    theme: () => new Proxy({}, { get: () => "#000" }),
    tokens: () => ({}),
    settings: () => ({}),
    div: chain, v_flex: chain, h_flex: chain, v_virtual_list: chain, Scrollbar: chain,
  };
  const { GitDiff } = await load(DIFF, scope, ["GitDiff"]);
  const cx: any = { focus_handle: () => ({}), spawn() {}, notify() {} };
  const view = new GitDiff();
  view.init(p, cx);
  return { view, draw: () => view.render(cx) };
}

const bigTwo = (): Space[] => [
  { id: "s1", folder: "/projects/one", active: true, tabs: [{ id: "t1", active: true }] },
  { id: "s2", folder: "/projects/two", tabs: [{ id: "t2", active: true }] },
];

test("a space's diff pane keeps what its own space asked for while another space asks for its own", async () => {
  const w = world(bigTwo(), [{ pane: "pa", mod: DIFF, tab: "t1" }, { pane: "pb", mod: DIFF, tab: "t2" }]);
  const { showDiff } = await load("yuzora-sidebar-git", w.scope, ["showDiff"]);
  const kept = { cwd: "/projects/one", on: null, path: "kept.txt", mode: "unstaged", at: 10 };
  const a = await diffPane(w, kept);
  showDiff({ cwd: "/projects/one", on: null, path: "a-file.txt", orig: null, mode: "unstaged", at: 20 });
  a.draw();
  expect(a.view.spec.path).toBe("a-file.txt");
  a.view.sel = { 0: { del: new Set(), add: new Set([3]) } };
  a.view.hunk = 2;
  w.show("s2");
  showDiff({ cwd: "/projects/two", on: null, path: "b-file.txt", orig: null, mode: "unstaged", at: 30 });
  w.show("s1");
  a.draw();
  expect(a.view.spec.path).toBe("a-file.txt");
  expect(a.view.picked()).toEqual({ 0: { del: [], add: [3] } });
  expect(a.view.hunk).toBe(2);
});

test("the diff pane follows its own space's request or a plain one, the newer of the two, and leaves another space's alone", async () => {
  const props = { cwd: "/projects/two", on: null, path: "mine.txt", mode: "unstaged", at: 10 };
  const next = (path: string, at: number) => ({ ...props, path, at });
  const wanted = async (put: [string, object][]) => {
    const w = world(two(), []);
    w.store.set(RUN, SESSION);
    for (const [k, v] of put) w.store.set(k, v);
    const { view } = await diffPane(w, props);
    view.born = 0;
    return view.wanted().path;
  };
  expect(await wanted([["yuzora.git.diff.s2", next("own.txt", 20)]])).toBe("own.txt");
  expect(await wanted([["yuzora.git.diff", next("plain.txt", 20)]])).toBe("plain.txt");
  expect(await wanted([["yuzora.git.diff.s1", next("elsewhere.txt", 20)]])).toBe("mine.txt");
  expect(await wanted([["yuzora.git.diff.s2", next("own.txt", 20)], ["yuzora.git.diff", next("plain.txt", 30)]])).toBe("plain.txt");
  expect(await wanted([["yuzora.git.diff.s2", next("own.txt", 40)], ["yuzora.git.diff", next("plain.txt", 30)]])).toBe("own.txt");
});

// What merge and rebase pane build at init, the call that starts a place, and the real opener
// that asks for one: the sidebar's merge, the log menu's rebase.
const FOLLOWERS = [["yuzora-git-merge", "GitMerge", "start"], ["yuzora-git-rebase", "GitRebase", "begin"]] as const;

async function follower(w: ReturnType<typeof world>, mod: string, name: string, start: string, p: object) {
  const stub = { new: () => ({ on() {}, set_auto_grow() {}, set_value() {}, release() {} }) };
  const scope = { ...w.scope, props: () => p, TextareaState: stub, InputState: stub, VirtualListScrollHandle: { new: () => ({ release() {} }) } };
  const loaded = await load(mod, scope, [name]);
  const asked: any[] = [];
  loaded[name].prototype[start] = function (spec: unknown) { asked.push(spec); this.spec = spec; this.at = (spec as any).at ?? 0; };
  const view = new loaded[name]();
  view.init(p, { focus_handle: () => ({}) });
  return { asked, draw: () => { try { view.render({}); } catch {} } };
}

async function askFromShownSpace(w: ReturnType<typeof world>, mod: string, repo: string) {
  if (mod === "yuzora-git-merge") return (await sidebar(w, { root: repo, machine: null })).openMerge("a.txt");
  const { GitLog } = await load(LOG, { ...w.scope, props: () => ({ overlay: "o1" }) }, ["GitLog"]);
  const popup = new GitLog();
  popup.p = { overlay: "o1" };
  popup.m = { cwd: repo, on: null, hash: "abc" };
  popup.choose({ id: "rebase" }, {});
}

for (const [mod, name, start] of FOLLOWERS) {
  const KEY = `yuzora.git.${mod.slice("yuzora-git-".length)}`;

  test(`${mod}: a pane added in a space does not take the request another space made`, async () => {
    const w = world(two(), [{ pane: "p1", mod, tab: "t1" }]);
    w.show("s1");
    await askFromShownSpace(w, mod, "/projects/one");
    expect(w.store.size).toBe(1);
    w.show("s2");
    const pane = await follower(w, mod, name, start, {});
    pane.draw();
    expect(pane.asked).toEqual([]);
  });

  test(`${mod}: a pane with no props takes its own space's last request, from the first time it is drawn`, async () => {
    const w = world(two(), []);
    w.store.set(`${KEY}.s1`, { cwd: "/projects/one", on: null, at: 20 });
    w.store.set(`${KEY}.s2`, { cwd: "/projects/two", on: null, at: 10 });
    const pane = await follower(w, mod, name, start, {});
    expect(pane.asked).toEqual([]);
    pane.draw();
    expect(pane.asked).toEqual([{ cwd: "/projects/two", on: null, at: 10 }]);
  });

  test(`${mod}: a pane with no props takes a plain request, an agent's, and the newer of that and its space's`, async () => {
    const w = world(two(), []);
    w.store.set(RUN, SESSION);
    w.store.set(KEY, { cwd: "/agent" });
    const pane = await follower(w, mod, name, start, {});
    pane.draw();
    expect(pane.asked).toEqual([{ cwd: "/agent" }]);

    const both = world(two(), []);
    both.store.set(RUN, SESSION);
    both.store.set(KEY, { cwd: "/agent", at: 30 });
    both.store.set(`${KEY}.s2`, { cwd: "/projects/two", at: 20 });
    const newer = await follower(both, mod, name, start, {});
    newer.draw();
    expect(newer.asked).toEqual([{ cwd: "/agent", at: 30 }]);
  });

  test(`${mod}: a pane opened with a place keeps it unless its space asks for a newer one`, async () => {
    const props = { cwd: "/projects/two", on: null, at: 10 };
    const w = world(two(), []);
    const pane = await follower(w, mod, name, start, props);
    expect(pane.asked).toEqual([props]);
    w.store.set(`${KEY}.s2`, { cwd: "/projects/two", on: null, at: 5 });
    w.store.set(`${KEY}.s1`, { cwd: "/projects/one", on: null, at: 50 });
    pane.draw();
    expect(pane.asked).toHaveLength(1);
    w.store.set(`${KEY}.s2`, { cwd: "/projects/two", on: null, at: 20 });
    pane.draw();
    expect(pane.asked).toHaveLength(2);
  });
}

// shared.json outlives the app, and a plain request is an order given during one run, so what an
// earlier run left in it (beta.4 wrote every request there, and a pane of a new space was
// shown it) is not followed by any pane.
for (const [mod, name, start] of FOLLOWERS) {
  const KEY = `yuzora.git.${mod.slice("yuzora-git-".length)}`;
  const left = { cwd: "/projects/one", on: null, path: "a.txt", at: 5 };

  test(`${mod}: a plain request an earlier run left is not taken by a pane in a space that has not asked, nor by one in a new space`, async () => {
    const w = world(two(), []);
    w.store.set(KEY, left);
    const pane = await follower(w, mod, name, start, {});
    pane.draw();
    expect(pane.asked).toEqual([]);
    expect(w.store.get(KEY) ?? null).toBeNull();
    w.state.spaces.push({ id: "s3", folder: "/projects/three", tabs: [{ id: "t4", active: true }] });
    w.show("s3");
    const later = await follower(w, mod, name, start, {});
    later.draw();
    expect(later.asked).toEqual([]);
  });

  test(`${mod}: a plain request written in this run is taken, dated or not, by a pane that was drawn before it`, async () => {
    const w = world(two(), []);
    w.store.set(KEY, left);
    const pane = await follower(w, mod, name, start, {});
    pane.draw();
    expect(pane.asked).toEqual([]);
    w.store.set(KEY, { cwd: "/agent", at: Date.now() });
    pane.draw();
    expect(pane.asked).toEqual([{ cwd: "/agent", at: expect.any(Number) }]);
    const plain = world(two(), []);
    const other = await follower(plain, mod, name, start, {});
    other.draw();
    plain.store.set(KEY, { cwd: "/agent" });
    other.draw();
    expect(other.asked).toEqual([{ cwd: "/agent" }]);
  });

  test(`${mod}: its own space's request survives a restart, and a plain one from the earlier run beside it does not`, async () => {
    const w = world(two(), []);
    w.store.set(RUN, EARLIER);
    w.store.set(KEY, { cwd: "/agent", at: 90 });
    w.store.set(`${KEY}.s2`, { cwd: "/projects/two", on: null, at: 10 });
    const pane = await follower(w, mod, name, start, {});
    pane.draw();
    expect(pane.asked).toEqual([{ cwd: "/projects/two", on: null, at: 10 }]);
  });
}

test("a diff pane is not shown the plain request an earlier run left, kept pane or a new one, and follows one written in this run", async () => {
  const left = { cwd: "/projects/one", on: null, path: "old-one.txt", mode: "unstaged", at: 20 };
  const kept = { cwd: "/projects/two", on: null, path: "kept.txt", mode: "unstaged", at: 3 };
  const w = world(two(), []);
  w.store.set(RUN, EARLIER);
  w.store.set("yuzora.git.diff", left);
  const pane = await diffPane(w, kept);
  pane.draw();
  expect(pane.view.spec.path).toBe("kept.txt");
  const fresh = await diffPane(w, {});
  fresh.draw();
  expect(fresh.view.spec).toBeNull();
  w.store.set("yuzora.git.diff", { ...kept, path: "agent.txt", at: Date.now() });
  pane.draw();
  expect(pane.view.spec.path).toBe("agent.txt");
});

test("the first read of a run retires the plain requests of all three kinds once, and nothing else", async () => {
  const w = world(two(), []);
  w.store.set(RUN, EARLIER);
  for (const k of PLAIN_KEYS) w.store.set(k, { cwd: "/left", at: 1 });
  w.store.set("yuzora.git.merge.s1", { cwd: "/projects/one", at: 2 });
  const pane = await diffPane(w, {});
  pane.draw();
  for (const k of PLAIN_KEYS) expect(w.store.get(k) ?? null, k).toBeNull();
  expect(w.store.get("yuzora.git.merge.s1")).toEqual({ cwd: "/projects/one", at: 2 });
  expect(w.store.get(RUN)).toBe(SESSION);
  const writes = w.calls.length;
  pane.draw();
  expect(w.calls.length).toBe(writes);
});

// What the openers share is written into each mod that needs it (a mod cannot import another's).
const HELPERS = ["shownSpace", "findPane", "requestKey", "retirePlain", "requested", "openRequested", "findLog", "claimLog"];
const definition = (src: string, name: string) => src.match(new RegExp(`\\nfunction ${name}\\([\\s\\S]*?\\n}\\n`))?.[0];
const calls = (src: string, name: string) => new RegExp(`(?<![\\w$.])${name}\\(`).test(src.replace(new RegExp(`\\nfunction ${name}\\(`), ""));
const gitMods = async () => (await readdir(MODS, { withFileTypes: true })).filter((d) => d.isDirectory() && /^yuzora-(git|sidebar-git)/.test(d.name)).map((d) => d.name);

test("a pane helper is the same wherever it is written, and every mod that calls one has it", async () => {
  const names = await gitMods();
  expect(names.length).toBeGreaterThan(5);
  const sources = Object.fromEntries(await Promise.all(names.map(async (m) => [m, (await read(m))!])));
  const written = new Map<string, Set<string>>();
  for (const name of HELPERS) {
    const copies = names.map((m) => [m, definition(sources[m], name)] as const).filter(([, d]) => d);
    expect(copies.length, `${name} is written nowhere`).toBeGreaterThan(0);
    written.set(name, new Set(copies.map(([m]) => m)));
    for (const [m, d] of copies) expect(d, `${name} in ${m}`).toBe(copies[0][1]);
  }
  expect(new Set(names.map((m) => sources[m].match(/const LOGS = .*;/)?.[0]).filter(Boolean)).size).toBe(1);
  for (const m of names) {
    for (const name of HELPERS) if (calls(sources[m], name)) expect(written.get(name)!.has(m), `${m} calls ${name} without having it`).toBe(true);
  }
});

// A lookup of panes by mod can be written many ways, so `panes` may only be named in the two
// functions that check the tab, and in the reads that look no pane up by its mod (by its id, or
// the folders of the panes in the shown space). Only its name is read, not how a lookup is spelt:
// a new read has to be added here once someone has checked what it looks for.
const LOOKUPS = ["findPane", "findLog"];
const OTHER_READS = [
  "panes().find((p) => p.pane === f.pane)",
  "panes().filter((p) => tabs.has(p.tab) && p.cwd).map((p) => p.cwd)",
];
// A `//` inside a string is not a comment, so strings are walked over rather than matched by a pattern.
// A quote that opens no string ends at the end of its line.
function withoutComments(src: string) {
  let out = "";
  let i = 0;
  while (i < src.length) {
    const c = src[i];
    if (c === "/" && src[i + 1] === "/") {
      while (i < src.length && src[i] !== "\n") i++;
    } else if (c === "/" && src[i + 1] === "*") {
      const end = src.indexOf("*/", i + 2);
      i = end < 0 ? src.length : end + 2;
    } else if (c === '"' || c === "'" || c === "`") {
      let j = i + 1;
      while (j < src.length && src[j] !== c && (c === "`" || src[j] !== "\n")) j += src[j] === "\\" ? 2 : 1;
      out += src.slice(i, j + 1);
      i = j + 1;
    } else out += src[i++];
  }
  return out;
}

function strayPanes(src: string) {
  let code = withoutComments(src.replace(/^import [\s\S]*?;\n/gm, ""));
  for (const name of LOOKUPS) code = code.replace(new RegExp(`\\nfunction ${name}\\([\\s\\S]*?\\n}\\n`), "\n");
  for (const read of OTHER_READS) code = code.split(read).join("");
  return [...code.matchAll(/\bpanes\b/g)].length + [...src.matchAll(/import\s*\*\s*as|\bpanes\s+as\b/g)].length;
}

test("the guard on panes() catches a lookup however it is written", () => {
  const body = (line: string) => `import { panes } from "herdr";\nfunction opener() {\n  ${line}\n}\n`;
  for (const line of [
    `const pane = panes().find((p) => p.mod === "yuzora-git-merge");`,
    `const pane = panes().find((p) =>\n    p.mod === "yuzora-git-merge");`,
    `const pane = panes().find(({ mod }) => mod === "yuzora-git-merge");`,
    `const all = panes(); const pane = all.find((p) => p.mod === "yuzora-git-merge");`,
    `const pane = panes().findLast((p) => p.mod === "yuzora-git-merge");`,
    `const pane = panes().find((p) => p.mod === "yuzora-git-merge" && focused()?.tab);`,
    `const pane = panes().filter((p) => p.mod === "yuzora-git-merge")[0];`,
    `const pane = remote.startsWith("https://") && panes().find((p) => p.mod === "yuzora-git-merge");`,
    `const url = 'http://x'; const pane = panes().find((p) => p.mod === "yuzora-git-merge");`,
    "const url = `https://${host}/`; const pane = panes().find((p) => p.mod === \"yuzora-git-merge\");",
  ]) expect(strayPanes(body(line)), line).toBeGreaterThan(0);
  expect(strayPanes('import { panes as list } from "herdr";\n')).toBeGreaterThan(0);
  expect(strayPanes('import * as herdr from "herdr";\n')).toBeGreaterThan(0);
  expect(strayPanes(`// panes() lists every space\nfunction findPane(mod, space) {\n  return panes().find((p) => p.mod === mod) ?? null;\n}\n`)).toBe(0);
  expect(strayPanes(`/* panes() */\nconst a = 1; // panes().find()\nconst b = "// not a comment";\n`)).toBe(0);
});

test("no Git mod reads panes() outside the functions that check the tab", async () => {
  for (const d of await gitMods()) expect(strayPanes((await read(d))!), d).toBe(0);
});
