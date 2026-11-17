// yuzora(unsaved): what each button of the unsaved-changes dialog does, in order, through the handler its click runs.
// main.js runs as shipped with herdr, yuzora:core, yuzora:files and the element builders stubbed.
// Run: bun test scripts/yuzora-unsaved.test.ts
import { test, expect } from "bun:test";
import { readFile } from "node:fs/promises";

const main = await readFile(new URL("../defaults/mods/yuzora-unsaved/main.js", import.meta.url), "utf8");
const code = main.replace(/^import [\s\S]*?;\n/gm, "").replace(/^export default class/m, "class");

const flush = async () => { for (let i = 0; i < 30; i++) await Promise.resolve(); };
const deep: any = new Proxy({}, { get: () => deep });

type Doc = { doc: number; name: string; path: string; on: string | null };
type State = { dirty: boolean; deleted: boolean };

function dialog(docs: Doc[], states: Record<number, State>, saved: (doc: number) => any = () => ({ ok: true, conflict: false, error: null })) {
  const calls: any[][] = [];
  const clicks: Record<string, (e: any, cx: any) => void> = {};
  let keys: ((e: any, cx: any) => void) | null = null;
  let texts: string[] = [];
  const node = (): any => {
    let id = "";
    const el: any = new Proxy({}, {
      get: (_t, prop) => (prop === "then" ? undefined : (...args: any[]) => {
        if (prop === "id") id = args[0];
        if (prop === "on_click") clicks[id] = args[0];
        if (prop === "on_key_down") keys = args[0];
        if (prop === "child" && typeof args[0] === "string") texts.push(args[0]);
        if (prop === "when" && args[0]) args[1](el);
        return el;
      }),
    });
    return el;
  };
  const stubs: Record<string, any> = {
    View: class {}, div: node, h_flex: node, v_flex: node,
    theme: () => deep, props: () => ({ docs, command: { name: "pane.close", args: { pane: "e1" } }, overlay: "ov1" }),
    closeOverlay: (id: string) => calls.push(["closeOverlay", id]),
    run: (name: string, args: any) => calls.push(["run", name, args]),
    t: (k: string, vars?: any) => (vars ? `${k} ${JSON.stringify(vars)}` : k),
    documentState: (doc: number) => states[doc] ?? null,
    saveDocument: async (doc: number, options: any) => {
      calls.push(["save", doc, options]);
      const r = saved(doc);
      states[doc].dirty = !r.ok || Boolean(r.typedMeanwhile);
      return r;
    },
    discardDocument: (doc: number) => { calls.push(["discard", doc]); states[doc].dirty = false; },
  };
  const Unsaved = new Function(...Object.keys(stubs), `${code}\nreturn Unsaved;`)(...Object.values(stubs));
  const cx: any = { focus_handle: () => ({ focus() {} }), spawn: (fn: (cx: any) => Promise<any>) => { fn(cx); }, notify() {}, stop_propagation() {} };
  const view = new Unsaved();
  view.init(null, cx);
  const frame = () => { texts = []; view.render(cx); return texts; };
  frame();
  const click = async (id: string) => { clicks[id](null, cx); await flush(); frame(); };
  const key = async (k: string) => { keys!({ key: k }, cx); await flush(); frame(); };
  return { calls, click, key, frame };
}

const a: Doc = { doc: 1, name: "a.txt", path: "/w/a.txt", on: null };
const b: Doc = { doc: 2, name: "b.md", path: "/srv/b.md", on: "lab" };
const dirty = () => ({ 1: { dirty: true, deleted: false }, 2: { dirty: true, deleted: false } });

test("Save writes every file, then closes the dialog, then runs the command again", async () => {
  const d = dialog([a, b], dirty());
  await d.click("us-save");
  expect(d.calls).toEqual([
    ["save", 1, { force: false }],
    ["save", 2, { force: false }],
    ["closeOverlay", "ov1"],
    ["run", "pane.close", { pane: "e1" }],
  ]);
});

test("Save writes a deleted file back where it was, and never forces over a change on disk", async () => {
  const d = dialog([a, b], { 1: { dirty: true, deleted: true }, 2: { dirty: true, deleted: false } });
  await d.click("us-save");
  expect(d.calls.filter((c) => c[0] === "save")).toEqual([["save", 1, { force: true }], ["save", 2, { force: false }]]);
});

test("a save that fails keeps the dialog, shows why beside the file, and the command does not run", async () => {
  const d = dialog([a, b], dirty(), (doc) => (doc === 2 ? { ok: false, conflict: false, error: "Permission denied" } : { ok: true, conflict: false, error: null }));
  await d.click("us-save");
  expect(d.calls.map((c) => c[0])).toEqual(["save", "save"]);
  expect(d.frame()).toContain(`error.save {"error":"Permission denied"}`);
  const conflict = dialog([a], dirty(), () => ({ ok: false, conflict: true, error: null }));
  await conflict.click("us-save");
  expect(conflict.calls.map((c) => c[0])).toEqual(["save"]);
  expect(conflict.frame()).toContain("error.conflict");
});

test("after a failed save, Save again writes what is still unsaved and goes on", async () => {
  let refuse = true;
  const d = dialog([a, b], dirty(), (doc) => (doc === 2 && refuse ? { ok: false, conflict: false, error: "busy" } : { ok: true, conflict: false, error: null }));
  await d.click("us-save");
  refuse = false;
  await d.click("us-save");
  expect(d.calls).toEqual([
    ["save", 1, { force: false }],
    ["save", 2, { force: false }],
    ["save", 2, { force: false }],
    ["closeOverlay", "ov1"],
    ["run", "pane.close", { pane: "e1" }],
  ]);
  expect(d.frame().some((s) => s.startsWith("error."))).toBe(false);
});

test("a file typed into while it was saved keeps the dialog open", async () => {
  const d = dialog([a], dirty(), () => ({ ok: true, conflict: false, error: null, typedMeanwhile: true }));
  await d.click("us-save");
  expect(d.calls.map((c) => c[0])).toEqual(["save"]);
});

test("Don't Save drops every file's changes, then closes the dialog, then runs the command again", async () => {
  const d = dialog([a, b], dirty());
  await d.click("us-dont-save");
  expect(d.calls).toEqual([["discard", 1], ["discard", 2], ["closeOverlay", "ov1"], ["run", "pane.close", { pane: "e1" }]]);
});

test("Cancel and Esc only close the dialog", async () => {
  for (const close of [(d: any) => d.click("us-cancel"), (d: any) => d.key("escape")]) {
    const d = dialog([a, b], dirty());
    await close(d);
    expect(d.calls).toEqual([["closeOverlay", "ov1"]]);
  }
});

test("Enter is Save", async () => {
  const d = dialog([a], dirty());
  await d.key("enter");
  expect(d.calls.map((c) => c[0])).toEqual(["save", "closeOverlay", "run"]);
});

test("one file is named in the title, several are counted, each listed with where it is", () => {
  expect(dialog([a], dirty()).frame()).toContain(`title_one {"name":"a.txt"}`);
  const texts = dialog([a, b], dirty()).frame();
  expect(texts).toContain(`title_many {"count":2}`);
  expect(texts).toContain("/w");
  expect(texts).toContain(`/srv · on {"machine":"lab"}`);
});
