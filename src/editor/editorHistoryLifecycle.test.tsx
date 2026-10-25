import { act, cleanup, render, waitFor } from "@testing-library/react"
import { redo, redoDepth, undo, undoDepth } from "@codemirror/commands"
import type { EditorView } from "@codemirror/view"
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks"
import { afterEach, beforeEach, expect, it, vi } from "vitest"

import { useWorkspaceStore } from "@/state/workspaceStore"
import { WorkspaceResourcesBridge } from "@/workbench/WorkspaceResourcesBridge"
import { clearAll, documentGeneration, dropDocument, getDocument, reloadDocument, renameDocument, updateBuffer } from "./documentRegistry"
import { clearEditorViewStatesForTest } from "./editorViewState"
import { EditorPane } from "./EditorPane"
import { getView } from "./viewRegistry"

vi.mock("@/features/logs/userAction", () => ({ logUserAction: vi.fn(async () => undefined) }))
vi.mock("@/workbench/ExternalChangeResolver", () => ({ maybeInterceptSave: () => false }))
vi.mock("@/lib/actionFeedback", () => ({ showActionError: vi.fn(async () => undefined) }))

const A = "/history/a.txt", B = "/history/b.txt", C = "/history/renamed.txt"
const initialWorkspace = useWorkspaceStore.getState()
const ready = vi.fn()
const save = vi.fn()
let disk = "disk"
let readError = false
const tab = (path: string) => ({ path, name: path.split("/").at(-1)!, dirty: false, externallyModified: false })

function DocumentPanes() {
    const groups = useWorkspaceStore(state => state.groups)
    return <><WorkspaceResourcesBridge />{groups.map((group, index) => <div key={group.id ?? index}>
        {group.activePath && <EditorPane key={group.activePath} path={group.activePath} groupIndex={index} onReady={ready} />}
    </div>)}</>
}
async function viewFor(path = A, previous?: EditorView) {
    await waitFor(() => {
        expect(getView(path)).toBeDefined()
        if (previous) expect(getView(path)).not.toBe(previous)
    })
    return getView(path)!
}
function edit(view: EditorView, text = "edited ") {
    act(() => view.dispatch({ changes: { from: 0, insert: text } }))
}
async function switchAwayAndBack(previous: EditorView) {
    act(() => useWorkspaceStore.getState().setActiveTab(0, B))
    await viewFor(B)
    act(() => useWorkspaceStore.getState().setActiveTab(0, A))
    return viewFor(A, previous)
}

beforeEach(() => {
    disk = "disk"
    readError = false
    ready.mockReset()
    save.mockReset().mockResolvedValue(1)
    clearAll()
    clearEditorViewStatesForTest()
    mockIPC((command, payload) => {
        if (command === "open_file") {
            if (readError) throw new Error("fixture read failed")
            return { kind: "full", content: disk, size: disk.length, lineEnding: "lf" }
        }
        if (command === "save_file") return save(payload)
    })
    useWorkspaceStore.setState({ workspacePath: "/history", groups: [{ id: "left", tabs: [tab(A), tab(B)], activePath: A }], activeGroupIndex: 0, pendingReveal: null })
})
afterEach(() => {
    cleanup()
    clearAll()
    clearEditorViewStatesForTest()
    clearMocks()
    useWorkspaceStore.setState(initialWorkspace, true)
})

it("keeps undo and redo when switching away from an open file", async () => {
    render(<DocumentPanes />)
    const first = await viewFor()
    edit(first)
    expect(undoDepth(first.state)).toBe(1)
    const reopened = await switchAwayAndBack(first)
    expect(reopened.state.doc.toString()).toBe("edited disk")
    act(() => { expect(undo(reopened)).toBe(true) })
    expect(reopened.state.doc.toString()).toBe("disk")
    act(() => { expect(redo(reopened)).toBe(true) })
    expect(reopened.state.doc.toString()).toBe("edited disk")
})

it("keeps redo after Undo followed by split-and-move", async () => {
    render(<DocumentPanes />)
    const first = await viewFor()
    edit(first)
    act(() => { expect(undo(first)).toBe(true) })
    expect(redoDepth(first.state)).toBe(1)
    act(() => useWorkspaceStore.getState().splitAndMoveRight(0, A))
    const moved = await viewFor(A, first)
    expect(useWorkspaceStore.getState().groups[1].activePath).toBe(A)
    act(() => { expect(redo(moved)).toBe(true) })
    expect(moved.state.doc.toString()).toBe("edited disk")
})

it.each(["success", "failure"])("retains Undo across save %s and remount", async outcome => {
    render(<DocumentPanes />)
    const first = await viewFor()
    edit(first)
    if (outcome === "failure") save.mockRejectedValueOnce(new Error("fixture save failed"))
    const callback = ready.mock.calls.find(call => call[0] === first)![1] as () => void
    await act(async () => { callback(); await Promise.resolve() })
    await waitFor(() => expect(save).toHaveBeenCalledOnce())
    const reopened = await switchAwayAndBack(first)
    act(() => { expect(undo(reopened)).toBe(true) })
    expect(reopened.state.doc.toString()).toBe("disk")
})

