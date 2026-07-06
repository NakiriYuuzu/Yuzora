export interface TranscriptAction {
    label: string
    kind: string
    payload?: unknown
}

export interface MsgEntry {
    who: "you" | "agent"
    text: string
    streaming: boolean
}

export interface BlockEntry {
    kind: "tool" | "diff" | "perm" | "error" | "plan"
    text: string
    meta?: string
    actions?: TranscriptAction[]
}

export type TranscriptEntry = MsgEntry | BlockEntry
