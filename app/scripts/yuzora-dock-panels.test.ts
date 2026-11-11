// The right dock's Yuzora modes (files, git, database, ssh: stack children `yuzora-dock-*`) stay shown across
// Space switches; Files and Changes stay per Space. Two copies of syncPanel remember the panel per Space
// (upstream tabs and yuzora-shell-tabs) and each got one early return for the dock's children
// (`// yuzora(dock):`). This runs each file's own syncPanel, found by its text, so a sync that rewrites or
// moves the method, or a lost early return, fails here. Only this repo's shipped mod source is run.
import { expect, test } from "bun:test";
import { readFile } from "node:fs/promises";

const FILES = ["tabs", "yuzora-shell-tabs"];
const EARLY_RETURN = 'if (typeof now === "string" && now.startsWith("yuzora-dock-")) { this.space = space.id; this.pending = null; return; }';

async function method(mod: string): Promise<string[]> {
  const lines = (await readFile(new URL(`../defaults/mods/${mod}/main.js`, import.meta.url), "utf8")).split("\n");
  const start = lines.findIndex((l) => l === "  syncPanel(space, panel, body, enabled) {");
  const end = lines.findIndex((l, i) => i > start && l === "  }");
  expect(start, `${mod}: syncPanel not found`).toBeGreaterThanOrEqual(0);
  expect(end, `${mod}: end of syncPanel not found`).toBeGreaterThan(start);
  return lines.slice(start, end + 1);
}

async function harness(mod: string) {
  const body = (await method(mod)).join("\n");
  const calls: string[] = [];
  const store: Record<string, string> = {};
  const run = (name: string, args: unknown) => { calls.push(`${name} ${JSON.stringify(args)}`); };
  const localStorage = { setItem: (k: string, v: string) => { store[k] = v; }, getItem: (k: string) => store[k] ?? null };
  const Bar = new Function("run", "localStorage", `return class { constructor() { this.panels = {}; }\n${body}\n}`)(run, localStorage);
  const bar = new Bar();
  const enabled = ["files", "changes"];
  const shown = (active: string) => ({ panel: { hidden: false }, body: { active } });
  const closed = { panel: { hidden: true }, body: { active: "files" } };
  const step = (space: string, s: { panel: object; body: object }) => bar.syncPanel({ id: space }, s.panel, s.body, enabled);
  const take = () => calls.splice(0);
  return { bar, store, shown, closed, step, take };
}

for (const mod of FILES) {
  test(`${mod}: the dock's early return is there, worded the same in both files`, async () => {
    const text = (await method(mod)).join("\n");
    expect(text).toContain("// yuzora(dock):");
    expect(text).toContain(EARLY_RETURN);
    // right after `const now = …`
    const lines = await method(mod);
    const at = lines.findIndex((l) => l.trim().startsWith("const now = panel.hidden"));
    expect(lines[at + 2].trim()).toBe(EARLY_RETURN);
  });

  test(`${mod}: a dock mode shown in A stays when switching to B, and is not A's choice`, async () => {
    const h = await harness(mod);
    h.step("A", h.closed); h.step("A", h.shown("yuzora-dock-git")); h.take();
    h.step("B", h.shown("yuzora-dock-git"));
    expect(h.take()).toEqual([]);
    expect(JSON.parse(h.store.panels ?? "{}")).toEqual({ A: null });
    // a later switch is not "the first time" again
    h.step("A", h.shown("yuzora-dock-git"));
    expect(h.take()).toEqual([]);
  });

  test(`${mod}: a dock mode shown in B does not bring back A's Files, which A keeps`, async () => {
    const h = await harness(mod);
    h.step("A", h.shown("files")); h.step("B", h.shown("files")); h.take();
    h.step("B", h.shown("yuzora-dock-git")); h.step("A", h.shown("yuzora-dock-git"));
    expect(h.take()).toEqual([]);
    expect(JSON.parse(h.store.panels).A).toBe("files");
  });

  test(`${mod}: Files stays per Space`, async () => {
    const h = await harness(mod);
    h.step("A", h.closed); h.step("A", h.shown("files")); h.take();
    h.step("B", h.shown("files"));
    expect(h.take()).toEqual(['layout.hide {"id":"panel"}']);
    h.step("B", h.closed); h.step("A", h.closed);
    expect(h.take()).toEqual(['layout.select {"stack":"panel-body","child":"files"}', 'layout.show {"id":"panel"}']);
  });

  test(`${mod}: closing the dock in B gives A its own choice back`, async () => {
    const h = await harness(mod);
    h.step("A", h.shown("files")); h.step("A", h.shown("yuzora-dock-git")); h.step("B", h.shown("yuzora-dock-git")); h.step("B", h.closed); h.take();
    h.step("A", h.closed);
    expect(h.take()).toEqual(['layout.select {"stack":"panel-body","child":"files"}', 'layout.show {"id":"panel"}']);
  });

  test(`${mod}: another yuzora-* stack child is treated as upstream treats it`, async () => {
    const h = await harness(mod);
    h.step("A", h.closed); h.step("A", h.shown("yuzora-other")); h.take();
    h.step("B", h.shown("yuzora-other"));
    expect(h.take()).toEqual(['layout.hide {"id":"panel"}']);
  });
}
