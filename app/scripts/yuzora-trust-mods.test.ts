// yuzora(L2c): the default mods that run git themselves (changes, files,
// worktree, spaces) run none of the programs an untrusted repo's own config
// names, and run as before once it is trusted. git is real (a fixture repo
// with a clean and a smudge filter and an fsmonitor hook); yuzora:git's
// answer (status().trust) is stubbed.
// Run: bun test scripts/yuzora-trust-mods.test.ts
import { test, expect, afterAll } from "bun:test";
import { readFile, mkdtemp, rm, writeFile, chmod, mkdir, utimes, realpath, readdir } from "node:fs/promises";
import { existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const mods = new URL("../defaults/mods/", import.meta.url);
const read = (mod: string) => readFile(new URL(`${mod}/main.js`, mods), "utf8");

// Every name a mod imports, each given a stub (or a do-anything element).
const element: any = new Proxy(function () {}, { get: () => element, apply: () => element });
async function load(mod: string, cls: string, stubs: Record<string, unknown>) {
  const src = await read(mod);
  const names = [...src.matchAll(/^import\s*\{([^}]*)\}\s*from/gm)].flatMap((m) => m[1].split(",").map((s) => s.trim().split(/\s+as\s+/).pop()!).filter(Boolean));
  const body = src.replace(/^import .*;\n/gm, "").replace(`export default class ${cls}`, `class ${cls}`).replace(/^export /gm, "");
  return new Function(...names, `${body}; return ${cls};`)(...names.map((n) => (n in stubs ? stubs[n] : n === "View" ? class {} : element)));
}

// A repo whose own config runs programs, each leaving ran-<name> behind.
const base = await realpath(await mkdtemp(join(tmpdir(), "yuzora-l2c-")));
const home = join(base, "home"), repo = join(base, "repo");
const env = { ...process.env, HOME: home, GIT_CONFIG_NOSYSTEM: "1", GIT_CONFIG_GLOBAL: join(home, ".gitconfig") };
function git(args: string[], cwd = repo) {
  const p = Bun.spawnSync(["git", ...args], { cwd, env });
  if (p.exitCode !== 0) throw new Error(`git ${args.join(" ")}: ${p.stderr}`);
}
await mkdir(home, { recursive: true });
await mkdir(repo, { recursive: true });
await writeFile(join(home, ".gitconfig"), "[user]\n\tname = t\n\temail = t@t\n[commit]\n\tgpgsign = false\n[init]\n\tdefaultBranch = main\n");
git(["init", "-q"]);
await writeFile(join(repo, "a.txt"), "1\n");
git(["add", "a.txt"]);
git(["commit", "-qm", "one"]);
// An uncommitted change: every status hashes it again (the clean filter runs).
await writeFile(join(repo, "a.txt"), "2\n");
for (const [key, name, then] of [["filter.evil.clean", "clean", "cat"], ["filter.evil.smudge", "smudge", "cat"], ["core.fsmonitor", "fsmonitor", "exit 1"]]) {
  await writeFile(join(base, `${name}.sh`), `#!/bin/sh\ntouch '${join(base, `ran-${name}`)}'\n${then}\n`);
  await chmod(join(base, `${name}.sh`), 0o755);
  git(["config", key, join(base, `${name}.sh`)]);
}
await writeFile(join(repo, ".git/info/attributes"), "*.txt filter=evil\n");
const KEYS = ["core.fsmonitor", "filter.evil.clean", "filter.evil.smudge"];
afterAll(() => rm(base, { recursive: true, force: true }));

// A new mtime too, whatever a status before wrote to the index.
const touch = () => utimes(join(repo, "a.txt"), 1_600_000_000, 1_600_000_000);
async function ran() {
  const names = (await readdir(base)).filter((n) => n.startsWith("ran-"));
  await Promise.all(names.map((n) => rm(join(base, n))));
  return names.map((n) => n.slice(4)).sort();
}

