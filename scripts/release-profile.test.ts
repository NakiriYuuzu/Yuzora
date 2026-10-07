import { readFile } from "node:fs/promises"
import { describe, expect, it } from "vitest"

describe("release profiles", () => {
  it.each(["../src-tauri/Cargo.toml", "../src-tauri/host/Cargo.toml"])(
    "%s strips symbols with thin LTO without overriding unwinding or codegen units",
    async (path) => {
      const manifest = await readFile(new URL(path, import.meta.url), "utf8")
      const profile = manifest.split(/^\[profile\.release\]\s*$/m)[1]?.split(/^\[/m)[0]
      expect(profile).toBeDefined()
      const settings = profile!.split("\n").map((line) => line.split("#")[0].trim()).filter(Boolean)
      expect(settings).toEqual(['strip = "symbols"', 'lto = "thin"'])
    }
  )
})
