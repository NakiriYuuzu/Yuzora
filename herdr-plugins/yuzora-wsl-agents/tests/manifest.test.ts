import { readFileSync, readdirSync, statSync } from "node:fs"
import { dirname, join, relative } from "node:path"
import { fileURLToPath } from "node:url"
import { describe, expect, it } from "vitest"
import {
  FORBIDDEN_REPORT_TOKENS,
  MIN_HERDR_VERSION,
  PLUGIN_ID,
  PLUGIN_VERSION
} from "../lib/constants"

const root = join(dirname(fileURLToPath(import.meta.url)), "..")

function walk(dir: string): string[] {
  const out: string[] = []
  for (const entry of readdirSync(dir)) {
    if (entry === "node_modules") continue
    const path = join(dir, entry)
    if (statSync(path).isDirectory()) out.push(...walk(path))
    else out.push(path)
  }
  return out
}

describe("plugin manifest", () => {
  const toml = readFileSync(join(root, "herdr-plugin.toml"), "utf8")

  it("declares Windows-only Herdr v1 metadata", () => {
    expect(toml).toMatch(/^id = "yuzora-wsl-agents"$/m)
    expect(toml).toContain(`version = "${PLUGIN_VERSION}"`)
    expect(toml).toContain(`min_herdr_version = "${MIN_HERDR_VERSION}"`)
    expect(toml).toContain('platforms = ["windows"]')
    expect(toml).not.toContain("linux")
    expect(toml).not.toContain("macos")
    expect(toml).not.toContain("claude")
    expect(toml).not.toContain("codex")
    expect(toml).not.toContain("pane.exited")
  })

  it("uses PowerShell Bypass argv and fixed pane/action ids", () => {
    expect(toml).toContain("-NoProfile")
    expect(toml).toContain("-ExecutionPolicy")
    expect(toml).toContain("Bypass")
    expect(toml).toContain('id = "install-pi"')
    expect(toml).toContain('id = "status"')
    expect(toml).toContain('id = "open-wsl-shell"')
    expect(toml).toContain('id = "open-pi"')
    expect(toml).toContain('id = "uninstall-pi"')
    expect(toml).toContain('id = "wsl-shell"')
    expect(toml).toContain('id = "wsl-pi"')
    expect(toml).toContain('placement = "tab"')
    expect(PLUGIN_ID).toBe("yuzora-wsl-agents")
  })

  it("does not ship Claude/Codex adapters or session-report calls", () => {
    const files = walk(root)
    expect(files.some((file) => file.includes("/adapters/claude/"))).toBe(false)
    expect(files.some((file) => file.includes("/adapters/codex/"))).toBe(false)
    const runtime = files.filter((file) => {
      if (file.endsWith("herdr-wsl-report")) return false
      return (
        file.includes("/adapters/") ||
        file.includes("/scripts/") ||
        file.endsWith("herdr-plugin.toml")
      )
    })
    for (const file of runtime) {
      const text = readFileSync(file, "utf8")
      for (const token of FORBIDDEN_REPORT_TOKENS) {
        expect(text.toLowerCase(), relative(root, file)).not.toContain(token.toLowerCase())
      }
    }
  })
})
