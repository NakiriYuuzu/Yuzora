import { describe, expect, it } from "vitest"

import { lineDiffCounts } from "./lineDiff"

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
})
