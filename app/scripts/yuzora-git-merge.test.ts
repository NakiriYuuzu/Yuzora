// yuzora(L2): the Git merge pane's result and edit logic (defaults/mods/yuzora-git-merge).
// Run: bun test scripts/yuzora-git-merge.test.ts
import { test, expect } from "bun:test";
import { readFile } from "node:fs/promises";

const source = (await readFile(new URL("../defaults/mods/yuzora-git-merge/main.js", import.meta.url), "utf8"))
  .replace(/^import [\s\S]*?;\n/gm, "").replace("export default class GitMerge", "class GitMerge");

// The pane without its window: a chunk list, an edit box, a cx that only counts.
function pane(chunks: object[]) {
  const GitMerge = new Function("View", `${source}; return GitMerge;`)(class {});
  const view = new GitMerge();
  let box = "";
  view.edit = { value: () => box, set_value(v: string) { box = v; }, focus() {} };
  view.handle = { focus() {} };
  view.session = { chunks };
  view.choices = {};
  const cx = { notify() {}, spawn() {} };
  return { view, cx, box: () => box, type(v: string) { box = v; } };
}

const chunk = (index: number, ours: string[], theirs: string[], eol = { base: true, ours: true, theirs: true }) =>
  ({ index, kind: "conflict", base: ["mid"], ours, theirs, eol });

test("the box shows each line with its end; Use sends them back ending as the source did", () => {
  const { view, cx, box, type } = pane([chunk(0, ["OURS"], ["THEIRS"])]);
  view.startEdit(0, cx);
  expect(box()).toBe("OURS\n");
  type("MINE");
  view.useEdit(cx);
  expect(view.choices[0]).toEqual({ text: "MINE\n" });
  expect(view.resultOf(view.session.chunks[0])).toEqual(["MINE"]);
});

test("a file's last line without a newline stays without one", () => {
  const { view, cx, type } = pane([chunk(0, ["OURS"], ["THEIRS"], { base: false, ours: false, theirs: false })]);
  view.startEdit(0, cx);
  type("MINE");
  view.useEdit(cx);
  expect(view.choices[0]).toEqual({ text: "MINE" });
});

test("an emptied box drops the chunk; one blank line is a newline", () => {
  const { view, cx, type } = pane([chunk(0, ["OURS"], ["THEIRS"])]);
  view.startEdit(0, cx);
  type("");
  view.useEdit(cx);
  expect(view.choices[0]).toEqual({ text: "" });
  expect(view.resultOf(view.session.chunks[0])).toEqual([]);
  view.startEdit(0, cx);
  type("\n");
  view.useEdit(cx);
  expect(view.choices[0]).toEqual({ text: "\n" });
  expect(view.resultOf(view.session.chunks[0])).toEqual([""]);
});

test("a side that deleted the chunk opens an empty box that stays empty", () => {
  const { view, cx, box } = pane([chunk(0, [], ["THEIRS"])]);
  view.startEdit(0, cx);
  expect(box()).toBe("");
  view.useEdit(cx);
  expect(view.choices[0]).toEqual({ text: "" });
});

test("opening the editor and pressing Use without a change keeps the result", () => {
  const ends = { base: true, ours: true, theirs: true }, open = { base: false, ours: false, theirs: false };
  for (const [ours, eol] of [[[], ends], [[""], ends], [["a"], ends], [["a", ""], ends], [["a", "b"], open]] as const) {
    const c = chunk(0, [...ours], ["X"], eol);
    const { view, cx } = pane([c]);
    view.choices = { 0: "ours" };
    view.startEdit(0, cx);
    view.useEdit(cx);
    // An empty chunk has no last line to end.
    expect([ours, view.resultOf(c), ours.length ? view.endsLine(c) : "-"]).toEqual([ours, [...ours], ours.length ? eol.ours : "-"]);
  }
});

test("whether a choice ends its last line follows the side it takes", () => {
  const last = chunk(0, ["OURS"], ["THEIRS"], { base: true, ours: false, theirs: true });
  const { view } = pane([last]);
  expect(view.endsLine(last)).toBe(false);
  for (const [choice, ends] of [["ours", false], ["theirs", true], ["base", true], ["both", true], ["both-theirs-first", false], ["auto", false]] as const) {
    view.choices = { 0: choice };
    expect([choice, view.endsLine(last)]).toEqual([choice, ends]);
  }
  view.choices = { 0: { text: "x\n" } };
  expect(view.endsLine(last)).toBe(true);
});
