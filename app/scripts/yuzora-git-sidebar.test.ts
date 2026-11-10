// yuzora(v0.1.0 E1): the Git sidebar follows the place it is shown at, and what it does is done to
// the repo it shows. main.js runs as shipped with herdr, yuzora:git and the element builders
// stubbed: every git call waits to be answered, and a frame is the tree the render built.
// Run: bun test scripts/yuzora-git-sidebar.test.ts
import { test, expect } from "bun:test";
import { readFile } from "node:fs/promises";

// SIDEBAR_GIT_MAIN: another copy of main.js to hold the tests against (a mutation, the code before a fix).
const main = await readFile(process.env.SIDEBAR_GIT_MAIN ?? new URL("../defaults/mods/yuzora-sidebar-git/main.js", import.meta.url), "utf8");
const code = main.replace(/^import [\s\S]*?;\n/gm, "").replace(/^export default class/m, "class");

const DATA = Symbol("node");
type Node = { id?: string; texts: string[]; kids: Node[]; handlers: Record<string, (...a: any[]) => any>; keys?: string[] };
const flush = async () => { for (let i = 0; i < 40; i++) await Promise.resolve(); };

function builder(): any {
  const n: Node = { texts: [], kids: [], handlers: {} };
  const add = (c: any) => { if (typeof c === "string") n.texts.push(c); else if (c?.[DATA]) n.kids.push(c[DATA]); };
  const self: any = new Proxy({}, {
    get: (_t, prop) => {
      if (prop === DATA) return n;
      if (prop === "then") return undefined;
      return (...args: any[]) => {
        if (prop === "id") n.id = args[0];
        else if (prop === "child") add(args[0]);
        else if (prop === "children") args[0].forEach(add);
        else if (prop === "when") { if (args[0]) args[1](self); }
        else if (String(prop).startsWith("on_") && typeof args.at(-1) === "function") n.handlers[prop as string] = args.at(-1);
        return self;
      };
    },
  });
  return self;
}

const walk = (n: Node, out: Node[] = []): Node[] => { out.push(n); n.kids.forEach((k) => walk(k, out)); return out; };

type Gate = { args: any[]; resolve: (v: any) => void; reject: (e: any) => void; done: boolean };
const NAMES = ["open", "status", "changed", "messages", "stage", "unstage", "discard", "commitChecks", "commit", "push", "pushPreview", "sequencer", "fetch", "trust", "repos"] as const;

const ROOT_OF: Record<string, string> = { "/a": "/a", "/a/sub": "/a", "/b": "/b", "/c": "/c" };
const info = (root: string) => ({ id: `:${root}`, root, machine: null, gitDir: `${root}/.git`, commonDir: `${root}/.git` });
const file = (path: string) => ({ path, kind: "M", add: 1, del: 0, binary: false });
// What each repo has changed: unstaged files, staged files.
const CHANGES: Record<string, { unstaged: string[]; staged?: string[]; upstream?: string | null }> = {
  "/a": { unstaged: ["x.txt", "only-a.txt"] },
  "/b": { unstaged: ["x.txt"], staged: ["y.txt"] },
  "/c": { unstaged: [] },
};
const statusOf = (root: string) => {
  const c = CHANGES[root] ?? { unstaged: [] };
  const staged = (c.staged ?? []).map(file), unstaged = c.unstaged.map(file);
  return {
    repo: `:${root}`, root, machine: null, generation: 1,
    branch: { head: "main", oid: "abc", upstream: null, ahead: 0, behind: 0, detached: false, initial: false },
    operation: { kind: "none", step: null, onto: null, head: null, interactive: false },
    staged, unstaged, untracked: [], conflicts: [],
    counts: { staged: staged.length, unstaged: unstaged.length, untracked: 0, conflicts: 0, total: staged.length + unstaged.length },
    stash: 0, truncated: false, numstatSkipped: false,
  };
};

