import { afterEach, beforeEach, expect, test, vi } from "vitest"

const openPicker = vi.hoisted(() => vi.fn())
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: openPicker }))
vi.mock("@/lib/ipc", () => ({
    openWorkspace: vi.fn(),
    startWatch: vi.fn().mockResolvedValue(undefined),
    saveFile: vi.fn().mockResolvedValue(0),
    allowWorkspaceAssetScope: vi.fn().mockResolvedValue(undefined)
}))
vi.mock("@/features/logs/userAction", () => ({ logUserAction: vi.fn() }))
vi.mock("@/editor/saveDocument", () => ({ saveDirtyTab: vi.fn() }))

import { openWorkspaceAtPath, pickWorkspace } from "@/lib/workspaceActions"
import { allowWorkspaceAssetScope, openWorkspace } from "@/lib/ipc"
import { saveDirtyTab } from "@/editor/saveDocument"
import { useConfirmDialogStore } from "@/state/confirmDialogStore"
import { useRecentWorkspacesStore } from "@/state/recentWorkspaces"
import { useWorkspaceStore } from "@/state/workspaceStore"

// The Bun-hosted test runtime injects an empty localStorage global; install a
// minimal in-memory Storage so recentWorkspaces.record can run (mirrors
// workspaceRail.test.tsx).
function installLocalStorage(): void {
    const store = new Map<string, string>()
    Object.defineProperty(globalThis, "localStorage", {
        value: {
            getItem: (k: string) => (store.has(k) ? store.get(k)! : null),
            setItem: (k: string, v: string) => void store.set(k, String(v)),
            removeItem: (k: string) => void store.delete(k),
            clear: () => store.clear(),
            key: (i: number) => [...store.keys()][i] ?? null,
            get length() {
                return store.size
            }
        },
        configurable: true,
        writable: true
    })
}

const dirtyWorkspace = () =>
    useWorkspaceStore.setState({
        workspacePath: "/old",
        activeGroupIndex: 0,
        groups: [
            {
                activePath: "/old/a.ts",
                tabs: [{ path: "/old/a.ts", name: "a.ts", dirty: true, externallyModified: false }]
            }
        ]
    })

beforeEach(() => {
    installLocalStorage()
    localStorage.clear()
    vi.mocked(openWorkspace).mockReset().mockResolvedValue("/canonical")
    vi.mocked(allowWorkspaceAssetScope).mockReset().mockResolvedValue(undefined)
    vi.mocked(saveDirtyTab).mockReset().mockResolvedValue(undefined)
    openPicker.mockReset().mockResolvedValue(null)
    useConfirmDialogStore.setState({ pending: null })
    useRecentWorkspacesStore.setState({ list: [] })
})

afterEach(() => {
    useConfirmDialogStore.setState({ pending: null })
})

test("有 dirty 分頁：cancel → 不開新工作區、workspace 不變", async () => {
    dirtyWorkspace()
    const p = openWorkspaceAtPath("/new")
    // requestUnsavedDecision 於 promise executor 同步設定 pending
    expect(useConfirmDialogStore.getState().pending).not.toBeNull()
    useConfirmDialogStore.getState().respond("cancel")
    await p
    expect(openWorkspace).not.toHaveBeenCalled()
    expect(saveDirtyTab).not.toHaveBeenCalled()
    expect(useWorkspaceStore.getState().workspacePath).toBe("/old")
})

test("有 dirty 分頁：discard → 不存檔、直接開新工作區", async () => {
    dirtyWorkspace()
    const p = openWorkspaceAtPath("/new")
    useConfirmDialogStore.getState().respond("discard")
    await p
    expect(saveDirtyTab).not.toHaveBeenCalled()
    expect(openWorkspace).toHaveBeenCalledWith("/new")
    expect(useWorkspaceStore.getState().workspacePath).toBe("/canonical")
})

test("有 dirty 分頁：save → 先存檔再開新工作區", async () => {
    dirtyWorkspace()
    const p = openWorkspaceAtPath("/new")
    useConfirmDialogStore.getState().respond("save")
    await p
    expect(saveDirtyTab).toHaveBeenCalledWith("/old/a.ts")
    expect(openWorkspace).toHaveBeenCalledWith("/new")
})

test("無 dirty 分頁：不彈 modal、直接開新工作區", async () => {
    useWorkspaceStore.setState({
        workspacePath: "/old",
        activeGroupIndex: 0,
        groups: [{ activePath: null, tabs: [] }]
    })
    await openWorkspaceAtPath("/new")
    expect(useConfirmDialogStore.getState().pending).toBeNull()
    expect(openWorkspace).toHaveBeenCalledWith("/new")
})

// P3：image tabs 走 asset protocol，openWorkspaceAtPath 必須 await scope grant，
// 否則 session restore 開回的圖片分頁會與 grant 競速（batch 2 reviewer NB-1）。
test("asset scope grant 完成前 openWorkspaceAtPath 不 resolve", async () => {
    useWorkspaceStore.setState({
        workspacePath: null,
        activeGroupIndex: 0,
        groups: [{ activePath: null, tabs: [] }]
    })
    let releaseGrant!: () => void
    vi.mocked(allowWorkspaceAssetScope).mockImplementation(
        () => new Promise<void>((resolve) => {
            releaseGrant = resolve
        })
    )
    let settled = false
    const pending = openWorkspaceAtPath("/new").then(() => {
        settled = true
    })
    await vi.waitFor(() => expect(allowWorkspaceAssetScope).toHaveBeenCalledWith("/canonical"))
    await new Promise((resolve) => setTimeout(resolve, 0))
    expect(settled).toBe(false)
    releaseGrant()
    await pending
    expect(settled).toBe(true)
})

test("asset scope grant 失敗不阻斷開 workspace（warn 後照常記錄與監看）", async () => {
    useWorkspaceStore.setState({
        workspacePath: null,
        activeGroupIndex: 0,
        groups: [{ activePath: null, tabs: [] }]
    })
    vi.mocked(allowWorkspaceAssetScope).mockRejectedValue(new Error("scope denied"))
    const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {})
    await openWorkspaceAtPath("/new")
    expect(useWorkspaceStore.getState().workspacePath).toBe("/canonical")
    expect(warnSpy).toHaveBeenCalledWith(
        "allow_workspace_asset_scope failed:",
        expect.any(Error)
    )
    expect(useRecentWorkspacesStore.getState().list.length).toBeGreaterThan(0)
    warnSpy.mockRestore()
})

test("pickWorkspace native picker cancel 回傳 false", async () => {
    expect(await pickWorkspace()).toBe(false)
    expect(openWorkspace).not.toHaveBeenCalled()
})

test("pickWorkspace 選擇路徑後取消 dirty switch 仍回傳 false", async () => {
    dirtyWorkspace()
    openPicker.mockResolvedValue("/new")
    const pending = pickWorkspace()
    await vi.waitFor(() => expect(useConfirmDialogStore.getState().pending).not.toBeNull())
    useConfirmDialogStore.getState().respond("cancel")
    expect(await pending).toBe(false)
    expect(openWorkspace).not.toHaveBeenCalled()
})
