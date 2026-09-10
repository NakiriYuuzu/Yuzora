import { createHash } from "node:crypto"
import { mkdtempSync, mkdirSync, readFileSync, writeFileSync, existsSync, symlinkSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import { resolve } from "node:path"
import { spawnSync } from "node:child_process"
import { expect, it } from "vitest"

it.skipIf(process.platform === "win32")("cleanup previews, preserves foreign files/hooks, and can be repeated", () => {
  const root = mkdtempSync(resolve(tmpdir(), "yuzora-cleanup-test-"))
  try {
    const agent = resolve(root, "中文 agent")
    const extensions = resolve(agent, "extensions")
    mkdirSync(extensions, { recursive: true })
    const script = readFileSync("src-tauri/resources/legacy-cleanup/cleanup-wsl-adapter.sh", "utf8")
    const fixture = "// disposable adapter fixture\n"
    const hash = createHash("sha256").update(fixture).digest("hex")
    // Exercise the shipped logic with a harmless fixture instead of carrying
    // the removed executable adapter in the active source tree.
    const runner = resolve(root, "cleanup.sh")
    writeFileSync(runner, script.replaceAll(/\b[a-f0-9]{64}\b/g, hash))
    const adapter = resolve(extensions, "yuzora-herdr-wsl.ts")
    const report = resolve(extensions, "yuzora-herdr-wsl-report")
    const official = resolve(extensions, "herdr-agent-state.ts")
    writeFileSync(adapter, fixture)
    writeFileSync(report, "user-modified reporter")
    writeFileSync(official, "user official integration")
    const run = (apply: boolean) => spawnSync("sh", [runner, ...(apply ? ["--apply"] : []), agent], { encoding: "utf8" })
    expect(run(false).status).toBe(0)
    expect(existsSync(adapter)).toBe(true)
    expect(run(true).status).toBe(0)
    expect(existsSync(adapter)).toBe(false)
    expect(readFileSync(report, "utf8")).toBe("user-modified reporter")
    expect(readFileSync(official, "utf8")).toBe("user official integration")
    symlinkSync(official, adapter)
    expect(run(true).stdout).toContain("preserved (modified or not owned)")
    expect(readFileSync(official, "utf8")).toBe("user official integration")
    expect(run(true).status).toBe(0)
  } finally { rmSync(root, { recursive: true, force: true }) }
})