function world(settings: object = {}) {
  const w = {
    cwd: "/a", pinned: null as null | { root: string; on: null },
    shared: new Map<string, any>(), toasts: [] as any[], overlays: [] as any[], timers: [] as { ms: number; fn: (cx: any) => void }[],
    asks: Object.fromEntries(NAMES.map((n) => [n, [] as Gate[]])) as Record<(typeof NAMES)[number], Gate[]>,
    box: { value: "", handlers: {} as Record<string, (...a: any[]) => void> },
    author: { value: "" },
  };
  const ask = (name: (typeof NAMES)[number], args: any[]) => new Promise((resolve, reject) => { w.asks[name].push({ args, resolve, reject, done: false }); });
  const pending = (name: (typeof NAMES)[number], on?: (g: Gate) => boolean) => w.asks[name].filter((g) => !g.done && (!on || on(g)));
  const answer = async (g: Gate, value: any) => { g.done = true; g.resolve(value); await flush(); };
  const fail = async (g: Gate, error: any) => { g.done = true; g.reject(error); await flush(); };
  // Answers every open, status and messages that waits, the way a repo would, until none does.
  const settle = async () => {
    for (let turn = 0; turn < 20; turn++) {
      await flush();
      const calls = [...pending("open"), ...pending("status"), ...pending("messages")];
      if (!calls.length) return;
      for (const g of calls) {
        if (w.asks.open.includes(g)) await answer(g, info(ROOT_OF[g.args[0]]));
        else if (w.asks.status.includes(g)) await answer(g, statusOf(String(g.args[0]).slice(1)));
        else await answer(g, { history: [], template: null, last: null });
      }
    }
    throw new Error("git is still being asked");
  };
  const stubs: Record<string, any> = {
    View: class {},
    div: builder, svg: builder, v_flex: builder, h_flex: builder, Input: { new: builder }, Textarea: { new: builder }, Scrollbar: { vertical: builder },
    v_virtual_list: (id: string, count: number, _sizes: number[], keyOf: (i: number) => string, rows: (r: { start: number; end: number }) => any[]) => {
      const list = builder(), n = list[DATA] as Node;
      n.id = id; n.keys = Array.from({ length: count }, (_, i) => keyOf(i));
      for (const r of rows({ start: 0, end: count })) n.kids.push(r[DATA]);
      return list;
    },
    TextareaState: { new: () => ({ set_auto_grow() {}, on: (name: string, fn: any) => { w.box.handlers[name] = fn; }, value: () => w.box.value, set_value: (v: string) => { w.box.value = v; } }) },
    InputState: { new: () => ({ on() {}, value: () => w.author.value, set_value: (v: string) => { w.author.value = v; } }) },
    spaces: () => [{ id: "s1", active: true, folder: "/a", machine: null, tabs: [{ id: "t1" }] }],
    panes: () => [{ pane: "p1", tab: "t1", cwd: w.cwd }],
    focused: () => ({ space: "s1", tab: "t1", pane: "p1", cwd: null }),
    theme: () => new Proxy({}, { get: () => "#000000" }), tokens: () => ({}),
    run() {}, openOverlay: (o: any) => w.overlays.push(o), closeOverlay() {}, notify: (n: any) => w.toasts.push(n),
    shared: (k: string) => (k === "yuzora.git.repo.s1" ? w.pinned : w.shared.get(k) ?? null),
    setShared: (k: string, v: any) => { w.shared.set(k, v); }, props: () => null, nextCommand: () => new Promise(() => {}),
    t: (k: string) => k, gitSettings: () => ({ showPushDialog: false, ...settings }),
    open: (cwd: string, on: string | null) => ask("open", [cwd, on]),
    status: (id: string, o: object) => ask("status", [id, o]),
    changed: (id: string, gen: number) => ask("changed", [id, gen]),
    messages: (id: string) => ask("messages", [id]),
    stage: (id: string, paths: string[]) => ask("stage", [id, paths]),
    unstage: (id: string, paths: string[]) => ask("unstage", [id, paths]),
    discard: (id: string, paths: string[]) => ask("discard", [id, paths]),
    commitChecks: (id: string, amend: boolean) => ask("commitChecks", [id, amend]),
    commit: (id: string, o: any) => ask("commit", [id, o]),
    push: (id: string, o: any) => ask("push", [id, o]),
    pushPreview: (id: string) => ask("pushPreview", [id]),
    sequencer: (id: string, a: string) => ask("sequencer", [id, a]),
    fetch: (id: string, o: any) => ask("fetch", [id, o]),
    trust: (id: string) => ask("trust", [id]),
    repos: (...a: any[]) => ask("repos", a),
  };
  const SidebarGit = new Function(...Object.keys(stubs), `${code}\nreturn SidebarGit;`)(...Object.values(stubs));
  const cx: any = {
    spawn: (fn: (cx: any) => Promise<any>) => { fn(cx); },
    notify() {}, focus_handle: () => ({ focus() {} }), stop_propagation() {}, write_to_clipboard() {},
    timer: { every: (ms: number, fn: any) => { w.timers.push({ ms, fn }); }, after() {} },
  };
  const view = new SidebarGit();
  view.init(null, cx);

  const frame = () => {
    const root = (view.render(cx) as any)[DATA] as Node;
    const nodes = walk(root);
    const list = nodes.find((n) => n.keys);
    const texts = nodes.flatMap((n) => n.texts);
    // The list's rows as drawn: the key the list gave each, the section it is in, and the file it shows.
    let section = "";
    const rows = (list?.kids ?? []).map((row, i) => {
      const inner = walk(row), name = inner.find((n) => n.id === "name")?.texts[0], dir = inner.find((n) => n.id === "dir")?.texts[0];
      const head = inner.flatMap((n) => n.texts).map((x) => /^section\.(\w+) · \d+$/.exec(x)?.[1]).find(Boolean);
      if (head) section = head;
      return { key: list!.keys![i], section, path: name ? (dir ? `${dir}/${name}` : name) : null };
    });
    return {
      root, nodes, texts, list: list ?? null, rows,
      files: rows.filter((r) => r.path).map((r) => r.path as string),
      keyOf: (path: string, section = "unstaged") => rows.find((r) => r.path === path && r.section === section)!.key,
      button: (label: string) => nodes.find((n) => n.texts.includes(label)) ?? null,
    };
  };
  // The list's own press: where in it the mouse went down (counted from the left), and how wide it is.
  const aim = (f: ReturnType<typeof frame>, slot: number) => {
    const git = f.nodes.find((n) => n.id === "git-list")!;
    git.handlers.on_size_change({ width: 300 });
    git.handlers.on_mouse_down({ local_position: { x: 300 - 6 - 22 * slot - 3 } });
  };
  const press = (f: ReturnType<typeof frame>, key: string) => f.list!.handlers.on_item_click(key, cx);
  const type = (text: string) => { w.box.value = text; w.box.handlers.change?.({}, cx); };
  const tick = () => w.timers.filter((t) => t.ms === 1000).forEach((t) => t.fn(cx));
  const go = (cwd: string) => { w.cwd = cwd; };
  const badge = () => w.shared.get("yuzora.badge.git") ?? null;
  return { w, view, cx, frame, aim, press, type, tick, go, settle, pending, answer, fail, badge };
}

