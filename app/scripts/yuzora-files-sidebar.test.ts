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

// A sidebar whose host is the test's: every listing, git status and stat waits to be answered.
function sidebar() {
  const world = {
    spaces: [
      { id: "s1", active: true, folder: "/a", machine: null },
      { id: "s2", active: false, folder: "/b", machine: null },
    ],
    toasts: [] as any[], opened: [] as string[], log: [] as string[], lists: [] as { id: string; count: number }[],
    calls: { list: [] as Call[], git: [] as Call[], stat: [] as Call[] },
  };
  const ask = (kind: "list" | "git" | "stat", args: any[]) => {
    const [call, promise] = waiting();
    call.args = args;
    world.calls[kind].push(call);
    return promise;
  };
  const chain: any = new Proxy(function () {}, {
    get: (_t, prop) => prop === "then" ? undefined : (...args: any[]) => {
      for (const a of args) if (typeof a === "string") world.log.push(a);
      if (prop === "when") { if (args[0]) args[1](chain); }
      return chain;
    },
  });
  const maker = () => chain;
  const input = { on() {}, value: () => "", set_value() {}, focus() {}, set_selected_range() {} };
  const stubs: Record<string, any> = {
    View: class {},
    div: maker, svg: maker, v_flex: maker, h_flex: maker,
    uniform_list: (id: string, count: number, _key: any, render: (r: { start: number; end: number }) => any) => {
      world.lists.push({ id, count });
      render({ start: 0, end: count });
      return chain;
    },
    InputState: { new: () => input }, Input: { new: maker }, Scrollbar: { vertical: maker },
    spaces: () => world.spaces, panes: () => [], focused: () => null, theme: () => new Proxy({}, { get: () => "#000000" }),
    run() {}, home: () => "/home/me", props: () => null, openOverlay() {}, closeOverlay() {},
    shared: () => null, setShared() {}, notify: (n: any) => world.toasts.push(n), watchFolder: () => new Promise(() => {}),
    window: () => ({ platform: "macos" }),
    t: (key: string, vars?: object) => (vars ? `${key} ${JSON.stringify(vars)}` : key), tokens: () => ({}),
    listDir: (path: string, options: object) => ask("list", [path, options]),
    gitStatus: (cwd: string, options: object) => ask("git", [cwd, options]),
    stat: (path: string, options: object) => ask("stat", [path, options]),
    createFile() {}, createFolder() {}, rename() {}, trash() {}, reveal() {}, copyText() {}, findFiles() {}, grep() {}, unsavedCount: () => 0,
    openFile: (path: string) => world.opened.push(path), moved() {},
    fileIcon: () => ({ icon: "file", color: "dim" }),
    localStorage: { getItem: () => null, setItem() {} },
  };
  const View = new Function(...Object.keys(stubs), `${source}\nreturn FilesSidebar;`)(...Object.values(stubs));
  const cx: any = { spawn: (fn: (cx: any) => Promise<any>) => { fn(cx); }, notify() {}, timer: { every() {}, after() {} } };
  const view = new View();
  view.init(null, cx);
  const pending = (kind: "list" | "git" | "stat") => world.calls[kind].filter(c => !c.answered);
  const answer = async (call: Call, value: any) => { call.answered = true; call.resolve(value); await flush(); };
  const fail = async (call: Call, error: Error) => { call.answered = true; call.reject(error); await flush(); };
  // The frame: what it says and which lists it draws.
  const frame = () => {
    world.log.length = 0; world.lists.length = 0;
    view.render(cx);
    return { texts: [...world.log], lists: [...world.lists] };
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
  return { view, cx, world, pending, answer, fail, frame, settle, show };
}

const A = { "/a": listing("src/", "x.txt", "gone.txt"), "/a/src": listing("a.js") };
const B = { "/b": listing("other.txt") };

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
  s.view.click("src", s.cx);
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
  s.frame();

  s.view.click("gone.txt", s.cx);
  await flush();
  expect(s.world.opened).toEqual([]);
  expect(s.pending("stat").map(c => c.args[0])).toEqual(["/a/gone.txt"]);
  await s.answer(s.pending("stat")[0], null);
  expect(s.world.opened).toEqual([]);
  expect(s.world.toasts.map(n => n.title)).toEqual(['error.gone {"name":"gone.txt"}']);
  expect(s.frame().texts).not.toContain("gone.txt");

  s.view.click("x.txt", s.cx);
  await flush();
  await s.answer(s.pending("stat")[0], { len: 1, mtime_ms: 1 });
  expect(s.world.opened).toEqual(["/a/x.txt"]);
});

test("a click in a tree that is not being read again opens at once, with no check", async () => {
  const s = sidebar();
  await s.settle(A, repo("/a"));
  s.view.click("x.txt", s.cx);
  expect(s.world.opened).toEqual(["/a/x.txt"]);
  expect(s.world.calls.stat).toHaveLength(0);
});

test("a folder that is gone when it is opened leaves the list with a note", async () => {
  const s = sidebar();
  await s.settle(A, repo("/a"));
  s.view.click("src", s.cx);
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
