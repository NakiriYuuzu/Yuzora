import { writeText } from "@tauri-apps/plugin-clipboard-manager"

import i18n from "@/lib/i18n"
import { getViewEntry } from "@/editor/viewRegistry"
import { isAbsolutePath } from "@/lib/paths"
import { gitStage, gitUnstage } from "@/lib/ipc"
import { pickWorkspace } from "@/lib/workspaceActions"
import { useAgentStore } from "@/state/agentStore"
import { dbProfileUiErrorCode, useDbStore } from "@/state/dbStore"
import { useGitStore } from "@/state/gitStore"
import { useGitRollbackDialogStore } from "@/state/gitRollbackDialogStore"
import { useUiStore } from "@/state/uiStore"
import { useSshStore } from "@/state/sshStore"
import { MAX_PANES, useTerminalStore } from "@/state/terminalStore"
import { useWorkspaceStore } from "@/state/workspaceStore"
import {
  copyPreviewUrl,
  goBackPreview,
  goForwardPreview,
  openPreviewExternally,
  previewTargetCanGoBack,
  previewTargetCanGoForward,
  previewTargetHasRunningServer,
  previewTargetHasUrl,
  previewTargetIsCurrent,
  reloadPreview,
  stopPreviewDevServer,
} from "@/preview/previewCommands"
import {
  clearTerminalBuffer,
  closeTerminal,
  copyTerminalSelection,
  pasteTerminalClipboard,
  splitTerminal,
  terminalTargetExists,
} from "@/terminal/terminalCommands"
import { getTerminalView } from "@/terminal/terminalViewRegistry"
import {
  CONTEXT_MENU_CANCELLED,
  CONTEXT_MENU_COMPLETED,
  type ContextMenuAvailability,
  type ContextMenuCommandDefinition,
  type ContextMenuKind,
  type ContextMenuRegistry,
  type ContextMenuRequest,
  type ContextMenuRequestFor,
} from "@/app/workbench/contextMenuModel"
import { executeLegacyContextMenuAction, worktreeCompareTarget } from "@/state/contextMenuStore"
import {
  exactGitChanges,
  gitChangeRows,
  uniquePaths,
  type GitChangeKey,
  type GitChangeRow,
} from "@/workbench/git/gitChangeSelection"

export type ContextMenuItem = ContextMenuCommandDefinition
export type ContextMenuEntry = ContextMenuCommandDefinition | "separator"

export interface ResolvedContextMenuItem {
  type: "command"
  command: ContextMenuCommandDefinition
  label: string
  availability: ContextMenuAvailability
}

export interface ResolvedContextMenuSeparator {
  type: "separator"
}

export type ResolvedContextMenuEntry = ResolvedContextMenuItem | ResolvedContextMenuSeparator

const DISABLED_TARGET = "contextMenu.disabled.targetUnavailable"
const DISABLED_NOTHING = "contextMenu.disabled.nothingToDo"
const DISABLED_NO_SELECTION = "contextMenu.disabled.noSelection"
const DISABLED_READONLY = "contextMenu.disabled.readonly"
const DISABLED_FORMATTER_CHECKING = "contextMenu.disabled.formatterChecking"
const DISABLED_TWO_GROUP_LIMIT = "contextMenu.disabled.twoGroupLimit"
const DISABLED_NOT_REPOSITORY = "contextMenu.disabled.notRepository"
const DISABLED_GIT_BUSY = "contextMenu.disabled.gitBusy"
const DISABLED_CONNECTING = "contextMenu.disabled.connecting"
const DISABLED_AUTHENTICATION_PENDING = "contextMenu.disabled.authenticationPending"
const DISABLED_DB_CONNECTING = "contextMenu.disabled.dbConnecting"
const DISABLED_DB_ALREADY_OPEN = "contextMenu.disabled.dbAlreadyOpen"
const DISABLED_TERMINAL_UNAVAILABLE = "contextMenu.disabled.terminalUnavailable"
const DISABLED_TERMINAL_PANE_LIMIT = "contextMenu.disabled.terminalPaneLimit"
const DISABLED_NO_PENDING_RESPONSE = "contextMenu.disabled.noPendingResponse"
const DISABLED_NO_BACK_HISTORY = "contextMenu.disabled.noBackHistory"
const DISABLED_NO_FORWARD_HISTORY = "contextMenu.disabled.noForwardHistory"

