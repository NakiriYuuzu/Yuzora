import { Channel, invoke } from "@tauri-apps/api/core"

import type {
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
  HerdrRuntimeTarget,
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
  HerdrWorktreeListResult,
  HerdrWslDistribution,
  HerdrWslWorkspaceLocation
} from "./herdrTypes"

/**
 * Herdr IPC wrappers — payload names match Rust `herdr_service` commands.
 */

function runtimeTargetPayload(runtimeTarget?: HerdrRuntimeTarget | null): {
  runtimeTarget?: HerdrRuntimeTarget | null
} {
  // Omit the field for legacy WebView callers. Rust treats omission as Native.
  return runtimeTarget === undefined ? {} : { runtimeTarget: runtimeTarget ?? null }
}

export function herdrSessions(
  runtimeTarget?: HerdrRuntimeTarget | null
): Promise<HerdrNamedSession[]> {
  return invoke("herdr_sessions", runtimeTargetPayload(runtimeTarget))
}

/** Lists installed distros without starting any of them. */
export function herdrWslDistributions(): Promise<HerdrWslDistribution[]> {
  return invoke("herdr_wsl_distributions")
}

export function herdrWslRuntimeToHostPath(
  distro: string,
  runtimePath: string
): Promise<HerdrWslWorkspaceLocation> {
  return invoke("herdr_wsl_runtime_to_host_path", { distro, runtimePath })
}

export function herdrWslHostToRuntimePath(
  distro: string,
  hostPath: string
): Promise<HerdrWslWorkspaceLocation> {
  return invoke("herdr_wsl_host_to_runtime_path", { distro, hostPath })
}

export function herdrCapabilities(
  sessionName?: string | null,
  runtimeTarget?: HerdrRuntimeTarget | null
): Promise<HerdrCapabilities> {
  return invoke("herdr_capabilities", {
    sessionName: sessionName ?? null,
    ...runtimeTargetPayload(runtimeTarget)
  })
}

export function herdrSnapshot(
  sessionName?: string | null,
  runtimeTarget?: HerdrRuntimeTarget | null
): Promise<HerdrSnapshotResult> {
  return invoke("herdr_snapshot", {
    sessionName: sessionName ?? null,
    ...runtimeTargetPayload(runtimeTarget)
  })
}

export function herdrWorkspaceFocus(args: {
  runtimeTarget?: HerdrRuntimeTarget | null
  sessionName?: string | null
  workspaceId: string
}): Promise<void> {
  return invoke("herdr_workspace_focus", {
    sessionName: args.sessionName ?? null,
    workspaceId: args.workspaceId,
    ...runtimeTargetPayload(args.runtimeTarget)
  })
}

export function herdrWorkspaceCreate(
  request: HerdrWorkspaceCreateRequest = {}
): Promise<HerdrWorkspaceCreateResult> {
  return invoke("herdr_workspace_create", {
    sessionName: request.sessionName ?? null,
    cwd: request.cwd ?? null,
    label: request.label ?? null,
    focus: request.focus ?? true,
    ...runtimeTargetPayload(request.runtimeTarget)
  })
}

export function herdrWorkspaceRename(
  request: HerdrWorkspaceRenameRequest
): Promise<void> {
  return invoke("herdr_workspace_rename", {
    sessionName: request.sessionName ?? null,
    workspaceId: request.workspaceId,
    label: request.label,
    ...runtimeTargetPayload(request.runtimeTarget)
  })
}

export function herdrWorkspaceClose(
  request: HerdrWorkspaceCloseRequest
): Promise<void> {
  return invoke("herdr_workspace_close", {
    sessionName: request.sessionName ?? null,
    workspaceId: request.workspaceId,
    ...runtimeTargetPayload(request.runtimeTarget)
  })
}

