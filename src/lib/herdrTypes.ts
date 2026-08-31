/**
 * Frontend Herdr DTOs — mirror Rust `herdr_service` camelCase IPC shapes.
 * Raw snapshot payload keeps Herdr snake_case wire fields; normalized view
 * models live alongside for ADE Spaces/Agents/Panes UI.
 */

export type HerdrEventsStatus = "deferred" | "available" | "unavailable"

/** App-global Herdr binary preference. */
export type HerdrBinarySource = "global" | "default"

export type HerdrReadSource =
  | "visible"
  | "recent"
  | "recent-unwrapped"
  | "detection"

export type HerdrReadFormat = "text" | "ansi"

/** Connector open mode (backend wire). */
export type HerdrTerminalMode = "observe" | "control"

/** Attachment role reported by open result. */
export type HerdrTerminalRole = "observer" | "controller"

export type HerdrScrollDirection = "up" | "down"

export type HerdrAgentStatus = "idle" | "working" | "blocked" | "done" | "unknown"

export type HerdrConnectionState =
  | "idle"
  | "connecting"
  | "ready"
  | "unsupported"
  | "error"
  | "stopped"

/** `pane.split` direction (protocol-19). */
export type HerdrSplitDirection = "right" | "down"

/** `pane.zoom` mode (protocol-19). */
export type HerdrPaneZoomMode = "toggle" | "on" | "off"

/** Named persistent Herdr session from `herdr session list --json`. */
export interface HerdrNamedSession {
  name: string
  default: boolean
  running: boolean
  sessionDir: string
  socketPath: string
}

export interface HerdrServerCapability {
  running: boolean
  version?: string | null
  protocol?: number | null
  compatible?: boolean | null
  socketPath?: string | null
  capabilities?: unknown
}

export interface HerdrApiCapability {
  snapshot: boolean
  ping: boolean
  tabCreate: boolean
  workspaceFocus: boolean
  workspaceCreate: boolean
  workspaceRename: boolean
  workspaceClose: boolean
  tabRename: boolean
  tabClose: boolean
  tabFocus: boolean
  /** Protocol-19 `tab.move { tab_id, insert_index }`. */
  tabMove?: boolean
  paneFocus: boolean
  paneRename: boolean
  paneSplit: boolean
  paneZoom: boolean
  paneSwap: boolean
  paneClose: boolean
  layoutExport: boolean
  layoutSetSplitRatio: boolean
  /** Server-advertised Agent manifest catalog. */
  agentManifests?: boolean
  /** Starts a validated manifest kind in a freshly-created pane. */
  agentStart?: boolean
  agentGet: boolean
  agentRead: boolean
  eventsSubscribe: boolean
  /** Schema-gated read-only `worktree.list` (protocol 19). */
  worktreeList: boolean
  /** Advertised method names for honest menu gating. */
  methods: string[]
  schemaProtocol?: number | null
  schemaVersion?: number | null
  reason?: string | null
}

export interface HerdrTerminalCapability {
  observe: boolean
  control: boolean
  takeover: boolean
  input: boolean
  resize: boolean
  scroll: boolean
  release: boolean
  create: boolean
  reason?: string | null
}

export interface HerdrEventsCapability {
  status: HerdrEventsStatus
  reason?: string | null
}

export interface HerdrBinarySourceInfo {
  configured: HerdrBinarySource
  active?: HerdrBinarySource | null
  resolved?: HerdrBinarySource | null
  available: boolean
  path?: string | null
  reason?: string | null
  version?: string | null
  protocol?: number | null
  configuredAvailable?: boolean | null
  configuredPath?: string | null
  configuredReason?: string | null
  configuredVersion?: string | null
  configuredProtocol?: number | null
  configurationError?: string | null
  restartRequired: boolean
}

export interface HerdrBinarySourceSetResult {
  configured: HerdrBinarySource
  restartRequired: boolean
}

