import { execFileSync } from "node:child_process"
import { mkdtempSync, readFileSync, rmSync } from "node:fs"
import { join } from "node:path"
import { tmpdir } from "node:os"

import { describe, expect, it } from "vitest"
import { parseReleaseNoteLines } from "../src/lib/releaseNotes"
import { CHANGELOGS, releaseNotesForTag, type Changelogs } from "./release-notes"

function repositoryChangelogs(): Changelogs {
  return {
    "CHANGELOG.md": readFileSync("CHANGELOG.md", "utf8"),
    "CHANGELOG.zh-TW.md": readFileSync("CHANGELOG.zh-TW.md", "utf8"),
  }
}

// Each version's heading (未發布 and Unreleased are the same), with the item and nested-item counts of
// each section; the first entry counts the items before the version's first "### " heading.
function outline(changelog: string) {
  const versions: { heading: string; sections: { items: number; nested: number }[] }[] = []
  for (const line of changelog.split(/\r?\n/)) {
    if (line.startsWith("## [")) {
      versions.push({ heading: line.replace(/ - 未發布$/, " - Unreleased"), sections: [{ items: 0, nested: 0 }] })
    } else if (versions.length > 0) {
      const sections = versions[versions.length - 1].sections
      if (line.startsWith("### ")) sections.push({ items: 0, nested: 0 })
      else if (line.startsWith("- ")) sections[sections.length - 1].items += 1
      else if (/^\s+- /.test(line)) sections[sections.length - 1].nested += 1
    }
  }
  return versions
}

describe("releaseNotesForTag", () => {
  it("discloses the v0.0.9 Apple signing limitation and retained updater verification", () => {
    const notes = releaseNotesForTag(repositoryChangelogs(), "v0.0.9")
    expect(notes).toContain("未經 Apple Developer ID 簽章或公證")
    expect(notes).toContain("Tauri updater 簽章")
    const english = notes.slice(0, notes.indexOf("### 繁體中文"))
    expect(english).toContain("Apple Developer ID")
    expect(english).toContain("Gatekeeper")
  })

  it("tells beta.2's readers in both languages about Gatekeeper and SmartScreen, and to install it by hand", () => {
    const notes = releaseNotesForTag(repositoryChangelogs(), "v0.1.0-beta.2")
    const split = notes.indexOf("### 繁體中文")
    const english = notes.slice(0, split)
    const chinese = notes.slice(split)
    for (const part of [english, chinese]) {
      expect(part).toContain("Gatekeeper")
      expect(part).toContain("SmartScreen")
    }
    expect(english).toContain("download and install beta.2 yourself")
    expect(chinese).toContain("請自行下載安裝 beta.2")
  })

  it("returns the release tag's section from both changelogs, English first", () => {
    const changelogs = {
      "CHANGELOG.md": `# Changelog

## [0.0.3] - 2026-07-17

### Improved

- After an update, the app shows what this version changed.

## [0.0.2] - 2026-07-16

### Added

- Check for updates from Settings.
`,
      "CHANGELOG.zh-TW.md": `# Changelog

## [0.0.3] - 2026-07-17

### 改善

- 更新後會清楚顯示這個版本帶來的改變。

## [0.0.2] - 2026-07-16

### 新增

- 可在設定中檢查更新。
`,
    }

    expect(releaseNotesForTag(changelogs, "v0.0.3")).toBe(
      "### English\n\n### Improved\n\n- After an update, the app shows what this version changed.\n\n" +
        "### 繁體中文\n\n### 改善\n\n- 更新後會清楚顯示這個版本帶來的改變。"
    )
  })

  it("extracts a beta changelog section", () => {
    const changelogs = {
      "CHANGELOG.md": `# Changelog

## [0.0.9-beta.1] - 2026-08-17

### Known limitations

- Manual download only.

## [0.0.8] - 2026-08-15
`,
      "CHANGELOG.zh-TW.md": `# Changelog

## [0.0.9-beta.1] - 2026-08-17

### 已知限制

- 僅供手動下載。

## [0.0.8] - 2026-08-15
`,
    }

    expect(releaseNotesForTag(changelogs, "v0.0.9-beta.1")).toBe(
      "### English\n\n### Known limitations\n\n- Manual download only.\n\n### 繁體中文\n\n### 已知限制\n\n- 僅供手動下載。"
    )
  })

  it("titles each language with a heading the installed apps' update notes render", () => {
    const lines = parseReleaseNoteLines(releaseNotesForTag(repositoryChangelogs(), "v0.0.9"))
    const headings = lines.filter((line) => line.kind === "heading").map((line) => line.text)
    expect(headings[0]).toBe("English")
    expect(headings).toContain("繁體中文")
    expect(lines.filter((line) => line.kind === "text" && line.text.startsWith("#"))).toEqual([])
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
      const expected = `${releaseNotesForTag(repositoryChangelogs(), tag)}\n`
      const payload = Buffer.from(handoff, "utf8").toString("base64")

      expect(stdout).toBe(`User-facing release notes verified for ${tag}\n`)
      expect(handoff).toBe(expected)
      expect(Buffer.from(payload, "base64").toString("utf8")).toBe(expected)
      expect(handoff).not.toBe(stdout)
    } finally {
      rmSync(directory, { recursive: true, force: true })
    }
  })

  it("blocks a release when either changelog is missing its section", () => {
    const section = "## [0.0.3] - 2026-07-17\n\n### Fixed\n\n- A fix.\n"
    expect(() =>
      releaseNotesForTag({ "CHANGELOG.md": "# Changelog\n", "CHANGELOG.zh-TW.md": section }, "v0.0.3")
    ).toThrow("CHANGELOG.md must include version 0.0.3")
    expect(() =>
      releaseNotesForTag({ "CHANGELOG.md": section, "CHANGELOG.zh-TW.md": "# Changelog\n" }, "v0.0.3")
    ).toThrow("CHANGELOG.zh-TW.md must include version 0.0.3")
  })
})

describe("the English and Traditional Chinese changelogs", () => {
  it("list the same versions with the same items in each section", () => {
    const changelogs = repositoryChangelogs()
    const english = outline(changelogs["CHANGELOG.md"])
    const chinese = outline(changelogs["CHANGELOG.zh-TW.md"])
    expect(english.map((v) => v.heading)).toEqual(chinese.map((v) => v.heading))
    expect(english).toEqual(chinese)
    expect(english.length).toBeGreaterThan(0)
  })

  it("link to each other at the top", () => {
    const changelogs = repositoryChangelogs()
    for (const { path } of CHANGELOGS) {
      const top = changelogs[path].slice(0, changelogs[path].indexOf("## ["))
      for (const other of CHANGELOGS.filter((c) => c.path !== path)) {
        expect(top).toContain(`href="${other.path}"`)
      }
    }
  })
})
