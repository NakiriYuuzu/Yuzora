import { bufferContent } from "@/editor/documentRegistry"
import { saveDirtyTab } from "@/editor/saveDocument"
import { getView } from "@/editor/viewRegistry"
import { isFileTab } from "@/lib/markdownPreviewTab"
import { useConfirmDialogStore } from "@/state/confirmDialogStore"
import { useWorkspaceStore } from "@/state/workspaceStore"

/**
 * 目前所有 group 中「未儲存」分頁的去重路徑清單。preview 分頁沒有可寫入的
 * 檔案內容，永遠不計入。
 */
export function dirtyTabPaths(): string[] {
    return [
        ...new Set(
            useWorkspaceStore
                .getState()
                .groups.flatMap((g) => g.tabs)
                .filter((tab) => isFileTab(tab) && tab.dirty)
                .map((tab) => tab.path)
        )
    ]
}

type UnsavedLabels = {
    title: string
    description: string
    saveLabel: string
}

/**
 * 破壞性動作（切換工作區、關閉視窗）前的共用 dirty gate：沒有未儲存分頁就直接
 * 放行；否則彈出共用的確認對話框。macOS ⌘Q 攔不到，原因見 AppShell 的 close
 * handler 註解。
 *
 * 回傳 true 代表「可以繼續進行破壞性動作」；false 代表使用者取消，或選了儲存
 * 但其中一個檔案寫入失敗（例如 mixed-EOL 被擋下）、或儲存期間又有新的編輯——
 * 此時內容仍留在記憶體，呼叫端必須中止動作，否則使用者的編輯會永久消失。
 */
export function confirmDiscardingUnsaved(labels: UnsavedLabels): Promise<boolean> {
    return confirmDiscardingPaths(dirtyTabPaths, labels)
}

/**
 * 使用者已回覆過的未存狀態：每個 dirty 路徑連同當下的編輯內容。preflight 通過後
 * 到實際丟棄分頁之間可能隔著遠端 I/O 或排隊的 HERDR focus，期間仍可輸入。
 */
export type DirtyTabsSnapshot = ReadonlyMap<string, string | undefined>

/** live 編輯器的內容；未掛載時取卸載時寫回 registry 的 buffer。 */
function editedContent(path: string): string | undefined {
    return getView(path)?.state.doc.toString() ?? bufferContent(path)
}

export function snapshotDirtyTabs(): DirtyTabsSnapshot {
    return new Map(dirtyTabPaths().map((path) => [path, editedContent(path)]))
}

/**
 * 目前 dirty、但 snapshot 沒涵蓋的路徑：新變 dirty，或內容自 snapshot 後被編輯過。
 * 兩邊都拿不到內容才視為相同；只有一邊拿得到時算未涵蓋（fail-closed）。
 */
export function dirtyPathsSince(snapshot: DirtyTabsSnapshot): string[] {
    return dirtyTabPaths().filter((path) => !snapshot.has(path) || snapshot.get(path) !== editedContent(path))
}

/** 只詢問 snapshot 之後才出現的未存編輯；沒有就直接放行。 */
export function confirmDiscardingUnsavedSince(
    snapshot: DirtyTabsSnapshot,
    labels: UnsavedLabels
): Promise<boolean> {
    return confirmDiscardingPaths(() => dirtyPathsSince(snapshot), labels)
}

async function confirmDiscardingPaths(pendingPaths: () => string[], labels: UnsavedLabels): Promise<boolean> {
    const dirtyPaths = pendingPaths()
    if (dirtyPaths.length === 0) return true

    const decision = await useConfirmDialogStore.getState().requestUnsavedDecision(labels)
    if (decision === "cancel") return false
    if (decision === "save") {
        for (const dirtyPath of dirtyPaths) {
            const outcome = await saveDirtyTab(dirtyPath)
            if (outcome.kind !== "saved") return false
        }
        // 儲存期間編輯器仍可輸入：之後才變 dirty 的檔案還沒寫入，不能放行。
        if (pendingPaths().length > 0) return false
    }
    return true
}
