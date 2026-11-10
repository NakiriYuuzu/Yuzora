// yuzora(v0.1.0 F1): the Files sidebar's own lifecycle, main.js and the modules it is made of run
// as shipped, with herdr, Yuzora and the element builders stubbed.
// Run: bun test scripts/yuzora-files-sidebar.test.ts
import { test, expect } from "bun:test";
import { readFile } from "node:fs/promises";

const dir = new URL("../defaults/mods/yuzora-sidebar-files/", import.meta.url);
const [main, tree, gitState, paths, open] = await Promise.all(["main.js", "tree-state.js", "git-state.js", "paths.js", "open.js"].map(f => readFile(new URL(f, dir), "utf8")));
const code = (s: string) => s.replace(/^import [\s\S]*?;\n/gm, "").replace(/^export \{[^}]*\} from [^;]*;\n/gm, "").replace(/^export default class/m, "class").replace(/^export (?=async function|function|const|class)/gm, "");
const source = [paths, gitState, tree, main].map(code).join("\n");
// The sidebar with open.js as shipped, over a stub Herdr that records the commands it is told to run.
const sourceWithOpen = [paths, gitState, tree, open, main].map(code).join("\n");

type Call = { args: any[]; resolve: (v: any) => void; reject: (e: any) => void; answered: boolean };
const waiting = (): [Call, Promise<any>] => {
  let resolve!: (v: any) => void, reject!: (e: any) => void;
  const promise = new Promise((a, b) => { resolve = a; reject = b; });
  return [{ args: [], resolve, reject, answered: false }, promise];
};
const flush = async () => { for (let i = 0; i < 30; i++) await Promise.resolve(); };
const entry = (name: string, dir = false) => ({ name, dir, link: false, hidden: false, size: 0 });
const listing = (...names: string[]) => ({ entries: names.map(n => entry(n.replace(/\/$/, ""), n.endsWith("/"))), truncated: false, skipped: false });
const repo = (top: string, marks = {}) => ({ top, branch: "main", detached: false, ahead: 0, behind: 0, marks, truncated: false, untrusted: false });

const two = (a: string, b: string, machine: string | null = null) => [
  { id: "s1", active: true, folder: a, machine },
  { id: "s2", active: false, folder: b, machine },
];