/** Read-only protocol-19 `worktree.list` against the selected named session. */
export function herdrWorktreeList(args: {
  runtimeTarget?: HerdrRuntimeTarget | null
  sessionName?: string | null
  cwd?: string | null
  workspaceId?: string | null
} = {}): Promise<HerdrWorktreeListResult> {
  return invoke("herdr_worktree_list", {
    sessionName: args.sessionName ?? null,
    cwd: args.cwd ?? null,
    workspaceId: args.workspaceId ?? null,
    ...runtimeTargetPayload(args.runtimeTarget)
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
    focus: request.focus ?? true,
    ...runtimeTargetPayload(request.runtimeTarget)
  })
}

export function herdrTabFocus(request: HerdrTabFocusRequest): Promise<void> {
  return invoke("herdr_tab_focus", {
    sessionName: request.sessionName ?? null,
    tabId: request.tabId,
    ...runtimeTargetPayload(request.runtimeTarget)
  })
}

export function herdrTabRename(request: HerdrTabRenameRequest): Promise<void> {
  return invoke("herdr_tab_rename", {
    sessionName: request.sessionName ?? null,
    tabId: request.tabId,
    label: request.label,
    ...runtimeTargetPayload(request.runtimeTarget)
  })
}

export function herdrTabClose(request: HerdrTabCloseRequest): Promise<void> {
  return invoke("herdr_tab_close", {
    sessionName: request.sessionName ?? null,
    tabId: request.tabId,
    ...runtimeTargetPayload(request.runtimeTarget)
  })
}

export function herdrTabMove(request: HerdrTabMoveRequest): Promise<void> {
  return invoke("herdr_tab_move", {
    sessionName: request.sessionName ?? null,
    tabId: request.tabId,
    insertIndex: request.insertIndex,
    ...runtimeTargetPayload(request.runtimeTarget)
  })
}

export function herdrPaneFocus(request: HerdrPaneFocusRequest): Promise<void> {
  return invoke("herdr_pane_focus", {
    sessionName: request.sessionName ?? null,
    paneId: request.paneId,
    ...runtimeTargetPayload(request.runtimeTarget)
  })
}

export function herdrPaneRename(request: HerdrPaneRenameRequest): Promise<void> {
  return invoke("herdr_pane_rename", {
    sessionName: request.sessionName ?? null,
    paneId: request.paneId,
    label: request.label ?? null,
    ...runtimeTargetPayload(request.runtimeTarget)
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
    focus: request.focus ?? true,
    ...runtimeTargetPayload(request.runtimeTarget)
  })
}

export function herdrPaneZoom(request: HerdrPaneZoomRequest = {}): Promise<void> {
  return invoke("herdr_pane_zoom", {
    sessionName: request.sessionName ?? null,
    paneId: request.paneId ?? null,
    mode: request.mode ?? null,
    ...runtimeTargetPayload(request.runtimeTarget)
  })
}

export function herdrPaneSwap(request: HerdrPaneSwapRequest = {}): Promise<void> {
  return invoke("herdr_pane_swap", {
    sessionName: request.sessionName ?? null,
    sourcePaneId: request.sourcePaneId ?? null,
    targetPaneId: request.targetPaneId ?? null,
    paneId: request.paneId ?? null,
    direction: request.direction ?? null,
    ...runtimeTargetPayload(request.runtimeTarget)
  })
}

export function herdrPaneClose(request: HerdrPaneCloseRequest): Promise<void> {
  return invoke("herdr_pane_close", {
    sessionName: request.sessionName ?? null,
    paneId: request.paneId,
    ...runtimeTargetPayload(request.runtimeTarget)
  })
}

export function herdrLayoutExport(
  request: HerdrLayoutExportRequest = {}
): Promise<HerdrLayoutDescription> {
  return invoke("herdr_layout_export", {
    sessionName: request.sessionName ?? null,
    tabId: request.tabId ?? null,
    paneId: request.paneId ?? null,
    ...runtimeTargetPayload(request.runtimeTarget)
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
    ratio: request.ratio,
    ...runtimeTargetPayload(request.runtimeTarget)
  })
}

