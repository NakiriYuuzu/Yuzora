import { afterEach, beforeEach, expect, test, vi } from "vitest"
import { Text } from "@codemirror/state"
import type { EditorView } from "@codemirror/view"

vi.mock("@/editor/saveDocument", () => ({ saveDirtyTab: vi.fn() }))
vi.mock(import("@/editor/documentRegistry"), async (importOriginal) => ({
    ...(await importOriginal()),
    bufferContent: vi.fn()
}))

import { bufferContent } from "@/editor/documentRegistry"
import { saveDirtyTab } from "@/editor/saveDocument"
import { registerView, unregisterView } from "@/editor/viewRegistry"
import {
    confirmDiscardingUnsaved,
    confirmDiscardingUnsavedSince,
    dirtyPathsSince,
    dirtyTabPaths,
    snapshotDirtyTabs
} from "@/lib/unsavedGuard"
import { useConfirmDialogStore } from "@/state/confirmDialogStore"
import { markdownPreviewPath } from "@/lib/markdownPreviewTab"
import { PREVIEW_TAB_PATH, useWorkspaceStore } from "@/state/workspaceStore"

const LABELS = { title: "t", description: "d", saveLabel: "s" }

const tab = (path: string, dirty: boolean, kind?: "file" | "preview" | "markdown-preview", sourcePath?: string) => ({
    path,
    name: path,
    dirty,
    externallyModified: false,
    ...(kind ? { kind } : {}),
    ...(sourcePath ? { sourcePath } : {})
})

beforeEach(() => {
    // 跟真正的存檔一樣：寫入後分頁變回乾淨。
    vi.mocked(saveDirtyTab).mockReset().mockImplementation(async (path) => {
        useWorkspaceStore.getState().markDirty(path, false)
        return { kind: "saved" }
    })
    vi.mocked(bufferContent).mockReset()
    useConfirmDialogStore.setState({ pending: null })
    useWorkspaceStore.setState({
        workspacePath: "/w",
        activeGroupIndex: 0,
        groups: [{ activePath: null, tabs: [] }]
    })
})

afterEach(() => {
    useConfirmDialogStore.setState({ pending: null })
})

test("dirtyTabPaths：跨 group 去重、排除乾淨分頁與 preview 分頁", () => {
    useWorkspaceStore.setState({
        activeGroupIndex: 0,
        groups: [
            { activePath: null, tabs: [tab("/w/a.ts", true), tab("/w/clean.ts", false)] },
            {
                activePath: null,
                tabs: [
                    tab("/w/a.ts", true),
                    tab("/w/b.ts", true),
                    tab(PREVIEW_TAB_PATH, true, "preview"),
                    tab(markdownPreviewPath("/w/r.md"), true, "markdown-preview", "/w/r.md")
                ]
            }
        ]
    })

    expect(dirtyTabPaths()).toEqual(["/w/a.ts", "/w/b.ts"])
})

test("沒有 dirty 分頁：不彈對話框、直接放行", async () => {
    expect(await confirmDiscardingUnsaved(LABELS)).toBe(true)
    expect(useConfirmDialogStore.getState().pending).toBeNull()
    expect(saveDirtyTab).not.toHaveBeenCalled()
})

test("cancel：回傳 false 且不存檔", async () => {
    useWorkspaceStore.setState({
        activeGroupIndex: 0,
        groups: [{ activePath: null, tabs: [tab("/w/a.ts", true)] }]
    })

    const pending = confirmDiscardingUnsaved(LABELS)
    expect(useConfirmDialogStore.getState().pending).toMatchObject(LABELS)
    useConfirmDialogStore.getState().respond("cancel")

    expect(await pending).toBe(false)
    expect(saveDirtyTab).not.toHaveBeenCalled()
})

test("discard：不存檔但放行", async () => {
    useWorkspaceStore.setState({
        activeGroupIndex: 0,
        groups: [{ activePath: null, tabs: [tab("/w/a.ts", true)] }]
    })

    const pending = confirmDiscardingUnsaved(LABELS)
    useConfirmDialogStore.getState().respond("discard")

    expect(await pending).toBe(true)
    expect(saveDirtyTab).not.toHaveBeenCalled()
})

test("save：全部存檔成功才放行", async () => {
    useWorkspaceStore.setState({
        activeGroupIndex: 0,
        groups: [{ activePath: null, tabs: [tab("/w/a.ts", true), tab("/w/b.ts", true)] }]
    })

    const pending = confirmDiscardingUnsaved(LABELS)
    useConfirmDialogStore.getState().respond("save")

    expect(await pending).toBe(true)
    expect(saveDirtyTab).toHaveBeenCalledWith("/w/a.ts")
    expect(saveDirtyTab).toHaveBeenCalledWith("/w/b.ts")
})

// issue #21 的原始情境：mixed-EOL 檔案被儲存檢查擋下。存檔失敗仍放行等於直接
// 丟掉使用者內容，所以這裡必須回 false，且不再嘗試後續檔案。
test("save：任一檔案儲存失敗即中止，不再存後續檔案", async () => {
    useWorkspaceStore.setState({
        activeGroupIndex: 0,
        groups: [{ activePath: null, tabs: [tab("/w/a.ts", true), tab("/w/b.ts", true)] }]
    })
    vi.mocked(saveDirtyTab).mockResolvedValueOnce({ kind: "blocked", reason: "mixed" })

    const pending = confirmDiscardingUnsaved(LABELS)
    useConfirmDialogStore.getState().respond("save")

    expect(await pending).toBe(false)
    expect(saveDirtyTab).toHaveBeenCalledTimes(1)
    expect(saveDirtyTab).toHaveBeenCalledWith("/w/a.ts")
})

