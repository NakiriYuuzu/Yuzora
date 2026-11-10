// yuzora(v010): what the side by side Git diff decides about its layout from the lines
// (defaults/mods/yuzora-git-diff). Where the sides end and how far they scroll are measured in the
// window, in lifecycle_tests.rs; here are the parts that need no window.
// Run: bun test scripts/yuzora-git-diff-layout.test.ts
import { test, expect } from "bun:test";
import { readFile } from "node:fs/promises";

const src = await readFile(new URL("../defaults/mods/yuzora-git-diff/main.js", import.meta.url), "utf8");
const fonts = JSON.parse(await readFile(new URL("./fixtures/yuzora-git-diff-font-widths.json", import.meta.url), "utf8")) as {
  samples: { text: string; units: number; cells: number }[];
};

type Fit = { show: string[]; rest: number };
const { ratioOf, bound, widest, LOOK, LINE, LAYOUT, WORST } = (() => {
  const body = src.replace(/^import [\s\S]*?;\n/gm, "").replace("export default class GitDiff", "class GitDiff");
  return new Function("View", `${body}; return { ratioOf, bound, widest, LOOK, LINE, LAYOUT, WORST };`)(class {});
})() as {
  ratioOf: (v: unknown) => number;
  bound: (text: string, meter: { left: number }) => number;
  widest: (rows: object[]) => { l: Fit; r: Fit };
  LOOK: number;
  LINE: number;
  LAYOUT: number;
  WORST: number;
};

const plenty = () => ({ left: Infinity });
// What each character takes in the diff's font, in cells, as it was measured.
const measured = new Map(fonts.samples.filter((s) => [...s.text].length === 1).map((s) => [s.text, s.cells]));
const truly = (text: string) => [...text].reduce((n, ch) => n + (measured.get(ch) ?? 1), 0);

test("a kept share is used as it is within 15% to 85%, clamped outside, and half when it is not a number", () => {
  expect([0.5, 0.3, 0.85, 0.15].map(ratioOf)).toEqual([0.5, 0.3, 0.85, 0.15]);
  expect([0, 0.1, -3, 0.9, 5].map(ratioOf)).toEqual([0.15, 0.15, 0.15, 0.85, 0.85]);
  expect([undefined, null, NaN, Infinity, "0.3", [0.3], {}].map(ratioOf)).toEqual([0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5]);
});

test("what a line can take at most is never less than the font draws it, for every character measured", () => {
  const short = fonts.samples.filter((s) => bound(s.text, plenty()) < s.cells);
  expect(short).toEqual([]);
});

test("an ASCII line takes a cell a character, no more, and a wide one is not taken at three", () => {
  expect(bound("hello", plenty())).toBe(5);
  expect(bound("", plenty())).toBe(0);
  expect(bound("漢".repeat(100), plenty())).toBe(200);
  expect(bound("é".repeat(100), plenty())).toBeLessThanOrEqual(125);
  expect(bound("\u{1f600}".repeat(100), plenty())).toBe(300);
});

test("what is not read is taken at its worst: the rest of a long line, and a line past what is left to read", () => {
  expect(bound("a".repeat(LINE + 10), plenty())).toBe(LINE + WORST * 10);
  expect(bound("a".repeat(50), { left: 0 })).toBe(WORST * 50);
  const meter = { left: 120 };
  expect(bound("a".repeat(100), meter)).toBe(100);
  expect(meter.left).toBeLessThan(0);
  expect(bound("a".repeat(100), meter)).toBe(WORST * 100);
});

const row = (lt?: string, rt?: string) => ({ k: "~", ...(lt !== undefined && { lt }), ...(rt !== undefined && { rt }) });

test("the lines to lay out are the ones that may take the most cells, not the most characters", () => {
  const rows = [row("a".repeat(10), "b"), row("漢".repeat(6), "b"), row("a".repeat(11), "b")];
  expect(widest(rows).l.show).toEqual(["漢".repeat(6), "a".repeat(11), "a".repeat(10)]);
});

