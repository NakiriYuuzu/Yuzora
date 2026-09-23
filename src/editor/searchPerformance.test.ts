import { expect, it, vi } from "vitest"
import { EditorState } from "@codemirror/state"
import { EditorView } from "@codemirror/view"
import { closeSearchPanel, findNext, openSearchPanel, SearchQuery, searchPanelOpen, setSearchQuery } from "@codemirror/search"
import { buildExtensions } from "./cmExtensions"
import { conflictMarkers } from "./conflictMarkers"

it("opens and navigates search in a large document without rebuilding its text or marking it dirty", () => {
    const dirty = vi.fn()
    const doc = "const text = 'needle';\n".repeat(100000)
    const view = new EditorView({ state: EditorState.create({ doc, extensions: [...buildExtensions("/large.ts", { readonly: false, syntaxOff: true }, dirty, () => {}, true), conflictMarkers()] }), parent: document.body })
    const original = view.state.doc
    const flatten = vi.spyOn(original, "toString")
    const start = performance.now()
    try {
        for (let i = 0; i < 10; i++) {
            openSearchPanel(view)
            view.dispatch({ effects: setSearchQuery.of(new SearchQuery({ search: "needle" })) })
            expect(searchPanelOpen(view.state)).toBe(true)
            expect(findNext(view)).toBe(true)
            closeSearchPanel(view)
        }
        expect(dirty).not.toHaveBeenCalled()
        expect(flatten).not.toHaveBeenCalled()
        expect(view.state.doc).toBe(original)
        console.info(`100k-line search, 10 open/find/close cycles: ${Math.round(performance.now() - start)} ms (jsdom, excludes browser layout)`)
    } finally {
        flatten.mockRestore()
        view.destroy()
    }
})
