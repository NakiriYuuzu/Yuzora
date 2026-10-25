import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks"
import { afterEach, beforeEach, expect, it, vi } from "vitest"

import { clearAll, documentGeneration, getDocument, updateBuffer } from "@/editor/documentRegistry"
import { useConfirmDialogStore } from "@/state/confirmDialogStore"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { EditorArea } from "./EditorArea"
import { WorkspaceResourcesBridge } from "./WorkspaceResourcesBridge"

vi.mock("./TabBar", () => ({ TabBar: () => null }))
vi.mock("../editor/EditorPane", () => ({ EditorPane: () => null }))
vi.mock("@/app/panels/HerdrTerminalPage", () => ({ HerdrTerminalPage: () => null }))
vi.mock("@/app/panels/PreviewPanel", () => ({ PreviewPanel: () => null }))
vi.mock("./MarkdownPreview", () => ({ isMarkdownPath: () => false, MarkdownPreview: () => null }))
vi.mock("./ImageView", () => ({ isImagePath: () => false, ImageView: () => null }))
vi.mock("./SvgSplitView", () => ({ isSvgPath: () => false, SvgSplitView: () => null }))
vi.mock("@/lib/actionFeedback", () => ({ showActionError: vi.fn(async () => {}) }))

const initialWorkspace = useWorkspaceStore.getState()
const rightPath = "/repo/right.ts"
const saveFile = vi.fn()
const file = (path: string, dirty = false) => ({
    path, name: path.split("/").at(-1)!, kind: "file" as const, dirty, externallyModified: false
})

beforeEach(() => {
    saveFile.mockReset().mockResolvedValue(7)
    mockIPC((command, payload) => {
        if (command === "open_file") return { kind: "full", content: "disk", size: 4, lineEnding: "lf" }
        if (command === "save_file") return saveFile(payload)
    })
    useWorkspaceStore.setState({
        workspacePath: "/repo",
        groups: [
            { id: "left", tabs: [file("/repo/left.ts")], activePath: "/repo/left.ts" },
            { id: "right", tabs: [file(rightPath, true), file("/repo/active.ts")], activePath: "/repo/active.ts" }
        ],
        activeGroupIndex: 1
    })
})

afterEach(() => {
    useConfirmDialogStore.getState().respond("cancel")
    cleanup()
    clearAll()
    clearMocks()
    useWorkspaceStore.setState(initialWorkspace, true)
})

async function mountSplit() {
    await getDocument(rightPath)
    updateBuffer(rightPath, "unsaved", documentGeneration(rightPath))
    render(<><WorkspaceResourcesBridge /><EditorArea /></>)
}

async function closeSplit() {
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Close split" })))
}

it("keeps an inactive dirty right-side buffer until the close is confirmed", async () => {
    await mountSplit()
    await closeSplit()
    expect((await getDocument(rightPath)).result).toMatchObject({ content: "unsaved" })
    expect(useWorkspaceStore.getState().groups).toHaveLength(2)
    expect(useConfirmDialogStore.getState().pending).not.toBeNull()
})

it("closes a clean split without prompting for a retained left-side dirty file", async () => {
    useWorkspaceStore.getState().markDirty(rightPath, false)
    useWorkspaceStore.getState().markDirty("/repo/left.ts", true)
    await mountSplit()
    await closeSplit()
    expect(useWorkspaceStore.getState().groups).toHaveLength(1)
    expect(useConfirmDialogStore.getState().pending).toBeNull()
    expect(useWorkspaceStore.getState().groups[0].tabs[0].dirty).toBe(true)
})

it("preserves a dirty buffer with a remaining file owner in the left group", async () => {
    useWorkspaceStore.setState(state => ({
        groups: [{ ...state.groups[0], tabs: [file(rightPath, true)], activePath: rightPath }, state.groups[1]]
    }))
    await mountSplit()
    await closeSplit()
    expect(useWorkspaceStore.getState().groups).toHaveLength(1)
    expect(useConfirmDialogStore.getState().pending).toBeNull()
    expect((await getDocument(rightPath)).result).toMatchObject({ content: "unsaved" })
})

it.each(["cancel", "discard"] as const)("honors %s without writing the dirty file", async decision => {
    await mountSplit()
    await closeSplit()
    await act(async () => useConfirmDialogStore.getState().respond(decision))
    expect(saveFile).not.toHaveBeenCalled()
    expect(useWorkspaceStore.getState().groups).toHaveLength(decision === "cancel" ? 2 : 1)
    expect((await getDocument(rightPath)).result).toMatchObject({ content: decision === "cancel" ? "unsaved" : "disk" })
})

it("saves every disappearing dirty file before closing the split", async () => {
    const other = "/repo/other.ts"
    useWorkspaceStore.setState(state => ({ groups: [state.groups[0], {
        ...state.groups[1], tabs: [...state.groups[1].tabs, file(other, true)]
    }] }))
    await getDocument(other)
    updateBuffer(other, "other edits", documentGeneration(other))
    await mountSplit()
    await closeSplit()
    await act(async () => useConfirmDialogStore.getState().respond("save"))
    await waitFor(() => expect(useWorkspaceStore.getState().groups).toHaveLength(1))
    expect(saveFile.mock.calls.map(([payload]) => [payload.path, payload.content])).toEqual([
        [rightPath, "unsaved"], [other, "other edits"]
    ])
})

