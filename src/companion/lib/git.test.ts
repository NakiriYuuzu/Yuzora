import { describe, expect, it } from "vitest"
import { formatWhen, spaceKey, splitPath, visibleRefs } from "./git"

describe("git helpers", () => {
  it("keys a Space by session and workspace without letting names run together", () => {
    expect(spaceKey("a", "b c")).not.toBe(spaceKey("a b", "c"))
  })

  it("splits a path into its folder and file name", () => {
    expect(splitPath("src/lib/a.ts")).toEqual({ dir: "src/lib/", name: "a.ts" })
    expect(splitPath("README.md")).toEqual({ dir: "", name: "README.md" })
  })

  it("formats git's Unix seconds in the phone's language", () => {
    expect(formatWhen(1_760_000_000, "en")).toMatch(/2025/)
    expect(formatWhen(1_760_000_000, "zh-TW", true)).toMatch(/2025/)
  })

  it("folds HEAD into the local branch it points at and shows it alone when detached", () => {
    const head = { name: "HEAD", kind: "head" as const }
    const main = { name: "main", kind: "local" as const }
    const tag = { name: "v1", kind: "tag" as const }
    expect(visibleRefs([head, main, tag])).toEqual([main, tag])
    expect(visibleRefs([head, tag])).toEqual([head, tag])
  })
})
