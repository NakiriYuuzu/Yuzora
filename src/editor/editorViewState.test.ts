import { afterEach, describe, expect, it } from "vitest"
import { history, historyField, undo } from "@codemirror/commands"
import { EditorSelection, EditorState } from "@codemirror/state"
import { EditorView } from "@codemirror/view"
import {
    clearEditorViewStatesForTest,
    editorViewStateTracker,
    rememberEditorHistory,
    renameEditorViewState,
    restoreEditorHistory,
    restoreEditorViewState
} from "./editorViewState"
import { registerView, unregisterView } from "./viewRegistry"

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

describe("editor undo history", () => {
    const docOf = (text: string) => EditorState.create({ doc: text }).doc

    it("replays undo history onto a new state for the exact same text", () => {
        let state = EditorState.create({ doc: "abc", extensions: history() })
        state = state.update({ changes: { from: 3, insert: "d" }, userEvent: "input.type" }).state
        rememberEditorHistory("/w", "/w/a.txt", state)

        const saved = restoreEditorHistory("/w", "/w/a.txt", docOf("abcd"))
        expect(saved).toBeDefined()
        let restored = EditorState.fromJSON(
            { doc: "abcd", selection: EditorSelection.single(4).toJSON(), history: saved },
            { extensions: history() },
            { history: historyField }
        )
        expect(undo({ state: restored, dispatch: tr => { restored = tr.state } })).toBe(true)
        expect(restored.doc.toString()).toBe("abc")
    })

    it("drops the history when the text changed, and keys it by workspace and rename", () => {
        let state = EditorState.create({ doc: "abc", extensions: history() })
        state = state.update({ changes: { from: 3, insert: "d" } }).state
        rememberEditorHistory("/w", "/w/a.txt", state)

        expect(restoreEditorHistory("/w", "/w/a.txt", docOf("abcx"))).toBeUndefined()
        expect(restoreEditorHistory("/other", "/w/a.txt", docOf("abcd"))).toBeUndefined()
        renameEditorViewState("/w", "/w/a.txt", "/w/b.txt")
        expect(restoreEditorHistory("/w", "/w/a.txt", docOf("abcd"))).toBeUndefined()
        expect(restoreEditorHistory("/w", "/w/b.txt", docOf("abcd"))).toBeDefined()
    })

    it("records nothing for a state without the history extension", () => {
        rememberEditorHistory("/w", "/w/a.txt", EditorState.create({ doc: "abc" }))
        expect(restoreEditorHistory("/w", "/w/a.txt", docOf("abc"))).toBeUndefined()
    })

    it("records nothing when there is nothing to undo, so browsing cannot evict real edits", () => {
        let edited = EditorState.create({ doc: "abc", extensions: history() })
        edited = edited.update({ changes: { from: 3, insert: "d" } }).state
        rememberEditorHistory("/w", "/w/edited.txt", edited)
        for (let i = 0; i < 40; i++) {
            rememberEditorHistory("/w", `/w/viewed-${i}.txt`, EditorState.create({ doc: "x", extensions: history() }))
        }
        expect(restoreEditorHistory("/w", "/w/viewed-0.txt", docOf("x"))).toBeUndefined()
        expect(restoreEditorHistory("/w", "/w/edited.txt", docOf("abcd"))).toBeDefined()
    })

    it("does not keep a history larger than the budget", () => {
        // Rich Markdown edits replace the whole document and never join, so each
        // undo step keeps a full copy of the previous text.
        const big = "x".repeat(2_500_000)
        let state = EditorState.create({ doc: big, extensions: history() })
        for (const next of ["y", "z"]) {
            state = state.update({
                changes: { from: 0, to: state.doc.length, insert: next.repeat(2_500_000) },
                userEvent: "input.richMarkdown"
            }).state
        }
        rememberEditorHistory("/w", "/w/big.txt", state)
        expect(restoreEditorHistory("/w", "/w/big.txt", state.doc)).toBeUndefined()
    })

    it("takes the live view's history when an open document is renamed", () => {
        const view = new EditorView({
            state: EditorState.create({ doc: "abc", extensions: history() }),
            parent: document.body
        })
        views.push(view)
        registerView("/w/old.txt", view)
        try {
            view.dispatch({ changes: { from: 3, insert: "d" } })
            renameEditorViewState("/w", "/w/old.txt", "/w/new.txt")
            expect(restoreEditorHistory("/w", "/w/new.txt", docOf("abcd"))).toBeDefined()
            expect(restoreEditorHistory("/w", "/w/old.txt", docOf("abcd"))).toBeUndefined()
        } finally {
            unregisterView("/w/old.txt", view)
        }
    })
})
