// yuzora(L2): where a Git diff opens — a modal, the open diff pane, or a new
// pane — as the Git settings say (defaults/mods/yuzora-git-*).
// Run: bun test scripts/yuzora-git-diff-open.test.ts
import { test, expect } from "bun:test";
import { readFile } from "node:fs/promises";

const read = (mod: string) => readFile(new URL(`../defaults/mods/${mod}/main.js`, import.meta.url), "utf8");
const fn = (src: string, name: string) => src.match(new RegExp(`\\nfunction ${name}\\([\\s\\S]*?\\n}\\n`))?.[0].trim();

// Every mod that opens a diff carries the same two functions (mods can't share a file).
const OPENERS = ["yuzora-sidebar-git", "yuzora-git-log", "yuzora-git-blame", "yuzora-git-repo"];
const sources = Object.fromEntries(await Promise.all([...OPENERS, "yuzora-git-diff"].map(async (m) => [m, await read(m)])));

test("each opener has the same showDiff and placeDiff, and the diff pane the same placeDiff", () => {
  const show = fn(sources[OPENERS[0]], "showDiff"), place = fn(sources[OPENERS[0]], "placeDiff");
  expect(show).toBeTruthy();
  expect(place).toBeTruthy();
  for (const m of OPENERS) {
    expect(fn(sources[m], "showDiff")).toBe(show);
    expect(fn(sources[m], "placeDiff")).toBe(place);
    // No path to the old way around them.
    expect(sources[m]).not.toContain('mod: "yuzora-git-diff", props: spec });\n  }');
  }
  expect(fn(sources["yuzora-git-diff"], "placeDiff")).toBe(place);
});

