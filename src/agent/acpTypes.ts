export interface TranscriptAction {
    label: string
    kind: string
    payload?: unknown
}

export interface MsgEntry {
    who: "you" | "agent"
    text: string
    streaming: boolean
    // 使用者訊息附帶的圖片縮圖（in-memory only）：不寫入 Session Index 或任何
    // localStorage 持久化（ADR 0001／plan C2）；restored replay 的重現依賴 agent。
    images?: { mimeType: string; dataUrl: string }[]
}

export interface BlockEntry {
    kind: "tool" | "diff" | "perm" | "error" | "plan" | "thought" | "notice"
    text: string
    meta?: string
    actions?: TranscriptAction[]
}

export type TranscriptEntry = MsgEntry | BlockEntry