// A sidebar whose host is the test's: every listing, git status and stat waits to be answered. `real`: files are
// opened by open.js itself (what it runs is in `world.runs`) instead of a stub that notes the Space shown.
function sidebar(spaces: any[] = two("/a", "/b"), real = false) {
  const world = {
    spaces,
    toasts: [] as any[], opened: [] as { path: string; on: string | null; space: string }[], log: [] as string[], lists: [] as { id: string; count: number }[],
    query: "", keys: [] as string[], onClick: null as any, onSecondary: null as any, menus: [] as any[], shared: new Map<string, any>(),
    timers: [] as { ms: number; fn: () => void; fired: boolean }[],
    calls: { list: [] as Call[], git: [] as Call[], stat: [] as Call[], watch: [] as Call[], make: [] as Call[] },
    name: "", props: null as any, clicks: new Map<string, any>(), moved: [] as any[][],
    runs: [] as [string, any][], paneList: [] as any[], focus: null as any,
  };
  const active = () => world.spaces.find(s => s.active)?.id;
  const ask = (kind: "list" | "git" | "stat" | "watch" | "make", args: any[]) => {
    const [call, promise] = waiting();
    call.args = args;
    world.calls[kind].push(call);
    return promise;
  };
  let id = "";
  const chain: any = new Proxy(function () {}, {
    get: (_t, prop) => prop === "then" ? undefined : (...args: any[]) => {
      for (const a of args) if (typeof a === "string") world.log.push(a);
      if (prop === "id") id = args[0];
      if (prop === "on_click") world.clicks.set(id, args[0]);
      if (prop === "on_item_click") world.onClick = args[0];
      if (prop === "on_item_secondary_click") world.onSecondary = args[0];
      if (prop === "when") { if (args[0]) args[1](chain); }
      return chain;
    },
  });
  const maker = () => chain;
  // The engine refuses to change an input, or to ask for a redraw, while a frame is being built.
  let building = false;
  const inFrame = (what: string) => { if (building) throw new TypeError(`${what} cannot run during render or layout`); };
  const inputCalls: any[][] = [];
  // The search box is the first input made, the name being typed for a new file or a rename the second.
  const inputs: any[] = [];
  const makeInput = () => {
    const input = {
      on() {}, value: () => (inputs[0] === input ? world.query : world.name),
      set_value: (v: string) => { inputCalls.push(["set_value", v]); inFrame("set_value"); },
      focus: () => { inputCalls.push(["focus"]); inFrame("focus"); },
      set_selected_range: (range: object) => { inputCalls.push(["set_selected_range", range]); inFrame("set_selected_range"); },
    };
    inputs.push(input);
    return input;
  };
  const stubs: Record<string, any> = {
    View: class {},
    div: maker, svg: maker, v_flex: maker, h_flex: maker,
    uniform_list: (id: string, count: number, keyOf: (i: number) => string, render: (r: { start: number; end: number }) => any) => {
      world.lists.push({ id, count });
      if (id !== "results") world.keys = Array.from({ length: count }, (_, i) => keyOf(i));
      render({ start: 0, end: count });
      return chain;
    },
    InputState: { new: makeInput }, Input: { new: maker }, Scrollbar: { vertical: maker },
    spaces: () => world.spaces, panes: () => world.paneList, focused: () => world.focus, theme: () => new Proxy({}, { get: () => "#000000" }),
    run: (name: string, args: any) => { world.runs.push([name, args]); }, home: () => "/home/me", props: () => world.props, openOverlay: (o: any) => world.menus.push(o), closeOverlay() {},
    shared: (key: string) => world.shared.get(key) ?? null, setShared: (key: string, value: any) => world.shared.set(key, value), notify: (n: any) => world.toasts.push(n), watchFolder: (path: string) => ask("watch", [path]),
    window: () => ({ platform: "macos" }),
    t: (key: string, vars?: object) => (vars ? `${key} ${JSON.stringify(vars)}` : key), tokens: () => ({}),
    listDir: (path: string, options: object) => ask("list", [path, options]),
    gitStatus: (cwd: string, options: object) => ask("git", [cwd, options]),
    stat: (path: string, options: object) => ask("stat", [path, options]),
    createFile: (path: string, options: object) => ask("make", ["createFile", path, options]),
    createFolder: (path: string, options: object) => ask("make", ["createFolder", path, options]),
    rename: (from: string, to: string, options: object) => ask("make", ["rename", from, to, options]),
    trash() {}, reveal() {}, copyText() {}, findFiles() {}, grep() {}, unsavedCount: () => 0,
    openFile: (path: string, on: string | null) => world.opened.push({ path, on: on ?? null, space: active() }), moved: (...args: any[]) => world.moved.push(args),
    kindOf: (path: string) => (/\.(png|svg)$/i.test(path) ? "image" : /\.md$/i.test(path) ? "markdown" : "text"),
    documentFor: () => null, focusDocument() {}, revealLine() {},
    fileIcon: () => ({ icon: "file", color: "dim" }),
    localStorage: { getItem: () => null, setItem() {} },
  };
  if (real) {
    delete stubs.openFile; delete stubs.moved;
    world.paneList = [{ pane: "p1", tab: "t1" }];
    world.focus = { space: active(), tab: "t1", pane: "p1" };
  }
  const { FilesSidebar: View, menuItems } = new Function(...Object.keys(stubs), `${real ? sourceWithOpen : source}\nreturn { FilesSidebar, menuItems };`)(...Object.values(stubs));
  const cx: any = {
    spawn: (fn: (cx: any) => Promise<any>) => { fn(cx); }, notify() { inFrame("cx.notify()"); },
    timer: { every() {}, after: (ms: number, fn: (cx: any) => void) => { world.timers.push({ ms, fn: () => fn(cx), fired: false }); } },
  };
  const view = new View();
  view.init(null, cx);
  const pending = (kind: "list" | "git" | "stat" | "watch" | "make") => world.calls[kind].filter(c => !c.answered);
  const answer = async (call: Call, value: any) => { call.answered = true; call.resolve(value); await flush(); };
  const fail = async (call: Call, error: Error) => { call.answered = true; call.reject(error); await flush(); };
  // The frame: what it says, which lists it draws, and the key each of the tree's rows was drawn with.
  const frame = () => {
    world.log.length = 0; world.lists.length = 0;
    building = true;
    try { view.render(cx); } finally { building = false; }
    return { texts: [...world.log], lists: [...world.lists], keys: [...world.keys] };
  };
  // A click on the i-th row of a frame, as the list reports it: by the key the frame gave the row.
  const press = (shown: { keys: string[] }, i: number) => world.onClick(shown.keys[i], cx);
  const pressRight = (shown: { keys: string[] }, i: number) => world.onSecondary(shown.keys[i], {}, cx);
  // The timers a click started and nothing has fired yet.
  const fireTimers = (atLeast = 1000) => {
    for (const t of world.timers) if (!t.fired && t.ms >= atLeast) { t.fired = true; t.fn(); }
  };
  // The timers of a click's check that are due within `ms`: later ones are not yet.
  const fireWithin = (ms: number) => {
    for (const t of world.timers) if (!t.fired && t.ms >= 1000 && t.ms <= ms) { t.fired = true; t.fn(); }
  };
  // What a frame left to run at once (a request from the popup is answered there, never in the frame).
  const tasks = () => {
    for (const t of world.timers) if (!t.fired && t.ms === 0) { t.fired = true; t.fn(); }
  };
  // Answers whatever is asked of the host until nothing is: root and folders from `folders`, git with `git`.
  const settle = async (folders: Record<string, any>, git: any) => {
    for (let turn = 0; turn < 20; turn++) {
      await flush();
      const calls = [...pending("git"), ...pending("list")];
      if (!calls.length) return;
      for (const c of calls) {
        if (world.calls.git.includes(c)) await answer(c, git);
        else await answer(c, folders[c.args[0]] ?? listing());
      }
    }
    throw new Error("the host is still being asked");
  };
  const show = (space: string) => { for (const s of world.spaces) s.active = s.id === space; };
  return { view, cx, world, inputCalls, pending, answer, fail, frame, press, pressRight, fireTimers, fireWithin, tasks, settle, show, menuItems };
}

const A = { "/a": listing("src/", "x.txt", "gone.txt"), "/a/src": listing("a.js") };
const B = { "/b": listing("other.txt") };
const [SRC, X, GONE] = [0, 1, 2];

test("the first view of a folder says loading, then shows the tree", async () => {
  const s = sidebar();
  expect(s.frame().texts).toContain("empty.loading");
  await s.settle(A, repo("/a", { "/a/x.txt": "M" }));
  const frame = s.frame();
  expect(frame.texts).not.toContain("empty.loading");
  expect(frame.texts).toEqual(expect.arrayContaining(["src", "x.txt", "gone.txt"]));
  expect(frame.lists).toEqual([{ id: "tree:|/a", count: 3 }]);
});

test("the root is drawn as soon as it is listed, before git has answered", async () => {
  const s = sidebar();
  await flush();
  expect(s.pending("git")).toHaveLength(1);
  expect(s.pending("list").map(c => c.args[0])).toEqual(["/a"]);
  await s.answer(s.pending("list")[0], A["/a"]);
  const frame = s.frame();
  expect(frame.texts).not.toContain("empty.loading");
  expect(frame.lists[0].count).toBe(3);
  expect(s.pending("git")).toHaveLength(1);
});

