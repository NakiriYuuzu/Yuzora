import { describe, expect, it } from "vitest"
import { herdrWorkspaceInsertIndex } from "./herdrWorkspaceReorder"

describe("herdrWorkspaceInsertIndex", () => {
  it("maps before and after drops to HERDR removal-first indices", () => {
    expect(herdrWorkspaceInsertIndex(0, 2, false, 4)).toBe(1)
    expect(herdrWorkspaceInsertIndex(0, 2, true, 4)).toBe(2)
    expect(herdrWorkspaceInsertIndex(3, 1, false, 4)).toBe(1)
    expect(herdrWorkspaceInsertIndex(3, 1, true, 4)).toBe(2)
    expect(herdrWorkspaceInsertIndex(0, 1, false, 4)).toBeNull()
  })

  it("rejects invalid or self drops", () => {
    expect(herdrWorkspaceInsertIndex(-1, 0, false, 2)).toBeNull()
    expect(herdrWorkspaceInsertIndex(0, 2, false, 2)).toBeNull()
    expect(herdrWorkspaceInsertIndex(1, 1, false, 2)).toBeNull()
    expect(herdrWorkspaceInsertIndex(1, 0, false, 2)).toBe(0)
  })
})
