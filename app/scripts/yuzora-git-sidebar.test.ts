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

const ROOT_OF: Record<string, string> = { "/a": "/a", "/a/sub": "/a", "/b": "/b", "/b/sub": "/b", "/c": "/c" };
const PIN = "yuzora.git.repo.s1";
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

function world(settings: object = {}, props: any = null) {
  const w = {
    cwd: "/a", props, pinned: null as null | { root: string; on: null },
    // What a repo's status says beyond statusOf, by root; the messages every repo answers with.
    status: {} as Record<string, object>, history: [] as string[], template: null as string | null,
    shared: new Map<string, any>(), toasts: [] as any[], overlays: [] as any[], runs: [] as any[], timers: [] as { ms: number; fn: (cx: any) => void }[],
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
        else if (w.asks.status.includes(g)) await answer(g, statusFor(String(g.args[0]).slice(1)));
        else await answer(g, { history: w.history, template: w.template, last: null });
      }
    }
    throw new Error("git is still being asked");
  };
  const statusFor = (root: string) => ({ ...statusOf(root), ...(w.status[root] ?? {}) });
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
    run: (...a: any[]) => { w.runs.push(a); }, openOverlay: (o: any) => w.overlays.push(o), closeOverlay() {}, notify: (n: any) => w.toasts.push(n),
    shared: (k: string) => (w.shared.has(k) ? w.shared.get(k) : k === PIN ? w.pinned : null),
    setShared: (k: string, v: any) => { w.shared.set(k, v); }, props: () => w.props, nextCommand: () => new Promise(() => {}),
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
  const submit = () => w.box.handlers.submit?.({ modifiers: { platform: true } }, cx);
  const pin = (root: string | null) => { w.shared.set(PIN, root ? { root, on: null } : null); };
  return { w, view, cx, frame, aim, press, type, tick, go, settle, pending, answer, fail, badge, submit, pin };
}

test("the status bar's line counts one change in the singular and more in the plural", async () => {
  const s = world();
  s.go("/b");
  await s.settle();
  s.frame();
  expect(s.w.shared.get("yuzora.status.git").text).toBe("⎇ main · status.changes_other");
  s.w.status["/b"] = { staged: [], unstaged: [file("x.txt")], counts: { staged: 0, unstaged: 1, untracked: 0, conflicts: 0, total: 1 }, generation: 2 };
  s.go("/a"); s.frame(); await s.settle();
  s.go("/b"); s.frame(); await s.settle(); s.frame();
  expect(s.w.shared.get("yuzora.status.git").text).toBe("⎇ main · status.changes_one");
});

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
  s.go("/a");
  s.frame();
  expect(s.w.author.value).toBe("Mallory <m@example.com>");
  s.go("/b");
  s.frame();
  await s.settle();
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

test("the warnings a commit's checks raise are the repo's that asked, even when they arrive after it left", async () => {
  const s = world();
  await s.settle();
  s.type("alpha message");
  s.frame().button("commit.button")!.handlers.on_click({}, s.cx);
  await flush();
  s.go("/b");
  s.frame();
  await s.settle();
  await s.answer(s.pending("commitChecks")[0], [{ kind: "todo", files: ["x.txt"] }]);
  expect(s.frame().texts).not.toContain("commit.anyway");
  s.go("/a");
  s.frame();
  await s.settle();
  expect(s.frame().texts).toContain("commit.anyway");
});