test("switching back to a folder shows its tree on the first frame, and reads it again behind it", async () => {
  const s = sidebar();
  await s.settle(A, repo("/a", { "/a/x.txt": "M" }));
  s.press(s.frame(), SRC);
  await s.settle(A, repo("/a", { "/a/x.txt": "M" }));
  s.show("s2");
  expect(s.frame().texts).toContain("empty.loading");
  await s.settle(B, repo("/b"));

  s.show("s1");
  const back = s.frame();
  expect(back.texts).not.toContain("empty.loading");
  // The tree as it was left: src open with its file, one list of its own.
  expect(back.texts).toEqual(expect.arrayContaining(["src", "a.js", "x.txt", "gone.txt"]));
  expect(back.lists).toEqual([{ id: "tree:|/a", count: 4 }]);
  // And it is being read again, git included.
  expect(s.pending("git")).toHaveLength(1);
  expect(s.pending("list").map(c => c.args[0]).sort()).toEqual(["/a", "/a/src"]);
});

test("a click on a row that is gone, in a tree still being read again, drops it and says so", async () => {
  const s = sidebar();
  await s.settle(A, repo("/a"));
  s.show("s2");
  s.frame();
  await s.settle(B, repo("/b"));
  s.show("s1");
  const back = s.frame();

  s.press(back, GONE);
  await flush();
  expect(s.world.opened).toEqual([]);
  expect(s.pending("stat").map(c => c.args[0])).toEqual(["/a/gone.txt"]);
  await s.answer(s.pending("stat")[0], null);
  expect(s.world.opened).toEqual([]);
  expect(s.world.toasts.map(n => n.title)).toEqual(['error.gone {"name":"gone.txt"}']);
  expect(s.frame().texts).not.toContain("gone.txt");

  s.press(s.frame(), X);
  await flush();
  await s.answer(s.pending("stat")[0], { len: 1, mtime_ms: 1 });
  expect(s.world.opened.map(o => o.path)).toEqual(["/a/x.txt"]);
});

test("a click in a tree that is not being read again opens at once, with no check", async () => {
  const s = sidebar();
  await s.settle(A, repo("/a"));
  s.press(s.frame(), X);
  expect(s.world.opened.map(o => o.path)).toEqual(["/a/x.txt"]);
  expect(s.world.calls.stat).toHaveLength(0);
});

test("a folder that is gone when it is opened leaves the list with a note", async () => {
  const s = sidebar();
  await s.settle(A, repo("/a"));
  s.press(s.frame(), SRC);
  await flush();
  await s.fail(s.pending("list")[0], new Error("cannot read /a/src"));
  await s.answer(s.pending("stat")[0], null);
  expect(s.world.toasts.map(n => n.title)).toEqual(['error.gone {"name":"src"}']);
  expect(s.frame().texts).not.toContain("src");
});

test("an untrusted folder shows no marks from before, and git is asked on every switch", async () => {
  const s = sidebar();
  await s.settle(A, { ...repo("/a"), untrusted: true });
  expect(s.frame().texts).toContain("git.untrusted");
  s.show("s2");
  s.frame();
  await s.settle(B, repo("/b"));
  s.show("s1");
  const back = s.frame();
  expect(back.texts).toContain("git.untrusted");
  expect(s.pending("git")).toHaveLength(1);
});

// The row a click lands on is the one it was drawn for.

const SUB = "/repo/sub";
// A folder inside a repo, first shown: its own rows are drawn while git has not said where the repo's top is.
async function provisional(spaces = two(SUB, "/b")) {
  const s = sidebar(spaces);
  await flush();
  await s.answer(s.pending("list")[0], listing("README.md"));
  const drawn = s.frame();
  expect(drawn.texts).toContain("README.md");
  return { s, drawn };
}

test("a click on a row opens that row's file, though git moves the root before the check answers", async () => {
  const { s, drawn } = await provisional();
  s.press(drawn, 0);
  await flush();
  expect(s.pending("stat").map(c => c.args[0])).toEqual([`${SUB}/README.md`]);
  await s.answer(s.pending("git")[0], repo("/repo"));
  await s.answer(s.pending("list").find(c => c.args[0] === "/repo")!, listing("README.md", "sub/"));
  await s.answer(s.pending("stat")[0], { len: 1, mtime_ms: 1 });
  expect(s.world.opened.map(o => o.path)).toEqual([`${SUB}/README.md`]);
});

test("a file that is gone is dropped from the tree it was in, not from the tree that replaced it", async () => {
  const { s, drawn } = await provisional();
  s.press(drawn, 0);
  await flush();
  await s.answer(s.pending("git")[0], repo("/repo"));
  await s.answer(s.pending("list").find(c => c.args[0] === "/repo")!, listing("README.md", "sub/"));
  await s.answer(s.pending("stat")[0], null);
  expect(s.world.opened).toEqual([]);
  expect(s.world.toasts.map(n => n.title)).toEqual(['error.gone {"name":"README.md"}']);
  expect(s.frame().texts).toContain("README.md");
});

test("the tree drawn from the folder stays on screen, and can be clicked, until the repo's top has been listed", async () => {
  const { s, drawn } = await provisional();
  await s.answer(s.pending("git")[0], repo("/repo"));
  const between = s.frame();
  expect(between.texts).not.toContain("empty.loading");
  expect(between.texts).toContain("README.md");
  expect(between.keys).toEqual(drawn.keys);
  s.press(between, 0);
  await flush();
  await s.answer(s.pending("stat")[0], { len: 1, mtime_ms: 1 });
  expect(s.world.opened.map(o => o.path)).toEqual([`${SUB}/README.md`]);
  await s.answer(s.pending("list").find(c => c.args[0] === "/repo")!, listing("README.md", "sub/"));
  const after = s.frame();
  expect(after.texts).not.toContain("empty.loading");
  expect(after.keys).toEqual(["/repo/README.md", "/repo/sub"]);
});

test("a click from the frame drawn before the root moved opens the file it was drawn for", async () => {
  const { s, drawn } = await provisional();
  await s.answer(s.pending("git")[0], repo("/repo"));
  await s.answer(s.pending("list").find(c => c.args[0] === "/repo")!, listing("README.md", "sub/"));
  const moved = s.frame();
  expect(moved.keys).not.toEqual(drawn.keys);
  s.press(drawn, 0);
  await flush();
  expect(s.pending("stat").map(c => c.args[0])).toEqual([`${SUB}/README.md`]);
  expect(s.world.opened).toEqual([]);
  await s.answer(s.pending("stat")[0], { len: 1, mtime_ms: 1 });
  expect(s.world.opened.map(o => o.path)).toEqual([`${SUB}/README.md`]);
});

