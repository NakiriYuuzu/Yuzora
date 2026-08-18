import { create } from "zustand"

import {
  herdrCapabilities,
  herdrSessions,
  herdrSnapshot,
  herdrTabFocus,
  herdrTabRename,
  herdrTerminalCreate,
  herdrTerminalRelease,
  herdrWorkspaceCreate,
  herdrWorkspaceFocus,
  herdrWorktreeList
} from "@/lib/herdrIpc"
import { HERDR_LIVE_SESSION_ID, normalizeHerdrSnapshot } from "@/lib/herdrNormalize"
import {
  herdrRuntimeKey,
  herdrSessionKey,
  loadEnabledHerdrRuntimeTargets,
  loadHerdrRuntimeTarget,
  normalizeHerdrRuntimeTarget,
  persistEnabledHerdrRuntimeTargets,
  persistHerdrRuntimeTarget,
  sameHerdrRuntimeTarget
} from "@/lib/herdrRuntime"
import type {
  HerdrAgentInfo,
  HerdrAgentStatus,
  HerdrAttentionItem,
  HerdrAttentionKind,
  HerdrCapabilities,
  HerdrConnectionState,
  HerdrNamedSession,
  HerdrRuntimeTarget,
  HerdrSessionRuntime,
  HerdrSnapshot,
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
  runtimeTarget?: HerdrRuntimeTarget | null
  terminalId: string
  target: string
  paneId?: string | null
  mode: HerdrTerminalMode
  role: HerdrTerminalRole
  takeover: boolean
}

export type HerdrCreateTerminalResult = {
  herdrSessionId: string
  runtimeTarget: HerdrRuntimeTarget
  workspaceId: string
  terminalId: string
  paneId?: string | null
  tabId?: string | null
  title?: string | null
}

export type HerdrActivationResult =
  | { ok: true }
  | { ok: false; cancelled?: boolean; error?: string }

/** Shared attention namespace: Runtime Environment + named session + pane. */
export function herdrAttentionKey(
  sessionName: string,
  paneId: string,
  runtimeTarget?: HerdrRuntimeTarget | null
): string {
  return `${herdrSessionKey(runtimeTarget, sessionName)}::${paneId}`
}

