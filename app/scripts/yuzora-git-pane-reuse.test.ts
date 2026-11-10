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
  };
  const ran = (name: string) => calls.filter((c) => c[0] === "run" && c[1] === name).map((c) => c[2]);
  return { scope, calls, store, state, ran };
}

// Two spaces, the second on screen: "one" has tab t1, "two" has tab t2.
const two = (): Space[] => [
  { id: "s1", folder: "/projects/one", tabs: [{ id: "t1", active: true }] },
  { id: "s2", folder: "/projects/two", active: true, tabs: [{ id: "t2", active: true }, { id: "t3" }] },
];
const LOG = "yuzora-git-log";

// The graph pane the app would have: the real GitLog, drawn once.
async function logPane(w: ReturnType<typeof world>, p: object) {
  const scope = {
    ...w.scope,
    props: () => p,
    open: async (cwd: string, on: string | null) => ({ id: 1, root: cwd, machine: on }),
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
  const { SidebarGit } = await load("yuzora-sidebar-git", w.scope, ["SidebarGit"]);
  const side = new SidebarGit();
  side.repo = repo;
  return side;
}

const graph = { cwd: "/projects/two", on: null };

test("the graph button in another space opens that space's own log, not the first space's", async () => {
  const w = world(two(), [{ pane: "p1", mod: LOG, tab: "t1" }]);
  (await sidebar(w, { root: "/projects/two", machine: null })).openLog();
  expect(w.ran("pane.focus")).toEqual([]);
  expect(w.ran("tab.new")).toEqual([{ name: "log.tab", space: "s2", mod: LOG, props: graph }]);
});

test("pressed again in the same space, the graph button focuses the log open there", async () => {
  const w = world(two(), [{ pane: "p7", mod: LOG, tab: "t3" }]);
  const pane = await logPane(w, graph);
  w.state.spaces[1].tabs = [{ id: "t2" }, { id: "t3", active: true }];
  pane.view.render(pane.cx);
  await pane.settle();
  (await sidebar(w, { root: "/projects/two", machine: null })).openLog();
  expect(w.ran("pane.focus")).toEqual([{ pane: "p7" }]);
  expect(w.ran("tab.new")).toEqual([]);
});

test("each space keeps its own graph, even for the same repo", async () => {
  const w = world(two(), [{ pane: "p1", mod: LOG, tab: "t1" }, { pane: "p2", mod: LOG, tab: "t2" }]);
  w.state.spaces[0].active = true; w.state.spaces[1].active = false;
  const first = await logPane(w, graph);
  first.view.render(first.cx);
  w.state.spaces[0].active = false; w.state.spaces[1].active = true;
  const second = await logPane(w, graph);
  second.view.render(second.cx);
  await Promise.all([first.settle(), second.settle()]);
  const side = await sidebar(w, { root: "/projects/two", machine: null });
  side.openLog();
  expect(w.ran("pane.focus")).toEqual([{ pane: "p2" }]);
  w.state.spaces[0].active = true; w.state.spaces[1].active = false;
  side.openLog();
  expect(w.ran("pane.focus")).toEqual([{ pane: "p2" }, { pane: "p1" }]);
});

test("when the space's repo changes, the graph button opens that repo's log, not the old one", async () => {
  const w = world(two(), [{ pane: "p7", mod: LOG, tab: "t2" }]);
  const pane = await logPane(w, { cwd: "/projects/two", on: null });
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
  (await sidebar(w, { root: "/projects/two", machine: null })).openLog();
  expect(w.ran("pane.focus")).toEqual([]);
  expect(w.ran("tab.new")).toHaveLength(1);
});

test("a log that is gone is not focused", async () => {
  const w = world(two(), [{ pane: "p7", mod: LOG, tab: "t2" }]);
  const pane = await logPane(w, graph);
  pane.view.render(pane.cx);
  await pane.settle();
  w.state.panes = [];
  (await sidebar(w, { root: "/projects/two", machine: null })).openLog();
  expect(w.ran("pane.focus")).toEqual([]);
  expect(w.ran("tab.new")).toHaveLength(1);
});

test("closing the graph lets go of its place, and only its own", async () => {
  const w = world(two(), [{ pane: "p7", mod: LOG, tab: "t2" }]);
  const first = await logPane(w, graph);
  first.view.render(first.cx);
  await first.settle();
  w.state.spaces[1].tabs = [{ id: "t2" }, { id: "t3", active: true }];
  const second = await logPane(w, graph);
  second.view.render(second.cx);
  await second.settle();
  await first.view.close();
  w.state.panes = [{ pane: "p9", mod: LOG, tab: "t3" }];
  const side = await sidebar(w, { root: "/projects/two", machine: null });
  side.openLog();
  expect(w.ran("pane.focus")).toEqual([{ pane: "p9" }]);
  await second.view.close();
  side.openLog();
  expect(w.ran("tab.new")).toHaveLength(1);
});

test("a log with no repo in its props takes the folder of the space it is first drawn in", async () => {
  const w = world(two(), []);
  // The app switches to space two, runs init, and only then publishes it.
  w.state.spaces[0].active = true;
  w.state.spaces[1].active = false;
  const pane = await logPane(w, {});
  const asked: unknown[][] = [];
  pane.view.begin = (_cx: unknown, cwd: string) => asked.push([cwd, pane.view.on]);
  w.state.spaces[0].active = false;
  w.state.spaces[1].active = true;
  pane.view.render(pane.cx);
  expect(asked).toEqual([["/projects/two", null]]);
});

test("a log with no repo asks the shown space's machine, and claims the repo it opened", async () => {
  const w = world([{ id: "m/s1", folder: "/srv/app", machine: "m", active: true, tabs: [{ id: "m/t1", active: true }] }], [{ pane: "m/p1", mod: LOG, tab: "m/t1" }]);
  const pane = await logPane(w, {});
  pane.view.render(pane.cx);
  await pane.settle();
  pane.view.render(pane.cx);
  await pane.settle();
  (await sidebar(w, { root: "/srv/app", machine: "m" })).openLog();
  expect(w.ran("pane.focus")).toEqual([{ pane: "m/p1" }]);
});

// merge and rebase: one pane per space that follows a request.
test("merge: another space's pane is left alone, the shown space's is asked and focused", async () => {
  const w = world(two(), [{ pane: "p1", mod: "yuzora-git-merge", tab: "t1" }]);
  const side = await sidebar(w, { root: "/projects/two", machine: null });
  side.openMerge("a.txt");
  expect(w.ran("pane.focus")).toEqual([]);
  expect(w.store.has("yuzora.git.merge")).toBe(false);
  expect(w.ran("tab.new")).toEqual([{ name: "merge.tab", space: "s2", mod: "yuzora-git-merge", props: expect.objectContaining({ cwd: "/projects/two", path: "a.txt", space: "s2" }) }]);
  w.state.panes = [{ pane: "p1", mod: "yuzora-git-merge", tab: "t1" }, { pane: "p2", mod: "yuzora-git-merge", tab: "t3" }];
  side.openMerge("b.txt");
  expect(w.ran("pane.focus")).toEqual([{ pane: "p2" }]);
  expect(w.store.get("yuzora.git.merge")).toMatchObject({ cwd: "/projects/two", path: "b.txt", space: "s2" });
});

test("the log, the branch list and the rebase editor open merge in the shown space too", async () => {
  const elsewhere: Pane[] = [{ pane: "p1", mod: "yuzora-git-merge", tab: "t1" }, { pane: "p5", mod: "yuzora-git-rebase", tab: "t1" }];
  const tabs = (mod: string) => [{ pane: "p1", mod: "yuzora-git-merge", tab: "t1" }, { pane: "p3", mod, tab: "t3" }];
  const repo = { root: "/projects/two", machine: null };

  const log = world(two(), elsewhere);
  const { openMerge } = await load(LOG, log.scope, ["openMerge"]);
  openMerge(repo);
  expect(log.ran("pane.focus")).toEqual([]);
  expect(log.ran("tab.new")).toHaveLength(1);
  log.state.panes = tabs("yuzora-git-merge");
  openMerge(repo);
  expect(log.ran("pane.focus")).toEqual([{ pane: "p3" }]);

  const menu = world(two(), elsewhere);
  const { GitLog } = await load(LOG, { ...menu.scope, props: () => ({ overlay: "o1" }) }, ["GitLog"]);
  const popup = new GitLog();
  popup.p = { overlay: "o1" };
  popup.m = { cwd: "/projects/two", on: null, hash: "abc" };
  popup.choose({ id: "rebase" }, {});
  expect(menu.ran("pane.focus")).toEqual([]);
  expect(menu.ran("tab.new")).toEqual([{ name: "tab.rebase", space: "s2", mod: "yuzora-git-rebase", props: expect.objectContaining({ base: "abc^", space: "s2" }) }]);
  menu.state.panes = tabs("yuzora-git-rebase");
  popup.choose({ id: "rebase" }, {});
  expect(menu.ran("pane.focus")).toEqual([{ pane: "p3" }]);

  const list = world(two(), elsewhere);
  const { GitBranches } = await load("yuzora-git-branches", list.scope, ["GitBranches"]);
  const branches = new GitBranches();
  branches.repo = repo;
  branches.close = () => {};
  const later: Promise<unknown>[] = [];
  const cx: any = { notify() {}, spawn: (f: (cx: unknown) => Promise<unknown>) => { const r = f(cx); later.push(r); return r; } };
  branches.act(cx, "merge", async () => ({ conflicts: [{ path: "a" }] }));
  await Promise.all(later);
  expect(list.ran("pane.focus")).toEqual([]);
  expect(list.ran("tab.new")).toEqual([{ name: "conflicts", space: "s2", mod: "yuzora-git-merge", props: expect.objectContaining({ cwd: "/projects/two", space: "s2" }) }]);
  list.state.panes = tabs("yuzora-git-merge");
  branches.act(cx, "merge", async () => ({ conflicts: [{ path: "a" }] }));
  await Promise.all(later);
  expect(list.ran("pane.focus")).toEqual([{ pane: "p3" }]);

  const todo = world(two(), elsewhere);
  const { GitRebase } = await load("yuzora-git-rebase", todo.scope, ["GitRebase"]);
  const editor = new GitRebase();
  editor.repo = repo;
  editor.openMerge(null);
  expect(todo.ran("pane.focus")).toEqual([]);
  expect(todo.ran("tab.new")).toEqual([{ name: "merge.tab", space: "s2", mod: "yuzora-git-merge", props: expect.objectContaining({ cwd: "/projects/two", space: "s2" }) }]);
  todo.state.panes = tabs("yuzora-git-merge");
  editor.openMerge(null);
  expect(todo.ran("pane.focus")).toEqual([{ pane: "p3" }]);
});

// diff: four openers carry the same showDiff.
const DIFF_OPENERS = ["yuzora-sidebar-git", "yuzora-git-log", "yuzora-git-blame", "yuzora-git-repo"];
const spec = { cwd: "/projects/two", on: null, path: "a.txt", orig: null, mode: "unstaged", at: 1 };

for (const mod of DIFF_OPENERS) {
  test(`${mod}: a diff pane in another space is left alone, the shown space's follows the request`, async () => {
    const w = world(two(), [{ pane: "p1", mod: "yuzora-git-diff", tab: "t1" }]);
    const { showDiff } = await load(mod, w.scope, ["showDiff"]);
    showDiff(spec);
    expect(w.ran("pane.focus")).toEqual([]);
    expect(w.store.has("yuzora.git.diff")).toBe(false);
    expect(w.calls).toEqual([["openOverlay", expect.objectContaining({ mod: "yuzora-git-diff", kind: "dialog", props: spec })]]);
    w.state.panes = [{ pane: "p1", mod: "yuzora-git-diff", tab: "t1" }, { pane: "p2", mod: "yuzora-git-diff", tab: "t3" }];
    showDiff(spec);
    expect(w.ran("pane.focus")).toEqual([{ pane: "p2" }]);
    expect(w.store.get("yuzora.git.diff")).toEqual({ ...spec, space: "s2" });
  });
}

test("a pane follows a request for its own space, or for none, and leaves another space's alone", async () => {
  const props = { cwd: "/projects/two", on: null, path: "mine.txt", mode: "unstaged", at: 10 };
  const request = (extra: object) => ({ ...props, path: "next.txt", at: 20, ...extra });
  for (const [space, follows] of [["s2", true], [undefined, true], ["s1", false]] as const) {
    const w = world(two(), []);
    w.store.set("yuzora.git.diff", request({ space }));
    const { GitDiff } = await load("yuzora-git-diff", { ...w.scope, props: () => props }, ["GitDiff"]);
    const view = new GitDiff();
    view.born = 0;
    expect(view.wanted().path, `yuzora-git-diff for ${space}`).toBe(follows ? "next.txt" : "mine.txt");
  }
  for (const [mod, name, follow] of [["yuzora-git-merge", "GitMerge", "start"], ["yuzora-git-rebase", "GitRebase", "begin"]] as const) {
    for (const [space, follows] of [["s2", true], [undefined, true], ["s1", false]] as const) {
      const w = world(two(), []);
      const key = `yuzora.git.${mod.slice("yuzora-git-".length)}`;
      w.store.set(key, { cwd: "/projects/one", on: null, at: 20, space });
      const loaded = await load(mod, w.scope, [name]);
      const view = new loaded[name]();
      const asked: unknown[] = [];
      view.at = 10;
      view[follow] = (sh: unknown) => asked.push(sh);
      try { view.render({}); } catch {}
      expect(asked, `${mod} for ${space}`).toHaveLength(follows ? 1 : 0);
    }
  }
});

// What the openers share is written into each mod that needs it (a mod cannot import another's).
const MODS_WITH_HELPERS = ["yuzora-sidebar-git", "yuzora-git-log", "yuzora-git-blame", "yuzora-git-repo", "yuzora-git-branches", "yuzora-git-rebase", "yuzora-git-merge", "yuzora-git-diff"];
const HELPERS = ["shownSpace", "findPane", "forShown", "openRequested", "logKey", "findLog", "claimLog", "releaseLog"];
const definition = (src: string, name: string) => src.match(new RegExp(`\\nfunction ${name}\\([\\s\\S]*?\\n}\\n`))?.[0];
const calls = (src: string, name: string) => new RegExp(`(?<![\\w$.])${name}\\(`).test(src.replace(new RegExp(`\\nfunction ${name}\\(`), ""));

test("a pane helper is the same wherever it is written, and every mod that calls one has it", async () => {
  const sources = Object.fromEntries(await Promise.all(MODS_WITH_HELPERS.map(async (m) => [m, (await read(m))!])));
  const written = new Map<string, Set<string>>();
  for (const name of HELPERS) {
    const copies = MODS_WITH_HELPERS.map((m) => [m, definition(sources[m], name)] as const).filter(([, d]) => d);
    expect(copies.length, `${name} is written nowhere`).toBeGreaterThan(0);
    written.set(name, new Set(copies.map(([m]) => m)));
    for (const [m, d] of copies) expect(d, `${name} in ${m}`).toBe(copies[0][1]);
  }
  expect(new Set(MODS_WITH_HELPERS.map((m) => sources[m].match(/const LOGS = .*;/)?.[0]).filter(Boolean)).size).toBe(1);
  for (const m of MODS_WITH_HELPERS) {
    for (const name of HELPERS) if (calls(sources[m], name)) expect(written.get(name)!.has(m), `${m} calls ${name} without having it`).toBe(true);
  }
});

test("no Git mod looks a pane up by its mod without checking the tab it is in", async () => {
  const dirs = (await readdir(MODS, { withFileTypes: true })).filter((d) => d.isDirectory() && /^yuzora-(git|sidebar-git)/.test(d.name)).map((d) => d.name);
  expect(dirs.length).toBeGreaterThan(5);
  for (const d of dirs) {
    const src = (await read(d))!;
    for (const lookup of src.match(/panes\(\)\s*\.\s*(?:find|some|filter)\(\s*\(?\s*\w+\s*\)?\s*=>[^;\n]*\.mod\b[^;\n]*/g) ?? []) expect(lookup, d).toMatch(/\.tab\b/);
  }
});