test("a click from the old frame on a file that is gone is told so, and a click on a path never listed does nothing", async () => {
  const { s, drawn } = await provisional();
  await s.answer(s.pending("git")[0], repo("/repo"));
  await s.answer(s.pending("list").find(c => c.args[0] === "/repo")!, listing("README.md", "sub/"));
  s.frame();
  s.press(drawn, 0);
  await flush();
  await s.answer(s.pending("stat")[0], null);
  expect(s.world.opened).toEqual([]);
  expect(s.world.toasts.map(n => n.title)).toEqual(['error.gone {"name":"README.md"}']);
  s.world.onClick("/repo/sub/never-listed.txt", s.cx);
  await flush();
  expect(s.pending("stat")).toHaveLength(0);
});

test("a right click from the old frame opens the popup for the file it was drawn for", async () => {
  const { s, drawn } = await provisional();
  await s.answer(s.pending("git")[0], repo("/repo"));
  await s.answer(s.pending("list").find(c => c.args[0] === "/repo")!, listing("README.md", "sub/"));
  s.frame();
  s.pressRight(drawn, 0);
  expect(s.world.menus).toHaveLength(1);
  expect(s.world.menus[0].props.menu).toMatchObject({ abs: `${SUB}/README.md`, root: SUB, dir: false });
  expect(s.view.here.selected).toBeNull();
});

test("a folder selected in the tree drawn from the folder is not a folder of the repo's top", async () => {
  const s = sidebar(two(SUB, "/b"));
  await flush();
  await s.answer(s.pending("list")[0], listing("lib/", "README.md"));
  s.press(s.frame(), 0);
  await flush();
  expect(s.view.selectedDir()).toBe(`${SUB}/lib`);
  await s.answer(s.pending("git")[0], repo("/repo"));
  for (const c of s.pending("list")) await s.answer(c, c.args[0] === "/repo" ? listing("lib/", "sub/") : listing("x.js"));
  s.frame();
  expect(s.view.here.selected).toBeNull();
  expect(s.view.selectedDir()).toBe("/repo");
});

test("a popup opened on the tree drawn from the folder still gets its new file after the root has moved", async () => {
  const { s, drawn } = await provisional();
  s.pressRight(drawn, 0);
  const menu = s.world.menus[0].props.menu;
  await s.answer(s.pending("git")[0], repo("/repo"));
  await s.answer(s.pending("list").find(c => c.args[0] === "/repo")!, listing("README.md", "sub/"));
  s.world.shared.set("yuzora.files.request", { action: "new-file", root: menu.root, seq: 7, dir: SUB });
  s.frame();
  s.tasks();
  expect(s.view.edit).toMatchObject({ kind: "new-file", dir: SUB });
  expect(s.world.toasts).toEqual([]);
});

test("the popup menu is for the file of the row it was opened on, under the root that row was drawn from", async () => {
  const { s, drawn } = await provisional();
  s.pressRight(drawn, 0);
  expect(s.world.menus).toHaveLength(1);
  expect(s.world.menus[0].props.menu).toMatchObject({ abs: `${SUB}/README.md`, root: SUB, dir: false });
});

// The Space a file is opened in is the one it was clicked in.

async function revalidating(spaces = two("/a", "/b")) {
  const s = sidebar(spaces);
  await s.settle(A, repo("/a"));
  s.show("s2");
  s.frame();
  await s.settle(B, repo("/b"));
  s.show("s1");
  return { s, back: s.frame() };
}

test("a file checked after the Space changed is not opened in the Space that is shown by then", async () => {
  const { s, back } = await revalidating();
  s.press(back, X);
  await flush();
  expect(s.pending("stat")).toHaveLength(1);
  s.show("s2");
  s.frame();
  await s.answer(s.pending("stat")[0], { len: 1, mtime_ms: 1 });
  expect(s.world.opened).toEqual([]);
  expect(s.world.toasts).toEqual([]);
});

test("two Spaces on one folder share its tree, and a click still belongs to the Space it was made in", async () => {
  const s = sidebar([{ id: "s1", active: true, folder: "/a", machine: null }, { id: "s2", active: false, folder: "/a", machine: null }]);
  await s.settle(A, repo("/a"));
  s.show("s2");
  const there = s.frame();
  s.press(there, X);
  await flush();
  expect(s.pending("stat")).toHaveLength(1);
  s.show("s1");
  s.frame();
  await s.answer(s.pending("stat")[0], { len: 1, mtime_ms: 1 });
  expect(s.world.opened).toEqual([]);
});

test("a file in a tree being read again opens in the Space it was clicked in, once the check says it is there", async () => {
  const { s, back } = await revalidating();
  s.press(back, X);
  await flush();
  await s.answer(s.pending("stat")[0], { len: 1, mtime_ms: 1 });
  expect(s.world.opened).toEqual([{ path: "/a/x.txt", on: null, space: "s1" }]);
});

test("a check that has not answered in time does not hold the click: the file is opened and its pane says what is wrong", async () => {
  const { s, back } = await revalidating(two("/a", "/b", "m1"));
  s.press(back, X);
  await flush();
  expect(s.world.opened).toEqual([]);
  s.fireTimers();
  await flush();
  expect(s.world.opened).toEqual([{ path: "/a/x.txt", on: "m1", space: "s1" }]);
  expect(s.world.toasts).toEqual([]);
  await s.answer(s.pending("stat")[0], null);
  expect(s.world.opened).toHaveLength(1);
  expect(s.frame().texts).not.toContain("x.txt");
});

test("a check that is late and then finds the Space changed opens nothing", async () => {
  const { s, back } = await revalidating(two("/a", "/b", "m1"));
  s.press(back, X);
  s.show("s2");
  s.frame();
  s.fireTimers();
  await flush();
  expect(s.world.opened).toEqual([]);
});