it("keeps the split and dirty buffer when saving fails", async () => {
    saveFile.mockRejectedValue(new Error("fixture write failed"))
    await mountSplit()
    await closeSplit()
    await act(async () => useConfirmDialogStore.getState().respond("save"))
    expect(saveFile).toHaveBeenCalledTimes(1)
    expect(useWorkspaceStore.getState().groups).toHaveLength(2)
    expect(useWorkspaceStore.getState().groups[1].tabs[0].dirty).toBe(true)
    expect((await getDocument(rightPath)).result).toMatchObject({ content: "unsaved" })
})

it.each(["save", "discard"] as const)("rejects a stale %s decision after switching away and back", async decision => {
    await mountSplit()
    await closeSplit()
    const groups = useWorkspaceStore.getState().groups
    act(() => {
        useWorkspaceStore.getState().setWorkspace("/other")
        useWorkspaceStore.setState({ workspacePath: "/repo", groups })
    })
    await getDocument(rightPath)
    updateBuffer(rightPath, "new workspace edits", documentGeneration(rightPath))
    await act(async () => useConfirmDialogStore.getState().respond(decision))
    expect(saveFile).not.toHaveBeenCalled()
    expect(useWorkspaceStore.getState().groups).toHaveLength(2)
    expect((await getDocument(rightPath)).result).toMatchObject({ content: "new workspace edits" })
})

it.each(["save", "discard"] as const)("rejects %s when another right-side file becomes dirty while confirming", async decision => {
    await mountSplit()
    await closeSplit()
    act(() => useWorkspaceStore.getState().markDirty("/repo/active.ts", true))
    await act(async () => useConfirmDialogStore.getState().respond(decision))
    expect(saveFile).not.toHaveBeenCalled()
    expect(useWorkspaceStore.getState().groups).toHaveLength(2)
    expect(useWorkspaceStore.getState().groups[1].tabs.every(tab => tab.dirty)).toBe(true)
})

it("does not clear or close edits made while the save is in flight", async () => {
    let finish!: (size: number) => void
    saveFile.mockImplementation(() => new Promise<number>(resolve => { finish = resolve }))
    await mountSplit()
    await closeSplit()
    await act(async () => useConfirmDialogStore.getState().respond("save"))
    await waitFor(() => expect(saveFile).toHaveBeenCalledTimes(1))
    updateBuffer(rightPath, "newer edits", documentGeneration(rightPath))
    await act(async () => finish(7))
    expect(useWorkspaceStore.getState().groups).toHaveLength(2)
    expect(useWorkspaceStore.getState().groups[1].tabs[0].dirty).toBe(true)
    expect((await getDocument(rightPath)).result).toMatchObject({ content: "newer edits" })
})

it("does not clear a replacement workspace's dirty flag when an old save finishes", async () => {
    let finish!: (size: number) => void
    saveFile.mockImplementation(() => new Promise<number>(resolve => { finish = resolve }))
    await mountSplit()
    await closeSplit()
    await act(async () => useConfirmDialogStore.getState().respond("save"))
    await waitFor(() => expect(saveFile).toHaveBeenCalledTimes(1))
    const groups = useWorkspaceStore.getState().groups
    act(() => useWorkspaceStore.setState({ workspacePath: "/replacement", groups }))
    await getDocument(rightPath)
    updateBuffer(rightPath, "replacement edits", documentGeneration(rightPath))
    await act(async () => finish(7))
    expect(useWorkspaceStore.getState().groups).toHaveLength(2)
    expect(useWorkspaceStore.getState().groups[1].tabs[0].dirty).toBe(true)
    expect((await getDocument(rightPath)).result).toMatchObject({ content: "replacement edits" })
})

it("keeps one pending confirmation on repeated clicks", async () => {
    await mountSplit()
    await closeSplit()
    const pending = useConfirmDialogStore.getState().pending
    await closeSplit()
    expect(useConfirmDialogStore.getState().pending).toBe(pending)
    expect(useWorkspaceStore.getState().groups).toHaveLength(2)
})

it("keeps a line-ending preference changed while the save is in flight", async () => {
    let finish!: (size: number) => void
    saveFile.mockImplementation(() => new Promise<number>(resolve => { finish = resolve }))
    useWorkspaceStore.getState().setLineEnding(rightPath, "lf")
    await mountSplit()
    await closeSplit()
    await act(async () => useConfirmDialogStore.getState().respond("save"))
    await waitFor(() => expect(saveFile).toHaveBeenCalledTimes(1))
    act(() => useWorkspaceStore.getState().setLineEnding(rightPath, "crlf"))
    await act(async () => finish(7))
    expect(useWorkspaceStore.getState().groups).toHaveLength(2)
    expect(useWorkspaceStore.getState().groups[1].tabs[0]).toMatchObject({ dirty: true, lineEnding: "crlf" })
})
