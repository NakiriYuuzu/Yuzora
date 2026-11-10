// yuzora(L2): which Git pane an opener reuses (defaults/mods/yuzora-git-*).
// Run: bun test scripts/yuzora-git-pane-reuse.test.ts
import { test, expect } from "bun:test";
import { readFile } from "node:fs/promises";

type Tab = { id: string; active?: boolean };
type Space = { id: string; active?: boolean; folder?: string; machine?: string | null; tabs: Tab[] };
type Pane = { pane: string; mod: string; tab: string };

const read = (mod: string, file = "main.js") => readFile(new URL(`../defaults/mods/${mod}/${file}`, import.meta.url), "utf8").catch(() => null);
const strip = (src: string) => src.replace(/^import [\s\S]*?;\n/gm, "").replace(/^export default class/m, "class").replace(/^export (?=function|const|class)/gm, "");

// A mod's own source, run with herdr and the host modules stubbed: the names
// it exports to its siblings (git-panes.js) join the stubs of main.js.
async function load(mod: string, scope: Record<string, unknown>, take: string[]) {
  let names = Object.keys(scope), values = Object.values(scope);
  const lib = await read(mod, "git-panes.js");
  if (lib) {
    const exported = [...lib.matchAll(/^export (?:function|const) ([\w$]+)/gm)].map((m) => m[1]);
    const made = new Function(...names, `${strip(lib)}\nreturn { ${exported.join(", ")} };`)(...values);
    names = [...names, ...exported];
    values = [...values, ...exported.map((n) => made[n])];
  }
  return new Function(...names, `${strip((await read(mod))!)}\nreturn { ${take.join(", ")} };`)(...values);
}

function world(spaces: Space[], panes: Pane[], settings: object = {}) {
  const calls: unknown[][] = [];
  const store = new Map<string, unknown>();
  const scope = {
    View: class {},
    spaces: () => spaces.map((s) => ({ folder: "", machine: null, active: false, ...s })),
    panes: () => panes,
    focused: () => null,
    shared: (k: string) => store.get(k) ?? null,
    setShared: (k: string, v: unknown) => { store.set(k, v); calls.push(["setShared", k, v]); },
    run: (name: string, args: unknown) => calls.push(["run", name, args]),
    openOverlay: (spec: unknown) => calls.push(["openOverlay", spec]),
    gitSettings: () => ({ diffOpenIn: "modal", diffKeepIn: "split", diffFollowPane: true, ...settings }),
    t: (k: string) => k,
  };
  return { scope, calls, store };
}

const two: Space[] = [
  { id: "s1", tabs: [{ id: "t1", active: true }] },
  { id: "s2", active: true, tabs: [{ id: "t2", active: true }] },
];

test("the graph button in another space opens that space's own log, not the first space's", async () => {
  const w = world(two, [{ pane: "p1", mod: "yuzora-git-log", tab: "t1" }]);
  const { SidebarGit } = await load("yuzora-sidebar-git", w.scope, ["SidebarGit"]);
  const side = new SidebarGit();
  side.repo = { root: "/projects/two", machine: null };
  side.openLog();
  expect(w.calls.filter((c) => c[1] === "pane.focus")).toEqual([]);
  expect(w.calls).toContainEqual(["run", "tab.new", expect.objectContaining({ mod: "yuzora-git-log", props: { cwd: "/projects/two", on: null } })]);
});
