// yuzora(v010): which lines of a Git diff a selection covers (defaults/mods/yuzora-git-diff).
// The rows are what the Rust side makes (`diffView`): a selection names places in rows (a row, and a place in its
// line), and the lines it covers are worked out here as runs of one side's lines, cut where it begins and ends. The
// text of those lines (tabs, wide characters, trailing spaces, a last line without a newline) is the Rust side's, in
// yuzora-git's own tests; what the pane does with a click, a drag, a key and a menu is in lifecycle_tests.rs.
// Run: bun test scripts/yuzora-git-diff-copy.test.ts
import { test, expect } from "bun:test";
import { readFile } from "node:fs/promises";

const src = await readFile(new URL("../defaults/mods/yuzora-git-diff/main.js", import.meta.url), "utf8");

type Row = Record<string, unknown>;
type Place = { row: number; at: number };
type Mark = { side: "left" | "right" | "unified"; anchor: Place; head: Place } | null;
type Run = [string, number, number];

const { copyRuns, touchedHunks, hunkRuns, covers, whole, reach, lines, cutOf, empty, EOL } = (() => {
  const body = src.replace(/^import [\s\S]*?;\n/gm, "").replace("export default class GitDiff", "class GitDiff");
  return new Function("View", `${body}; return { copyRuns, touchedHunks, hunkRuns, covers, whole, reach, lines, cutOf, empty, EOL };`)(class {});
})() as {
  copyRuns: (rows: Row[], mark: Mark) => Run[];
  touchedHunks: (rows: Row[], mark: Mark) => number[];
  hunkRuns: (view: { hunks: Row[] }, hunk: number) => Run[];
  covers: (mark: Mark, row: number) => boolean;
  whole: (row: number, side: string) => Mark;
  reach: (mark: Mark, row: number) => Mark;
  lines: (mark: Mark) => boolean;
  cutOf: (rows: Row[], mark: Mark) => [number, number];
  empty: (mark: Mark) => boolean;
  EOL: number;
};

// Line numbers are 1-based in the rows; a run is 0-based, half open.
const same = (l: number, r: number): Row => ({ k: "=", l, r, t: "same" });
const changed = (h: number, l?: number, r?: number): Row => ({ k: "~", h, ...(l && { l, li: l - 1, lt: "old" }), ...(r && { r, ri: r - 1, rt: "new" }) });
const fold = (n: number): Row => ({ k: "fold", n });
const gone = (h: number, l: number): Row => ({ k: "-", h, l, li: l - 1, t: "old" });
const added = (h: number, r: number): Row => ({ k: "+", h, r, ri: r - 1, t: "new" });
// Whole lines, from row `anchor`'s to row `head`'s (up the rows when `head` is above).
const sel = (side: "left" | "right" | "unified", anchor: number, head = anchor): Mark => {
  const down = head >= anchor;
  return { side, anchor: { row: anchor, at: down ? -1 : EOL }, head: { row: head, at: down ? EOL : -1 } };
};
// Characters, from place `a` of row `ar` to place `h` of row `hr`.
const chars = (side: "left" | "right" | "unified", ar: number, a: number, hr: number, h: number): Mark => ({ side, anchor: { row: ar, at: a }, head: { row: hr, at: h } });

const side = [same(1, 1), changed(0, 2, 2), changed(0, 3), changed(0, undefined, 3), same(4, 4)];

test("side by side, a side's selection is that side's lines of the rows it covers", () => {
  expect(copyRuns(side, sel("left", 0, 4))).toEqual([["l", 0, 4]]);
  expect(copyRuns(side, sel("right", 0, 4))).toEqual([["r", 0, 4]]);
  expect(copyRuns(side, sel("left", 1, 1))).toEqual([["l", 1, 2]]);
});

test("a row with no line on the side adds none: an added line has no left line, a deleted one no right line", () => {
  expect(copyRuns(side, sel("left", 3, 3))).toEqual([]);
  expect(copyRuns(side, sel("right", 2, 2))).toEqual([]);
  expect(copyRuns(side, sel("left", 3, 4))).toEqual([["l", 3, 4]]);
});

test("an unchanged row is a line of either side, under its own number on each", () => {
  const rows = [same(5, 7), changed(0, 6, 8)];
  expect(copyRuns(rows, sel("left", 0, 1))).toEqual([["l", 4, 6]]);
  expect(copyRuns(rows, sel("right", 0, 1))).toEqual([["r", 6, 8]]);
});

test("a fold adds nothing and breaks the run: the lines it hides are not copied", () => {
  const rows = [same(1, 1), same(2, 2), fold(5), same(8, 8), same(9, 9)];
  expect(copyRuns(rows, sel("left", 0, 4))).toEqual([["l", 0, 2], ["l", 7, 9]]);
  expect(copyRuns(rows, sel("right", 1, 3))).toEqual([["r", 1, 2], ["r", 7, 8]]);
  expect(copyRuns(rows, sel("left", 2, 2))).toEqual([]);
});

