import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { EditorState } from "@codemirror/state"
import { EditorView } from "@codemirror/view"
import { history, undo } from "@codemirror/commands"
import { useEffect, useRef } from "react"

const fixture = vi.hoisted(() => ({ content: "# Hello\n\nWorld", save: vi.fn(), view: null as EditorView | null }))
vi.mock("./EditorPane", () => ({
    EditorPane: ({ onReady }: { onReady: (view: EditorView, save: () => void, editable: boolean) => void }) => {
        const ref = useRef<HTMLDivElement>(null)
        useEffect(() => {
            const view = new EditorView({ state: EditorState.create({ doc: fixture.content, extensions: [history()] }), parent: ref.current! })
            fixture.view = view
            onReady(view, fixture.save, true)
            return () => { view.destroy(); fixture.view = null }
        }, [onReady])
        return <div ref={ref} data-testid="source-owner" />
    }
}))
vi.mock("@/workbench/MarkdownPreview", () => ({ MarkdownPreview: () => <div>Safe reading preview</div> }))
import { RichMarkdownEditor } from "./RichMarkdownEditor"

beforeEach(() => {
    Range.prototype.getClientRects = () => [] as unknown as DOMRectList
    Range.prototype.getBoundingClientRect = () => new DOMRect()
    fixture.content = "# Hello\n\nWorld"; fixture.save.mockReset() })
afterEach(cleanup)

it("keeps the original bytes and CM identity through document/source switches", async () => {
    render(<RichMarkdownEditor path="/w/a.md" groupIndex={0} />)
    await screen.findByRole("textbox", { name: "Markdown rich text editor" })
    const view = fixture.view
    fireEvent.click(screen.getByRole("radio", { name: "Source" }))
    fireEvent.click(screen.getByRole("radio", { name: "Document" }))
    expect(fixture.view).toBe(view)
    expect(view?.state.doc.toString()).toBe(fixture.content)
})

it("rich edits and source undo share one document; Mod+S delegates to its real save path", async () => {
    render(<RichMarkdownEditor path="/w/a.md" groupIndex={0} />)
    const rich = await screen.findByRole("textbox", { name: "Markdown rich text editor" })
    fireEvent.click(screen.getByRole("button", { name: "Bullet list" }))
    await waitFor(() => expect(fixture.view?.state.doc.toString()).not.toBe(fixture.content))
    act(() => { undo(fixture.view!) })
    await waitFor(() => expect(fixture.view?.state.doc.toString()).toBe(fixture.content))
    fireEvent.keyDown(rich, { key: "s", ctrlKey: true })
    expect(fixture.save).toHaveBeenCalledOnce()
})

it("unsupported source is read only in document mode and remains byte-for-byte intact", async () => {
    fixture.content = "---\ntitle: untouched\n---\n\n<div>raw</div>"
    render(<RichMarkdownEditor path="/w/a.md" groupIndex={0} />)
    await screen.findByText("Safe reading preview")
    expect(screen.queryByRole("textbox", { name: "Markdown rich text editor" })).toBeNull()
    expect(fixture.view?.state.doc.toString()).toBe(fixture.content)
})
