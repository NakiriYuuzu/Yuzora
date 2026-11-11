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
  return Object.assign(new Function(...Object.keys(values), body)(...Object.values(values)) as new () => any, { imported });
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

// The table pane's Structure tab shows what the open itself read of the table,
// beside the rows. Asking for it again with `describe` would put a second copy
// of the same queries on the sidebar's connection, in front of the rows' wait.
const target = { connection: "c", database: null, schema: "public", name: "t" };
const idle = async (id: number) => ({ id, running: false, version: 1, elapsedMs: 0, error: null, cancelled: false, results: [] });
const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

const table = async (overrides: Record<string, unknown>) => {
  const Pane = await load("yuzora-db-table", "DbTable", idle, { setEditorText: () => undefined, ...overrides });
  const pane = new Pane();
  pane.key = "k";
  pane.target = target;
  return { Pane, pane };
};

test("yuzora-db-table does not import describe: the open's own description is the Structure tab's", async () => {
  const { Pane } = await table({});
  expect(Pane.imported).not.toContain("describe");
  expect(Pane.imported).toContain("tableInfo");
});

test("yuzora-db-table asks the job it started for the description", async () => {
  const asked: number[] = [];
  const { pane } = await table({
    openTable: () => 42,
    tableInfo: async (id: number) => {
      asked.push(id);
      return { ddl: "CREATE TABLE t ()", columns: [] };
    },
  });
  pane.load(cx);
  await settle();
  expect(asked).toEqual([42]);
  expect(pane.info).toMatchObject({ ddl: "CREATE TABLE t ()" });
  expect(pane.infoError).toBeNull();
});

test("yuzora-db-table keeps the description of the newest open when an older one answers late", async () => {
  const waiting = new Map<number, (info: unknown) => void>();
  let next = 1;
  const { pane } = await table({
    openTable: () => next++,
    tableInfo: (id: number) => new Promise((resolve) => waiting.set(id, resolve)),
  });
  pane.load(cx);
  pane.load(cx);
  waiting.get(2)!({ ddl: "second", columns: [] });
  await settle();
  waiting.get(1)!({ ddl: "first", columns: [] });
  await settle();
  expect(pane.info).toMatchObject({ ddl: "second" });
});

test("yuzora-db-table keeps the description when a query runs before it arrives", async () => {
  let arrive: (info: unknown) => void = () => undefined;
  const { pane } = await table({ openTable: () => 5, tableInfo: () => new Promise((resolve) => (arrive = resolve)) });
  pane.load(cx);
  pane.jobId = 99;
  arrive({ ddl: "late", columns: [] });
  await settle();
  expect(pane.info).toMatchObject({ ddl: "late" });
});

test("yuzora-db-table shows why the description could not be read", async () => {
  const { pane } = await table({
    openTable: () => 5,
    tableInfo: async () => {
      throw new Error("There is no table or view public.t");
    },
  });
  pane.load(cx);
  await settle();
  expect(pane.info).toBeNull();
  expect(pane.infoError).toBe("There is no table or view public.t");
});

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

test("yuzora-db-table's toolbar has Stop for whatever is going, and Run where Run goes ahead of it", async () => {
  const offers = async (jobKind: "table" | "sql", tab: string, running = true) => {
    const { pane } = await busyPane(jobKind, 0);
    pane.tab = tab;
    pane.state = { ...pane.state, running };
    return [pane.showsStop(), pane.showsRun()];
  };
  expect(await offers("table", "sql")).toEqual([true, true]);
  expect(await offers("table", "data")).toEqual([true, false]);
  expect(await offers("table", "structure")).toEqual([true, false]);
  expect(await offers("sql", "sql")).toEqual([true, false]);
  expect(await offers("sql", "data")).toEqual([true, false]);
  expect(await offers("table", "sql", false)).toEqual([false, true]);
});

test("yuzora-db-table's Run with nothing typed leaves the read of the table going", async () => {
  const { pane, calls } = await busyPane("table");
  const empty = await load("yuzora-db-table", "DbTable", never, {
    t: (key: string) => key,
    editorText: () => ({ text: "  \n", selection: "" }),
    query: (...args: unknown[]) => calls.query.push(args),
    cancel: (id: number) => calls.cancel.push(id),
    result: () => ({ edits: 0 }),
  });
  const quiet = new empty();
  Object.assign(quiet, { key: "k", target: pane.target, tab: "sql", jobId: 41, jobKind: "table", state: pane.state, shown: 0 });
  quiet.run(cx);
  expect(calls.query).toEqual([]);
  expect(calls.cancel).toEqual([]);
  expect(quiet).toMatchObject({ jobId: 41, jobKind: "table", tab: "sql" });
});
