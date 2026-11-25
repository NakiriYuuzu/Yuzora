// yuzora(L1): a pane someone named (pane.rename) shows that name in Yuzora's pane header.
// Run from app/: bun test scripts/yuzora-pane-name.test.ts
import { expect, test } from "bun:test";
import { readFile } from "node:fs/promises";

const source = (await readFile(new URL("../defaults/mods/yuzora-shell-pane/main.js", import.meta.url), "utf8"))
  .replace(/^import [\s\S]*?;\n/gm, "").replace(/^export default class/m, "class").replace(/^export /gm, "");
const describe = new Function("View", `${source}; return describe;`)(class {});

const shell = { pane: "p1", program: { shell: true, name: "zsh" }, title: "~/app", cwd: "/r/app", branch: "main", name: null };
const agent = { pane: "p2", program: { agent: true, name: "Claude Code", icon: "claude.svg" }, title: "fixing tests", cwd: "/r/app", name: null };
const tool = { pane: "p3", mod: "yuzora-git-diff", title: "yuzora-git-diff", name: null };
const tools = [{ mod: "yuzora-git-diff", label: "Git diff", icon: "diff.svg" }];

test("a named terminal pane shows its name, and still says where it is", () => {
  expect(describe({ ...shell, name: "build" }, [])).toMatchObject({ name: "build", where: "app · main" });
  expect(describe({ ...agent, name: "reviewer" }, [])).toMatchObject({ name: "reviewer", icon: "claude.svg" });
});

test("a named tool pane shows its name and keeps its tool's icon", () => {
  expect(describe({ ...tool, name: "review" }, tools)).toMatchObject({ name: "review", icon: "diff.svg" });
});

test("a pane nobody named is called after its program or its tool", () => {
  expect(describe(shell, []).name).toBe("zsh");
  expect(describe(agent, []).name).toBe("Claude Code");
  expect(describe(tool, tools).name).toBe("Git diff");
  expect(describe({ ...shell, name: "" }, []).name).toBe("zsh");
});
