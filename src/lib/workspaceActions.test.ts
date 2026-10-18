import { afterEach, beforeEach, expect, test, vi } from "vitest"

const openPicker = vi.hoisted(() => vi.fn())
vi.mock("@/state/folderPickerStore", () => ({ chooseWorkspaceFolder: openPicker }))
vi.mock("@/lib/ipc", () => ({
    openWorkspace: vi.fn(),
    startWatch: vi.fn().mockResolvedValue(undefined),
    saveFile: vi.fn().mockResolvedValue(0),
    allowWorkspaceAssetScope: vi.fn().mockResolvedValue(undefined)
}))
vi.mock("@/features/logs/userAction", () => ({ logUserAction: vi.fn() }))
vi.mock("@/editor/saveDocument", () => ({ saveDirtyTab: vi.fn() }))

import { openWorkspaceAtPath, pickWorkspace } from "@/lib/workspaceActions"
import { allowWorkspaceAssetScope, openWorkspace, startWatch } from "@/lib/ipc"
import { saveDirtyTab } from "@/editor/saveDocument"
import { snapshotDirtyTabs } from "@/lib/unsavedGuard"
import { useConfirmDialogStore } from "@/state/confirmDialogStore"
import { useRecentWorkspacesStore } from "@/state/recentWorkspaces"
import { saveWorkspaceSession } from "@/state/workspaceSession"
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
    vi.mocked(openWorkspace).mockReset().mockResolvedValue({
        canonicalPath: "/canonical",
        capabilityId: "ws-capability"
    })
    vi.mocked(allowWorkspaceAssetScope).mockReset().mockResolvedValue(undefined)
    vi.mocked(startWatch).mockReset().mockResolvedValue(undefined)
    // Like the real save: a written tab is clean again.
    vi.mocked(saveDirtyTab).mockReset().mockImplementation(async (path) => {
        useWorkspaceStore.getState().markDirty(path, false)
        return { kind: "saved" }
    })
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

test("drops a delayed restore when the user opens a workspace during canonicalization", async () => {
    useWorkspaceStore.setState({ workspacePath: null, groups: [{ tabs: [], activePath: null }] })
    let finish!: (value: Awaited<ReturnType<typeof openWorkspace>>) => void
    vi.mocked(openWorkspace).mockImplementationOnce(() => new Promise((resolve) => { finish = resolve }))
    const opening = openWorkspaceAtPath("/restore", { shouldOpen: () => !useWorkspaceStore.getState().workspacePath })
    await Promise.resolve()
    useWorkspaceStore.getState().setWorkspace("/chosen")
    finish({ canonicalPath: "/restore", capabilityId: "late" })
    expect(await opening).toBe(false)
    expect(useWorkspaceStore.getState().workspacePath).toBe("/chosen")
})

test("有 dirty 分頁：discard → 不存檔、直接開新工作區", async () => {
    dirtyWorkspace()
    const p = openWorkspaceAtPath("/new")
    useConfirmDialogStore.getState().respond("discard")
    await p
    expect(saveDirtyTab).not.toHaveBeenCalled()
    expect(openWorkspace).toHaveBeenCalledWith("/new")
    expect(useWorkspaceStore.getState().workspacePath).toBe("/canonical")
    expect(useWorkspaceStore.getState().workspaceCapabilityId).toBe("ws-capability")
})

test("有 dirty 分頁：save → 先存檔再開新工作區", async () => {
    dirtyWorkspace()
    const p = openWorkspaceAtPath("/new")
    useConfirmDialogStore.getState().respond("save")
    await p
    expect(saveDirtyTab).toHaveBeenCalledWith("/old/a.ts")
    expect(openWorkspace).toHaveBeenCalledWith("/new")
})

test("有 dirty Mixed 分頁：save 被 block → 不切換工作區", async () => {
    dirtyWorkspace()
    vi.mocked(saveDirtyTab).mockResolvedValue({ kind: "blocked", reason: "mixed" })
    const pending = openWorkspaceAtPath("/new")
    useConfirmDialogStore.getState().respond("save")

    await pending

    expect(saveDirtyTab).toHaveBeenCalledWith("/old/a.ts")
    expect(openWorkspace).not.toHaveBeenCalled()
    expect(useWorkspaceStore.getState().workspacePath).toBe("/old")
})

