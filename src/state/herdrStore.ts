import { canonicalRuntimeWorkspace, parseRuntimeScope, runtimeOwner, sessionScope, StaleRuntimeResponse } from "@/lib/herdrProvider"
import { LOCAL_HOST_ID } from "@/lib/runtimeIdentity"
import { bindWorkspaceRoot, directoryForSelection, projectWorkspaceRoots } from "@/lib/herdrWorkspaceRoots"
import { create } from "zustand"

import {
  herdrCapabilities,
  herdrPaneFocus,
  herdrSessions,
  herdrSnapshot,
  herdrStartupStatus,
  herdrTabFocus,
  herdrTabRename,
  herdrTerminalCreate,
  herdrTerminalRelease,
  herdrWorkspaceCreate,
  herdrWorkspaceFocus,
  herdrWorktreeList
} from "@/lib/herdrIpc"
import {
  HERDR_LIVE_SESSION_ID,
  normalizeHerdrSnapshot
} from "@/lib/herdrNormalize"
import type {
  HerdrAgentInfo,
  HerdrAgentStatus,
  HerdrAttentionItem,
  HerdrAttentionKind,
  HerdrCapabilities,
  HerdrConnectionState,
  HerdrNamedSession,
  HerdrSessionRuntime,
  HerdrSnapshot,
  HerdrStartupStatus,
  HerdrSpaceInfo,
  HerdrSubscriptionEvent,
  HerdrTabInfo,
  HerdrTerminalMode,
  HerdrTerminalRole,
  HerdrWorktreeListResult
} from "@/lib/herdrTypes"
import {
  buildWorktreeInventory,
  mergeSpaceWorktreeProvenance
} from "@/lib/herdrWorktree"
import i18n from "@/lib/i18n"
import { confirmDiscardingUnsaved } from "@/lib/unsavedGuard"
import { openWorkspaceAtPath } from "@/lib/workspaceActions"
import { useAgentMruStore } from "./agentMruStore"
import { useUiStore } from "@/state/uiStore"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { canonicalPathKey, workspacePathBasename } from "@/lib/paths"

export interface HerdrAttachmentRecord {
  /** Backend connector session id. */
  sessionId: string
  /** Owning Yuzora page path (tab surface). */
  pagePath: string
  /** Leaf key within the page (paneId or terminalId). */
  paneKey: string
  herdrSessionId: string
  terminalId: string
  target: string
  paneId?: string | null
  mode: HerdrTerminalMode
  role: HerdrTerminalRole
  takeover: boolean
}

export type HerdrCreateTerminalResult = {
  herdrSessionId: string
  workspaceId: string
  terminalId: string
  paneId?: string | null
  tabId?: string | null
  title?: string | null
}

export type HerdrActivationResult =
  | { ok: true }
  | { ok: false; cancelled?: boolean; error?: string }

export function herdrAttentionKey(sessionName: string, paneId: string): string {
  return JSON.stringify([sessionName, paneId])
}

function attentionKindForStatus(
  status: string
): HerdrAttentionKind | null {
  if (status === "blocked") return "blocked"
  if (status === "done") return "done"
  if (status === "unknown") return "unknown"
  return null
}

interface HerdrState {
  herdrStartup: HerdrStartupStatus
  setHerdrStartup: (status: HerdrStartupStatus) => void
  sessions: HerdrNamedSession[]
  selectedSessionName: string | null
  runtimesBySession: Record<string, HerdrSessionRuntime>
  selectedSpaceBySession: Record<string, string | null>
  /** Convenience mirrors of the selected session runtime. */
  connectionState: HerdrConnectionState
  capabilities: HerdrCapabilities | null
  snapshot: HerdrSnapshot | null
  errorMessage: string | null
  selectedSpaceId: string | null
  /** Bumped after topology mutations so tab surfaces reload layout. */
  topologyRevision: number
  /** Latest Agent activation that targets one pane of a split tab. */
  paneFocusRequest: { sessionName: string; paneId: string; seq: number } | null
  attachments: Map<string, HerdrAttachmentRecord>
  /** Attention items keyed by sessionName::paneId. */
  attentionByKey: Map<string, HerdrAttentionItem>
  /** Live event subscription health for the selected session. */
  eventsHealthy: boolean
  eventsSubscriptionId: string | null
  /** `cached` is for idle polls; user actions and lifecycle changes read authoritatively. */
  refreshSessions: (options?: { cached?: boolean }) => Promise<void>
  selectSession: (sessionName: string) => Promise<void>
  bootstrap: (sessionName?: string | null) => Promise<void>
  refreshSnapshot: (sessionName?: string | null) => Promise<boolean>
  setWorkspaceReordering: (sessionName: string, active: boolean) => void
  applyWorkspaceOrder: (sessionName: string, workspaceIds: string[]) => boolean
  applySnapshot: (sessionName: string, snapshot: HerdrSnapshot) => void
  setSelectedSpaceId: (spaceId: string | null) => void
  clearError: () => void
  bumpTopologyRevision: () => void
  registerAttachment: (attachmentKey: string, record: HerdrAttachmentRecord) => void
  updateAttachmentPaneId: (attachmentKey: string, paneId: string | null | undefined) => void
  updateAttachmentMode: (
    attachmentKey: string,
    mode: HerdrTerminalMode,
    role: HerdrTerminalRole
  ) => void
  /** `after` delays only the connector release IPC (e.g. behind a mouse flush); the entry goes at once. */
  releaseAttachment: (attachmentKey: string, after?: Promise<unknown>) => Promise<void>
  releaseAttachmentsForPage: (pagePath: string) => Promise<void>
  releaseAllAttachments: () => Promise<void>
  createTerminalInSelectedSpace: () => Promise<HerdrCreateTerminalResult | null>
  createSpaceFromFolder: (
    cwd: string,
    label?: string | null
  ) => Promise<HerdrActivationResult & { space?: HerdrSpaceInfo | null }>
  canCreateTerminal: () => boolean
  /** workspace.create is intentionally independent of workspace.focus for empty sessions. */
  canCreateSpace: () => boolean
  canMutateSelectedSession: () => boolean
  canFocusSelectedTab: () => boolean
  canMoveSelectedTab: () => boolean
  createTerminalBlockedReason: () => string | null
  createSpaceBlockedReason: () => string | null
  mutationBlockedReason: () => string | null
  spaces: () => HerdrSpaceInfo[]
  agents: () => HerdrAgentInfo[]
  agentsInSpace: (spaceId: string) => HerdrAgentInfo[]
  tabs: () => HerdrTabInfo[]
  tabsInSpace: (spaceId: string) => HerdrTabInfo[]
  selectedSession: () => HerdrNamedSession | null
  activateSpace: (args: {
    sessionName: string
    workspaceId: string
    path?: string | null
  }) => Promise<HerdrActivationResult>
  /** `focusPane` also focuses `tab.paneId`; tab.focus alone restores the tab's last pane. */
  activateTab: (tab: HerdrTabInfo, options?: { focusPane?: boolean }) => Promise<HerdrActivationResult>
  activateAgent: (agent: HerdrAgentInfo) => Promise<HerdrActivationResult>
  /** Restore focused-Space Herdr pages from the snapshot without mutating Herdr. */
  restoreFocusedState: (sessionName: string) => Promise<HerdrActivationResult>
  applySubscriptionEvent: (sessionName: string, event: HerdrSubscriptionEvent) => void
  setEventsHealth: (
    sessionName: string,
    healthy: boolean,
    subscriptionId?: string | null
  ) => void
  /** Reconcile read-only worktree.list inventory for a named session. */
  refreshWorktreeInventory: (sessionName?: string | null) => Promise<void>
  markAttentionSeen: (sessionName: string, paneId: string) => void
  attentionItems: (sessionName?: string | null) => HerdrAttentionItem[]
}

export function isHerdrStartupPending(
  state: Pick<HerdrState, "herdrStartup">,
  session: HerdrNamedSession | null | undefined
): boolean {
  return state.herdrStartup.state === "starting" && !!session?.default && !session.hostId
}

/** IPC snapshots/inventories are JSON trees, not class instances or cyclic graphs. */
function equalHerdrData(a: unknown, b: unknown): boolean {
  if (Object.is(a, b)) return true
  if (!a || !b || typeof a !== "object" || typeof b !== "object") return false
  if (Array.isArray(a) !== Array.isArray(b)) return false
  const left = a as Record<string, unknown>, right = b as Record<string, unknown>
  const keys = Object.keys(left)
  return keys.length === Object.keys(right).length && keys.every(key =>
    Object.prototype.hasOwnProperty.call(right, key) && equalHerdrData(left[key], right[key]))
}

function shareById<T>(previous: T[], next: T[], key: (item: T) => string): T[] {
  if (!Array.isArray(previous) || !Array.isArray(next)) return next
  const byKey = new Map(previous.map(item => [key(item), item]))
  const shared = next.map(item => {
    const old = byKey.get(key(item))
    return old !== undefined && equalHerdrData(old, item) ? old : item
  })
  return shared.length === previous.length && shared.every((item, index) => item === previous[index])
    ? previous : shared
}

/** Keep identities of unchanged spaces/agents/tabs/terminals so consumers re-render only for what changed. */
function shareSnapshot(previous: HerdrSnapshot, next: HerdrSnapshot): HerdrSnapshot {
  if (equalHerdrData(previous, next)) return previous
  return {
    ...next,
    spaces: shareById(previous.spaces, next.spaces, item => item.id),
    agents: shareById(previous.agents, next.agents, item => item.id),
    tabs: shareById(previous.tabs, next.tabs, item => item.id),
    terminals: shareById(previous.terminals, next.terminals, item => item.terminalId)
  }
}