test("a new place is followed in the next frame, without waiting for the sidebar's timer", async () => {
  const s = world();
  await s.settle();
  expect(s.frame().files).toEqual(["x.txt", "only-a.txt"]);
  s.go("/b");
  s.frame();
  expect(s.pending("open").map((g) => g.args[0])).toEqual(["/b"]);
});

test("the timer still follows a place for a copy that is not drawn", async () => {
  const s = world();
  await s.settle();
  s.go("/b");
  s.tick();
  expect(s.pending("open").map((g) => g.args[0])).toEqual(["/b"]);
});

test("a place seen before shows what its repo had at the first frame, and reads it again behind it", async () => {
  const s = world();
  await s.settle();
  s.go("/b");
  s.frame();
  await s.settle();
  expect(s.frame().files).toEqual(["y.txt", "x.txt"]);
  s.go("/a");
  const back = s.frame();
  expect(back.files).toEqual(["x.txt", "only-a.txt"]);
  expect(back.texts).not.toContain("loading");
  expect(s.pending("open").map((g) => g.args[0])).toEqual(["/a"]);
  await s.settle();
  expect(s.pending("status").length + s.w.asks.status.filter((g) => g.args[0] === ":/a").length).toBeGreaterThan(1);
});

test("a place never seen shows nothing of the repo it left once its own repo is known", async () => {
  const s = world();
  await s.settle();
  s.go("/c");
  s.frame();
  await s.answer(s.pending("open")[0], info("/c"));
  const f = s.frame();
  expect(f.files).toEqual([]);
  expect(f.texts).toContain("loading");
});

