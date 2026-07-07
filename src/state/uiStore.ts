import { create } from "zustand"

import { DEFAULT_MODE, type Mode } from "@/app/modes"

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
    openDiffInGitMode: (path) =>
        set({ mode: "git", gitSelectedPath: path, gitSelectedStaged: false }),
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
