// yuzora(L2): the repository picker's keys (defaults/mods/yuzora-sidebar-git).
// Run: bun test scripts/yuzora-git-picker.test.ts
import { test, expect } from "bun:test";
import { readFile } from "node:fs/promises";

const src = await readFile(new URL("../defaults/mods/yuzora-sidebar-git/main.js", import.meta.url), "utf8");
const method = (name: string) => src.match(new RegExp(`\\n  (${name}\\([^)]*\\) \\{[\\s\\S]*?\\n  })\\n`))![1];

// pickAt and pickKey run on a stand-in `this`, with the mod's two calls faked.
function world(found: unknown, pinned: boolean, current: string | null) {
  const log: (string | null)[] = [], closed: number[] = [];
  const mod: any = {
    picker: { pinned, current }, found, hi: undefined,
    pick: (root: string | null) => log.push(root),
  };
  const build = new Function("props", "closeOverlay", `return { ${method("pickAt")}, ${method("pickKey")} };`);
  Object.assign(mod, build(() => ({ overlay: 1 }), () => closed.push(1)));
  const key = (k: string, list: any[]) => mod.pickKey({ key: k }, { stop_propagation() {}, notify() {} }, list);
  return { mod, key, log, closed };
}

const list = [{ root: "/r/a" }, { root: "/r/b" }];

test("Enter before the list has loaded does nothing, even with a repo pinned", () => {
  const w = world(null, true, "/r/a");
  w.key("enter", []);
  expect(w.log).toEqual([]);
});

test("Enter after it loaded picks the row the keys are on, or follows the focus on row 0", () => {
  const w = world({ repos: list }, true, "/r/b");
  w.key("enter", list);
  expect(w.log).toEqual(["/r/b"]);
  const f = world({ repos: list }, false, null);
  f.key("enter", list);
  expect(f.log).toEqual([null]);
});
