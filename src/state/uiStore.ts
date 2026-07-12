import { create } from "zustand"

import { DEFAULT_MODE, type Mode } from "@/app/modes"
import {
    currentGitChange,
    sameGitChange,
    type GitChangeKey,
    type GitChangeRow
} from "@/workbench/git/gitChangeSelection"

type SettingsTargetOptions = {
    language?: string
    source?: string
}

interface UiState {
    mode: Mode
    setMode: (mode: Mode) => void
    gitSelectedPath: string | null            // GitPanel Local changes 目前選中檔
    gitSelectedStaged: boolean                // 選中的是 staged 節或 changes 節
    selectGitFile: (path: string | null, staged: boolean) => void
    gitChangeSelection: GitChangeKey[]
    gitChangePrimary: GitChangeKey | null
    gitChangeAnchor: GitChangeKey | null
    selectGitChange: (
        key: GitChangeKey,
        order: readonly GitChangeRow[],
        mode: "single" | "toggle" | "range"
    ) => void
    ensureGitChangeContextSelection: (key: GitChangeKey) => void
    reconcileGitChangeSelection: (
        rows: readonly GitChangeRow[],
        movedSides?: Readonly<Record<string, boolean>>
    ) => void
    openDiffInGitMode: (path: string) => void // FileTree 入口：selectGitFile+setMode("git")
    resolverPath: string | null               // external-change 解決器目標檔
    openResolver: (path: string) => void
    closeResolver: () => void
    settingsOpen: boolean                      // Settings 對話框開關（單一真相，AppShell 訂閱）
    settingsSection: string | null             // 開啟時鎖定的 section（null＝預設/記憶上次）
    settingsLanguage: string | null            // LSP section 要聚焦的語言（null＝不指定）
    settingsLogSource: string | null           // Logs section 要預填的 source（null＝全部 sources）
    // 每次 openSettings ++1。相同 target 連開（primitive 不變、zustand 短路）時，用它讓
    // SettingsDialog 的 sync effect 仍能重新觸發，把手動切走的 section 拉回目標。
    settingsNonce: number
    // 開啟 Settings；帶參數可直接跳至某 section／LSP 語言（openSettings("lsp","python")）
    // 或預填 Logs source（openSettings("logs",{ source:"lsp" })）。
    // 無參數＝一般開啟（rail avatar／CommandPalette），不指定 section 語言。
    openSettings: (section?: string, target?: string | SettingsTargetOptions) => void
    setSettingsOpen: (open: boolean) => void
    terminalOpen: boolean
    toggleTerminal: () => void
    traceEnabled: boolean                      // LSP JSON-RPC trace（in-memory，不持久化；重啟＝off）
    setTraceEnabled: (enabled: boolean) => void
    // navCollapsed／command palette open 是 AppShell 的 local state（非 store），
    // context menu 的 dispatch 活在 React 樹外，構不到它們。這兩個 nonce 只是信號：
    // 每次呼叫 ++1，AppShell 用 ref 比對前值、變了就代它們的 local setter 動作
    // （同 settingsNonce 的作法）。
    sidebarToggleRequest: number
    requestSidebarToggle: () => void
    paletteOpenRequest: number
    requestOpenPalette: () => void
}

// Exported so the test setup can reset the store between tests (zustand stores
// persist across the module graph, so state leaks otherwise).
export const uiInitialState = {
    mode: DEFAULT_MODE,
    gitSelectedPath: null,
    gitSelectedStaged: false,
    gitChangeSelection: [] as GitChangeKey[],
    gitChangePrimary: null as GitChangeKey | null,
    gitChangeAnchor: null as GitChangeKey | null,
    resolverPath: null,
    settingsOpen: false,
    settingsSection: null,
    settingsLanguage: null,
    settingsLogSource: null,
    settingsNonce: 0,
    terminalOpen: false,
    traceEnabled: false,
    sidebarToggleRequest: 0,
    paletteOpenRequest: 0
}

