import { act, cleanup, render, screen } from "@testing-library/react"
import { afterEach, expect, test, vi } from "vitest"
import { EditorState } from "@codemirror/state"
import { EditorView } from "@codemirror/view"
import type { OpenFileResult } from "../lib/types"
import { registerView, unregisterView } from "../editor/viewRegistry"
import { getDocument } from "../editor/documentRegistry"
import { MarkdownPreview, __renderMarkdownCallCount } from "./MarkdownPreview"
import { useWorkspaceStore } from "../state/workspaceStore"

let result: OpenFileResult
let generation = 0
vi.mock("../editor/documentRegistry", () => ({
    documentGeneration: vi.fn(() => generation),
    getDocument: vi.fn(async () => ({ result }))
}))
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }))

const path = "/w/subscribed.md"
const views: EditorView[] = []
function makeView(content: string) {
    const view = new EditorView({ state: EditorState.create({ doc: content }) })
    views.push(view)
    return view
}

afterEach(() => {
    cleanup()
    unregisterView(path)
    for (const view of views.splice(0)) view.destroy()
    generation = 0
    vi.restoreAllMocks()
})

test("late registration and document edits update Markdown without polling or advancing timers", async () => {
    const interval = vi.spyOn(globalThis, "setInterval")
    result = { kind: "full", content: "# Disk", size: 6, lineEnding: "lf" }
    const rendered = render(<MarkdownPreview sourcePath={path} />)
    await screen.findByRole("heading", { name: "Disk" })
    const view = makeView("# Live")
    await act(async () => registerView(path, view))
    expect(screen.getByRole("heading", { name: "Live" })).toBeInTheDocument()
    await act(async () => view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: "# Edited" } }))
    expect(screen.getByRole("heading", { name: "Edited" })).toBeInTheDocument()
    const renders = __renderMarkdownCallCount()
    await act(async () => view.dispatch({ selection: { anchor: 0 } }))
    expect(__renderMarkdownCallCount()).toBe(renders)
    expect(interval.mock.calls.some(([, delay]) => delay === 400)).toBe(false)
    rendered.unmount()
    expect(view.state.facet(EditorView.updateListener)).toHaveLength(0)
})

test("external reload remounts re-check full → tooLarge → full and replace same-kind content", async () => {
    result = { kind: "full", content: "# Small", size: 7, lineEnding: "lf" }
    const first = makeView("# Small")
    registerView(path, first)
    render(<MarkdownPreview sourcePath={path} />)
    await screen.findByRole("heading", { name: "Small" })
    await act(async () => {
        result = { kind: "tooLarge", size: 20_000_000 }
        unregisterView(path, first)
    })
    expect(screen.getByTestId("markdown-preview-downgrade")).toBeInTheDocument()
    const second = makeView("# Restored")
    await act(async () => {
        result = { kind: "full", content: "# Restored", size: 10, lineEnding: "lf" }
        registerView(path, second)
    })
    expect(screen.getByRole("heading", { name: "Restored" })).toBeInTheDocument()
    await act(async () => {
        result = { kind: "full", content: "# Reloaded", size: 10, lineEnding: "lf" }
        unregisterView(path, second)
        registerView(path, makeView("# Reloaded"))
    })
    expect(screen.getByRole("heading", { name: "Reloaded" })).toBeInTheDocument()
})

test("path changes detach the old view and ignore its pending document refresh", async () => {
    result = { kind: "full", content: "# First", size: 7, lineEnding: "lf" }
    const first = makeView("# First")
    registerView(path, first)
    const rendered = render(<MarkdownPreview sourcePath={path} />)
    await screen.findByRole("heading", { name: "First" })
    let resolveRead!: (entry: { result: OpenFileResult }) => void
    vi.mocked(getDocument).mockImplementationOnce(() => new Promise((resolve) => { resolveRead = resolve }))
    await act(async () => first.dispatch({ changes: { from: 0, to: first.state.doc.length, insert: "# Stale" } }))
    result = { kind: "full", content: "# Second", size: 8, lineEnding: "lf" }
    rendered.rerender(<MarkdownPreview sourcePath="/w/second.md" />)
    await screen.findByRole("heading", { name: "Second" })
    expect(first.state.facet(EditorView.updateListener)).toHaveLength(0)
    await act(async () => resolveRead({ result: { kind: "full", content: "# Stale", size: 7, lineEnding: "lf" } }))
    expect(screen.getByRole("heading", { name: "Second" })).toBeInTheDocument()
    expect(screen.queryByRole("heading", { name: "Stale" })).not.toBeInTheDocument()
})

test("inactive source reloads update content and full ↔ tooLarge kind without a view", async () => {
    result = { kind: "full", content: "# Inactive", size: 10, lineEnding: "lf" }
    const rendered = render(<MarkdownPreview sourcePath={path} />)
    await screen.findByRole("heading", { name: "Inactive" })
    const reads = vi.mocked(getDocument).mock.calls.length
    await act(async () => useWorkspaceStore.setState({ pendingReveal: null }))
    expect(getDocument).toHaveBeenCalledTimes(reads)
    await act(async () => {
        result = { kind: "tooLarge", size: 20_000_000 }
        generation++
        useWorkspaceStore.setState({ pendingReveal: null })
    })
    expect(screen.getByTestId("markdown-preview-downgrade")).toBeInTheDocument()
    await act(async () => {
        result = { kind: "full", content: "# Smaller", size: 9, lineEnding: "lf" }
        generation++
        useWorkspaceStore.setState({ pendingReveal: null })
    })
    expect(screen.getByRole("heading", { name: "Smaller" })).toBeInTheDocument()
    await act(async () => {
        result = { kind: "full", content: "# Reloaded inactive", size: 19, lineEnding: "lf" }
        generation++
        useWorkspaceStore.setState({ pendingReveal: null })
    })
    expect(screen.getByRole("heading", { name: "Reloaded inactive" })).toBeInTheDocument()
    rendered.unmount()
    const readsAfterUnmount = vi.mocked(getDocument).mock.calls.length
    generation++
    useWorkspaceStore.setState({ pendingReveal: null })
    expect(getDocument).toHaveBeenCalledTimes(readsAfterUnmount)
})