const available = (): ContextMenuAvailability => ({ visible: true, enabled: true })
const hidden = (): ContextMenuAvailability => ({ visible: false, enabled: false })
const disabled = (disabledReasonKey: string): ContextMenuAvailability => ({
  visible: true,
  enabled: false,
  disabledReasonKey,
})

type CommandOptions<K extends ContextMenuKind> = {
  availability: (request: ContextMenuRequestFor<K>) => ContextMenuAvailability
  danger: boolean
  executor: (
    request: ContextMenuRequestFor<K>
  ) => "completed" | "cancelled" | Promise<"completed" | "cancelled">
  labelKey?: string
  label?: (request: ContextMenuRequestFor<K>) => string
}

function item<K extends ContextMenuKind>(
  id: string,
  options: CommandOptions<K>
): ContextMenuCommandDefinition {
  return {
    id,
    label: (request) => options.label?.(request as ContextMenuRequestFor<K>)
      ?? i18n.t(`contextMenu.${options.labelKey ?? id}`, { ns: "menus" }),
    availability: (request) => options.availability(request as ContextMenuRequestFor<K>),
    danger: options.danger,
    executor: (request) => options.executor(request as ContextMenuRequestFor<K>),
  }
}

function legacy<K extends ContextMenuKind>(id: string) {
  return (request: ContextMenuRequestFor<K>) => executeLegacyContextMenuAction(request, id)
}

function currentWorkspace(workspacePath: string | null): boolean {
  return workspacePath !== null && useWorkspaceStore.getState().workspacePath === workspacePath
}

function tabExists(request: ContextMenuRequestFor<"tab">): boolean {
  if (useWorkspaceStore.getState().workspacePath !== request.workspacePath) return false
  return useWorkspaceStore
    .getState()
    .groups[request.groupIndex]?.tabs.some((tab) => tab.path === request.path) ?? false
}

function editorExists(request: ContextMenuRequestFor<"editor">): boolean {
  if (useWorkspaceStore.getState().workspacePath !== request.workspacePath) return false
  const group = useWorkspaceStore.getState().groups[request.groupIndex]
  const entry = getViewEntry(request.path)
  return group?.activePath === request.path && entry?.groupIndex === request.groupIndex
}

function editorHasSelection(request: ContextMenuRequestFor<"editor">): boolean {
  const entry = getViewEntry(request.path)
  return Boolean(entry?.view.state.selection.ranges.some((range) => !range.empty))
}

function rightSplitAvailability(groupIndex: number): ContextMenuAvailability {
  const groups = useWorkspaceStore.getState().groups
  if (!groups[groupIndex]) return disabled(DISABLED_TARGET)
  return groups.length >= 2 && groupIndex >= groups.length - 1
    ? disabled(DISABLED_TWO_GROUP_LIMIT)
    : available()
}

function terminalViewAvailability(
  request: ContextMenuRequestFor<"terminal">,
  selectionRequired = false
): ContextMenuAvailability {
  if (!terminalTargetExists(request)) return disabled(DISABLED_TARGET)
  const view = getTerminalView(request.sessionId)
  if (!view) return disabled(DISABLED_TERMINAL_UNAVAILABLE)
  return selectionRequired && !view.hasSelection()
    ? disabled(DISABLED_NO_SELECTION)
    : available()
}

function terminalSplitAvailability(
  request: ContextMenuRequestFor<"terminal">
): ContextMenuAvailability {
  if (!terminalTargetExists(request)) return disabled(DISABLED_TARGET)
  const panes = useTerminalStore.getState().layouts[request.workspacePath]?.panes ?? []
  return panes.length >= MAX_PANES
    ? disabled(DISABLED_TERMINAL_PANE_LIMIT)
    : available()
}

