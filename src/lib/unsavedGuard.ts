import { saveDirtyTab } from "@/editor/saveDocument"
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

/**
 * 破壞性動作（切換工作區、關閉視窗）前的共用 dirty gate：沒有未儲存分頁就直接
 * 放行；否則彈出共用的確認對話框。macOS ⌘Q 攔不到，原因見 AppShell 的 close
 * handler 註解。
 *
 * 回傳 true 代表「可以繼續進行破壞性動作」；false 代表使用者取消，或選了儲存
 * 但其中一個檔案寫入失敗（例如 mixed-EOL 被擋下）——此時內容仍留在記憶體，
 * 呼叫端必須中止動作，否則使用者的編輯會永久消失。
 */
export async function confirmDiscardingUnsaved(labels: {
    title: string
    description: string
    saveLabel: string
}): Promise<boolean> {
    const dirtyPaths = dirtyTabPaths()
    if (dirtyPaths.length === 0) return true

    const decision = await useConfirmDialogStore.getState().requestUnsavedDecision(labels)
    if (decision === "cancel") return false
    if (decision === "save") {
        for (const dirtyPath of dirtyPaths) {
            const outcome = await saveDirtyTab(dirtyPath)
            if (outcome.kind !== "saved") return false
        }
    }
    return true
}
