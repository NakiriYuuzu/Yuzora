import { configDefaults } from "vitest/config";
import { describe, expect, it } from "vitest";
import viteConfig from "./vite.config";
import { readFileSync } from "node:fs";
import path from "node:path";

const nodeTests: string[] = JSON.parse(
  readFileSync(path.resolve(import.meta.dirname, "src/test/nodeTests.json"), "utf8"),
);

describe("Vitest environment projects", () => {
  it("keeps unlisted tests in jsdom with DOM setup", () => {
    expect(nodeTests).not.toContain("vite.config.test.ts");
    expect(globalThis.document.createElement("div")).toBeInstanceOf(globalThis.HTMLElement);
    expect(globalThis.window.matchMedia("(prefers-color-scheme: dark)").matches).toBe(false);
  });

  it("uses an explicit, duplicate-free node allowlist", () => {
    expect(new Set(nodeTests).size).toBe(nodeTests.length);
    expect(nodeTests.length).toBeGreaterThan(0);
    expect(nodeTests.every((file) => !/[*?{}]/.test(file))).toBe(true);
  });

  it("inherits globals and exclusions while assigning each file to one project", async () => {
    const config = await (typeof viteConfig === "function"
      ? viteConfig({ command: "serve", mode: "test" })
      : viteConfig);
    const excluded = [...configDefaults.exclude, "**/.superpowers/**", "output/**"];
    expect(config.test?.globals).toBe(true);
    expect(config.test?.exclude).toEqual(excluded);
    expect(config.test?.projects).toEqual([
      {
        extends: true,
        test: { name: "node", environment: "node", include: nodeTests },
      },
      {
        extends: true,
        test: {
          name: "jsdom",
          environment: "jsdom",
          setupFiles: "./src/test/setup.ts",
          exclude: [...excluded, ...nodeTests],
        },
      },
    ]);
  });
});
