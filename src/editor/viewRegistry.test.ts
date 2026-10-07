import { afterEach, describe, expect, it, vi } from "vitest"
import { EditorState } from "@codemirror/state"
import { EditorView } from "@codemirror/view"

import {
    getView,
    getViewEntry,
    registerView,
    subscribeView,
    unregisterView
} from "./viewRegistry"

const testViews: EditorView[] = []
afterEach(() => {
    for (const view of testViews.splice(0)) view.destroy()
})

function makeView(): EditorView {
    const view = new EditorView({ state: EditorState.create({ doc: "" }) })
    testViews.push(view)
    return view
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

    it("resolves Windows drive / verbatim / slash aliases to the same view", () => {
        const v = makeView()
        const raw = String.raw`\\?\C:\Users\Yuuzu\project\src\main.ts`
        registerView(raw, v)

        expect(getView("C:/Users/Yuuzu/project/src/main.ts")).toBe(v)
        expect(getView(String.raw`C:\Users\Yuuzu\project\src\main.ts`)).toBe(v)
        expect(getView("c:/Users/Yuuzu/project/src/main.ts")).toBe(v)

        expect(getViewEntry(raw)).toMatchObject({ view: v })

        unregisterView("C:/Users/Yuuzu/project/src/main.ts", v)
        expect(getView(raw)).toBeUndefined()
    })

    it("resolves Windows UNC and forward-slash UNC aliases to the same view", () => {
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

            registerView(uncPath, windows, { groupIndex: 1, readonly: false })
            registerView(posixPath, posix, { groupIndex: 2, readonly: true })
            expect(getViewEntry(uncPath)).toMatchObject({
                view: windows,
                groupIndex: 1,
                readonly: false
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

describe("viewRegistry subscriptions", () => {
    it("attaches to a late view, coalesces typing, and ignores selection-only updates", async () => {
        const listener = vi.fn()
        const stop = subscribeView("/w/late.md", listener)
        const view = makeView()
        registerView("/w/late.md", view)
        expect(listener).not.toHaveBeenCalled()
        await Promise.resolve()
        expect(listener.mock.calls).toEqual([["view"]])
        listener.mockClear()
        view.dispatch({ changes: { from: 0, insert: "a" } })
        view.dispatch({ changes: { from: 1, insert: "b" } })
        await Promise.resolve()
        expect(listener.mock.calls).toEqual([["document"]])
        listener.mockClear()
        view.dispatch({ selection: { anchor: 1 } })
        await Promise.resolve()
        expect(listener).not.toHaveBeenCalled()
        unregisterView("/w/late.md", view)
        await Promise.resolve()
        expect(listener.mock.calls).toEqual([["view"]])
        expect(view.state.facet(EditorView.updateListener)).toHaveLength(0)
        stop()
    })

    it("shares one listener, removes it on last unsubscribe, and cancels queued notifications", async () => {
        const view = makeView()
        registerView("/w/shared.md", view)
        const first = vi.fn()
        const second = vi.fn()
        const stopFirst = subscribeView("/w/shared.md", first)
        const stopSecond = subscribeView("/w/shared.md", second)
        expect(view.state.facet(EditorView.updateListener)).toHaveLength(1)
        view.dispatch({ changes: { from: 0, insert: "a" } })
        stopFirst()
        stopFirst()
        await Promise.resolve()
        expect(first).not.toHaveBeenCalled()
        expect(second).toHaveBeenCalledWith("document")
        stopSecond()
        expect(view.state.facet(EditorView.updateListener)).toHaveLength(0)
        for (let i = 0; i < 10; i++) {
            const stop = subscribeView("/w/shared.md", first)
            expect(view.state.facet(EditorView.updateListener)).toHaveLength(1)
            stop()
            expect(view.state.facet(EditorView.updateListener)).toHaveLength(0)
        }
        unregisterView("/w/shared.md", view)
        await Promise.resolve()
        expect(first).not.toHaveBeenCalled()
    })

    it("follows replacements and ignores stale unregisters and old document changes", async () => {
        const first = makeView()
        const second = makeView()
        registerView("/w/replace.md", first)
        const listener = vi.fn()
        const stop = subscribeView("/w/replace.md", listener)
        registerView("/w/replace.md", second)
        unregisterView("/w/replace.md", first)
        await Promise.resolve()
        expect(listener.mock.calls).toEqual([["view"]])
        expect(first.state.facet(EditorView.updateListener)).toHaveLength(0)
        listener.mockClear()
        first.dispatch({ changes: { from: 0, insert: "old" } })
        await Promise.resolve()
        expect(listener).not.toHaveBeenCalled()
        second.dispatch({ changes: { from: 0, insert: "new" } })
        await Promise.resolve()
        expect(listener).toHaveBeenCalledWith("document")
        stop()
        unregisterView("/w/replace.md", second)
    })

    it("uses exact POSIX precedence then falls back to the Windows UNC subscription", async () => {
        const windows = makeView()
        const posix = makeView()
        const path = "//server/share/file.ts"
        const unc = String.raw`\\Server\Share\File.ts`
        const listener = vi.fn()
        const stop = subscribeView(path, listener)
        registerView(unc, windows)
        await Promise.resolve()
        registerView(path, posix)
        await Promise.resolve()
        expect(windows.state.facet(EditorView.updateListener)).toHaveLength(0)
        expect(posix.state.facet(EditorView.updateListener)).toHaveLength(1)
        unregisterView(path, posix)
        await Promise.resolve()
        expect(listener).toHaveBeenCalledTimes(3)
        expect(windows.state.facet(EditorView.updateListener)).toHaveLength(1)
        stop()
        unregisterView(unc, windows)
    })
})