test("a check that fails is not an answer that the file is there or gone: the file is opened, the row stays", async () => {
  const { s, back } = await revalidating(two("/a", "/b", "m1"));
  s.press(back, X);
  await flush();
  await s.fail(s.pending("stat")[0], new Error("machine m1 is not connected"));
  expect(s.world.opened).toEqual([{ path: "/a/x.txt", on: "m1", space: "s1" }]);
  expect(s.world.toasts).toEqual([]);
  expect(s.frame().texts).toContain("x.txt");
});

test("a click waits at most 1.5 seconds for the check of the file", async () => {
  const { s, back } = await revalidating(two("/a", "/b", "m1"));
  s.press(back, X);
  await flush();
  s.fireWithin(1499);
  await flush();
  expect(s.world.opened).toEqual([]);
  s.fireWithin(1500);
  await flush();
  expect(s.world.opened.map(o => o.path)).toEqual(["/a/x.txt"]);
});

test("a tree whose last read failed is checked before a click opens a file, as a tree being read is", async () => {
  const s = sidebar(two("/a", "/b", "m1"));
  await s.settle(A, repo("/a"));
  s.view.reload(s.cx, true);
  await flush();
  for (const c of s.pending("list")) await s.fail(c, new Error("offline"));
  await s.answer(s.pending("git")[0], repo("/a"));
  const stale = s.frame();
  expect(stale.texts).toContain("offline");
  s.press(stale, GONE);
  await flush();
  expect(s.world.opened).toEqual([]);
  expect(s.pending("stat").map(c => c.args[0])).toEqual(["/a/gone.txt"]);
  await s.answer(s.pending("stat")[0], null);
  expect(s.world.toasts.map(n => n.title)).toEqual(['error.gone {"name":"gone.txt"}']);
});

test("files open in the order they were clicked, whichever check answers first", async () => {
  const { s, back } = await revalidating();
  s.press(back, X);
  s.press(back, GONE);
  await flush();
  const [first, second] = s.pending("stat");
  await s.answer(second, { len: 1, mtime_ms: 1 });
  expect(s.world.opened).toEqual([]);
  await s.answer(first, { len: 1, mtime_ms: 1 });
  expect(s.world.opened.map(o => o.path)).toEqual(["/a/x.txt", "/a/gone.txt"]);
});

test("a file clicked after the tree is up to date still opens after the one whose check is slow", async () => {
  const { s, back } = await revalidating(two("/a", "/b", "m1"));
  s.press(back, X);
  await flush();
  await s.settle(A, repo("/a"));
  const fresh = s.frame();
  s.press(fresh, GONE);
  await flush();
  expect(s.world.opened).toEqual([]);
  s.fireTimers();
  await flush();
  expect(s.world.opened.map(o => o.path)).toEqual(["/a/x.txt", "/a/gone.txt"]);
});

// Reads that are asked for while one is running are not lost.

test("a change made while git is still being asked is read as soon as that read ends", async () => {
  const s = sidebar();
  await flush();
  await s.answer(s.pending("list")[0], listing("x.txt"));
  s.view.reload(s.cx);
  await flush();
  await s.answer(s.pending("git")[0], repo("/a"));
  await s.settle({ "/a": listing("x.txt", "y.txt") }, repo("/a"));
  expect(s.frame().texts).toEqual(expect.arrayContaining(["x.txt", "y.txt"]));
});

test("coming back to a folder while its read is still running reads it again when that read ends", async () => {
  const s = sidebar(two("/a", "/b", "m1"));
  await s.settle({ "/a": listing("x.txt") }, repo("/a"));
  s.view.reload(s.cx, true);
  await flush();
  const running = s.pending("git")[0];
  for (const c of s.pending("list")) await s.answer(c, listing("x.txt"));
  s.show("s2");
  s.frame();
  await flush();
  for (const c of [...s.pending("git"), ...s.pending("list")]) if (c !== running) await s.answer(c, s.world.calls.git.includes(c) ? repo("/b") : listing("other.txt"));
  s.show("s1");
  s.frame();
  await flush();
  await s.answer(running, repo("/a"));
  await s.settle({ "/a": listing("x.txt", "y.txt") }, repo("/a"));
  expect(s.frame().texts).toContain("y.txt");
});

// The tree's top follows what git last said.

test("a failed first read of git does not fix the root to the folder: the next answer moves it", async () => {
  const s = sidebar(two(SUB, "/b"));
  await flush();
  await s.fail(s.pending("git")[0], new Error("git timed out"));
  await s.answer(s.pending("list")[0], listing("inner.txt"));
  expect(s.view.here.root).toBe(SUB);
  s.view.reload(s.cx);
  await s.settle({ [SUB]: listing("inner.txt"), "/repo": listing("sub/", "top.txt") }, repo("/repo"));
  expect(s.view.here.root).toBe("/repo");
  expect(s.frame().texts).toEqual(expect.arrayContaining(["sub", "top.txt"]));
  expect(s.pending("watch").map(c => c.args[0])).toContain("/repo");
});

// What a read that did not count leaves behind.

test("the failed listing of the folder git then moved off does not mark the new root's tree", async () => {
  const s = sidebar(two(SUB, "/b"));
  await flush();
  await s.answer(s.pending("git")[0], repo("/repo"));
  await s.fail(s.pending("list").find(c => c.args[0] === SUB)!, new Error(`EACCES ${SUB}`));
  await s.answer(s.pending("list").find(c => c.args[0] === "/repo")!, listing("a.txt"));
  const texts = s.frame().texts;
  expect(texts).toContain("a.txt");
  expect(texts).not.toContain(`EACCES ${SUB}`);
  expect(s.pending("watch").map(c => c.args[0])).toEqual(["/repo"]);
});