/** Canonical store key; native legacy aliases are read compatibility only. */
export function herdrStoreRuntimeKey(
  sessionName: string,
  runtimeTarget?: HerdrRuntimeTarget | null
): string {
  return herdrSessionKey(runtimeTarget, sessionName)
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
  sessions: HerdrNamedSession[]
  selectedSessionName: string | null
  /** Native until a later provider-selection phase exposes remote targets. */
  selectedRuntimeTarget: HerdrRuntimeTarget
  /** User-enabled targets; Native is always retained. Selection may start only the selected WSL target. */
  enabledRuntimeTargets: HerdrRuntimeTarget[]
  /** Canonical entries use herdrStoreRuntimeKey; native name aliases remain for legacy readers. */
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
  attachments: Map<string, HerdrAttachmentRecord>
  /** Attention items keyed by sessionName::paneId. */
  attentionByKey: Map<string, HerdrAttentionItem>
  /** Live event subscription health for the selected session. */
  eventsHealthy: boolean
  eventsSubscriptionId: string | null
  refreshSessions: (runtimeTarget?: HerdrRuntimeTarget | null) => Promise<void>
  /** Explicit user action: selecting WSL is the only point that may start it. */
  selectRuntimeTarget: (runtimeTarget: HerdrRuntimeTarget) => Promise<void>
  selectSession: (sessionName: string, runtimeTarget?: HerdrRuntimeTarget | null) => Promise<void>
  bootstrap: (
    sessionName?: string | null,
    runtimeTarget?: HerdrRuntimeTarget | null
  ) => Promise<void>
  refreshSnapshot: (
    sessionName?: string | null,
    runtimeTarget?: HerdrRuntimeTarget | null
  ) => Promise<boolean>
  applySnapshot: (
    sessionName: string,
    snapshot: HerdrSnapshot,
    runtimeTarget?: HerdrRuntimeTarget | null
  ) => void
  setSelectedSpaceId: (
    spaceId: string | null,
    runtimeTarget?: HerdrRuntimeTarget | null,
    sessionName?: string | null
  ) => void
  clearError: () => void
  bumpTopologyRevision: () => void
  registerAttachment: (attachmentKey: string, record: HerdrAttachmentRecord) => void
  updateAttachmentPaneId: (attachmentKey: string, paneId: string | null | undefined) => void
  updateAttachmentMode: (
    attachmentKey: string,
    mode: HerdrTerminalMode,
    role: HerdrTerminalRole
  ) => void
  releaseAttachment: (attachmentKey: string) => Promise<void>
  releaseAttachmentsForPage: (pagePath: string) => Promise<void>
  releaseAllAttachments: () => Promise<void>
  createTerminalInSelectedSpace: () => Promise<HerdrCreateTerminalResult | null>
  createSpaceFromFolder: (
    cwd: string,
    label?: string | null
  ) => Promise<HerdrActivationResult & { space?: HerdrSpaceInfo | null }>
  canCreateTerminal: () => boolean
  canMutateSelectedSession: () => boolean
  canFocusSelectedTab: () => boolean
  canMoveSelectedTab: () => boolean
  createTerminalBlockedReason: () => string | null
  mutationBlockedReason: () => string | null
  spaces: () => HerdrSpaceInfo[]
  agents: () => HerdrAgentInfo[]
  agentsInSpace: (spaceId: string) => HerdrAgentInfo[]
  tabs: () => HerdrTabInfo[]
  tabsInSpace: (spaceId: string) => HerdrTabInfo[]
  selectedSession: () => HerdrNamedSession | null
  activateSpace: (args: {
    sessionName: string
    runtimeTarget?: HerdrRuntimeTarget | null
    workspaceId: string
    path?: string | null
  }) => Promise<HerdrActivationResult>
  activateTab: (tab: HerdrTabInfo) => Promise<HerdrActivationResult>
  activateAgent: (agent: HerdrAgentInfo) => Promise<HerdrActivationResult>
  /** Restore focused-Space Herdr pages from the snapshot without mutating Herdr. */
  restoreFocusedState: (
    sessionName: string,
    runtimeTarget?: HerdrRuntimeTarget | null
  ) => Promise<HerdrActivationResult>
  applySubscriptionEvent: (
    sessionName: string,
    event: HerdrSubscriptionEvent,
    runtimeTarget?: HerdrRuntimeTarget | null
  ) => void
  setEventsHealth: (
    sessionName: string,
    healthy: boolean,
    subscriptionId?: string | null,
    runtimeTarget?: HerdrRuntimeTarget | null
  ) => void
  /** Reconcile read-only worktree.list inventory for one runtime-scoped named session. */
  refreshWorktreeInventory: (
    sessionName?: string | null,
    runtimeTarget?: HerdrRuntimeTarget | null
  ) => Promise<void>
  markAttentionSeen: (
    sessionName: string,
    paneId: string,
    runtimeTarget?: HerdrRuntimeTarget | null
  ) => void
  attentionItems: (
    sessionName?: string | null,
    runtimeTarget?: HerdrRuntimeTarget | null
  ) => HerdrAttentionItem[]
  canInspectAgent: (
    sessionName?: string | null,
    runtimeTarget?: HerdrRuntimeTarget | null
  ) => boolean
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

function runtimeOf(
  state: Pick<HerdrState, "runtimesBySession" | "selectedSessionName" | "selectedRuntimeTarget">,
  sessionName?: string | null,
  runtimeTarget?: HerdrRuntimeTarget | null
): HerdrSessionRuntime {
  const name = sessionName ?? state.selectedSessionName
  if (!name) return emptyRuntime()
  const target = normalizeHerdrRuntimeTarget(runtimeTarget ?? state.selectedRuntimeTarget)
  const canonical = herdrStoreRuntimeKey(name, target)
  // Native aliases are only a migration bridge for persisted/legacy selectors.
  // Prefer the legacy mirror for Native so older state hydration that updates
  // only that key remains observable; WSL never falls back across runtimes.
  return (target.kind === "native" ? state.runtimesBySession[name] : undefined) ??
    state.runtimesBySession[canonical] ??
    emptyRuntime()
}

function withRuntime(
  state: HerdrState,
  sessionName: string,
  patch: Partial<HerdrSessionRuntime>,
  runtimeTarget?: HerdrRuntimeTarget | null
): Partial<HerdrState> {
  const target = normalizeHerdrRuntimeTarget(runtimeTarget ?? state.selectedRuntimeTarget)
  const canonical = herdrStoreRuntimeKey(sessionName, target)
  const previous = runtimeOf(state, sessionName, target)
  const nextRuntime: HerdrSessionRuntime = { ...previous, ...patch, runtimeTarget: target }
  const runtimesBySession: Record<string, HerdrSessionRuntime> = {
    ...state.runtimesBySession,
    [canonical]: nextRuntime
  }
  // Do not break persisted/native-only consumers while canonicalizing new keys.
  if (target.kind === "native") runtimesBySession[sessionName] = nextRuntime
  if (
    state.selectedSessionName === sessionName &&
    sameHerdrRuntimeTarget(state.selectedRuntimeTarget, target)
  ) {
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

function projectSelected(
  state: HerdrState,
  selectedSessionName: string | null,
  selectedRuntimeTarget: HerdrRuntimeTarget = state.selectedRuntimeTarget
): Partial<HerdrState> {
  const runtime = selectedSessionName
    ? runtimeOf(state, selectedSessionName, selectedRuntimeTarget)
    : emptyRuntime()
  const sessionKey = selectedSessionName
    ? herdrStoreRuntimeKey(selectedSessionName, selectedRuntimeTarget)
    : null
  return {
    selectedSessionName,
    selectedRuntimeTarget,
    connectionState: runtime.connectionState,
    capabilities: runtime.capabilities,
    snapshot: runtime.snapshot,
    errorMessage: runtime.errorMessage,
    selectedSpaceId: sessionKey
      ? (state.selectedSpaceBySession[sessionKey] ??
          (selectedRuntimeTarget.kind === "native"
            ? state.selectedSpaceBySession[selectedSessionName!]
            : null) ??
          null)
      : null
  }
}

function selectedSpaceFor(
  state: Pick<HerdrState, "selectedSpaceBySession" | "selectedRuntimeTarget">,
  sessionName: string,
  runtimeTarget?: HerdrRuntimeTarget | null
): string | null {
  const target = normalizeHerdrRuntimeTarget(runtimeTarget ?? state.selectedRuntimeTarget)
  return state.selectedSpaceBySession[herdrStoreRuntimeKey(sessionName, target)] ??
    (target.kind === "native" ? state.selectedSpaceBySession[sessionName] : null) ??
    null
}

function withSelectedSpace(
  state: HerdrState,
  sessionName: string,
  spaceId: string | null,
  runtimeTarget?: HerdrRuntimeTarget | null
): Record<string, string | null> {
  const target = normalizeHerdrRuntimeTarget(runtimeTarget ?? state.selectedRuntimeTarget)
  const selectedSpaceBySession = {
    ...state.selectedSpaceBySession,
    [herdrStoreRuntimeKey(sessionName, target)]: spaceId
  }
  if (target.kind === "native") selectedSpaceBySession[sessionName] = spaceId
  return selectedSpaceBySession
}

/** A named session is scoped to its Runtime Environment, not globally unique. */
function sessionFor(
  state: Pick<HerdrState, "sessions">,
  sessionName: string,
  runtimeTarget?: HerdrRuntimeTarget | null
): HerdrNamedSession | null {
  const target = normalizeHerdrRuntimeTarget(runtimeTarget)
  return state.sessions.find(
    (session) =>
      session.name === sessionName &&
      sameHerdrRuntimeTarget(session.runtimeTarget, target)
  ) ?? null
}

function runtimeKeyFor(
  sessionName: string,
  runtimeTarget?: HerdrRuntimeTarget | null
): string {
  return herdrStoreRuntimeKey(sessionName, normalizeHerdrRuntimeTarget(runtimeTarget))
}

function runtimeTargetPayload(target: HerdrRuntimeTarget): {
  runtimeTarget?: HerdrRuntimeTarget
} {
  return target.kind === "native" ? {} : { runtimeTarget: target }
}

function releaseAttachmentConnector(record: HerdrAttachmentRecord): Promise<void> {
  return record.runtimeTarget === undefined || record.runtimeTarget === null
    ? herdrTerminalRelease(record.sessionId)
    : herdrTerminalRelease(record.sessionId, record.runtimeTarget)
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
      focused: space.id === tab.workspaceId
    })),
    tabs: snapshot.tabs.map((candidate) => ({
      ...candidate,
      active: candidate.id === tab.id,
      focused: candidate.id === tab.id
    })),
    focusedWorkspaceId: tab.workspaceId,
    focusedTabId: tab.id,
    focusedPaneId: tab.paneId ?? snapshot.focusedPaneId ?? null
  }
}

const initialRuntimeTarget = loadHerdrRuntimeTarget()
const initialEnabledRuntimeTargets = loadEnabledHerdrRuntimeTargets()
const herdrInitialEnabledRuntimeTargets = initialEnabledRuntimeTargets.some((target) =>
  sameHerdrRuntimeTarget(target, initialRuntimeTarget)
)
  ? initialEnabledRuntimeTargets
  : [...initialEnabledRuntimeTargets, initialRuntimeTarget]

export const herdrInitialState = {
  sessions: [] as HerdrNamedSession[],
  selectedSessionName: null as string | null,
  selectedRuntimeTarget: initialRuntimeTarget,
  enabledRuntimeTargets: herdrInitialEnabledRuntimeTargets,
  runtimesBySession: {} as Record<string, HerdrSessionRuntime>,
  selectedSpaceBySession: {} as Record<string, string | null>,
  connectionState: "idle" as HerdrConnectionState,
  capabilities: null as HerdrCapabilities | null,
  snapshot: null as HerdrSnapshot | null,
  errorMessage: null as string | null,
  selectedSpaceId: null as string | null,
  topologyRevision: 0,
  attachments: new Map<string, HerdrAttachmentRecord>(),
  attentionByKey: new Map<string, HerdrAttentionItem>(),
  eventsHealthy: false,
  eventsSubscriptionId: null as string | null
}

