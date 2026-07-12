import { confirm } from "@tauri-apps/plugin-dialog"
import { readText, writeText } from "@tauri-apps/plugin-clipboard-manager"

import type { ContextMenuCommandOutcome } from "@/app/workbench/contextMenuModel"
import { loadTerminalSettings } from "@/app/workbench/settingsStorage"
import i18n from "@/lib/i18n"
import { ptyClose } from "@/lib/ipc"
import {
  MAX_PANES,
  useTerminalStore,
  type TerminalSessionMeta,
  type TerminalSplitDirection,
} from "@/state/terminalStore"
import { getTerminalView } from "@/terminal/terminalViewRegistry"

export interface TerminalCommandTarget {
  workspacePath: string
  paneId: string
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
  const pane = state.layouts[target.workspacePath]?.panes.find(
    (candidate) => candidate.paneId === target.paneId
  )
  const session = state.sessions[target.sessionId]
  return pane?.sessionId === target.sessionId && session?.workspace === target.workspacePath
}

export function createTerminalSessionMeta(workspace: string): TerminalSessionMetaWithArgs {
  const terminalSettings = loadTerminalSettings()
  const shell = terminalSettings.shellPath.trim()
  const shellArgs = parseShellArgs(terminalSettings.shellArgs)
  const sessionId =
    typeof crypto !== "undefined" && "randomUUID" in crypto
      ? crypto.randomUUID()
      : `terminal-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`
  const paneCount = useTerminalStore.getState().layouts[workspace]?.panes.length ?? 0

  return {
    sessionId,
    title: `Terminal ${paneCount + 1}`,
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
  target: TerminalCommandTarget,
  direction: TerminalSplitDirection
): ContextMenuCommandOutcome {
  if (!terminalTargetExists(target)) return cancelled()
  const state = useTerminalStore.getState()
  if ((state.layouts[target.workspacePath]?.panes.length ?? 0) >= MAX_PANES) return cancelled()
  state.splitFrom(
    target.workspacePath,
    target.paneId,
    createTerminalSessionMeta(target.workspacePath),
    direction
  )
  return completed()
}

export async function closeTerminal(
  target: TerminalCommandTarget
): Promise<ContextMenuCommandOutcome> {
  if (!terminalTargetExists(target)) return cancelled()
  const confirmed = await confirm(
    i18n.t("contextMenu.terminal.closeConfirmMessage", { ns: "menus" }),
    {
      title: i18n.t("contextMenu.terminal.closeConfirmTitle", { ns: "menus" }),
      kind: "warning",
    }
  )
  if (!confirmed) return cancelled()
  if (!terminalTargetExists(target)) return cancelled()

  await ptyClose(target.sessionId)
  if (terminalTargetExists(target)) {
    useTerminalStore.getState().removeSession(target.workspacePath, target.sessionId)
  }
  return completed()
}
