import { invoke } from "@/lib/ipc"
import { useWorkspaceStore } from "@/state/workspaceStore"

// issue #40 AC 第 2 條：user action 必須帶「正確的 structured workspace」或
// 「明確的 null reason」。在這之前 workspace_path 恆為 null（63/63 筆），使得
// 沒有任何一筆 user action 能被歸屬到工作區。
//
// workspace 由**預設參數**從 workspaceStore 取得，所以 46 個既有呼叫點一行都不
// 用改就自動帶上正確的值；需要覆寫的呼叫端（例如記錄的是另一個工作區的動作）
// 才顯式傳入。
export const NO_WORKSPACE_REASON = "no_workspace_open"

function currentWorkspacePath(): string | null {
    // getState() 而非 hook：logUserAction 會從 event handler、effect、甚至非
    // React 的模組（agentRouter 等）呼叫，不能綁 render 週期。
    return useWorkspaceStore.getState().workspacePath
}

export function logUserAction(
    event: string,
    message: string,
    metadata: Record<string, unknown> = {},
    workspacePath: string | null = currentWorkspacePath()
): Promise<void> {
    // 沒有開啟工作區（啟動後尚未開專案、或動作本來就與工作區無關）時，
    // workspace_path 仍是 null，但 metadata 會說明「為什麼是 null」——AC 要的
    // 是「正確的值**或**明確的 null reason」，而不是無聲的 null。
    const enriched =
        workspacePath === null
            ? { ...metadata, workspace_null_reason: NO_WORKSPACE_REASON }
            : metadata
    return invoke("log_event", {
        event: {
            level: "info",
            kind: "user_action",
            source: "ui",
            workspace_path: workspacePath,
            event,
            message,
            metadata: enriched
        }
    })
        .then(() => undefined)
        .catch(() => undefined)
}