function gitAvailability(request: ContextMenuRequestFor<"git"> | ContextMenuRequestFor<"status">) {
  const state = useGitStore.getState()
  if (state.environment?.status !== "ready" || state.environment.root !== request.repositoryRoot) {
    return disabled(DISABLED_NOT_REPOSITORY)
  }
  if (state.busy) return disabled(DISABLED_GIT_BUSY)
  return available()
}

function gitTargetMatches(request: ContextMenuRequestFor<"git"> | ContextMenuRequestFor<"status">): boolean {
  const environment = useGitStore.getState().environment
  return environment?.status === "ready" && environment.root === request.repositoryRoot
}

function sshHostAvailability(request: ContextMenuRequestFor<"sshhost">): ContextMenuAvailability {
  const state = useSshStore.getState()
  if (!state.hosts.some((host) => host.id === request.hostId)) return hidden()
  if (state.pendingAuthHostId === request.hostId) {
    return disabled(DISABLED_AUTHENTICATION_PENDING)
  }
  return state.sessions[request.hostId]?.status === "connecting"
    ? disabled(DISABLED_CONNECTING)
    : available()
}

function previewHistoryAvailability(
  request: ContextMenuRequestFor<"preview">,
  direction: "back" | "forward"
): ContextMenuAvailability {
  if (!previewTargetIsCurrent(request)) return disabled(DISABLED_TARGET)
  const canNavigate = direction === "back"
    ? previewTargetCanGoBack(request)
    : previewTargetCanGoForward(request)
  return canNavigate
    ? available()
    : disabled(direction === "back" ? DISABLED_NO_BACK_HISTORY : DISABLED_NO_FORWARD_HISTORY)
}

function previewUrlAvailability(
  request: ContextMenuRequestFor<"preview">
): ContextMenuAvailability {
  if (request.url === null) return hidden()
  return previewTargetHasUrl(request) ? available() : disabled(DISABLED_TARGET)
}

function gitChangeKeys(request: ContextMenuRequestFor<"gitChange">): GitChangeKey[] {
  return request.selected.map((target) => ({ ...target }))
}

function latestGitChangeRows(request: ContextMenuRequestFor<"gitChange">): GitChangeRow[] | null {
  const state = useGitStore.getState()
  if (state.environment?.status !== "ready" || state.environment.root !== request.repositoryRoot) {
    return null
  }
  return gitChangeRows(state.status)
}

function exactApplicableGitChanges(
  request: ContextMenuRequestFor<"gitChange">,
  applies: (row: GitChangeRow) => boolean
): GitChangeRow[] {
  const rows = latestGitChangeRows(request)
  if (!rows) return []
  return exactGitChanges(gitChangeKeys(request), rows).filter(applies)
}

function gitChangeAvailability(
  request: ContextMenuRequestFor<"gitChange">,
  applies: (row: GitChangeRow) => boolean,
  requireAll = false
): ContextMenuAvailability {
  const rows = latestGitChangeRows(request)
  if (!rows) return hidden()
  const exact = exactGitChanges(gitChangeKeys(request), rows)
  const applicable = exact.filter(applies)
  if (applicable.length === 0 || (requireAll && exact.length !== request.selected.length)) {
    return hidden()
  }
  const busy = useGitStore.getState().busy
  return busy
    ? disabled(String(i18n.t("contextMenu.disabled.gitBusyOperation", {
        ns: "menus",
        operation: busy,
      })))
    : available()
}

async function runGitChangeStageOperation(
  request: ContextMenuRequestFor<"gitChange">,
  staged: boolean
): Promise<"completed" | "cancelled"> {
  const applicable = exactApplicableGitChanges(request, (row) => row.staged !== staged)
  const paths = uniquePaths(applicable)
  if (paths.length === 0) return CONTEXT_MENU_CANCELLED
  const ok = await useGitStore.getState().runOp(staged ? "stage" : "unstage", () =>
    staged
      ? gitStage(request.repositoryRoot, paths)
      : gitUnstage(request.repositoryRoot, paths)
  )
  if (!ok) return CONTEXT_MENU_CANCELLED
  useUiStore.getState().reconcileGitChangeSelection(
    gitChangeRows(useGitStore.getState().status),
    Object.fromEntries(paths.map((path) => [path, staged]))
  )
  return CONTEXT_MENU_COMPLETED
}