/** Module-level in-flight guards — not part of reactive state. */
const sessionsInFlight = new Map<string, Promise<void>>()
const bootstrapInFlight = new Map<string, Promise<void>>()
const refreshInFlight = new Map<string, Promise<boolean>>()
const pendingRefresh = new Set<string>()
const MAX_REFRESH_RETRIES = 2
const worktreeInventoryInFlight = new Map<string, Promise<void>>()
const worktreeInventoryRequestedGeneration = new Map<string, number>()
const snapshotGeneration = new Map<string, number>()
const tabActivationGeneration = new Map<string, number>()
const tabActivationTail = new Map<string, Promise<void>>()
let sessionSelectionGeneration = 0

async function acquireTabActivation(runtimeSessionKey: string): Promise<() => void> {
  const previous = tabActivationTail.get(runtimeSessionKey) ?? Promise.resolve()
  let releaseGate!: () => void
  const gate = new Promise<void>((resolve) => {
    releaseGate = resolve
  })
  const tail = previous.catch(() => undefined).then(() => gate)
  tabActivationTail.set(runtimeSessionKey, tail)
  await previous.catch(() => undefined)

  let released = false
  return () => {
    if (released) return
    released = true
    releaseGate()
    void tail.then(() => {
      if (tabActivationTail.get(runtimeSessionKey) === tail) {
        tabActivationTail.delete(runtimeSessionKey)
      }
    })
  }
}