function sameFields<T extends object>(state: T, patch: Partial<T>): boolean {
  return (Object.keys(patch) as (keyof T)[]).every(key => Object.is(state[key], patch[key]))
}

function shareSessions(previous: HerdrNamedSession[], fetched: HerdrNamedSession[]): HerdrNamedSession[] {
  const byScope = new Map(previous.map(session => [sessionScope(session), session]))
  const shared = fetched.map(session => {
    const old = byScope.get(sessionScope(session))
    return old && equalHerdrData(old, session) ? old : session
  })
  return shared.length === previous.length && shared.every((session, index) => session === previous[index])
    ? previous : shared
}

function projectStartup(state: HerdrState): HerdrState {
  const session = state.sessions.find(session => session.default && !session.hostId)
  if (!session) return state
  const scope = sessionScope(session)!
  const runtime = state.runtimesBySession[scope]
  const pending = isHerdrStartupPending(state, session)
  if (!pending && (session.running || state.herdrStartup.state !== "failed")) return state
  const patch = buildRuntimePatch(state, scope, {
    connectionState: pending ? "connecting" : "error",
    errorMessage: pending ? null : state.herdrStartup.error,
    capabilities: pending ? null : runtime?.capabilities ?? null
  })
  return patch.runtimesBySession ? { ...state, ...patch } : state
}

function emptyRuntime(): HerdrSessionRuntime {
  return {
    capabilities: null,
    snapshot: null,
    baseSnapshot: null,
    worktreeInventory: null,
    connectionState: "idle",
    errorMessage: null
  }
}

function capabilityIdentity(state: HerdrState, scope: string): string | null {
  const owner = runtimeOwner(scope)
  if (!owner && parseRuntimeScope(scope).hostId !== LOCAL_HOST_ID) return null
  const session = state.sessions.find(item => sessionScope(item) === scope)
  return JSON.stringify([owner, session?.socketPath ?? null])
}

function withInventoryOnSnapshot(
  snapshot: HerdrSnapshot,
  inventory: HerdrSessionRuntime["worktreeInventory"]
): HerdrSnapshot {
  if (!inventory) return snapshot
  return {
    ...snapshot,
    spaces: mergeSpaceWorktreeProvenance(snapshot.spaces, inventory)
  }
}

function worktreeProjectionScope(snapshot: HerdrSnapshot | null | undefined): string {
  if (!snapshot) return ""
  return JSON.stringify(
    snapshot.spaces.map((space) => [
      space.id,
      space.path ?? null,
      space.repoKey ?? null,
      space.repoRoot ?? null,
      space.isLinkedWorktree ?? null
    ])
  )
}

function sameWorktreeProjectionScope(
  left: HerdrSnapshot | null | undefined,
  right: HerdrSnapshot
): boolean {
  if (left === right) return true
  if (!left || left.spaces.length !== right.spaces.length) return false
  return left.spaces.every((space, index) => {
    const other = right.spaces[index]
    return space.id === other.id &&
      (space.path ?? null) === (other.path ?? null) &&
      (space.repoKey ?? null) === (other.repoKey ?? null) &&
      (space.repoRoot ?? null) === (other.repoRoot ?? null) &&
      (space.isLinkedWorktree ?? null) === (other.isLinkedWorktree ?? null)
  })
}

function runtimeOf(
  state: Pick<HerdrState, "runtimesBySession" | "selectedSessionName">,
  sessionName?: string | null
): HerdrSessionRuntime {
  const key = sessionName ?? state.selectedSessionName
  if (!key) return emptyRuntime()
  return state.runtimesBySession[key] ?? emptyRuntime()
}

// Composed updates must never spread the old state over their other fields.
function buildRuntimePatch(
  state: HerdrState,
  sessionName: string,
  patch: Partial<HerdrSessionRuntime>
): Partial<HerdrState> {
  const previous = state.runtimesBySession[sessionName] ?? emptyRuntime()
  if (state.runtimesBySession[sessionName] && sameFields(previous, patch)) return {}
  const nextRuntime: HerdrSessionRuntime = { ...previous, ...patch }
  const runtimesBySession = {
    ...state.runtimesBySession,
    [sessionName]: nextRuntime
  }
  if (state.selectedSessionName === sessionName) {
    return {
      runtimesBySession,
      connectionState: nextRuntime.connectionState,
      capabilities: nextRuntime.capabilities,
      snapshot: nextRuntime.snapshot,
      errorMessage: nextRuntime.errorMessage
    }
  }
  return { runtimesBySession }
}

// Only for direct set callbacks: preserve state identity on a no-op.
function withRuntime(
  state: HerdrState,
  sessionName: string,
  patch: Partial<HerdrSessionRuntime>
): Partial<HerdrState> {
  const runtimePatch = buildRuntimePatch(state, sessionName, patch)
  return runtimePatch.runtimesBySession ? runtimePatch : state
}

function projectSelected(state: HerdrState, selectedSessionName: string | null): Partial<HerdrState> {
  const runtime = selectedSessionName
    ? (state.runtimesBySession[selectedSessionName] ?? emptyRuntime())
    : emptyRuntime()
  return {
    selectedSessionName,
    connectionState: runtime.connectionState,
    capabilities: runtime.capabilities,
    snapshot: runtime.snapshot,
    errorMessage: runtime.errorMessage,
    eventsHealthy: runtime.eventsHealthy ?? false,
    eventsSubscriptionId: runtime.eventsSubscriptionId ?? null,
    selectedSpaceId: selectedSessionName
      ? (state.selectedSpaceBySession[selectedSessionName] ?? null)
      : null
  }
}

function unsupportedReason(caps: HerdrCapabilities): string | null {
  if (!caps.binaryPath) {
    return caps.api.reason ?? caps.terminal.reason ?? "Herdr binary not found on PATH"
  }
  if (!caps.api.snapshot && caps.api.reason?.includes("not running")) {
    return caps.api.reason
  }
  if (!caps.api.snapshot && !caps.api.reason?.includes("not running")) {
    // Still allow stopped session metadata browsing when binary exists.
    if (caps.api.reason?.includes("not running")) return caps.api.reason
  }
  if (!caps.binaryPath) {
    return caps.api.reason ?? caps.terminal.reason ?? "Herdr binary not found on PATH"
  }
  // Unsupported only when binary itself cannot snapshot even if running.
  if (!caps.api.snapshot && caps.api.reason && !caps.server.running) {
    // Distinguish stopped vs truly unsupported below in bootstrap.
  }
  if (!caps.binaryPath) return "Herdr binary not found on PATH"
  return null
}

function isStoppedReason(message: string | null | undefined): boolean {
  return Boolean(message && message.includes("not running"))
}

function pathsMatch(a: string | null | undefined, b: string | null | undefined): boolean {
  if (!a || !b) return false
  try {
    return canonicalPathKey(a) === canonicalPathKey(b)
  } catch {
    return a.replace(/\/+$/, "") === b.replace(/\/+$/, "")
  }
}

function withFocusedTab(snapshot: HerdrSnapshot, tab: HerdrTabInfo): HerdrSnapshot {
  return {
    ...snapshot,
    spaces: snapshot.spaces.map((space) => ({
      ...space,
      focused: space.id === tab.workspaceId,
      activeTabId: space.id === tab.workspaceId ? tab.id : space.activeTabId
    })),
    tabs: snapshot.tabs.map((candidate) => ({
      ...candidate,
      active: candidate.workspaceId === tab.workspaceId ? candidate.id === tab.id : candidate.active,
      focused: candidate.id === tab.id
    })),
    focusedWorkspaceId: tab.workspaceId,
    focusedTabId: tab.id,
    focusedPaneId: tab.paneId ?? snapshot.focusedPaneId ?? null
  }
}

export const herdrInitialState = {
  herdrStartup: { state: "ready", error: null } as HerdrStartupStatus,
  sessions: [] as HerdrNamedSession[],
  selectedSessionName: null as string | null,
  runtimesBySession: {} as Record<string, HerdrSessionRuntime>,
  selectedSpaceBySession: {} as Record<string, string | null>,
  connectionState: "idle" as HerdrConnectionState,
  capabilities: null as HerdrCapabilities | null,
  snapshot: null as HerdrSnapshot | null,
  errorMessage: null as string | null,
  selectedSpaceId: null as string | null,
  topologyRevision: 0,
  paneFocusRequest: null as { sessionName: string; paneId: string; seq: number } | null,
  attachments: new Map<string, HerdrAttachmentRecord>(),
  attentionByKey: new Map<string, HerdrAttentionItem>(),
  eventsHealthy: false,
  eventsSubscriptionId: null as string | null
}

/** Module-level in-flight guards — not part of reactive state. */
let sessionsInFlight: Promise<void> | null = null
let sessionsInFlightCached = false
let startupGeneration = 0
const bootstrapInFlight = new Map<string, Promise<void>>()
const refreshInFlight = new Map<string, Promise<boolean>>()
const pendingRefresh = new Set<string>()
const MAX_REFRESH_RETRIES = 2
const worktreeInventoryInFlight = new Map<string, Promise<void>>()
const worktreeInventoryRequestedGeneration = new Map<string, number>()
const workspaceOrderGeneration = new Map<string, number>()
const workspaceReordering = new Set<string>()
let selectionTail: Promise<void> = Promise.resolve()
let pendingSelections = 0
/** One create transaction per named session prevents duplicate first Spaces/Agents. */
const spaceCreationInFlight = new Set<string>()
let sessionSelectionGeneration = 0

