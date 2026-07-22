// parent（ACP server）↔ session host（每 session 一個子行程）的 IPC 訊息型別，
// 以及兩邊共用的小工具。ACP wire 本身是手寫 JSON-RPC（嚴格 LF JSONL）——
// fixture 契約要求不經 SDK、不引入 readline（pi 官方文件明示 readline 會壞 JSONL）。

export interface AcpPermissionOption {
    optionId: string
    name: string
    kind: "allow_once" | "allow_always" | "reject_once" | "reject_always"
}

export type ElicitationAction = "accept" | "decline" | "cancel"

/** parent → session host */
export type ParentToHost =
    | {
        t: "init"
        cwd: string
        /** client initialize 宣告了 elicitation.form capability（P3）。 */
        formElicitation: boolean
        /** session/load 時的目標 pi session id；child 以 SessionManager.list(cwd) 解析檔案。 */
        loadSessionId?: string
        /** restore-on-demand 已知 session file（跳過 list 掃描）。 */
        sessionFile?: string
    }
    | { t: "prompt"; id: number; text: string; images: { mimeType: string; data: string }[] }
    | { t: "cancel" }
    | { t: "set-config"; id: number; configId: string; value: string }
    | { t: "replay"; id: number }
    /** 於 new/load 回應送出後觸發 startupInfo chunk 與 available_commands_update。 */
    | { t: "announce" }
    | { t: "dialog-answer"; reqId: number; optionId: string | null }
    | { t: "elicit-answer"; reqId: number; action: ElicitationAction; content?: Record<string, unknown> }
    | { t: "shutdown" }

/** session host → parent */
export type HostToParent =
    | {
        t: "ready"
        sessionId: string
        sessionFile: string | null
        configOptions: unknown[]
        models: unknown
        modes: unknown
        startupInfo: string | null
    }
    | { t: "init-error"; code: "auth_required" | "invalid_params" | "internal"; message: string }
    /** ACP session/update 的 update 部分（parent 補 sessionId 包成 notification）。 */
    | { t: "update"; update: Record<string, unknown> }
    | { t: "result"; id: number; ok: true; data?: unknown }
    | { t: "result"; id: number; ok: false; authRequired?: boolean; invalidParams?: boolean; message: string }
    | {
        t: "dialog"
        reqId: number
        options: AcpPermissionOption[]
        toolCall: Record<string, unknown>
    }
    /** ACP elicitation/create（form mode）請求；parent 補 sessionId 後轉發 client。 */
    | {
        t: "elicit"
        reqId: number
        message: string
        requestedSchema: Record<string, unknown>
        meta?: Record<string, unknown>
    }
    | { t: "log"; message: string }

export function asRecord(value: unknown): Record<string, unknown> {
    return value !== null && typeof value === "object" ? value as Record<string, unknown> : {}
}

/** 嚴格 LF 分隔的 JSONL 讀取器（不使用 readline）。 */
export function createLineSplitter(onLine: (line: string) => void): (chunk: string) => void {
    let buffer = ""
    return (chunk: string) => {
        buffer += chunk
        const lines = buffer.split("\n")
        buffer = lines.pop() ?? ""
        for (const line of lines) {
            const trimmed = line.trim()
            if (trimmed !== "") onLine(trimmed)
        }
    }
}