/** Nested capability document from `herdr_capabilities`. */
export interface HerdrCapabilities {
  binaryPath?: string | null
  binaryVersion?: string | null
  /** Protocol advertised by the selected binary (discovered, never hardcoded). */
  binaryProtocol?: number | null
  channel?: string | null
  binarySource: HerdrBinarySourceInfo
  server: HerdrServerCapability
  api: HerdrApiCapability
  terminal: HerdrTerminalCapability
  events: HerdrEventsCapability
}

export interface HerdrAgentDetails {
  terminalId: string
  agentStatus: string
  workspaceId: string
  tabId: string
  paneId: string
  focused: boolean
  revision: number
  agent?: string | null
  displayAgent?: string | null
  name?: string | null
  title?: string | null
  cwd?: string | null
  foregroundCwd?: string | null
  interactiveReady?: boolean | null
  launchPending?: boolean | null
  stateLabels: Record<string, string>
}

export interface HerdrAgentReadResult {
  paneId: string
  workspaceId: string
  tabId: string
  source: HerdrReadSource
  format: HerdrReadFormat
  text: string
  revision: number
  truncated: boolean
  /** True when Yuzora refused to deliver the full agent text (over 512 KiB). */
  tooLarge?: boolean
}

export interface HerdrAgentCatalogEntry {
  agent: string
  source: string
  sourceKind: string
  activeVersion?: string | null
  warning?: string | null
  /** Advisory Yuzora-process PATH detection; Herdr remains launch authority. */
  detectedBinaryPath?: string | null
  /** Backend-owned allowlist; callers send only a boolean opt-in. */
  bypassFlags: string[]
}

export interface HerdrAgentCreateRequest {
  sessionName?: string | null
  workspaceId: string
  kind: string
  bypassPermissions?: boolean | null
}

export interface HerdrAgentCreateResult {
  name: string
  kind: string
  terminalId: string
  paneId: string
  tabId: string
  workspaceId: string
  title?: string | null
}

export type HerdrSubscriptionEvent =
  | { type: "subscribed"; subscriptionId: string }
  | {
      type: "agent_status_changed"
      subscriptionId: string
      paneId: string
      workspaceId: string
      agentStatus: string
      agent?: string | null
      displayAgent?: string | null
      title?: string | null
      stateLabels: Record<string, string>
    }
  | {
      type: "pane_exited"
      subscriptionId: string
      paneId: string
      workspaceId: string
    }
  | {
      /** Dirty signal for worktree.created/opened/removed — inventory is re-listed. */
      type: "worktree_changed"
      subscriptionId: string
      kind: "created" | "opened" | "removed"
      workspaceId?: string | null
    }
  | {
      /** Dirty signal for tab/workspace topology — snapshot is the recovery truth. */
      type: "topology_changed"
      subscriptionId: string
      kind:
        | "tab.created"
        | "tab.closed"
        | "tab.moved"
        | "workspace.created"
        | "workspace.closed"
        | "workspace.moved"
        | "workspace.reordered"
      workspaceId?: string | null
      tabId?: string | null
    }
  | { type: "error"; subscriptionId: string; message: string }
  | { type: "disconnected"; subscriptionId: string; reason?: string | null }

export type HerdrAttentionKind = "blocked" | "done" | "unknown" | "error"

export interface HerdrAttentionItem {
  key: string
  sessionName: string
  paneId: string
  workspaceId?: string | null
  agentStatus: HerdrAgentStatus | string
  kind: HerdrAttentionKind
  title?: string | null
  displayAgent?: string | null
  seen: boolean
  updatedAt: number
}

/** Raw IPC result from `herdr_snapshot`. */
export interface HerdrSnapshotResult {
  protocol: number
  version: string
  /** Full Herdr snapshot object (snake_case wire fields preserved). */
  snapshot: unknown
}

export interface HerdrSpaceInfo {
  id: string
  label: string
  order: number
  focused: boolean
  activeTabId?: string | null
  path?: string | null
  status?: HerdrAgentStatus | null
  agentCount?: number
  terminalCount?: number
  /** Raw `workspaces[].tab_count` summary; prefer snapshot.tabs ownership when present. */
  tabCount?: number
  /** Read-only worktree provenance (snapshot + worktree.list merge). */
  repoKey?: string | null
  repoName?: string | null
  repoRoot?: string | null
  sourceCheckoutPath?: string | null
  branch?: string | null
  isLinkedWorktree?: boolean | null
  isDetached?: boolean | null
  isPrunable?: boolean | null
  isBare?: boolean | null
  worktreeLabel?: string | null
}

