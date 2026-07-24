import { confirm } from "@tauri-apps/plugin-dialog"

import type { ContextMenuCommandOutcome } from "@/app/workbench/contextMenuModel"
import { loadTerminalSettings } from "@/app/workbench/settingsStorage"
import i18n from "@/lib/i18n"
import { ptyActivity, ptyClose } from "@/lib/ipc"
import type { TerminalProfile } from "@/lib/types"
import {
  MAX_VISIBLE_TERMINAL_PANES,
  terminalDisplayTitle,
  useTerminalStore,
  type TerminalSessionMeta,
} from "@/state/terminalStore"

export interface TerminalCommandTarget {
  workspacePath: string
  paneId?: string
  sessionId: string
}

export type TerminalSessionMetaWithArgs = TerminalSessionMeta & { shellArgs?: string[] }

const completed = (): ContextMenuCommandOutcome => "completed"
const cancelled = (): ContextMenuCommandOutcome => "cancelled"

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

export function createTerminalSessionMeta(
  workspace: string,
  profile?: TerminalProfile,
): TerminalSessionMetaWithArgs {
  const terminalSettings = loadTerminalSettings()
  const selectedProfile = profile ?? terminalSettings.defaultProfile
  const shell = selectedProfile.shell.trim()
  const configuredArgs = selectedProfile.args
  const shellArgs = configuredArgs.length > 0 ? [...configuredArgs] : undefined
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
    profileName: selectedProfile?.name,
    cwdStrategy: selectedProfile.cwdStrategy,
    imeAnchorMode: terminalSettings.imeAnchorMode === "tui" ? "tui" : "cursor",
    cols: 80,
    rows: 24,
  }
}

export function splitTerminal(
  target: TerminalCommandTarget,
  profile?: TerminalProfile,
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
    createTerminalSessionMeta(target.workspacePath, profile)
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