/** Serialize foreground focus RPCs across Spaces, tabs and named sessions.
 * A newer intent invalidates old UI commits immediately, then runs after the
 * previous native focus mutation settles so it remains authoritative too. */
async function acquireSelection(): Promise<() => void> {
  pendingSelections += 1
  const previous = selectionTail
  let release!: () => void
  const gate = new Promise<void>(resolve => { release = resolve })
  selectionTail = previous.catch(() => undefined).then(() => gate)
  await previous.catch(() => undefined)
  return () => { pendingSelections -= 1; release() }
}

export const useHerdrStore = create<HerdrState>((set, get) => ({
  ...herdrInitialState,

  setHerdrStartup(status) {
    startupGeneration++
    const state = get()
    const herdrStartup = equalHerdrData(state.herdrStartup, status) ? state.herdrStartup : status
    const projected = projectStartup(herdrStartup === state.herdrStartup ? state : { ...state, herdrStartup })
    if (projected !== state) set(projected)
  },

  selectedSession() {
    const name = get().selectedSessionName
    if (!name) return null
    return get().sessions.find((s) => sessionScope(s) === name) ?? null
  },

  async refreshSessions(options) {
    const cached = options?.cached === true
    if (sessionsInFlight) {
      // An explicit refresh must not settle for an idle poll's cached inventory.
      if (cached || !sessionsInFlightCached) return sessionsInFlight
      await sessionsInFlight
      return get().refreshSessions(options)
    }
    sessionsInFlightCached = cached
    sessionsInFlight = (async () => {
      try {
        const generation = startupGeneration
        const startup = await herdrStartupStatus()
        if (generation === startupGeneration) get().setHerdrStartup(startup)
        const fetched = await herdrSessions(cached)
        const state = get()
        const sessions = shareSessions(state.sessions, fetched)
        let selectedSessionName = state.selectedSessionName
        if (
          !selectedSessionName ||
          !sessions.some((session) => sessionScope(session) === selectedSessionName)
        ) {
          selectedSessionName =
            sessionScope(sessions.find((session) => session.default)) ??
            sessionScope(sessions[0]) ??
            null
        }
        const patch = { sessions, ...projectSelected(state, selectedSessionName) }
        const projected = projectStartup(sameFields(state, patch) ? state : { ...state, ...patch })
        if (projected !== state) set(projected)
        const local = get().sessions.find(session => session.default && !session.hostId)
        if (local && !local.running && get().herdrStartup.state === "ready" &&
          get().runtimesBySession[sessionScope(local)!]?.connectionState === "connecting") {
          await get().bootstrap(sessionScope(local))
        }
      } catch (error) {
        if (error instanceof StaleRuntimeResponse) return
        const message = error instanceof Error ? error.message : String(error)
        set((state) => {
          if (state.herdrStartup.state === "starting") return state
          const patch = {
            errorMessage: message,
            connectionState: state.snapshot || state.sessions.length > 0 ? state.connectionState : "error" as const
          }
          return sameFields(state, patch) ? state : patch
        })
      } finally {
        sessionsInFlight = null
      }
    })()
    return sessionsInFlight
  },

  async selectSession(sessionName) {
    const session = get().sessions.find((item) => sessionScope(item) === sessionName)
    if (!session) return
    sessionSelectionGeneration += 1
    set((state) => ({
      ...projectSelected(state, sessionName),
      selectedSpaceId: state.selectedSpaceBySession[sessionName] ?? null
    }))
    // Switching sessions must not close pages — only selection changes.
    if (isHerdrStartupPending(get(), session)) {
      set(state => projectStartup(state))
    } else if (session.running) {
      const state = get()
      const runtime = state.runtimesBySession[sessionName]
      const identity = capabilityIdentity(state, sessionName)
      // A live subscription already maintains this negotiated runtime. Selecting
      // it must not clear capabilities and tear down its terminal connectors.
      const ready = runtime?.connectionState === "ready" && !runtime.errorMessage
        && runtime.snapshot && runtime.capabilities?.server.running
        && runtime.capabilities.server.compatible !== false
        && runtime.eventsHealthy && runtime.eventsSubscriptionId
        && identity !== null && runtime.capabilityIdentity === identity
      if (!ready) await state.bootstrap(sessionName)
    } else {
      set((state) =>
        withRuntime(state, sessionName, {
          connectionState: "stopped",
          errorMessage:
            i18n.t("herdrNav.sessionStopped", {
              name: sessionName,
              defaultValue: `Session "${sessionName}" is not running. Start it with \`herdr session attach ${sessionName}\`.`
            }) ?? null
        })
      )
    }
  },

  async bootstrap(sessionName) {
    const resolved =
      sessionName ??
      get().selectedSessionName ??
      sessionScope(get().sessions.find((s) => s.default)) ??
      HERDR_LIVE_SESSION_ID
    const owner = JSON.stringify(runtimeOwner(resolved))
    const identity = capabilityIdentity(get(), resolved)
    const key = JSON.stringify([resolved, owner, identity])
    const current = () => JSON.stringify(runtimeOwner(resolved)) === owner
      && capabilityIdentity(get(), resolved) === identity
    const existing = bootstrapInFlight.get(key)
    if (existing) return existing

    const task = (async () => {
      set((state) => withRuntime(state, resolved, {
        connectionState: "connecting",
        capabilities: null,
        capabilityIdentity: null,
        errorMessage: null
      }))
      try {
        const named = get().sessions.find((s) => sessionScope(s) === resolved)
        if (isHerdrStartupPending(get(), named)) return
        if (named && !named.running) {
          set((state) =>
            withRuntime(state, resolved, {
              connectionState: "stopped",
              errorMessage: i18n.t("herdrNav.sessionStopped", { name: resolved })
            })
          )
          return
        }

        const capabilities = await herdrCapabilities(resolved)
        if (!current()) throw new StaleRuntimeResponse()
        if (isStoppedReason(capabilities.api.reason) || !capabilities.server.running) {
          set((state) =>
            withRuntime(state, resolved, {
              capabilities,
              connectionState: "stopped",
              errorMessage:
                capabilities.api.reason ??
                i18n.t("herdrNav.sessionStopped", { name: resolved })
            })
          )
          return
        }

        if (!capabilities.binaryPath) {
          set((state) =>
            withRuntime(state, resolved, {
              capabilities,
              connectionState: "unsupported",
              errorMessage: unsupportedReason(capabilities) ?? "Herdr binary not found"
            })
          )
          return
        }

        if (!capabilities.api.snapshot) {
          set((state) =>
            withRuntime(state, resolved, {
              capabilities,
              connectionState: "unsupported",
              errorMessage: unsupportedReason(capabilities) ?? capabilities.api.reason
            })
          )
          return
        }

        set((state) => withRuntime(state, resolved, { capabilities }))
        const raw = await herdrSnapshot(resolved)
        if (!current()) throw new StaleRuntimeResponse()
        const snapshot = normalizeHerdrSnapshot(raw, resolved)
        get().applySnapshot(resolved, snapshot)
        set((state) =>
          withRuntime(state, resolved, {
            connectionState: "ready",
            capabilityIdentity: identity,
            errorMessage: null
          })
        )
        // Authoritative inventory reconcile after snapshot recovery.
        await get().refreshWorktreeInventory(resolved)
      } catch (error) {
        if (!current() || error instanceof StaleRuntimeResponse) return
        const message = error instanceof Error ? error.message : String(error)
        if (isStoppedReason(message)) {
          set((state) =>
            withRuntime(state, resolved, {
              connectionState: "stopped",
              errorMessage: message
            })
          )
          return
        }
        const hadSnapshot = runtimeOf(get(), resolved).snapshot !== null
        set((state) =>
          withRuntime(state, resolved, {
            connectionState: hadSnapshot ? "ready" : "error",
            errorMessage: message
          })
        )
      }
    })().finally(() => {
      // A stopped Session settles synchronously; clearing only after the task
      // is registered keeps a settled bootstrap from blocking later ones.
      if (bootstrapInFlight.get(key) === task) bootstrapInFlight.delete(key)
    })
    bootstrapInFlight.set(key, task)
    return task
  },

  async refreshSnapshot(sessionName) {
    const resolved = sessionName ?? get().selectedSessionName
    if (!resolved) return false
    const named = get().sessions.find((s) => sessionScope(s) === resolved)
    if (isHerdrStartupPending(get(), named)) {
      set(state => projectStartup(state))
      return false
    }
    if (named && !named.running) {
      set((state) =>
        withRuntime(state, resolved, {
          connectionState: "stopped",
          errorMessage: i18n.t("herdrNav.sessionStopped", { name: resolved })
        })
      )
      return false
    }
    const existing = refreshInFlight.get(resolved)
    if (existing) {
      pendingRefresh.add(resolved)
      return existing
    }
    const task = (async () => {
      let consecutiveFailures = 0
      try {
        while (true) {
          let passSucceeded = false
          // Requests that arrive during this pass are authoritative trailing
          // refreshes. Consume only requests that predate the pass here.
          pendingRefresh.delete(resolved)
          try {
            const orderGeneration = workspaceOrderGeneration.get(resolved) ?? 0
            const raw = await herdrSnapshot(resolved)
            // A pre-drop or in-mutation snapshot cannot undo a confirmed order.
            if (workspaceReordering.has(resolved)) return false
            if (orderGeneration !== (workspaceOrderGeneration.get(resolved) ?? 0)) {
              pendingRefresh.add(resolved)
              continue
            }
            const snapshot = normalizeHerdrSnapshot(raw, resolved)
            get().applySnapshot(resolved, snapshot)
            set((state) =>
              withRuntime(state, resolved, {
                connectionState: "ready",
                errorMessage: null
              })
            )
            // Agent status/output updates do not invalidate repository inventory.
            // HerdrBridge refreshes it every 30s; topology changes clear it in applySnapshot.
            if (!runtimeOf(get(), resolved).worktreeInventory) {
              await get().refreshWorktreeInventory(resolved)
            }
            consecutiveFailures = 0
            passSucceeded = true
          } catch (error) {
            if (error instanceof StaleRuntimeResponse) return false
            const message = error instanceof Error ? error.message : String(error)
            if (isStoppedReason(message)) {
              pendingRefresh.delete(resolved)
              set((state) =>
                withRuntime(state, resolved, {
                  connectionState: "stopped",
                  errorMessage: message
                })
              )
              return false
            }
            consecutiveFailures += 1
            if (consecutiveFailures <= MAX_REFRESH_RETRIES) {
              pendingRefresh.add(resolved)
            }
            const hadSnapshot = runtimeOf(get(), resolved).snapshot !== null
            if (hadSnapshot) {
              set((state) =>
                withRuntime(state, resolved, {
                  errorMessage: message,
                  connectionState: "ready"
                })
              )
            } else {
              const current = runtimeOf(get(), resolved).connectionState
              set((state) =>
                withRuntime(state, resolved, {
                  errorMessage: message,
                  connectionState:
                    current === "connecting" || current === "idle" ? "error" : current
                })
              )
            }
          }
          if (!pendingRefresh.delete(resolved)) return passSucceeded
        }
      } finally {
        refreshInFlight.delete(resolved)
        pendingRefresh.delete(resolved)
      }
    })()
    refreshInFlight.set(resolved, task)
    return task
  },

  async refreshWorktreeInventory(sessionName) {
    const resolved = sessionName ?? get().selectedSessionName
    if (!resolved) return
    const named = get().sessions.find((s) => sessionScope(s) === resolved)
    if (named && !named.running) return
    const runtime = runtimeOf(get(), resolved)
    if (!runtime.capabilities?.api.worktreeList) return

    const owner = runtimeOwner(resolved)
    if (!owner && parseRuntimeScope(resolved).hostId !== LOCAL_HOST_ID) return
    const ownerIdentity = JSON.stringify(owner)
    const key = JSON.stringify([resolved, ownerIdentity])
    const isCurrent = () => {
      const state = get()
      const session = state.sessions.find((s) => sessionScope(s) === resolved)
      return JSON.stringify(runtimeOwner(resolved)) === ownerIdentity &&
        Boolean(state.runtimesBySession[resolved]?.capabilities?.api.worktreeList) &&
        (session ? session.running : !named)
    }
    const requestedGeneration =
      (worktreeInventoryRequestedGeneration.get(key) ?? 0) + 1
    worktreeInventoryRequestedGeneration.set(key, requestedGeneration)
    const existing = worktreeInventoryInFlight.get(key)
    if (existing) return existing

    const task = (async () => {
      while (isCurrent()) {
        const completedGeneration =
          worktreeInventoryRequestedGeneration.get(key) ?? requestedGeneration
        const current = runtimeOf(get(), resolved)
        const baseSnapshot = current.baseSnapshot ?? current.snapshot
        const scopeAtStart = worktreeProjectionScope(baseSnapshot)
        const spaces = baseSnapshot?.spaces ?? []
        const lists: HerdrWorktreeListResult[] = []
        const failedScopes: string[] = []
        const seenRepoKeys = new Set<string>()
        const representatives = spaces.filter((space) => {
          if (!space.repoKey) return true
          if (seenRepoKeys.has(space.repoKey)) return false
          seenRepoKeys.add(space.repoKey)
          return true
        })

        // Query one representative for each known repository. Unknown-repo
        // Spaces remain workspace-id scoped; path is never used as identity.
        for (const space of representatives) {
          let listed = false
          for (let attempt = 0; attempt < 2 && !listed; attempt += 1) {
            if (!isCurrent()) return
            try {
              const list = await herdrWorktreeList({
                sessionName: resolved,
                workspaceId: space.id
              })
              if (!isCurrent()) return
              lists.push(list)
              listed = true
            } catch (error) {
              if (!isCurrent() || error instanceof StaleRuntimeResponse) return
              if (attempt === 1) failedScopes.push(space.repoKey ?? space.id)
            }
          }
        }

        if (!isCurrent()) return
        const latest = runtimeOf(get(), resolved)
        // Status-only snapshots keep list results valid. Only changed repository
        // topology requires a fresh pass before projecting onto the latest snapshot.
        if (worktreeProjectionScope(latest.baseSnapshot ?? latest.snapshot) !== scopeAtStart) {
          continue
        }

        const inventory = buildWorktreeInventory(resolved, lists, failedScopes)
        set((state) => {
          const latest = runtimeOf(state, resolved)
          if (equalHerdrData(latest.worktreeInventory, inventory)) return state
          const projectionBase = latest.baseSnapshot ?? latest.snapshot
          const projectedSnapshot = projectionBase
            ? withInventoryOnSnapshot(projectionBase, inventory)
            : null
          return withRuntime(state, resolved, {
            worktreeInventory: inventory,
            snapshot: latest.snapshot && projectedSnapshot ? shareSnapshot(latest.snapshot, projectedSnapshot) : projectedSnapshot
          })
        })
        if (
          (worktreeInventoryRequestedGeneration.get(key) ?? 0) <=
          completedGeneration
        ) {
          break
        }
      }
    })().finally(() => {
      // Empty topologies can settle synchronously; retire only after registration.
      if (worktreeInventoryInFlight.get(key) === task) {
        worktreeInventoryInFlight.delete(key)
        worktreeInventoryRequestedGeneration.delete(key)
      }
    })
    worktreeInventoryInFlight.set(key, task)
    return task
  },

  setWorkspaceReordering(sessionName, active) {
    workspaceOrderGeneration.set(sessionName, (workspaceOrderGeneration.get(sessionName) ?? 0) + 1)
    if (active) workspaceReordering.add(sessionName)
    else workspaceReordering.delete(sessionName)
  },

  applyWorkspaceOrder(sessionName, workspaceIds) {
    const runtime = get().runtimesBySession[sessionName]
    const snapshot = runtime?.snapshot
    if (!snapshot || workspaceIds.length !== snapshot.spaces.length || new Set(workspaceIds).size !== workspaceIds.length || workspaceIds.some(id => !snapshot.spaces.some(s => s.id === id))) return false
    const order = (value: HerdrSnapshot) => ({ ...value, spaces: workspaceIds.map((id, index) => ({ ...value.spaces.find(s => s.id === id)!, order: index })) })
    workspaceOrderGeneration.set(sessionName, (workspaceOrderGeneration.get(sessionName) ?? 0) + 1)
    set(state => withRuntime(state, sessionName, { snapshot: order(snapshot), ...(runtime.baseSnapshot ? { baseSnapshot: order(runtime.baseSnapshot) } : {}) }))
    return true
  },

  applySnapshot(sessionName, snapshot) {
    snapshot = projectWorkspaceRoots(sessionName, snapshot)
    const previousRuntime = get().runtimesBySession[sessionName]
    if (previousRuntime?.baseSnapshot && equalHerdrData(previousRuntime.baseSnapshot, snapshot)) {
      snapshot = previousRuntime.baseSnapshot
    }
    const inventory = previousRuntime?.worktreeInventory ?? null
    const canReuseInventory =
      inventory !== null && sameWorktreeProjectionScope(previousRuntime?.baseSnapshot, snapshot)
    const reusableInventory = canReuseInventory ? inventory : null
    const projectedSnapshot = withInventoryOnSnapshot(snapshot, reusableInventory)
    const mergedSnapshot = previousRuntime?.snapshot
      ? shareSnapshot(previousRuntime.snapshot, projectedSnapshot) : projectedSnapshot
    set((state) => {
      const selectedStillExists = mergedSnapshot.spaces.some(
        (s) => s.id === state.selectedSpaceBySession[sessionName]
      )
      const focused =
        mergedSnapshot.focusedWorkspaceId ??
        mergedSnapshot.spaces.find((s) => s.focused)?.id ??
        null
      const fallback = mergedSnapshot.spaces[0]?.id ?? null
      // Herdr owns runtime focus. Mirror an explicit focused workspace; preserve
      // local selection only when the snapshot does not advertise one.
      const nextSpace =
        focused ??
        (selectedStillExists ? state.selectedSpaceBySession[sessionName] ?? null : fallback)
      const selectedSpaceBySession = state.selectedSpaceBySession[sessionName] === nextSpace
        ? state.selectedSpaceBySession
        : { ...state.selectedSpaceBySession, [sessionName]: nextSpace }
      const runtimePatch = buildRuntimePatch(state, sessionName, {
        baseSnapshot: snapshot,
        worktreeInventory: reusableInventory,
        snapshot: mergedSnapshot
      })
      // Snapshot reconciliation is the recovery truth after event disconnects;
      // protocol 19 exposes no event cursor to replay missed transitions.
      const attentionByKey = new Map(state.attentionByKey)
      const livePaneKeys = new Set<string>()
      for (const agent of mergedSnapshot.agents) {
        if (!agent.paneId) continue
        const key = herdrAttentionKey(sessionName, agent.paneId)
        livePaneKeys.add(key)
        const kind = attentionKindForStatus(agent.status)
        if (!kind) {
          attentionByKey.delete(key)
          continue
        }
        const previous = attentionByKey.get(key)
        const unchanged = previous?.kind === kind && previous.agentStatus === agent.status
        const item: HerdrAttentionItem = {
          key,
          sessionName,
          paneId: agent.paneId,
          workspaceId: agent.workspaceId,
          agentStatus: agent.status,
          kind,
          title: agent.title ?? previous?.title ?? null,
          displayAgent: agent.displayAgent ?? previous?.displayAgent ?? null,
          seen: unchanged ? previous?.seen ?? false : false,
          updatedAt: unchanged ? previous?.updatedAt ?? Date.now() : Date.now()
        }
        attentionByKey.set(key, previous && equalHerdrData(previous, item) ? previous : item)
      }
      for (const [key, item] of attentionByKey) {
        if (item.sessionName === sessionName && !livePaneKeys.has(key)) {
          attentionByKey.delete(key)
        }
      }
      const sharedAttention = attentionByKey.size === state.attentionByKey.size &&
        [...attentionByKey].every(([key, item]) => state.attentionByKey.get(key) === item)
        ? state.attentionByKey : attentionByKey
      const patch = {
        ...runtimePatch,
        selectedSpaceBySession,
        attentionByKey: sharedAttention,
        selectedSpaceId:
          state.selectedSessionName === sessionName
            ? nextSpace
            : state.selectedSpaceId
      }
      return sameFields(state, patch) ? state : patch
    })
    const defaultSessionName = sessionScope(get().sessions.find((session) => session.default && !session.hostId))
    useWorkspaceStore
      .getState()
      .reconcileHerdrPagesFromSnapshot(mergedSnapshot, defaultSessionName)
  },

  setSelectedSpaceId(spaceId) {
    const sessionName = get().selectedSessionName
    if (!sessionName) {
      set({ selectedSpaceId: spaceId })
      return
    }
    set((state) => ({
      selectedSpaceId: spaceId,
      selectedSpaceBySession: {
        ...state.selectedSpaceBySession,
        [sessionName]: spaceId
      }
    }))
  },

  clearError() {
    const sessionName = get().selectedSessionName
    if (!sessionName) {
      set({ errorMessage: null })
      return
    }
    set((state) => withRuntime(state, sessionName, { errorMessage: null }))
  },

  bumpTopologyRevision() {
    set((state) => ({ topologyRevision: state.topologyRevision + 1 }))
  },

  registerAttachment(attachmentKey, record) {
    set((state) => {
      const attachments = new Map(state.attachments)
      attachments.set(attachmentKey, record)
      return { attachments }
    })
  },

  updateAttachmentPaneId(attachmentKey, paneId) {
    set((state) => {
      const current = state.attachments.get(attachmentKey)
      if (!current) return state
      const attachments = new Map(state.attachments)
      attachments.set(attachmentKey, { ...current, paneId: paneId ?? null })
      return { attachments }
    })
  },

  updateAttachmentMode(attachmentKey, mode, role) {
    set((state) => {
      const current = state.attachments.get(attachmentKey)
      if (!current) return state
      const attachments = new Map(state.attachments)
      attachments.set(attachmentKey, {
        ...current,
        mode,
        role,
        takeover: mode === "control" ? current.takeover : false
      })
      return { attachments }
    })
  },

  async releaseAttachment(attachmentKey, after) {
    const record = get().attachments.get(attachmentKey)
    if (!record) return
    // Dropped right away: a remount may register the same key before `after` settles.
    set((state) => {
      const attachments = new Map(state.attachments)
      attachments.delete(attachmentKey)
      return { attachments }
    })
    await after?.catch(() => undefined)
    // Never pane.close — release connector only.
    await herdrTerminalRelease(record.sessionId).catch(() => undefined)
  },

  async releaseAttachmentsForPage(pagePath) {
    const entries = Array.from(get().attachments.entries()).filter(
      ([, record]) => record.pagePath === pagePath
    )
    if (entries.length === 0) return
    set((state) => {
      const attachments = new Map(state.attachments)
      for (const [key] of entries) attachments.delete(key)
      return { attachments }
    })
    await Promise.all(
      entries.map(([, record]) =>
        herdrTerminalRelease(record.sessionId).catch(() => undefined)
      )
    )
  },

  async releaseAllAttachments() {
    const entries = Array.from(get().attachments.entries())
    set({ attachments: new Map() })
    await Promise.all(
      entries.map(([, record]) =>
        herdrTerminalRelease(record.sessionId).catch(() => undefined)
      )
    )
  },

  canMutateSelectedSession() {
    const session = get().selectedSession()
    if (session && !session.running) return false
    const caps = get().capabilities
    return Boolean(caps?.server.running && caps.api.snapshot && caps.api.workspaceFocus)
  },

  canFocusSelectedTab() {
    return Boolean(get().canMutateSelectedSession() && get().capabilities?.api.tabFocus)
  },

  canMoveSelectedTab() {
    return Boolean(get().canMutateSelectedSession() && get().capabilities?.api.tabMove)
  },

  canCreateTerminal() {
    if (!get().canMutateSelectedSession()) return false
    const caps = get().capabilities
    return Boolean(caps?.api.tabCreate && caps.terminal.create)
  },

  canCreateSpace() {
    const state = get()
    const sessionName = state.selectedSessionName
    const session = state.selectedSession()
    if (!sessionName || (session && !session.running)) return false
    const caps = state.capabilities
    const previousSpace = state.selectedSpaceBySession[sessionName] ?? null
    return Boolean(
      caps?.server.running &&
      caps.api.snapshot &&
      caps.api.workspaceCreate &&
      (!previousSpace || caps.api.workspaceFocus)
    )
  },

  mutationBlockedReason() {
    const session = get().selectedSession()
    if (session && !session.running) {
      return i18n.t("herdrNav.sessionStopped", { name: session.name })
    }
    const caps = get().capabilities
    return caps?.api.reason ?? caps?.terminal.reason ?? null
  },

  createTerminalBlockedReason() {
    if (get().canCreateTerminal()) return null
    return (
      get().mutationBlockedReason() ??
      get().capabilities?.terminal.reason ??
      get().capabilities?.api.reason ??
      "Herdr tab.create unavailable"
    )
  },

  createSpaceBlockedReason() {
    if (get().canCreateSpace()) return null
    const state = get()
    const session = state.selectedSession()
    if (!state.selectedSessionName) {
      return i18n.t("herdrNav.selectSessionFirst", { ns: "workbench" })
    }
    if (session && !session.running) {
      return i18n.t("herdrNav.sessionStopped", { ns: "workbench", name: session.name })
    }
    const caps = state.capabilities
    const previousSpace = state.selectedSpaceBySession[state.selectedSessionName] ?? null
    if (
      previousSpace &&
      caps?.server.running &&
      caps.api.snapshot &&
      caps.api.workspaceCreate &&
      !caps.api.workspaceFocus
    ) {
      return "Herdr workspace.focus unavailable"
    }
    return caps?.api.reason ?? "Herdr workspace.create unavailable"
  },

  async createTerminalInSelectedSpace() {
    const { selectedSpaceId, selectedSessionName } = get()
    if (!selectedSpaceId || !selectedSessionName || !get().canCreateTerminal()) return null
    try {
      const selectedSpace = get().spaces().find((space) => space.id === selectedSpaceId)
      const folderName = selectedSpace?.path
        ? workspacePathBasename(selectedSpace.path)
        : null
      const title = folderName?.trim() || selectedSpace?.label?.trim() || null
      const created = await herdrTerminalCreate({
        sessionName: selectedSessionName,
        workspaceId: selectedSpaceId,
        title
      })
      set((state) => withRuntime(state, selectedSessionName, { errorMessage: null }))
      void get().refreshSnapshot(selectedSessionName)
      return {
        herdrSessionId: selectedSessionName,
        workspaceId: selectedSpaceId,
        terminalId: created.terminalId,
        paneId: created.paneId,
        tabId: created.tabId,
        title: created.title?.trim() || title
      }
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error)
      set((state) => withRuntime(state, selectedSessionName, { errorMessage: message }))
      return null
    }
  },

  async createSpaceFromFolder(cwd, label) {
    const stateBefore = get()
    const sessionName = stateBefore.selectedSessionName
    if (!sessionName || !get().canCreateSpace()) {
      return {
        ok: false,
        error: get().createSpaceBlockedReason() ?? "Herdr workspace.create unavailable"
      }
    }
    if (spaceCreationInFlight.has(sessionName)) {
      return {
        ok: false,
        error: i18n.t("herdrNav.createSpaceInProgress", { ns: "workbench" })
      }
    }
    spaceCreationInFlight.add(sessionName)
    try {
      const previousSpace = stateBefore.selectedSpaceBySession[sessionName] ?? null
      const currentWorkspace = useWorkspaceStore.getState().workspacePath
      const needsWorkspaceSwitch = Boolean(cwd && !pathsMatch(cwd, currentWorkspace))

      // Do not mutate Herdr before resolving potentially unsaved local edits.
      if (needsWorkspaceSwitch) {
        const proceed = await confirmDiscardingUnsaved({
          title: i18n.t("unsavedDialog.switchWorkspaceTitle", { ns: "menus" }),
          description: i18n.t("unsavedDialog.switchWorkspaceDescription", { ns: "menus" }),
          saveLabel: i18n.t("unsavedDialog.saveAll", { ns: "menus" })
        })
        if (!proceed) return { ok: false, cancelled: true }
      }

      let created
      try {
        cwd = await canonicalRuntimeWorkspace(sessionName, cwd)
        created = await herdrWorkspaceCreate({
          sessionName,
          cwd,
          label: label ?? null,
          focus: true
        })
        bindWorkspaceRoot(sessionName, created.workspaceId, cwd)
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error)
        set((state) => withRuntime(state, sessionName, { errorMessage: message }))
        return { ok: false, error: message }
      }

      const rollbackFocus = async () => {
        if (!previousSpace) return
        await herdrWorkspaceFocus({ sessionName, workspaceId: previousSpace }).catch(() => undefined)
      }
      if (needsWorkspaceSwitch) {
        try {
          const opened = await openWorkspaceAtPath(cwd, { skipUnsavedGuard: true })
          if (opened === false) {
            await rollbackFocus()
            void get().refreshSnapshot(sessionName)
            return { ok: false, cancelled: true }
          }
        } catch (error) {
          await rollbackFocus()
          void get().refreshSnapshot(sessionName)
          const message = error instanceof Error ? error.message : String(error)
          set((state) => withRuntime(state, sessionName, { errorMessage: message }))
          return { ok: false, error: message }
        }
      }

      if (created.tabId) {
        const terminalLabel =
          label?.trim() || cwd.replace(/[\\/]+$/, "").split(/[\\/]/).pop() || created.label
        await herdrTabRename({ sessionName, tabId: created.tabId, label: terminalLabel }).catch(
          () => undefined
        )
      }

      // A mutation may have succeeded even when follow-up state fails. Never
      // pretend success or create a speculative local Space in that case.
      if (!(await get().refreshSnapshot(sessionName))) {
        const message = i18n.t("herdrNav.createSpaceAppliedButRefreshFailed", { ns: "workbench" })
        set((state) => withRuntime(state, sessionName, { errorMessage: message }))
        return { ok: false, error: message }
      }
      const space = runtimeOf(get(), sessionName).snapshot?.spaces.find(
        (item) => item.id === created.workspaceId
      )
      if (!space) {
        const message = i18n.t("herdrNav.createSpaceMissingAfterRefresh", { ns: "workbench" })
        set((state) => withRuntime(state, sessionName, { errorMessage: message }))
        return { ok: false, error: message }
      }
      get().setSelectedSpaceId(created.workspaceId)
      set((state) => withRuntime(state, sessionName, { errorMessage: null }))
      return { ok: true, space }
    } finally {
      spaceCreationInFlight.delete(sessionName)
    }
  },

  spaces() {
    return get().snapshot?.spaces ?? []
  },

  agents() {
    return get().snapshot?.agents ?? []
  },

  agentsInSpace(spaceId) {
    return (get().snapshot?.agents ?? []).filter((agent) => agent.workspaceId === spaceId)
  },

  tabs() {
    return get().snapshot?.tabs ?? []
  },

  tabsInSpace(spaceId) {
    return (get().snapshot?.tabs ?? []).filter((tab) => tab.workspaceId === spaceId)
  },

  async activateSpace({ sessionName, workspaceId, path }) {
    const stateBefore = get()
    const previousSession = stateBefore.selectedSessionName
    const previousSpace =
      previousSession != null
        ? stateBefore.selectedSpaceBySession[previousSession] ?? null
        : null

    const session = stateBefore.sessions.find((item) => sessionScope(item) === sessionName)
    if (session && !session.running) {
      return {
        ok: false,
        error: i18n.t("herdrNav.sessionStopped", { name: sessionName })
      }
    }
    const targetCaps = stateBefore.runtimesBySession[sessionName]?.capabilities
    if (!targetCaps?.server.running || !targetCaps.api.workspaceFocus) {
      return {
        ok: false,
        error: targetCaps?.api.reason ?? "herdr workspace.focus unavailable"
      }
    }

    // A non-Git Space has no worktree metadata. Explicit selection can open
    // its pane directory after the host canonicalizes it, just like a picker.
    path ??= directoryForSelection(stateBefore.runtimesBySession[sessionName]?.snapshot ?? null, workspaceId)
    const currentWorkspace = useWorkspaceStore.getState().workspacePath
    const needsWorkspaceSwitch = Boolean(path && !pathsMatch(path, currentWorkspace))

    const activationGeneration = ++sessionSelectionGeneration
    const isLatestActivation = () => sessionSelectionGeneration === activationGeneration

    // 1) Unsaved guard BEFORE any Herdr/Yuzora mutation.
    if (needsWorkspaceSwitch) {
      const proceed = await confirmDiscardingUnsaved({
        title: i18n.t("unsavedDialog.switchWorkspaceTitle", { ns: "menus" }),
        description: i18n.t("unsavedDialog.switchWorkspaceDescription", { ns: "menus" }),
        saveLabel: i18n.t("unsavedDialog.saveAll", { ns: "menus" })
      })
      if (!proceed) {
        return { ok: false, cancelled: true }
      }
    }

    const releaseActivation = await acquireSelection()
    try {
      if (!isLatestActivation()) return { ok: false, cancelled: true }
      if (previousSession && previousSpace) {
        useWorkspaceStore.getState().rememberSpaceNavigation(previousSession, previousSpace)
      }

      // 2) Focus Herdr Space on the target running session.
      try {
        if (path) {
          path = await canonicalRuntimeWorkspace(sessionName, path)
          if (!isLatestActivation()) return { ok: false, cancelled: true }
          bindWorkspaceRoot(sessionName, workspaceId, path)
        }
        await herdrWorkspaceFocus({ sessionName, workspaceId })
        if (!isLatestActivation()) return { ok: false, cancelled: true }
      } catch (error) {
        if (!isLatestActivation()) return { ok: false, cancelled: true }
        const message = error instanceof Error ? error.message : String(error)
        return { ok: false, error: message }
      }

      // 3) Guarded Yuzora workspace switch when a Space path exists.
      if (path && !pathsMatch(path, useWorkspaceStore.getState().workspacePath)) {
        try {
          const opened = await openWorkspaceAtPath(path, {
            skipUnsavedGuard: true,
            shouldOpen: isLatestActivation
          })
          if (!isLatestActivation()) return { ok: false, cancelled: true }
          // The unsaved preflight already completed before Herdr focus.
          if (opened === false) {
            // Best-effort rollback Herdr focus.
            if (previousSession && previousSpace) {
              await herdrWorkspaceFocus({
                sessionName: previousSession,
                workspaceId: previousSpace
              }).catch(() => undefined)
            }
            return { ok: false, cancelled: true }
          }
        } catch (error) {
          if (!isLatestActivation()) return { ok: false, cancelled: true }
          if (previousSession && previousSpace) {
            await herdrWorkspaceFocus({
              sessionName: previousSession,
              workspaceId: previousSpace
            }).catch(() => undefined)
          }
          const message = error instanceof Error ? error.message : String(error)
          return { ok: false, error: message }
        }
      }

      if (!isLatestActivation()) return { ok: false, cancelled: true }
      // 4) Commit session/Space selection only after success. workspace.focus
      // selects the Space's active tab in Herdr, so mirror that known topology
      // immediately instead of waiting up to one bridge-poll interval.
      set((state) => {
        const selectedSpaceBySession = {
          ...state.selectedSpaceBySession,
          [sessionName]: workspaceId
        }
        const runtime = state.runtimesBySession[sessionName]
        const snapshot = runtime?.snapshot
        const space = snapshot?.spaces.find((item) => item.id === workspaceId)
        const activeTab = snapshot?.tabs.find(
          (tab) =>
            tab.workspaceId === workspaceId &&
            (tab.id === space?.activeTabId || (!space?.activeTabId && tab.active))
        )
        return {
          selectedSpaceBySession,
          ...projectSelected({ ...state, selectedSpaceBySession }, sessionName),
          selectedSpaceId: workspaceId,
          ...(snapshot && activeTab
            ? buildRuntimePatch(state, sessionName, {
                errorMessage: null,
                snapshot: withFocusedTab(snapshot, activeTab),
                ...(runtime?.baseSnapshot
                  ? { baseSnapshot: withFocusedTab(runtime.baseSnapshot, activeTab) }
                  : {})
              })
            : {})
        }
      })

      const focusedSnapshot = get().runtimesBySession[sessionName]?.snapshot
      if (focusedSnapshot?.focusedWorkspaceId === workspaceId) {
        useWorkspaceStore.getState().hydrateHerdrPagesFromSnapshot(
          focusedSnapshot,
          sessionScope(get().sessions.find((item) => item.default && !item.hostId))
        )
      }
      const activeTab = focusedSnapshot?.tabs.find(
        (tab) => tab.workspaceId === workspaceId && tab.id === focusedSnapshot.focusedTabId
      )
      if (activeTab?.terminalId) {
        useWorkspaceStore.getState().openHerdrTerminalPage({
          herdrSessionId: sessionName,
          terminalId: activeTab.terminalId,
          title: activeTab.label,
          paneId: activeTab.paneId ?? focusedSnapshot?.focusedPaneId ?? null,
          herdrTabId: activeTab.id,
          herdrWorkspaceId: activeTab.workspaceId
        })
        if (activeTab.paneId || focusedSnapshot?.focusedPaneId) {
          get().markAttentionSeen(
            sessionName,
            activeTab.paneId ?? focusedSnapshot!.focusedPaneId!
          )
        }
      }
      useWorkspaceStore.getState().restoreSpaceNavigation(sessionName, workspaceId)
      useWorkspaceStore.getState().rememberSpaceNavigation(sessionName, workspaceId)
      useUiStore.getState().setMode("ade")
      return { ok: true }
    } finally {
      releaseActivation()
    }
  },

  async activateTab(tab, options) {
    const sessionName = tab.sessionName ?? get().selectedSessionName ?? HERDR_LIVE_SESSION_ID
    if (!tab.terminalId) {
      return { ok: false, error: "Herdr tab has no terminalId" }
    }

    const stateBefore = get()
    const session = stateBefore.sessions.find((item) => sessionScope(item) === sessionName)
    const runtime = stateBefore.runtimesBySession[sessionName]
    if (session && !session.running) {
      return {
        ok: false,
        error: i18n.t("herdrNav.sessionStopped", { name: sessionName })
      }
    }
    if (
      !runtime?.capabilities?.server.running ||
      !runtime.capabilities.api.workspaceFocus ||
      !runtime.capabilities.api.tabFocus
    ) {
      return {
        ok: false,
        error: runtime?.capabilities?.api.reason ?? "herdr workspace.focus/tab.focus unavailable"
      }
    }

    let space = runtime.snapshot?.spaces.find((item) => item.id === tab.workspaceId)
    if (!space) return { ok: false, error: "Herdr Space is unavailable" }
    space = { ...space, path: directoryForSelection(runtime.snapshot, tab.workspaceId, tab.paneId) }
    const currentWorkspace = useWorkspaceStore.getState().workspacePath
    const needsWorkspaceSwitch = Boolean(space?.path && !pathsMatch(space.path, currentWorkspace))

    const activationGeneration = ++sessionSelectionGeneration
    const isLatestActivation = () => sessionSelectionGeneration === activationGeneration
    const focusPaneId = options?.focusPane ? tab.paneId ?? null : null

    // Unsaved preflight must complete before workspace.focus or tab.focus.
    if (needsWorkspaceSwitch) {
      const proceed = await confirmDiscardingUnsaved({
        title: i18n.t("unsavedDialog.switchWorkspaceTitle", { ns: "menus" }),
        description: i18n.t("unsavedDialog.switchWorkspaceDescription", { ns: "menus" }),
        saveLabel: i18n.t("unsavedDialog.saveAll", { ns: "menus" })
      })
      if (!proceed) return { ok: false, cancelled: true }
    }

    const releaseActivation = await acquireSelection()

    try {
      if (!isLatestActivation()) return { ok: false, cancelled: true }
      if (stateBefore.selectedSessionName && stateBefore.selectedSpaceId) {
        useWorkspaceStore.getState().rememberSpaceNavigation(stateBefore.selectedSessionName, stateBefore.selectedSpaceId)
      }

      // Capture rollback state only after earlier mutations in this session
      // have settled. The foreground queue prevents a stale RPC from applying
      // after a newer activation and stealing authoritative Herdr focus.
      const stateAtMutation = get()
      const previousRuntime = stateAtMutation.runtimesBySession[sessionName]
      const previousSpace = previousRuntime?.snapshot?.focusedWorkspaceId ??
        stateAtMutation.selectedSpaceBySession[sessionName] ??
        null
      const previousTab = previousRuntime?.snapshot?.focusedTabId ?? null

      const rollbackFocus = async () => {
        if (!previousSpace) return
        await herdrWorkspaceFocus({
          sessionName,
          workspaceId: previousSpace
        }).catch(() => undefined)
        if (previousTab && isLatestActivation()) {
          await herdrTabFocus({
            sessionName,
            tabId: previousTab
          }).catch(() => undefined)
        }
      }

      try {
        if (space.path) {
          const path = await canonicalRuntimeWorkspace(sessionName, space.path)
          if (!isLatestActivation()) return { ok: false, cancelled: true }
          space = { ...space, path }
          bindWorkspaceRoot(sessionName, tab.workspaceId, path)
        }
        await herdrWorkspaceFocus({ sessionName, workspaceId: tab.workspaceId })
        if (!isLatestActivation()) return { ok: false, cancelled: true }
        await herdrTabFocus({ sessionName, tabId: tab.id })
        if (!isLatestActivation()) return { ok: false, cancelled: true }
        if (focusPaneId && runtime.capabilities.api.paneFocus) {
          // Best effort: the tab is already active, and the page still selects the pane.
          try { await herdrPaneFocus({ sessionName, paneId: focusPaneId }) } catch { /* keep the tab */ }
          if (!isLatestActivation()) return { ok: false, cancelled: true }
        }
      } catch (error) {
        if (!isLatestActivation()) return { ok: false, cancelled: true }
        await rollbackFocus()
        if (!isLatestActivation()) return { ok: false, cancelled: true }
        const message = error instanceof Error ? error.message : String(error)
        set((state) => withRuntime(state, sessionName, { errorMessage: message }))
        return { ok: false, error: message }
      }

      if (space?.path && !pathsMatch(space.path, useWorkspaceStore.getState().workspacePath)) {
        try {
          const opened = await openWorkspaceAtPath(space.path, { skipUnsavedGuard: true, shouldOpen: isLatestActivation })
          if (!isLatestActivation()) return { ok: false, cancelled: true }
          if (opened === false) {
            await rollbackFocus()
            return { ok: false, cancelled: true }
          }
        } catch (error) {
          if (!isLatestActivation()) return { ok: false, cancelled: true }
          await rollbackFocus()
          if (!isLatestActivation()) return { ok: false, cancelled: true }
          const message = error instanceof Error ? error.message : String(error)
          set((state) => withRuntime(state, sessionName, { errorMessage: message }))
          return { ok: false, error: message }
        }
      }

      if (!isLatestActivation()) return { ok: false, cancelled: true }
      set((state) => {
        const selectedSpaceBySession = {
          ...state.selectedSpaceBySession,
          [sessionName]: tab.workspaceId
        }
        const runtime = state.runtimesBySession[sessionName]
        const snapshot = runtime?.snapshot
        return {
          selectedSpaceBySession,
          ...projectSelected({ ...state, selectedSpaceBySession }, sessionName),
          selectedSpaceId: tab.workspaceId,
          // A mounted page keeps its layout; this tells it which leaf to select.
          ...(focusPaneId
            ? { paneFocusRequest: { sessionName, paneId: focusPaneId, seq: (state.paneFocusRequest?.seq ?? 0) + 1 } }
            : {}),
          ...buildRuntimePatch(state, sessionName, {
            errorMessage: null,
            ...(snapshot ? { snapshot: withFocusedTab(snapshot, tab) } : {}),
            ...(runtime?.baseSnapshot
              ? { baseSnapshot: withFocusedTab(runtime.baseSnapshot, tab) }
              : {})
          })
        }
      })
      useWorkspaceStore.getState().openHerdrTerminalPage({
        herdrSessionId: sessionName,
        terminalId: tab.terminalId,
        title: tab.label,
        paneId: tab.paneId ?? null,
        herdrTabId: tab.id,
        herdrWorkspaceId: tab.workspaceId
      })
      useWorkspaceStore.getState().rememberSpaceNavigation(sessionName, tab.workspaceId)
      if (tab.paneId) get().markAttentionSeen(sessionName, tab.paneId)
      useUiStore.getState().setMode("ade")
      // The bounded bridge poll will reconcile the authoritative snapshot. Avoid
      // an immediate read-after-focus because protocol-19 may briefly return the
      // previous focused workspace/tab and overwrite this committed selection.
      return { ok: true }
    } finally {
      releaseActivation()
    }
  },

  async activateAgent(agent) {
    const sessionName =
      agent.sessionName ?? get().selectedSessionName ?? HERDR_LIVE_SESSION_ID
    if (!agent.terminalId) {
      return { ok: false, error: "Agent has no terminalId" }
    }

    if (agent.tabId) {
      const runtimeTabs = get().runtimesBySession[sessionName]?.snapshot?.tabs ?? []
      const owningTab = runtimeTabs.find((tab) => tab.id === agent.tabId)
      if (owningTab) {
        // A split tab holds several Agents; select this Agent's pane, not the tab's last one.
        const result = await get().activateTab({
          ...owningTab,
          paneId: agent.paneId ?? owningTab.paneId,
          terminalId: agent.terminalId,
          sessionName
        }, { focusPane: Boolean(agent.paneId) })
        if (result.ok) useAgentMruStore.getState().touch(sessionName, agent.id)
        return result
      }
    }

    const space =
      get().runtimesBySession[sessionName]?.snapshot?.spaces.find(
        (item) => item.id === agent.workspaceId
      ) ?? get().spaces().find((item) => item.id === agent.workspaceId)

    const activationRequest = get().activateSpace({
      sessionName,
      workspaceId: agent.workspaceId,
      path: space?.path ?? null
    })
    const agentGeneration = sessionSelectionGeneration
    const activation = await activationRequest
    if (!activation.ok) return activation
    if (sessionSelectionGeneration !== agentGeneration) return { ok: false, cancelled: true }

    useWorkspaceStore.getState().openHerdrTerminalPage({
      herdrSessionId: sessionName,
      terminalId: agent.terminalId,
      title: agent.title ?? agent.name,
      paneId: agent.paneId ?? null,
      herdrTabId: agent.tabId ?? null,
      herdrWorkspaceId: agent.workspaceId
    })
    useWorkspaceStore.getState().rememberSpaceNavigation(sessionName, agent.workspaceId)
    if (agent.paneId) get().markAttentionSeen(sessionName, agent.paneId)
    useUiStore.getState().setMode("ade")
    useAgentMruStore.getState().touch(sessionName, agent.id)
    return { ok: true }
  },

  async restoreFocusedState(sessionName) {
    if (pendingSelections > 0) return { ok: false, cancelled: true }
    const restoreGeneration = sessionSelectionGeneration
    const isCurrentSelection = () =>
      sessionSelectionGeneration === restoreGeneration &&
      get().selectedSessionName === sessionName
    const readTarget = () => {
      if (!isCurrentSelection()) return { kind: "cancelled" as const }
      const state = get()
      const session = state.sessions.find((item) => sessionScope(item) === sessionName)
      const snapshot = state.runtimesBySession[sessionName]?.snapshot
      if ((session && !session.running) || !snapshot) {
        return {
          kind: "error" as const,
          error: session && !session.running
            ? i18n.t("herdrNav.sessionStopped", { name: sessionName })
            : "Herdr snapshot unavailable"
        }
      }
      const tab = snapshot.tabs.find((item) => item.id === snapshot.focusedTabId)
      const space = snapshot.spaces.find((item) => item.id === snapshot.focusedWorkspaceId)
      if (!tab || !space || !tab.terminalId) {
        return { kind: "error" as const, error: "Herdr focused tab is unavailable" }
      }
      if (!space.path) return { kind: "error" as const, error: i18n.t("rootRequired", { ns: "hosts" }) }
      return {
        kind: "ok" as const,
        snapshot,
        tab,
        space,
        focusKey: `${space.id}:${tab.id}`
      }
    }

    const initial = readTarget()
    if (initial.kind === "cancelled") return { ok: false, cancelled: true }
    if (initial.kind === "error") return { ok: false, error: initial.error }

    const focusKey = initial.focusKey
    let target = initial
    const adoptLatestOrCancel = (): HerdrActivationResult | null => {
      const latest = readTarget()
      if (latest.kind === "cancelled") return { ok: false, cancelled: true }
      if (latest.kind === "error") return { ok: false, error: latest.error }
      if (latest.focusKey !== focusKey) return { ok: false, cancelled: true }
      target = latest
      return null
    }

    const currentWorkspace = useWorkspaceStore.getState().workspacePath
    const needsWorkspaceSwitch = Boolean(
      target.space.path && !pathsMatch(target.space.path, currentWorkspace)
    )
    if (needsWorkspaceSwitch) {
      const proceed = await confirmDiscardingUnsaved({
        title: i18n.t("unsavedDialog.switchWorkspaceTitle", { ns: "menus" }),
        description: i18n.t("unsavedDialog.switchWorkspaceDescription", { ns: "menus" }),
        saveLabel: i18n.t("unsavedDialog.saveAll", { ns: "menus" })
      })
      if (!proceed) return { ok: false, cancelled: true }
      const afterConfirm = adoptLatestOrCancel()
      if (afterConfirm) return afterConfirm
      try {
        const opened = await openWorkspaceAtPath(target.space.path!, {
          skipUnsavedGuard: true,
          shouldOpen: () => adoptLatestOrCancel() === null
        })
        if (opened === false) return { ok: false, cancelled: true }
      } catch (error) {
        const stale = adoptLatestOrCancel()
        if (stale) return stale
        const message = error instanceof Error ? error.message : String(error)
        set((state) => withRuntime(state, sessionName, { errorMessage: message }))
        return { ok: false, error: message }
      }
      const afterOpen = adoptLatestOrCancel()
      if (afterOpen) return afterOpen
    }

    const beforeCommit = adoptLatestOrCancel()
    if (beforeCommit) return beforeCommit
    set((state) => {
      if (
        sessionSelectionGeneration !== restoreGeneration ||
        state.selectedSessionName !== sessionName
      ) {
        return state
      }
      const selectedSpaceBySession = {
        ...state.selectedSpaceBySession,
        [sessionName]: target.space.id
      }
      return {
        selectedSpaceBySession,
        ...projectSelected({ ...state, selectedSpaceBySession }, sessionName),
        selectedSpaceId: target.space.id,
        ...buildRuntimePatch(state, sessionName, { errorMessage: null })
      }
    })
    if (!isCurrentSelection()) return { ok: false, cancelled: true }
    const workspaceBeforeHydration = useWorkspaceStore.getState()
    const activePage = workspaceBeforeHydration.groups[workspaceBeforeHydration.activeGroupIndex]?.tabs.find(
      (page) => page.path === workspaceBeforeHydration.groups[workspaceBeforeHydration.activeGroupIndex]?.activePath
    )
    if (activePage && activePage.kind !== "herdr-terminal" && !needsWorkspaceSwitch) {
      workspaceBeforeHydration.rememberSpaceNavigation(sessionName, target.space.id)
    }
    const defaultSessionName =
      sessionScope(get().sessions.find((session) => session.default && !session.hostId))
    useWorkspaceStore
      .getState()
      .hydrateHerdrPagesFromSnapshot(target.snapshot, defaultSessionName)
    useWorkspaceStore.getState().restoreSpaceNavigation(sessionName, target.space.id)
    if (target.tab.paneId || target.snapshot.focusedPaneId) {
      get().markAttentionSeen(
        sessionName,
        target.tab.paneId ?? target.snapshot.focusedPaneId!
      )
    }
    useUiStore.getState().setMode("ade")
    return { ok: true }
  },

  applySubscriptionEvent(sessionName, event) {
    const current = get()
    if (event.type === "subscribed") {
      get().setEventsHealth(sessionName, true, event.subscriptionId)
      return
    }
    const subscriptionId = current.runtimesBySession[sessionName]?.eventsSubscriptionId ??
      (current.selectedSessionName === sessionName ? current.eventsSubscriptionId : null)
    if (subscriptionId !== event.subscriptionId) return
    if (event.type === "error" || event.type === "disconnected") {
      get().setEventsHealth(sessionName, false, null)
      if (event.type === "error") set((state) => withRuntime(state, sessionName, { errorMessage: event.message }))
      return
    }
    get().setEventsHealth(sessionName, true, event.subscriptionId)
    if (event.type === "pane_exited") {
      const key = herdrAttentionKey(sessionName, event.paneId)
      set((state) => {
        const attentionByKey = new Map(state.attentionByKey)
        attentionByKey.delete(key)
        return { attentionByKey }
      })
      return
    }
    if (event.type === "worktree_changed") {
      // Dirty signal only — authoritative recovery is list + snapshot.
      void get().refreshWorktreeInventory(sessionName)
      return
    }
    if (event.type === "topology_changed") {
      return
    }
    if (event.type !== "agent_status_changed" || !event.paneId) return

    const kind = attentionKindForStatus(event.agentStatus)
    const key = herdrAttentionKey(sessionName, event.paneId)
    set((state) => {
      const attentionByKey = new Map(state.attentionByKey)
      if (!kind) {
        // Idle/working clear temporary unknown/done/blocked attention for this pane.
        attentionByKey.delete(key)
        return { attentionByKey }
      }
      const previous = attentionByKey.get(key)
      attentionByKey.set(key, {
        key,
        sessionName,
        paneId: event.paneId,
        workspaceId: event.workspaceId,
        agentStatus: event.agentStatus as HerdrAgentStatus,
        kind,
        title: event.title ?? previous?.title ?? null,
        displayAgent: event.displayAgent ?? previous?.displayAgent ?? null,
        // Reading never marks seen; only focus/activation does.
        // blocked always remains attention-visible.
        seen: kind === "done" ? (previous?.seen ?? false) : false,
        updatedAt: Date.now()
      })
      return { attentionByKey }
    })
  },

  setEventsHealth(sessionName, healthy, subscriptionId = null) {
    set((state) => {
      const patch = {
        ...buildRuntimePatch(state, sessionName, { eventsHealthy: healthy, eventsSubscriptionId: subscriptionId }),
        ...(state.selectedSessionName === sessionName ? { eventsHealthy: healthy, eventsSubscriptionId: subscriptionId } : {})
      }
      return sameFields(state, patch) ? state : patch
    })
  },

  markAttentionSeen(sessionName, paneId) {
    if (!sessionName || !paneId) return
    const key = herdrAttentionKey(sessionName, paneId)
    set((state) => {
      const current = state.attentionByKey.get(key)
      if (!current) return state
      const attentionByKey = new Map(state.attentionByKey)
      if (current.kind === "done") {
        attentionByKey.set(key, { ...current, seen: true })
      } else if (current.kind === "blocked" || current.kind === "unknown") {
        // Keep blocked/unknown until status changes; focus does not hide them.
        attentionByKey.set(key, { ...current, seen: true })
      } else {
        attentionByKey.delete(key)
      }
      return { attentionByKey }
    })
  },

  attentionItems(sessionName) {
    const selected = sessionName
    const items = Array.from(get().attentionByKey.values()).filter((item) => {
      if (selected && item.sessionName !== selected) return false
      if (item.kind === "done" && item.seen) return false
      return true
    })
    items.sort((a, b) => b.updatedAt - a.updatedAt)
    return items
  },
}))
