import { create } from "zustand"

import type { FileGrade, LspDisplayState, LspServerInfo } from "../lib/types"
import { lspLanguageOf } from "../lib/types"

// Derives the status-bar / Settings display state from the three inputs the UI
// combines: the Rust process status, whether the client has finished the
// initialize handshake, and the file's LSP grade.
//   - non-full grade (limited / very long line / …) → syntaxOnly (LSP not mounted)
//   - process missing  → missing
//   - process crashed  → failed
//   - initialized      → ready
//   - otherwise        → starting
export function deriveDisplayState(
    info: LspServerInfo | null,
    initialized: boolean,
    grade: FileGrade
): LspDisplayState {
    if (grade !== "full") return "syntaxOnly"
    if (info) {
        if (info.status.status === "missing") return "missing"
        if (info.status.status === "crashed") return "failed"
        // A deliberately stopped process (profile switch, manual stop) has no
        // active LSP even if a prior initialize left `initialized` true. Report
        // syntaxOnly (honest current capability); the next file mount restarts it.
        if (info.status.status === "stopped") return "syntaxOnly"
    }
    if (initialized) return "ready"
    return "starting"
}

interface LspState {
    servers: Record<string, LspServerInfo> // key = language
    initialized: Record<string, boolean> // language → initialize response received
    setServerInfo: (info: LspServerInfo) => void
    setInitialized: (language: string, ok: boolean) => void
    displayFor: (path: string, grade: FileGrade) => { language: string; serverId: string; state: LspDisplayState }
    reset: () => void
}

const lspInitialState = {
    servers: {} as Record<string, LspServerInfo>,
    initialized: {} as Record<string, boolean>
}

export const useLspStore = create<LspState>()((set, get) => ({
    ...lspInitialState,

    setServerInfo: (info) =>
        set((s) => ({ servers: { ...s.servers, [info.language]: info } })),

    setInitialized: (language, ok) =>
        set((s) => ({ initialized: { ...s.initialized, [language]: ok } })),

    displayFor: (path, grade) => {
        const language = lspLanguageOf(path)
        if (!language) return { language: "", serverId: "", state: "syntaxOnly" }
        const info = get().servers[language] ?? null
        const initialized = get().initialized[language] ?? false
        return {
            language,
            serverId: info?.serverId ?? "",
            state: deriveDisplayState(info, initialized, grade)
        }
    },

    reset: () => set({ servers: {}, initialized: {} })
}))
