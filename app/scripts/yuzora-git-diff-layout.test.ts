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

type Rank = { keys: Float64Array; base: number; rows: object[]; top: number };
type Fit = { l: Rank; r: Rank; at: number; count: number; done: boolean; rows: object[]; slice: number; hurt: number; trying: { left: number; at: number; slice: number } | null };
type Gauge = { key: string; l: Side; r: Side };
type Side = { left: number | undefined; got: number; blind: number; rounds: number };
type Pane = {
  fit: Fit;
  gauge: Gauge | null;
  unit: { key: string; w: number } | null;
  gauged(): Gauge;
  settled(name: string): boolean;
  batch(name: string): { from: number; lines: object[]; cost: number };
  measured(g: Gauge, name: string, batch: object, size: { width: number }, cx: { notify(): void }): void;
  span(name: string): number;
};
const zoomed = { zoom: 1 };
const { ratioOf, bound, ranking, scan, cutSlice, entry, cost, GitDiff, LINE, LAYOUT, TEXTS, WORST, HALF, FRAME, SLICE, STEP, TOLL, SLOW } = (() => {
  const body = src.replace(/^import [\s\S]*?;\n/gm, "").replace("export default class GitDiff", "class GitDiff");
  return new Function("View", "settings", `${body}; return { ratioOf, bound, ranking, scan, cutSlice, entry, cost, GitDiff, LINE, LAYOUT, TEXTS, WORST, HALF, FRAME, SLICE, STEP, TOLL, SLOW };`)(class {}, () => zoomed);
})() as {
  ratioOf: (v: unknown) => number;
  bound: (text: string, meter: { left: number }) => number;
  ranking: (rows: object[]) => Fit;
  scan: (fit: Fit, meter: { left: number }) => void;
  cutSlice: (fit: Fit) => void;
  entry: (rank: Rank, at: number) => { cells: number; r: Record<string, unknown> };
  cost: (chars: number, pieces: number) => number;
  GitDiff: { prototype: object };
  LINE: number;
  LAYOUT: number;
  TEXTS: number;
  WORST: number;
  HALF: number;
  FRAME: number;
  SLICE: number;
  STEP: number;
  TOLL: number;
  SLOW: number;
};

const plenty = () => ({ left: Infinity });
// The rows ranked, all of them at once.
const ranks = (rows: object[]) => {
  const fit = ranking(rows);
  scan(fit, plenty());
  return fit;
};
// The rows ranked a slice at a time, as the pane does: how many slices it took, and the most rows one went through.
const ranksInSlices = (rows: object[], slice = SLICE) => {
  const fit = ranking(rows);
  let slices = 0, most = 0;
  while (!fit.done) {
    const from = fit.at;
    scan(fit, { left: slice });
    most = Math.max(most, fit.at - from);
    slices++;
  }
  return { fit, slices, most };
};
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

test("the rest of a line past what is read of it is taken at its worst, and what reading costs is taken from the meter", () => {
  expect(bound("a".repeat(LINE + 10), plenty())).toBe(LINE + WORST * 10);
  const meter = { left: 10000 };
  expect(bound("a".repeat(100), meter)).toBe(100);
  expect(meter.left).toBe(10000 - TOLL - 100);
  expect(bound("é漢".repeat(10), meter)).toBeGreaterThan(20);
  expect(meter.left).toBe(10000 - TOLL - 100 - TOLL - 20 - SLOW * 20);
});

