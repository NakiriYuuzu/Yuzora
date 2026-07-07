import { describe, expect, it } from "vitest"

import { firstAbsolutePath, isAbsolutePath } from "./paths"

describe("isAbsolutePath", () => {
  it("accepts posix absolute paths", () => {
    expect(isAbsolutePath("/workspace")).toBe(true)
    expect(isAbsolutePath("/")).toBe(true)
  })

  it("accepts Windows drive-letter paths with either slash", () => {
    expect(isAbsolutePath("C:\\Users\\me")).toBe(true)
    expect(isAbsolutePath("d:/projects")).toBe(true)
  })

  it("accepts Windows verbatim and UNC prefixes (canonicalize output)", () => {
    // std::fs::canonicalize returns the \\?\ extended-length form on Windows.
    expect(isAbsolutePath("\\\\?\\C:\\Users\\me\\proj")).toBe(true)
    expect(isAbsolutePath("\\\\server\\share")).toBe(true)
  })

  it("rejects relative paths and empty values", () => {
    expect(isAbsolutePath(".")).toBe(false)
    expect(isAbsolutePath("./src")).toBe(false)
    expect(isAbsolutePath("workspace")).toBe(false)
    expect(isAbsolutePath("")).toBe(false)
    expect(isAbsolutePath(null)).toBe(false)
    expect(isAbsolutePath(undefined)).toBe(false)
  })
})

describe("firstAbsolutePath", () => {
  it("returns the first absolute candidate in order", () => {
    expect(firstAbsolutePath(".", null, "/workspace", "/other")).toBe("/workspace")
    expect(firstAbsolutePath(null, "C:\\repo")).toBe("C:\\repo")
  })

  it("returns null when no candidate is absolute", () => {
    expect(firstAbsolutePath(".", "workspace", null, undefined)).toBeNull()
    expect(firstAbsolutePath()).toBeNull()
  })
})
