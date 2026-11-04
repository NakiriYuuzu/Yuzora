// The interface is English only: no zh-TW strings ship, no language setting
// is offered, and every string a mod asks for with t("…") is in its en.json.
import { expect, test } from "bun:test";
import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";

const app = join(import.meta.dir, "..");
const SKIP = new Set(["vendor", "target", "node_modules", ".git", "dist"]);

function walk(dir: string, out: string[] = []): string[] {
  for (const name of readdirSync(dir)) {
    if (SKIP.has(name) || name.startsWith(".")) continue; // dotfiles (.DS_Store…) are not shipped
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

test("an i18n/ folder holds en.json and nothing else", () => {
  const found = ["defaults", "src", "crates"]
    .flatMap((d) => walk(join(app, d)))
    .filter((f) => /\/i18n\/[^/]+$/.test(f) && !f.endsWith("/i18n/en.json"));
  expect(found).toEqual([]);
  expect(["defaults", "src", "crates"].flatMap((d) => walk(join(app, d))).filter((f) => f.endsWith("zh-TW.json"))).toEqual([]);
});

test("no mod or frame offers or reads a language", () => {
  const mods = join(app, "defaults/mods");
  expect(readdirSync(mods).filter((m) => m.startsWith("yuzora-")).length).toBeGreaterThan(30);
  const files = [...walk(mods), ...walk(join(app, "defaults/frame"))].filter((f) => f.endsWith(".js"));
  expect(files.length).toBeGreaterThan(50);
  for (const file of files) {
    expect(readFileSync(file, "utf8"), file).not.toMatch(/setLocale|\blocale\(\)|zh-TW/);
  }
  const en = JSON.parse(readFileSync(join(mods, "yuzora-settings-general/i18n/en.json"), "utf8"));
  expect(Object.keys(en)).not.toContain("language");
});

// Only literal keys, t("a.b"): a key built at run time (t(`x.${y}`)) is not checked.
test("every static t() key of a mod is in its en.json", () => {
  const mods = join(app, "defaults");
  const dirs = readdirSync(join(mods, "mods")).map((m) => join(mods, "mods", m)).concat(join(mods, "frame/settings"));
  let checked = 0;
  for (const dir of dirs) {
    const strings = join(dir, "i18n/en.json");
    if (!existsSync(strings)) continue; // a mod without strings
    const en = flat(JSON.parse(readFileSync(strings, "utf8"))); // a broken en.json fails the test
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