test("an open folder that cannot be listed again is closed on screen as well as in state", async () => {
  const s = sidebar(two("/a", "/b", "m1"));
  await s.settle(A, repo("/a"));
  s.press(s.frame(), SRC);
  await s.settle(A, repo("/a"));
  expect(s.frame().texts).toContain("a.js");
  s.view.reload(s.cx, true);
  await flush();
  for (const c of s.pending("list")) {
    if (c.args[0] === "/a/src") await s.fail(c, new Error("Permission denied"));
    else await s.answer(c, A[c.args[0] as keyof typeof A]);
  }
  await s.settle(A, repo("/a"));
  expect(s.frame().texts).not.toContain("a.js");
});

test("a folder cut short stops saying so once it is closed", async () => {
  const s = sidebar();
  const cut = { ...listing("f1"), truncated: true };
  const tree = { "/a": listing("big/", "x.txt"), "/a/big": cut };
  await s.settle(tree, repo("/a"));
  s.press(s.frame(), 0);
  await s.settle(tree, repo("/a"));
  expect(s.frame().texts.some(t => t.startsWith("empty.limited"))).toBe(true);
  s.press(s.frame(), 0);
  expect(s.frame().texts.some(t => t.startsWith("empty.limited"))).toBe(false);
  s.view.reload(s.cx, true);
  await s.settle(tree, repo("/a"));
  expect(s.frame().texts.some(t => t.startsWith("empty.limited"))).toBe(false);
});

test("a clean repo that becomes untrusted says so", async () => {
  const s = sidebar();
  await s.settle(A, repo("/a"));
  expect(s.frame().texts).not.toContain("git.untrusted");
  s.view.reload(s.cx, true);
  await s.settle(A, { ...repo("/a"), untrusted: true });
  expect(s.frame().texts).toContain("git.untrusted");
});

test("a folder that cannot be opened says so in the Space it was opened in, and in no other", async () => {
  const s = sidebar([{ id: "s1", active: true, folder: "/a", machine: null }, { id: "s2", active: false, folder: "/a", machine: null }]);
  await s.settle(A, repo("/a"));
  s.press(s.frame(), SRC);
  await flush();
  s.show("s2");
  s.frame();
  await s.fail(s.pending("list").find(c => c.args[0] === "/a/src")!, new Error("cannot read /a/src"));
  await s.answer(s.pending("stat")[0], { len: 0, mtime_ms: 1 });
  expect(s.world.toasts).toEqual([]);
});

test("a refresh asked for while the timer's read runs makes an open search follow the files; the timer's reads alone leave it to the watcher", async () => {
  const s = sidebar();
  await s.settle(A, repo("/a"));
  s.world.query = "x";
  s.view.searching = true;
  const searches = () => s.world.timers.filter(t => t.ms === 60).length;
  const before = searches();
  s.view.reload(s.cx, true);
  await flush();
  s.view.reload(s.cx);
  await s.settle(A, repo("/a"));
  expect(searches() - before).toBe(1);
  s.view.reload(s.cx, true);
  await s.settle(A, repo("/a"));
  expect(searches() - before).toBe(1);
});

test("a closed folder with a change below it carries a dot, and a changed file its letter", async () => {
  const s = sidebar();
  const marks = { "/a/src/a.js": "M", "/a/x.txt": "A" };
  await s.settle(A, repo("/a", marks));
  const closed = s.frame().texts;
  expect(closed).toEqual(expect.arrayContaining(["•", "A"]));
  expect(closed).not.toContain("M");
  s.press(s.frame(), SRC);
  await s.settle(A, repo("/a", marks));
  const open = s.frame().texts;
  expect(open).toEqual(expect.arrayContaining(["M", "A"]));
  expect(open).not.toContain("•");
});

// New file, new folder and rename work on the machine and in the folder of the row they were started on.

const spacesAt = (folder: string, machine: string | null) => [
  { id: "s1", active: true, folder, machine },
  { id: "s2", active: false, folder: "/b", machine: null },
];
async function sidebarAt(machine: string | null, folder = "/srv") {
  const s = sidebar(spacesAt(folder, machine));
  await s.settle({ [folder]: listing("src/", "a.txt") }, repo(folder));
  return s;
}
const SRC_ROW = 0, A_ROW = 1;

// The name typed in the bar above the tree, and Enter.
async function submit(s: Awaited<ReturnType<typeof sidebarAt>>, name: string) {
  s.world.name = name;
  s.view.submitEdit(s.cx);
  await flush();
}
const makes = (s: Awaited<ReturnType<typeof sidebarAt>>) => s.world.calls.make.map(c => c.args);

