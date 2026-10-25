import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { EditorState } from "@codemirror/state"
import { EditorView } from "@codemirror/view"
import * as ipc from "../lib/ipc"
import type { OpenFileResult } from "../lib/types"
import { registerView, unregisterView } from "../editor/viewRegistry"
import { clearAll, documentGeneration } from "../editor/documentRegistry"
import { useUiStore } from "../state/uiStore"
import { useWorkspaceStore } from "../state/workspaceStore"
import { ExternalChangeResolver } from "./ExternalChangeResolver"

vi.mock("../lib/ipc", () => {
    const openFileSnapshot = vi.fn()
    return {
        openFileSnapshot,
        openFile: async (path: string) => { const snapshot = await openFileSnapshot(path); snapshot.accept(); return snapshot.result },
        saveFile: vi.fn(async () => 0),
    }
})
let notify: (event: { payload: { workspaceRoot: string; paths: string[] } }) => void
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async (_name: string, callback: typeof notify) => { notify = callback; return () => {} }) }))
vi.mock("../lib/actionFeedback", () => ({ showActionError: vi.fn(async () => {}) }))
vi.mock("@/features/logs/userAction", () => ({ logUserAction: vi.fn(async () => {}) }))
const PATH = "/w/a.ts"
const workspace = useWorkspaceStore.getState(), ui = useUiStore.getState()
let main: EditorView
type Snapshot = Awaited<ReturnType<typeof ipc.openFileSnapshot>>
const snapshot = (content: string): Snapshot => ({ result: { kind: "full", content, size: content.length, lineEnding: "lf" }, accept: vi.fn() })
function deferred<T>() {
    let resolve!: (value: T) => void, reject!: (error: unknown) => void
    const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no })
    return { promise, resolve, reject }
}
const changed = () => notify({ payload: { workspaceRoot: "/w", paths: [PATH] } })
const merge = () => EditorView.findFromDOM(document.querySelector(".external-resolver-merge .cm-editor") as HTMLElement)!
const ready = () => waitFor(() => expect(screen.getByRole("button", { name: "解決並存檔" })).toBeEnabled())
beforeEach(() => {
    clearAll()
    useWorkspaceStore.setState({ ...workspace, workspacePath: "/w" }, true)
    useUiStore.setState(ui, true)
    vi.mocked(ipc.openFileSnapshot).mockReset().mockResolvedValue(snapshot("initial disk"))
    vi.mocked(ipc.saveFile).mockClear()
    main = new EditorView({ state: EditorState.create({ doc: "mine" }), parent: document.body })
    registerView(PATH, main)
    const store = useWorkspaceStore.getState()
    store.openTab(PATH); store.hydrateLineEnding(PATH, "lf", documentGeneration(PATH))
    store.markDirty(PATH, true); store.markExternallyModified(PATH, true)
    useUiStore.getState().openResolver(PATH)
})
afterEach(() => { cleanup(); unregisterView(PATH, main); main.destroy(); clearAll() })

it.each(["initial", "watcher"] as const)("does not replace the latest disk with a late %s read", async first => {
    const older = deferred<Snapshot>(), newer = deferred<Snapshot>()
    if (first === "initial") vi.mocked(ipc.openFileSnapshot).mockReturnValueOnce(older.promise)
    render(<ExternalChangeResolver />)
    if (first === "watcher") {
        await ready()
        vi.mocked(ipc.openFileSnapshot).mockReturnValueOnce(older.promise)
        act(changed)
    } else await waitFor(() => expect(ipc.openFileSnapshot).toHaveBeenCalledTimes(1))
    vi.mocked(ipc.openFileSnapshot).mockReturnValueOnce(newer.promise)
    act(changed)
    const oldDisk = snapshot("older disk"), newDisk = snapshot("newest disk")
    await act(async () => { newer.resolve(newDisk); await newer.promise })
    await act(async () => { older.resolve(oldDisk); await older.promise })
    await ready()
    fireEvent.click(screen.getByRole("button", { name: "全部採用磁碟版" }))
    fireEvent.click(screen.getByRole("button", { name: "解決並存檔" }))
    await waitFor(() => expect(ipc.saveFile).toHaveBeenCalledWith(PATH, "newest disk"))
    expect(oldDisk.accept).not.toHaveBeenCalled()
    expect(newDisk.accept).toHaveBeenCalledTimes(1)
})

it("ignores a late initial failure after a newer watcher read succeeded", async () => {
    const initial = deferred<Snapshot>()
    vi.mocked(ipc.openFileSnapshot).mockReturnValueOnce(initial.promise)
    render(<ExternalChangeResolver />)
    await waitFor(() => expect(ipc.openFileSnapshot).toHaveBeenCalledTimes(1))
    vi.mocked(ipc.openFileSnapshot).mockResolvedValueOnce(snapshot("newest disk"))
    await act(async () => changed())
    await act(async () => { initial.reject(new Error("old missing reply")); await initial.promise.catch(() => {}) })
    await ready()
    expect(screen.queryByText("磁碟上的檔案已不存在，無法比對差異。")).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: "全部採用磁碟版" }))
    expect(merge().state.doc.toString()).toBe("newest disk")
})

it.each(["cancel", "workspace", "session"] as const)("does not accept a read revision after %s invalidates the resolver", async change => {
    const pending = deferred<Snapshot>()
    vi.mocked(ipc.openFileSnapshot).mockReturnValueOnce(pending.promise)
    render(<ExternalChangeResolver />)
    await waitFor(() => expect(ipc.openFileSnapshot).toHaveBeenCalledTimes(1))
    if (change === "cancel") fireEvent.click(screen.getByRole("button", { name: "取消" }))
    if (change === "workspace") act(() => { useWorkspaceStore.setState({ workspacePath: "/other" }); clearAll() })
    if (change === "session") act(() => useUiStore.getState().openResolver(PATH))
    const stale = snapshot("stale disk")
    await act(async () => { pending.resolve(stale); await pending.promise })
    expect(stale.accept).not.toHaveBeenCalled()
    expect(main.state.doc.toString()).toBe("mine")
    expect(ipc.saveFile).not.toHaveBeenCalled()
})

it("accepts an unchanged latest revision without replacing merge progress", async () => {
    render(<ExternalChangeResolver />)
    await ready()
    const view = merge()
    view.dispatch({ changes: { from: 0, insert: "progress " } })
    const latest = snapshot("initial disk")
    vi.mocked(ipc.openFileSnapshot).mockResolvedValueOnce(latest)
    await act(async () => changed())
    expect(merge()).toBe(view)
    expect(merge().state.doc.toString()).toBe("progress mine")
    expect(latest.accept).toHaveBeenCalledTimes(1)
})

it("preserves merge progress through the latest binary and text disk grades", async () => {
    render(<ExternalChangeResolver />)
    await ready()
    merge().dispatch({ changes: { from: 0, insert: "progress " } })
    vi.mocked(ipc.openFileSnapshot).mockResolvedValueOnce({ result: { kind: "binary", size: 10 } as OpenFileResult, accept: vi.fn() })
    await act(async () => changed())
    await screen.findByRole("button", { name: "採用磁碟版（重新載入）" })
    expect(document.querySelector(".external-resolver-merge .cm-editor")).toBeNull()
    vi.mocked(ipc.openFileSnapshot).mockResolvedValueOnce(snapshot("new text"))
    await act(async () => changed())
    await ready()
    expect(merge().state.doc.toString()).toBe("progress mine")
    fireEvent.click(screen.getByRole("button", { name: "全部採用磁碟版" }))
    expect(merge().state.doc.toString()).toBe("new text")
})