/** Protocol-19 `WorktreeSourceInfo` (camelCase IPC). */
export interface HerdrWorktreeSourceInfo {
  repoKey: string
  repoName: string
  repoRoot: string
  sourceCheckoutPath: string
  sourceWorkspaceId?: string | null
}

/** Protocol-19 `WorktreeInfo` (camelCase IPC). */
export interface HerdrWorktreeInfo {
  path: string
  branch?: string | null
  isBare: boolean
  isDetached: boolean
  isPrunable: boolean
  isLinkedWorktree: boolean
  label: string
  openWorkspaceId?: string | null
}

/** Result of schema-gated `worktree.list`. */
export interface HerdrWorktreeListResult {
  source: HerdrWorktreeSourceInfo
  worktrees: HerdrWorktreeInfo[]
}

/** Session-scoped inventory keyed by open workspace id only. */
export interface HerdrWorktreeInventory {
  sessionName: string
  lists: HerdrWorktreeListResult[]
  /** Query scopes that failed during this pass; omitted provenance is not treated as current. */
  failedScopes: string[]
  byOpenWorkspaceId: Record<
    string,
    { worktree: HerdrWorktreeInfo; source: HerdrWorktreeSourceInfo }
  >
}

export interface HerdrAgentInfo {
  id: string
  name: string
  status: HerdrAgentStatus
  workspaceId: string
  tabId?: string | null
  paneId?: string | null
  /** Live terminal identity; page keys bind to this, not paneId. */
  terminalId?: string | null
  title?: string | null
  displayAgent?: string | null
  focused?: boolean
  /** Owning named session (frontend-annotated). */
  sessionName?: string | null
  /** Owning Space label for ADE Agents list. */
  spaceLabel?: string | null
}

export interface HerdrTerminalInfo {
  terminalId: string
  paneId?: string | null
  workspaceId?: string | null
  tabId?: string | null
  title?: string | null
  cwd?: string | null
  status?: HerdrAgentStatus | null
}

/** Persistent Herdr tab with a representative pane/terminal for opening its page. */
export interface HerdrTabInfo {
  id: string
  label: string
  order: number
  workspaceId: string
  paneCount: number
  status: HerdrAgentStatus
  /** Active tab inside its owning Space. */
  active: boolean
  /** Globally focused tab in the named session. */
  focused: boolean
  paneId?: string | null
  terminalId?: string | null
  /** Owning named session (frontend-annotated). */
  sessionName?: string | null
}

/**
 * Frontend-normalized snapshot for Spaces/Agents/Tabs/Panes.
 * `herdrSessionId` is the named Herdr session (or legacy `live` fallback).
 */
export interface HerdrSnapshot {
  herdrSessionId: string
  protocol: number
  version: string
  spaces: HerdrSpaceInfo[]
  agents: HerdrAgentInfo[]
  tabs: HerdrTabInfo[]
  terminals: HerdrTerminalInfo[]
  focusedWorkspaceId?: string | null
  focusedTabId?: string | null
  focusedPaneId?: string | null
  focusedTerminalId?: string | null
  revision?: number | null
  /** Original wire payload for defensive consumers. */
  raw: unknown
}

export interface HerdrTerminalOpenResult {
  sessionId: string
  target: string
  mode: HerdrTerminalMode
  role: HerdrTerminalRole
  cols: number
  rows: number
  takeover: boolean
}

/**
 * Events delivered over the Tauri Channel passed to `herdr_terminal_open`.
 * Frame invariant: first frame must be full; seq must be contiguous (backend-enforced).
 */
export type HerdrTerminalEvent =
  | {
      type: "frame"
      sessionId: string
      seq: number
      full: boolean
      encoding: string
      width: number
      height: number
      bytesBase64: string
    }
  | { type: "closed"; sessionId: string; reason?: string | null }
  | {
      type: "resync"
      sessionId: string
      expectedSeq?: number | null
      receivedSeq?: number | null
      message: string
    }
  | { type: "error"; sessionId: string; code: string; message: string }

