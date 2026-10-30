import { readFile } from "node:fs/promises"
import { describe, expect, it } from "vitest"

describe("release profiles", () => {
  it("strips symbols with thin LTO in the workspace manifest without overriding unwinding or codegen units", async () => {
    const manifest = await readFile("Cargo.toml", "utf8")
    const profile = manifest.split(/^\[profile\.release\]\s*$/m)[1]?.split(/^\[/m)[0]
    expect(profile).toBeDefined()
    const settings = profile!.split("\n").map((line) => line.split("#")[0].trim()).filter(Boolean)
    expect(settings).toEqual(['strip = "symbols"', 'lto = "thin"'])
  })

  // Cargo ignores profiles outside the workspace root manifest.
  it.each(["src-tauri/Cargo.toml", "src-tauri/host/Cargo.toml", "src-tauri/companion/Cargo.toml"])(
    "%s leaves profiles to the workspace manifest",
    async (path) => {
      const manifest = await readFile(path, "utf8")
      expect(manifest).not.toMatch(/^\[profile\./m)
    }
  )
})
