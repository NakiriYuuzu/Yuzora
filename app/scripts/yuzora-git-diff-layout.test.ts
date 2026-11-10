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

type Rank = { keys: Float64Array; base: number; rows: object[] };
type Gauge = { key: string; l: Side; r: Side };
type Side = { left: number; got: number; spent: number; blind: number; rounds: number };
type Pane = {
  fit: { l: Rank; r: Rank };
  gauge: Gauge | null;
  unit: { key: string; w: number } | null;
  gauged(): Gauge;
  settled(name: string): boolean;
  batch(name: string): { from: number; lines: object[]; cost: number };
  measured(g: Gauge, name: string, batch: object, size: { width: number }, cx: { notify(): void }): void;
  span(name: string): number;
};
const zoomed = { zoom: 1 };
const { ratioOf, bound, ranks, entry, cost, GitDiff, LINE, LAYOUT, WORST, HALF, FRAME, MEASURE, LOOK } = (() => {
  const body = src.replace(/^import [\s\S]*?;\n/gm, "").replace("export default class GitDiff", "class GitDiff");
  return new Function("View", "settings", `${body}; return { ratioOf, bound, ranks, entry, cost, GitDiff, LINE, LAYOUT, WORST, HALF, FRAME, MEASURE, LOOK };`)(class {}, () => zoomed);
})() as {
  ratioOf: (v: unknown) => number;
  bound: (text: string, meter: { left: number }) => number;
  ranks: (rows: object[], meter?: { left: number }) => { l: Rank; r: Rank };
  entry: (rank: Rank, at: number) => { cells: number; r: Record<string, unknown> };
  cost: (chars: number, pieces: number) => number;
  GitDiff: { prototype: object };
  LINE: number;
  LAYOUT: number;
  WORST: number;
  HALF: number;
  FRAME: number;
  MEASURE: number;
  LOOK: number;
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

const row = (lt?: string, rt?: string, pieces?: string[]) => ({
  k: "~",
  ...(lt !== undefined && { lt }),
  ...(rt !== undefined && { rt }),
  ...(pieces && { ls: pieces.map((t, i) => ({ t, h: i % 2 === 1 })) }),
});
// The texts of a side in the order they are laid out in: widest first by what they can take at most.
const order = (rows: object[], side: "l" | "r" = "l") => {
  const r = ranks(rows, plenty())[side];
  return Array.from({ length: r.keys.length }, (_, i) => entry(r, r.keys.length - 1 - i));
};
const texts = (rows: object[], side: "l" | "r" = "l") => order(rows, side).map((e) => e.r[`${side}t`] ?? e.r.t);

test("the lines are laid out in the order of the cells they may take, not of their characters, and all of them are in it", () => {
  const rows = [row("a".repeat(10), "b"), row("漢".repeat(6), "b"), row("a".repeat(11), "b")];
  expect(texts(rows)).toEqual(["漢".repeat(6), "a".repeat(11), "a".repeat(10)]);
  const many = Array.from({ length: 5000 }, (_, i) => row("a".repeat(1 + (i % 40)), "b"));
  expect(order(many)).toHaveLength(5000);
  expect(order(many)[0].cells).toBe(40);
  expect(order(many).map((e) => e.cells)).toEqual(order(many).map((e) => e.cells).sort((a, b) => b - a));
});

test("a line drawn in pieces may take half a cell more for each piece after the first, which the window rounds up", () => {
  const one = "a".repeat(100), pieces = Array.from({ length: 34 }, (_, i) => (i < 33 ? "aaa" : "a"));
  const [plain] = order([row(one, "b")]);
  const [piecewise] = order([row(one, "b", pieces)]);
  expect(plain.cells).toBe(100);
  expect(piecewise.cells).toBe(100 + HALF * (pieces.length - 1));
});

test("a bound is a whole number of quarters of a cell, which is how it is kept in the order", () => {
  const rows = [row("é".repeat(3), "b"), row("a漢\u{1f600}\u26a1é", "b", ["a", "漢", "\u{1f600}", "\u26a1é"]), row("x".repeat(7), "y", ["x", "x".repeat(6)])];
  for (const e of [...order(rows), ...order(rows, "r")]) expect(Number.isInteger(e.cells * 4)).toBe(true);
});

test("a line that is the same on both sides, in an unchanged row, is read once", () => {
  const same = Array.from({ length: 1000 }, (_, i) => ({ k: "=", t: `${"é".repeat(20)} ${i}` }));
  const apart = same.map((r) => row(r.t, `${r.t} `));
  const once = reading(() => ranks(same)).read, twice = reading(() => ranks(apart)).read;
  expect(once).toBeLessThan(twice * 0.6);
  expect(order(same, "r").map((e) => e.cells)).toEqual(order(same, "l").map((e) => e.cells));
});

test("an unchanged row counts on both sides, a side that has no line counts on none", () => {
  const rows = [{ k: "=", t: "same line" }, row("only on the left"), row(undefined, "only right is a longer line")];
  expect(texts(rows, "l")).toEqual(["only on the left", "same line"]);
  expect(texts(rows, "r")).toEqual(["only right is a longer line", "same line"]);
});

// What the window draws a line as wide as: each piece is as wide as its text in the font, rounded up to a pixel
// of the window (a pixel is 1/zoom of what the pane writes lengths in), and the line has 6 of indent.
const CELL = 6.9;
const drawn = (r: Record<string, unknown>, side: "l" | "r", zoom: number) => {
  const pieces = (r[`${side}s`] as { t: string }[] | undefined)?.map((p) => p.t) ?? [String(r[`${side}t`] ?? r.t)];
  return 6 + pieces.reduce((w, p) => w + Math.ceil(truly(p) * CELL * zoom - 1e-9) / zoom, 0);
};

// Measures the rows' lines as the pane does, a batch a frame, laying out each batch as the window does (here, by the
// table of widths and the rounding), until nothing is asked for.
function measure(rows: object[], zoom: number, ruler = CELL) {
  zoomed.zoom = zoom;
  const pane = Object.create(GitDiff.prototype) as Pane;
  Object.assign(pane, { fit: ranks(rows), gauge: null, unit: null });
  const cx = { notify() {} };
  let frames = 0;
  const laid = { l: 0, r: 0 };
  for (; frames < 1000; frames++) {
    const g = pane.gauged();
    let asked = false;
    for (const name of ["l", "r"] as const) {
      if (pane.settled(name)) continue;
      const batch = pane.batch(name);
      if (!batch.lines.length) continue;
      asked = true;
      laid[name] += batch.lines.length;
      const width = Math.max(...batch.lines.map((r) => drawn(r as Record<string, unknown>, name, zoom)));
      pane.measured(g, name, batch, { width: width * zoom }, cx);
    }
    if (pane.unit?.key !== g.key) {
      asked = true;
      pane.unit = { key: g.key, w: ruler };
    }
    if (!asked) break;
  }
  return { pane, frames, laid };
}

const widestDrawn = (rows: Record<string, unknown>[], side: "l" | "r", zoom: number) =>
  Math.max(0, ...rows.filter((r) => r[`${side}t`] !== undefined || r.t !== undefined).map((r) => drawn(r, side, zoom)));

test("an ordinary diff is measured in a frame: the lines that cannot be wider than the widest laid out are not laid out", () => {
  const rows = Array.from({ length: 6000 }, (_, i) => row("a".repeat(20 + ((i * 37) % 100)), "b".repeat(20 + ((i * 11) % 100))));
  const { pane, frames, laid } = measure(rows, 1);
  expect(frames).toBeLessThanOrEqual(2);
  expect(laid.l).toBeLessThanOrEqual(1);
  expect(pane.span("l")).toBeCloseTo(widestDrawn(rows, "l", 1), 6);
});

test("lines in a script the table does not list are laid out until the widest is found, not taken at three cells", () => {
  const rows = (n: number) => Array.from({ length: n }, (_, i) => row("م".repeat(150 - (i % 50)), "b"));
  for (const n of [5, 300]) {
    const { pane, laid } = measure(rows(n), 1);
    expect(pane.span("l")).toBeCloseTo(widestDrawn(rows(n) as Record<string, unknown>[], "l", 1), 6);
    expect(laid.l).toBeLessThanOrEqual(n);
  }
});

test("what each frame lays out is a few lines, whatever the diff, and a line too long to lay out is passed over", () => {
  const rows = Array.from({ length: 20000 }, (_, i) => row("a".repeat(30 + (i % 60)), "b"));
  const pane = Object.assign(Object.create(GitDiff.prototype), { fit: ranks(rows), gauge: null, unit: null }) as Pane;
  const g = pane.gauged();
  expect(pane.batch("l").lines).toHaveLength(1);
  // After a few frames a batch is as many as a frame lays out, and no more.
  g.l.rounds = 10;
  const batch = pane.batch("l");
  expect(batch.cost).toBeLessThanOrEqual(FRAME);
  expect(batch.lines.length).toBeGreaterThan(FRAME / 2 / cost(90, 1));
  expect(batch.lines.length).toBeLessThanOrEqual(FRAME / cost(30, 1) + 1);
  const long = "a".repeat(LAYOUT + 1);
  const over = Object.assign(Object.create(GitDiff.prototype), { fit: ranks([row(long, "b"), row("short", "b")]), gauge: null, unit: null }) as Pane;
  const gauge = over.gauged();
  expect(over.batch("l").lines).toEqual([row("short", "b")]);
  expect(gauge.l.blind).toBe(LAYOUT + 1);
  // What it could take at most is still counted in the room, since nothing says how wide it is.
  over.unit = { key: gauge.key, w: CELL };
  expect(over.span("l")).toBeGreaterThanOrEqual((LAYOUT + 1) * CELL);
});

test("when what may be laid out runs out, the range is what the lines left could take, never less", () => {
  // A hundred lines of 6,000 Chinese characters: 1.67 cells each in the font, 2 at most while they are read. Reading
  // takes 156,000 of the 4,000,000 that may be read, so the first 25 are, and the other 75 are taken at 3 cells a
  // character without being read, which puts them first in line. Laying a line out takes 6,080 of the 262,144 that
  // may, so 43 are laid out, and the 57 left may be as wide as any: the widest of them is taken at 18,000 cells.
  const rows = Array.from({ length: 100 }, () => row("漢".repeat(6000), "b"));
  const { pane, laid } = measure(rows, 1);
  expect(laid.l).toBe(Math.ceil(MEASURE / cost(6000, 1)));
  expect(pane.settled("l")).toBe(false);
  expect(pane.gauge!.l.left).toBeGreaterThanOrEqual(0);
  expect(pane.span("l")).toBeGreaterThanOrEqual(widestDrawn(rows as Record<string, unknown>[], "l", 1));
  expect(pane.span("l")).toBeCloseTo(6 + 18000 * CELL, 6);
});

// Replaces the methods of strings and of regular expressions that go through a whole string for the length of
// `f`, adding up the characters they were asked to read (a regular expression's own, only when it is not a string's).
function reading<T>(f: () => T): { out: T; read: number } {
  let read = 0, inside = 0;
  const saved: [object, string, unknown][] = [];
  const count = (proto: object, names: string[], size: (self: unknown, args: unknown[]) => number) => {
    for (const name of names) {
      const method = (proto as Record<string, unknown>)[name] as (...a: unknown[]) => unknown;
      saved.push([proto, name, method]);
      (proto as Record<string, unknown>)[name] = function (this: unknown, ...args: unknown[]) {
        if (!inside) read += size(this, args);
        inside++;
        try {
          return method.apply(this, args);
        } finally {
          inside--;
        }
      };
    }
  };
  count(String.prototype, ["replace", "replaceAll", "split", "indexOf", "lastIndexOf", "includes", "match", "matchAll", "search", "normalize", "toLowerCase", "toUpperCase", "trim", "localeCompare"], (self) => (self as string).length);
  count(String.prototype, ["slice", "substring", "substr", "codePointAt", "charCodeAt", "charAt", "at"], (_self, args) => Math.max(1, Math.abs(Number(args[1] ?? 1) - Number(args[0] ?? 0)) || 1));
  count(RegExp.prototype, ["exec", "test"], (_self, args) => String(args[0]).length);
  try {
    return { out: f(), read };
  } finally {
    for (const [proto, name, method] of saved.reverse()) (proto as Record<string, unknown>)[name] = method;
  }
}

test("how much of a diff is read does not depend on how big it is, however it is read", () => {
  // Thirty lines of 2,000,000 characters, then a hundred thousand more: 66,000,000 characters in all.
  const rows = [...Array.from({ length: 30 }, () => row("a".repeat(2e6) + "é")), ...Array.from({ length: 1e5 }, () => row("short line of text"))];
  const { out, read } = reading(() => ranks(rows));
  // A line's head is copied once and scanned once.
  expect(read).toBeLessThanOrEqual(2.5 * LOOK);
  expect(out.l.keys.length).toBe(1e5 + 30);
  // Nothing was read of what came after, and each line of it is taken at its worst: three cells a character.
  expect(entry(out.l, 0).cells).toBe(WORST * "short line of text".length);
});

test("the count of what is read sees every way a line can be read", () => {
  const line = "abc".repeat(1000);
  expect(reading(() => line.split("")).read).toBe(3000);
  expect(reading(() => line.replace(/b/g, "")).read).toBe(3000);
  expect(reading(() => /c/.test(line)).read).toBe(3000);
  expect(reading(() => line.slice(0, 10)).read).toBe(10);
});

test("the widest line is found whatever the table of cells says of the rest: any lines, any zoom, in the font as measured", () => {
  // A small generator (xorshift32), so a failure is the same one every time.
  let state = 2463534242;
  const next = () => ((state ^= state << 13), (state ^= state >>> 17), (state ^= state << 5), (state >>> 0) / 4294967296);
  const alphabet = [...measured.keys()];
  const pick = (kind: number) => (kind < 0.55 ? String.fromCharCode(97 + Math.floor(next() * 26)) : alphabet[Math.floor(next() * alphabet.length)]);
  const failures: string[] = [];
  for (let trial = 0; trial < 400; trial++) {
    // Each line leans on one kind of character or another, so that lines of different kinds meet.
    // Half the trials have lines of one character each, so that lines of one kind meet lines of another.
    const uniform = trial % 2 === 1, zoom = [0.5, 0.8, 1, 1.2, 1.5, 2, 3][trial % 7];
    const lines = Array.from({ length: 1 + Math.floor(next() * 40) }, () => {
      const lean = next(), length = Math.floor(next() * 300);
      if (uniform) return pick(next()).repeat(length);
      return Array.from({ length }, () => pick(next() < lean ? next() : 0)).join("");
    }).filter(Boolean);
    // Some lines are drawn in pieces, as the word diff cuts them.
    const rows = lines.map((t, i) => {
      if (i % 3) return row(t, "b");
      const pieces: string[] = [];
      for (let at = 0; at < t.length; ) {
        const n = 1 + Math.floor(next() * 12);
        pieces.push(t.slice(at, at + n));
        at += n;
      }
      return row(t, "b", pieces);
    });
    const { pane } = measure(rows, zoom);
    const want = widestDrawn(rows, "l", zoom), got = pane.span("l");
    // Never short by more than the pixel it may be (a line of the same bound as the widest laid out may be
    // that much wider), and no more than that over once nothing is left.
    if (got < want - 2 / zoom - 1e-6) failures.push(`trial ${trial} at zoom ${zoom}: the range is ${got} for lines drawn ${want} wide`);
    if (pane.settled("l") && got > want + 1e-6) failures.push(`trial ${trial} at zoom ${zoom}: settled at ${got} for lines drawn ${want} wide`);
    if (!pane.settled("l") && pane.gauge!.l.spent < MEASURE && pane.gauge!.l.left >= 0) failures.push(`trial ${trial}: stopped with lines left and room to lay them out`);
  }
  expect(failures).toEqual([]);
});