test("save：I/O 失敗同樣不放行", async () => {
    useWorkspaceStore.setState({
        activeGroupIndex: 0,
        groups: [{ activePath: null, tabs: [tab("/w/a.ts", true)] }]
    })
    vi.mocked(saveDirtyTab).mockResolvedValue({ kind: "failed" })

    const pending = confirmDiscardingUnsaved(LABELS)
    useConfirmDialogStore.getState().respond("save")

    expect(await pending).toBe(false)
})

test("save：檔案在寫入期間被編輯（changed）時不放行", async () => {
    useWorkspaceStore.setState({
        activeGroupIndex: 0,
        groups: [{ activePath: null, tabs: [tab("/w/a.ts", true)] }]
    })
    vi.mocked(saveDirtyTab).mockResolvedValueOnce({ kind: "changed" })

    const pending = confirmDiscardingUnsaved(LABELS)
    useConfirmDialogStore.getState().respond("save")

    expect(await pending).toBe(false)
})

// preflight 之後的編輯偵測（A11b-F4）。node 環境沒有 DOM，用只帶 state.doc 的假 view
// 代表掛載中的編輯器。
const fakeView = (text: string) => ({ state: { doc: Text.of([text]) } }) as unknown as EditorView

function seedDirty(...paths: string[]) {
    useWorkspaceStore.setState({
        activeGroupIndex: 0,
        groups: [{ activePath: null, tabs: paths.map((path) => tab(path, true)) }]
    })
}

test("dirtyPathsSince：snapshot 之後沒有變動就沒有未涵蓋的路徑", () => {
    seedDirty("/w/a.ts")
    expect(dirtyPathsSince(snapshotDirtyTabs())).toEqual([])
})

test("dirtyPathsSince：snapshot 之後才變 dirty 的路徑會被列出", () => {
    seedDirty("/w/a.ts")
    const snapshot = snapshotDirtyTabs()
    seedDirty("/w/a.ts", "/w/b.ts")
    expect(dirtyPathsSince(snapshot)).toEqual(["/w/b.ts"])
})

test("dirtyPathsSince：已回覆過的 dirty 路徑，live 文件被再編輯就重新列出", () => {
    seedDirty("/w/a.ts")
    const view = fakeView("one")
    registerView("/w/a.ts", view)
    try {
        const snapshot = snapshotDirtyTabs()
        expect(dirtyPathsSince(snapshot)).toEqual([])
        ;(view as unknown as { state: { doc: Text } }).state = { doc: Text.of(["two"]) }
        expect(dirtyPathsSince(snapshot)).toEqual(["/w/a.ts"])
    } finally {
        unregisterView("/w/a.ts", view)
    }
})

test("dirtyPathsSince：編輯器卸載後拿不到內容而無法比對 → 保守視為未涵蓋", () => {
    seedDirty("/w/a.ts")
    const view = fakeView("one")
    registerView("/w/a.ts", view)
    const snapshot = snapshotDirtyTabs()
    unregisterView("/w/a.ts", view)
    expect(dirtyPathsSince(snapshot)).toEqual(["/w/a.ts"])
})

test("dirtyPathsSince：回覆時未掛載、之後被打開編輯再切走 → 依 buffer 內容重新列出", () => {
    seedDirty("/w/a.ts")
    vi.mocked(bufferContent).mockReturnValue("one")
    const snapshot = snapshotDirtyTabs()
    vi.mocked(bufferContent).mockReturnValue("two")
    expect(dirtyPathsSince(snapshot)).toEqual(["/w/a.ts"])
})

test("dirtyPathsSince：回覆時開著、之後只是切走沒有輸入 → 不重問", () => {
    seedDirty("/w/a.ts")
    const view = fakeView("one")
    registerView("/w/a.ts", view)
    const snapshot = snapshotDirtyTabs()
    unregisterView("/w/a.ts", view)
    vi.mocked(bufferContent).mockReturnValue("one")
    expect(dirtyPathsSince(snapshot)).toEqual([])
})

test("confirmDiscardingUnsavedSince：沒有新的編輯就不彈對話框", async () => {
    seedDirty("/w/a.ts")
    expect(await confirmDiscardingUnsavedSince(snapshotDirtyTabs(), LABELS)).toBe(true)
    expect(useConfirmDialogStore.getState().pending).toBeNull()
})

test("confirmDiscardingUnsavedSince：只詢問、只存 snapshot 之後新增的檔案", async () => {
    seedDirty("/w/a.ts")
    const snapshot = snapshotDirtyTabs()
    seedDirty("/w/a.ts", "/w/b.ts")

    const pending = confirmDiscardingUnsavedSince(snapshot, LABELS)
    expect(useConfirmDialogStore.getState().pending).toMatchObject(LABELS)
    useConfirmDialogStore.getState().respond("save")

    expect(await pending).toBe(true)
    expect(saveDirtyTab).toHaveBeenCalledTimes(1)
    expect(saveDirtyTab).toHaveBeenCalledWith("/w/b.ts")
})