test("moving to another folder of the repo shown keeps its list and reads nothing again", async () => {
  const s = world();
  await s.settle();
  const reads = () => ["status", "messages"].map((n) => s.w.asks[n as "status"].length);
  const before = reads();
  s.go("/a/sub");
  s.frame();
  s.tick();
  const f = s.frame();
  expect(f.files).toEqual(["x.txt", "only-a.txt"]);
  expect(f.texts).not.toContain("loading");
  await s.settle();
  expect(s.frame().files).toEqual(["x.txt", "only-a.txt"]);
  expect(reads()).toEqual(before);
  s.go("/a");
  s.frame();
  await s.settle();
  expect(reads()).toEqual(before);
});

test("a place that is no repo says so, and the repo it left is shown again when the place is", async () => {
  const s = world();
  await s.settle();
  s.go("/b");
  s.frame();
  await s.settle();
  s.go("/nowhere");
  s.frame();
  await s.fail(s.pending("open")[0], new Error("fatal: not a git repository"));
  const f = s.frame();
  expect(f.files).toEqual([]);
  expect(f.texts).toContain("notRepo");
  s.go("/a");
  expect(s.frame().files).toEqual(["x.txt", "only-a.txt"]);
});

test("a click on a row drawn for the repo left does nothing to the repo shown now, even for the same path", async () => {
  const s = world();
  await s.settle();
  const before = s.frame();
  s.aim(before, 0);
  const stale = before.keyOf("x.txt");
  s.go("/b");
  s.frame();
  await s.settle();
  const now = s.frame();
  expect(now.files).toContain("x.txt");
  s.aim(now, 0);
  s.press(now, stale);
  await flush();
  expect(s.w.asks.stage).toEqual([]);
  s.press(now, now.keyOf("x.txt"));
  await flush();
  expect(s.w.asks.stage.map((g) => g.args)).toEqual([[":/b", ["x.txt"]]]);
});

test("a stage, unstage or discard goes to the repo whose list it was clicked in", async () => {
  const s = world();
  await s.settle();
  s.go("/b");
  s.frame();
  await s.settle();
  const f = s.frame();
  s.aim(f, 0);
  s.press(f, f.keyOf("y.txt", "staged"));
  await flush();
  expect(s.w.asks.unstage.map((g) => g.args)).toEqual([[":/b", ["y.txt"]]]);
  s.aim(f, 1);
  s.press(f, f.keyOf("x.txt"));
  await flush();
  expect(s.frame().texts).toContain("discard.ask");
  s.go("/a");
  s.frame();
  await s.settle();
  const a = s.frame();
  expect(a.texts).not.toContain("discard.ask");
  expect(s.w.asks.discard).toEqual([]);
});

test("a status that comes late for the repo left changes neither the repo shown nor the badge", async () => {
  const s = world();
  await flush();
  const lateA = s.pending("open")[0];
  await s.answer(lateA, info("/a"));
  const slow = s.pending("status")[0];
  s.go("/b");
  s.frame();
  await s.settle();
  expect(s.frame().files).toEqual(["y.txt", "x.txt"]);
  expect(s.badge()).toBe(2);
  await s.answer(slow, statusOf("/a"));
  expect(s.frame().files).toEqual(["y.txt", "x.txt"]);
  expect(s.badge()).toBe(2);
  s.go("/a");
  expect(s.frame().files).toEqual(["x.txt", "only-a.txt"]);
});

test("an open that comes late for a place left does not take the screen back", async () => {
  const s = world();
  const lateA = s.pending("open")[0];
  s.go("/b");
  s.frame();
  await s.answer(s.pending("open", (g) => g.args[0] === "/b")[0], info("/b"));
  await s.answer(lateA, info("/a"));
  await s.settle();
  expect(s.frame().files).toEqual(["y.txt", "x.txt"]);
  expect(s.w.asks.status.map((g) => g.args[0])).toEqual([":/b"]);
});

test("the message typed in one repo is not the message of another", async () => {
  const s = world();
  await s.settle();
  s.type("alpha message");
  expect(s.frame().button("commit.button")!.handlers.on_click).toBeDefined();
  s.go("/b");
  s.frame();
  await s.settle();
  expect(s.w.box.value).toBe("");
  expect(s.frame().button("commit.button")!.handlers.on_click).toBeUndefined();
  s.type("beta message");
  s.go("/a");
  s.frame();
  await s.settle();
  expect(s.w.box.value).toBe("alpha message");
  s.go("/b");
  s.frame();
  expect(s.w.box.value).toBe("beta message");
});

