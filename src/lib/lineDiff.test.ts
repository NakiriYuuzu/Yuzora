import { describe, expect, it, vi } from "vitest"

import { LINE_DIFF_CELL_LIMIT, lineDiffCounts } from "./lineDiff"

describe("lineDiffCounts", () => {
    it("empty diff → 0/0", () => {
        expect(lineDiffCounts("", "")).toEqual({ added: 0, deleted: 0 })
        expect(lineDiffCounts("a\nb\n", "a\nb\n")).toEqual({ added: 0, deleted: 0 })
    })

    it("pure additions", () => {
        expect(lineDiffCounts("", "a\nb\n")).toEqual({ added: 2, deleted: 0 })
        expect(lineDiffCounts("a\n", "a\nb\nc\n")).toEqual({ added: 2, deleted: 0 })
    })

    it("pure deletions", () => {
        expect(lineDiffCounts("a\nb\n", "")).toEqual({ added: 0, deleted: 2 })
        expect(lineDiffCounts("a\nb\nc\n", "a\n")).toEqual({ added: 0, deleted: 2 })
    })

    it("single-line modification counts as +1/−1", () => {
        expect(lineDiffCounts("a\nb\nc\n", "a\nB\nc\n")).toEqual({ added: 1, deleted: 1 })
    })

    it("mixed add + delete", () => {
        // remove "b", add "x" and "y"
        expect(lineDiffCounts("a\nb\nc\n", "a\nc\nx\ny\n")).toEqual({ added: 2, deleted: 1 })
    })

    it("trailing newline does not inflate line count", () => {
        expect(lineDiffCounts("a", "a\n")).toEqual({ added: 0, deleted: 0 })
    })

    it("returns null when the LCS table would exceed the cell limit", () => {
        const n = Math.floor(Math.sqrt(LINE_DIFF_CELL_LIMIT)) + 1
        const a = Array.from({ length: n }, (_, i) => `a${i}`).join("\n")
        const b = Array.from({ length: n }, (_, i) => `b${i}`).join("\n")
        expect(n * n).toBeGreaterThan(LINE_DIFF_CELL_LIMIT)
        expect(lineDiffCounts(a, b)).toBeNull()
        expect(lineDiffCounts("a\n", "b\n")).toEqual({ added: 1, deleted: 1 })
    })

    it("rejects oversized inputs before splitting into line arrays", () => {
        const n = Math.floor(Math.sqrt(LINE_DIFF_CELL_LIMIT)) + 1
        const a = Array.from({ length: n }, (_, i) => `a${i}`).join("\n")
        const b = Array.from({ length: n }, (_, i) => `b${i}`).join("\n")
        const split = vi.spyOn(String.prototype, "split")
        split.mockClear()
        expect(lineDiffCounts(a, b)).toBeNull()
        expect(split).not.toHaveBeenCalled()
        split.mockRestore()
    })

    it("counts an empty side without splitting the other side", () => {
        const original = Array.from({ length: 4000 }, (_, i) => `l${i}`).join("\n")
        const split = vi.spyOn(String.prototype, "split")
        split.mockClear()
        expect(lineDiffCounts(original, "")).toEqual({ added: 0, deleted: 4000 })
        expect(split).not.toHaveBeenCalled()
        split.mockRestore()
    })
})
