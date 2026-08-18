import { describe, expect, it } from "vitest"

import { herdrPagePath, isHerdrPagePath, parseHerdrPagePath } from "./herdrPages"

const ubuntu = { kind: "wsl" as const, distro: "Ubuntu" }

describe("herdrPages", () => {
  it("encodes and parses native (sessionId, terminalId) identity", () => {
    const path = herdrPagePath("default", "term/1")
    expect(path).toBe("yuzora://herdr/default/term%2F1")
    expect(parseHerdrPagePath(path)).toEqual({
      herdrSessionId: "default",
      terminalId: "term/1",
      runtimeTarget: { kind: "native" },
      legacy: true
    })
    expect(isHerdrPagePath(path)).toBe(true)
  })

  it("keeps Native/default and WSL Ubuntu/default terminal ids separate", () => {
    const native = herdrPagePath("default", "t1")
    const wsl = herdrPagePath("default", "t1", ubuntu)
    expect(native).not.toBe(wsl)
    expect(parseHerdrPagePath(native)?.runtimeTarget).toEqual({ kind: "native" })
    expect(parseHerdrPagePath(wsl)?.runtimeTarget).toEqual(ubuntu)
  })

  it("decodes a v1 persisted page as Native without rewriting its path", () => {
    expect(parseHerdrPagePath("yuzora://herdr/default/t1")).toEqual({
      herdrSessionId: "default",
      terminalId: "t1",
      runtimeTarget: { kind: "native" },
      legacy: true
    })
  })

  it("rejects non-herdr paths", () => {
    expect(parseHerdrPagePath("/w/a.ts")).toBeNull()
    expect(parseHerdrPagePath("yuzora://preview")).toBeNull()
    expect(isHerdrPagePath("/w/a.ts")).toBe(false)
  })
})
