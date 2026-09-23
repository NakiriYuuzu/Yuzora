import { afterEach, describe, expect, it } from "vitest"
import { EditorSelection, EditorState } from "@codemirror/state"
import { EditorView } from "@codemirror/view"
import {
    clearEditorViewStatesForTest,
    editorViewStateTracker,
    renameEditorViewState,
    restoreEditorViewState
} from "./editorViewState"

const views: EditorView[] = []

function mount(doc: string, workspace: string | null, path: string) {
    const view = new EditorView({
        state: EditorState.create({ doc, extensions: editorViewStateTracker(workspace, path) }),
        parent: document.body
    })
    views.push(view)
    return view
}

afterEach(() => {
    for (const view of views.splice(0)) view.destroy()
    clearEditorViewStatesForTest()
})

describe("editorViewState", () => {
    it("remembers the selection and scroll anchor after the view is destroyed", () => {
        const doc = Array.from({ length: 300 }, (_, i) => `line ${i + 1}`).join("\n")
        const view = mount(doc, "/w", "/w/a.txt")
        const line200 = view.state.doc.line(200)
        view.dispatch({ selection: EditorSelection.cursor(line200.from + 2) })
        view.scrollDOM.dispatchEvent(new Event("scroll"))
        view.destroy()

        const restored = restoreEditorViewState("/w", "/w/a.txt", EditorState.create({ doc }).doc)
        expect(restored.selection?.main.head).toBe(line200.from + 2)
        expect(restored.scrollTo).toBeDefined()
    })

    it("keys state by workspace and follows renames", () => {
        const view = mount("hello\nworld", "/w", "/w/a.txt")
        view.dispatch({ selection: EditorSelection.cursor(8) })
        const doc = view.state.doc

        expect(restoreEditorViewState("/other", "/w/a.txt", doc)).toEqual({})
        renameEditorViewState("/w", "/w/a.txt", "/w/b.txt")
        expect(restoreEditorViewState("/w", "/w/a.txt", doc)).toEqual({})
        expect(restoreEditorViewState("/w", "/w/b.txt", doc).selection?.main.head).toBe(8)
    })

    it("clamps the selection and skips scroll when the document changed on disk", () => {
        const view = mount("0123456789", "/w", "/w/a.txt")
        view.dispatch({ selection: EditorSelection.cursor(9) })
        view.scrollDOM.dispatchEvent(new Event("scroll"))

        const restored = restoreEditorViewState("/w", "/w/a.txt", EditorState.create({ doc: "0123" }).doc)
        expect(restored.selection?.main.head).toBe(4)
        expect(restored.scrollTo).toBeUndefined()
    })
})
