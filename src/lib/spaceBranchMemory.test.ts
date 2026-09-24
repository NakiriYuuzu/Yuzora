import { beforeEach, expect, it, vi } from "vitest"
import {
  SPACE_BRANCH_MEMORY_KEY,
  SPACE_BRANCH_MEMORY_LIMIT,
  rememberSpaceBranch,
  rememberedSpaceBranch,
  resetSpaceBranchMemoryCache,
} from "./spaceBranchMemory"

let values: Map<string, string>
beforeEach(() => {
  values = new Map()
  vi.stubGlobal("localStorage", {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => { values.set(key, value) },
  })
  resetSpaceBranchMemoryCache()
})

it("remembers the latest branch per Space and survives a reload", () => {
  rememberSpaceBranch("a", "w1")
  rememberSpaceBranch("b", "w9")
  rememberSpaceBranch("a", "w2")
  resetSpaceBranchMemoryCache()
  expect(rememberedSpaceBranch("a")).toBe("w2")
  expect(rememberedSpaceBranch("b")).toBe("w9")
  expect(rememberedSpaceBranch("missing")).toBeNull()
  expect(JSON.parse(values.get(SPACE_BRANCH_MEMORY_KEY)!)).toEqual([["b", "w9"], ["a", "w2"]])
})

it("keeps only the most recently used entries", () => {
  for (let i = 0; i <= SPACE_BRANCH_MEMORY_LIMIT; i++) rememberSpaceBranch(`space-${i}`, `w${i}`)
  expect(rememberedSpaceBranch("space-0")).toBeNull()
  expect(rememberedSpaceBranch(`space-${SPACE_BRANCH_MEMORY_LIMIT}`)).toBe(`w${SPACE_BRANCH_MEMORY_LIMIT}`)
})

it("ignores corrupt storage and keeps working when storage throws", () => {
  values.set(SPACE_BRANCH_MEMORY_KEY, "{not json")
  expect(rememberedSpaceBranch("a")).toBeNull()
  resetSpaceBranchMemoryCache()
  values.set(SPACE_BRANCH_MEMORY_KEY, JSON.stringify([["a", 1], ["b", "w2"], "x"]))
  expect(rememberedSpaceBranch("a")).toBeNull()
  expect(rememberedSpaceBranch("b")).toBe("w2")
  vi.stubGlobal("localStorage", {
    getItem: () => { throw new Error("denied") },
    setItem: () => { throw new Error("denied") },
  })
  resetSpaceBranchMemoryCache()
  rememberSpaceBranch("c", "w3")
  expect(rememberedSpaceBranch("c")).toBe("w3")
})
