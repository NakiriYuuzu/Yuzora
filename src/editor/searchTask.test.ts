import { describe, expect, it } from "vitest"
import { Text } from "@codemirror/state"
import { runSearchTask, type SearchTask } from "./searchTask"

const task = (overrides: Partial<SearchTask> = {}): SearchTask => ({ id: 1, action: "next", from: 0, to: 0, query: { search: "needle", replace: "changed", regexp: false, literal: false, caseSensitive: false, wholeWord: false }, ...overrides })

describe("background document search", () => {
    it("preserves overlapping matches when searching backwards", () => {
        const input = task({ action: "prev", from: 6, to: 6 })
        input.query.search = "ana"
        expect(runSearchTask(Text.of(["banana"]), input).ranges).toEqual([{ from: 3, to: 6 }])
    })

    it("finds distant matches, wraps and navigates backwards", () => {
        const doc = Text.of(["needle", ...Array<string>(100000).fill("other text"), "needle"])
        const last = doc.length - 6
        expect(runSearchTask(doc, task({ from: 0, to: 6 })).ranges).toEqual([{ from: last, to: doc.length }])
        expect(runSearchTask(doc, task({ from: last, to: doc.length })).ranges).toEqual([{ from: 0, to: 6 }])
        expect(runSearchTask(doc, task({ action: "prev", from: last, to: doc.length })).ranges).toEqual([{ from: 0, to: 6 }])
        expect(runSearchTask(doc, task({ action: "prev" })).ranges).toEqual([{ from: last, to: doc.length }])
    })

    it("preserves Unicode normalization and whole-word matching", () => {
        const input = task()
        input.query = { ...input.query, search: "café", wholeWord: true }
        const doc = Text.of(["caféteria CAFE\u0301 café"])
        expect(runSearchTask(doc, input).ranges).toEqual([{ from: 10, to: 15 }])
        expect(runSearchTask(doc, { ...input, action: "select" }).ranges).toEqual([{ from: 10, to: 15 }, { from: 16, to: 20 }])
        input.query.search = "foo"
        expect(runSearchTask(Text.of(["$foo foo"]), { ...input, wordChars: "$" }).ranges).toEqual([{ from: 5, to: 8 }])
    })

    it("handles multiline regex replacement groups without changing offsets", () => {
        const input = task({ action: "replaceAll" })
        input.query = { ...input.query, search: "(a)\\n(b)", regexp: true, replace: "$2-$1" }
        const result = runSearchTask(Text.of(["a", "b a", "b"]), input)
        expect(result.changes).toEqual([{ from: 0, to: 3, insert: "b-a" }, { from: 4, to: 7, insert: "b-a" }])
    })

    it("does not replace a partial normalized character and caps selection output", () => {
        const input = task({ action: "replaceAll" })
        input.query = { ...input.query, search: "f" }
        expect(runSearchTask(Text.of(["ﬀ"]), input).changes).toEqual([])
        expect(runSearchTask(Text.of(["needle ".repeat(1001)]), task({ action: "select" }))).toMatchObject({ error: "tooMany", ranges: [] })
    })

    it("moves past the current zero-width regex match in both directions", () => {
        const input = task({ from: 4, to: 4 })
        input.query = { ...input.query, search: "^", replace: "> ", regexp: true }
        const doc = Text.of(["abc", "def", "ghi"])
        expect(runSearchTask(doc, input).ranges).toEqual([{ from: 8, to: 8 }])
        expect(runSearchTask(doc, { ...input, from: 8, to: 8 }).ranges).toEqual([{ from: 0, to: 0 }])
        expect(runSearchTask(doc, { ...input, action: "prev" }).ranges).toEqual([{ from: 0, to: 0 }])
        expect(runSearchTask(doc, { ...input, action: "prev", from: 0, to: 0 }).ranges).toEqual([{ from: 8, to: 8 }])
        expect(runSearchTask(doc, { ...input, action: "replace" })).toMatchObject({ changes: [{ from: 4, to: 4, insert: "> " }], ranges: [{ from: 8, to: 8 }] })
    })

    it("wraps a regex previous-match search from the selection start like CodeMirror", () => {
        const input = task({ action: "prev", from: 0, to: 6 })
        input.query = { ...input.query, regexp: true }
        expect(runSearchTask(Text.of(["needle"]), input).ranges).toEqual([{ from: 0, to: 6 }])
        expect(runSearchTask(Text.of(["needle other needle"]), input).ranges).toEqual([{ from: 13, to: 19 }])
    })

    it("replaces only the selected current match, then finds the next", () => {
        expect(runSearchTask(Text.of(["needle needle"]), task({ action: "replace", to: 6 }))).toMatchObject({ changes: [{ from: 0, to: 6, insert: "changed" }], ranges: [{ from: 7, to: 13 }] })
        expect(runSearchTask(Text.of(["needle needle"]), task({ action: "replace" })).changes).toBeUndefined()
    })
})
