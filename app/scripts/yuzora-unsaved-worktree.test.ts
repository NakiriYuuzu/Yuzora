// yuzora(unsaved): Remove worktree removes nothing while the Space's close waits on the Save / Don't Save / Cancel
// dialog, says why, and removes once the Space has closed. git is real (a repo and a linked worktree in a temp
// folder). The window is a stub: run() only queues a command, and the window runs it on its next turn (`cx.sleep`
// here), where a Space with unsaved files answers space.close with the dialog and stays open.
// Run: bun test scripts/yuzora-unsaved-worktree.test.ts
import { test, expect, afterAll } from "bun:test";
import { readFile, mkdtemp, mkdir, rm, writeFile, realpath } from "node:fs/promises";
import { existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const source = await readFile(new URL("../defaults/mods/worktree/main.js", import.meta.url), "utf8");
const names = [...source.matchAll(/^import\s*\{([^}]*)\}\s*from/gm)].flatMap((m) => m[1].split(",").map((s) => s.trim().split(/\s+as\s+/).pop()!).filter(Boolean));
const body = source.replace(/^import .*;\n/gm, "").replace("export default class Worktree", "class Worktree");

// Elements that keep their text, so a test reads what the dialog shows.
function element() {
  const node = { children: [] as any[] };
  const el: any = new Proxy(node, { get: (_, method) => {
    if (method === "node") return node;
    return (...args: any[]) => {
      if (method === "child") node.children.push(args[0]?.node ?? args[0]);
      if (method === "when" && args[0]) args[1](el);
      return el;
    };
  } });
  return el;
}
const textOf = (node: any): string => typeof node === "string" ? node : (node?.children ?? []).map(textOf).join(" ");

const base = await realpath(await mkdtemp(join(tmpdir(), "yuzora-unsaved-worktree-")));
const home = join(base, "home");
const env = { ...process.env, HOME: home, GIT_CONFIG_NOSYSTEM: "1", GIT_CONFIG_GLOBAL: join(home, ".gitconfig") };
function git(args: string[], cwd: string) {
  const p = Bun.spawnSync(["git", ...args], { cwd, env });
  if (p.exitCode !== 0) throw new Error(`git ${args.join(" ")}: ${p.stderr}`);
}
await mkdir(home, { recursive: true });
await writeFile(join(home, ".gitconfig"), "[user]\n\tname = t\n\temail = t@t\n[commit]\n\tgpgsign = false\n[init]\n\tdefaultBranch = main\n");
afterAll(() => rm(base, { recursive: true, force: true }));

// A repo with a linked worktree on `branch`, and the Remove worktree dialog opened on the Space showing it.
async function removing(branch: string, unsaved: boolean) {
  const repo = join(base, `repo-${branch}`), checkout = join(base, `wt-${branch}`);
  await mkdir(repo, { recursive: true });
  git(["init", "-q"], repo);
  await writeFile(join(repo, "a.txt"), "1\n");
  git(["add", "a.txt"], repo);
  git(["commit", "-qm", "one"], repo);
  git(["worktree", "add", "-q", "-b", branch, checkout], repo);

  let spaces = [{ id: "wt", name: branch, folder: checkout, machine: null, tabs: [] }];
  const queued: [string, any][] = [], runs: [string, any][] = [], closed: string[] = [];
  const stubs: Record<string, unknown> = {
    View: class {}, div: element, v_flex: element, h_flex: element, Input: { new: element }, Scrollbar: { vertical: element },
    InputState: { new: () => ({ on() {}, value: () => "", focus() {} }) },
    theme: () => ({}), tr: (key: string) => key, home: () => home, appHome: () => join(home, ".config/yuzora"), readTextFile: async () => null,
    props: () => ({ overlay: "remove", space: "wt", action: "remove" }),
    closeOverlay: (id: string) => closed.push(id),
    run: (name: string, args: any) => { runs.push([name, args]); queued.push([name, args]); },
    spaces: () => spaces, panes: () => [],
    gitOpen: async () => ({ id: 1 }), gitStatus: async () => ({ branch }),
    exec: async (command: string, args: string[], o: { cwd?: string } = {}) => {
      const p = Bun.spawn([command, ...args], { cwd: o.cwd, env, stdout: "pipe", stderr: "pipe" });
      const [code, stdout, stderr] = await Promise.all([p.exited, new Response(p.stdout).text(), new Response(p.stderr).text()]);
      return { code, stdout, stderr };
    },
  };
  const Worktree = new Function(...names, `${body}; return Worktree;`)(...names.map((n) => (n in stubs ? stubs[n] : element)));
  const tasks: Promise<unknown>[] = [];
  const cx: any = {
    focus_handle: () => ({ focus() {} }), timer: { after() {} }, notify() {}, stop_propagation() {},
    spawn: (fn: (cx: unknown) => Promise<unknown>) => tasks.push(fn(cx)),
    async sleep() {
      for (const [name, args] of queued.splice(0)) {
        if (name === "space.close" && !unsaved) spaces = spaces.filter((s) => s.id !== args.id);
      }
    },
  };
  const view = new Worktree();
  view.init({}, cx);
  while (tasks.length) await tasks.shift();
  // Enter in the dialog.
  const enter = async () => { await view.work(cx); };
  const answered = () => { spaces = []; };
  return { view, cx, checkout, runs, closed, enter, answered };
}

test("while the Space asks about unsaved changes, nothing is removed and the dialog says why", async () => {
  const { view, cx, checkout, runs, closed, enter } = await removing("asks", true);
  await enter();
  expect(runs).toEqual([["space.close", { id: "wt", confirmed: true }]]);
  expect(existsSync(checkout)).toBe(true);
  expect(closed).toEqual([]);
  expect(textOf(view.render(cx).node)).toContain("unsaved changes");
});

test("once the answer has closed the Space, Enter again removes the worktree", async () => {
  const { checkout, runs, closed, enter, answered } = await removing("answered", true);
  await enter();
  expect(existsSync(checkout)).toBe(true);
  answered();
  await enter();
  expect(runs).toEqual([["space.close", { id: "wt", confirmed: true }]]);
  expect(existsSync(checkout)).toBe(false);
  expect(closed).toEqual(["remove"]);
});

test("with nothing unsaved, the Space closes and the worktree is removed at once", async () => {
  const { checkout, runs, closed, enter } = await removing("quiet", false);
  await enter();
  expect(runs).toEqual([["space.close", { id: "wt", confirmed: true }]]);
  expect(existsSync(checkout)).toBe(false);
  expect(closed).toEqual(["remove"]);
});
