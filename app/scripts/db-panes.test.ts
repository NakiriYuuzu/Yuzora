import { expect, test } from "bun:test";
import { readFile } from "node:fs/promises";

// The table and query panes watch a job until it stops. A job the core no
// longer has (a result superseded, a task that died) must show up as an error
// in the pane, not as "loading" for ever.
const load = async (
  mod: string,
  name: string,
  job: (id: number, seen: number) => Promise<unknown>,
  extra: Record<string, unknown> = {},
) => {
  const source = await readFile(new URL(`../defaults/mods/${mod}/main.js`, import.meta.url), "utf8");
  const imported = [...source.matchAll(/^import \{([^}]*)\} from/gm)].flatMap((m) => m[1].split(",").map((s) => s.trim()));
  const values: Record<string, unknown> = Object.fromEntries(imported.map((n) => [n, () => undefined]));
  Object.assign(values, { View: class {}, job }, extra);
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

// Run in the SQL tab while the pane's own job goes on. A read of the table can be stuck behind a transaction this
// very SQL would end, so Run goes ahead of it; a statement of the person's own is theirs to Stop.
const never = () => new Promise<never>(() => {});

const busyPane = async (jobKind: "table" | "sql", edits = 0) => {
  const calls = { query: [] as unknown[][], openTable: 0, cancel: [] as number[] };
  const Pane = await load("yuzora-db-table", "DbTable", never, {
    t: (key: string) => key,
    editorText: () => ({ text: "COMMIT", selection: "" }),
    query: (...args: unknown[]) => {
      calls.query.push(args);
      return 99;
    },
    openTable: () => {
      calls.openTable++;
      return 100;
    },
    cancel: (id: number) => calls.cancel.push(id),
    result: () => ({ edits }),
  });
  const pane = new Pane();
  pane.key = "k";
  pane.target = { connection: "c", database: null, schema: "public", name: "first", kind: "table" };
  pane.tab = "sql";
  pane.shown = 0;
  pane.jobId = 41;
  pane.jobKind = jobKind;
  pane.state = { id: 41, running: true, version: 1, elapsedMs: 0, error: null, cancelled: false, results: [{ id: 5 }] };
  return { pane, calls };
};

test("yuzora-db-table runs the SQL over a read of the table that is still going", async () => {
  const { pane, calls } = await busyPane("table");
  pane.runSql(cx);
  expect(calls.query).toEqual([["c", null, "k", "COMMIT"]]);
  expect(pane).toMatchObject({ jobId: 99, jobKind: "sql", tab: "data" });
  expect(calls.cancel).toEqual([]);
});

test("yuzora-db-table does not run a second statement over one of the person's own", async () => {
  const { pane, calls } = await busyPane("sql");
  pane.runSql(cx);
  pane.run(cx);
  expect(calls.query).toEqual([]);
  expect(pane).toMatchObject({ jobId: 41, jobKind: "sql", tab: "sql" });
  expect(pane.confirm ?? null).toBeNull();
});

test("yuzora-db-table asks about unwritten edits before the SQL goes ahead of a read", async () => {
  const { pane, calls } = await busyPane("table", 2);
  pane.runSql(cx);
  expect(calls.query).toEqual([]);
  expect(pane.jobId).toBe(41);
  expect(pane.confirm).toMatchObject({ count: 2 });
  pane.confirm.action(cx);
  expect(calls.query).toEqual([["c", null, "k", "COMMIT"]]);
});

test("yuzora-db-table leaves the table's Refresh alone while the table is being read", async () => {
  const { pane, calls } = await busyPane("table");
  pane.tab = "data";
  pane.run(cx);
  expect(calls.openTable).toBe(0);
  expect(pane.jobId).toBe(41);
});

test("yuzora-db-table's SQL tab says what Run does, or why it waits", async () => {
  const read = await busyPane("table");
  expect(read.pane.sqlHint()).toBe("sql_over_read");
  const own = await busyPane("sql");
  expect(own.pane.sqlHint()).toBe("sql_busy");
  const idle = await busyPane("table");
  idle.pane.state = { ...idle.pane.state, running: false };
  expect(idle.pane.sqlHint()).toBe("sql_hint");
});
