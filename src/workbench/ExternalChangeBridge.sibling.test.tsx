import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { mockIPC, clearMocks } from "@tauri-apps/api/mocks"
import { undo, undoDepth } from "@codemirror/commands"
import type { EditorView } from "@codemirror/view"
import { EditorPane } from "../editor/EditorPane"
import { clearAll, documentGeneration, dropDocument, getDocument, renameDocument, reloadDocument, saveDocumentContent } from "../editor/documentRegistry"
import type { OpenFileResult } from "../lib/types"
import { useUiStore } from "../state/uiStore"
import { ExternalChangeResolver, maybeInterceptSave } from "./ExternalChangeResolver"
import { getView } from "../editor/viewRegistry"
import { useWorkspaceStore } from "../state/workspaceStore"
import { useFileTreeStore } from "../state/fileTreeStore"
import { recentlySaved } from "../lib/saveSuppress"
import { ExternalChangeBridge } from "./ExternalChangeBridge"

const path = "/w/target/notes.txt"
let listener: (event: { payload: { workspaceRoot: string; paths: string[] } }) => void
vi.mock("@tauri-apps/api/event", () => ({
    listen: vi.fn(async (_event, callback) => { listener = callback; return () => {} })
}))
vi.mock("@/features/logs/userAction", () => ({ logUserAction: vi.fn(async () => {}) }))
let disk = "before\n"
let save: () => void
const initial = useWorkspaceStore.getState()
const onReady = (_view: EditorView, onSave: () => void) => { save = onSave }

// The same generation-keyed EditorPane contract used by EditorArea.
function Document() {
    useWorkspaceStore(state => state.groups)
    return <EditorPane key={documentGeneration(path)} path={path} groupIndex={0}
        onReady={onReady} />
}

beforeEach(() => {
    clearAll()
    disk = "before\n"
    useUiStore.getState().closeResolver()
    useWorkspaceStore.setState({ ...initial, workspacePath: "/w" }, true)
    useWorkspaceStore.getState().openTab(path)
    useFileTreeStore.setState({ trees: {} })
    Range.prototype.getClientRects = () => [] as unknown as DOMRectList
    Range.prototype.getBoundingClientRect = () => new DOMRect()
    mockIPC((command, args) => {
        if (command === "open_file") {
            return { kind: "full", content: disk, size: new TextEncoder().encode(disk).length, lineEnding: "lf" }
        }
        if (command === "save_file") {
            disk = (args as { content: string }).content
            return 1
        }
        if (command === "list_dir") return []
    })
})
afterEach(() => { cleanup(); clearAll(); clearMocks(); vi.restoreAllMocks() })

async function mount() {
    render(<><ExternalChangeBridge /><Document /></>)
    await waitFor(() => expect(getView(path)).toBeDefined())
    return getView(path)!
}
function tab() { return useWorkspaceStore.getState().groups[0].tabs[0] }
async function siblingEvent() {
    // Explicitly cross the 750ms own-save suppression window without waiting.
    const now = Date.now()
    const clock = vi.spyOn(Date, "now").mockReturnValue(now + 1000)
    expect(recentlySaved.snapshot().has(path)).toBe(false)
    await act(async () => { listener({ payload: { workspaceRoot: "/w", paths: ["/w/target"] } }) })
    clock.mockRestore()
}
function edit(view: EditorView) {
    act(() => view.dispatch({ changes: { from: 0, insert: "saved edit\n" } }))
}

it("a saved edit survives a sibling-only event with the same EditorView and undo depth", async () => {
    const view = await mount()
    edit(view)
    await act(async () => save())
    await waitFor(() => expect(tab().dirty).toBe(false))
    const depth = undoDepth(view.state)
    expect(depth).toBeGreaterThan(0)
    const generation = documentGeneration(path)
    await siblingEvent()
    expect(getView(path)).toBe(view)
    expect(documentGeneration(path)).toBe(generation)
    expect(undoDepth(view.state)).toBe(depth)
    act(() => { expect(undo(view)).toBe(true) })
    expect(view.state.doc.toString()).toBe("before\n")
})

it("an unchanged disk snapshot does not flag a dirty descendant as conflicting", async () => {
    const view = await mount()
    edit(view)
    await siblingEvent()
    expect(tab().dirty).toBe(true)
    expect(tab().externallyModified).toBe(false)
    expect(getView(path)).toBe(view)
})

it.each([false, true])("a changed disk read survives continued typing and intercepts the next save (line ending changed: %s)", async (changeLineEnding) => {
    const view = await mount()
    edit(view)
    const originalTab = tab()
    let finish!: (result: OpenFileResult) => void
    const write = vi.fn()
    mockIPC(command => {
        if (command === "open_file") return new Promise<OpenFileResult>(resolve => { finish = resolve })
        if (command === "save_file") write()
        if (command === "list_dir") return []
    })
    await siblingEvent()
    edit(view)
    expect(view.state.doc.toString()).toBe("saved edit\nsaved edit\nbefore\n")
    if (changeLineEnding) {
        act(() => useWorkspaceStore.getState().setLineEnding(path, "crlf"))
        expect(tab()).not.toBe(originalTab)
    }
    await act(async () => finish({ kind: "full", content: "external", size: 8, lineEnding: "lf" }))
    expect(tab().externallyModified).toBe(true)
    await act(async () => save())
    expect(useUiStore.getState().resolverPath).toBe(path)
    expect(write).not.toHaveBeenCalled()
    expect(getView(path)).toBe(view)
})

