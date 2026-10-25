import { afterEach, describe, expect, it, vi } from "vitest"
import { EditorState } from "@codemirror/state"
import { EditorView } from "@codemirror/view"
import { history, redo, redoDepth, undo, undoDepth } from "@codemirror/commands"
import { findNext, openSearchPanel, search, SearchQuery, setSearchQuery } from "@codemirror/search"
import { largeFileSearch } from "./largeFileSearch"
import { runSearchTask, type SearchTask, type SearchTaskResult } from "./searchTask"

let view: EditorView | undefined
afterEach(() => { view?.destroy(); view = undefined })

const cases = [
    { name: "optional group", doc: "b ab", search: "(a)?b", replace: "$1X", expected: "X aX" },
    { name: "alternation", doc: "a b", search: "(a)|(b)", replace: "$1:$2", expected: "a: :b" },
    { name: "empty capture", doc: "b", search: "()b", replace: "$1X", expected: "X" },
    { name: "capture plus numeric suffix", doc: "b ab", search: "(a)?b", replace: "$12", expected: "2 a2" },
    { name: "twelfth optional capture", doc: "abcdefghijk abcdefghijkz", search: "(a)(b)(c)(d)(e)(f)(g)(h)(i)(j)(k)(z)?", replace: "$12!", expected: "! z!" },
    { name: "unrecognized reference", doc: "b ab", search: "(a)?b", replace: "$9-$0", expected: "$9-$0 $9-$0" },
    { name: "dollar and whole match", doc: "b ab", search: "(a)?b", replace: "<$&>|$$|$1", expected: "<b>|$| <ab>|$|a" },
    { name: "ordinary capture", doc: "ab ab", search: "(a)b", replace: "$1X", expected: "aX aX" },
    { name: "multiline capture", doc: "b\nc ab\nc", search: "(a)?b\\n(c)", replace: "$1-$2", expected: "-c a-c" }
]

function setup(doc: string, query: string, replace: string, large: boolean, options: { regexp?: boolean; literal?: boolean } = {}) {
    const prefix = large ? "x".repeat(256 * 1024) + "\n" : ""
    const worker = { postMessage: vi.fn<(task: SearchTask) => void>(), terminate: vi.fn(), onmessage: null as null | ((event: MessageEvent<SearchTaskResult>) => void), onerror: null }
    const createWorker = vi.fn(() => worker as unknown as Worker)
    view = new EditorView({ state: EditorState.create({ doc: prefix + doc, extensions: [history(), search(), largeFileSearch(createWorker)] }), parent: document.body })
    openSearchPanel(view)
    view.dispatch({ effects: setSearchQuery.of(new SearchQuery({ search: query, replace, regexp: true, caseSensitive: true, ...options })) })
    return { prefix, worker, createWorker }
}

function clickAndDeliver(action: "replace" | "replaceAll", worker: ReturnType<typeof setup>["worker"], large: boolean) {
    const before = view!.state.doc
    view!.dom.querySelector<HTMLButtonElement>(`button[name="${action}"]`)!.click()
    if (large) {
        expect(worker.postMessage).toHaveBeenCalledOnce()
        const task = worker.postMessage.mock.calls[0][0]
        // Preserve the real plugin/worker result contract, replacing only the
        // asynchronous transport with a deterministic delivery.
        worker.onmessage!({ data: runSearchTask(before, task) } as MessageEvent<SearchTaskResult>)
    }
}

for (const large of [false, true]) {
    describe(`${large ? "worker" : "small-file"} regex replacement`, () => {
        for (const test of [
            { name: "plain text dollar references", doc: "(a)?b (a)?b", query: "(a)?b", replacement: "$1X", options: { regexp: false }, expected: "$1X $1X" },
            { name: "replacement escapes", doc: "b ab", query: "(a)?b", replacement: "$1\\n\\t\\\\", options: {}, expected: "\n\t\\ a\n\t\\" },
            { name: "literal replacement escapes", doc: "b ab", query: "(a)?b", replacement: "$1\\n", options: { literal: true }, expected: "\\n a\\n" },
        ]) {
            it(`preserves ${test.name}`, () => {
                const { prefix, worker } = setup(test.doc, test.query, test.replacement, large, test.options)
                clickAndDeliver("replaceAll", worker, large)
                expect(view!.state.doc.toString()).toBe(prefix + test.expected)
                expect(undo(view!)).toBe(true)
                expect(view!.state.doc.toString()).toBe(prefix + test.doc)
                expect(redo(view!)).toBe(true)
                expect(view!.state.doc.toString()).toBe(prefix + test.expected)
            })
        }
        for (const test of cases) {
            it(`replaces all ${test.name} and preserves a single undo/redo step`, () => {
                const { prefix, worker, createWorker } = setup(test.doc, test.search, test.replace, large)
                clickAndDeliver("replaceAll", worker, large)
                expect(view!.state.doc.toString()).toBe(prefix + test.expected)
                expect(undoDepth(view!.state)).toBe(1)
                expect(undo(view!)).toBe(true)
                expect(view!.state.doc.toString()).toBe(prefix + test.doc)
                expect(undoDepth(view!.state)).toBe(0)
                expect(redoDepth(view!.state)).toBe(1)
                expect(redo(view!)).toBe(true)
                expect(view!.state.doc.toString()).toBe(prefix + test.expected)
                if (!large) expect(createWorker).not.toHaveBeenCalled()
            })
        }
        it("replaces one optional-capture match without altering the next match", () => {
            const { prefix, worker, createWorker } = setup("b ab", "(a)?b", "$1X", large)
            expect(findNext(view!)).toBe(true)
            expect(view!.state.selection.main.from).toBe(prefix.length)
            expect(view!.state.selection.main.to).toBe(prefix.length + 1)
            clickAndDeliver("replace", worker, large)
            expect(view!.state.doc.toString()).toBe(prefix + "X ab")
            expect(undo(view!)).toBe(true)
            expect(view!.state.doc.toString()).toBe(prefix + "b ab")
            expect(undoDepth(view!.state)).toBe(0)
            expect(redo(view!)).toBe(true)
            expect(view!.state.doc.toString()).toBe(prefix + "X ab")
            if (!large) expect(createWorker).not.toHaveBeenCalled()
        })
    })
}
