// What a person reads where the app names itself says Yuzora: the macOS app menu and the palette's Quit.
// Run: bun test scripts/yuzora-brand.test.ts
import { test, expect } from "bun:test";
import { readFile } from "node:fs/promises";

const read = (path: string) => readFile(new URL(`../${path}`, import.meta.url), "utf8");

test("the macOS menu bar names Yuzora, and no item says Herdr", async () => {
  const source = await read("src/macos.rs");
  const labels = [...source.matchAll(/(?:Menu::new|command|MenuItem::os_action|MenuItem::os_submenu)\("([^"]+)"/g)].map((m) => m[1]);
  expect(/Menu::new\("([^"]+)"/.exec(source)?.[1]).toBe("Yuzora");
  expect(labels).toEqual(expect.arrayContaining(["Hide Yuzora", "Quit Yuzora", "Welcome to Yuzora", "Settings…", "Window"]));
  expect(labels.filter((label) => /herdr/i.test(label))).toEqual([]);
});

test("the palette's Quit names Yuzora, and typing herdr still finds it", async () => {
  const quit = /^\s*\["app\.quit", \{\}, "App", "([^"]+)", "([^"]+)"\]/m.exec(await read("defaults/frame/palette/main.js"));
  expect(quit?.[1]).toBe("Quit Yuzora");
  expect(quit?.[2].split(" ")).toContain("herdr");
});
