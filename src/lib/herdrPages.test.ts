import { describe, expect, it } from "vitest"

import { herdrPagePath, isHerdrPagePath, parseHerdrPagePath } from "./herdrPages"

describe("herdrPages", () => {
  it("encodes and parses (sessionId, terminalId) identity", () => {
    const path = herdrPagePath("default", "term/1")
    expect(path.startsWith("yuzora://herdr/")).toBe(true)
    expect(parseHerdrPagePath(path)).toEqual({
      herdrSessionId: "default",
      terminalId: "term/1"
    })
    expect(isHerdrPagePath(path)).toBe(true)
  })

  it("keeps distinct sessions with the same terminal id separate", () => {
    const a = herdrPagePath("session-a", "t1")
    const b = herdrPagePath("session-b", "t1")
    expect(a).not.toBe(b)
    expect(parseHerdrPagePath(a)?.herdrSessionId).toBe("session-a")
    expect(parseHerdrPagePath(b)?.herdrSessionId).toBe("session-b")
  })

  it("rejects non-herdr paths", () => {
    expect(parseHerdrPagePath("/w/a.ts")).toBeNull()
    expect(parseHerdrPagePath("yuzora://preview")).toBeNull()
    expect(isHerdrPagePath("/w/a.ts")).toBe(false)
  })
})