test("unified, the lines are the ones shown, in the order shown: deleted from the left, added from the right", () => {
  const rows = [same(1, 1), gone(0, 2), gone(0, 3), added(0, 2), added(0, 3), same(4, 4)];
  expect(copyRuns(rows, sel("unified", 0, 5))).toEqual([["l", 0, 3], ["r", 1, 3], ["l", 3, 4]]);
  expect(copyRuns(rows, sel("unified", 3, 3))).toEqual([["r", 1, 2]]);
  // An unchanged row whose sides differ (whitespace ignored) is shown as its left text.
  expect(copyRuns([{ k: "=", l: 1, r: 1, t: "a ", rt: "a" }], sel("unified", 0))).toEqual([["l", 0, 1]]);
});

test("a selection reaches the same lines whichever way it was made", () => {
  expect(copyRuns(side, sel("left", 4, 0))).toEqual(copyRuns(side, sel("left", 0, 4)));
  expect(copyRuns(side, sel("right", 3, 1))).toEqual(copyRuns(side, sel("right", 1, 3)));
});

test("no selection covers no lines, and a selection past the rows is cut at the rows", () => {
  expect(copyRuns(side, null)).toEqual([]);
  expect(copyRuns(side, sel("left", 3, 40))).toEqual([["l", 3, 4]]);
  expect(copyRuns([], sel("left", 0, 3))).toEqual([]);
});

test("a selection across hunks touches each hunk that has a selected line, once, in order", () => {
  const rows = [changed(0, 1, 1), same(2, 2), changed(1, 3, 3), changed(1, 4, 4), same(5, 5), changed(2, 6, 6)];
  expect(touchedHunks(rows, sel("left", 0, 5))).toEqual([0, 1, 2]);
  expect(touchedHunks(rows, sel("left", 5, 2))).toEqual([1, 2]);
  expect(touchedHunks(rows, sel("left", 1, 1))).toEqual([]);
  expect(touchedHunks(rows, null)).toEqual([]);
});

test("a hunk is touched by the lines selected on a side, not by rows that have none there", () => {
  const rows = [changed(0, undefined, 1), changed(1, 2, 2)];
  expect(touchedHunks(rows, sel("left", 0, 1))).toEqual([1]);
  expect(touchedHunks(rows, sel("right", 0, 1))).toEqual([0, 1]);
  expect(touchedHunks([added(0, 1), gone(1, 1)], sel("unified", 0, 1))).toEqual([0, 1]);
});

test("a hunk's new lines are its right-hand range, none for a hunk that only deletes", () => {
  const view = { hunks: [{ a: [1, 3], b: [1, 2] }, { a: [5, 7], b: [4, 4] }] };
  expect(hunkRuns(view, 0)).toEqual([["r", 1, 2]]);
  expect(hunkRuns(view, 1)).toEqual([]);
  expect(hunkRuns(view, 2)).toEqual([]);
});

test("a click selects its row's line whole; Shift reaches there by lines from the line the selection began on", () => {
  expect(whole(3, "left")).toEqual(sel("left", 3));
  expect(reach(sel("right", 1), 4)).toEqual(sel("right", 1, 4));
  expect(reach(sel("right", 1, 4), 0)).toEqual(sel("right", 1, 0));
  expect([sel("left", 3), sel("left", 4, 1)].map(lines)).toEqual([true, true]);
  expect(lines(chars("left", 1, -1, 2, EOL))).toBe(true);
  // From a line's first character (a drag): characters, though they are the same text.
  expect(lines(chars("left", 1, 0, 2, EOL))).toBe(false);
});

test("whole lines are copied whole; characters are cut where the selection begins and where it ends", () => {
  expect(cutOf(side, sel("left", 0, 4))).toEqual([0, EOL]);
  expect(cutOf(side, sel("left", 4, 0))).toEqual([0, EOL]);
  // From place 5 of row 1 to place 2 of row 4, either way: the left's lines 2 to 4.
  for (const m of [chars("left", 1, 5, 4, 2), chars("left", 4, 2, 1, 5)]) {
    expect([copyRuns(side, m), cutOf(side, m)]).toEqual([[["l", 1, 4]], [5, 2]]);
  }
  // Ending on a row with no line on its side (an added line, on the left): the last line copied goes to its end.
  expect(cutOf(side, chars("left", 1, 5, 3, 2))).toEqual([5, EOL]);
  // Beginning on one (a deleted line, on the right): the first line copied from its start.
  expect(cutOf(side, chars("right", 2, 3, 4, 1))).toEqual([0, 1]);
});

test("a selection that ends where it began is of nothing; a click's line is not", () => {
  expect(empty(chars("left", 1, 4, 1, 4))).toBe(true);
  expect(empty(chars("left", 1, 4, 1, 5))).toBe(false);
  expect(empty(chars("left", 1, 4, 2, 0))).toBe(false);
  expect(empty(whole(1, "left"))).toBe(false);
  expect(empty(null)).toBe(true);
});

test("a row is covered from the first row of the selection to the last, whichever way it was made", () => {
  expect([0, 1, 2, 3, 4].map((i) => covers(sel("left", 3, 1), i))).toEqual([false, true, true, true, false]);
  expect(covers(null, 0)).toBe(false);
});