export interface HerdrCreateTerminalRequest {
  sessionName?: string | null
  workspaceId?: string | null
  title?: string | null
}

export interface HerdrCreateTerminalResult {
  terminalId: string
  paneId: string
  tabId: string
  workspaceId: string
  title?: string | null
}

export interface HerdrWorkspaceCreateRequest {
  sessionName?: string | null
  cwd?: string | null
  label?: string | null
  focus?: boolean | null
}

export interface HerdrWorkspaceCreateResult {
  workspaceId: string
  label: string
  path?: string | null
  tabId?: string | null
  terminalId?: string | null
  paneId?: string | null
}

/** Pane identity from `pane.split` / `pane.focus`. */
export interface HerdrPaneIdentity {
  paneId: string
  terminalId: string
  tabId: string
  workspaceId: string
  title?: string | null
}

/** Recursive BSP node from `layout.export` / `layout.set_split_ratio`. */
export type HerdrLayoutNode =
  | {
      type: "pane"
      paneId?: string | null
      label?: string | null
      cwd?: string | null
    }
  | {
      type: "split"
      direction: HerdrSplitDirection
      ratio: number
      first: HerdrLayoutNode
      second: HerdrLayoutNode
    }

/** Protocol-19 `LayoutDescription` (camelCase IPC). */
export interface HerdrLayoutDescription {
  workspaceId: string
  tabId: string
  zoomed: boolean
  focusedPaneId: string
  root: HerdrLayoutNode
}

export interface HerdrWorkspaceRenameRequest {
  sessionName?: string | null
  workspaceId: string
  label: string
}

export interface HerdrWorkspaceCloseRequest {
  sessionName?: string | null
  workspaceId: string
}

export interface HerdrTabCreateRequest {
  sessionName?: string | null
  workspaceId?: string | null
  label?: string | null
  cwd?: string | null
  focus?: boolean | null
}

export interface HerdrTabFocusRequest {
  sessionName?: string | null
  tabId: string
}

export interface HerdrTabRenameRequest {
  sessionName?: string | null
  tabId: string
  label: string
}

export interface HerdrTabCloseRequest {
  sessionName?: string | null
  tabId: string
}

export interface HerdrTabMoveRequest {
  sessionName?: string | null
  tabId: string
  insertIndex: number
}

export interface HerdrPaneFocusRequest {
  sessionName?: string | null
  paneId: string
}

export interface HerdrPaneRenameRequest {
  sessionName?: string | null
  paneId: string
  label?: string | null
}

export interface HerdrPaneSplitRequest {
  sessionName?: string | null
  direction: HerdrSplitDirection
  targetPaneId?: string | null
  workspaceId?: string | null
  cwd?: string | null
  ratio?: number | null
  focus?: boolean | null
}

export interface HerdrPaneZoomRequest {
  sessionName?: string | null
  paneId?: string | null
  mode?: HerdrPaneZoomMode | null
}

export interface HerdrPaneSwapRequest {
  sessionName?: string | null
  sourcePaneId?: string | null
  targetPaneId?: string | null
  paneId?: string | null
  direction?: string | null
}

export interface HerdrPaneCloseRequest {
  sessionName?: string | null
  paneId: string
}

export interface HerdrLayoutExportRequest {
  sessionName?: string | null
  tabId?: string | null
  paneId?: string | null
}

export interface HerdrLayoutSetSplitRatioRequest {
  sessionName?: string | null
  tabId?: string | null
  paneId?: string | null
  /** Boolean path: false = first, true = second. */
  path: boolean[]
  ratio: number
}

export interface HerdrSessionRuntime {
  capabilities: HerdrCapabilities | null
  snapshot: HerdrSnapshot | null
  /** Undecorated normalized snapshot used as the authoritative projection base. */
  baseSnapshot?: HerdrSnapshot | null
  /** Read-only worktree.list inventory for this named session. */
  worktreeInventory: HerdrWorktreeInventory | null
  connectionState: HerdrConnectionState
  errorMessage: string | null
}