export const useUiStore = create<UiState>()((set) => ({
    ...uiInitialState,
    setMode: (mode) => set({ mode }),
    selectGitFile: (path, staged) =>
        set({ gitSelectedPath: path, gitSelectedStaged: staged }),
    selectGitChange: (key, order, mode) =>
        set((state) => {
            const current = state.gitChangeSelection
            let selection: GitChangeKey[]
            let primary: GitChangeKey | null = key
            let anchor: GitChangeKey | null = key

            if (mode === "toggle") {
                const exists = current.some((candidate) => sameGitChange(candidate, key))
                selection = exists
                    ? current.filter((candidate) => !sameGitChange(candidate, key))
                    : [...current, key]
                if (exists && sameGitChange(state.gitChangePrimary, key)) {
                    primary = selection.at(-1) ?? null
                } else if (exists) {
                    primary = state.gitChangePrimary
                }
            } else if (mode === "range") {
                const rangeAnchor = currentGitChange(
                    state.gitChangeAnchor ?? state.gitChangePrimary ?? key,
                    order
                ) ?? key
                const from = order.findIndex((candidate) => sameGitChange(candidate, rangeAnchor))
                const to = order.findIndex((candidate) => sameGitChange(candidate, key))
                selection = from === -1 || to === -1
                    ? [key]
                    : order.slice(Math.min(from, to), Math.max(from, to) + 1)
                anchor = rangeAnchor
            } else {
                selection = [key]
            }

            return {
                gitChangeSelection: selection,
                gitChangePrimary: primary,
                gitChangeAnchor: anchor,
                gitSelectedPath: primary?.path ?? null,
                gitSelectedStaged: primary?.staged ?? false
            }
        }),
    ensureGitChangeContextSelection: (key) =>
        set((state) => {
            if (state.gitChangeSelection.some((candidate) => sameGitChange(candidate, key))) {
                return state
            }
            return {
                gitChangeSelection: [key],
                gitChangePrimary: key,
                gitChangeAnchor: key,
                gitSelectedPath: key.path,
                gitSelectedStaged: key.staged
            }
        }),
    reconcileGitChangeSelection: (rows, movedSides = {}) =>
        set((state) => {
            const remap = (key: GitChangeKey | null): GitChangeRow | null => {
                if (!key) return null
                const desiredSide = Object.prototype.hasOwnProperty.call(movedSides, key.path)
                    ? movedSides[key.path]
                    : key.staged
                return rows.find((row) => row.path === key.path && row.staged === desiredSide)
                    ?? rows.find((row) => sameGitChange(row, key))
                    ?? rows.find((row) => row.path === key.path)
                    ?? null
            }
            const seen = new Set<string>()
            const selection = state.gitChangeSelection.flatMap((key) => {
                const row = remap(key)
                if (!row) return []
                const id = `${row.staged ? "s" : "c"}:${row.path}`
                if (seen.has(id)) return []
                seen.add(id)
                return [row]
            })
            const primary = remap(state.gitChangePrimary)
                ?? (state.gitChangePrimary ? selection[0] ?? null : null)
            const anchor = remap(state.gitChangeAnchor) ?? primary
            const diff = primary ?? (state.gitSelectedPath
                ? rows.find((row) => row.path === state.gitSelectedPath && row.staged === (
                    Object.prototype.hasOwnProperty.call(movedSides, state.gitSelectedPath)
                        ? movedSides[state.gitSelectedPath]
                        : state.gitSelectedStaged
                )) ?? rows.find((row) => row.path === state.gitSelectedPath) ?? null
                : null)
            return {
                gitChangeSelection: selection,
                gitChangePrimary: primary,
                gitChangeAnchor: anchor,
                gitSelectedPath: diff?.path ?? null,
                gitSelectedStaged: diff?.staged ?? false
            }
        }),
    openDiffInGitMode: (path) =>
        set({
            mode: "git",
            gitSelectedPath: path,
            gitSelectedStaged: false,
            gitChangeSelection: [],
            gitChangePrimary: null,
            gitChangeAnchor: null
        }),
    openResolver: (path) => set({ resolverPath: path }),
    closeResolver: () => set({ resolverPath: null }),
    openSettings: (section, target) =>
        set((s) => {
            const language = typeof target === "string" ? target : target?.language
            const source = typeof target === "object" ? target.source : undefined
            return {
                settingsOpen: true,
                settingsSection: section ?? null,
                settingsLanguage: language ?? null,
                settingsLogSource: source ?? null,
                settingsNonce: s.settingsNonce + 1
            }
        }),
    setSettingsOpen: (open) => set({ settingsOpen: open }),
    toggleTerminal: () => set((s) => ({ terminalOpen: !s.terminalOpen })),
    setTraceEnabled: (enabled) => set({ traceEnabled: enabled }),
    requestSidebarToggle: () => set((s) => ({ sidebarToggleRequest: s.sidebarToggleRequest + 1 })),
    requestOpenPalette: () => set((s) => ({ paletteOpenRequest: s.paletteOpenRequest + 1 }))
}))
