// What a person reads where the app names itself says Yuzora: the macOS app menu and the palette's Quit.
// Run: bun test scripts/yuzora-brand.test.ts
import { test, expect } from "bun:test";
import { readFile } from "node:fs/promises";

const read = (path: string) => readFile(new URL(`../${path}`, import.meta.url), "utf8");

test("the macOS app menu hides and quits Yuzora", async () => {
  const labels = [...(await read("src/macos.rs")).matchAll(/(?:Menu::new|MenuItem::action)\("([^"]+)"/g)].map((m) => m[1]);
  expect(labels).toEqual(["Yuzora", "Settings", "Hide Yuzora", "Hide Others", "Show All", "Quit Yuzora"]);
});

test("the palette's Quit names Yuzora, and typing herdr still finds it", async () => {
  const quit = /^\s*\["app\.quit", \{\}, "App", "([^"]+)", "([^"]+)"\]/m.exec(await read("defaults/frame/palette/main.js"));
  expect(quit?.[1]).toBe("Quit Yuzora");
  expect(quit?.[2].split(" ")).toContain("herdr");
});
