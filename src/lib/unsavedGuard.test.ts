import { afterEach, beforeEach, expect, test, vi } from "vitest"

vi.mock("@/editor/saveDocument", () => ({ saveDirtyTab: vi.fn() }))

import { saveDirtyTab } from "@/editor/saveDocument"
import { confirmDiscardingUnsaved, dirtyTabPaths } from "@/lib/unsavedGuard"
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
    vi.mocked(saveDirtyTab).mockReset().mockResolvedValue({ kind: "saved" })
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
