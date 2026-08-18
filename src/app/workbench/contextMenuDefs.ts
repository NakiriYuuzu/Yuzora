import i18n from "@/lib/i18n"
import { getViewEntry } from "@/editor/viewRegistry"
import {
  herdrPaneClose,
  herdrPaneRename,
  herdrPaneSplit,
  herdrPaneSwap,
  herdrPaneZoom,
  herdrTabCreate,
  herdrWorkspaceClose,
  herdrWorkspaceRename
} from "@/lib/herdrIpc"
import { canonicalPathKey } from "@/lib/paths"
import { requestAppConfirmation } from "@/state/appDialogStore"
import { requestTextInputDialog } from "@/state/textInputDialogStore"
import { gitStage, gitUnstage } from "@/lib/ipc"
import { pickWorkspace } from "@/lib/workspaceActions"
import { dbProfileUiErrorCode, useDbStore } from "@/state/dbStore"
import { useGitStore } from "@/state/gitStore"
import { useGitRollbackDialogStore } from "@/state/gitRollbackDialogStore"
import { herdrStoreRuntimeKey, useHerdrStore } from "@/state/herdrStore"
import { normalizeHerdrRuntimeTarget, sameHerdrRuntimeTarget } from "@/lib/herdrRuntime"
import { useRecentWorkspacesStore } from "@/state/recentWorkspaces"
import { useUiStore } from "@/state/uiStore"
import { useSshStore } from "@/state/sshStore"
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
  beginRenameTerminal,
  closeTerminal,
  terminalTargetExists,
} from "@/terminal/terminalCommands"
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
  isConflictChange,
  isStageableChange,
  isUnstageableChange,
  uniquePaths,
  type GitChangeKey,
  type GitChangeRow,
} from "@/workbench/git/gitChangeSelection"
import { resolveProjectPresentation } from "@/app/workbench/projectPresentation"
import {
  closeHerdrTabIdempotently,
  openCreatedHerdrTabAndRequestName,
  renameHerdrTabWithDialog
} from "@/lib/herdrTabActions"

interface ResolvedContextMenuItem {
  type: "command"
  command: ContextMenuCommandDefinition
  label: string
  availability: ContextMenuAvailability
}

