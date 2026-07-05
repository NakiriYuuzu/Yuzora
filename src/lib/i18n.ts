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
    }
} as const

export type Strings = typeof strings