test("the error of a stage that fails after the sidebar moved on is shown with the repo it was for", async () => {
  const s = world();
  await s.settle();
  const f = s.frame();
  s.aim(f, 0);
  s.press(f, f.keyOf("x.txt"));
  await flush();
  s.go("/b");
  s.frame();
  await s.settle();
  await s.fail(s.pending("stage")[0], new Error("index.lock exists"));
  await s.settle();
  expect(s.frame().texts.join("\n")).not.toContain("index.lock exists");
  s.go("/a");
  s.frame();
  await s.settle();
  expect(s.frame().texts.join("\n")).toContain("index.lock exists");
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

test("a repo whose status cannot be read when it is shown again shows the error, not the list it had", async () => {
  const s = world();
  await s.settle();
  s.go("/b");
  s.frame();
  await s.settle();
  s.go("/a");
  expect(s.frame().files).toEqual(["x.txt", "only-a.txt"]);
  await s.settle();
  s.go("/b");
  s.frame();
  await s.settle();
  s.go("/a");
  s.frame();
  await s.answer(s.pending("open")[0], info("/a"));
  await s.fail(s.pending("status")[0], new Error("fatal: cannot change to '/a'"));
  const f = s.frame();
  expect(f.files).toEqual([]);
  expect(f.texts).toContain("Error: fatal: cannot change to '/a'");
});

test("a repo that is no longer shown is not read again when it changes", async () => {
  const s = world();
  await s.settle();
  const watching = s.pending("changed").map((g) => g.args[0]);
  expect(watching).toEqual([":/a"]);
  s.go("/b");
  s.frame();
  await s.settle();
  const reads = s.w.asks.status.length;
  await s.answer(s.pending("changed", (g) => g.args[0] === ":/a")[0], 2);
  await s.settle();
  expect(s.w.asks.status.length).toBe(reads);
  expect(s.pending("changed").map((g) => g.args[0])).toEqual([":/b"]);
});

test("an action on the repo shown while the next place is still being named is done to the repo shown", async () => {
  const s = world();
  await s.settle();
  const f = s.frame();
  s.go("/c");
  s.frame();
  s.aim(f, 0);
  s.press(f, f.keyOf("x.txt"));
  await flush();
  expect(s.w.asks.stage.map((g) => g.args)).toEqual([[":/a", ["x.txt"]]]);
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

// ---- v0.1.0 E1 r1: what is on screen is what a click acts on ----------------------------------------------------

type S = ReturnType<typeof world>;
type F = ReturnType<S["frame"]>;
const WRITES = ["stage", "unstage", "discard", "commitChecks", "commit", "push", "pushPreview", "sequencer", "fetch", "trust"] as const;
const writes = (s: S) => WRITES.flatMap((n) => s.w.asks[n].map((g) => `${n} ${JSON.stringify(g.args)}`));
// What a click has done to the world outside the sidebar: git writes, panes and popups opened, toasts.
const effects = (s: S) => JSON.stringify([writes(s), s.w.overlays, s.w.runs, s.w.toasts]);
// A control by its name: the id it is drawn with starts with it (and names its repo).
const named = (f: F, name: string) => f.nodes.find((n) => n.id === name || n.id?.startsWith(`${name}:`))!;
const click = (s: S, f: F, name: string) => named(f, name).handlers.on_click({}, s.cx);
const clickable = (f: F) => f.nodes.filter((n) => n.handlers.on_click).map((n) => n.id ?? "(no id)");

// Every handler a frame has, called the way the window calls them from that frame: each click, each row's click in
// every slot and its right click, and ⌘Enter in the message box.
function fire(s: S, f: F) {
  for (const n of f.nodes) n.handlers.on_click?.({}, s.cx);
  for (const key of f.list?.keys ?? []) {
    for (const slot of [0, 1, 2]) {
      s.aim(f, slot);
      f.list!.handlers.on_item_click(key, s.cx);
    }
    f.list!.handlers.on_item_secondary_click(key, {}, s.cx);
  }
  s.submit();
}

async function visit(s: S, count: number, tag: string) {
  for (let i = 0; i < count; i++) {
    const r = `/${tag}${i}`;
    ROOT_OF[r] = r; CHANGES[r] = { unstaged: [`${tag}${i}.txt`] };
    s.go(r); s.frame(); await s.settle();
  }
}

// A frame of repo A with every control the sidebar draws: the commit box with its history, author and
// warnings, a discard asked about, and the operation bar (with conflicts to resolve).
async function crowded(s: S, operation = "merging") {
  s.w.status["/a"] = { operation: { kind: operation, step: null, onto: "main", head: null, interactive: false }, conflicts: [file("c.txt")] };
  s.w.history = ["one", "two"];
  await s.settle();
  s.go("/b"); s.frame(); await s.settle();
  s.type("beta draft");
  s.go("/a"); s.frame(); await s.settle();
  s.type("alpha message");
  click(s, s.frame(), "more");
  click(s, s.frame(), "history-toggle");
  const f = s.frame();
  s.aim(f, 1); s.press(f, f.keyOf("x.txt"));
  click(s, s.frame(), "commit");
  await flush();
  await s.answer(s.pending("commitChecks")[0], [{ kind: "todo", files: ["x.txt"] }]);
  return s.frame();
}

// The window still shows `onScreen`, drawn for the repo it was, when `open` brings another repo's view to the screen
// outside a draw, and that repo's status is read: nothing is drawn after.
async function takenOver(s: S) {
  s.go("/b/sub");
  const onScreen = s.frame();
  await s.answer(s.pending("open", (g) => g.args[0] === "/b/sub")[0], info("/b"));
  await s.settle();
  return onScreen;
}

test("every control of a frame does nothing once another repo took the screen, whatever repo's frame it was", async () => {
  for (const operation of ["merging", "rebasing"]) {
    const s = world();
    const onScreen = await crowded(s, operation);
    const want = ["repo-pick", "graph", "branch", "amend", "signoff", "noverify", "history-toggle", "more", "m-0", "m-1", "commit", "commit-push", "push",
      "w-yes", "w-no", "discard-yes", "discard-no", "op-resolve", "op-continue", "op-abort", ...(operation === "merging" ? [] : ["op-skip"])];
    const ids = clickable(onScreen);
    for (const name of want) expect(ids.filter((id) => id === name || id.startsWith(`${name}:`)).length).toBe(1);
    expect(ids.length).toBe(want.length);
    expect(onScreen.rows.length).toBeGreaterThan(3);
    await takenOver(s);
    expect(s.w.box.value).toBe("beta draft");
    const before = effects(s), notes = s.view.v.note, amend = s.view.v.draft.amend;
    fire(s, onScreen);
    await flush();
    expect(effects(s)).toBe(before);
    expect(s.w.box.value).toBe("beta draft");
    expect([s.view.v.note, s.view.v.draft.amend, s.view.v.busy]).toEqual([notes, amend, null]);
    expect(s.w.asks.commitChecks.length).toBe(1);
  }
});

test("every control of a frame names its repo, so a press in one repo's frame and a release in another's are not one click", async () => {
  const s = world();
  const a = await crowded(s);
  const mine = clickable(a);
  expect(mine.length).toBeGreaterThan(15);
  for (const id of mine) expect(id.endsWith(":" + ":/a")).toBe(true);
  s.go("/b"); s.frame(); await s.settle();
  for (const id of clickable(s.frame())) expect(id.endsWith(":" + ":/b")).toBe(true);
  expect(clickable(s.frame()).filter((id) => mine.includes(id))).toEqual([]);
});

test("Commit drawn for one repo does not commit another's draft when the click lands before the next frame", async () => {
  const s = world();
  await s.settle();
  s.go("/b"); s.frame(); await s.settle();
  s.type("beta draft");
  s.go("/a"); s.frame(); await s.settle();
  s.type("alpha message");
  const onScreen = await takenOver(s);
  click(s, onScreen, "commit");
  await flush();
  expect(s.pending("commitChecks")).toEqual([]);
  expect(s.w.asks.commit).toEqual([]);
});

test("Commit anyway, Push and the operation bar drawn for one repo do not act on another repo", async () => {
  const s = world();
  const onScreen = await crowded(s);
  await takenOver(s);
  const before = effects(s);
  for (const name of ["w-yes", "push", "commit-push", "op-abort", "op-continue", "amend"]) click(s, onScreen, name);
  await flush();
  expect(effects(s)).toBe(before);
  expect(s.w.asks.commit).toEqual([]);
  expect(s.view.v.draft.amend).toBe(false);
});

test("⌘Enter in the message box commits the repo whose box was on screen, not the one that took the screen after it", async () => {
  const s = world();
  await s.settle();
  s.go("/b"); s.frame(); await s.settle();
  s.type("beta draft");
  s.go("/a"); s.frame(); await s.settle();
  s.type("alpha message");
  s.frame();
  s.go("/b/sub");
  s.frame();
  await s.answer(s.pending("open", (g) => g.args[0] === "/b/sub")[0], info("/b"));
  await s.settle();
  s.submit();
  await flush();
  expect(s.w.asks.commitChecks).toEqual([]);
  s.frame();
  s.submit();
  await flush();
  expect(s.w.asks.commitChecks.map((g) => g.args)).toEqual([[":/b", false]]);
});

test("the amend ticked for one repo is not ticked for another, and the other's commit is not an amend", async () => {
  const s = world();
  await s.settle();
  click(s, s.frame(), "amend");
  expect(s.frame().texts).toContain("commit.amendButton");
  s.go("/b"); s.frame(); await s.settle();
  expect(s.frame().texts).not.toContain("commit.amendButton");
  s.type("beta");
  s.frame().button("commit.button")!.handlers.on_click({}, s.cx);
  await flush();
  expect(s.pending("commitChecks")[0].args).toEqual([":/b", false]);
  s.go("/a"); s.frame(); await s.settle();
  expect(s.frame().texts).toContain("commit.amendButton");
});

// ---- what a repo holds is not let go of -------------------------------------------------------------------------

test("a commit message, an author, or an amend typed for a repo is there when it is shown again, after any number of other repos", async () => {
  for (const kind of ["message", "author", "amend"]) {
    const s = world();
    await s.settle();
    if (kind === "message") s.type("feat: a long message I wrote for A");
    if (kind === "author") s.w.author.value = "Pair <p@example.com>";
    if (kind === "amend") click(s, s.frame(), "amend");
    await visit(s, 12, `keep-${kind}-`);
    s.go("/a"); s.frame(); await s.settle();
    expect([kind, s.w.box.value, s.w.author.value, s.frame().texts.includes("commit.amendButton")]).toEqual([
      kind, kind === "message" ? "feat: a long message I wrote for A" : "", kind === "author" ? "Pair <p@example.com>" : "", kind === "amend"]);
  }
});

test("a repo whose push is going on, or whose commit waits on a question, is not let go of either", async () => {
  const s = world();
  await s.settle();
  s.frame().button("push")!.handlers.on_click({}, s.cx);
  await flush();
  await visit(s, 12, "busy");
  await s.fail(s.pending("pushPreview")[0], new Error("rejected"));
  s.go("/a"); s.frame(); await s.settle();
  expect(s.frame().texts).toContain("Error: rejected");

  const q = world();
  await q.settle();
  q.type("alpha message");
  q.frame().button("commit.button")!.handlers.on_click({}, q.cx);
  await flush();
  await q.answer(q.pending("commitChecks")[0], [{ kind: "todo", files: ["x.txt"] }]);
  q.type("");
  await visit(q, 12, "ask");
  q.go("/a"); q.frame(); await q.settle();
  expect(q.frame().texts).toContain("commit.anyway");
});

test("the repos that hold nothing are still let go of: a message that is only the commit template is not a draft", async () => {
  const s = world();
  s.w.template = "TEMPLATE";
  await s.settle();
  expect(s.w.box.value).toBe("TEMPLATE");
  await visit(s, 12, "tpl");
  s.go("/a");
  s.frame();
  await s.answer(s.pending("open")[0], info("/a"));
  expect(s.frame().texts).toContain("loading");
});

// ---- the waits of an arrival -------------------------------------------------------------------------------------

const aside = async (s: S) => {
  await s.settle();
  s.go("/b"); s.frame(); await s.settle();
  s.go("/a"); s.frame(); await s.settle();
};
const goes = async (s: S, to: string) => { s.pin(null); s.go(to); s.frame(); await s.settle(); };
const there = { repos: [{ rel: ".", root: "/p", name: "p" }], truncated: false };

test("a pinned repo's open that fails does not put its error over the place the person went to meanwhile", async () => {
  for (const reply of ["still there", "cannot be looked at"] as const) {
    const s = world();
    await aside(s);
    s.pin("/p"); s.frame();
    await s.fail(s.pending("open", (g) => g.args[0] === "/p")[0], new Error("fatal: detected dubious ownership"));
    expect(s.pending("repos").length).toBe(1);
    await goes(s, "/b");
    const reads = s.w.asks.status.length;
    if (reply === "still there") await s.answer(s.pending("repos")[0], there);
    else await s.fail(s.pending("repos")[0], new Error("machine away"));
    const f = s.frame();
    expect(f.files).toEqual(["y.txt", "x.txt"]);
    expect(f.texts.join("\n")).not.toMatch(/dubious|machine away/);
    expect(s.badge()).toBe(2);
    await s.answer(s.pending("changed", (g) => g.args[0] === ":/b").at(-1)!, 5);
    await s.settle();
    expect(s.w.asks.status.length).toBe(reads + 1);
  }
});

test("a pinned repo git opened as the one above, being looked at while the person moves on, changes nothing where they went", async () => {
  const s = world();
  await aside(s);
  s.pin("/p"); s.frame();
  await s.answer(s.pending("open", (g) => g.args[0] === "/p")[0], info("/above"));
  expect(s.pending("repos").map((g) => g.args[0])).toEqual(["/above"]);
  await goes(s, "/b");
  await s.answer(s.pending("repos")[0], { repos: [], truncated: false });
  expect(s.pending("repos")).toEqual([]);
  expect(s.w.toasts).toEqual([]);
  expect(s.frame().files).toEqual(["y.txt", "x.txt"]);
});

test("a pinned repo git opened as the one above, found to be pinned there after the person moved on, puts no error where they went", async () => {
  const s = world();
  await aside(s);
  s.pin("/p"); s.frame();
  await s.answer(s.pending("open", (g) => g.args[0] === "/p")[0], info("/above"));
  await s.answer(s.pending("repos")[0], { repos: [], truncated: false });
  expect(s.pending("repos").map((g) => g.args[0])).toEqual(["/p"]);
  s.go("/b");
  s.pin(null);
  s.frame();
  await s.settle();
  await s.answer(s.pending("repos")[0], there);
  const f = s.frame();
  expect(f.files).toEqual(["y.txt", "x.txt"]);
  expect(f.texts).not.toContain("repo.opensAbove");
});

test("a read that fails for the repo shown asks about the place it was shown for, not the one coming", async () => {
  const s = world();
  await s.settle();
  const watch = s.pending("changed")[0];
  s.pin("/p"); s.frame();
  await s.answer(watch, 2);
  await s.fail(s.pending("status", (g) => g.args[0] === ":/a" && g.args[1].force === false).at(-1)!, new Error("fatal: boom"));
  expect(s.pending("repos")).toEqual([]);
  expect(s.w.toasts).toEqual([]);
});

// ---- a repo remembered is shown at once, and trusted only when this visit has read it ------------------------

const WRITE_CONTROLS = ["commit", "commit-push", "push", "op-abort", "op-continue", "w-yes", "discard-yes", "trust"];

test("a repo shown again from memory can be looked at at once, but nothing it draws writes until this visit has read it", async () => {
  const s = world();
  await crowded(s);
  const left = writes(s).length;
  s.go("/b"); s.frame(); await s.settle();
  s.go("/a");
  const back = s.frame();
  expect(back.files).toEqual(["c.txt", "x.txt", "only-a.txt"]);
  expect(back.texts).not.toContain("loading");
  expect(clickable(back).length).toBeGreaterThan(15);
  fire(s, back);
  await flush();
  expect(writes(s).length).toBe(left);
  await s.settle();
  const read = s.frame();
  fire(s, read);
  await flush();
  expect(writes(s).length).toBeGreaterThan(left);
  expect(writes(s).join("\n")).toMatch(/stage|discard|commitChecks|sequencer/);
});

test("a click on a row's buttons, or on Stage all, waits for this visit's read; opening its diff does not", async () => {
  const s = world();
  await s.settle();
  s.go("/b"); s.frame(); await s.settle();
  s.go("/a");
  const back = s.frame();
  s.aim(back, 0); s.press(back, back.keyOf("x.txt"));
  s.aim(back, 1); s.press(back, back.keyOf("x.txt"));
  s.aim(back, 3); s.press(back, back.keyOf("x.txt"));
  await flush();
  expect(s.w.asks.stage).toEqual([]);
  expect(s.frame().texts).not.toContain("discard.ask");
  expect(s.w.overlays.length + s.w.runs.length).toBeGreaterThan(0);
  await s.settle();
  const read = s.frame();
  s.aim(read, 1); s.press(read, read.keyOf("x.txt"));
  expect(s.frame().texts).toContain("discard.ask");
});

test("a repo that now waits for trust shows the notice the moment its status says so, and its remembered list could not be acted on before", async () => {
  const s = world();
  await s.settle();
  s.go("/b"); s.frame(); await s.settle();
  s.go("/a");
  const back = s.frame();
  expect(back.files).toEqual(["x.txt", "only-a.txt"]);
  s.type("alpha");
  s.frame().button("commit.button")!.handlers.on_click({}, s.cx);
  await flush();
  s.aim(back, 0); s.press(back, back.keyOf("x.txt"));
  await flush();
  expect(writes(s)).toEqual([]);
  await s.answer(s.pending("open", (g) => g.args[0] === "/a")[0], info("/a"));
  const status = s.pending("status", (g) => g.args[0] === ":/a");
  await s.answer(status.at(-1)!, { ...statusOf("/a"), trust: ["core.fsmonitor"] });
  const f = s.frame();
  expect(f.texts).toContain("trust.title");
  expect(f.files).toEqual([]);
  expect(s.badge()).toBe(null);
  expect(s.w.shared.get("yuzora.status.git")).toEqual({ text: "⎇ trust.status", tone: "blocked" });
});

test("a trust notice remembered is not answered before this visit has read the folder again", async () => {
  const s = world();
  s.w.status["/a"] = { trust: ["core.fsmonitor"] };
  await s.settle();
  expect(s.frame().texts).toContain("trust.title");
  s.go("/b"); s.frame(); await s.settle();
  s.go("/a");
  const back = s.frame();
  expect(back.texts).toContain("trust.title");
  click(s, back, "trust");
  await flush();
  expect(s.w.asks.trust).toEqual([]);
  await s.settle();
  click(s, s.frame(), "trust");
  await flush();
  expect(s.w.asks.trust.map((g) => g.args)).toEqual([[":/a"]]);
});

test("a status read for an earlier visit that comes back after the repo was shown again does not count as this visit's", async () => {
  const s = world();
  await s.settle();
  s.frame().button("push")!.handlers.on_click({}, s.cx);
  await flush();
  s.go("/b"); s.frame(); await s.settle();
  await s.fail(s.pending("pushPreview")[0], new Error("rejected"));
  const late = s.pending("status", (g) => g.args[0] === ":/a")[0];
  s.go("/a");
  const back = s.frame();
  await s.answer(late, statusOf("/a"));
  s.aim(back, 0); s.press(back, back.keyOf("x.txt"));
  await flush();
  expect(s.w.asks.stage).toEqual([]);
  await s.settle();
  const read = s.frame();
  s.aim(read, 0); s.press(read, read.keyOf("x.txt"));
  await flush();
  expect(s.w.asks.stage.length).toBe(1);
});

// ---- a question asked is the question the person sees ---------------------------------------------------------

test("a discard asked about is not asked again when the repo is shown again", async () => {
  const s = world();
  await s.settle();
  const f = s.frame();
  s.aim(f, 1); s.press(f, f.keyOf("x.txt"));
  expect(s.frame().texts).toContain("discard.ask");
  s.go("/b"); s.frame(); await s.settle();
  s.go("/a");
  const back = s.frame();
  expect(back.texts).not.toContain("discard.ask");
  await s.settle();
  expect(s.frame().texts).not.toContain("discard.ask");
  expect(s.w.asks.discard).toEqual([]);
});

test("a discard asked about is dropped when the file turns out to be another kind of change, and kept while it is the same", async () => {
  const s = world();
  await s.settle();
  const ask = async () => { const f = s.frame(); s.aim(f, 1); s.press(f, f.keyOf("x.txt")); expect(s.frame().texts).toContain("discard.ask"); };
  const refresh = async (st: object) => {
    await s.answer(s.pending("changed")[0], s.w.asks.changed.length + 1);
    await s.answer(s.pending("status")[0], { ...statusOf("/a"), ...st });
  };
  await ask();
  await refresh({ unstaged: [file("x.txt"), file("only-a.txt")].map((x) => ({ ...x, add: 5 })) });
  expect(s.frame().texts).toContain("discard.ask");
  await refresh({ unstaged: [file("only-a.txt")], untracked: [{ path: "x.txt", kind: "?", add: 0, del: 0, binary: false }] });
  expect(s.frame().texts).not.toContain("discard.ask");
  expect(s.w.asks.discard).toEqual([]);
  await ask();
  await refresh({ unstaged: [file("only-a.txt")] });
  expect(s.frame().texts).not.toContain("discard.ask");
});

test("the file menu does not open from a list that has not been read in this visit", async () => {
  const s = world();
  await s.settle();
  s.go("/b"); s.frame(); await s.settle();
  s.go("/a");
  const back = s.frame();
  const key = back.keyOf("x.txt");
  back.list!.handlers.on_item_secondary_click(key, {}, s.cx);
  expect(s.w.overlays).toEqual([]);
  await s.settle();
  const read = s.frame();
  read.list!.handlers.on_item_secondary_click(key, {}, s.cx);
  expect(s.w.overlays.length).toBe(1);
});

// ---- the header says what is shown --------------------------------------------------------------------------------

test("while a place is being opened the header still says what is on screen: its name, and whether that is the pinned one", async () => {
  const s = world();
  await s.settle();
  const header = (f: F) => { const pick = named(f, "repo-pick"); return { dot: pick.kids.length === 3, name: pick.kids.at(-2)!.texts[0] }; };
  expect(header(s.frame())).toEqual({ dot: false, name: "a" });
  s.pin("/p");
  expect(header(s.frame())).toEqual({ dot: false, name: "a" });
  await s.answer(s.pending("open", (g) => g.args[0] === "/p")[0], info("/p"));
  expect(header(s.frame())).toEqual({ dot: true, name: "p" });
});

test("the Yes of a discard asked about discards the file it names, though another was asked about since", async () => {
  const s = world();
  await s.settle();
  const f = s.frame();
  s.aim(f, 1); s.press(f, f.keyOf("x.txt"));
  const asked = s.frame();
  s.aim(asked, 1); s.press(asked, asked.keyOf("only-a.txt"));
  click(s, asked, "discard-yes");
  await flush();
  expect(s.w.asks.discard).toEqual([]);
  expect(s.frame().texts).toContain("discard.ask");
  click(s, s.frame(), "discard-yes");
  await flush();
  await s.answer(s.pending("status").at(-1)!, statusOf("/a"));
  expect(s.w.asks.discard.map((g) => g.args)).toEqual([[":/a", ["only-a.txt"]]]);
});

test("a discard confirmed asks git for the file again, and discards it only if it is still the kind of change asked about", async () => {
  const s = world();
  await s.settle();
  const ask = () => { const f = s.frame(); s.aim(f, 1); s.press(f, f.keyOf("x.txt")); click(s, s.frame(), "discard-yes"); };
  ask();
  await flush();
  const read = s.pending("status").at(-1)!;
  expect(read.args).toEqual([":/a", { force: true }]);
  expect(s.w.asks.discard).toEqual([]);
  await s.answer(read, statusOf("/a"));
  expect(s.w.asks.discard.map((g) => g.args)).toEqual([[":/a", ["x.txt"]]]);
  await s.answer(s.pending("discard")[0], { ok: true });
  await s.settle();

  ask();
  await flush();
  await s.answer(s.pending("status").at(-1)!, { ...statusOf("/a"), unstaged: [file("only-a.txt")], untracked: [{ path: "x.txt", kind: "?", add: 0, del: 0, binary: false }] });
  await s.settle();
  expect(s.w.asks.discard.length).toBe(1);
  const f = s.frame();
  expect(f.texts).toContain("discard.changed");
  expect(f.texts).not.toContain("discard.ask");
  expect(f.rows.find((r) => r.path === "x.txt")).toBeDefined();
});

test("a quiet fetch waits for the repo to be read in this visit, like any other write", async () => {
  const s = world({ autoFetchMinutes: 1 });
  await s.settle();
  const fetchTimer = () => { s.view.fetchedAt = 0; s.w.timers.filter((t) => t.ms === 60000).forEach((t) => t.fn(s.cx)); };
  s.go("/b"); s.frame(); await s.settle();
  s.go("/a"); s.frame();
  fetchTimer();
  await flush();
  expect(s.w.asks.fetch).toEqual([]);
  await s.settle();
  fetchTimer();
  await flush();
  expect(s.w.asks.fetch.map((g) => g.args)).toEqual([[":/a", { auto: true }]]);
});

test("the file menu's Discard reads git again, and discards the file only if it is still the kind of change the menu was opened for", async () => {
  const menu = { cwd: "/a", on: null, section: "unstaged", path: "x.txt", orig: null, kind: "M" };
  for (const then of ["the same", "untracked"]) {
    const s = world({}, { menu, overlay: "o1" });
    const press = () => named(s.frame(), "mi-discard").handlers.on_click({}, s.cx);
    press();
    press();
    await flush();
    await s.answer(s.pending("open")[0], info("/a"));
    const read = s.pending("status")[0];
    expect(read.args).toEqual([":/a", { force: true }]);
    expect(s.w.asks.discard).toEqual([]);
    await s.answer(read, then === "the same" ? statusOf("/a") : { ...statusOf("/a"), unstaged: [file("only-a.txt")], untracked: [{ path: "x.txt", kind: "?", add: 0, del: 0, binary: false }] });
    expect([then, s.w.asks.discard.map((g) => g.args)]).toEqual([then, then === "the same" ? [[":/a", ["x.txt"]]] : []]);
    if (then !== "the same") expect(s.frame().texts).toContain("discard.changed");
  }
});

test("the file menu is opened with the kind of change its file was listed as", async () => {
  const s = world();
  await s.settle();
  const f = s.frame();
  f.list!.handlers.on_item_secondary_click(f.keyOf("x.txt"), {}, s.cx);
  expect(s.w.overlays.at(-1).props.menu).toMatchObject({ path: "x.txt", section: "unstaged", kind: "M" });
});

// ---- v0.1.0 E1 r2: what the second review found ------------------------------------------------------------------

test("the failure of a push or a commit-and-push started in a repo is still there when it is shown again, after any number of other repos", async () => {
  const p = world();
  await p.settle();
  p.frame().button("push")!.handlers.on_click({}, p.cx);
  await flush();
  p.go("/b"); p.frame(); await p.settle();
  await p.fail(p.pending("pushPreview")[0], new Error("rejected"));
  await visit(p, 9, "pushfail");
  p.go("/a"); p.frame(); await p.settle();
  expect(p.frame().texts).toContain("Error: rejected");

  const c = world();
  await c.settle();
  c.type("feat: x");
  c.frame().button("commit.andPush")!.handlers.on_click({}, c.cx);
  await flush();
  await c.answer(c.pending("commitChecks")[0], []);
  c.go("/b"); c.frame(); await c.settle();
  await c.answer(c.pending("commit")[0], { ok: true, output: "" });
  await c.settle();
  await c.fail(c.pending("pushPreview")[0], new Error("! [rejected] main -> main (fetch first)"));
  await c.settle();
  await visit(c, 9, "cpfail");
  c.go("/a"); c.frame(); await c.settle();
  expect(c.frame().texts.join("\n")).toContain("rejected");
});

test("a failure that was seen and gone from the screen does not keep its repo from being let go of once the next action clears it", async () => {
  const s = world();
  await s.settle();
  s.frame().button("push")!.handlers.on_click({}, s.cx);
  await flush();
  await s.fail(s.pending("pushPreview")[0], new Error("rejected"));
  await s.settle();
  expect(s.frame().texts).toContain("Error: rejected");
  s.frame().button("push")!.handlers.on_click({}, s.cx);
  await flush();
  await s.answer(s.pending("pushPreview").at(-1)!, { upstream: "origin/main" });
  await s.answer(s.pending("push")[0], { ok: true, output: "" });
  await s.settle();
  await visit(s, 12, "cleared");
  s.go("/a");
  s.frame();
  await s.answer(s.pending("open")[0], info("/a"));
  expect(s.frame().texts).toContain("loading");
});

test("an untracked file asked about is still asked about after the list is read again, and Yes deletes it once git still lists it as untracked", async () => {
  const s = world();
  const untracked = [{ path: "new.txt", kind: "?", add: 0, del: 0, binary: false }];
  s.w.status["/a"] = { untracked };
  await s.settle();
  const f = s.frame();
  s.aim(f, 1); s.press(f, f.keyOf("new.txt"));
  expect(s.frame().texts).toContain("discard.ask");
  await s.answer(s.pending("changed")[0], 2);
  await s.settle();
  expect(s.frame().texts).toContain("discard.ask");
  click(s, s.frame(), "discard-yes");
  await flush();
  const read = s.pending("status").at(-1)!;
  expect(read.args).toEqual([":/a", { force: true }]);
  expect(s.w.asks.discard).toEqual([]);
  await s.answer(read, { ...statusOf("/a"), untracked });
  expect(s.w.asks.discard.map((g) => g.args)).toEqual([[":/a", ["new.txt"]]]);
});

test("on a repo shown again and not yet read, what only looks still answers: the graph, the branch, the repo picker, the history", async () => {
  const s = world();
  s.w.history = ["one"];
  await s.settle();
  s.go("/b"); s.frame(); await s.settle();
  s.go("/a");
  const back = s.frame();
  expect(s.view.v.confirmed).toBe(false);
  const before = () => s.w.overlays.length + s.w.runs.length;
  for (const name of ["graph", "branch", "repo-pick"]) {
    const was = before();
    click(s, back, name);
    expect([name, before() - was]).toEqual([name, 1]);
  }
  click(s, back, "history-toggle");
  click(s, s.frame(), "m-0");
  expect(s.w.box.value).toBe("one");
});

test("Cancel on the checks' warnings, then Commit anyway from the same frame, does not commit", async () => {
  const s = world();
  await s.settle();
  s.type("feat: x");
  s.frame().button("commit.button")!.handlers.on_click({}, s.cx);
  await flush();
  await s.answer(s.pending("commitChecks")[0], [{ kind: "todo", files: ["x.txt"] }]);
  const f = s.frame();
  expect(f.texts).toContain("commit.anyway");
  click(s, f, "w-no");
  click(s, f, "w-yes");
  await flush();
  expect(s.w.asks.commit).toEqual([]);
  expect(s.w.asks.commitChecks.length).toBe(1);
});

test("a count of lines past 100,000 is cut to thousands and never reads 1000k", async () => {
  const s = world();
  const big = [[99999, "+99999"], [150499, "+150k"], [999499, "+999k"], [999500, "+999k"], [999999, "+999k"], [1000000, "+999k+"]] as const;
  const unstaged = big.map(([add], i) => ({ ...file(`f${i}.txt`), add }));
  s.w.status["/a"] = { unstaged, counts: { staged: 0, unstaged: unstaged.length, untracked: 0, conflicts: 0, total: unstaged.length } };
  await s.settle();
  const shown = s.frame().nodes.filter((n) => n.id === "add").map((n) => n.texts[0]);
  expect(shown).toEqual(big.map(([, text]) => text));
});
