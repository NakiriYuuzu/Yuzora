// yuzora(v0.1.0 F1): the Files sidebar's own lifecycle, main.js and the modules it is made of run
// as shipped, with herdr, Yuzora and the element builders stubbed.
// Run: bun test scripts/yuzora-files-sidebar.test.ts
import { test, expect } from "bun:test";
import { readFile } from "node:fs/promises";

const dir = new URL("../defaults/mods/yuzora-sidebar-files/", import.meta.url);
const [main, tree, gitState, paths] = await Promise.all(["main.js", "tree-state.js", "git-state.js", "paths.js"].map(f => readFile(new URL(f, dir), "utf8")));
const code = (s: string) => s.replace(/^import [\s\S]*?;\n/gm, "").replace(/^export default class/m, "class").replace(/^export (?=async function|function|const|class)/gm, "");
const source = [paths, gitState, tree, main].map(code).join("\n");

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

// A sidebar whose host is the test's: every listing, git status and stat waits to be answered.
function sidebar(spaces: any[] = two("/a", "/b")) {
  const world = {
    spaces,
    toasts: [] as any[], opened: [] as { path: string; on: string | null; space: string }[], log: [] as string[], lists: [] as { id: string; count: number }[],
    query: "", keys: [] as string[], onClick: null as any, onSecondary: null as any, menus: [] as any[],
    timers: [] as { ms: number; fn: () => void; fired: boolean }[],
    calls: { list: [] as Call[], git: [] as Call[], stat: [] as Call[], watch: [] as Call[] },
  };
  const active = () => world.spaces.find(s => s.active)?.id;
  const ask = (kind: "list" | "git" | "stat" | "watch", args: any[]) => {
    const [call, promise] = waiting();
    call.args = args;
    world.calls[kind].push(call);
    return promise;
  };
  const chain: any = new Proxy(function () {}, {
    get: (_t, prop) => prop === "then" ? undefined : (...args: any[]) => {
      for (const a of args) if (typeof a === "string") world.log.push(a);
      if (prop === "on_item_click") world.onClick = args[0];
      if (prop === "on_item_secondary_click") world.onSecondary = args[0];
      if (prop === "when") { if (args[0]) args[1](chain); }
      return chain;
    },
  });
  const maker = () => chain;
  const input = { on() {}, value: () => world.query, set_value() {}, focus() {}, set_selected_range() {} };
  const stubs: Record<string, any> = {
    View: class {},
    div: maker, svg: maker, v_flex: maker, h_flex: maker,
    uniform_list: (id: string, count: number, keyOf: (i: number) => string, render: (r: { start: number; end: number }) => any) => {
      world.lists.push({ id, count });
      if (id !== "results") world.keys = Array.from({ length: count }, (_, i) => keyOf(i));
      render({ start: 0, end: count });
      return chain;
    },
    InputState: { new: () => input }, Input: { new: maker }, Scrollbar: { vertical: maker },
    spaces: () => world.spaces, panes: () => [], focused: () => null, theme: () => new Proxy({}, { get: () => "#000000" }),
    run() {}, home: () => "/home/me", props: () => null, openOverlay: (o: any) => world.menus.push(o), closeOverlay() {},
    shared: () => null, setShared() {}, notify: (n: any) => world.toasts.push(n), watchFolder: (path: string) => ask("watch", [path]),
    window: () => ({ platform: "macos" }),
    t: (key: string, vars?: object) => (vars ? `${key} ${JSON.stringify(vars)}` : key), tokens: () => ({}),
    listDir: (path: string, options: object) => ask("list", [path, options]),
    gitStatus: (cwd: string, options: object) => ask("git", [cwd, options]),
    stat: (path: string, options: object) => ask("stat", [path, options]),
    createFile() {}, createFolder() {}, rename() {}, trash() {}, reveal() {}, copyText() {}, findFiles() {}, grep() {}, unsavedCount: () => 0,
    openFile: (path: string, on: string | null) => world.opened.push({ path, on: on ?? null, space: active() }), moved() {},
    fileIcon: () => ({ icon: "file", color: "dim" }),
    localStorage: { getItem: () => null, setItem() {} },
  };
  const View = new Function(...Object.keys(stubs), `${source}\nreturn FilesSidebar;`)(...Object.values(stubs));
  const cx: any = {
    spawn: (fn: (cx: any) => Promise<any>) => { fn(cx); }, notify() {},
    timer: { every() {}, after: (ms: number, fn: () => void) => { world.timers.push({ ms, fn, fired: false }); } },
  };
  const view = new View();
  view.init(null, cx);
  const pending = (kind: "list" | "git" | "stat" | "watch") => world.calls[kind].filter(c => !c.answered);
  const answer = async (call: Call, value: any) => { call.answered = true; call.resolve(value); await flush(); };
  const fail = async (call: Call, error: Error) => { call.answered = true; call.reject(error); await flush(); };
  // The frame: what it says, which lists it draws, and the key each of the tree's rows was drawn with.
  const frame = () => {
    world.log.length = 0; world.lists.length = 0;
    view.render(cx);
    return { texts: [...world.log], lists: [...world.lists], keys: [...world.keys] };
  };
  // A click on the i-th row of a frame, as the list reports it: by the key the frame gave the row.
  const press = (shown: { keys: string[] }, i: number) => world.onClick(shown.keys[i], cx);
  const pressRight = (shown: { keys: string[] }, i: number) => world.onSecondary(shown.keys[i], {}, cx);
  // The timers a click started and nothing has fired yet.
  const fireTimers = (atLeast = 1000) => {
    for (const t of world.timers) if (!t.fired && t.ms >= atLeast) { t.fired = true; t.fn(); }
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
  return { view, cx, world, pending, answer, fail, frame, press, pressRight, fireTimers, settle, show };
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

test("a click from the frame drawn before the root moved lands on no row of the new root", async () => {
  const { s, drawn } = await provisional();
  await s.answer(s.pending("git")[0], repo("/repo"));
  await s.answer(s.pending("list").find(c => c.args[0] === "/repo")!, listing("README.md", "sub/"));
  const moved = s.frame();
  expect(moved.keys).not.toEqual(drawn.keys);
  s.press(drawn, 0);
  s.pressRight(drawn, 0);
  await flush();
  expect(s.world.opened).toEqual([]);
  expect(s.pending("stat")).toHaveLength(0);
  expect(s.world.menus).toEqual([]);
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
