// The menu a right-click on a diff row opens (defaults/mods/yuzora-git-diff, props.menu): what a row does when
// git cannot give the text. The popup is exactly as tall as its three rows, so nothing can be said under them.
// Run: bun test scripts/yuzora-git-diff-menu.test.ts
import { test, expect } from "bun:test";
import { readFile } from "node:fs/promises";

const src = await readFile(new URL("../defaults/mods/yuzora-git-diff/main.js", import.meta.url), "utf8");
const body = src.replace(/^import [\s\S]*?;\n/gm, "").replace("export default class GitDiff", "class GitDiff");
const flush = async () => { for (let i = 0; i < 20; i++) await Promise.resolve(); };

function menu(patch: () => Promise<string>) {
  const w = { texts: [] as string[], clicks: {} as Record<string, (e: unknown, cx: unknown) => void>, toasts: [] as any[], closed: [] as unknown[], clipboard: null as string | null };
  const element = (): any => {
    let id = "";
    const self: any = new Proxy({}, {
      get: (_t, prop) => (prop === "then" ? undefined : (...args: any[]) => {
        if (prop === "id") id = args[0];
        else if (prop === "child" && typeof args[0] === "string") w.texts.push(args[0]);
        else if (prop === "on_click") w.clicks[id] = args[0];
        else if (prop === "when" && args[0]) args[1](self);
        return self;
      }),
    });
    return self;
  };
  const stubs: Record<string, any> = {
    View: class {}, div: element, v_flex: element, theme: () => new Proxy({}, { get: () => "#000000" }),
    props: () => ({ overlay: "o1" }), closeOverlay: (o: unknown) => w.closed.push(o), notify: (n: any) => w.toasts.push(n),
    t: (key: string) => key, diffText: () => "the lines", diffPatch: patch,
  };
  const GitDiff = new Function(...Object.keys(stubs), `${body}; return GitDiff;`)(...Object.values(stubs));
  const view = Object.create(GitDiff.prototype);
  view.offer = { id: 7, runs: [["l", 0, 1]], hunk: [["l", 0, 2]], hunks: [0], whitespace: "none" };
  const cx: any = { spawn: (fn: (cx: any) => Promise<any>) => { fn(cx); }, notify() {}, write_to_clipboard: (text: string) => { w.clipboard = text; } };
  const draw = () => { w.texts.length = 0; w.clicks = {}; view.renderOffer(cx); return [...w.texts]; };
  const click = async (id: string) => { draw(); w.clicks[id]({}, cx); await flush(); };
  return { w, draw, click };
}

test("a row that cannot copy says why in a toast and closes the menu, with nothing drawn under the three rows", async () => {
  const m = menu(async () => { throw new Error("this diff is gone; reload it"); });
  await m.click("patch");
  expect(m.w.toasts).toEqual([{ title: "copy.failed", body: "this diff is gone; reload it", level: "error" }]);
  expect(m.w.closed).toEqual(["o1"]);
  expect(m.w.clipboard).toBeNull();
  expect(m.draw()).toEqual(["menu.copy_one", "menu.hunk", "menu.patch"]);
});

test("a row that copies still puts the text on the clipboard, says so and closes the menu", async () => {
  const m = menu(async () => "the patch");
  await m.click("patch");
  expect(m.w.clipboard).toBe("the patch");
  expect(m.w.toasts.map((n) => [n.title, n.level])).toEqual([["copied.patch", "success"]]);
  expect(m.w.closed).toEqual(["o1"]);
});
