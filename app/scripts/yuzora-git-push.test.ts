// The push dialog's main.js as shipped, with herdr, yuzora:git and the element builders stubbed.
// Run: bun test scripts/yuzora-git-push.test.ts
import { test, expect } from "bun:test";
import { readFile } from "node:fs/promises";

const main = await readFile(new URL("../defaults/mods/yuzora-git-push/main.js", import.meta.url), "utf8");
const code = main.replace(/^import [\s\S]*?;\n/gm, "").replace(/^export default class/m, "class");
const flush = async () => { for (let i = 0; i < 30; i++) await Promise.resolve(); };

async function dialog(commits: number) {
  const texts: string[] = [];
  const builder: any = new Proxy(function () {}, {
    get: (_t, prop) => (prop === "then" ? undefined : (...args: any[]) => {
      if (prop === "child" && typeof args[0] === "string") texts.push(args[0]);
      if (prop === "children") args[0].forEach((c: any) => typeof c === "string" && texts.push(c));
      if (prop === "when" && args[0]) args[1](builder);
      return builder;
    }),
    apply: () => builder,
  });
  const preview = {
    remote: "origin", branch: "main", target: "main", upstream: "origin/main", remotes: ["origin"], newBranch: false, tags: [], needsUpstream: false, hasUpstream: true,
    commits: Array.from({ length: commits }, (_, i) => ({ oid: `c${i}`, short: `c${i}`, subject: `subject ${i}`, author: "a", time: 0 })),
  };
  const stubs: Record<string, any> = {
    View: class {}, div: () => builder, v_flex: () => builder, h_flex: () => builder, Input: { new: () => builder },
    InputState: { new: () => ({ on() {}, value: () => "main", set_value() {} }) },
    props: () => ({ cwd: "/a", on: null }), closeOverlay() {}, theme: () => new Proxy({}, { get: () => "#000000" }), notify() {},
    t: (k: string) => k, open: async () => ({ id: ":/a", root: "/a", machine: null }), pushPreview: async () => preview, push: async () => ({}), cancel() {},
    gitSettings: () => ({}),
  };
  const body = code.replace("settings as gitSettings", "gitSettings");
  const GitPush = new Function(...Object.keys(stubs), `${body}\nreturn GitPush;`)(...Object.values(stubs));
  const cx: any = { spawn: (fn: (cx: any) => Promise<any>) => { fn(cx); }, notify() {}, focus_handle: () => ({ focus() {} }), stop_propagation() {} };
  const view = new GitPush();
  view.init(null, cx);
  await flush();
  view.render(cx);
  return texts;
}

test("one commit to push is counted in the singular, more in the plural, none as nothing", async () => {
  expect(await dialog(1)).toContain("commits_one");
  expect(await dialog(2)).toContain("commits_other");
  expect(await dialog(0)).toContain("nothing");
});
