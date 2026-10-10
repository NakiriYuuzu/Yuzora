import { afterEach, expect, it, vi } from "vitest"
import { waitFor } from "@testing-library/react"
import { EditorState, StateEffect } from "@codemirror/state"
import { EditorView } from "@codemirror/view"
import { language } from "@codemirror/language"
import { undo } from "@codemirror/commands"
import { buildExtensions, languageExtensions, languageExtensionFromPath, loadLanguageExtension } from "./cmExtensions"

const grammar = vi.hoisted(() => {
    let release!: () => void
    const ready = new Promise<void>(resolve => { release = resolve })
    return { ready, release, imports: vi.fn() }
})
vi.mock("@codemirror/lang-rust", async importOriginal => {
    grammar.imports()
    await grammar.ready
    return importOriginal()
})
vi.mock("@codemirror/lang-java", () => {
    throw new Error("Optional grammar unavailable")
})

const views: EditorView[] = []
afterEach(() => { views.forEach(view => view.destroy()) })

it("loads once on demand, preserves live editor state, and ignores retired views and paths", async () => {
    const changed = vi.fn()
    const extensions = (path: string, syntaxOff = false) => buildExtensions(path, { readonly: false, syntaxOff }, changed, () => {}, false)
    const makeView = (path: string, syntaxOff = false) => {
        const view = new EditorView({ state: EditorState.create({ doc: "fn main() {}", extensions: extensions(path, syntaxOff) }), parent: document.body })
        views.push(view)
        return view
    }

    // Large-file guards must avoid even requesting the optional grammar.
    const limited = makeView("large.rs", true)
    await Promise.resolve()
    expect(grammar.imports).not.toHaveBeenCalled()
    expect(languageExtensionFromPath("main.rs")).toBeNull()

    const retired = makeView("old.rs")
    const switched = makeView("previous.rs")
    const active = makeView("main.rs")
    const editorDom = active.dom
    const pending = loadLanguageExtension("main.rs")
    expect(loadLanguageExtension("another.rs")).toBe(pending)
    await waitFor(() => expect(grammar.imports).toHaveBeenCalledTimes(1))
    expect(active.state.facet(language)).toBeNull()
    active.dispatch({ changes: { from: 0, insert: "// edit\n" }, selection: { anchor: 3 } })
    active.scrollDOM.scrollTop = 120
    const selection = active.state.selection
    const doc = active.state.doc

    retired.destroy()
    const retiredDispatch = vi.spyOn(retired, "dispatch")
    switched.dispatch({ effects: StateEffect.reconfigure.of(extensions("now.txt")) })
    await loadLanguageExtension("now.txt")
    const switchedDispatch = vi.spyOn(switched, "dispatch")
    grammar.release()
    const loaded = await pending
    await waitFor(() => expect(active.state.facet(language)?.name).toBe("rust"))

    expect(languageExtensionFromPath("cached.rs")).toBe(loaded)
    expect(active.dom).toBe(editorDom)
    expect(active.state.doc).toBe(doc)
    expect(active.state.selection.eq(selection)).toBe(true)
    expect(active.scrollDOM.scrollTop).toBe(120)
    expect(changed).toHaveBeenCalledTimes(1)
    expect(limited.state.facet(language)).toBeNull()
    expect(switched.state.facet(language)).toBeNull()
    expect(retiredDispatch).not.toHaveBeenCalled()
    expect(switchedDispatch).not.toHaveBeenCalled()
    expect(undo(active)).toBe(true)
    expect(active.state.doc.toString()).toBe("fn main() {}")
    expect(makeView("cached.rs").state.facet(language)?.name).toBe("rust")
})

it("keeps cached grammar and invokes the callback without reconfiguring the view", async () => {
    await loadLanguageExtension("cached.ts")
    const loaded = vi.fn()
    let reconfigurations = 0
    const view = new EditorView({
        doc: "const value = 1",
        extensions: [
            ...languageExtensions("cached.ts", false, loaded),
            EditorView.updateListener.of(update => {
                reconfigurations += update.transactions.filter(transaction => transaction.reconfigured).length
            })
        ],
        parent: document.body
    })
    views.push(view)
    const doc = view.state.doc
    const syntax = view.state.facet(language)
    expect(syntax).not.toBeNull()
    await waitFor(() => expect(loaded).toHaveBeenCalledOnce())
    expect(loaded).toHaveBeenCalledWith(view)
    expect(reconfigurations).toBe(0)
    expect(view.state.doc).toBe(doc)
    expect(view.state.facet(language)).toBe(syntax)
})

it("installs grammar when an earlier configuration is mounted after loading finishes", async () => {
    const loaded = vi.fn()
    const extensions = languageExtensions("prepared.json", false, loaded)
    await loadLanguageExtension("prepared.json")
    let reconfigurations = 0
    const view = new EditorView({
        doc: '{"value": 1}',
        extensions: [extensions, EditorView.updateListener.of(update => {
            reconfigurations += update.transactions.filter(transaction => transaction.reconfigured).length
        })],
        parent: document.body
    })
    views.push(view)
    expect(view.state.facet(language)).toBeNull()
    await waitFor(() => expect(loaded).toHaveBeenCalledOnce())
    expect(view.state.facet(language)).not.toBeNull()
    expect(reconfigurations).toBe(1)
})

it("does not notify a view destroyed before its cached grammar callback", async () => {
    await loadLanguageExtension("retired.css")
    const loaded = vi.fn()
    const view = new EditorView({
        doc: "body { color: red }",
        extensions: languageExtensions("retired.css", false, loaded),
        parent: document.body
    })
    views.push(view)
    view.destroy()
    await Promise.resolve()
    await Promise.resolve()
    expect(loaded).not.toHaveBeenCalled()
})

it("does not retain cache or pending entries for arbitrary unsupported suffixes", async () => {
    const writes = vi.spyOn(Map.prototype, "set")
    try {
        for (let index = 0; index < 200; index++) {
            const path = `entry.unsupported_lifecycle_${index}`
            expect(await loadLanguageExtension(path)).toBeNull()
            expect(languageExtensionFromPath(path)).toBeNull()
        }
        expect(writes.mock.calls.filter(([key]) =>
            typeof key === "string" && key.startsWith("unsupported_lifecycle_")
        )).toHaveLength(0)
    } finally {
        writes.mockRestore()
    }
})

it("retires the pending load after an optional grammar rejects", async () => {
    const first = loadLanguageExtension("first.java")
    expect(loadLanguageExtension("concurrent.java")).toBe(first)
    // Vitest wraps a failed module factory; the original rejection is its cause.
    await expect(first).rejects.toMatchObject({ cause: { message: "Optional grammar unavailable" } })
    const retry = loadLanguageExtension("retry.java")
    expect(retry).not.toBe(first)
    await expect(retry).rejects.toMatchObject({ cause: { message: "Optional grammar unavailable" } })
    expect(languageExtensionFromPath("failed.java")).toBeNull()
})
