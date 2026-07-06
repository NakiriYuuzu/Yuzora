/**
 * Central UI string table. Plain constants — no framework, no locale switching
 * (that is a future milestone); the nested categories reserve room for growth.
 * New user-facing copy migrates here progressively; today only the strings that
 * a call site reads through `strings` live here.
 */
export const strings = {
    git: {
        branchNamePlaceholder: "分支名稱…"
    },
    lsp: {
        quickFix: "快速修正",
        goToSymbolPlaceholder: "跳至符號…",
        workspaceSymbolsPlaceholder: "搜尋工作區符號…",
        symbolPickerTitle: "符號選擇器",
        noSymbols: "無符號"
    },
    terminal: {
        shellLabel: "Shell 設定",
        shellDescription: "Shell path 留空時，後端會依 $SHELL → getpwuid → 系統預設 shell 解析；填值時優先使用這個路徑。Default args 會隨終端機設定保存。",
        noSessions: "尚無終端機工作階段",
        emptyDescription: "開啟新的終端機，在這裡執行指令。"
    },
    preview: {
        devServerLabel: "Dev server 覆寫",
        devServerDescription: "需要手動指定時，這裡的 command 與 port 會成為 preview 啟動 dev server 的優先來源。",
        noDevServer: "無 dev server",
        emptyTitle: "啟動或連接 dev server",
        emptyDescription: "dev server 執行後，preview 會顯示在這裡。",
        start: "啟動 dev server",
        detecting: "偵測中…",
        noCandidates: "找不到可啟動的 dev server",
        settingsHint: "請在 Settings 的 Preview 區塊設定 dev server command 與 port。",
        portOccupied: "偵測到 port 已被使用",
        connectExisting: "連接現有 server",
        alternatePort: "替代 port",
        startChangedPort: "切換 port 後啟動",
        retryDetect: "重新偵測",
        failedTitle: "dev server 啟動失敗",
        retryStart: "重試啟動",
        exitedTitle: "dev server 已停止"
    }
} as const

export type Strings = typeof strings
