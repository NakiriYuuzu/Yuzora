// yuzora(L2): the Git log pane's own state (defaults/mods/yuzora-git-log).
// Run: bun test scripts/yuzora-git-log.test.ts
import { test, expect } from "bun:test";
import { readFile } from "node:fs/promises";

const src = await readFile(new URL("../defaults/mods/yuzora-git-log/main.js", import.meta.url), "utf8");

// The pane's class with herdr and Yuzora stubbed: only what initLog touches.
function logView() {
  const body = src.replace(/^import [\s\S]*?;\n/gm, "").replace("export default class GitLog", "class GitLog");
  const stubs = {
    View: class {},
    props: () => ({}),
    spaces: () => [],
    getSetting: () => ({}),
    VirtualListScrollHandle: { new: () => ({}) },
    InputState: { new: () => ({ on() {} }) },
    t: (k: string) => k,
  };
  const GitLog = new Function(...Object.keys(stubs), `${body}; return GitLog;`)(...Object.values(stubs));
  return new GitLog();
}

test("reading the log again forgets a Trust and fetch that failed", () => {
  const view = logView();
  view.p = { filter: {} };
  view.lazyErr = "untrusted folder";
  view.initLog({ spawn() {} });
  expect(view.lazyErr).toBeNull();
});
