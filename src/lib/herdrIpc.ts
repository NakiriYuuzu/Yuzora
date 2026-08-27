import { Channel, invoke } from "@tauri-apps/api/core"

import type {
  HerdrAgentCatalogEntry,
  HerdrAgentCreateRequest,
  HerdrAgentCreateResult,
  HerdrAgentDetails,
  HerdrAgentReadResult,
  HerdrBinarySource,
  HerdrBinarySourceInfo,
  HerdrBinarySourceSetResult,
  HerdrCapabilities,
  HerdrCreateTerminalRequest,
  HerdrCreateTerminalResult,
  HerdrLayoutDescription,
  HerdrLayoutExportRequest,
  HerdrLayoutSetSplitRatioRequest,
  HerdrNamedSession,
  HerdrPaneCloseRequest,
  HerdrPaneFocusRequest,
  HerdrPaneIdentity,
  HerdrPaneRenameRequest,
  HerdrPaneSplitRequest,
  HerdrPaneSwapRequest,
  HerdrPaneZoomRequest,
  HerdrReadFormat,
  HerdrReadSource,
  HerdrScrollDirection,
  HerdrSnapshotResult,
  HerdrSubscriptionEvent,
  HerdrTabCloseRequest,
  HerdrTabCreateRequest,
  HerdrTabFocusRequest,
  HerdrTabMoveRequest,
  HerdrTabRenameRequest,
  HerdrTerminalEvent,
  HerdrTerminalMode,
  HerdrTerminalOpenResult,
  HerdrWorkspaceCloseRequest,
  HerdrWorkspaceCreateRequest,
  HerdrWorkspaceCreateResult,
  HerdrWorkspaceRenameRequest,
  HerdrWorktreeListResult
} from "./herdrTypes"

/**
 * Herdr IPC wrappers — payload names match Rust `herdr_service` commands.
 */

export function herdrSessions(): Promise<HerdrNamedSession[]> {
  return invoke("herdr_sessions")
}

export function herdrCapabilities(
  sessionName?: string | null
): Promise<HerdrCapabilities> {
  return invoke("herdr_capabilities", {
    sessionName: sessionName ?? null
  })
}

export function herdrSnapshot(
  sessionName?: string | null
): Promise<HerdrSnapshotResult> {
  return invoke("herdr_snapshot", {
    sessionName: sessionName ?? null
  })
}

export function herdrWorkspaceFocus(args: {
  sessionName?: string | null
  workspaceId: string
}): Promise<void> {
  return invoke("herdr_workspace_focus", {
    sessionName: args.sessionName ?? null,
    workspaceId: args.workspaceId
  })
}

export function herdrWorkspaceCreate(
  request: HerdrWorkspaceCreateRequest = {}
): Promise<HerdrWorkspaceCreateResult> {
  return invoke("herdr_workspace_create", {
    sessionName: request.sessionName ?? null,
    cwd: request.cwd ?? null,
    label: request.label ?? null,
    focus: request.focus ?? true
  })
}

export function herdrWorkspaceRename(
  request: HerdrWorkspaceRenameRequest
): Promise<void> {
  return invoke("herdr_workspace_rename", {
    sessionName: request.sessionName ?? null,
    workspaceId: request.workspaceId,
    label: request.label
  })
}

export function herdrWorkspaceClose(
  request: HerdrWorkspaceCloseRequest
): Promise<void> {
  return invoke("herdr_workspace_close", {
    sessionName: request.sessionName ?? null,
    workspaceId: request.workspaceId
  })
}

/** Read-only protocol-19 `worktree.list` against the selected named session. */
export function herdrWorktreeList(args: {
  sessionName?: string | null
  cwd?: string | null
  workspaceId?: string | null
} = {}): Promise<HerdrWorktreeListResult> {
  return invoke("herdr_worktree_list", {
    sessionName: args.sessionName ?? null,
    cwd: args.cwd ?? null,
    workspaceId: args.workspaceId ?? null
  })
}

