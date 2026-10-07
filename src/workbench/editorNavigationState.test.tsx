import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import type { Editor } from "@tiptap/react"
import { EditorSelection } from "@codemirror/state"
import { getView } from "@/editor/viewRegistry"
import { clearAll } from "@/editor/documentRegistry"
import { clearEditorViewStatesForTest } from "@/editor/editorViewState"
import { clearMarkdownViewStatesForTest } from "@/editor/markdownViewState"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { useUiStore } from "@/state/uiStore"

const fixture = vi.hoisted(() => ({ rich: null as Editor | null }))
vi.mock("@tiptap/react", async importOriginal => {
    const original = await importOriginal<typeof import("@tiptap/react")>()
    return {
        ...original,
        useEditor: (...args: Parameters<typeof original.useEditor>) => {
            const editor = original.useEditor(...args)
            fixture.rich = editor
            return editor
        }
    }
})
vi.mock("@/lib/ipc", async importOriginal => ({
    ...await importOriginal<typeof import("@/lib/ipc")>(),
    openFileSnapshot: vi.fn(async () => ({
        result: { kind: "full", content: "## Heading\n\nFirst paragraph\n\nSecond paragraph", size: 45, lineEnding: "lf" },
        accept: vi.fn()
    })),
    logEvent: vi.fn(async () => {})
}))
vi.mock("@/features/logs/userAction", () => ({ logUserAction: vi.fn(async () => {}) }))
vi.mock("./TabBar", () => ({ TabBar: () => null }))
vi.mock("@/app/panels/HerdrTerminalPage", () => ({ HerdrTerminalPage: () => null }))
vi.mock("@/app/panels/PreviewPanel", () => ({ PreviewPanel: () => null }))

import { EditorArea } from "./EditorArea"

const originalWorkspace = useWorkspaceStore.getState()
const file = (path: string) => ({ path, name: path.split("/").at(-1)!, dirty: false, externallyModified: false })

beforeEach(() => {
    Range.prototype.getClientRects = () => [] as unknown as DOMRectList
    Range.prototype.getBoundingClientRect = () => new DOMRect()
    useUiStore.setState({ mode: "files" })
    useWorkspaceStore.setState({ ...originalWorkspace, workspacePath: "/w", pendingReveal: null,
        groups: [{ id: "navigation-test", tabs: [file("/w/a.txt"), file("/w/a.md"), file("/w/b.txt")], activePath: "/w/a.txt" }], activeGroupIndex: 0 }, true)
})
afterEach(() => {
    cleanup()
    clearAll()
    clearEditorViewStatesForTest()
    clearMarkdownViewStatesForTest()
    useWorkspaceStore.setState(originalWorkspace, true)
    fixture.rich = null
})

async function switchTo(path: string) {
    act(() => useWorkspaceStore.getState().setActiveTab(0, path))
    await waitFor(() => expect(getView(path)).toBeDefined())
}

it("restores a source selection through the actual A → B → A tab lifecycle", async () => {
    render(<EditorArea />)
    await switchTo("/w/a.txt")
    const first = getView("/w/a.txt")!
    act(() => first.dispatch({ selection: EditorSelection.range(12, 20) }))
    await switchTo("/w/b.txt")
    await switchTo("/w/a.txt")
    expect(getView("/w/a.txt")!.state.selection.main).toMatchObject({ anchor: 12, head: 20 })
})

it.each([[12, 20], [20, 12]])("restores a Markdown document selection (%i → %i) through A → B → A", async (anchor, head) => {
    render(<EditorArea />)
    await switchTo("/w/a.md")
    await screen.findByRole("textbox", { name: "Markdown rich text editor" })
    const first = fixture.rich!
    act(() => { first.commands.setTextSelection({ from: anchor, to: head }) })
    await switchTo("/w/b.txt")
    await switchTo("/w/a.md")
    await screen.findByRole("textbox", { name: "Markdown rich text editor" })
    expect(fixture.rich!.state.selection).toMatchObject({ anchor, head })
})