export const CONTEXT_MENU_DEFS: ContextMenuRegistry = {
  general: [
    item<"general">("cmCmdPalette", { availability: available, danger: false, executor: legacy("cmCmdPalette") }),
    "separator",
    item<"general">("cmSettings", { availability: available, danger: false, executor: legacy("cmSettings") }),
    item<"general">("cmHideSidebar", { availability: available, danger: false, executor: legacy("cmHideSidebar") }),
  ],
  rail: [
    item<"rail">("cmOpenWorkspace", {
      availability: available,
      danger: false,
      executor: async () => (await pickWorkspace()) ? CONTEXT_MENU_COMPLETED : CONTEXT_MENU_CANCELLED,
    }),
    item<"rail">("cmSettings", { availability: available, danger: false, executor: legacy("cmSettings") }),
    "separator",
    item<"rail">("cmHideSidebar", { availability: available, danger: false, executor: legacy("cmHideSidebar") }),
  ],
  explorer: [
    item<"explorer">("cmNewFile", {
      availability: (request) => currentWorkspace(request.workspacePath) ? available() : hidden(),
      danger: false,
      executor: legacy("cmNewFile"),
    }),
    item<"explorer">("cmNewFolder", {
      availability: (request) => currentWorkspace(request.workspacePath) ? available() : hidden(),
      danger: false,
      executor: legacy("cmNewFolder"),
    }),
    "separator",
    item<"explorer">("cmRefresh", {
      availability: (request) => currentWorkspace(request.workspacePath) ? available() : hidden(),
      danger: false,
      executor: () => {
        useWorkspaceStore.getState().refreshTree()
        return CONTEXT_MENU_COMPLETED
      },
    }),
    item<"explorer">("cmCopyPath", {
      availability: (request) => currentWorkspace(request.workspacePath) ? available() : hidden(),
      danger: false,
      executor: legacy("cmCopyPath"),
    }),
  ],
  file: [
    item<"file">("cmOpen", {
      availability: (request) => request.isDirectory ? hidden() : currentWorkspace(request.workspacePath) ? available() : disabled(DISABLED_TARGET),
      danger: false,
      executor: legacy("cmOpen"),
    }),
    item<"file">("cmOpenSplit", {
      availability: (request) => request.isDirectory
        ? hidden()
        : currentWorkspace(request.workspacePath)
          ? rightSplitAvailability(request.sourceGroupIndex)
          : disabled(DISABLED_TARGET),
      danger: false,
      executor: legacy("cmOpenSplit"),
    }),
    item<"file">("cmOpenInBrowser", {
      availability: (request) => request.isDirectory || !/\.html?$/i.test(request.path)
        ? hidden()
        : currentWorkspace(request.workspacePath) ? available() : disabled(DISABLED_TARGET),
      danger: false,
      executor: legacy("cmOpenInBrowser"),
    }),
    "separator",
    item<"file">("cmRename", {
      availability: (request) => currentWorkspace(request.workspacePath) ? available() : disabled(DISABLED_TARGET),
      danger: false,
      executor: legacy("cmRename"),
    }),
    item<"file">("cmCopyRel", {
      availability: (request) => currentWorkspace(request.workspacePath) ? available() : disabled(DISABLED_TARGET),
      danger: false,
      executor: legacy("cmCopyRel"),
    }),
    item<"file">("cmReveal", {
      availability: (request) => currentWorkspace(request.workspacePath) ? available() : disabled(DISABLED_TARGET),
      danger: false,
      executor: legacy("cmReveal"),
    }),
    "separator",
    item<"file">("cmDelete", {
      availability: (request) => currentWorkspace(request.workspacePath) ? available() : disabled(DISABLED_TARGET),
      danger: true,
      executor: legacy("cmDelete"),
    }),
  ],
  tab: [
    item<"tab">("cmCloseTab", {
      availability: (request) => tabExists(request) ? available() : disabled(DISABLED_TARGET),
      danger: false,
      executor: legacy("cmCloseTab"),
    }),
    item<"tab">("cmCloseOthers", {
      availability: (request) => {
        if (!tabExists(request)) return disabled(DISABLED_TARGET)
        const tabs = useWorkspaceStore.getState().groups[request.groupIndex]?.tabs ?? []
        return tabs.some((tab) => tab.path !== request.path) ? available() : disabled(DISABLED_NOTHING)
      },
      danger: false,
      executor: legacy("cmCloseOthers"),
    }),
    item<"tab">("cmCloseAll", {
      availability: (request) => tabExists(request) ? available() : disabled(DISABLED_TARGET),
      danger: false,
      executor: legacy("cmCloseAll"),
    }),
    "separator",
    item<"tab">("cmCopyRel", {
      availability: (request) => tabExists(request) ? available() : disabled(DISABLED_TARGET),
      danger: false,
      executor: legacy("cmCopyRel"),
    }),
    item<"tab">("cmSplit", {
      availability: (request) => {
        if (!tabExists(request)) return disabled(DISABLED_TARGET)
        const tab = useWorkspaceStore.getState().groups[request.groupIndex]?.tabs.find(
          (candidate) => candidate.path === request.path
        )
        if (tab?.kind === "preview") return hidden()
        return rightSplitAvailability(request.groupIndex)
      },
      danger: false,
      executor: legacy("cmSplit"),
    }),
  ],
  editor: [
    item<"editor">("cmCut", {
      availability: (request) => {
        if (!editorExists(request)) return disabled(DISABLED_TARGET)
        if (!editorHasSelection(request)) return disabled(DISABLED_NO_SELECTION)
        return getViewEntry(request.path)?.readonly ? disabled(DISABLED_READONLY) : available()
      },
      danger: false,
      executor: legacy("cmCut"),
    }),
    item<"editor">("cmCopy", {
      availability: (request) => !editorExists(request)
        ? disabled(DISABLED_TARGET)
        : editorHasSelection(request) ? available() : disabled(DISABLED_NO_SELECTION),
      danger: false,
      executor: legacy("cmCopy"),
    }),
    item<"editor">("cmPaste", {
      availability: (request) => !editorExists(request)
        ? disabled(DISABLED_TARGET)
        : getViewEntry(request.path)?.readonly ? disabled(DISABLED_READONLY) : available(),
      danger: false,
      executor: legacy("cmPaste"),
    }),
    "separator",
    item<"editor">("cmCompareHead", {
      availability: (request) => editorExists(request) && worktreeCompareTarget(request.path)
        ? available() : hidden(),
      danger: false,
      executor: legacy("cmCompareHead"),
    }),
    item<"editor">("cmFormatDoc", {
      availability: (request) => {
        if (!editorExists(request)) return hidden()
        const entry = getViewEntry(request.path)
        if (!entry) return hidden()
        if (entry.readonly) return disabled(DISABLED_READONLY)
        if (entry.formatter === "checking") return disabled(DISABLED_FORMATTER_CHECKING)
        return entry.formatter === "available" && entry.formatDocument ? available() : hidden()
      },
      danger: false,
      executor: legacy("cmFormatDoc"),
    }),
    item<"editor">("cmCmdPalette", { availability: available, danger: false, executor: legacy("cmCmdPalette") }),
  ],
  terminal: [
    item<"terminal">("cmCopySel", {
      availability: (request) => terminalViewAvailability(request, true),
      danger: false,
      executor: copyTerminalSelection,
    }),
    item<"terminal">("cmPaste", {
      availability: terminalViewAvailability,
      danger: false,
      executor: pasteTerminalClipboard,
    }),
    "separator",
    item<"terminal">("cmSplitTermRight", {
      availability: terminalSplitAvailability,
      danger: false,
      executor: (request) => splitTerminal(request, "right"),
    }),
    item<"terminal">("cmSplitTermDown", {
      availability: terminalSplitAvailability,
      danger: false,
      executor: (request) => splitTerminal(request, "down"),
    }),
    item<"terminal">("cmClear", {
      availability: terminalViewAvailability,
      danger: false,
      executor: clearTerminalBuffer,
    }),
    item<"terminal">("cmCloseTerminal", {
      availability: (request) => terminalTargetExists(request) ? available() : disabled(DISABLED_TARGET),
      danger: true,
      executor: closeTerminal,
    }),
  ],
  agentSession: [
    item<"agentSession">("cmContinueSession", {
      availability: (request) => useAgentStore.getState().sessions.has(request.sessionId)
        ? available() : disabled(DISABLED_TARGET),
      danger: false,
      executor: async (request) => {
        if (!useAgentStore.getState().sessions.has(request.sessionId)) return CONTEXT_MENU_CANCELLED
        await useAgentStore.getState().continueSession(request.sessionId)
        return CONTEXT_MENU_COMPLETED
      },
    }),
    item<"agentSession">("cmCancelResponse", {
      availability: (request) => {
        const session = useAgentStore.getState().sessions.get(request.sessionId)
        if (!session) return disabled(DISABLED_TARGET)
        return session.pendingTurn === true ? available() : disabled(DISABLED_NO_PENDING_RESPONSE)
      },
      danger: false,
      executor: async (request) => {
        if (!useAgentStore.getState().sessions.has(request.sessionId)) return CONTEXT_MENU_CANCELLED
        return (await useAgentStore.getState().cancel(request.sessionId))
          ? CONTEXT_MENU_COMPLETED
          : CONTEXT_MENU_CANCELLED
      },
    }),
    item<"agentSession">("cmRenameSession", {
      availability: (request) => useAgentStore.getState().sessions.has(request.sessionId)
        ? available() : disabled(DISABLED_TARGET),
      danger: false,
      executor: (request) => {
        if (!useAgentStore.getState().sessions.has(request.sessionId)) return CONTEXT_MENU_CANCELLED
        useAgentStore.getState().beginRenameSession(request.sessionId)
        return CONTEXT_MENU_COMPLETED
      },
    }),
    "separator",
    item<"agentSession">("cmCopyWorkingDirectory", {
      availability: (request) => isAbsolutePath(
        useAgentStore.getState().sessions.get(request.sessionId)?.cwd
      ) ? available() : hidden(),
      danger: false,
      executor: async (request) => {
        const cwd = useAgentStore.getState().sessions.get(request.sessionId)?.cwd
        if (!isAbsolutePath(cwd)) return CONTEXT_MENU_CANCELLED
        await writeText(cwd)
        return CONTEXT_MENU_COMPLETED
      },
    }),
    "separator",
    item<"agentSession">("cmRemoveSession", {
      availability: (request) => useAgentStore.getState().sessions.has(request.sessionId)
        ? available() : disabled(DISABLED_TARGET),
      danger: true,
      executor: async (request) => {
        const confirmed = await useAgentStore.getState().requestRemoveSessionConfirm(request.sessionId)
        if (!confirmed) return CONTEXT_MENU_CANCELLED
        if (!useAgentStore.getState().sessions.has(request.sessionId)) return CONTEXT_MENU_CANCELLED
        return (await useAgentStore.getState().removeSession(request.sessionId))
          ? CONTEXT_MENU_COMPLETED
          : CONTEXT_MENU_CANCELLED
      },
    }),
  ],
  git: [
    item<"git">("cmCopyHash", {
      availability: (request) => gitTargetMatches(request) && useGitStore.getState().status?.headOid ? available() : hidden(),
      danger: false,
      executor: legacy("cmCopyHash"),
    }),
    item<"git">("cmCopyBranch", {
      availability: (request) => gitTargetMatches(request) && useGitStore.getState().status?.branch ? available() : hidden(),
      danger: false,
      executor: legacy("cmCopyBranch"),
    }),
    "separator",
    item<"git">("cmFetch", { availability: gitAvailability, danger: false, executor: legacy("cmFetch") }),
    item<"git">("cmPull", { availability: gitAvailability, danger: false, executor: legacy("cmPull") }),
    item<"git">("cmPush", { availability: gitAvailability, danger: false, executor: legacy("cmPush") }),
  ],
  gitChange: [
    item<"gitChange">("cmStageSelected", {
      availability: (request) => gitChangeAvailability(request, (row) => !row.staged),
      danger: false,
      executor: (request) => runGitChangeStageOperation(request, true),
    }),
    item<"gitChange">("cmUnstageSelected", {
      availability: (request) => gitChangeAvailability(request, (row) => row.staged),
      danger: false,
      executor: (request) => runGitChangeStageOperation(request, false),
    }),
    "separator",
    item<"gitChange">("cmRollbackSelected", {
      availability: (request) => gitChangeAvailability(request, () => true, true),
      danger: true,
      executor: async (request) => {
        const rows = latestGitChangeRows(request)
        if (!rows) return CONTEXT_MENU_CANCELLED
        const targets = gitChangeKeys(request)
        if (exactGitChanges(targets, rows).length !== targets.length) {
          return CONTEXT_MENU_CANCELLED
        }
        const confirmed = await useGitRollbackDialogStore.getState().request({
          repositoryRoot: request.repositoryRoot,
          targets,
        })
        return confirmed ? CONTEXT_MENU_COMPLETED : CONTEXT_MENU_CANCELLED
      },
    }),
  ],
  status: [
    item<"status">("cmCopyHash", {
      availability: (request) => gitTargetMatches(request) && useGitStore.getState().status?.headOid ? available() : hidden(),
      danger: false,
      executor: legacy("cmCopyHash"),
    }),
    item<"status">("cmCopyBranch", {
      availability: (request) => gitTargetMatches(request) && useGitStore.getState().status?.branch ? available() : hidden(),
      danger: false,
      executor: legacy("cmCopyBranch"),
    }),
    "separator",
    item<"status">("cmFetch", { availability: gitAvailability, danger: false, executor: legacy("cmFetch") }),
    item<"status">("cmPull", { availability: gitAvailability, danger: false, executor: legacy("cmPull") }),
    item<"status">("cmPush", { availability: gitAvailability, danger: false, executor: legacy("cmPush") }),
  ],
  sshhost: [
    item<"sshhost">("cmOpenSsh", {
      availability: sshHostAvailability,
      danger: false,
      executor: legacy("cmOpenSsh"),
    }),
    item<"sshhost">("cmOpenSftp", {
      availability: sshHostAvailability,
      danger: false,
      executor: legacy("cmOpenSftp"),
    }),
    "separator",
    item<"sshhost">("cmCopyAddr", {
      availability: (request) => useSshStore.getState().hosts.some((host) => host.id === request.hostId)
        ? available() : hidden(),
      danger: false,
      executor: legacy("cmCopyAddr"),
    }),
    item<"sshhost">("cmDisconnect", {
      availability: (request) => {
        const state = useSshStore.getState()
        if (!state.hosts.some((host) => host.id === request.hostId)) return hidden()
        const session = state.sessions[request.hostId]
        return session?.status === "connected" && session.sessionId ? available() : hidden()
      },
      danger: true,
      executor: legacy("cmDisconnect"),
    }),
  ],
  dbconn: [
    item<"dbconn">("cmOpenDb", {
      label: (request) => {
        const state = useDbStore.getState()
        const live = state.connections.some(
          (connection) => connection.descriptorId === request.descriptorId
        )
        const saved = state.saved.find((entry) => entry.id === request.descriptorId)
        const needsCredentialPrompt = saved?.kind !== "sqlite"
          && (saved?.credentialState === "required" || saved?.credentialState === "unavailable")
        const key = live || !saved
          ? "cmOpenDb"
          : needsCredentialPrompt
            ? "cmReconnectDbWithPrompt"
            : "cmReconnectDb"
        return i18n.t(`contextMenu.${key}`, { ns: "menus" })
      },
      availability: (request) => {
        const state = useDbStore.getState()
        const saved = state.saved.find((entry) => entry.id === request.descriptorId)
        if (!saved) return disabled(DISABLED_TARGET)
        if (state.sessions[request.descriptorId]?.status === "connecting") {
          return disabled(DISABLED_DB_CONNECTING)
        }
        const live = state.connections.find((connection) => connection.descriptorId === request.descriptorId)
        if (live?.connId === state.activeConnId) return disabled(DISABLED_DB_ALREADY_OPEN)
        return available()
      },
      danger: false,
      executor: async (request) => {
        const result = await useDbStore
          .getState()
          .openOrReconnectSavedConnection(request.descriptorId)
        if (result.outcome === "error") {
          throw new Error(i18n.t(`database.profileError.${dbProfileUiErrorCode(result.error)}`, {
            ns: "workbench",
          }))
        }
        return result.outcome
      },
    }),
    "separator",
    item<"dbconn">("cmCopyAddr", {
      label: (request) => {
        const saved = useDbStore.getState().saved.find(
          (entry) => entry.id === request.descriptorId
        )
        const key = saved?.kind === "sqlite" ? "cmCopyDbFilePath" : "cmCopyDbAddress"
        return i18n.t(`contextMenu.${key}`, { ns: "menus" })
      },
      availability: (request) => useDbStore.getState().saved.some((entry) => entry.id === request.descriptorId)
        ? available() : disabled(DISABLED_TARGET),
      danger: false,
      executor: legacy("cmCopyAddr"),
    }),
    item<"dbconn">("cmDisconnect", {
      availability: (request) => useDbStore.getState().connections.some((connection) => connection.descriptorId === request.descriptorId)
        ? available() : hidden(),
      danger: true,
      executor: legacy("cmDisconnect"),
    }),
  ],
  preview: [
    item<"preview">("cmPreviewBack", {
      availability: (request) => previewHistoryAvailability(request, "back"),
      danger: false,
      executor: goBackPreview,
    }),
    item<"preview">("cmPreviewForward", {
      availability: (request) => previewHistoryAvailability(request, "forward"),
      danger: false,
      executor: goForwardPreview,
    }),
    item<"preview">("cmPreviewReload", {
      availability: previewUrlAvailability,
      danger: false,
      executor: reloadPreview,
    }),
    "separator",
    item<"preview">("cmCopyUrl", {
      availability: previewUrlAvailability,
      danger: false,
      executor: copyPreviewUrl,
    }),
    item<"preview">("cmOpenExternal", {
      availability: previewUrlAvailability,
      danger: false,
      executor: openPreviewExternally,
    }),
    "separator",
    item<"preview">("cmStopDevServer", {
      availability: (request) => previewTargetHasRunningServer(request) ? available() : hidden(),
      danger: true,
      executor: stopPreviewDevServer,
    }),
  ],
}

export function resolveContextMenuEntries(request: ContextMenuRequest): ResolvedContextMenuEntry[] {
  const resolved: ResolvedContextMenuEntry[] = []
  for (const entry of CONTEXT_MENU_DEFS[request.kind]) {
    if (entry === "separator") {
      if (resolved.length > 0 && resolved.at(-1)?.type !== "separator") resolved.push({ type: "separator" })
      continue
    }
    const availability = entry.availability(request)
    if (!availability.visible) continue
    resolved.push({ type: "command", command: entry, label: entry.label(request), availability })
  }
  if (resolved.at(-1)?.type === "separator") resolved.pop()
  return resolved
}

export function commandFor(request: ContextMenuRequest, actionId: string): ContextMenuCommandDefinition | null {
  const entry = CONTEXT_MENU_DEFS[request.kind].find(
    (candidate) => candidate !== "separator" && candidate.id === actionId
  )
  return entry === undefined || entry === "separator" ? null : entry
}
