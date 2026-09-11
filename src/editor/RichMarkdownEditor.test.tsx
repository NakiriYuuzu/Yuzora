import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { EditorState } from "@codemirror/state"
import { EditorView } from "@codemirror/view"
import { history, undo } from "@codemirror/commands"
import { useEffect, useRef } from "react"

const fixture = vi.hoisted(() => ({ content: "# Hello\n\nWorld", save: vi.fn(), view: null as EditorView | null, editable: true }))
vi.mock("./EditorPane", () => ({
    EditorPane: ({ onReady }: { onReady: (view: EditorView, save: () => void, editable: boolean) => void }) => {
        const ref = useRef<HTMLDivElement>(null)
        useEffect(() => {
            const view = new EditorView({ state: EditorState.create({ doc: fixture.content, extensions: [history()] }), parent: ref.current! })
            fixture.view = view
            onReady(view, fixture.save, fixture.editable)
            return () => { view.destroy(); fixture.view = null }
        }, [onReady])
        return <div ref={ref} data-testid="source-owner" />
    }
}))
vi.mock("@/workbench/MarkdownDocumentPreview", () => ({ MarkdownDocumentPreview: ({ content }: { content: string }) => <div data-testid="document-preview">Safe reading preview<pre>{content}</pre></div> }))
import { RichMarkdownEditor } from "./RichMarkdownEditor"

beforeEach(() => {
    Range.prototype.getClientRects = () => [] as unknown as DOMRectList
    Range.prototype.getBoundingClientRect = () => new DOMRect()
    fixture.content = "# Hello\n\nWorld"; fixture.editable = true; fixture.save.mockReset() })
afterEach(() => { cleanup(); vi.restoreAllMocks(); vi.unstubAllGlobals() })

it("keeps the original bytes and CM identity through document/source switches", async () => {
    render(<RichMarkdownEditor path="/w/a.md" groupIndex={0} />)
    await screen.findByRole("textbox", { name: "Markdown rich text editor" })
    const view = fixture.view
    fireEvent.click(screen.getByRole("radio", { name: "Source" }))
    fireEvent.click(screen.getByRole("radio", { name: "Document" }))
    expect(fixture.view).toBe(view)
    expect(view?.state.doc.toString()).toBe(fixture.content)
})

it("reuses one document listener across repeated mode switches and still observes source changes", async () => {
    render(<RichMarkdownEditor path="/w/a.md" groupIndex={0} />)
    await screen.findByRole("textbox", { name: "Markdown rich text editor" })
    const count = fixture.view!.state.facet(EditorView.updateListener).length
    for (let i = 0; i < 8; i++) {
        fireEvent.click(screen.getByRole("radio", { name: "Source" }))
        fireEvent.click(screen.getByRole("radio", { name: "Document" }))
        expect(fixture.view!.state.facet(EditorView.updateListener)).toHaveLength(count)
    }
    act(() => { fixture.view!.dispatch({ changes: { from: 0, to: fixture.view!.state.doc.length, insert: "# Still connected" } }) })
    expect((await screen.findByRole("textbox", { name: "Markdown rich text editor" })).textContent).toBe("Still connected")
    expect(fixture.view!.state.facet(EditorView.updateListener)).toHaveLength(count)
})

it("rich edits and source undo share one document; Mod+S delegates to its real save path", async () => {
    render(<RichMarkdownEditor path="/w/a.md" groupIndex={0} />)
    await screen.findByRole("textbox", { name: "Markdown rich text editor" })
    fireEvent.click(screen.getByRole("button", { name: "Bullet list" }))
    await waitFor(() => expect(fixture.view?.state.doc.toString()).not.toBe(fixture.content))
    act(() => { undo(fixture.view!) })
    await waitFor(() => expect(fixture.view?.state.doc.toString()).toBe(fixture.content))
    fireEvent.keyDown(await screen.findByRole("textbox", { name: "Markdown rich text editor" }), { key: "s", ctrlKey: true })
    expect(fixture.save).toHaveBeenCalledOnce()
})

it("unsupported source is read only in document mode and remains byte-for-byte intact", async () => {
    fixture.content = "---\ntitle: untouched\n---\n\n<div>raw</div>"
    render(<RichMarkdownEditor path="/w/a.md" groupIndex={0} />)
    await screen.findByText("Safe reading preview")
    expect(screen.queryByRole("textbox", { name: "Markdown rich text editor" })).toBeNull()
    expect(fixture.view?.state.doc.toString()).toBe(fixture.content)
})

it.each([
    "# Ordinary\n\n![Keep image](figure.png)\n\n# Tail",
    "---\ntitle: untouched\n---\n\n# Ordinary\n\n# Tail",
    "# Ordinary\n\n<div>raw</div>\n\n# Tail",
    "[foo\\]bar]: https://example.com\n\n# Ordinary\n\n[text][foo\\]bar]\n",
])("renders the complete document when any block requires source preservation", async (content) => {
    fixture.content = content
    render(<RichMarkdownEditor path="/w/a.md" groupIndex={0} />)
    const preview = await screen.findByTestId("document-preview")
    expect(preview.querySelector("pre")!.textContent).toBe(content)
    expect(screen.queryByRole("textbox", { name: "Markdown rich text editor" })).toBeNull()
    expect(screen.queryByRole("toolbar")).toBeNull()
    expect(screen.queryByRole("navigation")).toBeNull()
    expect(screen.queryByRole("button", { name: /section/i })).toBeNull()
    expect(fixture.view?.state.doc.toString()).toBe(content)
})

