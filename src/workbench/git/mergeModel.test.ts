import { describe, expect, it } from "vitest"

import { mergeRegions, resolveSimpleConflict, splitLines } from "./mergeModel"

const lines = (...values: string[]) => values.map((value) => `${value}\n`)

describe("splitLines", () => {
    it("keeps terminators so joining restores the text", () => {
        expect(splitLines("a\nb\nc")).toEqual(["a\n", "b\n", "c"])
        expect(splitLines("a\r\nb\n")).toEqual(["a\r\n", "b\n"])
        expect(splitLines("")).toEqual([])
    })
})

describe("mergeRegions", () => {
    it("separates one-sided, identical and conflicting changes", () => {
        const base = lines("1", "2", "3", "4", "x", "5", "6", "7")
        const ours = lines("1", "2-ours", "3", "4", "x", "5-same", "6", "7-ours")
        const theirs = lines("1", "2", "3", "4-theirs", "x", "5-same", "6", "7-theirs")
        expect(mergeRegions(base, ours, theirs)).toEqual([
            { kind: "ours", base: [1, 2], ours: [1, 2], theirs: [1, 2] },
            { kind: "theirs", base: [3, 4], ours: [3, 4], theirs: [3, 4] },
            { kind: "both", base: [5, 6], ours: [5, 6], theirs: [5, 6] },
            { kind: "conflict", base: [7, 8], ours: [7, 8], theirs: [7, 8] }
        ])
    })

    it("tracks offsets after insertions and deletions on each side", () => {
        const base = lines("a", "b", "c", "d")
        const ours = lines("a", "a2", "a3", "b", "c", "d")
        const theirs = lines("a", "b", "d")
        expect(mergeRegions(base, ours, theirs)).toEqual([
            { kind: "ours", base: [1, 1], ours: [1, 3], theirs: [1, 1] },
            { kind: "theirs", base: [2, 3], ours: [4, 5], theirs: [2, 2] }
        ])
    })

    it("treats touching edits from both sides as one conflict", () => {
        const base = lines("a", "b", "c")
        const ours = lines("a", "B", "c")
        const theirs = lines("a", "b", "C")
        expect(mergeRegions(base, ours, theirs)).toEqual([
            { kind: "conflict", base: [1, 3], ours: [1, 3], theirs: [1, 3] }
        ])
    })

    it("reports no regions for identical inputs", () => {
        const base = lines("same")
        expect(mergeRegions(base, base, base)).toEqual([])
    })
})

describe("resolveSimpleConflict", () => {
    it("merges edits to different parts of the same line", () => {
        expect(resolveSimpleConflict("let a = 1, b = 2\n", "let a = 10, b = 2\n", "let a = 1, b = 20\n"))
            .toBe("let a = 10, b = 20\n")
    })

    it("refuses overlapping edits", () => {
        expect(resolveSimpleConflict("value = 1\n", "value = 2\n", "value = 3\n")).toBeNull()
    })
})
