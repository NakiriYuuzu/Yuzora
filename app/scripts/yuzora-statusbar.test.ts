// yuzora(v0.1.0 E1): the status bar's branch follows the focused folder in the frame it changes in.
// main.js runs as shipped with herdr, yuzora:shell and the element builders stubbed.
// Run: bun test scripts/yuzora-statusbar.test.ts
import { test, expect } from "bun:test";
import { readFile } from "node:fs/promises";

const main = await readFile(process.env.STATUSBAR_MAIN ?? new URL("../defaults/mods/yuzora-statusbar/main.js", import.meta.url), "utf8");
const code = main.replace(/^import [\s\S]*?;\n/gm, "").replace(/^export default class/m, "class").replace(/^export (?=function|const|class)/gm, "");

const flush = async () => { for (let i = 0; i < 30; i++) await Promise.resolve(); };
const texts: string[] = [];
const builder: any = new Proxy(function () {}, {
  get: (_t, prop) => (prop === "then" ? undefined : (...args: any[]) => {
    if (prop === "child" && typeof args[0] === "string") texts.push(args[0]);
    if (prop === "when" && args[0]) args[1](builder);
    return builder;
  }),
  apply: () => builder,
});
const deep: any = new Proxy({}, { get: () => deep });

type Ask = { cwd: string; on: string | null; resolve: (v: any) => void; reject: (e: any) => void; done: boolean };

function world() {
  const w = { cwd: "/a", active: true, now: 1_000_000, asks: [] as Ask[], timers: [] as { ms: number; fn: (cx: any) => void }[] };
  const stubs: Record<string, any> = {
    View: class {}, div: () => builder, svg: () => builder, h_flex: () => builder,
    spaces: () => [{ id: "s1", machine: null }], agents: () => [], panes: () => [{ pane: "p1", cwd: w.cwd, branch: "pane-branch" }], machines: () => [],
    focused: () => ({ space: "s1", pane: "p1", cwd: w.cwd, terminal: null }), updates: () => null, keys: () => [], window: () => ({ active: w.active, platform: "macos" }),
    theme: () => deep, run() {}, shared: () => null, setShared() {}, layout: () => null,
    t: (k: string) => k, tokens: () => deep,
    spaceBadges: () => ({ changed: false, kept: {}, badges: {} }),
    gitStatus: (cwd: string, o: { on: string | null }) => new Promise((resolve, reject) => { w.asks.push({ cwd, on: o.on, resolve, reject, done: false }); }),
  };
  const Statusbar = new Function(...Object.keys(stubs), "Date", `${code}\nreturn Statusbar;`)(...Object.values(stubs), { now: () => w.now });
  const cx: any = { spawn: (fn: (cx: any) => Promise<any>) => { fn(cx); }, notify() {}, timer: { every: (ms: number, fn: any) => { w.timers.push({ ms, fn }); } } };
  const view = new Statusbar();
  view.init(null, cx);
  const frame = () => { texts.length = 0; view.render(cx); return [...texts]; };
  const answer = async (a: Ask, status: any) => { a.done = true; a.resolve(status); await flush(); };
  const pending = () => w.asks.filter((a) => !a.done).map((a) => a.cwd);
  const tick = () => w.timers.filter((t) => t.ms === 2000).forEach((t) => t.fn(cx));
  return { w, view, frame, answer, pending, tick };
}

const status = (branch: string, ahead = 0, behind = 0) => ({ branch, ahead, behind });

test("a folder the focus moves to is asked about in the frame it is drawn in, not when a timer next comes round", async () => {
  const s = world();
  s.frame();
  expect(s.pending()).toEqual(["/a"]);
  await s.answer(s.w.asks[0], status("main", 2));
  s.w.cwd = "/b";
  s.frame();
  expect(s.pending()).toEqual(["/b"]);
});

test("a folder asked about once is not asked about again by drawing", async () => {
  const s = world();
  s.frame(); s.frame(); s.frame();
  expect(s.w.asks.length).toBe(1);
  await s.answer(s.w.asks[0], status("main"));
  s.frame(); s.frame();
  expect(s.w.asks.length).toBe(1);
});

test("a slow answer for the folder left does not hold up the folder moved to", async () => {
  const s = world();
  s.frame();
  s.w.cwd = "/b";
  s.frame();
  expect(s.pending()).toEqual(["/a", "/b"]);
  await s.answer(s.w.asks[1], status("topic", 0, 3));
  expect(s.frame()).toContain("topic");
  expect(s.frame()).toContain("↓3");
});

test("an answer for the folder left is not shown for the folder moved to", async () => {
  const s = world();
  s.frame();
  s.w.cwd = "/b";
  s.frame();
  await s.answer(s.w.asks[0], status("main", 5));
  const texts = s.frame();
  expect(texts).not.toContain("↑5");
  expect(texts).toContain("pane-branch");
});

test("a folder seen before shows its branch and counts in the first frame, and is asked again only when its time comes", async () => {
  const s = world();
  s.frame();
  await s.answer(s.w.asks[0], status("main", 2));
  s.w.cwd = "/b";
  s.frame();
  await s.answer(s.w.asks[1], status("topic"));
  s.w.cwd = "/a";
  const back = s.frame();
  expect(back).toContain("main");
  expect(back).toContain("↑2");
  expect(s.w.asks.length).toBe(2);
});

test("the timer asks again, every 15 s while the window is in front", async () => {
  const s = world();
  s.frame();
  await s.answer(s.w.asks[0], status("main"));
  s.w.now += 14_000;
  s.tick();
  expect(s.pending()).toEqual([]);
  s.w.now += 2_000;
  s.tick();
  expect(s.pending()).toEqual(["/a"]);
  await s.answer(s.w.asks[1], status("main"));
  s.w.active = false;
  s.w.now += 60_000;
  s.tick();
  expect(s.pending()).toEqual([]);
});

test("a failed ask leaves the folder with no counts and is not repeated by drawing", async () => {
  const s = world();
  s.frame();
  s.w.asks[0].done = true;
  s.w.asks[0].reject(new Error("fatal"));
  await flush();
  expect(s.frame()).toContain("pane-branch");
  expect(s.w.asks.length).toBe(1);
});