export const useHerdrStore = create<HerdrState>((set, get) => ({
  ...herdrInitialState,

  selectedSession() {
    const state = get()
    const name = state.selectedSessionName
    if (!name) return null
    return sessionFor(state, name, state.selectedRuntimeTarget)
  },

  async refreshSessions(runtimeTarget) {
    const target = normalizeHerdrRuntimeTarget(runtimeTarget ?? get().selectedRuntimeTarget)
    const existing = sessionsInFlight.get(herdrRuntimeKey(target))
    if (existing) return existing
    const task = (async () => {
      try {
        const discovered = await herdrSessions(target)
        const sessions = discovered.map((session) => ({ ...session, runtimeTarget: target }))
        set((state) => {
          const retained = state.sessions.filter(
            (session) => !sameHerdrRuntimeTarget(session.runtimeTarget, target)
          )
          const allSessions = [...retained, ...sessions]
          const isSelectedTarget = sameHerdrRuntimeTarget(state.selectedRuntimeTarget, target)
          let selectedSessionName = state.selectedSessionName
          if (isSelectedTarget && (
            !selectedSessionName ||
            !sessions.some((session) => session.name === selectedSessionName)
          )) {
            selectedSessionName =
              sessions.find((session) => session.default)?.name ??
              sessions[0]?.name ??
              null
          }
          if (!isSelectedTarget) return { sessions: allSessions }
          return {
            sessions: allSessions,
            ...projectSelected({ ...state, sessions: allSessions }, selectedSessionName, target)
          }
        })
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error)
        set((state) => {
          if (!sameHerdrRuntimeTarget(state.selectedRuntimeTarget, target)) return state
          const selectedSessionName = state.selectedSessionName
          if (!selectedSessionName) {
            return { errorMessage: message, connectionState: "error" }
          }
          const selectedRuntime = runtimeOf(state, selectedSessionName, target)
          const hasTargetInventory = state.sessions.some((session) =>
            sameHerdrRuntimeTarget(session.runtimeTarget, target)
          )
          return withRuntime(state, selectedSessionName, {
            errorMessage: message,
            connectionState:
              selectedRuntime.snapshot || hasTargetInventory
                ? selectedRuntime.connectionState
                : "error"
          }, target)
        })
      } finally {
        sessionsInFlight.delete(herdrRuntimeKey(target))
      }
    })()
    sessionsInFlight.set(herdrRuntimeKey(target), task)
    return task
  },

  async selectRuntimeTarget(runtimeTarget) {
    const target = normalizeHerdrRuntimeTarget(runtimeTarget)
    const current = get()
    if (sameHerdrRuntimeTarget(current.selectedRuntimeTarget, target)) {
      if (!current.enabledRuntimeTargets.some((item) => sameHerdrRuntimeTarget(item, target))) {
        const enabledRuntimeTargets = [...current.enabledRuntimeTargets, target]
        persistEnabledHerdrRuntimeTargets(enabledRuntimeTargets)
        set({ enabledRuntimeTargets })
      }
      await get().refreshSessions(target)
      return
    }
    sessionSelectionGeneration += 1
    persistHerdrRuntimeTarget(target)
    set((state) => {
      const enabledRuntimeTargets = state.enabledRuntimeTargets.some((item) =>
        sameHerdrRuntimeTarget(item, target)
      )
        ? state.enabledRuntimeTargets
        : [...state.enabledRuntimeTargets, target]
      persistEnabledHerdrRuntimeTargets(enabledRuntimeTargets)
      return {
        ...projectSelected(state, null, target),
        enabledRuntimeTargets,
        selectedSpaceId: null
      }
    })
    await get().refreshSessions(target)
    const selected = get().selectedSession()
    if (selected?.running) await get().bootstrap(selected.name, target)
  },

  async selectSession(sessionName, runtimeTarget) {
    const target = normalizeHerdrRuntimeTarget(runtimeTarget ?? get().selectedRuntimeTarget)
    const session = sessionFor(get(), sessionName, target)
    if (!session) return
    sessionSelectionGeneration += 1
    set((state) => ({
      ...projectSelected(state, sessionName, target),
      selectedSpaceId: selectedSpaceFor(state, sessionName, target)
    }))
    // Switching sessions must not close pages / TerminalDrawer — only selection changes.
    if (session.running) {
      await get().bootstrap(sessionName, target)
    } else {
      set((state) =>
        withRuntime(state, sessionName, {
          connectionState: "stopped",
          errorMessage:
            i18n.t("herdrNav.sessionStopped", {
              name: sessionName,
              defaultValue: `Session "${sessionName}" is not running. Start it with \`herdr session attach ${sessionName}\`.`
            }) ?? null
        }, target)
      )
    }
  },

  async bootstrap(sessionName, runtimeTarget) {
    const target = normalizeHerdrRuntimeTarget(runtimeTarget ?? get().selectedRuntimeTarget)
    const resolved =
      sessionName ??
      get().selectedSessionName ??
      get().sessions.find((s) => s.default && sameHerdrRuntimeTarget(s.runtimeTarget, target))?.name ??
      HERDR_LIVE_SESSION_ID
    const key = runtimeKeyFor(resolved, target)
    const existing = bootstrapInFlight.get(key)
    if (existing) return existing

    const task = (async () => {
      set((state) => withRuntime(state, resolved, {
        connectionState: "connecting",
        errorMessage: null
      }, target))
      try {
        const named = sessionFor(get(), resolved, target)
        if (named && !named.running) {
          set((state) =>
            withRuntime(state, resolved, {
              connectionState: "stopped",
              errorMessage: i18n.t("herdrNav.sessionStopped", { name: resolved })
            }, target)
          )
          return
        }

        const capabilities = target.kind === "native"
          ? await herdrCapabilities(resolved)
          : await herdrCapabilities(resolved, target)
        if (isStoppedReason(capabilities.api.reason) || !capabilities.server.running) {
          set((state) =>
            withRuntime(state, resolved, {
              capabilities,
              connectionState: "stopped",
              errorMessage:
                capabilities.api.reason ??
                i18n.t("herdrNav.sessionStopped", { name: resolved })
            }, target)
          )
          return
        }

        if (!capabilities.binaryPath) {
          set((state) =>
            withRuntime(state, resolved, {
              capabilities,
              connectionState: "unsupported",
              errorMessage: unsupportedReason(capabilities) ?? "Herdr binary not found"
            }, target)
          )
          return
        }

        if (!capabilities.api.snapshot) {
          set((state) =>
            withRuntime(state, resolved, {
              capabilities,
              connectionState: "unsupported",
              errorMessage: unsupportedReason(capabilities) ?? capabilities.api.reason
            }, target)
          )
          return
        }

        set((state) => withRuntime(state, resolved, { capabilities }, target))
        const raw = target.kind === "native"
          ? await herdrSnapshot(resolved)
          : await herdrSnapshot(resolved, target)
        const snapshot = normalizeHerdrSnapshot(raw, resolved, target)
        get().applySnapshot(resolved, snapshot, target)
        set((state) =>
          withRuntime(state, resolved, {
            connectionState: "ready",
            errorMessage: null
          }, target)
        )
        // Authoritative inventory reconcile after snapshot recovery.
        await get().refreshWorktreeInventory(resolved, target)
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error)
        if (isStoppedReason(message)) {
          set((state) =>
            withRuntime(state, resolved, {
              connectionState: "stopped",
              errorMessage: message
            }, target)
          )
          return
        }
        const hadSnapshot = runtimeOf(get(), resolved, target).snapshot !== null
        set((state) =>
          withRuntime(state, resolved, {
            connectionState: hadSnapshot ? "ready" : "error",
            errorMessage: message
          }, target)
        )
      } finally {
        bootstrapInFlight.delete(key)
      }
    })()
    bootstrapInFlight.set(key, task)
    return task
  },

  async refreshSnapshot(sessionName, runtimeTarget) {
    const target = normalizeHerdrRuntimeTarget(runtimeTarget ?? get().selectedRuntimeTarget)
    const resolved = sessionName ?? get().selectedSessionName
    if (!resolved) return false
    const key = runtimeKeyFor(resolved, target)
    const named = sessionFor(get(), resolved, target)
    if (named && !named.running) {
      set((state) =>
        withRuntime(state, resolved, {
          connectionState: "stopped",
          errorMessage: i18n.t("herdrNav.sessionStopped", { name: resolved })
        }, target)
      )
      return false
    }
    const existing = refreshInFlight.get(key)
    if (existing) {
      pendingRefresh.add(key)
      return existing
    }
    const task = (async () => {
      let consecutiveFailures = 0
      try {
        while (true) {
          let passSucceeded = false
          // Requests that arrive during this pass are authoritative trailing
          // refreshes. Consume only requests that predate the pass here.
          pendingRefresh.delete(key)
          try {
            const raw = target.kind === "native"
              ? await herdrSnapshot(resolved)
              : await herdrSnapshot(resolved, target)
            const snapshot = normalizeHerdrSnapshot(raw, resolved, target)
            get().applySnapshot(resolved, snapshot, target)
            set((state) =>
              withRuntime(state, resolved, {
                connectionState: "ready",
                errorMessage: null
              }, target)
            )
            await get().refreshWorktreeInventory(resolved, target)
            consecutiveFailures = 0
            passSucceeded = true
          } catch (error) {
            const message = error instanceof Error ? error.message : String(error)
            if (isStoppedReason(message)) {
              pendingRefresh.delete(key)
              set((state) =>
                withRuntime(state, resolved, {
                  connectionState: "stopped",
                  errorMessage: message
                }, target)
              )
              return false
            }
            consecutiveFailures += 1
            if (consecutiveFailures <= MAX_REFRESH_RETRIES) {
              pendingRefresh.add(key)
            }
            const hadSnapshot = runtimeOf(get(), resolved, target).snapshot !== null
            if (hadSnapshot) {
              set((state) =>
                withRuntime(state, resolved, {
                  errorMessage: message,
                  connectionState: "ready"
                }, target)
              )
            } else {
              const current = runtimeOf(get(), resolved, target).connectionState
              set((state) =>
                withRuntime(state, resolved, {
                  errorMessage: message,
                  connectionState:
                    current === "connecting" || current === "idle" ? "error" : current
                }, target)
              )
            }
          }
          if (!pendingRefresh.delete(key)) return passSucceeded
        }
      } finally {
        refreshInFlight.delete(key)
        pendingRefresh.delete(key)
      }
    })()
    refreshInFlight.set(key, task)
    return task
  },

  async refreshWorktreeInventory(sessionName, runtimeTarget) {
    const target = normalizeHerdrRuntimeTarget(runtimeTarget ?? get().selectedRuntimeTarget)
    const resolved = sessionName ?? get().selectedSessionName
    if (!resolved) return
    const key = runtimeKeyFor(resolved, target)
    const named = sessionFor(get(), resolved, target)
    if (named && !named.running) return
    const runtime = runtimeOf(get(), resolved, target)
    if (!runtime.capabilities?.api.worktreeList) return

    const requestedGeneration =
      (worktreeInventoryRequestedGeneration.get(key) ?? 0) + 1
    worktreeInventoryRequestedGeneration.set(key, requestedGeneration)
    const existing = worktreeInventoryInFlight.get(key)
    if (existing) return existing

    const task = (async () => {
      try {
        while (true) {
          const completedGeneration =
            worktreeInventoryRequestedGeneration.get(key) ?? requestedGeneration
          const snapshotAtStart = snapshotGeneration.get(key) ?? 0
          const current = runtimeOf(get(), resolved, target)
          const baseSnapshot = current.baseSnapshot ?? current.snapshot
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
              try {
                lists.push(
                  await herdrWorktreeList({
                    ...runtimeTargetPayload(target),
                    sessionName: resolved,
                    workspaceId: space.id
                  })
                )
                listed = true
              } catch {
                if (attempt === 1) failedScopes.push(space.repoKey ?? space.id)
              }
            }
          }

          // A newer snapshot invalidates the response. Queue exactly one pass
          // against the new authoritative topology instead of overlaying stale data.
          if ((snapshotGeneration.get(key) ?? 0) !== snapshotAtStart) {
            worktreeInventoryRequestedGeneration.set(
              key,
              Math.max(
                worktreeInventoryRequestedGeneration.get(key) ?? 0,
                completedGeneration + 1
              )
            )
            continue
          }

          const inventory = buildWorktreeInventory(resolved, lists, failedScopes)
          set((state) => {
            const latest = runtimeOf(state, resolved, target)
            const projectionBase = latest.baseSnapshot ?? latest.snapshot
            const projectedSnapshot = projectionBase
              ? withInventoryOnSnapshot(projectionBase, inventory)
              : null
            return withRuntime(state, resolved, {
              worktreeInventory: inventory,
              snapshot: projectedSnapshot
            }, target)
          })
          if (
            (worktreeInventoryRequestedGeneration.get(key) ?? 0) <=
            completedGeneration
          ) {
            break
          }
        }
      } finally {
        worktreeInventoryInFlight.delete(key)
      }
    })()
    worktreeInventoryInFlight.set(key, task)
    return task
  },

  applySnapshot(sessionName, snapshot, runtimeTarget) {
    const target = normalizeHerdrRuntimeTarget(runtimeTarget ?? snapshot.runtimeTarget)
    const scopedSessionKey = herdrStoreRuntimeKey(sessionName, target)
    const nextGeneration = (snapshotGeneration.get(scopedSessionKey) ?? 0) + 1
    snapshotGeneration.set(scopedSessionKey, nextGeneration)
    const previousRuntime = runtimeOf(get(), sessionName, target)
    const inventory = previousRuntime?.worktreeInventory ?? null
    const canReuseInventory =
      worktreeProjectionScope(previousRuntime?.baseSnapshot) ===
      worktreeProjectionScope(snapshot)
    const reusableInventory = canReuseInventory ? inventory : null
    const mergedSnapshot = withInventoryOnSnapshot(snapshot, reusableInventory)
    set((state) => {
      const selectedStillExists = mergedSnapshot.spaces.some(
        (s) => s.id === selectedSpaceFor(state, sessionName, target)
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
        (selectedStillExists ? selectedSpaceFor(state, sessionName, target) : fallback)
      const selectedSpaceBySession = withSelectedSpace(state, sessionName, nextSpace, target)
      const runtimePatch = withRuntime(state, sessionName, {
        baseSnapshot: snapshot,
        worktreeInventory: reusableInventory,
        snapshot: mergedSnapshot
      }, target)
      // Snapshot reconciliation is the recovery truth after event disconnects;
      // protocol 19 exposes no event cursor to replay missed transitions.
      const attentionByKey = new Map(state.attentionByKey)
      const livePaneKeys = new Set<string>()
      for (const agent of mergedSnapshot.agents) {
        if (!agent.paneId) continue
        const key = herdrAttentionKey(sessionName, agent.paneId, target)
        livePaneKeys.add(key)
        const kind = attentionKindForStatus(agent.status)
        if (!kind) {
          attentionByKey.delete(key)
          continue
        }
        const previous = attentionByKey.get(key)
        const unchanged = previous?.kind === kind && previous.agentStatus === agent.status
        attentionByKey.set(key, {
          key,
          runtimeTarget: target,
          sessionName,
          paneId: agent.paneId,
          workspaceId: agent.workspaceId,
          agentStatus: agent.status,
          kind,
          title: agent.title ?? previous?.title ?? null,
          displayAgent: agent.displayAgent ?? previous?.displayAgent ?? null,
          seen: unchanged ? previous?.seen ?? false : false,
          updatedAt: unchanged ? previous?.updatedAt ?? Date.now() : Date.now()
        })
      }
      for (const [key, item] of attentionByKey) {
        if (
          item.sessionName === sessionName &&
          sameHerdrRuntimeTarget(item.runtimeTarget, target) &&
          !livePaneKeys.has(key)
        ) {
          attentionByKey.delete(key)
        }
      }
      return {
        ...runtimePatch,
        selectedSpaceBySession,
        attentionByKey,
        selectedSpaceId:
          state.selectedSessionName === sessionName &&
          sameHerdrRuntimeTarget(state.selectedRuntimeTarget, target)
            ? nextSpace
            : state.selectedSpaceId
      }
    })
    const defaultSessionName = get().sessions.find(
      (session) => session.default && sameHerdrRuntimeTarget(session.runtimeTarget, target)
    )?.name ?? null
    useWorkspaceStore
      .getState()
      .reconcileHerdrPagesFromSnapshot(mergedSnapshot, defaultSessionName)
  },

  setSelectedSpaceId(spaceId, runtimeTarget, sessionName) {
    const stateBefore = get()
    const target = normalizeHerdrRuntimeTarget(runtimeTarget ?? stateBefore.selectedRuntimeTarget)
    const resolvedSessionName = sessionName ?? stateBefore.selectedSessionName
    if (!resolvedSessionName) {
      if (sameHerdrRuntimeTarget(stateBefore.selectedRuntimeTarget, target)) {
        set({ selectedSpaceId: spaceId })
      }
      return
    }
    set((state) => {
      const selectedSpaceBySession = withSelectedSpace(
        state,
        resolvedSessionName,
        spaceId,
        target
      )
      return {
        selectedSpaceBySession,
        ...(state.selectedSessionName === resolvedSessionName &&
        sameHerdrRuntimeTarget(state.selectedRuntimeTarget, target)
          ? { selectedSpaceId: spaceId }
          : {})
      }
    })
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
        takeover: mode === "control" ? current.takeover || true : false
      })
      return { attachments }
    })
  },

  async releaseAttachment(attachmentKey) {
    const record = get().attachments.get(attachmentKey)
    if (!record) return
    set((state) => {
      const attachments = new Map(state.attachments)
      attachments.delete(attachmentKey)
      return { attachments }
    })
    // Never pane.close — release connector only.
    await releaseAttachmentConnector(record).catch(() => undefined)
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
      entries.map(([, record]) => releaseAttachmentConnector(record).catch(() => undefined))
    )
  },

  async releaseAllAttachments() {
    const entries = Array.from(get().attachments.entries())
    set({ attachments: new Map() })
    await Promise.all(
      entries.map(([, record]) => releaseAttachmentConnector(record).catch(() => undefined))
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

  async createTerminalInSelectedSpace() {
    const { selectedSpaceId, selectedSessionName, selectedRuntimeTarget } = get()
    if (!selectedSpaceId || !selectedSessionName || !get().canCreateTerminal()) return null
    try {
      const selectedSpace = get().spaces().find((space) => space.id === selectedSpaceId)
      const folderName = selectedSpace?.path
        ? workspacePathBasename(selectedSpace.path)
        : null
      const title = folderName?.trim() || selectedSpace?.label?.trim() || null
      const created = await herdrTerminalCreate({
        ...runtimeTargetPayload(selectedRuntimeTarget),
        sessionName: selectedSessionName,
        workspaceId: selectedSpaceId,
        title
      })
      set((state) => withRuntime(state, selectedSessionName, { errorMessage: null }, selectedRuntimeTarget))
      void get().refreshSnapshot(selectedSessionName, selectedRuntimeTarget)
      return {
        herdrSessionId: selectedSessionName,
        runtimeTarget: selectedRuntimeTarget,
        workspaceId: selectedSpaceId,
        terminalId: created.terminalId,
        paneId: created.paneId,
        tabId: created.tabId,
        title: created.title?.trim() || title
      }
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error)
      set((state) => withRuntime(state, selectedSessionName, { errorMessage: message }, selectedRuntimeTarget))
      return null
    }
  },

  async createSpaceFromFolder(cwd, label) {
    const stateBefore = get()
    const sessionName = stateBefore.selectedSessionName
    const runtimeTarget = stateBefore.selectedRuntimeTarget
    if (!sessionName || !get().canMutateSelectedSession()) {
      return {
        ok: false,
        error: get().mutationBlockedReason() ?? "herdr workspace.create unavailable"
      }
    }
    const caps = get().capabilities
    if (!caps?.api.workspaceCreate) {
      const message = caps?.api.reason ?? "herdr workspace.create unavailable"
      set((state) => withRuntime(state, sessionName, { errorMessage: message }, runtimeTarget))
      return { ok: false, error: message }
    }

    const previousSession = stateBefore.selectedSessionName
    const previousRuntimeTarget = stateBefore.selectedRuntimeTarget
    const previousSpace = previousSession != null
      ? selectedSpaceFor(stateBefore, previousSession, previousRuntimeTarget)
      : null
    const currentWorkspace = useWorkspaceStore.getState().workspacePath
    const needsWorkspaceSwitch = Boolean(cwd && !pathsMatch(cwd, currentWorkspace))

    // 1) Unsaved preflight BEFORE any workspace.create / selection mutation.
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
      created = await herdrWorkspaceCreate({
        ...runtimeTargetPayload(runtimeTarget),
        sessionName,
        cwd,
        label: label ?? null,
        focus: true
      })
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error)
      set((state) => withRuntime(state, sessionName, { errorMessage: message }, runtimeTarget))
      return { ok: false, error: message }
    }

    const rollbackFocus = async () => {
      if (!previousSession || !previousSpace) return
      await herdrWorkspaceFocus({
        ...runtimeTargetPayload(previousRuntimeTarget),
        sessionName: previousSession,
        workspaceId: previousSpace
      }).catch(() => undefined)
    }

    // 2) The Host Path switch remains a Yuzora operation. Runtime path mapping
    // is introduced with the WSL adapter; this Native-only seam never guesses it.
    if (needsWorkspaceSwitch) {
      try {
        const opened = await openWorkspaceAtPath(cwd, { skipUnsavedGuard: true })
        if (opened === false) {
          await rollbackFocus()
          void get().refreshSnapshot(sessionName, runtimeTarget)
          return { ok: false, cancelled: true }
        }
      } catch (error) {
        await rollbackFocus()
        void get().refreshSnapshot(sessionName, runtimeTarget)
        const message = error instanceof Error ? error.message : String(error)
        set((state) => withRuntime(state, sessionName, { errorMessage: message }, runtimeTarget))
        return { ok: false, error: message }
      }
    }

    if (created.tabId) {
      const terminalLabel =
        label?.trim() || cwd.replace(/[\\/]+$/, "").split(/[\\/]/).pop() || created.label
      await herdrTabRename({
        ...runtimeTargetPayload(runtimeTarget),
        sessionName,
        tabId: created.tabId,
        label: terminalLabel
      }).catch(() => undefined)
    }

    await get().refreshSnapshot(sessionName, runtimeTarget)
    get().setSelectedSpaceId(created.workspaceId, runtimeTarget, sessionName)
    set((state) => withRuntime(state, sessionName, { errorMessage: null }, runtimeTarget))
    const space =
      get().spaces().find((item) => item.id === created.workspaceId) ??
      ({
        id: created.workspaceId,
        label: created.label,
        order: 0,
        focused: true,
        path: created.path ?? cwd
      } satisfies HerdrSpaceInfo)
    return { ok: true, space }
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

  async activateSpace({ sessionName, runtimeTarget, workspaceId, path }) {
    const stateBefore = get()
    const target = normalizeHerdrRuntimeTarget(runtimeTarget ?? stateBefore.selectedRuntimeTarget)
    const key = runtimeKeyFor(sessionName, target)
    const previousSession = stateBefore.selectedSessionName
    const previousRuntimeTarget = stateBefore.selectedRuntimeTarget
    const previousSpace = previousSession
      ? selectedSpaceFor(stateBefore, previousSession, previousRuntimeTarget)
      : null
    const session = sessionFor(stateBefore, sessionName, target)
    if (session && !session.running) {
      return { ok: false, error: i18n.t("herdrNav.sessionStopped", { name: sessionName }) }
    }
    const targetRuntime = runtimeOf(stateBefore, sessionName, target)
    const targetCaps = targetRuntime.capabilities
    if (!targetCaps?.server.running || !targetCaps.api.workspaceFocus) {
      return {
        ok: false,
        error: targetCaps?.api.reason ?? "herdr workspace.focus unavailable"
      }
    }

    const currentWorkspace = useWorkspaceStore.getState().workspacePath
    const needsWorkspaceSwitch = Boolean(path && !pathsMatch(path, currentWorkspace))
    if (needsWorkspaceSwitch) {
      const proceed = await confirmDiscardingUnsaved({
        title: i18n.t("unsavedDialog.switchWorkspaceTitle", { ns: "menus" }),
        description: i18n.t("unsavedDialog.switchWorkspaceDescription", { ns: "menus" }),
        saveLabel: i18n.t("unsavedDialog.saveAll", { ns: "menus" })
      })
      if (!proceed) return { ok: false, cancelled: true }
    }

    const rollbackFocus = async () => {
      if (!previousSession || !previousSpace) return
      await herdrWorkspaceFocus({
        ...runtimeTargetPayload(previousRuntimeTarget),
        sessionName: previousSession,
        workspaceId: previousSpace
      }).catch(() => undefined)
    }

    try {
      await herdrWorkspaceFocus({
        ...runtimeTargetPayload(target),
        sessionName,
        workspaceId
      })
    } catch (error) {
      return { ok: false, error: error instanceof Error ? error.message : String(error) }
    }

    if (needsWorkspaceSwitch && path) {
      try {
        const opened = await openWorkspaceAtPath(path, { skipUnsavedGuard: true })
        if (opened === false) {
          await rollbackFocus()
          return { ok: false, cancelled: true }
        }
      } catch (error) {
        await rollbackFocus()
        return { ok: false, error: error instanceof Error ? error.message : String(error) }
      }
    }

    set((state) => {
      const selectedSpaceBySession = withSelectedSpace(state, sessionName, workspaceId, target)
      const runtime = runtimeOf(state, sessionName, target)
      const snapshot = runtime.snapshot
      const space = snapshot?.spaces.find((item) => item.id === workspaceId)
      const activeTab = snapshot?.tabs.find(
        (tab) => tab.workspaceId === workspaceId &&
          (tab.id === space?.activeTabId || (!space?.activeTabId && tab.active))
      )
      return {
        selectedSpaceBySession,
        ...projectSelected({ ...state, selectedSpaceBySession }, sessionName, target),
        selectedSpaceId: workspaceId,
        ...(snapshot && activeTab
          ? withRuntime(state, sessionName, {
              errorMessage: null,
              snapshot: withFocusedTab(snapshot, activeTab),
              ...(runtime.baseSnapshot
                ? { baseSnapshot: withFocusedTab(runtime.baseSnapshot, activeTab) }
                : {})
            }, target)
          : {})
      }
    })

    const focusedSnapshot = runtimeOf(get(), sessionName, target).snapshot
    const activeTab = focusedSnapshot?.tabs.find(
      (tab) => tab.workspaceId === workspaceId && tab.id === focusedSnapshot.focusedTabId
    )
    if (activeTab?.terminalId) {
      useWorkspaceStore.getState().openHerdrTerminalPage({
        herdrSessionId: sessionName,
        runtimeTarget: target,
        terminalId: activeTab.terminalId,
        title: activeTab.label,
        paneId: activeTab.paneId ?? focusedSnapshot?.focusedPaneId ?? null,
        herdrTabId: activeTab.id,
        herdrWorkspaceId: activeTab.workspaceId
      })
      if (activeTab.paneId || focusedSnapshot?.focusedPaneId) {
        get().markAttentionSeen(
          sessionName,
          activeTab.paneId ?? focusedSnapshot!.focusedPaneId!,
          target
        )
      }
    }
    void key
    return { ok: true }
  },

  async activateTab(tab) {
    const sessionName = tab.sessionName ?? get().selectedSessionName ?? HERDR_LIVE_SESSION_ID
    const runtimeTarget = normalizeHerdrRuntimeTarget(tab.runtimeTarget ?? get().selectedRuntimeTarget)
    const key = runtimeKeyFor(sessionName, runtimeTarget)
    if (!tab.terminalId) return { ok: false, error: "Herdr tab has no terminalId" }

    const stateBefore = get()
    const session = sessionFor(stateBefore, sessionName, runtimeTarget)
    const runtime = runtimeOf(stateBefore, sessionName, runtimeTarget)
    if (session && !session.running) {
      return { ok: false, error: i18n.t("herdrNav.sessionStopped", { name: sessionName }) }
    }
    if (!runtime.capabilities?.server.running || !runtime.capabilities.api.workspaceFocus || !runtime.capabilities.api.tabFocus) {
      return {
        ok: false,
        error: runtime.capabilities?.api.reason ?? "herdr workspace.focus/tab.focus unavailable"
      }
    }

    const space = runtime.snapshot?.spaces.find((item) => item.id === tab.workspaceId)
    const currentWorkspace = useWorkspaceStore.getState().workspacePath
    const needsWorkspaceSwitch = Boolean(space?.path && !pathsMatch(space.path, currentWorkspace))
    if (needsWorkspaceSwitch) {
      const proceed = await confirmDiscardingUnsaved({
        title: i18n.t("unsavedDialog.switchWorkspaceTitle", { ns: "menus" }),
        description: i18n.t("unsavedDialog.switchWorkspaceDescription", { ns: "menus" }),
        saveLabel: i18n.t("unsavedDialog.saveAll", { ns: "menus" })
      })
      if (!proceed) return { ok: false, cancelled: true }
    }

    const activationGeneration = (tabActivationGeneration.get(key) ?? 0) + 1
    tabActivationGeneration.set(key, activationGeneration)
    const isLatestActivation = () => tabActivationGeneration.get(key) === activationGeneration
    const releaseActivation = await acquireTabActivation(key)
    try {
      if (!isLatestActivation()) return { ok: false, cancelled: true }
      const stateAtMutation = get()
      const previousRuntime = runtimeOf(stateAtMutation, sessionName, runtimeTarget)
      const previousSpace = previousRuntime.snapshot?.focusedWorkspaceId ??
        selectedSpaceFor(stateAtMutation, sessionName, runtimeTarget)
      const previousTab = previousRuntime.snapshot?.focusedTabId ?? null
      const rollbackFocus = async () => {
        if (!previousSpace) return
        await herdrWorkspaceFocus({
          ...runtimeTargetPayload(runtimeTarget),
          sessionName,
          workspaceId: previousSpace
        }).catch(() => undefined)
        if (previousTab) await herdrTabFocus({
          ...runtimeTargetPayload(runtimeTarget),
          sessionName,
          tabId: previousTab
        }).catch(() => undefined)
      }

      try {
        await herdrWorkspaceFocus({
          ...runtimeTargetPayload(runtimeTarget),
          sessionName,
          workspaceId: tab.workspaceId
        })
        if (!isLatestActivation()) return { ok: false, cancelled: true }
        await herdrTabFocus({
          ...runtimeTargetPayload(runtimeTarget),
          sessionName,
          tabId: tab.id
        })
        if (!isLatestActivation()) return { ok: false, cancelled: true }
      } catch (error) {
        if (!isLatestActivation()) return { ok: false, cancelled: true }
        await rollbackFocus()
        const message = error instanceof Error ? error.message : String(error)
        set((state) => withRuntime(state, sessionName, { errorMessage: message }, runtimeTarget))
        return { ok: false, error: message }
      }

      if (needsWorkspaceSwitch && space?.path) {
        try {
          const opened = await openWorkspaceAtPath(space.path, { skipUnsavedGuard: true })
          if (!isLatestActivation()) return { ok: false, cancelled: true }
          if (opened === false) {
            await rollbackFocus()
            return { ok: false, cancelled: true }
          }
        } catch (error) {
          if (!isLatestActivation()) return { ok: false, cancelled: true }
          await rollbackFocus()
          const message = error instanceof Error ? error.message : String(error)
          set((state) => withRuntime(state, sessionName, { errorMessage: message }, runtimeTarget))
          return { ok: false, error: message }
        }
      }

      if (!isLatestActivation()) return { ok: false, cancelled: true }
      set((state) => {
        const selectedSpaceBySession = withSelectedSpace(state, sessionName, tab.workspaceId, runtimeTarget)
        const latestRuntime = runtimeOf(state, sessionName, runtimeTarget)
        const snapshot = latestRuntime.snapshot
        return {
          selectedSpaceBySession,
          ...projectSelected({ ...state, selectedSpaceBySession }, sessionName, runtimeTarget),
          selectedSpaceId: tab.workspaceId,
          ...withRuntime(state, sessionName, {
            errorMessage: null,
            ...(snapshot ? { snapshot: withFocusedTab(snapshot, tab) } : {}),
            ...(latestRuntime.baseSnapshot
              ? { baseSnapshot: withFocusedTab(latestRuntime.baseSnapshot, tab) }
              : {})
          }, runtimeTarget)
        }
      })
      useWorkspaceStore.getState().openHerdrTerminalPage({
        herdrSessionId: sessionName,
        runtimeTarget,
        terminalId: tab.terminalId,
        title: tab.label,
        paneId: tab.paneId ?? null,
        herdrTabId: tab.id,
        herdrWorkspaceId: tab.workspaceId
      })
      if (tab.paneId) get().markAttentionSeen(sessionName, tab.paneId, runtimeTarget)
      useUiStore.getState().setMode("ade")
      return { ok: true }
    } finally {
      releaseActivation()
    }
  },

  async activateAgent(agent) {
    const sessionName = agent.sessionName ?? get().selectedSessionName ?? HERDR_LIVE_SESSION_ID
    const runtimeTarget = normalizeHerdrRuntimeTarget(agent.runtimeTarget ?? get().selectedRuntimeTarget)
    if (!agent.terminalId) return { ok: false, error: "Agent has no terminalId" }

    const runtime = runtimeOf(get(), sessionName, runtimeTarget)
    if (agent.tabId) {
      const owningTab = runtime.snapshot?.tabs.find((tab) => tab.id === agent.tabId)
      if (owningTab) {
        return get().activateTab({
          ...owningTab,
          paneId: agent.paneId ?? owningTab.paneId,
          terminalId: agent.terminalId,
          sessionName,
          runtimeTarget
        })
      }
    }

    const space = runtime.snapshot?.spaces.find((item) => item.id === agent.workspaceId)
    const activation = await get().activateSpace({
      runtimeTarget,
      sessionName,
      workspaceId: agent.workspaceId,
      path: space?.path ?? null
    })
    if (!activation.ok) return activation

    useWorkspaceStore.getState().openHerdrTerminalPage({
      herdrSessionId: sessionName,
      runtimeTarget,
      terminalId: agent.terminalId,
      title: agent.title ?? agent.name,
      paneId: agent.paneId ?? null,
      herdrTabId: agent.tabId ?? null,
      herdrWorkspaceId: agent.workspaceId
    })
    if (agent.paneId) get().markAttentionSeen(sessionName, agent.paneId, runtimeTarget)
    useUiStore.getState().setMode("ade")
    return { ok: true }
  },

  async restoreFocusedState(sessionName, runtimeTarget) {
    const target = normalizeHerdrRuntimeTarget(runtimeTarget ?? get().selectedRuntimeTarget)
    const key = runtimeKeyFor(sessionName, target)
    const restoreGeneration = sessionSelectionGeneration
    const isCurrentSelection = () =>
      sessionSelectionGeneration === restoreGeneration &&
      get().selectedSessionName === sessionName &&
      sameHerdrRuntimeTarget(get().selectedRuntimeTarget, target)
    const readTarget = () => {
      if (!isCurrentSelection()) return { kind: "cancelled" as const }
      const state = get()
      const session = sessionFor(state, sessionName, target)
      const snapshot = runtimeOf(state, sessionName, target).snapshot
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
      return { kind: "ok" as const, snapshot, tab, space, focusKey: `${space.id}:${tab.id}` }
    }

    const initial = readTarget()
    if (initial.kind === "cancelled") return { ok: false, cancelled: true }
    if (initial.kind === "error") return { ok: false, error: initial.error }
    const focusKey = initial.focusKey
    let focused = initial
    const adoptLatestOrCancel = (): HerdrActivationResult | null => {
      const latest = readTarget()
      if (latest.kind === "cancelled") return { ok: false, cancelled: true }
      if (latest.kind === "error") return { ok: false, error: latest.error }
      if (latest.focusKey !== focusKey) return { ok: false, cancelled: true }
      focused = latest
      return null
    }

    const currentWorkspace = useWorkspaceStore.getState().workspacePath
    const needsWorkspaceSwitch = Boolean(focused.space.path && !pathsMatch(focused.space.path, currentWorkspace))
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
        const opened = await openWorkspaceAtPath(focused.space.path!, { skipUnsavedGuard: true })
        if (opened === false) return { ok: false, cancelled: true }
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error)
        set((state) => withRuntime(state, sessionName, { errorMessage: message }, target))
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
        state.selectedSessionName !== sessionName ||
        !sameHerdrRuntimeTarget(state.selectedRuntimeTarget, target)
      ) return state
      const selectedSpaceBySession = withSelectedSpace(state, sessionName, focused.space.id, target)
      return {
        selectedSpaceBySession,
        ...projectSelected({ ...state, selectedSpaceBySession }, sessionName, target),
        selectedSpaceId: focused.space.id,
        ...withRuntime(state, sessionName, { errorMessage: null }, target)
      }
    })
    if (!isCurrentSelection()) return { ok: false, cancelled: true }
    const defaultSessionName = get().sessions.find(
      (session) => session.default && sameHerdrRuntimeTarget(session.runtimeTarget, target)
    )?.name ?? null
    useWorkspaceStore.getState().hydrateHerdrPagesFromSnapshot(focused.snapshot, defaultSessionName)
    if (focused.tab.paneId || focused.snapshot.focusedPaneId) {
      get().markAttentionSeen(sessionName, focused.tab.paneId ?? focused.snapshot.focusedPaneId!, target)
    }
    void key
    useUiStore.getState().setMode("ade")
    return { ok: true }
  },

  applySubscriptionEvent(sessionName, event, runtimeTarget) {
    const current = get()
    const target = normalizeHerdrRuntimeTarget(runtimeTarget ?? current.selectedRuntimeTarget)
    if (
      current.selectedSessionName !== sessionName ||
      !sameHerdrRuntimeTarget(current.selectedRuntimeTarget, target)
    ) return
    if (event.type === "subscribed") {
      set({ eventsHealthy: true, eventsSubscriptionId: event.subscriptionId })
      return
    }
    if (current.eventsSubscriptionId !== event.subscriptionId) return
    if (event.type === "error" || event.type === "disconnected") {
      set((state) => ({
        eventsHealthy: false,
        errorMessage: event.type === "error" ? event.message : state.errorMessage
      }))
      return
    }
    if (event.type === "pane_exited") {
      const attentionKey = herdrAttentionKey(sessionName, event.paneId, target)
      set((state) => {
        const attentionByKey = new Map(state.attentionByKey)
        attentionByKey.delete(attentionKey)
        return { attentionByKey, eventsHealthy: true }
      })
      return
    }
    if (event.type === "worktree_changed") {
      set({ eventsHealthy: true })
      void get().refreshWorktreeInventory(sessionName, target)
      return
    }
    if (event.type === "topology_changed") {
      set({ eventsHealthy: true })
      return
    }
    if (event.type !== "agent_status_changed" || !event.paneId) return

    const kind = attentionKindForStatus(event.agentStatus)
    const attentionKey = herdrAttentionKey(sessionName, event.paneId, target)
    set((state) => {
      const attentionByKey = new Map(state.attentionByKey)
      if (!kind) {
        attentionByKey.delete(attentionKey)
        return { attentionByKey, eventsHealthy: true }
      }
      const previous = attentionByKey.get(attentionKey)
      attentionByKey.set(attentionKey, {
        key: attentionKey,
        runtimeTarget: target,
        sessionName,
        paneId: event.paneId,
        workspaceId: event.workspaceId,
        agentStatus: event.agentStatus as HerdrAgentStatus,
        kind,
        title: event.title ?? previous?.title ?? null,
        displayAgent: event.displayAgent ?? previous?.displayAgent ?? null,
        seen: kind === "done" ? (previous?.seen ?? false) : false,
        updatedAt: Date.now()
      })
      return { attentionByKey, eventsHealthy: true }
    })
  },

  setEventsHealth(sessionName, healthy, subscriptionId = null, runtimeTarget) {
    const target = normalizeHerdrRuntimeTarget(runtimeTarget ?? get().selectedRuntimeTarget)
    set((state) => {
      if (
        state.selectedSessionName !== sessionName ||
        !sameHerdrRuntimeTarget(state.selectedRuntimeTarget, target)
      ) return state
      return {
        eventsHealthy: healthy,
        eventsSubscriptionId:
          subscriptionId === undefined ? state.eventsSubscriptionId : subscriptionId
      }
    })
  },

  markAttentionSeen(sessionName, paneId, runtimeTarget) {
    if (!sessionName || !paneId) return
    const target = normalizeHerdrRuntimeTarget(runtimeTarget)
    const key = herdrAttentionKey(sessionName, paneId, target)
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

  attentionItems(sessionName, runtimeTarget) {
    const selected = sessionName ?? get().selectedSessionName
    const target = normalizeHerdrRuntimeTarget(runtimeTarget ?? get().selectedRuntimeTarget)
    const items = Array.from(get().attentionByKey.values()).filter((item) => {
      if (selected && item.sessionName !== selected) return false
      if (!sameHerdrRuntimeTarget(item.runtimeTarget, target)) return false
      if (item.kind === "done" && item.seen) return false
      return true
    })
    items.sort((a, b) => b.updatedAt - a.updatedAt)
    return items
  },

  canInspectAgent(sessionName, runtimeTarget) {
    const state = get()
    const target = normalizeHerdrRuntimeTarget(runtimeTarget ?? state.selectedRuntimeTarget)
    const resolved = sessionName ?? state.selectedSessionName
    if (!resolved) return false
    const session = sessionFor(state, resolved, target)
    const caps = runtimeOf(state, resolved, target).capabilities ??
      (resolved === state.selectedSessionName &&
      sameHerdrRuntimeTarget(state.selectedRuntimeTarget, target)
        ? state.capabilities
        : null)
    if (!session?.running || !caps?.server.running) return false
    return Boolean(caps.api.agentGet && caps.api.agentRead)
  }
}))