// world() below hands the helpers every name they call, so only this catches
// a mod that calls one it never imported (a ReferenceError on the first click).
test("every function the diff helpers call is imported by the mod they live in", () => {
  const imported = (src: string) =>
    new Set([...src.matchAll(/^import\s*\{([\s\S]*?)\}\s*from/gm)].flatMap((m) => m[1].split(",").map((s) => s.trim().split(/\s+as\s+/).pop()!).filter(Boolean)));
  const missing: string[] = [];
  for (const [m, src] of Object.entries(sources)) {
    const known = new Set([...imported(src), ...[...src.matchAll(/^function ([\w$]+)\(/gm)].map((f) => f[1]), "if", "return"]);
    const code = [fn(src, "showDiff"), fn(src, "placeDiff"), src.match(/\n {2}keep\(\) (\{[\s\S]*?\n {2}})\n/)?.[1]].filter(Boolean).join("\n");
    expect(code).toContain("placeDiff");
    for (const c of code.matchAll(/(?<![.\w$])([A-Za-z_$][\w$]*)\s*\(/g)) if (!known.has(c[1])) missing.push(`${m}: ${c[1]}`);
  }
  expect([...new Set(missing)]).toEqual([]);
});

// The functions with herdr and the settings stubbed: what they asked for.
function world(settings: object, diffPane: string | null, focusedPane: string | null = "p1") {
  const calls: unknown[][] = [];
  const stubs = {
    spaces: () => [{ id: "s1", active: true, tabs: [{ id: "t1", active: true }] }],
    panes: () => (diffPane ? [{ pane: "p0", mod: "other", tab: "t1" }, { pane: diffPane, mod: "yuzora-git-diff", tab: "t1" }] : [{ pane: "p0", mod: "other", tab: "t1" }]),
    setShared: (k: string, v: unknown) => calls.push(["setShared", k, v]),
    run: (name: string, args: unknown) => calls.push(["run", name, args]),
    openOverlay: (spec: unknown) => calls.push(["openOverlay", spec]),
    focused: () => (focusedPane ? { pane: focusedPane } : null),
    gitSettings: () => ({ diffOpenIn: "modal", diffKeepIn: "split", diffFollowPane: true, ...settings }),
    t: (k: string) => (k === "tab.diff" ? "Diff" : k),
  };
  const src = ["shownSpace", "findPane", "placeDiff", "showDiff"].map((name) => fn(sources[OPENERS[0]], name)).join("\n") + "\nreturn { showDiff, placeDiff };";
  const { showDiff, placeDiff } = new Function(...Object.keys(stubs), src)(...Object.values(stubs));
  return { showDiff, placeDiff, calls };
}

const spec = { cwd: "/r", on: null, path: "a.txt", orig: null, mode: "unstaged", at: 1 };

test("by default a diff opens in a modal", () => {
  const w = world({}, null);
  w.showDiff(spec);
  expect(w.calls).toEqual([["openOverlay", { mod: "yuzora-git-diff", kind: "dialog", width: 0.9, height: 0.86, props: spec }]]);
});

test("an open diff pane follows the next file, unless the settings say otherwise", () => {
  const w = world({}, "p9");
  w.showDiff(spec);
  expect(w.calls).toEqual([["setShared", "yuzora.git.diff", { ...spec, space: "s1" }], ["run", "pane.focus", { pane: "p9" }]]);
  const off = world({ diffFollowPane: false }, "p9");
  off.showDiff(spec);
  expect(off.calls.map((c) => c[0])).toEqual(["openOverlay"]);
});

test("set to open in a pane, a diff goes where Keep would put it", () => {
  const split = world({ diffOpenIn: "pane" }, null, "p1");
  split.showDiff(spec);
  expect(split.calls).toEqual([["run", "pane.split", { direction: "right", pane: "p1", mod: "yuzora-git-diff", props: spec }]]);
  const tab = world({ diffOpenIn: "pane", diffKeepIn: "tab" }, null);
  tab.showDiff(spec);
  expect(tab.calls).toEqual([["run", "tab.new", { name: "Diff", mod: "yuzora-git-diff", props: spec }]]);
});

test("with no pane to split beside, Keep opens a tab", () => {
  const w = world({}, null, null);
  w.placeDiff(spec, "split");
  expect(w.calls).toEqual([["run", "tab.new", { name: "Diff", mod: "yuzora-git-diff", props: spec }]]);
});

// The diff view in a modal: its own props only, and Keep moves it into a pane.
function diffView(p: object, shared: object | null, settings: object = {}) {
  const calls: unknown[][] = [];
  const src = sources["yuzora-git-diff"].replace(/^import [\s\S]*?;\n/gm, "").replace("export default class GitDiff", "class GitDiff");
  const stubs = {
    View: class {},
    props: () => p,
    shared: () => shared,
    run: (name: string, args: unknown) => calls.push(["run", name, args]),
    focused: () => ({ pane: "p1" }),
    closeOverlay: (id: string) => calls.push(["closeOverlay", id]),
    gitSettings: () => ({ diffKeepIn: "split", ...settings }),
    t: (k: string) => k,
  };
  const GitDiff = new Function(...Object.keys(stubs), `${src}; return GitDiff;`)(...Object.values(stubs));
  const view = new GitDiff();
  view.layout = "unified"; view.whitespace = "trim"; view.context = -1;
  return { view, calls };
}

test("a diff in a modal shows what it was opened with, not the shared request", () => {
  const { view } = diffView({ ...spec, overlay: "o1" }, { ...spec, path: "b.txt", at: 9 });
  expect(view.wanted().path).toBe("a.txt");
  // A pane follows the newer shared request, as before.
  expect(diffView({ ...spec }, { ...spec, path: "b.txt", at: 9 }).view.wanted().path).toBe("b.txt");
});

test("Keep puts the diff beside the focused pane with its view, then closes the modal", () => {
  const { view, calls } = diffView({ ...spec, overlay: "o1" }, null);
  view.keep();
  expect(calls[0][1]).toBe("pane.split");
  const props = (calls[0][2] as { props: Record<string, unknown> }).props;
  expect(props).toMatchObject({ cwd: "/r", path: "a.txt", mode: "unstaged", layout: "unified", whitespace: "trim", context: -1 });
  expect(props.overlay).toBeUndefined();
  expect(props.at).toBeGreaterThan(spec.at);
  expect(calls[1]).toEqual(["closeOverlay", "o1"]);
});
