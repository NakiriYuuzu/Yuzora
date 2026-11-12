// A file's tab shows the file's icon, as the Files sidebar draws it, not its Space's badge.
// Run from app/: bun test scripts/yuzora-tab-file-icon.test.ts
import { expect, test } from "bun:test";
import { readFile } from "node:fs/promises";
import { leadOf, tabFile } from "../defaults/mods/yuzora-shell-tabs/lead.js";
import { fileIcon } from "../defaults/mods/yuzora-shell-tabs/icons.js";

const mod = (name: string, file: string) => readFile(new URL(`../defaults/mods/${name}/${file}`, import.meta.url), "utf8");

const editor = { pane: "p1", mod: "yuzora-editor", props: { path: "/repo/src/main.rs", on: null } };
const markdown = { pane: "p2", mod: "yuzora-markdown", props: { path: "/repo/README.md", on: null } };
const picture = { pane: "p3", mod: "yuzora-files-image", props: { path: "C:\\repo\\logo.png", on: null } };
const shell = { pane: "p4", terminal: "t1", program: { shell: true, name: "zsh" } };
const agent = { pane: "p5", terminal: "t2", program: { agent: true, name: "Claude Code" } };
const diff = { pane: "p6", mod: "yuzora-git-diff", props: { path: "src/main.rs" } };

test("a tab of one file is that file's", () => {
  expect(tabFile([editor])).toBe("/repo/src/main.rs");
  expect(tabFile([markdown])).toBe("/repo/README.md");
  expect(tabFile([picture])).toBe("C:\\repo\\logo.png");
});

test("a tab named after a terminal or an agent keeps its Space's badge", () => {
  expect(tabFile([shell])).toBeNull();
  expect(tabFile([editor, shell])).toBeNull();
  expect(tabFile([editor, shell, agent])).toBeNull();
  expect(leadOf([editor, shell, agent])).toBe(agent);
});

test("a tool pane that is not a file's, a file pane with no path and an empty tab are no file's", () => {
  expect(tabFile([diff])).toBeNull();
  expect(tabFile([{ pane: "p7", mod: "yuzora-editor", props: {} }])).toBeNull();
  expect(tabFile([])).toBeNull();
});

test("two files side by side: the first one's", () => {
  expect(tabFile([markdown, editor])).toBe("/repo/README.md");
});

test("the tab bar's icons are the Files sidebar's", async () => {
  expect(await mod("yuzora-shell-tabs", "icons.js")).toBe(await mod("yuzora-sidebar-files", "icons.js"));
  expect(fileIcon("main.rs")).toEqual({ icon: "code", color: "ansi3" });
});
