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

    it("matches a full-table oracle for repeated lines and shared boundaries", () => {
        function oracle(original: string, modified: string) {
            const lines = (text: string) => text === "" ? [] : text.replace(/\n$/, "").split("\n")
            const a = lines(original)
            const b = lines(modified)
            const table = Array.from({ length: a.length + 1 }, () => new Array<number>(b.length + 1).fill(0))
            for (let i = 1; i <= a.length; i++) {
                for (let j = 1; j <= b.length; j++) {
                    table[i][j] = a[i - 1] === b[j - 1]
                        ? table[i - 1][j - 1] + 1
                        : Math.max(table[i - 1][j], table[i][j - 1])
                }
            }
            const common = table[a.length][b.length]
            return { added: b.length - common, deleted: a.length - common }
        }
        const documents = new Set([""])
        let rows: string[][] = [[]]
        for (let length = 1; length <= 3; length++) {
            rows = rows.flatMap(prefix => ["", "a", "b"].map(line => [...prefix, line]))
            for (const row of rows) {
                documents.add(row.join("\n"))
                documents.add(row.join("\n") + "\n")
            }
        }
        for (const original of documents) {
            for (const modified of documents) {
                expect(lineDiffCounts(original, modified)).toEqual(oracle(original, modified))
            }
        }
        let seed = 41
        const random = () => (seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0)
        const values = ["", "a", "b", "\r", "中文😀"]
        for (let run = 0; run < 200; run++) {
            const text = () => Array.from({ length: random() % 20 }, () => values[random() % values.length]).join("\n") + (random() % 2 ? "\n" : "")
            const original = text()
            const modified = text()
            expect(lineDiffCounts(original, modified)).toEqual(oracle(original, modified))
        }
    })

    it("keeps the full-input cell limit for identical and mostly identical documents", () => {
        const n = Math.floor(Math.sqrt(LINE_DIFF_CELL_LIMIT)) + 1
        const original = Array.from({ length: n }, (_, i) => `line-${i}`).join("\n")
        const modified = original.replace("line-200", "changed-200")
        const split = vi.spyOn(String.prototype, "split")
        try {
            split.mockClear()
            expect(lineDiffCounts(original, original)).toBeNull()
            expect(lineDiffCounts(original, modified)).toBeNull()
            expect(split).not.toHaveBeenCalled()
        } finally {
            split.mockRestore()
        }
    })

    it("counts shared lines at either boundary of highly unbalanced inputs", () => {
        const rows = Array.from({ length: 40_000 }, (_, i) => `line-${i}`)
        const original = rows.join("\n")
        for (const selected of [rows.slice(0, 5), rows.slice(-5)]) {
            const modified = selected.join("\n")
            expect(lineDiffCounts(original, modified)).toEqual({ added: 0, deleted: 39_995 })
            expect(lineDiffCounts(modified, original)).toEqual({ added: 39_995, deleted: 0 })
        }
    })
})