it("edits a complete safe document beyond the former section limit", async () => {
    fixture.content = "# Start\n\n" + "ordinary text ".repeat(2600) + "\n\n# Included tail\n"
    render(<RichMarkdownEditor path="/w/a.md" groupIndex={0} />)
    const rich = await screen.findByRole("textbox", { name: "Markdown rich text editor" })
    expect(rich.textContent).toContain("Included tail")
    expect(screen.queryByRole("navigation")).toBeNull()
    fireEvent.click(screen.getByRole("button", { name: "Bullet list" }))
    await waitFor(() => expect(fixture.view?.state.doc.toString()).not.toBe(fixture.content))
    expect(fixture.view?.state.doc.toString()).toContain("Included tail")
    act(() => { undo(fixture.view!) })
    await waitFor(() => expect(fixture.view?.state.doc.toString()).toBe(fixture.content))
})

it.each([
    "# Start\n\n" + "ordinary paragraph\n\n".repeat(8000) + "# Entire tail",
    "```\n" + "very large code\n".repeat(10000) + "```\n\n# Entire tail",
    "# Start\n\n" + "漢".repeat(44000) + "\n\n# Entire tail",
])("renders all oversized content without mounting a rich editor or section index worker", async (content) => {
    const Worker = vi.fn()
    vi.stubGlobal("Worker", Worker)
    fixture.content = content
    render(<RichMarkdownEditor path="/w/a.md" groupIndex={0} />)
    const preview = await screen.findByTestId("document-preview")
    expect(preview.querySelector("pre")!.textContent).toBe(content)
    expect(screen.queryByRole("textbox", { name: "Markdown rich text editor" })).toBeNull()
    expect(screen.queryByRole("navigation")).toBeNull()
    expect(Worker).not.toHaveBeenCalled()
    expect(fixture.view?.state.doc.toString()).toBe(content)
})

it("uses only the latest of rapid external source changes", async () => {
    render(<RichMarkdownEditor path="/w/a.md" groupIndex={0} />)
    await screen.findByRole("textbox", { name: "Markdown rich text editor" })
    act(() => {
        fixture.view!.dispatch({ changes: { from: 0, to: fixture.view!.state.doc.length, insert: "old" } })
        fixture.view!.dispatch({ changes: { from: 0, to: 3, insert: "# Latest" } })
    })
    expect(screen.queryByRole("textbox", { name: "Markdown rich text editor" })).toBeNull()
    const rich = await screen.findByRole("textbox", { name: "Markdown rich text editor" })
    expect(rich.textContent).toBe("Latest")
    expect(fixture.view?.state.doc.toString()).toBe("# Latest")
})

it("refreshes complete reading content after source edits and returns to rich editing when safe", async () => {
    fixture.content = "# Original\n\n![Keep](image.png)"
    render(<RichMarkdownEditor path="/w/a.md" groupIndex={0} />)
    await screen.findByTestId("document-preview")
    fireEvent.click(screen.getByRole("radio", { name: "Source" }))
    expect(screen.queryByTestId("document-preview")).toBeNull()
    const latest = "# Changed\n\n<div>Entire HTML</div>\n\nTail"
    act(() => { fixture.view!.dispatch({ changes: { from: 0, to: fixture.view!.state.doc.length, insert: latest } }) })
    fireEvent.click(screen.getByRole("radio", { name: "Document" }))
    expect(screen.getByTestId("document-preview").querySelector("pre")!.textContent).toBe(latest)
    fireEvent.click(screen.getByRole("radio", { name: "Source" }))
    act(() => { fixture.view!.dispatch({ changes: { from: 0, to: fixture.view!.state.doc.length, insert: "# Safe now" } }) })
    fireEvent.click(screen.getByRole("radio", { name: "Document" }))
    expect((await screen.findByRole("textbox", { name: "Markdown rich text editor" })).textContent).toBe("Safe now")
})

it("respects the source owner's read-only guard and renders its entire content", async () => {
    fixture.editable = false
    fixture.content = "# Read only\n\n" + "ordinary text\n\n".repeat(100) + "# Last heading"
    render(<RichMarkdownEditor path="/w/a.md" groupIndex={0} />)
    const preview = await screen.findByTestId("document-preview")
    expect(preview.querySelector("pre")!.textContent).toBe(fixture.content)
    expect(screen.queryByRole("textbox", { name: "Markdown rich text editor" })).toBeNull()
    expect(screen.queryByRole("toolbar")).toBeNull()
    expect(fixture.view?.state.doc.toString()).toBe(fixture.content)
})

it("replaces the owner when the file path changes without exposing the previous document", async () => {
    const { rerender } = render(<RichMarkdownEditor path="/w/a.md" groupIndex={0} />)
    await screen.findByRole("textbox", { name: "Markdown rich text editor" })
    const oldView = fixture.view!
    fixture.content = "# Different file\n\n![Image](image.png)"
    rerender(<RichMarkdownEditor path="/w/b.md" groupIndex={0} />)
    const preview = await screen.findByTestId("document-preview")
    expect(preview.querySelector("pre")!.textContent).toBe(fixture.content)
    expect(fixture.view).not.toBe(oldView)
    expect(screen.queryByRole("textbox", { name: "Markdown rich text editor" })).toBeNull()
})