export function herdrTabCreate(
  request: HerdrTabCreateRequest = {}
): Promise<HerdrCreateTerminalResult> {
  return invoke("herdr_tab_create", {
    sessionName: request.sessionName ?? null,
    workspaceId: request.workspaceId ?? null,
    label: request.label ?? null,
    cwd: request.cwd ?? null,
    focus: request.focus ?? true
  })
}

export function herdrTabFocus(request: HerdrTabFocusRequest): Promise<void> {
  return invoke("herdr_tab_focus", {
    sessionName: request.sessionName ?? null,
    tabId: request.tabId
  })
}

export function herdrTabRename(request: HerdrTabRenameRequest): Promise<void> {
  return invoke("herdr_tab_rename", {
    sessionName: request.sessionName ?? null,
    tabId: request.tabId,
    label: request.label
  })
}

export function herdrTabClose(request: HerdrTabCloseRequest): Promise<void> {
  return invoke("herdr_tab_close", {
    sessionName: request.sessionName ?? null,
    tabId: request.tabId
  })
}

export function herdrTabMove(request: HerdrTabMoveRequest): Promise<void> {
  return invoke("herdr_tab_move", {
    sessionName: request.sessionName ?? null,
    tabId: request.tabId,
    insertIndex: request.insertIndex
  })
}

export function herdrPaneFocus(request: HerdrPaneFocusRequest): Promise<void> {
  return invoke("herdr_pane_focus", {
    sessionName: request.sessionName ?? null,
    paneId: request.paneId
  })
}

export function herdrPaneRename(request: HerdrPaneRenameRequest): Promise<void> {
  return invoke("herdr_pane_rename", {
    sessionName: request.sessionName ?? null,
    paneId: request.paneId,
    label: request.label ?? null
  })
}

export function herdrPaneSplit(
  request: HerdrPaneSplitRequest
): Promise<HerdrPaneIdentity> {
  return invoke("herdr_pane_split", {
    sessionName: request.sessionName ?? null,
    direction: request.direction,
    targetPaneId: request.targetPaneId ?? null,
    workspaceId: request.workspaceId ?? null,
    cwd: request.cwd ?? null,
    ratio: request.ratio ?? null,
    focus: request.focus ?? true
  })
}

export function herdrPaneZoom(request: HerdrPaneZoomRequest = {}): Promise<void> {
  return invoke("herdr_pane_zoom", {
    sessionName: request.sessionName ?? null,
    paneId: request.paneId ?? null,
    mode: request.mode ?? null
  })
}

export function herdrPaneSwap(request: HerdrPaneSwapRequest = {}): Promise<void> {
  return invoke("herdr_pane_swap", {
    sessionName: request.sessionName ?? null,
    sourcePaneId: request.sourcePaneId ?? null,
    targetPaneId: request.targetPaneId ?? null,
    paneId: request.paneId ?? null,
    direction: request.direction ?? null
  })
}

export function herdrPaneClose(request: HerdrPaneCloseRequest): Promise<void> {
  return invoke("herdr_pane_close", {
    sessionName: request.sessionName ?? null,
    paneId: request.paneId
  })
}

export function herdrLayoutExport(
  request: HerdrLayoutExportRequest = {}
): Promise<HerdrLayoutDescription> {
  return invoke("herdr_layout_export", {
    sessionName: request.sessionName ?? null,
    tabId: request.tabId ?? null,
    paneId: request.paneId ?? null
  })
}

export function herdrLayoutSetSplitRatio(
  request: HerdrLayoutSetSplitRatioRequest
): Promise<HerdrLayoutDescription> {
  return invoke("herdr_layout_set_split_ratio", {
    sessionName: request.sessionName ?? null,
    tabId: request.tabId ?? null,
    paneId: request.paneId ?? null,
    path: request.path,
    ratio: request.ratio
  })
}