for (const [where, machine] of [["a remote Space", "lab"], ["this machine", null]] as const) {
  test(`${where}: the toolbar's new file is made where the Space is, and opened there`, async () => {
    const s = await sidebarAt(machine);
    s.frame();
    expect(s.world.clicks.has("new-file")).toBe(true);
    s.world.clicks.get("new-file")(null, s.cx);
    expect(s.view.edit).toMatchObject({ kind: "new-file", dir: "/srv" });
    expect(s.world.toasts).toEqual([]);
    await submit(s, "n.txt");
    expect(makes(s)).toEqual([["createFile", "/srv/n.txt", { on: machine }]]);
    await s.answer(s.pending("make")[0], null);
    expect(s.world.opened).toEqual([{ path: "/srv/n.txt", on: machine, space: "s1" }]);
    expect(s.world.toasts).toEqual([]);
    expect(s.pending("list").map(c => [c.args[0], c.args[1].on])).toContainEqual(["/srv", machine]);
  });

  test(`${where}: the toolbar's new folder is made where the Space is, in the folder selected`, async () => {
    const s = await sidebarAt(machine);
    s.press(s.frame(), SRC_ROW);
    await s.settle({ "/srv/src": listing("x.js") }, repo("/srv"));
    s.frame();
    expect(s.view.selectedDir()).toBe("/srv/src");
    s.world.clicks.get("new-folder")(null, s.cx);
    await submit(s, "lib");
    expect(makes(s)).toEqual([["createFolder", "/srv/src/lib", { on: machine }]]);
    await s.answer(s.pending("make")[0], null);
    expect(s.world.opened).toEqual([]);
    expect(s.view.here.open.has("src")).toBe(true);
    expect(s.view.here.selected).toBe("src/lib");
  });

  test(`${where}: a rename from the right-click menu renames there, and tells the panes showing it`, async () => {
    const s = await sidebarAt(machine);
    s.pressRight(s.frame(), A_ROW);
    const menu = s.world.menus[0].props.menu;
    expect(menu).toMatchObject({ abs: "/srv/a.txt", on: machine });
    s.menuItems(menu).find((i: any) => i?.label === "menu.rename").run();
    s.frame();
    s.tasks();
    expect(s.view.edit).toMatchObject({ kind: "rename", path: "/srv/a.txt", dir: "/srv" });
    expect(s.inputCalls.slice(-3)).toEqual([["set_value", "a.txt"], ["set_selected_range", { start: 0, end: 1 }], ["focus"]]);
    expect(s.world.toasts).toEqual([]);
    await submit(s, "b.txt");
    expect(makes(s)).toEqual([["rename", "/srv/a.txt", "/srv/b.txt", { on: machine }]]);
    expect(s.world.moved).toEqual([]);
    await s.answer(s.pending("make")[0], null);
    expect(s.world.moved).toEqual([["/srv/a.txt", "/srv/b.txt", machine]]);
    expect(s.view.here.selected).toBe("b.txt");
  });

  test(`${where}: the menu's new file and rename start an edit, and a new folder from a folder row is made in it`, async () => {
    const s = await sidebarAt(machine);
    s.pressRight(s.frame(), SRC_ROW);
    const menu = s.world.menus[0].props.menu;
    s.menuItems(menu).find((i: any) => i?.label === "menu.new_folder").run();
    s.frame();
    s.tasks();
    expect(s.view.edit).toMatchObject({ kind: "new-folder", dir: "/srv/src" });
    await submit(s, "lib");
    expect(makes(s)).toEqual([["createFolder", "/srv/src/lib", { on: machine }]]);
  });

  test(`${where}: a name that cannot be made says why, and the tree is read again`, async () => {
    const s = await sidebarAt(machine);
    s.view.startEdit("new-file", null, s.view.selectedDir(), s.cx);
    await submit(s, "a.txt");
    await s.fail(s.pending("make")[0], new Error("/srv/a.txt already exists"));
    expect(s.world.toasts.map(n => [n.title, n.body])).toEqual([["error.new-file", "/srv/a.txt already exists"]]);
    expect(s.world.opened).toEqual([]);
    expect(s.pending("list").map(c => c.args[0])).toContain("/srv");
  });
}

test("the menu of a remote Space offers new file, new folder and rename, and still not the trash or the file manager", async () => {
  const s = await sidebarAt("lab");
  s.pressRight(s.frame(), A_ROW);
  const items = s.menuItems(s.world.menus[0].props.menu).filter((i: any) => i);
  const enabled = Object.fromEntries(items.map((i: any) => [i.label, i.enabled !== false]));
  expect(enabled).toMatchObject({ "menu.new_file": true, "menu.new_folder": true, "menu.rename": true, "menu.trash": false, "menu.copy_path": true });
  expect(Object.entries(enabled).filter(([label, on]) => !on).map(([label]) => label).sort()).toEqual(["menu.trash", expect.stringMatching(/^menu\.reveal/)].sort());
});

test("the toolbar of a remote Space has no read-only tooltip, and nothing is refused", async () => {
  const s = await sidebarAt("lab");
  const shown = s.frame();
  expect(shown.texts).not.toContain("remote.readonly");
  s.view.startEdit("new-folder", null, "/srv", s.cx);
  expect(s.world.toasts).toEqual([]);
  expect(s.view.edit).toMatchObject({ kind: "new-folder" });
});

test("a name typed for one machine is made on that machine, though another Space is shown by the time Enter is heard", async () => {
  const s = await sidebarAt("lab");
  s.view.startEdit("new-file", null, s.view.selectedDir(), s.cx);
  await submit(s, "n.txt");
  expect(makes(s)).toEqual([["createFile", "/srv/n.txt", { on: "lab" }]]);
  // The Space changes while the machine is asked.
  s.show("s2");
  s.frame();
  await s.settle(B, repo("/b"));
  await s.answer(s.pending("make")[0], null);
  // Not opened in the pane of another Space, and no other folder is marked or read for it.
  expect(s.world.opened).toEqual([]);
  expect(s.world.toasts).toEqual([]);
  expect(s.pending("list").map(c => [c.args[0], c.args[1].on])).toEqual([["/srv", "lab"]]);
  expect(s.view.here.cwd).toBe("/b");
  expect(s.view.here.selected).toBeNull();
});

test("an edit started in one Space is dropped when another is shown: nothing is made for it", async () => {
  const s = await sidebarAt("lab");
  s.view.startEdit("new-file", null, s.view.selectedDir(), s.cx);
  s.show("s2");
  s.frame();
  await submit(s, "n.txt");
  expect(makes(s)).toEqual([]);
});

test("a remote and a local Space on the same folder name are two places: the edit goes where its Space is", async () => {
  const s = sidebar([{ id: "s1", active: true, folder: "/srv", machine: "lab" }, { id: "s2", active: false, folder: "/srv", machine: null }]);
  await s.settle({ "/srv": listing("a.txt") }, repo("/srv"));
  s.show("s2");
  s.frame();
  await s.settle({ "/srv": listing("b.txt") }, repo("/srv"));
  s.view.startEdit("new-file", null, s.view.selectedDir(), s.cx);
  await submit(s, "n.txt");
  expect(makes(s)).toEqual([["createFile", "/srv/n.txt", { on: null }]]);
  await s.answer(s.pending("make")[0], null);
  expect(s.world.opened).toEqual([{ path: "/srv/n.txt", on: null, space: "s2" }]);
});

