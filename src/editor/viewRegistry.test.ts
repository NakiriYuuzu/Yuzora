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

    it("resolves Windows drive / verbatim / slash aliases to the same view", () => {
        const v = makeView()
        const raw = String.raw`\\?\C:\Users\Yuuzu\project\src\main.ts`
        registerView(raw, v)

        expect(getView("C:/Users/Yuuzu/project/src/main.ts")).toBe(v)
        expect(getView(String.raw`C:\Users\Yuuzu\project\src\main.ts`)).toBe(v)
        expect(getView("c:/Users/Yuuzu/project/src/main.ts")).toBe(v)

        updateViewMetadata("C:/Users/Yuuzu/project/src/main.ts", v, { groupIndex: 0 })
        expect(getViewEntry(raw)).toMatchObject({ view: v, groupIndex: 0 })

        unregisterView("C:/Users/Yuuzu/project/src/main.ts", v)
        expect(getView(raw)).toBeUndefined()
    })

    it("resolves Windows UNC and LSP forward-slash UNC aliases to the same view", () => {
        const v = makeView()
        const raw = String.raw`\\Server\Share\Project\src\main.ts`
        registerView(raw, v)

        expect(getView("//server/share/project/src/main.ts")).toBe(v)
        unregisterView("//SERVER/SHARE/PROJECT/src/main.ts", v)
        expect(getView(raw)).toBeUndefined()
    })

    it("keeps ordinary and double-slash POSIX paths case-sensitive", () => {
        const lower = makeView()
        const upper = makeView()
        const doubleSlash = makeView()
        registerView("/Users/yuuzu/App/main.ts", lower)
        registerView("/Users/yuuzu/App/Main.ts", upper)
        registerView("//CaseHost/Share/File.ts", doubleSlash)

        expect(getView("/Users/yuuzu/App/main.ts")).toBe(lower)
        expect(getView("/Users/yuuzu/App/Main.ts")).toBe(upper)
        expect(getView("//CaseHost/Share/File.ts")).toBe(doubleSlash)
        expect(getView("//casehost/share/file.ts")).toBeUndefined()

        unregisterView("/Users/yuuzu/App/main.ts", lower)
        expect(getView("/Users/yuuzu/App/Main.ts")).toBe(upper)
        unregisterView("/Users/yuuzu/App/Main.ts", upper)
        unregisterView("//CaseHost/Share/File.ts", doubleSlash)
    })

    it.each(["windows-first", "posix-first"] as const)(
        "keeps exact POSIX // and backslash UNC registrations separate (%s)",
        (order) => {
            const windows = makeView()
            const posix = makeView()
            const uncPath = String.raw`\\Server\Share\File.ts`
            const posixPath = "//server/share/file.ts"

            if (order === "windows-first") {
                registerView(uncPath, windows, { groupIndex: 1 })
                registerView(posixPath, posix, { groupIndex: 2 })
            } else {
                registerView(posixPath, posix, { groupIndex: 2 })
                registerView(uncPath, windows, { groupIndex: 1 })
            }

            // Exact POSIX identity always wins for the ambiguous forward-slash
            // spelling; unambiguous Windows syntax still resolves its own view.
            expect(getView(posixPath)).toBe(posix)
            expect(getView(uncPath)).toBe(windows)
            expect(getView(String.raw`\\server\share\file.ts`)).toBe(windows)

            updateViewMetadata(uncPath, windows, { formatter: "available" })
            updateViewMetadata(posixPath, posix, { readonly: true })
            expect(getViewEntry(uncPath)).toMatchObject({
                view: windows,
                groupIndex: 1,
                formatter: "available"
            })
            expect(getViewEntry(posixPath)).toMatchObject({
                view: posix,
                groupIndex: 2,
                readonly: true
            })

            unregisterView(posixPath, posix)
            expect(getView(posixPath)).toBe(windows)
            expect(getView(uncPath)).toBe(windows)

            unregisterView(uncPath, windows)
            expect(getView(posixPath)).toBeUndefined()
            expect(getView(uncPath)).toBeUndefined()
        }
    )
})