interface ResolvedContextMenuSeparator {
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
const DISABLED_NO_BACK_HISTORY = "contextMenu.disabled.noBackHistory"
const DISABLED_NO_FORWARD_HISTORY = "contextMenu.disabled.noForwardHistory"
const DISABLED_HERDR_UNAVAILABLE = "contextMenu.disabled.herdrUnavailable"
const DISABLED_HERDR_METHOD = "contextMenu.disabled.herdrMethodUnavailable"
const DISABLED_HERDR_NO_FOCUS = "contextMenu.disabled.herdrNoFocusedPane"

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

function recentWorkspaceAvailability(
  request: ContextMenuRequestFor<"recentWorkspace">
): ContextMenuAvailability {
  const key = canonicalPathKey(request.path)
  return useRecentWorkspacesStore.getState().list.some(
    (path) => canonicalPathKey(path) === key
  ) ? available() : disabled(DISABLED_TARGET)
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

function gitAvailability(request: ContextMenuRequestFor<"git"> | ContextMenuRequestFor<"status">) {
  const state = useGitStore.getState()
  if (state.environment?.status !== "ready" || state.environment.root !== request.repositoryRoot) {
    return disabled(DISABLED_NOT_REPOSITORY)
  }
  if (state.busy || state.snapshotStale) return disabled(DISABLED_GIT_BUSY)
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

function herdrSessionRuntime(
  sessionName: string,
  runtimeTarget?: ContextMenuRequestFor<"herdrTab">["runtimeTarget"]
) {
  const state = useHerdrStore.getState()
  const target = normalizeHerdrRuntimeTarget(runtimeTarget)
  const scopedSessions = state.sessions.filter((item) =>
    sameHerdrRuntimeTarget(item.runtimeTarget, target)
  )
  const session =
    scopedSessions.find((item) => item.name === sessionName) ??
    (sessionName === "live"
      ? scopedSessions.find((item) => item.default) ?? scopedSessions[0]
      : null)
  const resolvedName = session?.name ?? sessionName
  const runtime = state.runtimesBySession[
    herdrStoreRuntimeKey(resolvedName, target)
  ] ?? (target.kind === "native" ? state.runtimesBySession[resolvedName] : undefined)
  const capabilities = runtime?.capabilities ??
    (state.selectedSessionName === resolvedName &&
    sameHerdrRuntimeTarget(state.selectedRuntimeTarget, target)
      ? state.capabilities
      : null)
  return { session, capabilities }
}

function herdrMethodAvailability(
  sessionName: string,
  runtimeTarget: ContextMenuRequestFor<"herdrTab">["runtimeTarget"],
  flag:
    | "workspaceRename"
    | "workspaceClose"
    | "tabCreate"
    | "tabRename"
    | "tabClose"
    | "paneRename"
    | "paneSplit"
    | "paneZoom"
    | "paneSwap"
    | "paneClose",
  method: string
): ContextMenuAvailability {
  const { session, capabilities: caps } = herdrSessionRuntime(sessionName, runtimeTarget)
  if (!session?.running || !caps?.server.running) {
    return disabled(DISABLED_HERDR_UNAVAILABLE)
  }
  const flagOk = caps.api[flag] === true
  const methods = caps.api.methods ?? []
  const methodOk = flagOk && (methods.length === 0 || methods.includes(method))
  return methodOk ? available() : disabled(DISABLED_HERDR_METHOD)
}

async function afterHerdrMutation(
  sessionName: string,
  runtimeTarget?: ContextMenuRequestFor<"herdrTab">["runtimeTarget"]
): Promise<void> {
  // Missing persisted metadata is explicitly Native, not whichever runtime is
  // currently selected when the mutation settles.
  const target = normalizeHerdrRuntimeTarget(runtimeTarget)
  useHerdrStore.getState().bumpTopologyRevision()
  await useHerdrStore
    .getState()
    .refreshSnapshot(sessionName, target)
    .catch(() => undefined)
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

function gitChangeBusyAvailability(): ContextMenuAvailability {
  const state = useGitStore.getState()
  if (state.snapshotStale || state.environment?.status !== "ready") {
    return disabled(DISABLED_GIT_BUSY)
  }
  const busy = state.busy
  return busy
    ? disabled(String(i18n.t("contextMenu.disabled.gitBusyOperation", {
        ns: "menus",
        operation: busy,
      })))
    : available()
}

function gitChangeAvailability(
  request: ContextMenuRequestFor<"gitChange">,
  applies: (row: GitChangeKey) => boolean,
  requireAll = false
): ContextMenuAvailability {
  const rows = latestGitChangeRows(request)
  if (!rows) return hidden()
  const captured = gitChangeKeys(request)
  const capturedApplicable = captured.filter(applies)
  if (capturedApplicable.length === 0) return hidden()
  if (requireAll && capturedApplicable.length !== captured.length) return hidden()
  if (exactGitChanges(requireAll ? captured : capturedApplicable, rows).length !== (requireAll ? captured.length : capturedApplicable.length)) {
    return hidden()
  }
  return gitChangeBusyAvailability()
}

async function runGitChangeStageOperation(
  request: ContextMenuRequestFor<"gitChange">,
  staged: boolean
): Promise<"completed" | "cancelled"> {
  const applies = staged ? isStageableChange : isUnstageableChange
  const capturedApplicable = gitChangeKeys(request).filter(applies)
  const rows = latestGitChangeRows(request)
  if (!rows || capturedApplicable.length === 0) return CONTEXT_MENU_CANCELLED
  const exact = exactGitChanges(capturedApplicable, rows)
  if (exact.length !== capturedApplicable.length) return CONTEXT_MENU_CANCELLED
  const paths = uniquePaths(exact)
  if (paths.length === 0) return CONTEXT_MENU_CANCELLED
  const movedSides = Object.fromEntries(paths.map((path) => [path, staged]))
  const ok = await useGitStore.getState().runOp(
    staged ? "stage" : "unstage",
    () => staged
      ? gitStage(request.repositoryRoot, paths)
      : gitUnstage(request.repositoryRoot, paths),
    {
      afterMutationBeforeRefresh: () => {
        useUiStore.getState().applyGitChangeMovedSides(movedSides)
      }
    }
  )
  return ok ? CONTEXT_MENU_COMPLETED : CONTEXT_MENU_CANCELLED
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
  recentWorkspace: [
    item<"recentWorkspace">("cmEditRecentWorkspace", {
      availability: recentWorkspaceAvailability,
      danger: false,
      executor: (request) => {
        useUiStore.getState().openProjectEditor(request.path)
        return CONTEXT_MENU_COMPLETED
      },
    }),
    "separator",
    item<"recentWorkspace">("cmRemoveRecentWorkspace", {
      availability: recentWorkspaceAvailability,
      danger: false,
      executor: (request) => {
        const recent = useRecentWorkspacesStore.getState()
        const name = resolveProjectPresentation(
          request.path,
          recent.presentationFor(request.path)
        ).name
        recent.remove(request.path)

        const ui = useUiStore.getState()
        if (
          ui.projectEditorPath
          && canonicalPathKey(ui.projectEditorPath) === canonicalPathKey(request.path)
        ) {
          ui.closeProjectEditor()
        }
        ui.notifyRecentWorkspaceRemoved(name)
        return CONTEXT_MENU_COMPLETED
      },
    }),
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
      availability: (request) => {
        if (!tabExists(request)) return disabled(DISABLED_TARGET)
        const tab = useWorkspaceStore.getState().groups[request.groupIndex]?.tabs.find(
          (candidate) => candidate.path === request.path
        )
        if (tab?.kind === "preview" || tab?.kind === "markdown-preview") return hidden()
        return available()
      },
      danger: false,
      executor: legacy("cmCopyRel"),
    }),
    item<"tab">("cmSplit", {
      availability: (request) => {
        if (!tabExists(request)) return disabled(DISABLED_TARGET)
        const tab = useWorkspaceStore.getState().groups[request.groupIndex]?.tabs.find(
          (candidate) => candidate.path === request.path
        )
        if (tab?.kind === "preview" || tab?.kind === "markdown-preview") return hidden()
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
  terminalTab: [
    item<"terminalTab">("cmRenameTerminal", {
      availability: (request) => terminalTargetExists(request)
        ? available()
        : disabled(DISABLED_TARGET),
      danger: false,
      executor: beginRenameTerminal,
    }),
    "separator",
    item<"terminalTab">("cmCloseTerminal", {
      availability: (request) => terminalTargetExists(request)
        ? available()
        : disabled(DISABLED_TARGET),
      danger: true,
      executor: closeTerminal,
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
      availability: (request) => gitChangeAvailability(request, isStageableChange),
      danger: false,
      executor: (request) => runGitChangeStageOperation(request, true),
    }),
    item<"gitChange">("cmUnstageSelected", {
      availability: (request) => gitChangeAvailability(request, isUnstageableChange),
      danger: false,
      executor: (request) => runGitChangeStageOperation(request, false),
    }),
    "separator",
    item<"gitChange">("cmRollbackSelected", {
      availability: (request) => gitChangeAvailability(request, (row) => !isConflictChange(row), true),
      danger: true,
      executor: async (request) => {
        const rows = latestGitChangeRows(request)
        if (!rows) return CONTEXT_MENU_CANCELLED
        const targets = gitChangeKeys(request)
        if (targets.some(isConflictChange) || exactGitChanges(targets, rows).length !== targets.length) {
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
  herdrSpace: [
    item<"herdrSpace">("cmHerdrRenameSpace", {
      availability: (request) =>
        herdrMethodAvailability(request.sessionName, request.runtimeTarget, "workspaceRename", "workspace.rename"),
      danger: false,
      executor: async (request) => {
        const label = await requestTextInputDialog({
          title: i18n.t("contextMenu.prompt.rename", { ns: "menus" }),
          label: i18n.t("textInputDialog.nameLabel", { ns: "menus" }),
          initialValue: request.label ?? "",
          confirmLabel: i18n.t("textInputDialog.rename", { ns: "menus" })
        })
        if (!label || label === (request.label ?? "")) return CONTEXT_MENU_CANCELLED
        await herdrWorkspaceRename({
          sessionName: request.sessionName,
          runtimeTarget: request.runtimeTarget,
          workspaceId: request.workspaceId,
          label
        })
        await afterHerdrMutation(request.sessionName, request.runtimeTarget)
        return CONTEXT_MENU_COMPLETED
      },
    }),
    item<"herdrSpace">("cmHerdrNewTab", {
      availability: (request) =>
        herdrMethodAvailability(request.sessionName, request.runtimeTarget, "tabCreate", "tab.create"),
      danger: false,
      executor: async (request) => {
        const created = await herdrTabCreate({
          sessionName: request.sessionName,
          runtimeTarget: request.runtimeTarget,
          workspaceId: request.workspaceId,
          focus: true
        })
        await openCreatedHerdrTabAndRequestName({
          sessionName: request.sessionName,
          runtimeTarget: request.runtimeTarget,
          workspaceId: request.workspaceId,
          terminalId: created.terminalId,
          title: created.title,
          paneId: created.paneId,
          tabId: created.tabId
        })
        return CONTEXT_MENU_COMPLETED
      },
    }),
    item<"herdrSpace">("cmHerdrCloseSpace", {
      availability: (request) =>
        herdrMethodAvailability(request.sessionName, request.runtimeTarget, "workspaceClose", "workspace.close"),
      danger: true,
      executor: async (request) => {
        const ok = await requestAppConfirmation({
          title: i18n.t("contextMenu.confirm.closeHerdrSpaceTitle", { ns: "menus" }),
          description: i18n.t("contextMenu.confirm.closeHerdrSpace", {
            ns: "menus",
            name: request.label ?? request.workspaceId
          }),
          kind: "warning",
          destructive: true
        })
        if (!ok) return CONTEXT_MENU_CANCELLED
        await herdrWorkspaceClose({
          sessionName: request.sessionName,
          runtimeTarget: request.runtimeTarget,
          workspaceId: request.workspaceId
        })
        await afterHerdrMutation(request.sessionName, request.runtimeTarget)
        return CONTEXT_MENU_COMPLETED
      },
    }),
  ],
  herdrTab: [
    item<"herdrTab">("cmHerdrNewTab", {
      availability: (request) =>
        herdrMethodAvailability(request.sessionName, request.runtimeTarget, "tabCreate", "tab.create"),
      danger: false,
      executor: async (request) => {
        const created = await herdrTabCreate({
          sessionName: request.sessionName,
          runtimeTarget: request.runtimeTarget,
          workspaceId: request.workspaceId ?? undefined,
          focus: true
        })
        await openCreatedHerdrTabAndRequestName({
          sessionName: request.sessionName,
          runtimeTarget: request.runtimeTarget,
          workspaceId: request.workspaceId ?? null,
          terminalId: created.terminalId,
          title: created.title,
          paneId: created.paneId,
          tabId: created.tabId
        })
        return CONTEXT_MENU_COMPLETED
      },
    }),
    item<"herdrTab">("cmHerdrRenameTab", {
      availability: (request) =>
        request.tabId
          ? herdrMethodAvailability(request.sessionName, request.runtimeTarget, "tabRename", "tab.rename")
          : disabled(DISABLED_TARGET),
      danger: false,
      executor: async (request) => {
        if (!request.tabId) return CONTEXT_MENU_CANCELLED
        const renamed = await renameHerdrTabWithDialog({
          sessionName: request.sessionName,
          runtimeTarget: request.runtimeTarget,
          tabId: request.tabId,
          currentLabel: request.label ?? request.tabId,
          pagePath: request.pagePath
        })
        return renamed ? CONTEXT_MENU_COMPLETED : CONTEXT_MENU_CANCELLED
      },
    }),
    item<"herdrTab">("cmHerdrCloseTab", {
      availability: (request) =>
        request.tabId
          ? herdrMethodAvailability(request.sessionName, request.runtimeTarget, "tabClose", "tab.close")
          : disabled(DISABLED_TARGET),
      danger: true,
      executor: async (request) => {
        if (!request.tabId) return CONTEXT_MENU_CANCELLED
        const ok = await requestAppConfirmation({
          title: i18n.t("contextMenu.confirm.closeHerdrTabTitle", { ns: "menus" }),
          description: i18n.t("contextMenu.confirm.closeHerdrTab", {
            ns: "menus",
            name: request.label ?? request.tabId
          }),
          kind: "warning",
          destructive: true
        })
        if (!ok) return CONTEXT_MENU_CANCELLED
        await closeHerdrTabIdempotently(request.sessionName, request.tabId, request.runtimeTarget)
        if (request.pagePath) {
          useWorkspaceStore.getState().closeTabsByPath([request.pagePath])
        }
        await afterHerdrMutation(request.sessionName, request.runtimeTarget)
        return CONTEXT_MENU_COMPLETED
      },
    }),
  ],
  herdrPane: [
    item<"herdrPane">("cmHerdrRenamePane", {
      availability: (request) =>
        request.paneId
          ? herdrMethodAvailability(request.sessionName, request.runtimeTarget, "paneRename", "pane.rename")
          : disabled(DISABLED_TARGET),
      danger: false,
      executor: async (request) => {
        if (!request.paneId) return CONTEXT_MENU_CANCELLED
        const label = await requestTextInputDialog({
          title: i18n.t("contextMenu.prompt.rename", { ns: "menus" }),
          label: i18n.t("textInputDialog.nameLabel", { ns: "menus" }),
          initialValue: request.label ?? "",
          confirmLabel: i18n.t("textInputDialog.rename", { ns: "menus" })
        })
        if (!label || label === (request.label ?? "")) return CONTEXT_MENU_CANCELLED
        await herdrPaneRename({
          sessionName: request.sessionName,
          runtimeTarget: request.runtimeTarget,
          paneId: request.paneId,
          label
        })
        await afterHerdrMutation(request.sessionName, request.runtimeTarget)
        return CONTEXT_MENU_COMPLETED
      },
    }),
    item<"herdrPane">("cmHerdrClearPaneName", {
      availability: (request) => {
        if (!request.paneId || !request.label) return hidden()
        return herdrMethodAvailability(request.sessionName, request.runtimeTarget, "paneRename", "pane.rename")
      },
      danger: false,
      executor: async (request) => {
        if (!request.paneId) return CONTEXT_MENU_CANCELLED
        await herdrPaneRename({
          sessionName: request.sessionName,
          runtimeTarget: request.runtimeTarget,
          paneId: request.paneId,
          label: null
        })
        await afterHerdrMutation(request.sessionName, request.runtimeTarget)
        return CONTEXT_MENU_COMPLETED
      },
    }),
    item<"herdrPane">("cmHerdrSplitRight", {
      availability: (request) =>
        request.paneId
          ? herdrMethodAvailability(request.sessionName, request.runtimeTarget, "paneSplit", "pane.split")
          : disabled(DISABLED_TARGET),
      danger: false,
      executor: async (request) => {
        if (!request.paneId) return CONTEXT_MENU_CANCELLED
        await herdrPaneSplit({
          sessionName: request.sessionName,
          runtimeTarget: request.runtimeTarget,
          direction: "right",
          targetPaneId: request.paneId,
          workspaceId: request.workspaceId,
          focus: true
        })
        await afterHerdrMutation(request.sessionName, request.runtimeTarget)
        return CONTEXT_MENU_COMPLETED
      },
    }),
    item<"herdrPane">("cmHerdrSplitDown", {
      availability: (request) =>
        request.paneId
          ? herdrMethodAvailability(request.sessionName, request.runtimeTarget, "paneSplit", "pane.split")
          : disabled(DISABLED_TARGET),
      danger: false,
      executor: async (request) => {
        if (!request.paneId) return CONTEXT_MENU_CANCELLED
        await herdrPaneSplit({
          sessionName: request.sessionName,
          runtimeTarget: request.runtimeTarget,
          direction: "down",
          targetPaneId: request.paneId,
          workspaceId: request.workspaceId,
          focus: true
        })
        await afterHerdrMutation(request.sessionName, request.runtimeTarget)
        return CONTEXT_MENU_COMPLETED
      },
    }),
    item<"herdrPane">("cmHerdrZoomPane", {
      availability: (request) =>
        request.paneId
          ? herdrMethodAvailability(request.sessionName, request.runtimeTarget, "paneZoom", "pane.zoom")
          : disabled(DISABLED_TARGET),
      danger: false,
      executor: async (request) => {
        if (!request.paneId) return CONTEXT_MENU_CANCELLED
        await herdrPaneZoom({
          sessionName: request.sessionName,
          runtimeTarget: request.runtimeTarget,
          paneId: request.paneId,
          mode: "toggle"
        })
        await afterHerdrMutation(request.sessionName, request.runtimeTarget)
        return CONTEXT_MENU_COMPLETED
      },
    }),
    item<"herdrPane">("cmHerdrSwapPane", {
      availability: (request) => {
        if (!request.paneId) return disabled(DISABLED_TARGET)
        if (!request.focusedPaneId || request.focusedPaneId === request.paneId) {
          return disabled(DISABLED_HERDR_NO_FOCUS)
        }
        return herdrMethodAvailability(request.sessionName, request.runtimeTarget, "paneSwap", "pane.swap")
      },
      danger: false,
      executor: async (request) => {
        if (!request.paneId || !request.focusedPaneId) return CONTEXT_MENU_CANCELLED
        if (request.focusedPaneId === request.paneId) return CONTEXT_MENU_CANCELLED
        await herdrPaneSwap({
          sessionName: request.sessionName,
          runtimeTarget: request.runtimeTarget,
          sourcePaneId: request.paneId,
          targetPaneId: request.focusedPaneId
        })
        await afterHerdrMutation(request.sessionName, request.runtimeTarget)
        return CONTEXT_MENU_COMPLETED
      },
    }),
    item<"herdrPane">("cmHerdrClosePane", {
      availability: (request) =>
        request.paneId
          ? herdrMethodAvailability(request.sessionName, request.runtimeTarget, "paneClose", "pane.close")
          : disabled(DISABLED_TARGET),
      danger: true,
      executor: async (request) => {
        if (!request.paneId) return CONTEXT_MENU_CANCELLED
        const ok = await requestAppConfirmation({
          title: i18n.t("contextMenu.confirm.closeHerdrPaneTitle", { ns: "menus" }),
          description: i18n.t("contextMenu.confirm.closeHerdrPane", {
            ns: "menus",
            name: request.label ?? request.paneId
          }),
          kind: "warning",
          destructive: true
        })
        if (!ok) return CONTEXT_MENU_CANCELLED
        await herdrPaneClose({
          sessionName: request.sessionName,
          runtimeTarget: request.runtimeTarget,
          paneId: request.paneId
        })
        await afterHerdrMutation(request.sessionName, request.runtimeTarget)
        return CONTEXT_MENU_COMPLETED
      },
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
