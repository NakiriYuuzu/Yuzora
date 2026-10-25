import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { EditorState } from "@codemirror/state"
import { EditorView } from "@codemirror/view"
import * as ipc from "../lib/ipc"
import type { OpenFileResult } from "../lib/types"
import { registerView, unregisterView } from "../editor/viewRegistry"
import { clearAll, documentGeneration, getCachedDocument, getDocument, reloadDocument } from "../editor/documentRegistry"
import { useUiStore } from "../state/uiStore"
import { useWorkspaceStore } from "../state/workspaceStore"
import { ExternalChangeResolver } from "./ExternalChangeResolver"

vi.mock("../lib/ipc", () => {
    const openFile = vi.fn()
    return { openFile, openFileSnapshot: async (path: string) => ({ result: await openFile(path), accept: vi.fn() }), saveFile: vi.fn() }
})
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }))
vi.mock("../lib/actionFeedback", () => ({ showActionError: vi.fn(async () => {}) }))
vi.mock("@/features/logs/userAction", () => ({ logUserAction: vi.fn(async () => {}) }))

const PATH = "/w/a.ts"
const workspace = useWorkspaceStore.getState()
const ui = useUiStore.getState()
const views: EditorView[] = []
const full = (content: string): OpenFileResult => ({ kind: "full", content, size: content.length, lineEnding: "lf" })
function deferred<T>() {
    let resolve!: (value: T) => void
    let reject!: (error: unknown) => void
    const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no })
    return { promise, resolve, reject }
}
function mainView(content = "mine") {
    const view = new EditorView({ state: EditorState.create({ doc: content }), parent: document.body })
    views.push(view)
    registerView(PATH, view)
    return view
}
function tab() { return useWorkspaceStore.getState().groups[0].tabs.find(t => t.path === PATH)! }
function dirtyTab() {
    const store = useWorkspaceStore.getState()
    store.openTab(PATH)
    store.hydrateLineEnding(PATH, "lf", documentGeneration(PATH))
    store.markDirty(PATH, true)
    store.markExternallyModified(PATH, true)
}
async function openResolver(binary = false) {
    vi.mocked(ipc.openFile).mockResolvedValue(binary ? { kind: "binary", size: 10 } : full("disk"))
    useUiStore.getState().openResolver(PATH)
    const mounted = render(<ExternalChangeResolver />)
    await waitFor(() => expect(screen.getByRole("button", { name: binary ? "採用磁碟版（重新載入）" : "解決並存檔" })).toBeEnabled())
    return mounted
}
beforeEach(() => {
    clearAll()
    useWorkspaceStore.setState({ ...workspace, workspacePath: "/w" }, true)
    useUiStore.setState(ui, true)
    vi.mocked(ipc.openFile).mockReset()
    vi.mocked(ipc.saveFile).mockReset().mockResolvedValue(0)
    dirtyTab()
})
afterEach(() => {
    cleanup()
    for (const view of views.splice(0)) { unregisterView(PATH, view); view.destroy() }
    clearAll()
})

it.each(["cancel", "edit", "view", "workspace", "generation", "lineEnding", "reopen", "session"] as const)(
    "does not publish a late resolver save after %s changes its owner", async change => {
        const main = mainView()
        const save = deferred<number>()
        vi.mocked(ipc.saveFile).mockReturnValueOnce(save.promise)
        await openResolver()
        fireEvent.click(screen.getByRole("button", { name: "全部採用磁碟版" }))
        fireEvent.click(screen.getByRole("button", { name: "解決並存檔" }))
        await waitFor(() => expect(ipc.saveFile).toHaveBeenCalledWith(PATH, "disk"))
        let current = main
        if (change === "cancel" || change === "reopen") fireEvent.click(screen.getByRole("button", { name: "取消" }))
        if (change === "view") current = mainView("new owner")
        if (change === "workspace") act(() => {
            useWorkspaceStore.setState({ workspacePath: "/other" }); clearAll()
            useWorkspaceStore.setState({ workspacePath: "/w" }); dirtyTab()
        })
        if (change === "generation") await act(async () => { await reloadDocument(PATH) })
        if (change === "lineEnding") act(() => useWorkspaceStore.getState().setLineEnding(PATH, "crlf"))
        if (change === "session") act(() => {
            useUiStore.getState().closeResolver()
            useUiStore.getState().openResolver(PATH)
        })
        if (change === "cancel" || change === "edit" || change === "reopen") {
            current.dispatch({ changes: { from: 0, to: current.state.doc.length, insert: "new typing" } })
        }
        if (change === "reopen") {
            act(() => useUiStore.getState().openResolver(PATH))
            await screen.findByRole("button", { name: "解決並存檔" })
        }
        const expected = current.state.doc.toString()
        await act(async () => { save.resolve(4); await save.promise })
        expect(current.state.doc.toString()).toBe(expected)
        expect(main.state.doc.toString()).not.toBe("disk")
        expect(tab().dirty).toBe(true)
        expect(tab().externallyModified).toBe(true)
        expect(useUiStore.getState().resolverPath).toBe(change === "cancel" ? null : PATH)
    }
)