it("clears history after an accepted explicit disk reload", async () => {
    const pane = render(<EditorPane path={A} groupIndex={0} onReady={ready} />)
    const first = await viewFor()
    edit(first)
    disk = "new disk"
    await act(async () => { await reloadDocument(A) })
    pane.unmount()
    render(<EditorPane path={A} groupIndex={0} onReady={ready} />)
    const fresh = await viewFor(A, first)
    expect(fresh.state.doc.toString()).toBe("new disk")
    expect(undoDepth(fresh.state)).toBe(0)
    expect(redoDepth(fresh.state)).toBe(0)
})

it.each(["rejected", "unchanged"])("preserves history after %s reload", async mode => {
    render(<DocumentPanes />)
    const first = await viewFor()
    const entry = await getDocument(A)
    edit(first)
    if (mode === "rejected") {
        readError = true
        await expect(reloadDocument(A)).rejects.toThrow("fixture read failed")
        readError = false
    } else expect(await reloadDocument(A, () => true, "reconcile")).toBe(entry)
    const reopened = await switchAwayAndBack(first)
    act(() => { expect(undo(reopened)).toBe(true) })
    expect(reopened.state.doc.toString()).toBe("disk")
})

it("does not resurrect history when the last tab closes and reopens", async () => {
    render(<DocumentPanes />)
    const first = await viewFor()
    const entry = await getDocument(A)
    edit(first)
    act(() => useWorkspaceStore.getState().closeTab(0, A))
    await viewFor(B)
    act(() => useWorkspaceStore.getState().openTab(A))
    const fresh = await viewFor(A, first)
    expect(await getDocument(A)).not.toBe(entry)
    expect(fresh.state.doc.toString()).toBe("disk")
    expect(undoDepth(fresh.state)).toBe(0)
})

it("rejects old cleanup after same-generation drop and reopen", async () => {
    const pane = render(<EditorPane path={A} groupIndex={0} onReady={ready} />)
    const first = await viewFor()
    edit(first)
    const generation = documentGeneration(A)
    dropDocument(A)
    disk = "reopened disk"
    const freshEntry = await getDocument(A)
    expect(documentGeneration(A)).toBe(generation)
    pane.unmount()
    expect(freshEntry.result).toMatchObject({ content: "reopened disk" })
    render(<EditorPane path={A} groupIndex={0} onReady={ready} />)
    const fresh = await viewFor(A, first)
    expect(fresh.state.doc.toString()).toBe("reopened disk")
    expect(undoDepth(fresh.state)).toBe(0)
})

it("keeps the latest same-path view's buffer and history when an older pane unmounts", async () => {
    const oldPane = render(<EditorPane path={A} groupIndex={0} onReady={ready} />)
    const oldView = await viewFor()
    edit(oldView, "old ")
    const latestPane = render(<EditorPane path={A} groupIndex={1} onReady={ready} />)
    const latest = await viewFor(A, oldView)
    edit(latest, "latest ")
    oldPane.unmount()
    expect((await getDocument(A)).result).toMatchObject({ content: "disk" })
    latestPane.unmount()
    render(<EditorPane path={A} groupIndex={1} onReady={ready} />)
    const reopened = await viewFor(A, latest)
    expect(reopened.state.doc.toString()).toBe("latest disk")
    act(() => { expect(undo(reopened)).toBe(true) })
    expect(reopened.state.doc.toString()).toBe("disk")
})

it("transfers live history with the same document entry on rename", async () => {
    render(<DocumentPanes />)
    const first = await viewFor()
    const entry = await getDocument(A)
    edit(first)
    act(() => {
        renameDocument(A, C, first.state.doc.toString())
        useWorkspaceStore.setState({ groups: [{ id: "left", tabs: [tab(C), tab(B)], activePath: C }] })
    })
    const renamed = await viewFor(C)
    expect(await getDocument(C)).toBe(entry)
    act(() => { expect(undo(renamed)).toBe(true) })
    expect(renamed.state.doc.toString()).toBe("disk")
})

it("does not reuse history after the registry buffer changes to different text", async () => {
    const pane = render(<EditorPane path={A} groupIndex={0} onReady={ready} />)
    const first = await viewFor()
    edit(first)
    pane.unmount()
    updateBuffer(A, "replacement", documentGeneration(A))
    render(<EditorPane path={A} groupIndex={0} onReady={ready} />)
    const fresh = await viewFor(A, first)
    expect(fresh.state.doc.toString()).toBe("replacement")
    expect(undoDepth(fresh.state)).toBe(0)
})

it("does not carry history into a new workspace with the same file path", async () => {
    render(<DocumentPanes />)
    const first = await viewFor()
    edit(first)
    act(() => useWorkspaceStore.setState({ workspacePath: "/other" }))
    const fresh = await viewFor(A, first)
    expect(fresh.state.doc.toString()).toBe("disk")
    expect(undoDepth(fresh.state)).toBe(0)
})
