import type { ContextMenuKind } from "@/state/contextMenuStore"

export interface ContextMenuItem {
  id: string
  label: string
  danger?: boolean
}

export type ContextMenuEntry = ContextMenuItem | "separator"

// 各區域選單內容 — 對照設計文件 ctxDefs（Yuuzu Workbench.dc.html L3528）。
// item id 沿用設計的 cm* key，能力賦予階段可與設計的 ctxRun 1:1 對映。
// sshhost 目前沒有掛載點（host 列 UI 尚未存在），先進 registry，SSH 里程碑
// 只需要在 host 列補一行 handler。
export const CONTEXT_MENU_DEFS: Record<ContextMenuKind, ContextMenuEntry[]> = {
  general: [
    { id: "cmCmdPalette", label: "Command palette…" },
    { id: "cmRefresh", label: "Refresh" },
    "separator",
    { id: "cmSettings", label: "Settings…" },
    { id: "cmHideSidebar", label: "Toggle sidebar" },
  ],
  rail: [
    { id: "cmNewProject", label: "New project" },
    { id: "cmSettings", label: "Settings…" },
    "separator",
    { id: "cmHideSidebar", label: "Toggle sidebar" },
  ],
  explorer: [
    { id: "cmNewFile", label: "New file" },
    { id: "cmNewFolder", label: "New folder" },
    "separator",
    { id: "cmRefresh", label: "Refresh" },
    { id: "cmCopyPath", label: "Copy path" },
  ],
  file: [
    { id: "cmOpen", label: "Open" },
    { id: "cmOpenSplit", label: "Open to the side" },
    "separator",
    { id: "cmRename", label: "Rename…" },
    { id: "cmCopyRel", label: "Copy relative path" },
    { id: "cmReveal", label: "Reveal in Finder" },
    "separator",
    { id: "cmDelete", label: "Delete", danger: true },
  ],
  tab: [
    { id: "cmCloseTab", label: "Close tab" },
    { id: "cmCloseOthers", label: "Close others" },
    { id: "cmCloseAll", label: "Close all" },
    "separator",
    { id: "cmCopyRel", label: "Copy relative path" },
    { id: "cmSplit", label: "Split editor" },
  ],
  editor: [
    { id: "cmCut", label: "Cut" },
    { id: "cmCopy", label: "Copy" },
    { id: "cmPaste", label: "Paste" },
    "separator",
    // Diff viewer entry (design openDiffCurrent). Opens the active editor file's
    // working-tree diff in the Diff modal; the dispatch no-ops when the file has
    // no git changes (contextMenuStore.runContextMenuAction).
    { id: "cmCompareHead", label: "Compare with HEAD" },
    { id: "cmFormatDoc", label: "Format document" },
    { id: "cmCmdPalette", label: "Command palette…" },
  ],
  terminal: [
    { id: "cmCopySel", label: "Copy" },
    { id: "cmPaste", label: "Paste" },
    "separator",
    // 設計在 terminal 停靠進編輯器時把這項換成 "Dock to bottom"；dock 功能
    // 尚未存在，唯一可達狀態是 drawer 側文案。
    { id: "cmDockTerm", label: "Split with code" },
    { id: "cmSplitTermRight", label: "Split right" },
    { id: "cmSplitTermDown", label: "Split down" },
    { id: "cmClear", label: "Clear terminal" },
    { id: "cmKill", label: "Kill process", danger: true },
  ],
  agent: [
    { id: "cmStop", label: "Stop session" },
    { id: "cmDuplicate", label: "Duplicate" },
    { id: "cmRenameSession", label: "Rename session…" },
    "separator",
    { id: "cmCopyPath", label: "Copy path" },
  ],
  git: [
    { id: "cmStage", label: "Stage" },
    { id: "cmCopyHash", label: "Copy commit hash" },
    { id: "cmCopyBranch", label: "Copy branch name" },
    "separator",
    { id: "cmFetch", label: "Fetch" },
    { id: "cmPull", label: "Pull" },
    { id: "cmPush", label: "Push" },
    "separator",
    { id: "cmRollback", label: "Rollback", danger: true },
  ],
  status: [
    { id: "cmCopyBranch", label: "Copy branch name" },
    { id: "cmFetch", label: "Fetch" },
    { id: "cmPull", label: "Pull" },
    { id: "cmPush", label: "Push" },
  ],
  sshhost: [
    { id: "cmOpenSsh", label: "Open SSH terminal" },
    { id: "cmOpenSftp", label: "Open SFTP browser" },
    "separator",
    { id: "cmCopyAddr", label: "Copy host address" },
    { id: "cmDisconnect", label: "Disconnect", danger: true },
  ],
}
