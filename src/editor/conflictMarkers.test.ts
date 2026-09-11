import { describe, expect, it, vi } from "vitest"
import { EditorState } from "@codemirror/state"
import { EditorView } from "@codemirror/view"

import { applyResolution, conflictMarkers, scanConflicts } from "./conflictMarkers"

const doc = [
    "line0",
    "<<<<<<< HEAD",
    "mine a",
    "mine b",
    "=======",
    "theirs a",
    ">>>>>>> side",
    "tail"
].join("\n")

// Test helper: an EditorView with the conflictMarkers() extension installed, so
// resolution transactions run against the same StateField the widget uses.
function makeView(text: string): EditorView {
    const state = EditorState.create({ doc: text, extensions: [conflictMarkers()] })
    return new EditorView({ state })
}

// The plan's Step-1 helper: resolve a block by index, then read the doc back.
function acceptBlock(view: EditorView, index: number, choice: "current" | "incoming" | "both") {
    applyResolution(view, index, choice)
}

describe("scanConflicts", () => {
    it("finds block with ours and theirs ranges", () => {
        const blocks = scanConflicts(doc)
        expect(blocks).toHaveLength(1)
        expect(doc.slice(blocks[0].oursFrom, blocks[0].oursTo)).toBe("mine a\nmine b\n")
        expect(doc.slice(blocks[0].theirsFrom, blocks[0].theirsTo)).toBe("theirs a\n")
    })

    it("tolerates zdiff3 base section", () => {
        const z = "<<<<<<< HEAD\nmine\n||||||| base\nold\n=======\ntheirs\n>>>>>>> side\n"
        const blocks = scanConflicts(z)
        expect(blocks).toHaveLength(1)
        expect(z.slice(blocks[0].oursFrom, blocks[0].oursTo)).toBe("mine\n")
    })

    it("drops dangling block without end marker", () => {
        expect(scanConflicts("<<<<<<< HEAD\nx\n=======\ny\n")).toHaveLength(0)
    })
})

describe("conflict resolution transactions", () => {
    it("does not flatten a large conflict-free document on ordinary keystrokes", () => {
        let state = EditorState.create({ doc: "ordinary source line\n".repeat(60_000), extensions: [conflictMarkers()] })
        const toString = vi.spyOn(Object.getPrototypeOf(state.doc), "toString")
        try {
            for (let index = 0; index < 20; index++) state = state.update({ changes: { from: 5, insert: "x" } }).state
            expect(toString).not.toHaveBeenCalled()
            expect(state.doc.line(1).text).toContain("x".repeat(20))
        } finally { toString.mockRestore() }
    })

    it("detects a completed marker after typing and after deleting a leading prefix", () => {
        for (const text of ["<<<<<<< HEAD\nmine\n=======\ntheirs\n>>>>>>", "x<<<<<<< HEAD\nmine\n=======\ntheirs\n>>>>>>> side"]) {
            const view = makeView(text)
            expect(view.dom.querySelector(".cm-conflict-actions")).toBeNull()
            view.dispatch({ changes: text.startsWith("x") ? { from: 0, to: 1 } : { from: text.length, insert: "> side" } })
            expect(view.dom.querySelector(".cm-conflict-actions")).not.toBeNull()
            acceptBlock(view, 0, "current")
            expect(view.state.doc.toString()).toBe("mine\n")
            view.destroy()
        }
    })

    it("detects an outer conflict when a blocking nested start marker is removed", () => {
        const text = "<<<<<<< outer\nours\n=======\ntheirs\n<<<<<<< stray\nbody\n>>>>>>> incoming\n"
        const view = makeView(text)
        expect(view.dom.querySelector(".cm-conflict-actions")).toBeNull()
        const from = text.indexOf("<<<<<<< stray")
        view.dispatch({ changes: { from, to: from + 1 } })
        expect(view.dom.querySelector(".cm-conflict-actions")).not.toBeNull()
        acceptBlock(view, 0, "current")
        expect(view.state.doc.toString()).toBe("ours\n")
        view.destroy()
    })

    it("accept current keeps ours and strips markers", () => {
        const view = makeView(doc)
        acceptBlock(view, 0, "current")
        expect(view.state.doc.toString()).toBe("line0\nmine a\nmine b\ntail")
        view.destroy()
    })

    it("accept incoming keeps theirs and strips markers", () => {
        const view = makeView(doc)
        acceptBlock(view, 0, "incoming")
        expect(view.state.doc.toString()).toBe("line0\ntheirs a\ntail")
        view.destroy()
    })

    it("accept both concatenates", () => {
        const view = makeView(doc)
        acceptBlock(view, 0, "both")
        expect(view.state.doc.toString()).toBe("line0\nmine a\nmine b\ntheirs a\ntail")
        view.destroy()
    })
})