it("restores the rich viewport's recorded scroll offsets after switching tabs", async () => {
    render(<EditorArea />)
    await switchTo("/w/a.md")
    const rich = await screen.findByRole("textbox", { name: "Markdown rich text editor" })
    const viewport = rich.closest('[data-slot="scroll-area-viewport"]')!
    viewport.scrollTop = 350
    viewport.scrollLeft = 12
    fireEvent.scroll(viewport)
    await switchTo("/w/b.txt")
    await switchTo("/w/a.md")
    const restored = (await screen.findByRole("textbox", { name: "Markdown rich text editor" })).closest('[data-slot="scroll-area-viewport"]')!
    // jsdom verifies the lifecycle wiring; real layout/clamping needs browser acceptance.
    expect(restored.scrollTop).toBe(350)
    expect(restored.scrollLeft).toBe(12)
})

it("isolates Markdown navigation by workspace and restores it on a workspace round trip", async () => {
    render(<EditorArea />)
    await switchTo("/w/a.md")
    await screen.findByRole("textbox", { name: "Markdown rich text editor" })
    act(() => { fixture.rich!.commands.setTextSelection({ from: 12, to: 20 }) })
    act(() => { useWorkspaceStore.getState().setWorkspace("/"); useWorkspaceStore.getState().openTab("/w/a.md", 0) })
    await screen.findByRole("textbox", { name: "Markdown rich text editor" })
    await waitFor(() => expect(fixture.rich!.state.selection).toMatchObject({ anchor: 1, head: 1 }))
    act(() => { useWorkspaceStore.getState().setWorkspace("/w"); useWorkspaceStore.getState().openTab("/w/a.md", 0) })
    await waitFor(() => expect(fixture.rich!.state.selection).toMatchObject({ anchor: 12, head: 20 }))
})

it("keeps Markdown source mode when returning to its tab", async () => {
    render(<EditorArea />)
    await switchTo("/w/a.md")
    await screen.findByRole("textbox", { name: "Markdown rich text editor" })
    fireEvent.click(screen.getByRole("radio", { name: "Source" }))
    await switchTo("/w/b.txt")
    await switchTo("/w/a.md")
    expect(screen.getByRole("radio", { name: "Source" })).toHaveAttribute("data-state", "on")
})

it("retains the cursor and viewport after editing a Markdown document and returning", async () => {
    render(<EditorArea />)
    await switchTo("/w/a.md")
    const rich = await screen.findByRole("textbox", { name: "Markdown rich text editor" })
    const viewport = rich.closest('[data-slot="scroll-area-viewport"]')!
    viewport.scrollTop = 350
    fireEvent.scroll(viewport)
    act(() => { fixture.rich!.commands.setTextSelection(15); fixture.rich!.commands.insertContent("typed ") })
    const { anchor, head } = fixture.rich!.state.selection
    await switchTo("/w/b.txt")
    await switchTo("/w/a.md")
    const restored = await screen.findByRole("textbox", { name: "Markdown rich text editor" })
    expect(restored.textContent).toContain("typed")
    expect(fixture.rich!.state.selection).toMatchObject({ anchor, head })
    expect(restored.closest('[data-slot="scroll-area-viewport"]')!.scrollTop).toBe(350)
})

it("does not serialize an unchanged Markdown document when only the selection toolbar changes", async () => {
    render(<EditorArea />)
    await switchTo("/w/a.md")
    await screen.findByRole("textbox", { name: "Markdown rich text editor" })
    const serialize = vi.spyOn(fixture.rich!, "getMarkdown")
    for (let i = 0; i < 10; i++) {
        act(() => { fixture.rich!.commands.setTextSelection(i % 2 ? 15 : 3) })
    }
    expect(serialize).not.toHaveBeenCalled()
    serialize.mockRestore()
})