test("the author typed for one repo's commit is not used for another's", async () => {
  const s = world();
  await s.settle();
  s.w.author.value = "Mallory <m@example.com>";
  s.go("/b");
  s.frame();
  await s.settle();
  expect(s.w.author.value).toBe("");
  s.type("beta message");
  s.frame().button("commit.button")!.handlers.on_click({}, s.cx);
  await flush();
  await s.answer(s.pending("commitChecks")[0], []);
  expect(s.w.asks.commit[0].args[1].author).toBeUndefined();
});

test("a commit finishing after the sidebar moved on keeps its result and its message box to its own repo", async () => {
  const s = world();
  await s.settle();
  s.type("alpha message");
  s.frame().button("commit.button")!.handlers.on_click({}, s.cx);
  await flush();
  await s.answer(s.pending("commitChecks")[0], []);
  expect(s.w.asks.commit.map((g) => g.args)).toEqual([[":/a", { message: "alpha message", amend: false, signOff: false, noVerify: false, author: undefined }]]);
  s.go("/b");
  s.frame();
  await s.settle();
  s.type("beta message");
  await s.answer(s.pending("commit")[0], { ok: false, output: "hook says no" });
  const b = s.frame();
  expect(b.texts).not.toContain("hook says no");
  expect(s.w.box.value).toBe("beta message");
  s.go("/a");
  s.frame();
  await s.settle();
  expect(s.frame().texts).toContain("hook says no");
  expect(s.w.box.value).toBe("alpha message");
});

test("a commit that went through empties its own repo's message box, not the one shown", async () => {
  const s = world();
  await s.settle();
  s.type("alpha message");
  s.frame().button("commit.button")!.handlers.on_click({}, s.cx);
  await flush();
  await s.answer(s.pending("commitChecks")[0], []);
  s.go("/b");
  s.frame();
  await s.settle();
  s.type("beta message");
  await s.answer(s.pending("commit")[0], { ok: true });
  expect(s.w.box.value).toBe("beta message");
  expect(s.toasts?.length ?? s.w.toasts.length).toBe(1);
  s.go("/a");
  s.frame();
  await s.settle();
  expect(s.w.box.value).toBe("");
});

test("the warnings of a commit are the repo's that asked, and so is its busy button", async () => {
  const s = world();
  await s.settle();
  s.type("alpha message");
  s.frame().button("commit.button")!.handlers.on_click({}, s.cx);
  await flush();
  await s.answer(s.pending("commitChecks")[0], [{ kind: "todo", files: ["x.txt"] }]);
  expect(s.frame().texts).toContain("commit.anyway");
  s.go("/b");
  s.frame();
  await s.settle();
  expect(s.frame().texts).not.toContain("commit.anyway");
  s.go("/a");
  s.frame();
  await s.settle();
  expect(s.frame().texts).toContain("commit.anyway");
});

test("a push going on in one repo does not hold back the buttons of another", async () => {
  const s = world();
  await s.settle();
  s.frame().button("push")!.handlers.on_click({}, s.cx);
  await flush();
  expect(s.pending("pushPreview").length).toBe(1);
  s.go("/b");
  s.frame();
  await s.settle();
  expect(s.frame().button("push")!.handlers.on_click).toBeDefined();
  s.frame().button("push")!.handlers.on_click({}, s.cx);
  await flush();
  expect(s.pending("pushPreview").map((g) => g.args[0]).sort()).toEqual([":/a", ":/b"]);
});

test("the sidebar keeps a bounded number of repos to show again", async () => {
  const s = world();
  const roots = Array.from({ length: 40 }, (_, i) => `/r${i}`);
  for (const r of roots) { ROOT_OF[r] = r; CHANGES[r] = { unstaged: [`${r.slice(1)}.txt`] }; }
  await s.settle();
  for (const r of roots) { s.go(r); s.frame(); await s.settle(); }
  s.go(roots[0]);
  s.frame();
  await s.answer(s.pending("open")[0], info(roots[0]));
  expect(s.frame().texts).toContain("loading");
  s.go(roots.at(-1)!);
  expect(s.frame().files).toEqual([`${roots.at(-1)!.slice(1)}.txt`]);
});