const row = (lt?: string, rt?: string, pieces?: string[]) => ({
  k: "~",
  ...(lt !== undefined && { lt }),
  ...(rt !== undefined && { rt }),
  ...(pieces && { ls: pieces.map((t, i) => ({ t, h: i % 2 === 1 })) }),
});
// The texts of a side in the order they are laid out in: widest first by what they can take at most.
const order = (rows: object[], side: "l" | "r" = "l") => {
  const r = ranks(rows)[side];
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

test("a line that three cells a character cannot make as wide as the widest read is not read", () => {
  // Six lines of 900 characters, then five thousand of 200: 600 cells at most each, under the 900 of the four widest.
  const rows = [...Array.from({ length: 6 }, () => row("a".repeat(900))), ...Array.from({ length: 5000 }, () => row("b".repeat(200)))];
  const { out, read } = reading(() => ranks(rows));
  expect(read).toBeLessThan(20000);
  expect(out.l.keys.length).toBe(5006);
  expect(entry(out.l, 0).cells).toBe(WORST * 200);
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
function measure(rows: object[], zoom: number, ruler = CELL, each: (pane: Pane) => void = () => {}) {
  zoomed.zoom = zoom;
  const pane = Object.create(GitDiff.prototype) as Pane;
  Object.assign(pane, { fit: ranks(rows), gauge: null, unit: null });
  const cx = { notify() {}, timer: { after: () => ({ cancel() {} }) } };
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
    each(pane);
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

test("what each frame lays out is a few lines, whatever the diff, and a line that costs a frame by itself is laid out alone", () => {
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
});

test("a line that costs more than a frame is a batch of its own, and one past what can be laid out is passed over", () => {
  const heavy = "a".repeat(FRAME), short = row("short", "b");
  expect(cost(heavy.length, 1)).toBeGreaterThan(FRAME);
  const open = (rows: object[]) => {
    const pane = Object.assign(Object.create(GitDiff.prototype), { fit: ranks(rows), gauge: null, unit: null }) as Pane;
    pane.gauged().l.rounds = 10;
    return pane;
  };
  const alone = open([row(heavy, "b"), short, short]);
  expect(alone.batch("l").lines).toEqual([row(heavy, "b")]);
  expect(alone.batch("l").cost).toBeGreaterThan(FRAME);
  // The lines before it in the order go together and it comes after them, alone.
  const behind = open([row("a".repeat(FRAME - 500), "b"), row(heavy, "b")]);
  const first = behind.batch("l");
  expect(first.lines).toEqual([row(heavy, "b")]);
  behind.gauge!.l.left = first.from - first.lines.length;
  expect(behind.batch("l").lines).toEqual([row("a".repeat(FRAME - 500), "b")]);
  // Past LAYOUT characters or TEXTS pieces it is not laid out; what it could take at most is still in the room.
  const pieces = Array.from({ length: TEXTS + 1 }, () => "a");
  for (const past of [row("a".repeat(LAYOUT + 1), "b"), row(pieces.join(""), "b", pieces)]) {
    const over = open([past, short]);
    expect(over.batch("l").lines).toEqual([short]);
    expect(over.gauge!.l.blind).toBeGreaterThanOrEqual(entry(over.fit.l, 1).cells);
    over.unit = { key: over.gauge!.key, w: CELL };
    expect(over.span("l")).toBeGreaterThanOrEqual(entry(over.fit.l, 1).cells * CELL);
  }
});

test("a batch never costs more than a frame unless it is one line, whatever line comes first in it", () => {
  const costs = [FRAME - 100, FRAME - 1, FRAME / 2, FRAME / 4, 300, 90].map((c) => c - 64 - 16);
  const rows = costs.flatMap((n) => [row("a".repeat(n), "b"), row("a".repeat(n), "b")]);
  const { pane } = measure(rows, 1);
  const g = pane.gauge!;
  g.l.left = pane.fit.l.keys.length - 1;
  for (g.l.rounds = 0; g.l.left >= 0; g.l.rounds++) {
    const batch = pane.batch("l");
    expect(batch.lines.length).toBeGreaterThan(0);
    if (batch.lines.length > 1) expect(batch.cost).toBeLessThanOrEqual(FRAME);
    g.l.left = batch.from - batch.lines.length;
  }
});

test("lines are laid out until the widest is found, however many there are: nothing caps the total, only each frame", () => {
  // A hundred lines of 6,000 Chinese characters: 1.67 cells each in the font, 2 at most. All of them can be as wide as
  // the widest laid out, so all are laid out, five to a frame (a line costs 6,080 and a frame 32,768).
  const rows = Array.from({ length: 100 }, () => row("漢".repeat(6000), "b"));
  const { pane, laid, frames } = measure(rows, 1);
  expect(laid.l).toBe(100);
  expect(frames).toBeLessThanOrEqual(30);
  expect(pane.settled("l")).toBe(true);
  expect(pane.span("l")).toBeCloseTo(widestDrawn(rows as Record<string, unknown>[], "l", 1), 6);
  // Thousands of lines in a script the table does not list (three cells a character against one drawn): the range is
  // the widest drawn at the end, and never less than it at any frame on the way.
  const hebrew = Array.from({ length: 3000 }, (_, i) => row("א".repeat(150 - (i % 50)), "b"));
  const wide = widestDrawn(hebrew as Record<string, unknown>[], "l", 1);
  const lowest = { span: Infinity };
  const hebrewPane = measure(hebrew, 1, CELL, (pane) => (lowest.span = Math.min(lowest.span, pane.span("l"))));
  expect(hebrewPane.pane.span("l")).toBeCloseTo(wide, 6);
  expect(lowest.span).toBeGreaterThanOrEqual(wide - 1e-6);
  expect(hebrewPane.frames).toBeLessThanOrEqual(60);
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

test("how much of a diff is read in a slice does not depend on how big it is, however it is read", () => {
  // Thirty lines of 2,000,000 characters, then a hundred thousand more: 66,000,000 characters in all.
  const rows = [...Array.from({ length: 30 }, () => row("a".repeat(2e6) + "é")), ...Array.from({ length: 1e5 }, () => row("short line of text"))];
  const fit = ranking(rows);
  const slice = reading(() => scan(fit, { left: SLICE }));
  // A line's head is copied once and scanned once, and the slice ends with the line that used up its budget.
  expect(slice.read).toBeLessThanOrEqual(2 * (SLICE + LINE) + 1000);
  expect(fit.at).toBeLessThan(5);
  expect(fit.done).toBe(false);
});

test("the rows are ranked a slice at a time, no slice goes through more than its budget allows, and it ends as if done at once", () => {
  const rows = Array.from({ length: 30000 }, (_, i) => (i % 3 ? row("a".repeat(10 + (i % 90)), "b".repeat(i % 50)) : { k: "=", t: `${"é".repeat(i % 20)} unchanged ${i}` }));
  const slice = SLICE / 40, { fit, slices, most } = ranksInSlices(rows, slice);
  expect(slices).toBeGreaterThan(5);
  expect(most).toBeLessThanOrEqual(Math.ceil(slice / STEP));
  const whole = ranks(rows);
  expect(Array.from(fit.l.keys)).toEqual(Array.from(whole.l.keys));
  expect(Array.from(fit.r.keys)).toEqual(Array.from(whole.r.keys));
  // Rows that cost nothing to read still cost a step each: a million of them is not one slice.
  const empty = Array.from({ length: 5e5 }, () => ({ k: "=" }));
  expect(ranksInSlices(empty).slices).toBeGreaterThan(5e5 / (SLICE / STEP) - 1);
  expect(ranksInSlices([]).fit.done).toBe(true);
});

test("putting a side's lines in order is work of the slice: with the slice used up the other side waits for the next, and a small diff is done in its first", () => {
  // A slice of a tenth, so that 60,000 rows are many slices and putting one side in order (6 units a line) is more than one.
  const rows = Array.from({ length: 6e4 }, (_, i) => row("a".repeat(8 + (i % 5)), "b".repeat(8 + (i % 7))));
  const fit = ranking(rows);
  const ordered = () => [fit.l, fit.r].filter((side) => (side as { sorted?: boolean }).sorted).length;
  let calls = 0, most = 0;
  while (!fit.done) {
    const before = ordered();
    scan(fit, { left: SLICE / 10 });
    most = Math.max(most, ordered() - before);
    calls++;
  }
  expect(most).toBe(1);
  expect(calls).toBeGreaterThan(5);
  expect(Array.from(fit.l.keys)).toEqual(Array.from(ranks(rows).l.keys));
  expect(Array.from(fit.r.keys)).toEqual(Array.from(ranks(rows).r.keys));
  const small = ranking(Array.from({ length: 100 }, () => row("a", "b")));
  scan(small, { left: SLICE });
  expect(small.done).toBe(true);
});

// yuzora(v010): the engine cuts a slice off where the script calls or branches, and the next slice goes on from the last
// row that was finished. Whatever the place, every row is ranked once on each side that has a line, and what a line is
// taken at is never less than the font draws it (a cut can make it more, by a read that is not repeated, or less than it
// would have been, by one that is: both are bounds).
test("a slice cut off anywhere is gone on from without a row lost, and every line is still bounded from above", () => {
  const rows = [
    row("a".repeat(40), "b".repeat(30)), { k: "=", t: "unchanged line é" }, { k: "=", t: "" }, row("م".repeat(60), ""), row("", "x".repeat(8)),
    { k: "~", lt: "ab".repeat(20), ls: [{ t: "ab".repeat(10) }, { t: "ab".repeat(10), h: true }], rt: "cd".repeat(20), rs: [{ t: "cd".repeat(5) }, { t: "cd".repeat(15), h: true }] },
    row("✓ done ".repeat(9), "\u{1f600}".repeat(6)), { k: "=", t: "same on both sides ".repeat(4) }, row("w".repeat(70), "w".repeat(70)), row("short", "also short"),
  ];
  const cut = new Error("cut");
  const meterCutAt = (k: number) => {
    let left = 1e9, calls = 0;
    return { get left() { return left; }, set left(v: number) { if (++calls === k) throw cut; left = v; }, get calls() { return calls; } };
  };
  const probe = meterCutAt(0);
  scan(ranking(rows), probe);
  const decrements = probe.calls;
  expect(decrements).toBeGreaterThan(10);
  const whole = ranks(rows);
  for (let k = 1; k <= decrements; k++) {
    const fit = ranking(rows);
    expect(() => scan(fit, meterCutAt(k))).toThrow(cut);
    scan(fit, plenty());
    expect(fit.done).toBe(true);
    for (const name of ["l", "r"] as const) {
      const side = fit[name] as Rank & { n: number };
      const index = (key: number) => key - Math.floor(key / side.base) * side.base;
      const got = Array.from(side.keys).map(index).sort((a, b) => a - b), want = Array.from(whole[name].keys).map(index).sort((a, b) => a - b);
      expect(got).toEqual(want);
      for (let at = 0; at < side.keys.length; at++) {
        const { cells, r } = entry(side, at);
        const text = String((r[`${name}t`] as string | undefined) ?? r.t ?? "");
        expect(cells).toBeGreaterThanOrEqual(truly(text) - 1e-9);
      }
    }
  }
});

test("a slice that was cut off is followed by half of what it got done, and a row it got nowhere with is taken at its bound", () => {
  const fit = ranking(Array.from({ length: 50 }, () => row("a".repeat(10), "b")));
  fit.at = 20;
  fit.trying = { left: SLICE - 600000, at: 5, slice: SLICE };
  cutSlice(fit);
  expect([fit.slice, fit.hurt, fit.trying]).toEqual([300000, -1, null]);
  fit.trying = { left: SLICE - 50, at: 20, slice: 300000 };
  cutSlice(fit);
  expect([fit.slice, fit.hurt]).toEqual([300000, 20]);
  const hurt = ranking([row("a".repeat(10), "b"), row("é".repeat(30), "é".repeat(30)), row("a".repeat(10), "b")]);
  hurt.hurt = 1;
  scan(hurt, plenty());
  const cells = (side: "l" | "r") => Array.from({ length: 3 }, (_, at) => entry(hurt[side], at)).map(({ cells, r }) => [cells, (r as { lt?: string }).lt?.length ?? 0]);
  expect(cells("l")).toContainEqual([WORST * 30, 30]);
  expect(cells("r")).toContainEqual([WORST * 30, 30]);
  expect(cells("l")).toContainEqual([10, 10]);
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
    if (!pane.settled("l") && pane.gauge!.l.left >= 0) failures.push(`trial ${trial}: stopped with lines left to lay out`);
  }
  expect(failures).toEqual([]);
});
