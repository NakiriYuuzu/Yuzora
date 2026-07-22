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

  it("blocks a release when its changelog section is missing", () => {
    expect(() => releaseNotesForTag("# Changelog\n", "v0.0.3")).toThrow(
      "CHANGELOG.md must include version 0.0.3"
    )
  })
})
