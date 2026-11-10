// yuzora(L2): which file the diff pane shows (defaults/mods/yuzora-git-diff).
// Run: bun test scripts/yuzora-git-diff-pane.test.ts
import { test, expect } from "bun:test";
import { readFile } from "node:fs/promises";

const src = await readFile(new URL("../defaults/mods/yuzora-git-diff/main.js", import.meta.url), "utf8");

// The pane with herdr and Yuzora stubbed, opened with these props while
// the shared request is `request`.
function diffPane(p: object, request: object | null) {
  const body = src.replace(/^import [\s\S]*?;\n/gm, "").replace("export default class GitDiff", "class GitDiff");
  const stubs = { View: class {}, props: () => p, shared: () => request, focused: () => ({ space: "s1" }), gitSettings: () => ({}), VirtualListScrollHandle: { new: () => ({}) } };
  const GitDiff = new Function(...Object.keys(stubs), `${body}; return GitDiff;`)(...Object.values(stubs));
  const view = new GitDiff();
  view.init(p, { focus_handle: () => ({}), spawn() {}, notify() {} });
  return view;
}

// An agent opens one with `tab.new --props '{"cwd": …, "path": …}'` (no
// `at`), while the request a click left an hour ago is still kept.
test("a pane opened with a file and no time shows that file, not an older request", () => {
  const props = { cwd: "/r", on: null, path: "new.txt", mode: "unstaged" };
  const old = { ...props, path: "old.txt", at: Date.now() - 3_600_000 };
  expect(diffPane(props, old).wanted().path).toBe("new.txt");
  // A request made after it opened still moves it.
  expect(diffPane(props, { ...old, path: "next.txt", at: Date.now() + 1000 }).wanted().path).toBe("next.txt");
});