export function herdrTerminalOpen(args: {
  target: string
  mode?: HerdrTerminalMode | null
  takeover?: boolean | null
  cols: number
  rows: number
  sessionName?: string | null
  onEvent: (event: HerdrTerminalEvent) => void
}): Promise<HerdrTerminalOpenResult> {
  const ch = new Channel<HerdrTerminalEvent>()
  ch.onmessage = args.onEvent
  return invoke("herdr_terminal_open", {
    target: args.target,
    mode: args.mode ?? null,
    takeover: args.takeover ?? null,
    cols: args.cols,
    rows: args.rows,
    sessionName: args.sessionName ?? null,
    onEvent: ch
  })
}

export function herdrTerminalInput(
  sessionId: string,
  text?: string | null,
  bytesBase64?: string | null
): Promise<void> {
  return invoke("herdr_terminal_input", {
    sessionId,
    text: text ?? null,
    bytesBase64: bytesBase64 ?? null
  })
}

export function herdrTerminalResize(
  sessionId: string,
  cols: number,
  rows: number
): Promise<void> {
  return invoke("herdr_terminal_resize", { sessionId, cols, rows })
}

export function herdrTerminalScroll(
  sessionId: string,
  direction: HerdrScrollDirection,
  lines: number
): Promise<void> {
  return invoke("herdr_terminal_scroll", { sessionId, direction, lines })
}

export function herdrTerminalRelease(sessionId: string): Promise<void> {
  return invoke("herdr_terminal_release", { sessionId })
}

/** Create a new Herdr tab/root terminal via public `tab.create`. */
export function herdrTerminalCreate(
  request: HerdrCreateTerminalRequest = {}
): Promise<HerdrCreateTerminalResult> {
  return invoke("herdr_terminal_create", {
    sessionName: request.sessionName ?? null,
    workspaceId: request.workspaceId ?? null,
    title: request.title ?? null
  })
}

export function herdrAgentCatalog(
  sessionName?: string | null
): Promise<HerdrAgentCatalogEntry[]> {
  return invoke("herdr_agent_catalog", {
    sessionName: sessionName ?? null
  })
}

export function herdrAgentCreate(
  request: HerdrAgentCreateRequest
): Promise<HerdrAgentCreateResult> {
  return invoke("herdr_agent_create", {
    sessionName: request.sessionName ?? null,
    workspaceId: request.workspaceId,
    kind: request.kind,
    bypassPermissions: request.bypassPermissions ?? false
  })
}

export function herdrBinarySourceGet(): Promise<HerdrBinarySourceInfo> {
  return invoke("herdr_binary_source_get")
}

export function herdrBinarySourceSet(
  source: HerdrBinarySource
): Promise<HerdrBinarySourceSetResult> {
  return invoke("herdr_binary_source_set", { source })
}

export function herdrAgentGet(args: {
  sessionName?: string | null
  target: string
}): Promise<HerdrAgentDetails> {
  return invoke("herdr_agent_get", {
    sessionName: args.sessionName ?? null,
    target: args.target
  })
}

export function herdrAgentRead(args: {
  sessionName?: string | null
  target: string
  source: HerdrReadSource
  format?: HerdrReadFormat | null
  lines?: number | null
  stripAnsi?: boolean | null
}): Promise<HerdrAgentReadResult> {
  return invoke("herdr_agent_read", {
    sessionName: args.sessionName ?? null,
    target: args.target,
    source: args.source,
    format: args.format ?? null,
    lines: args.lines ?? null,
    stripAnsi: args.stripAnsi ?? null
  })
}

export function herdrEventsSubscribe(args: {
  sessionName?: string | null
  onEvent: (event: HerdrSubscriptionEvent) => void
}): Promise<string> {
  const ch = new Channel<HerdrSubscriptionEvent>()
  ch.onmessage = args.onEvent
  return invoke("herdr_events_subscribe", {
    sessionName: args.sessionName ?? null,
    onEvent: ch
  })
}

export function herdrEventsRelease(subscriptionId: string): Promise<void> {
  return invoke("herdr_events_release", { subscriptionId })
}
