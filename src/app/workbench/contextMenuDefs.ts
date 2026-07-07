import i18n from "@/lib/i18n"
import type { ContextMenuKind, ContextMenuPayload } from "@/state/contextMenuStore"

export interface ContextMenuItem {
  id: string
  label: string
  danger?: boolean
  // Optional visibility predicate — the item is only rendered when it returns
  // true for the current payload (e.g. cmOpenInBrowser only for .html/.htm files).
  when?: (payload: ContextMenuPayload) => boolean
}

export type ContextMenuEntry = ContextMenuItem | "separator"

const isHtmlPath = (payload: ContextMenuPayload): boolean =>
  !!payload.path && /\.html?$/i.test(payload.path)

// label is a getter (not a plain string) so it's resolved from the current
// i18n language on every read, not baked in once at module load — this object
// is a module-level singleton, and ContextMenu reads `.label` fresh each time
// the menu opens. Keys live at locales/{en,zh-TW}/menus.json under
// contextMenu.<id> (item ids double as translation keys).
function item(
  id: string,
  danger?: true,
  when?: (payload: ContextMenuPayload) => boolean
): ContextMenuItem {
  return {
    id,
    get label() {
      return i18n.t(`contextMenu.${id}`, { ns: "menus" })
    },
    ...(danger ? { danger } : {}),
    ...(when ? { when } : {})
  }
}

// 各區域選單內容 — 對照設計文件 ctxDefs（Yuuzu Workbench.dc.html L3528）。
// item id 沿用設計的 cm* key，能力賦予階段可與設計的 ctxRun 1:1 對映。
// sshhost 目前沒有掛載點（host 列 UI 尚未存在），先進 registry，SSH 里程碑
// 只需要在 host 列補一行 handler。
export const CONTEXT_MENU_DEFS: Record<ContextMenuKind, ContextMenuEntry[]> = {
  general: [
    item("cmCmdPalette"),
    item("cmRefresh"),
    "separator",
    item("cmSettings"),
    item("cmHideSidebar"),
  ],
  rail: [
    item("cmNewProject"),
    item("cmSettings"),
    "separator",
    item("cmHideSidebar"),
  ],
  explorer: [
    item("cmNewFile"),
    item("cmNewFolder"),
    "separator",
    item("cmRefresh"),
    item("cmCopyPath"),
  ],
  file: [
    item("cmOpen"),
    item("cmOpenSplit"),
    item("cmOpenInBrowser", undefined, isHtmlPath),
    "separator",
    item("cmRename"),
    item("cmCopyRel"),
    item("cmReveal"),
    "separator",
    item("cmDelete", true),
  ],
  tab: [
    item("cmCloseTab"),
    item("cmCloseOthers"),
    item("cmCloseAll"),
    "separator",
    item("cmCopyRel"),
    item("cmSplit"),
  ],
  editor: [
    item("cmCut"),
    item("cmCopy"),
    item("cmPaste"),
    "separator",
    // Diff viewer entry (design openDiffCurrent). Opens the active editor file's
    // working-tree diff in the Diff modal; the dispatch no-ops when the file has
    // no git changes (contextMenuStore.runContextMenuAction).
    item("cmCompareHead"),
    item("cmFormatDoc"),
    item("cmCmdPalette"),
  ],
  terminal: [
    item("cmCopySel"),
    item("cmPaste"),
    "separator",
    // 設計在 terminal 停靠進編輯器時把這項換成 "Dock to bottom"；dock 功能
    // 尚未存在，唯一可達狀態是 drawer 側文案。
    item("cmDockTerm"),
    item("cmSplitTermRight"),
    item("cmSplitTermDown"),
    item("cmClear"),
    item("cmKill", true),
  ],
  agent: [
    item("cmStop"),
    item("cmDuplicate"),
    item("cmRenameSession"),
    "separator",
    item("cmCopyPath"),
  ],
  git: [
    item("cmStage"),
    item("cmCopyHash"),
    item("cmCopyBranch"),
    "separator",
    item("cmFetch"),
    item("cmPull"),
    item("cmPush"),
    "separator",
    item("cmRollback", true),
  ],
  status: [
    item("cmCopyBranch"),
    item("cmFetch"),
    item("cmPull"),
    item("cmPush"),
  ],
  sshhost: [
    item("cmOpenSsh"),
    item("cmOpenSftp"),
    "separator",
    item("cmCopyAddr"),
    item("cmDisconnect", true),
  ],
}
