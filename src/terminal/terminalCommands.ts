import { confirm } from "@tauri-apps/plugin-dialog"
import { readText, writeText } from "@tauri-apps/plugin-clipboard-manager"

import type { ContextMenuCommandOutcome } from "@/app/workbench/contextMenuModel"
import { loadTerminalSettings } from "@/app/workbench/settingsStorage"
import i18n from "@/lib/i18n"
import { ptyActivity, ptyClose } from "@/lib/ipc"
import {
  MAX_VISIBLE_TERMINAL_PANES,
  terminalDisplayTitle,
  useTerminalStore,
  type TerminalSessionMeta,
} from "@/state/terminalStore"
import { getTerminalView } from "@/terminal/terminalViewRegistry"

export interface TerminalCommandTarget {
  workspacePath: string
  paneId?: string
  sessionId: string
}

export type TerminalSessionMetaWithArgs = TerminalSessionMeta & { shellArgs?: string[] }

const completed = (): ContextMenuCommandOutcome => "completed"
const cancelled = (): ContextMenuCommandOutcome => "cancelled"

function parseShellArgs(value: string): string[] | undefined {
  const args = value.trim().split(/\s+/).filter(Boolean)
  return args.length > 0 ? args : undefined
}

export function terminalTargetExists(target: TerminalCommandTarget): boolean {
  const state = useTerminalStore.getState()
  const layout = state.layouts[target.workspacePath]
  const session = state.sessions[target.sessionId]
  return Boolean(
    layout?.tabIds.includes(target.sessionId)
    && session?.workspace === target.workspacePath
  )
}

export function terminalPaneTargetExists(target: TerminalCommandTarget): boolean {
  if (!target.paneId || !terminalTargetExists(target)) return false
  return useTerminalStore
    .getState()
    .layouts[target.workspacePath]
    ?.panes.some(
      (pane) => pane.paneId === target.paneId && pane.sessionId === target.sessionId
    ) ?? false
}

export function createTerminalSessionMeta(workspace: string): TerminalSessionMetaWithArgs {
  const terminalSettings = loadTerminalSettings()
  const shell = terminalSettings.shellPath.trim()
  const shellArgs = parseShellArgs(terminalSettings.shellArgs)
  const sessionId =
    typeof crypto !== "undefined" && "randomUUID" in crypto
      ? crypto.randomUUID()
      : `terminal-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`
  const terminalNumber = useTerminalStore.getState().allocateTerminalNumber(workspace)

  return {
    sessionId,
    title: `Terminal ${terminalNumber}`,
    launchStatus: "opening",
    workspace,
    shell,
    shellArgs,
    cols: 80,
    rows: 24,
  }
}

export async function copyTerminalSelection(
  target: TerminalCommandTarget
): Promise<ContextMenuCommandOutcome> {
  if (!terminalTargetExists(target)) return cancelled()
  const view = getTerminalView(target.sessionId)
  if (!view?.hasSelection()) return cancelled()
  await writeText(view.getSelection())
  return completed()
}

export async function pasteTerminalClipboard(
  target: TerminalCommandTarget
): Promise<ContextMenuCommandOutcome> {
  if (!terminalTargetExists(target)) return cancelled()
  const view = getTerminalView(target.sessionId)
  if (!view || view.isReady?.() === false) return cancelled()
  const text = await readText()
  if (text.length === 0) return cancelled()
  if (
    !terminalTargetExists(target)
    || getTerminalView(target.sessionId) !== view
    || view.isReady?.() === false
  ) return cancelled()
  await view.paste(text)
  return completed()
}

export function clearTerminalBuffer(target: TerminalCommandTarget): ContextMenuCommandOutcome {
  if (!terminalTargetExists(target)) return cancelled()
  const view = getTerminalView(target.sessionId)
  if (!view) return cancelled()
  view.clear()
  return completed()
}

export function splitTerminal(
  target: TerminalCommandTarget
): ContextMenuCommandOutcome {
  if (!terminalPaneTargetExists(target)) return cancelled()
  const state = useTerminalStore.getState()
  if (
    (state.layouts[target.workspacePath]?.panes.length ?? 0)
    >= MAX_VISIBLE_TERMINAL_PANES
  ) return cancelled()
  state.splitFrom(
    target.workspacePath,
    target.paneId!,
    createTerminalSessionMeta(target.workspacePath)
  )
  return completed()
}

export function beginRenameTerminal(
  target: TerminalCommandTarget
): ContextMenuCommandOutcome {
  if (!terminalTargetExists(target)) return cancelled()
  useTerminalStore.getState().beginRename(target.workspacePath, target.sessionId)
  return completed()
}

export async function closeTerminal(
  target: TerminalCommandTarget
): Promise<ContextMenuCommandOutcome> {
  if (!terminalTargetExists(target)) return cancelled()
  const state = useTerminalStore.getState()
  const session = state.sessions[target.sessionId]
  let activity: "idle" | "busy" | "unknown" = "idle"
  if (session.launchStatus !== "failed") {
    activity = await ptyActivity(target.sessionId).catch(() => "unknown")
  }

  if (activity !== "idle") {
    const confirmed = await confirm(
      i18n.t(
        activity === "busy"
          ? "contextMenu.terminal.closeBusyConfirmMessage"
          : "contextMenu.terminal.closeUnknownConfirmMessage",
        {
          ns: "menus",
          title: terminalDisplayTitle(session),
        }
      ),
      {
        title: i18n.t("contextMenu.terminal.closeConfirmTitle", { ns: "menus" }),
        kind: "warning",
      }
    )
    if (!confirmed) return cancelled()
  }
  if (!terminalTargetExists(target)) return cancelled()

  await ptyClose(target.sessionId)
  if (terminalTargetExists(target)) {
    useTerminalStore.getState().removeSession(target.workspacePath, target.sessionId)
  }
  return completed()
}