it.each(["cancel", "edit", "reject-after-cancel"] as const)("does not accept a stale degraded reload after %s", async change => {
    const main = mainView()
    vi.mocked(ipc.openFile).mockResolvedValue(full("mine"))
    const entry = await getDocument(PATH)
    const generation = documentGeneration(PATH)
    await openResolver(true)
    const read = deferred<OpenFileResult>()
    vi.mocked(ipc.openFile).mockReturnValueOnce(read.promise)
    fireEvent.click(screen.getByRole("button", { name: "採用磁碟版（重新載入）" }))
    if (change !== "edit") {
        fireEvent.click(screen.getByRole("button", { name: "取消" }))
        act(() => useUiStore.getState().openResolver(PATH))
        await screen.findByRole("button", { name: "採用磁碟版（重新載入）" })
    }
    main.dispatch({ changes: { from: 0, to: main.state.doc.length, insert: "new typing" } })
    await act(async () => {
        if (change === "reject-after-cancel") read.reject(new Error("stale read"))
        else read.resolve(full("late disk"))
        await read.promise.catch(() => {})
    })
    expect(getCachedDocument(PATH)).toBe(entry)
    expect(documentGeneration(PATH)).toBe(generation)
    expect(main.state.doc.toString()).toBe("new typing")
    expect(tab().dirty).toBe(true)
    expect(tab().externallyModified).toBe(true)
    expect(useUiStore.getState().resolverPath).toBe(PATH)
})

it.each([false, true])("does not start reconciliation after the live buffer changed (degraded=%s)", async binary => {
    const main = mainView()
    await openResolver(binary)
    const reads = vi.mocked(ipc.openFile).mock.calls.length
    main.dispatch({ changes: { from: 0, insert: "new " } })
    fireEvent.click(screen.getByRole("button", { name: binary ? "採用磁碟版（重新載入）" : "解決並存檔" }))
    expect(ipc.saveFile).not.toHaveBeenCalled()
    expect(ipc.openFile).toHaveBeenCalledTimes(reads)
    expect(tab().dirty).toBe(true)
    expect(main.state.doc.toString()).toBe("new mine")
})

it("admits only one save while leaving cancellation available", async () => {
    mainView()
    const save = deferred<number>()
    vi.mocked(ipc.saveFile).mockReturnValueOnce(save.promise)
    await openResolver()
    const button = screen.getByRole("button", { name: "解決並存檔" })
    fireEvent.click(button)
    expect(button).toBeDisabled()
    expect(screen.getByRole("button", { name: "取消" })).toBeEnabled()
    fireEvent.click(button)
    await act(async () => { save.resolve(4); await save.promise })
    expect(ipc.saveFile).toHaveBeenCalledTimes(1)
    expect(tab().dirty).toBe(false)
})

it("retains merge edits made while a save is pending", async () => {
    const main = mainView()
    const save = deferred<number>()
    vi.mocked(ipc.saveFile).mockReturnValueOnce(save.promise)
    await openResolver()
    fireEvent.click(screen.getByRole("button", { name: "解決並存檔" }))
    const merge = EditorView.findFromDOM(document.querySelector(".external-resolver-merge .cm-editor") as HTMLElement)!
    merge.dispatch({ changes: { from: 0, insert: "later merge " } })
    await act(async () => { save.resolve(4); await save.promise })
    expect(merge.state.doc.toString()).toBe("later merge mine")
    expect(main.state.doc.toString()).toBe("mine")
    expect(tab().dirty).toBe(true)
    expect(useUiStore.getState().resolverPath).toBe(PATH)
    expect(screen.getByRole("button", { name: "解決並存檔" })).toBeEnabled()
})
