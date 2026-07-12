import { describe, expect, it } from "vitest"
import { EditorState } from "@codemirror/state"
import { EditorView } from "@codemirror/view"

import {
    getView,
    getViewEntry,
    registerView,
    unregisterView,
    updateViewMetadata
} from "./viewRegistry"

function makeView(): EditorView {
    return new EditorView({ state: EditorState.create({ doc: "" }) })
}

describe("viewRegistry", () => {
    it("registers and retrieves a view, then unregisters with the matching view", () => {
        const v = makeView()
        registerView("/w/a.ts", v)
        expect(getView("/w/a.ts")).toBe(v)
        unregisterView("/w/a.ts", v)
        expect(getView("/w/a.ts")).toBeUndefined()
    })

    it("unregister with a non-matching view keeps the current registration (m4)", () => {
        const first = makeView()
        const second = makeView()
        registerView("/w/b.ts", first)
        registerView("/w/b.ts", second) // a second split group overwrites the entry
        // The first pane unmounts and tries to remove its own (now stale) view; it
        // must NOT clobber the second group's live registration.
        unregisterView("/w/b.ts", first)
        expect(getView("/w/b.ts")).toBe(second)
        // The owning view removes it.
        unregisterView("/w/b.ts", second)
        expect(getView("/w/b.ts")).toBeUndefined()
    })

    it("unregister without a view removes unconditionally (back-compat)", () => {
        const v = makeView()
        registerView("/w/c.ts", v)
        unregisterView("/w/c.ts")
        expect(getView("/w/c.ts")).toBeUndefined()
    })

    it("tracks clicked-view metadata and only lets the owning view update it", () => {
        const owner = makeView()
        const stale = makeView()
        const formatDocument = async () => true
        registerView("/w/meta.ts", owner, {
            groupIndex: 1,
            readonly: true,
            formatter: "checking"
        })

        updateViewMetadata("/w/meta.ts", stale, { formatter: "available", formatDocument })
        expect(getViewEntry("/w/meta.ts")).toMatchObject({
            view: owner,
            groupIndex: 1,
            readonly: true,
            formatter: "checking"
        })

        updateViewMetadata("/w/meta.ts", owner, { formatter: "available", formatDocument })
        expect(getViewEntry("/w/meta.ts")).toMatchObject({
            view: owner,
            groupIndex: 1,
            readonly: true,
            formatter: "available",
            formatDocument
        })
        unregisterView("/w/meta.ts", owner)
    })
})
