import { describe, expect, it } from "vitest"
import { sanitizeCustomPath, sanitizeSelection } from "./herdrPath"

// Shared vector table: the Rust normaliser (win-rust lane) must agree on these rows.
const VECTORS: Array<[string, string]> = [
  ["C:\\Tools\\herdr.exe", "C:\\Tools\\herdr.exe"],
  ['  C:\\Tools\\herdr.exe  ', "C:\\Tools\\herdr.exe"],
  ['"C:\\Program Files\\Herdr\\herdr.exe"', "C:\\Program Files\\Herdr\\herdr.exe"],
  ["'/opt/my tools/herdr'", "/opt/my tools/herdr"],
  ['  " /opt/herdr "  ', "/opt/herdr"],
  ["C:\\a b\\herdr.exe", "C:\\a b\\herdr.exe"],
  ['"C:\\half\\herdr.exe', '"C:\\half\\herdr.exe'],
  ["'mismatch\"", "'mismatch\""],
  ['"', '"'],
  ["", ""],
  ["\u201cC:\\x y\\herdr.exe\u201d", "C:\\x y\\herdr.exe"]
]

describe("sanitizeCustomPath", () => {
  it.each(VECTORS)("%j -> %j", (input, expected) => {
    expect(sanitizeCustomPath(input)).toBe(expected)
  })
})

describe("sanitizeSelection", () => {
  it("cleans only custom paths", () => {
    expect(sanitizeSelection({ source: "custom", customPath: '"C:\\a b\\herdr.exe"' })).toEqual({ source: "custom", customPath: "C:\\a b\\herdr.exe" })
    expect(sanitizeSelection({ source: "default" })).toEqual({ source: "default" })
  })
})
