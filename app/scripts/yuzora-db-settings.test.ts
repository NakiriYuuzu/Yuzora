// The Database settings page's main.js as shipped, with yuzora:db and the element builders stubbed.
// Run: bun test scripts/yuzora-db-settings.test.ts
import { test, expect } from "bun:test";
import { readFile } from "node:fs/promises";

const main = await readFile(new URL("../defaults/mods/yuzora-db-settings/main.js", import.meta.url), "utf8");
const code = main.replace(/^import [\s\S]*?;\n/gm, "").replace(/^export default class/m, "class");

function page(start = 100_000, refuse: string | null = null) {
  const w = { stored: start, sets: [] as number[], field: "", texts: [] as string[], clicks: {} as Record<string, (e: unknown, cx: unknown) => void>, on: {} as Record<string, (e: unknown, cx: unknown) => void> };
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
    View: class {}, div: element, v_flex: element, h_flex: element, Input: { new: element },
    InputState: { new: (o: { value: string }) => { w.field = o.value; return { on: (name: string, fn: any) => { w.on[name] = fn; }, value: () => w.field, set_value: (v: string) => { w.field = v; } }; } },
    theme: () => new Proxy({}, { get: () => "#000000" }),
    t: (key: string, values?: object) => (values ? `${key} ${JSON.stringify(values)}` : key),
    rowLimit: () => ({ value: w.stored, min: 100, max: 5_000_000, default: 100_000 }),
    setRowLimit: (rows: number) => {
      if (refuse) throw new Error(refuse);
      w.sets.push(rows);
      w.stored = Math.min(5_000_000, Math.max(100, rows));
      return w.stored;
    },
  };
  const DbSettings = new Function(...Object.keys(stubs), `${code}\nreturn DbSettings;`)(...Object.values(stubs));
  const cx = { notify() {}, stop_propagation() {} };
  const view = new DbSettings();
  view.init(null, cx);
  const draw = () => { w.texts.length = 0; w.clicks = {}; view.render(cx); return [...w.texts]; };
  const type = (text: string, how: "submit" | "blur" = "submit") => { w.field = text; w.on[how]({}, cx); };
  return { w, draw, type, cx };
}

test("the row limit typed is stored on Enter or on leaving the field, and the field shows what was stored", () => {
  const p = page();
  expect(p.w.field).toBe("100000");
  p.type("250000");
  expect(p.w.sets).toEqual([250000]);
  expect(p.w.field).toBe("250000");
  p.type(" 5 ", "blur");
  expect(p.w.sets).toEqual([250000, 5]);
  expect(p.w.field).toBe("100");
});

test("what is not a number, or is the limit there already, stores nothing and the field goes back", () => {
  const p = page(250_000);
  for (const text of ["", "many", "250000", "250000.2"]) {
    p.type(text);
    expect(p.w.field).toBe("250000");
  }
  expect(p.w.sets).toEqual([]);
});

test("Reset is there only for a limit that was changed, and puts the default back", () => {
  const p = page();
  p.draw();
  expect(p.w.clicks["reset-maxRows"]).toBeUndefined();
  p.type("300");
  p.draw();
  p.w.clicks["reset-maxRows"]({}, p.cx);
  expect(p.w.sets).toEqual([300, 100000]);
  expect(p.w.field).toBe("100000");
  p.draw();
  expect(p.w.clicks["reset-maxRows"]).toBeUndefined();
});

test("a limit that could not be saved says why and keeps the one there was", () => {
  const p = page(100_000, "the disk is read-only");
  p.type("300");
  expect(p.w.field).toBe("100000");
  expect(p.draw()).toContain('error {"error":"the disk is read-only"}');
});