export function herdrTerminalOpen(args: {
  target: string
  mode?: HerdrTerminalMode | null
  takeover?: boolean | null
  cols: number
  rows: number
  runtimeTarget?: HerdrRuntimeTarget | null
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
    ...runtimeTargetPayload(args.runtimeTarget),
    onEvent: ch
  })
}

export function herdrTerminalInput(
  sessionId: string,
  text?: string | null,
  bytesBase64?: string | null,
  runtimeTarget?: HerdrRuntimeTarget | null
): Promise<void> {
  return invoke("herdr_terminal_input", {
    sessionId,
    text: text ?? null,
    bytesBase64: bytesBase64 ?? null,
    ...runtimeTargetPayload(runtimeTarget)
  })
}

export function herdrTerminalResize(
  sessionId: string,
  cols: number,
  rows: number,
  runtimeTarget?: HerdrRuntimeTarget | null
): Promise<void> {
  return invoke("herdr_terminal_resize", {
    sessionId,
    cols,
    rows,
    ...runtimeTargetPayload(runtimeTarget)
  })
}

export function herdrTerminalScroll(
  sessionId: string,
  direction: HerdrScrollDirection,
  lines: number,
  runtimeTarget?: HerdrRuntimeTarget | null
): Promise<void> {
  return invoke("herdr_terminal_scroll", {
    sessionId,
    direction,
    lines,
    ...runtimeTargetPayload(runtimeTarget)
  })
}

export function herdrTerminalRelease(
  sessionId: string,
  runtimeTarget?: HerdrRuntimeTarget | null
): Promise<void> {
  return invoke("herdr_terminal_release", {
    sessionId,
    ...runtimeTargetPayload(runtimeTarget)
  })
}

/** Create a new Herdr tab/root terminal via public `tab.create`. */
export function herdrTerminalCreate(
  request: HerdrCreateTerminalRequest = {}
): Promise<HerdrCreateTerminalResult> {
  return invoke("herdr_terminal_create", {
    sessionName: request.sessionName ?? null,
    workspaceId: request.workspaceId ?? null,
    title: request.title ?? null,
    ...runtimeTargetPayload(request.runtimeTarget)
  })
}

export function herdrBinarySourceGet(
  runtimeTarget?: HerdrRuntimeTarget | null
): Promise<HerdrBinarySourceInfo> {
  return invoke("herdr_binary_source_get", runtimeTargetPayload(runtimeTarget))
}

export function herdrBinarySourceSet(
  source: HerdrBinarySource,
  runtimeTarget?: HerdrRuntimeTarget | null
): Promise<HerdrBinarySourceSetResult> {
  return invoke("herdr_binary_source_set", {
    source,
    ...runtimeTargetPayload(runtimeTarget)
  })
}

export function herdrAgentGet(args: {
  runtimeTarget?: HerdrRuntimeTarget | null
  sessionName?: string | null
  target: string
}): Promise<HerdrAgentDetails> {
  return invoke("herdr_agent_get", {
    sessionName: args.sessionName ?? null,
    target: args.target,
    ...runtimeTargetPayload(args.runtimeTarget)
  })
}

export function herdrAgentRead(args: {
  runtimeTarget?: HerdrRuntimeTarget | null
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
    stripAnsi: args.stripAnsi ?? null,
    ...runtimeTargetPayload(args.runtimeTarget)
  })
}

export function herdrEventsSubscribe(args: {
  runtimeTarget?: HerdrRuntimeTarget | null
  sessionName?: string | null
  onEvent: (event: HerdrSubscriptionEvent) => void
}): Promise<string> {
  const ch = new Channel<HerdrSubscriptionEvent>()
  ch.onmessage = args.onEvent
  return invoke("herdr_events_subscribe", {
    sessionName: args.sessionName ?? null,
    ...runtimeTargetPayload(args.runtimeTarget),
    onEvent: ch
  })
}

export function herdrEventsRelease(
  subscriptionId: string,
  runtimeTarget?: HerdrRuntimeTarget | null
): Promise<void> {
  return invoke("herdr_events_release", {
    subscriptionId,
    ...runtimeTargetPayload(runtimeTarget)
  })
}
