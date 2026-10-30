import { configDefaults } from "vitest/config";
import { loadEnv } from "vite";
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
    const excluded = [...configDefaults.exclude, "**/.superpowers/**", "output/**", "spikes/companion-direct/**"];
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

describe("pdf.js decoder fallback assets", () => {
  it("keeps the file name pdf.js imports the JS fallback by", async () => {
    const config = await (typeof viteConfig === "function"
      ? viteConfig({ command: "build", mode: "production" })
      : viteConfig);
    const output = config.build?.rolldownOptions?.output;
    const assetFileNames = Array.isArray(output) ? undefined : output?.assetFileNames;
    if (typeof assetFileNames !== "function") throw new Error("assetFileNames is not a function");
    const asset = (name: string) => ({ type: "asset" as const, names: [name], originalFileNames: [], source: "" });
    expect(assetFileNames(asset("openjpeg_nowasm_fallback.js"))).toBe("assets/pdfjs/[name][extname]");
    expect(assetFileNames(asset("index.css"))).toBe("assets/[name]-[hash][extname]");
  });
});

describe("client env exposure", () => {
  it("never exposes the updater signing key to the client bundle", async () => {
    const config = await (typeof viteConfig === "function"
      ? viteConfig({ command: "build", mode: "production" })
      : viteConfig);
    const keys = ["TAURI_SIGNING_PRIVATE_KEY", "TAURI_SIGNING_PRIVATE_KEY_PASSWORD"];
    for (const key of keys) process.env[key] = "canary";
    try {
      const env = loadEnv("production", import.meta.dirname, config.envPrefix);
      for (const key of keys) expect(env).not.toHaveProperty(key);
    } finally {
      for (const key of keys) delete process.env[key];
    }
  });
});