// herdr's exec, run here, and what it was asked.
function world(trusted: boolean) {
  const calls: string[][] = [], opened: unknown[][] = [];
  const exec = async (command: string, args: string[], o: { cwd?: string } = {}) => {
    calls.push([command, ...args]);
    const p = Bun.spawn([command, ...args], { cwd: o.cwd, env, stdout: "pipe", stderr: "pipe" });
    const [code, stdout, stderr] = await Promise.all([p.exited, new Response(p.stdout).text(), new Response(p.stderr).text()]);
    return { code, stdout, stderr };
  };
  const stubs = {
    exec,
    gitOpen: async (cwd: string, on: string | null) => (opened.push([cwd, on]), { id: `:${cwd}` }),
    gitStatus: async () => (trusted ? { branch: "main" } : { branch: "main", trust: KEYS }),
    tr: (key: string, vars: { keys?: string } = {}) => `${key} (${vars.keys})`,
    spaces: () => [{ id: "s1", active: true, folder: repo, machine: null, tabs: [] }],
    panes: () => [],
    focused: () => null,
    watchFolder: async () => { throw new Error("no folder watch here"); },
    readTextFile: async () => null,
    theme: () => ({}),
    settings: () => ({}),
    window: () => ({ platform: process.platform === "win32" ? "windows" : process.platform }),
    home: () => home,
    InputState: { new: () => ({ on() {}, value: () => "", set_value() {}, focus() {} }) },
  };
  const tasks: Promise<unknown>[] = [];
  const cx: any = { focus_handle: () => ({ focus() {} }), timer: { every() {}, after() {} }, notify() {}, stop_propagation() {}, spawn: (fn: (cx: unknown) => Promise<unknown>) => tasks.push(fn(cx)) };
  const settle = async () => { while (tasks.length) await tasks.shift(); };
  // Only these read refs and nothing else (docs/beta2/git.md, outside yuzora:git).
  const refsOnly = () => calls.filter((c) => c[0] === "git").every((c) => c[1] === "rev-parse" && ["--show-toplevel", "--path-format=absolute"].includes(c[2]));
  return { stubs, calls, opened, cx, settle, refsOnly };
}

test("Changes: an untrusted folder shows the note, runs no status, diff or commit", async () => {
  await touch();
  await ran();
  const w = world(false);
  const Changes = await load("changes", "Changes", w.stubs);
  const view = new Changes();
  view.init({}, w.cx);
  await w.settle();
  expect(view.error).toBe(`untrusted (${KEYS.join(", ")})`);
  expect(view.files).toEqual([]);
  expect(w.opened).toEqual([[repo, null]]);
  view.message = "a message";
  view.commit(w.cx);
  await w.settle();
  expect(w.refsOnly()).toBe(true);
  expect(await ran()).toEqual([]);
});

test("Changes: a trusted folder lists and diffs as before", async () => {
  await touch();
  await ran();
  const w = world(true);
  const Changes = await load("changes", "Changes", w.stubs);
  const view = new Changes();
  view.init({}, w.cx);
  await w.settle();
  expect(view.error).toBe(null);
  expect(w.calls.some((c) => c[1] === "--no-optional-locks" && c.includes("status"))).toBe(true);
  // The clean filter ran: the fixture is a real one.
  expect(await ran()).toContain("clean");
});

test("Files: an untrusted folder lists its files without marks and says why", async () => {
  await touch();
  await ran();
  const w = world(false);
  const Files = await load("files", "Files", w.stubs);
  const view = new Files();
  view.init({}, w.cx);
  await w.settle();
  expect(view.entries.get("")?.map((e: { name: string }) => e.name)).toContain("a.txt");
  expect(view.marks).toEqual({});
  expect(view.untrusted).toBe(`untrusted (${KEYS.join(", ")})`);
  expect(w.refsOnly()).toBe(true);
  expect(await ran()).toEqual([]);
});

test("Files: when trust can't be told, no marks and no note", async () => {
  await touch();
  await ran();
  const w = world(true);
  const Files = await load("files", "Files", { ...w.stubs, gitStatus: async () => { throw new Error("git couldn't run"); } });
  const view = new Files();
  view.init({}, w.cx);
  await w.settle();
  expect(view.marks).toEqual({});
  expect(view.untrusted).toBe(null);
  expect(view.error).toBe(null);
  expect(await ran()).toEqual([]);
});

test("Files: a trusted folder marks changed files as before", async () => {
  await touch();
  await ran();
  const w = world(true);
  const Files = await load("files", "Files", w.stubs);
  const view = new Files();
  view.init({}, w.cx);
  await w.settle();
  expect(view.untrusted).toBe(null);
  expect(Object.keys(view.marks)).toEqual([`${repo}/a.txt`]);
  expect(await ran()).toContain("clean");
});

async function worktree(trusted: boolean, p: Record<string, unknown>) {
  const w = world(trusted);
  const runs: unknown[][] = [];
  const Worktree = await load("worktree", "Worktree", { ...w.stubs, props: () => p, run: (...a: unknown[]) => runs.push(a), closeOverlay() {} });
  const view = new Worktree();
  view.init({}, w.cx);
  await w.settle();
  return { view, w, runs };
}