test("有 dirty 分頁：save I/O failed → 不切換工作區", async () => {
    dirtyWorkspace()
    vi.mocked(saveDirtyTab).mockResolvedValue({ kind: "failed" })
    const pending = openWorkspaceAtPath("/new")
    useConfirmDialogStore.getState().respond("save")

    await pending

    expect(saveDirtyTab).toHaveBeenCalledWith("/old/a.ts")
    expect(openWorkspace).not.toHaveBeenCalled()
    expect(useWorkspaceStore.getState().workspacePath).toBe("/old")
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

// #57 T3：setWorkspace 之後的 assetScope／watcher／MRU 全部並行 fire-and-forget。
// 兩個 IPC 都懸置不 resolve，openWorkspaceAtPath 仍須完成並記錄 MRU——任何一個
// 被序列 await 都會讓本測試逾時。
test("編排並行：assetScope 與 startWatch 懸置時 openWorkspaceAtPath 照樣 resolve", async () => {
    useWorkspaceStore.setState({
        workspacePath: null,
        activeGroupIndex: 0,
        groups: [{ activePath: null, tabs: [] }]
    })
    vi.mocked(allowWorkspaceAssetScope).mockImplementation(() => new Promise<void>(() => {}))
    vi.mocked(startWatch).mockImplementation(() => new Promise<void>(() => {}))
    await openWorkspaceAtPath("/new")
    expect(useWorkspaceStore.getState().workspacePath).toBe("/canonical")
    expect(allowWorkspaceAssetScope).toHaveBeenCalledWith("/canonical")
    expect(startWatch).toHaveBeenCalledWith("/canonical")
    expect(useRecentWorkspacesStore.getState().list.length).toBeGreaterThan(0)
})

// T4 覆核修正（NB-1 回歸重建）：圖片分頁走 asset protocol，ImageView 一 mount
// 就發 <img> 請求；grant 未落地會 403 → 永久 loadError（錯誤狀態不重試）。
// 切回含圖片分頁的 workspace 時，還原必須等 grant 落地。
test("切回含圖片分頁的 workspace：grant 落地前不還原分頁", async () => {
    useWorkspaceStore.setState({
        workspacePath: null,
        activeGroupIndex: 0,
        groups: [{ activePath: null, tabs: [] }]
    })
    saveWorkspaceSession({
        workspacePath: "/canonical",
        tabs: ["/canonical/pic.png", "/canonical/a.ts"],
        activePath: "/canonical/pic.png"
    })
    let releaseGrant!: () => void
    vi.mocked(allowWorkspaceAssetScope).mockImplementation(
        () =>
            new Promise<void>((resolve) => {
                releaseGrant = () => resolve()
            })
    )
    let settled = false
    const pending = openWorkspaceAtPath("/new").then(() => {
        settled = true
    })
    await vi.waitFor(() => expect(allowWorkspaceAssetScope).toHaveBeenCalledWith("/canonical"))
    await new Promise((resolve) => setTimeout(resolve, 0))
    // grant 未落地：不得先開出分頁（ImageView 會立刻對 asset protocol 發請求）。
    expect(useWorkspaceStore.getState().groups[0].tabs).toEqual([])
    expect(settled).toBe(false)
    releaseGrant()
    await pending
    expect(settled).toBe(true)
    expect(useWorkspaceStore.getState().groups[0].tabs.map((t) => t.path)).toEqual([
        "/canonical/pic.png",
        "/canonical/a.ts"
    ])
    expect(useWorkspaceStore.getState().groups[0].activePath).toBe("/canonical/pic.png")
})

test("切回純文字分頁的 workspace：grant 懸置不阻斷還原（並行維持）", async () => {
    useWorkspaceStore.setState({
        workspacePath: null,
        activeGroupIndex: 0,
        groups: [{ activePath: null, tabs: [] }]
    })
    saveWorkspaceSession({
        workspacePath: "/canonical",
        tabs: ["/canonical/a.ts"],
        activePath: "/canonical/a.ts"
    })
    vi.mocked(allowWorkspaceAssetScope).mockImplementation(() => new Promise<void>(() => {}))
    await openWorkspaceAtPath("/new")
    expect(useWorkspaceStore.getState().groups[0].tabs.map((t) => t.path)).toEqual([
        "/canonical/a.ts"
    ])
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
    // fire-and-forget 後 warn 走 detached promise，等它落地再驗。
    await vi.waitFor(() =>
        expect(warnSpy).toHaveBeenCalledWith(
            "allow_workspace_asset_scope failed:",
            expect.any(Error)
        )
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

test("a superseded image-session restore never publishes a half-switched workspace", async () => {
    useWorkspaceStore.getState().setWorkspace("/current")
    saveWorkspaceSession({ workspacePath: "/canonical", tabs: ["/canonical/image.png"], activePath: "/canonical/image.png" })
    let finish!: () => void
    let current = true
    vi.mocked(allowWorkspaceAssetScope).mockImplementationOnce(() => new Promise<void>(resolve => { finish = resolve }))
    const opening = openWorkspaceAtPath("/restore", { skipUnsavedGuard: true, shouldOpen: () => current })
    await vi.waitFor(() => expect(finish).toBeDefined())
    expect(useWorkspaceStore.getState().workspacePath).toBe("/current")
    current = false
    finish()
    expect(await opening).toBe(false)
    expect(useWorkspaceStore.getState().workspacePath).toBe("/current")
    expect(startWatch).not.toHaveBeenCalled()
})

// A11b-F4：preflight 只看「當下」的 dirty；之後 await openWorkspace（遠端可能數秒）
// 期間新增的編輯，必須在 clearAll 丟棄舊工作區前再問一次。
const cleanWorkspace = () =>
    useWorkspaceStore.setState({
        workspacePath: "/old",
        activeGroupIndex: 0,
        groups: [
            {
                activePath: "/old/a.ts",
                tabs: [{ path: "/old/a.ts", name: "a.ts", dirty: false, externallyModified: false }]
            }
        ]
    })

function deferOpenWorkspace(): () => void {
    let finish!: (value: Awaited<ReturnType<typeof openWorkspace>>) => void
    vi.mocked(openWorkspace).mockImplementationOnce(
        () => new Promise((resolve) => { finish = resolve })
    )
    return () => finish({ canonicalPath: "/canonical", capabilityId: "ws-capability" })
}

test("preflight 通過後、openWorkspace 等待期間才變 dirty：clearAll 前重新詢問，cancel 保留舊工作區與編輯", async () => {
    cleanWorkspace()
    const finish = deferOpenWorkspace()
    const opening = openWorkspaceAtPath("/new")
    await vi.waitFor(() => expect(openWorkspace).toHaveBeenCalledWith("/new"))
    useWorkspaceStore.getState().markDirty("/old/a.ts", true)
    finish()
    await vi.waitFor(() => expect(useConfirmDialogStore.getState().pending).not.toBeNull())
    expect(useWorkspaceStore.getState().workspacePath).toBe("/old")
    useConfirmDialogStore.getState().respond("cancel")
    expect(await opening).toBe(false)
    expect(useWorkspaceStore.getState().workspacePath).toBe("/old")
    expect(useWorkspaceStore.getState().groups[0].tabs[0]).toMatchObject({ path: "/old/a.ts", dirty: true })
    expect(saveDirtyTab).not.toHaveBeenCalled()
    expect(startWatch).not.toHaveBeenCalled()
})

test("等待期間新增的 dirty：重新詢問選 save → 先存檔再切換", async () => {
    cleanWorkspace()
    const finish = deferOpenWorkspace()
    const opening = openWorkspaceAtPath("/new")
    await vi.waitFor(() => expect(openWorkspace).toHaveBeenCalledWith("/new"))
    useWorkspaceStore.getState().markDirty("/old/a.ts", true)
    finish()
    await vi.waitFor(() => expect(useConfirmDialogStore.getState().pending).not.toBeNull())
    useConfirmDialogStore.getState().respond("save")
    expect(await opening).toBe(true)
    expect(saveDirtyTab).toHaveBeenCalledWith("/old/a.ts")
    expect(useWorkspaceStore.getState().workspacePath).toBe("/canonical")
})

test("等待期間新增的 dirty：重新詢問選 discard → 只問一次就切換", async () => {
    cleanWorkspace()
    const finish = deferOpenWorkspace()
    const opening = openWorkspaceAtPath("/new")
    await vi.waitFor(() => expect(openWorkspace).toHaveBeenCalledWith("/new"))
    useWorkspaceStore.getState().markDirty("/old/a.ts", true)
    finish()
    await vi.waitFor(() => expect(useConfirmDialogStore.getState().pending).not.toBeNull())
    useConfirmDialogStore.getState().respond("discard")
    expect(await opening).toBe(true)
    expect(saveDirtyTab).not.toHaveBeenCalled()
    expect(useWorkspaceStore.getState().workspacePath).toBe("/canonical")
})

test("等待期間重新詢問後 shouldOpen 已失效：不切換工作區", async () => {
    cleanWorkspace()
    let current = true
    const finish = deferOpenWorkspace()
    const opening = openWorkspaceAtPath("/new", { shouldOpen: () => current })
    await vi.waitFor(() => expect(openWorkspace).toHaveBeenCalledWith("/new"))
    useWorkspaceStore.getState().markDirty("/old/a.ts", true)
    finish()
    await vi.waitFor(() => expect(useConfirmDialogStore.getState().pending).not.toBeNull())
    current = false
    useConfirmDialogStore.getState().respond("discard")
    expect(await opening).toBe(false)
    expect(useWorkspaceStore.getState().workspacePath).toBe("/old")
})

test("skipUnsavedGuard：呼叫端 preflight 之後新增的編輯同樣會被攔下", async () => {
    cleanWorkspace()
    // 呼叫端（HERDR 啟用路徑）preflight 通過當下的狀態；之後還有多趟 IPC。
    const unsavedBaseline = snapshotDirtyTabs()
    useWorkspaceStore.getState().markDirty("/old/a.ts", true)
    const finish = deferOpenWorkspace()
    const opening = openWorkspaceAtPath("/new", { skipUnsavedGuard: true, unsavedBaseline })
    await vi.waitFor(() => expect(openWorkspace).toHaveBeenCalledWith("/new"))
    finish()
    await vi.waitFor(() => expect(useConfirmDialogStore.getState().pending).not.toBeNull())
    useConfirmDialogStore.getState().respond("cancel")
    expect(await opening).toBe(false)
    expect(useWorkspaceStore.getState().workspacePath).toBe("/old")
})

test("skipUnsavedGuard：preflight 已選 discard 的 dirty 分頁不會被重複詢問", async () => {
    dirtyWorkspace()
    const unsavedBaseline = snapshotDirtyTabs()
    expect(await openWorkspaceAtPath("/new", { skipUnsavedGuard: true, unsavedBaseline })).toBe(true)
    expect(useConfirmDialogStore.getState().pending).toBeNull()
    expect(useWorkspaceStore.getState().workspacePath).toBe("/canonical")
})

test("skipUnsavedGuard 沒有帶 baseline：至少涵蓋進入之後新增的編輯", async () => {
    cleanWorkspace()
    const finish = deferOpenWorkspace()
    const opening = openWorkspaceAtPath("/new", { skipUnsavedGuard: true })
    await vi.waitFor(() => expect(openWorkspace).toHaveBeenCalledWith("/new"))
    useWorkspaceStore.getState().markDirty("/old/a.ts", true)
    finish()
    await vi.waitFor(() => expect(useConfirmDialogStore.getState().pending).not.toBeNull())
    useConfirmDialogStore.getState().respond("cancel")
    expect(await opening).toBe(false)
    expect(useWorkspaceStore.getState().workspacePath).toBe("/old")
})

test("skipUnsavedGuard 沒有帶 baseline：進入時已 dirty 的分頁視為呼叫端已處理", async () => {
    dirtyWorkspace()
    const opening = openWorkspaceAtPath("/new", { skipUnsavedGuard: true })
    await vi.waitFor(() => expect(useWorkspaceStore.getState().workspacePath).toBe("/canonical"))
    expect(useConfirmDialogStore.getState().pending).toBeNull()
    expect(await opening).toBe(true)
})
