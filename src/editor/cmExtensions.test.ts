import { expect, test } from "vitest"
import { hasVeryLongLine, languageExtensionFromPath } from "./cmExtensions"

test("hasVeryLongLine 偵測超長單行", () => {
    expect(hasVeryLongLine("short\nlines\n")).toBe(false)
    expect(hasVeryLongLine(`a\n${"x".repeat(10_001)}\nb`)).toBe(true)
})

test("languageExtensionFromPath 對已知副檔名回傳 extension、未知回傳 null", () => {
    expect(languageExtensionFromPath("/a.ts")).not.toBeNull()
    expect(languageExtensionFromPath("/a.rs")).not.toBeNull()
    expect(languageExtensionFromPath("/a.unknown")).toBeNull()
})