test("a request from the popup of another machine, or of a root this tree is not, starts nothing", async () => {
  const s = await sidebarAt("lab");
  s.frame();
  s.world.shared.set("yuzora.files.request", { action: "new-file", root: "/srv", on: null, seq: 1, dir: "/srv" });
  s.frame();
  s.tasks();
  expect(s.view.edit).toBeNull();
  s.world.shared.set("yuzora.files.request", { action: "new-file", root: "/elsewhere", on: "lab", seq: 2, dir: "/elsewhere" });
  s.frame();
  s.tasks();
  expect(s.view.edit).toBeNull();
  s.world.shared.set("yuzora.files.request", { action: "new-file", root: "/srv", on: "lab", seq: 3, dir: "/srv" });
  s.frame();
  s.tasks();
  expect(s.view.edit).toMatchObject({ kind: "new-file", dir: "/srv" });
  expect(s.world.toasts).toEqual([]);
});

test("a name made in a folder of the tree drawn from the folder is made there, whatever root git names next", async () => {
  const s = sidebar(spacesAt(SUB, "lab"));
  await flush();
  await s.answer(s.pending("list")[0], listing("lib/", "README.md"));
  s.press(s.frame(), 0);
  await flush();
  expect(s.view.selectedDir()).toBe(`${SUB}/lib`);
  s.view.startEdit("new-file", null, s.view.selectedDir(), s.cx);
  // The repo's top is named and listed before the name is entered.
  await s.answer(s.pending("git")[0], repo("/repo"));
  for (const c of s.pending("list")) await s.answer(c, c.args[0] === "/repo" ? listing("lib/", "sub/") : listing("x.js"));
  s.frame();
  await submit(s, "n.txt");
  expect(makes(s)).toEqual([["createFile", `${SUB}/lib/n.txt`, { on: "lab" }]]);
  await s.answer(s.pending("make")[0], null);
  expect(s.world.opened).toEqual([{ path: `${SUB}/lib/n.txt`, on: "lab", space: "s1" }]);
});

// Where a file opens: a tab of its own, unless the menu says to the side.

const props = (path: string, on: string | null = null, more = {}) => ({ path, on, opened: expect.any(Number), ...more });
const tabFor = (space: string, path: string, on: string | null = null, more = {}) =>
  ["tab.new", { name: path.split("/").pop(), space, mod: "yuzora-editor", props: props(path, on, more) }];
const opens = (s: ReturnType<typeof sidebar>) => s.world.runs.filter(([name]) => name === "tab.new" || name === "pane.split");

async function realSidebarAt(machine: string | null, folder = "/srv") {
  const s = sidebar(spacesAt(folder, machine), true);
  await s.settle({ [folder]: listing("src/", "a.txt", "pic.png") }, repo(folder));
  return s;
}

test("a click on a file opens it in a tab of its own, in the Space it was clicked in", async () => {
  const s = sidebar(two("/a", "/b"), true);
  await s.settle(A, repo("/a"));
  s.press(s.frame(), X);
  expect(opens(s)).toEqual([tabFor("s1", "/a/x.txt")]);
});

test("a click on an image opens it in a tab of its own", async () => {
  const s = await realSidebarAt(null);
  s.press(s.frame(), 2);
  expect(opens(s)).toEqual([["tab.new", { name: "pic.png", space: "s1", mod: "yuzora-files-image", props: props("/srv/pic.png") }]]);
});

test("a search hit opens in a tab of its own at its line", async () => {
  const s = sidebar(two("/a", "/b"), true);
  await s.settle(A, repo("/a"));
  s.view.results = [{ key: "k", root: "/a", on: null, path: "x.txt", line: 7, column: 2, text: "needle" }];
  s.view.openResult("k");
  expect(opens(s)).toEqual([tabFor("s1", "/a/x.txt", null, { line: 7, column: 2 })]);
});

for (const [where, machine] of [["a remote Space", "lab"], ["this machine", null]] as const) {
  test(`${where}: a file made from the toolbar opens in a tab of its own`, async () => {
    const s = await realSidebarAt(machine);
    s.view.startEdit("new-file", null, s.view.selectedDir(), s.cx);
    await submit(s, "n.txt");
    await s.answer(s.pending("make")[0], null);
    expect(opens(s)).toEqual([tabFor("s1", "/srv/n.txt", machine)]);
  });

  test(`${where}: the menu of a file opens it in a tab, or to the side of the pane that has focus`, async () => {
    const s = await realSidebarAt(machine);
    s.pressRight(s.frame(), A_ROW);
    const items = s.menuItems(s.world.menus[0].props.menu);
    const labels = items.filter((i: any) => i).map((i: any) => i.label);
    expect(labels.slice(0, 3)).toEqual(["menu.open", "menu.open_side", "menu.terminal"]);
    expect(items.find((i: any) => i?.label === "menu.open_side").enabled).not.toBe(false);
    items.find((i: any) => i?.label === "menu.open").run();
    expect(opens(s)).toEqual([tabFor("s1", "/srv/a.txt", machine)]);
    items.find((i: any) => i?.label === "menu.open_side").run();
    expect(opens(s).slice(1)).toEqual([["pane.split", { direction: "right", pane: "p1", mod: "yuzora-editor", props: props("/srv/a.txt", machine) }]]);
  });

  test(`${where}: an image opens to the side from the menu too`, async () => {
    const s = await realSidebarAt(machine);
    s.pressRight(s.frame(), 2);
    s.menuItems(s.world.menus[0].props.menu).find((i: any) => i?.label === "menu.open_side").run();
    expect(opens(s)).toEqual([["pane.split", { direction: "right", pane: "p1", mod: "yuzora-files-image", props: props("/srv/pic.png", machine) }]]);
  });
}

test("the menu of a folder has no Open and no Open to the Side", async () => {
  const s = await realSidebarAt(null);
  s.pressRight(s.frame(), SRC_ROW);
  const labels = s.menuItems(s.world.menus[0].props.menu).filter((i: any) => i).map((i: any) => i.label);
  expect(labels).not.toContain("menu.open");
  expect(labels).not.toContain("menu.open_side");
  expect(labels).toContain("menu.terminal");
});
