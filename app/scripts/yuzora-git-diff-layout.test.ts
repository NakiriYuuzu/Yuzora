// yuzora(v010): what the side by side Git diff decides about its layout from the lines
// (defaults/mods/yuzora-git-diff). Where the sides end and how far they scroll are measured in the
// window, in lifecycle_tests.rs; here are the parts that need no window.
// Run: bun test scripts/yuzora-git-diff-layout.test.ts
import { test, expect } from "bun:test";
import { readFile } from "node:fs/promises";

const src = await readFile(new URL("../defaults/mods/yuzora-git-diff/main.js", import.meta.url), "utf8");

const { ratioOf, cells, widest } = (() => {
  const body = src.replace(/^import [\s\S]*?;\n/gm, "").replace("export default class GitDiff", "class GitDiff");
  return new Function("View", `${body}; return { ratioOf, cells, widest };`)(class {});
})() as {
  ratioOf: (v: unknown) => number;
  cells: (text: string) => number;
  widest: (rows: object[]) => { l: string[]; r: string[] };
};

test("a kept share is used as it is within 15% to 85%, clamped outside, and half when it is not a number", () => {
  expect([0.5, 0.3, 0.85, 0.15].map(ratioOf)).toEqual([0.5, 0.3, 0.85, 0.15]);
  expect([0, 0.1, -3, 0.9, 5].map(ratioOf)).toEqual([0.15, 0.15, 0.15, 0.85, 0.85]);
  expect([undefined, null, NaN, Infinity, "0.3", [0.3], {}].map(ratioOf)).toEqual([0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5]);
});

test("a wide character (CJK, full width, emoji) takes two cells, anything else one", () => {
  expect(cells("hello")).toBe(5);
  expect(cells("漢字")).toBe(4);
  expect(cells("a漢")).toBe(3);
  expect(cells("カタカナ")).toBe(8);
  expect(cells("Ａ")).toBe(2);
  expect(cells("\u{1F600}")).toBe(2);
  expect(cells("한글")).toBe(4);
});

const row = (lt?: string, rt?: string) => ({ k: "~", ...(lt !== undefined && { lt }), ...(rt !== undefined && { rt }) });

test("the widest lines of a side are the ones taking the most cells, not the most characters", () => {
  const rows = [row("a".repeat(10), "b"), row("漢".repeat(6), "b"), row("a".repeat(11), "b")];
  expect(widest(rows).l).toEqual(["漢".repeat(6), "a".repeat(11), "a".repeat(10)]);
});

test("only the widest few are kept, and a line too short to beat them is not counted", () => {
  const ascii = ["a".repeat(10), "a".repeat(20), "a".repeat(30), "a".repeat(40), "a".repeat(50)];
  // Eight cells in four characters: no more than 8 cells, so it cannot beat the 20 that are kept.
  expect(widest([...ascii, "\u6F22".repeat(4)].map((t) => row(t))).l).toEqual(ascii.slice(1).reverse());
  // Fifty cells in twenty-five characters: twice its length may still beat them, and does.
  expect(widest([...ascii.slice(0, 4), "\u6F22".repeat(25)].map((t) => row(t))).l).toEqual(["\u6F22".repeat(25), ...ascii.slice(0, 4).reverse().slice(0, 3)]);
});

test("an unchanged row counts on both sides, a side that has no line counts on none", () => {
  const rows = [{ k: "=", t: "same line" }, row("only on the left"), row(undefined, "only right is a longer line")];
  const { l, r } = widest(rows);
  expect(l).toEqual(["only on the left", "same line"]);
  expect(r).toEqual(["only right is a longer line", "same line"]);
});