test("only the widest few are laid out, and the most the rest may take is what is known of the rest", () => {
  const ascii = [10, 20, 30, 40, 50].map((n) => "a".repeat(n));
  const { l } = widest([...ascii, "x"].map((t) => row(t)));
  expect(l.show).toEqual(ascii.slice(1).reverse());
  // The fifth line has ten cells, and a line of one character is not read at all: three cells at most.
  expect(l.rest).toBe(10);
  // Twenty-five wide characters take fifty cells at most: they are among the few, and push a line out.
  const wideOne = widest([...ascii.slice(0, 4), "漢".repeat(25)].map((t) => row(t))).l;
  expect(wideOne.show).toEqual(["漢".repeat(25), ...ascii.slice(0, 4).reverse().slice(0, 3)]);
  expect(wideOne.rest).toBe(10);
});

test("with only ASCII the lines that are not laid out can take no more than the least of those that are", () => {
  const lengths = Array.from({ length: 2000 }, (_, i) => 5 + ((i * 7919) % 150));
  const { l } = widest(lengths.map((n) => row("a".repeat(n))));
  const sorted = [...lengths].sort((a, b) => b - a);
  expect(l.show.map((t) => t.length)).toEqual(sorted.slice(0, 4));
  expect(l.rest).toBeLessThanOrEqual(sorted[3]);
  expect(l.rest).toBeGreaterThanOrEqual(sorted[4]);
});

test("an unchanged row counts on both sides, a side that has no line counts on none", () => {
  const rows = [{ k: "=", t: "same line" }, row("only on the left"), row(undefined, "only right is a longer line")];
  const { l, r } = widest(rows);
  expect(l.show).toEqual(["only on the left", "same line"]);
  expect(r.show).toEqual(["only right is a longer line", "same line"]);
});

test("a line too long to lay out is taken at what it can take, and not laid out", () => {
  const long = "a".repeat(LAYOUT + 1);
  const { l } = widest([row("short"), row(long)]);
  expect(l.show).toEqual(["short"]);
  expect(l.rest).toBe(LAYOUT + 1);
});

// Replaces `String.prototype.replace` for the length of `f`, adding up the characters it was asked to read.
function reading<T>(f: () => T): { out: T; read: number } {
  const replace = String.prototype.replace;
  let read = 0;
  String.prototype.replace = function (this: string, ...args: [never, never]) {
    read += this.length;
    return replace.apply(this, args);
  } as typeof String.prototype.replace;
  try {
    return { out: f(), read };
  } finally {
    String.prototype.replace = replace;
  }
}

test("how much of a diff is read does not depend on how big it is", () => {
  // Thirty lines of 2,000,000 characters, then a hundred thousand more: 66,000,000 characters in all.
  const rows = [...Array.from({ length: 30 }, () => row("a".repeat(2e6) + "é")), ...Array.from({ length: 1e5 }, () => row("short line of text"))];
  const { out, read } = reading(() => widest(rows));
  expect(read).toBeLessThanOrEqual(LOOK + LINE);
  // Nothing was read of what came after, and it is taken at its worst: three cells a character.
  expect(out.l.rest).toBeGreaterThanOrEqual(2e6);
});

test("the room left for lines that are not laid out is never less than the font draws any of them", () => {
  // A small generator (xorshift32), so a failure is the same one every time.
  let state = 2463534242;
  const next = () => ((state ^= state << 13), (state ^= state >>> 17), (state ^= state << 5), (state >>> 0) / 4294967296);
  const alphabet = [...measured.keys()];
  const pick = (kind: number) => (kind < 0.55 ? String.fromCharCode(97 + Math.floor(next() * 26)) : alphabet[Math.floor(next() * alphabet.length)]);
  const failures: string[] = [];
  for (let trial = 0; trial < 400; trial++) {
    // Each line leans on one kind of character or another, so that lines of different kinds meet.
    const lines = Array.from({ length: 1 + Math.floor(next() * 40) }, () => {
      const lean = next(), length = Math.floor(next() * 300);
      return Array.from({ length }, () => pick(next() < lean ? next() : 0)).join("");
    });
    const { l } = widest(lines.map((t) => row(t)));
    const widestTrue = Math.max(...lines.map(truly));
    const laidOut = Math.max(0, ...l.show.map(truly));
    // What is laid out is measured as it is, the rest is taken at `rest` cells.
    if (widestTrue > Math.max(laidOut, l.rest) + 1e-9) failures.push(`trial ${trial}: ${widestTrue} cells widest, ${laidOut} laid out, ${l.rest} for the rest`);
  }
  expect(failures).toEqual([]);
});