it.each(["save", "reload"])("a %s during comparison conservatively flags the still-open document", async lifecycle => {
    const view = await mount()
    edit(view)
    let finish!: (result: OpenFileResult) => void
    mockIPC(command => command === "open_file"
        ? new Promise<OpenFileResult>(resolve => { finish = resolve }) : [])
    await siblingEvent()
    mockIPC(command => command === "open_file"
        ? { kind: "full", content: "new baseline", size: 12, lineEnding: "lf" } : 1)
    await act(async () => {
        if (lifecycle === "save") {
            await saveDocumentContent(path, view.state.doc.toString())
            useWorkspaceStore.getState().markDirty(path, false)
        } else {
            await reloadDocument(path)
            useWorkspaceStore.getState().markDirty(path, false)
        }
        finish({ kind: "full", content: "external", size: 8, lineEnding: "lf" })
    })
    expect(tab().externallyModified).toBe(true)
    expect(useWorkspaceStore.getState().groups[0].tabs[0].dirty).toBe(false)
})

it.each([false, true])("a pending comparison follows the same registry entry on rename (read fails: %s)", async fails => {
    const entry = await getDocument(path)
    useWorkspaceStore.getState().markDirty(path, true)
    render(<ExternalChangeBridge />)
    let finish!: (result: OpenFileResult) => void
    let reject!: (error: Error) => void
    mockIPC(command => command === "open_file"
        ? new Promise<OpenFileResult>((resolve, fail) => { finish = resolve; reject = fail }) : [])
    await siblingEvent()
    const renamedPath = "/w/target/renamed.txt"
    act(() => {
        renameDocument(path, renamedPath, "my unsaved buffer")
        useWorkspaceStore.getState().updateTabPath(path, renamedPath)
    })
    expect(await getDocument(renamedPath)).toBe(entry)
    await act(async () => {
        if (fails) reject(new Error("old path gone"))
        else finish({ kind: "full", content: "external", size: 8, lineEnding: "lf" })
    })
    expect(tab().path).toBe(renamedPath)
    expect(tab().externallyModified).toBe(true)
    expect(maybeInterceptSave(renamedPath)).toBe(true)
})

it.each(["close", "reopen", "workspace"])("pending comparisons handle %s without flagging unrelated documents", async lifecycle => {
    await getDocument(path)
    useWorkspaceStore.getState().markDirty(path, true)
    render(<ExternalChangeBridge />)
    let finish!: (result: OpenFileResult) => void
    mockIPC(command => command === "open_file"
        ? new Promise<OpenFileResult>(resolve => { finish = resolve }) : [])
    await siblingEvent()
    act(() => {
        dropDocument(path)
        useWorkspaceStore.getState().closeTab(0, path)
        if (lifecycle === "workspace") {
            clearAll()
            useWorkspaceStore.setState({ workspacePath: "/other" })
        }
        useWorkspaceStore.getState().openTab(lifecycle === "close" ? "/w/unrelated.txt" : path)
    })
    await act(async () => finish({ kind: "full", content: "external", size: 8, lineEnding: "lf" }))
    expect(tab().externallyModified).toBe(lifecycle === "reopen")
})

it.each(["binary", "tooLarge"] as const)("explicit take-disk replaces the dirty pane when %s disk returns to its baseline", async kind => {
    const view = await mount()
    edit(view)
    const generation = documentGeneration(path)
    mockIPC(command => command === "open_file" ? { kind, size: 10 } : [])
    act(() => {
        useWorkspaceStore.getState().markExternallyModified(path, true)
        useUiStore.getState().openResolver(path)
    })
    render(<ExternalChangeResolver />)
    const takeDisk = await screen.findByRole("button", { name: "採用磁碟版（重新載入）" })
    mockIPC(command => command === "open_file"
        ? { kind: "full", content: disk, size: disk.length, lineEnding: "lf" } : [])
    fireEvent.click(takeDisk)
    await waitFor(() => expect(useUiStore.getState().resolverPath).toBe(null))
    await waitFor(() => expect(getView(path)).not.toBe(view))
    expect(documentGeneration(path)).toBe(generation + 1)
    expect(getView(path)!.state.doc.toString()).toBe("before\n")
    expect(tab().dirty).toBe(false)
    expect(tab().externallyModified).toBe(false)
})

it("normal text take-disk replaces a dirty buffer even when disk still equals the baseline", async () => {
    const view = await mount()
    edit(view)
    act(() => {
        useWorkspaceStore.getState().markExternallyModified(path, true)
        useUiStore.getState().openResolver(path)
    })
    render(<ExternalChangeResolver />)
    fireEvent.click(await screen.findByRole("button", { name: "全部採用磁碟版" }))
    fireEvent.click(screen.getByRole("button", { name: "解決並存檔" }))
    await waitFor(() => expect(useUiStore.getState().resolverPath).toBe(null))
    expect(getView(path)!.state.doc.toString()).toBe("before\n")
    expect(tab().dirty).toBe(false)
    expect(tab().externallyModified).toBe(false)
})

it.each([false, true])("a real descendant change still reloads or conflicts (dirty: %s)", async dirty => {
    const view = await mount()
    if (dirty) edit(view)
    disk = "external\n"
    await siblingEvent()
    if (dirty) {
        expect(tab().externallyModified).toBe(true)
        expect(getView(path)).toBe(view)
        expect(view.state.doc.toString()).toContain("saved edit")
    } else {
        await waitFor(() => expect(getView(path)).not.toBe(view))
        expect(getView(path)!.state.doc.toString()).toBe(disk)
    }
})