for (const action of ["remove", "new", "open"]) {
  test(`Worktree ${action}: an untrusted folder says so and runs nothing but rev-parse`, async () => {
    await touch();
    await ran();
    const { view, w, runs } = await worktree(false, { folder: repo, action, branch: action === "new" ? "feat" : undefined });
    expect(view.untrusted).toBe(`untrusted (${KEYS.join(", ")})`);
    expect(view.repo?.top).toBe(repo);
    // Enter (or a branch in the props) does nothing.
    view.text = "feat";
    await view.work(w.cx);
    await w.settle();
    expect(runs).toEqual([]);
    expect(w.refsOnly()).toBe(true);
    expect(await ran()).toEqual([]);
  });
}

test("Worktree: a trusted folder creates and reads as before", async () => {
  await touch();
  await ran();
  const removing = await worktree(true, { folder: repo, action: "remove" });
  expect(removing.view.untrusted).toBe(undefined);
  expect(removing.w.calls.some((c) => c.includes("status"))).toBe(true);
  expect(await ran()).toContain("clean");
  const { view, runs } = await worktree(true, { folder: repo, action: "new", branch: "feat" });
  expect(view.error).toBe("");
  expect(runs[0]?.[0]).toBe("space.new");
  // The checkout ran the smudge filter: the fixture is a real one.
  expect(await ran()).toContain("smudge");
  git(["worktree", "remove", "--force", join(home, ".herdr/worktrees/repo/feat")]);
  git(["branch", "-D", "feat"]);
});

test("Spaces: branch and ahead/behind come from yuzora:shell, never git status", async () => {
  await touch();
  await ran();
  const w = world(false);
  const asked: unknown[][] = [];
  const Spaces = await load("spaces", "Spaces", { ...w.stubs, gitStatus: async (...a: unknown[]) => (asked.push(a), { branch: "topic", upstream: "origin/topic", ahead: 2, behind: 1, changed: null, conflicted: null }) });
  const view = new Spaces();
  Object.assign(view, { git: {}, checked: {}, unsure: new Set(), watching: new Set() });
  const key = JSON.stringify([null, repo]);
  await view.check(key, w.cx);
  expect(asked).toEqual([[repo, { on: null }]]);
  expect(view.git[key]).toMatchObject({ name: "repo", branch: "topic", ahead: 2, behind: 1, linked: false });
  expect(w.refsOnly()).toBe(true);
  expect(await ran()).toEqual([]);
  // Not a repo: remembered as none, not asked again on the fallback.
  const none = await load("spaces", "Spaces", { ...w.stubs, gitStatus: async () => null });
  const other = Object.assign(new none(), { git: {}, checked: {}, unsure: new Set(), watching: new Set() });
  await other.check(key, w.cx);
  expect(other.git[key]).toBe(null);
  expect(other.unsure.size).toBe(0);
});

test("each mod's untrusted note is in both languages", async () => {
  for (const mod of ["changes", "files", "worktree"]) {
    const [en, zh] = await Promise.all(["en", "zh-TW"].map(async (l) => JSON.parse(await readFile(new URL(`${mod}/i18n/${l}.json`, mods), "utf8"))));
    expect(Object.keys(zh).sort()).toEqual(Object.keys(en).sort());
    for (const [, k] of (await read(mod)).matchAll(/\btr\("([\w.]+)"/g)) {
      expect(en[k]).toContain("{keys}");
      expect(zh[k]).toContain("{keys}");
    }
  }
});

// Mods can't share a file: each carries the same question.
test("each mod asks yuzora:git the same way", async () => {
  const fn = (src: string) => src.match(/\nasync function untrusted\([\s\S]*?\n}\n/)?.[0];
  const [first, ...rest] = await Promise.all(["changes", "files", "worktree"].map(read));
  expect(fn(first)).toContain("(await gitStatus((await gitOpen(cwd, on ?? null)).id)).trust ?? null");
  for (const src of rest) expect(fn(src)).toBe(fn(first));
});

// A new place a default mod runs git (an upstream sync) needs a look: is it
// refs only, or does it wait for trust? See docs/beta2/git.md.
test("the default mods run git only where it was looked at", async () => {
  const seen: Record<string, number> = {};
  for (const entry of await readdir(mods, { withFileTypes: true })) {
    if (!entry.isDirectory()) continue;
    const file = new URL(`${entry.name}/main.js`, mods);
    if (!existsSync(file)) continue;
    const n = ((await readFile(file, "utf8")).match(/exec\(\s*"git"/g) ?? []).length;
    if (n) seen[entry.name] = n;
  }
  expect(seen).toEqual({ changes: 4, files: 2, menu: 2, spaces: 2, worktree: 1 });
});
