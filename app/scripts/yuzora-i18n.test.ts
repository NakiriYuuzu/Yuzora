// The interface is English only: no zh-TW strings ship, no language setting
// is offered, and every string a mod asks for with t("…") is in its en.json.
import { expect, test } from "bun:test";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";

const app = join(import.meta.dir, "..");
const SKIP = new Set(["vendor", "target", "node_modules", ".git", "dist"]);

function walk(dir: string, out: string[] = []): string[] {
  for (const name of readdirSync(dir)) {
    if (SKIP.has(name)) continue;
    const path = join(dir, name);
    if (statSync(path).isDirectory()) walk(path, out);
    else out.push(path);
  }
  return out;
}

const flat = (value: unknown, prefix = "", out = new Set<string>()): Set<string> => {
  if (value && typeof value === "object") for (const [k, v] of Object.entries(value)) flat(v, prefix ? `${prefix}.${k}` : k, out);
  else out.add(prefix);
  return out;
};

test("no zh-TW.json ships in defaults, src or any crate", () => {
  const found = ["defaults", "src", "crates"].flatMap((d) => walk(join(app, d))).filter((f) => f.endsWith("zh-TW.json"));
  expect(found).toEqual([]);
});

test("the settings offer no language", () => {
  const dir = join(app, "defaults/mods/yuzora-settings-general");
  const main = readFileSync(join(dir, "main.js"), "utf8");
  expect(main).not.toMatch(/setLocale|locale\(\)|zh-TW/);
  expect(Object.keys(JSON.parse(readFileSync(join(dir, "i18n/en.json"), "utf8")))).not.toContain("language");
});

test("every static t() key of a mod is in its en.json", () => {
  const mods = join(app, "defaults");
  const dirs = readdirSync(join(mods, "mods")).map((m) => join(mods, "mods", m)).concat(join(mods, "frame/settings"));
  let checked = 0;
  for (const dir of dirs) {
    let en: Set<string>;
    try {
      en = flat(JSON.parse(readFileSync(join(dir, "i18n/en.json"), "utf8")));
    } catch {
      continue; // a mod without strings
    }
    for (const file of walk(dir).filter((f) => f.endsWith(".js") && !f.includes("/i18n/"))) {
      const src = readFileSync(file, "utf8");
      for (const [, key] of src.matchAll(/(?<![\w.$])t\(\s*"([\w.-]+)"/g)) {
        expect(en.has(key), `${file}: t("${key}") is not in en.json`).toBe(true);
        checked++;
      }
    }
  }
  expect(checked).toBeGreaterThan(100);
});
