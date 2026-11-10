import { expect, test } from "bun:test";
import { readFile } from "node:fs/promises";

// The table and query panes watch a job until it stops. A job the core no
// longer has (a result superseded, a task that died) must show up as an error
// in the pane, not as "loading" for ever.
const load = async (mod: string, name: string, job: (id: number, seen: number) => Promise<unknown>) => {
  const source = await readFile(new URL(`../defaults/mods/${mod}/main.js`, import.meta.url), "utf8");
  const imported = [...source.matchAll(/^import \{([^}]*)\} from/gm)].flatMap((m) => m[1].split(",").map((s) => s.trim()));
  const values: Record<string, unknown> = Object.fromEntries(imported.map((n) => [n, () => undefined]));
  Object.assign(values, { View: class {}, job });
  const body = source.replace(/^import .*;\n/gm, "").replace(`export default class ${name}`, `return class ${name}`);
  return new Function(...Object.keys(values), body)(...Object.values(values)) as new () => any;
};

const cx = {
  notifications: 0,
  notify() {
    this.notifications++;
  },
  spawn(work: (cx: unknown) => Promise<void>) {
    return work(cx);
  },
};

for (const [mod, name] of [["yuzora-db-table", "DbTable"], ["yuzora-db-query", "DbQuery"]]) {
  test(`${mod} shows a job that is gone as an error instead of loading for ever`, async () => {
    const Pane = await load(mod, name, async () => {
      throw new Error("That query is gone");
    });
    const pane = new Pane();
    pane.watch(cx, 7);
    await Promise.resolve();
    await Promise.resolve();
    expect(pane.state).toMatchObject({ running: false, error: "That query is gone", results: [] });
  });

  test(`${mod} leaves a pane alone when the job it watched is not its newest`, async () => {
    const Pane = await load(mod, name, async () => {
      throw new Error("That query is gone");
    });
    const pane = new Pane();
    pane.watch(cx, 7);
    pane.jobId = 8;
    await Promise.resolve();
    await Promise.resolve();
    expect(pane.state).toBeNull();
  });
}
