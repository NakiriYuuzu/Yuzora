import { execFileSync } from "node:child_process"
import { mkdtempSync, readFileSync, rmSync } from "node:fs"
import { join } from "node:path"
import { tmpdir } from "node:os"

import { describe, expect, it } from "vitest"
import { releaseNotesForTag } from "./release-notes"

describe("releaseNotesForTag", () => {
  it("returns only the user-facing changelog section for the release tag", () => {
    const changelog = `# Changelog

## [0.0.3] - 2026-07-17

### 改善

- 更新後會清楚顯示這個版本帶來的改變。

## [0.0.2] - 2026-07-16

### 新增

- 可在設定中檢查更新。
`

    expect(releaseNotesForTag(changelog, "v0.0.3")).toBe(
      "### 改善\n\n- 更新後會清楚顯示這個版本帶來的改變。"
    )
  })

  it("extracts a beta changelog section", () => {
    const changelog = `# Changelog

## [0.0.9-beta.1] - 2026-08-17

### 已知限制

- 僅供手動下載。

## [0.0.8] - 2026-08-15
`

    expect(releaseNotesForTag(changelog, "v0.0.9-beta.1")).toBe(
      "### 已知限制\n\n- 僅供手動下載。"
    )
  })

  it("writes the actual changelog notes to the handoff file instead of verifier stdout", () => {
    const tag = "v0.0.9-beta.1"
    const directory = mkdtempSync(join(tmpdir(), "yuzora-release-notes-"))
    const outputPath = join(directory, "release-notes.md")
    try {
      const stdout = execFileSync("bun", ["scripts/release-notes.ts", tag, outputPath], {
        encoding: "utf8"
      })
      const handoff = readFileSync(outputPath, "utf8")
      const expected = `${releaseNotesForTag(readFileSync("CHANGELOG.md", "utf8"), tag)}\n`
      const payload = Buffer.from(handoff, "utf8").toString("base64")

      expect(stdout).toBe(`User-facing release notes verified for ${tag}\n`)
      expect(handoff).toBe(expected)
      expect(Buffer.from(payload, "base64").toString("utf8")).toBe(expected)
      expect(handoff).not.toBe(stdout)
    } finally {
      rmSync(directory, { recursive: true, force: true })
    }
  })

  it("blocks a release when its changelog section is missing", () => {
    expect(() => releaseNotesForTag("# Changelog\n", "v0.0.3")).toThrow(
      "CHANGELOG.md must include version 0.0.3"
    )
  })
})
