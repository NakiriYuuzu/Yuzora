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
  return `${sessionName}::${paneId}`
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
  refreshSessions: () => Promise<void>
  selectSession: (sessionName: string) => Promise<void>
  bootstrap: (sessionName?: string | null) => Promise<void>
  refreshSnapshot: (sessionName?: string | null) => Promise<boolean>
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
    workspaceId: string
    path?: string | null
  }) => Promise<HerdrActivationResult>
  activateTab: (tab: HerdrTabInfo) => Promise<HerdrActivationResult>
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
  canInspectAgent: (sessionName?: string | null) => boolean
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
  state: Pick<HerdrState, "runtimesBySession" | "selectedSessionName">,
  sessionName?: string | null
): HerdrSessionRuntime {
  const key = sessionName ?? state.selectedSessionName
  if (!key) return emptyRuntime()
  return state.runtimesBySession[key] ?? emptyRuntime()
}

function withRuntime(
  state: HerdrState,
  sessionName: string,
  patch: Partial<HerdrSessionRuntime>
): Partial<HerdrState> {
  const previous = state.runtimesBySession[sessionName] ?? emptyRuntime()
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

export const herdrInitialState = {
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
  attachments: new Map<string, HerdrAttachmentRecord>(),
  attentionByKey: new Map<string, HerdrAttentionItem>(),
  eventsHealthy: false,
  eventsSubscriptionId: null as string | null
}

/** Module-level in-flight guards — not part of reactive state. */
let sessionsInFlight: Promise<void> | null = null
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

async function acquireTabActivation(sessionName: string): Promise<() => void> {
  const previous = tabActivationTail.get(sessionName) ?? Promise.resolve()
  let releaseGate!: () => void
  const gate = new Promise<void>((resolve) => {
    releaseGate = resolve
  })
  const tail = previous.catch(() => undefined).then(() => gate)
  tabActivationTail.set(sessionName, tail)
  await previous.catch(() => undefined)

  let released = false
  return () => {
    if (released) return
    released = true
    releaseGate()
    void tail.then(() => {
      if (tabActivationTail.get(sessionName) === tail) {
        tabActivationTail.delete(sessionName)
      }
    })
  }
}

export const useHerdrStore = create<HerdrState>((set, get) => ({
  ...herdrInitialState,

  selectedSession() {
    const name = get().selectedSessionName
    if (!name) return null
    return get().sessions.find((s) => s.name === name) ?? null
  },

  async refreshSessions() {
    if (sessionsInFlight) return sessionsInFlight
    sessionsInFlight = (async () => {
      try {
        const sessions = await herdrSessions()
        set((state) => {
          let selectedSessionName = state.selectedSessionName
          if (
            !selectedSessionName ||
            !sessions.some((session) => session.name === selectedSessionName)
          ) {
            selectedSessionName =
              sessions.find((session) => session.default)?.name ??
              sessions[0]?.name ??
              null
          }
          return {
            sessions,
            ...projectSelected({ ...state, sessions }, selectedSessionName)
          }
        })
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error)
        set((state) => ({
          errorMessage: message,
          connectionState:
            state.snapshot || state.sessions.length > 0 ? state.connectionState : "error"
        }))
      } finally {
        sessionsInFlight = null
      }
    })()
    return sessionsInFlight
  },

  async selectSession(sessionName) {
    const session = get().sessions.find((item) => item.name === sessionName)
    if (!session) return
    sessionSelectionGeneration += 1
    set((state) => ({
      ...projectSelected(state, sessionName),
      selectedSpaceId: state.selectedSpaceBySession[sessionName] ?? null
    }))
    // Switching sessions must not close pages / TerminalDrawer — only selection changes.
    if (session.running) {
      await get().bootstrap(sessionName)
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
      get().sessions.find((s) => s.default)?.name ??
      HERDR_LIVE_SESSION_ID
    const existing = bootstrapInFlight.get(resolved)
    if (existing) return existing

    const task = (async () => {
      set((state) => withRuntime(state, resolved, {
        connectionState: "connecting",
        errorMessage: null
      }))
      try {
        const named = get().sessions.find((s) => s.name === resolved)
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
        const snapshot = normalizeHerdrSnapshot(raw, resolved)
        get().applySnapshot(resolved, snapshot)
        set((state) =>
          withRuntime(state, resolved, {
            connectionState: "ready",
            errorMessage: null
          })
        )
        // Authoritative inventory reconcile after snapshot recovery.
        await get().refreshWorktreeInventory(resolved)
      } catch (error) {
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
      } finally {
        bootstrapInFlight.delete(resolved)
      }
    })()
    bootstrapInFlight.set(resolved, task)
    return task
  },

  async refreshSnapshot(sessionName) {
    const resolved = sessionName ?? get().selectedSessionName
    if (!resolved) return false
    const named = get().sessions.find((s) => s.name === resolved)
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
            const raw = await herdrSnapshot(resolved)
            const snapshot = normalizeHerdrSnapshot(raw, resolved)
            get().applySnapshot(resolved, snapshot)
            set((state) =>
              withRuntime(state, resolved, {
                connectionState: "ready",
                errorMessage: null
              })
            )
            await get().refreshWorktreeInventory(resolved)
            consecutiveFailures = 0
            passSucceeded = true
          } catch (error) {
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
    const named = get().sessions.find((s) => s.name === resolved)
    if (named && !named.running) return
    const runtime = runtimeOf(get(), resolved)
    if (!runtime.capabilities?.api.worktreeList) return

    const requestedGeneration =
      (worktreeInventoryRequestedGeneration.get(resolved) ?? 0) + 1
    worktreeInventoryRequestedGeneration.set(resolved, requestedGeneration)
    const existing = worktreeInventoryInFlight.get(resolved)
    if (existing) return existing

    const task = (async () => {
      try {
        while (true) {
          const completedGeneration =
            worktreeInventoryRequestedGeneration.get(resolved) ?? requestedGeneration
          const snapshotAtStart = snapshotGeneration.get(resolved) ?? 0
          const current = runtimeOf(get(), resolved)
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
          if ((snapshotGeneration.get(resolved) ?? 0) !== snapshotAtStart) {
            worktreeInventoryRequestedGeneration.set(
              resolved,
              Math.max(
                worktreeInventoryRequestedGeneration.get(resolved) ?? 0,
                completedGeneration + 1
              )
            )
            continue
          }

          const inventory = buildWorktreeInventory(resolved, lists, failedScopes)
          set((state) => {
            const latest = runtimeOf(state, resolved)
            const projectionBase = latest.baseSnapshot ?? latest.snapshot
            const projectedSnapshot = projectionBase
              ? withInventoryOnSnapshot(projectionBase, inventory)
              : null
            return withRuntime(state, resolved, {
              worktreeInventory: inventory,
              snapshot: projectedSnapshot
            })
          })
          if (
            (worktreeInventoryRequestedGeneration.get(resolved) ?? 0) <=
            completedGeneration
          ) {
            break
          }
        }
      } finally {
        worktreeInventoryInFlight.delete(resolved)
      }
    })()
    worktreeInventoryInFlight.set(resolved, task)
    return task
  },

  applySnapshot(sessionName, snapshot) {
    snapshotGeneration.set(sessionName, (snapshotGeneration.get(sessionName) ?? 0) + 1)
    const previousRuntime = get().runtimesBySession[sessionName]
    const inventory = previousRuntime?.worktreeInventory ?? null
    const canReuseInventory =
      worktreeProjectionScope(previousRuntime?.baseSnapshot) ===
      worktreeProjectionScope(snapshot)
    const reusableInventory = canReuseInventory ? inventory : null
    const mergedSnapshot = withInventoryOnSnapshot(snapshot, reusableInventory)
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
      const selectedSpaceBySession = {
        ...state.selectedSpaceBySession,
        [sessionName]: nextSpace
      }
      const runtimePatch = withRuntime(state, sessionName, {
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
        attentionByKey.set(key, {
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
        })
      }
      for (const [key, item] of attentionByKey) {
        if (item.sessionName === sessionName && !livePaneKeys.has(key)) {
          attentionByKey.delete(key)
        }
      }
      return {
        ...runtimePatch,
        selectedSpaceBySession,
        attentionByKey,
        selectedSpaceId:
          state.selectedSessionName === sessionName
            ? nextSpace
            : state.selectedSpaceId
      }
    })
    const defaultSessionName = get().sessions.find((session) => session.default)?.name ?? null
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
    if (!sessionName || !get().canMutateSelectedSession()) {
      return {
        ok: false,
        error: get().mutationBlockedReason() ?? "herdr workspace.create unavailable"
      }
    }
    const caps = get().capabilities
    if (!caps?.api.workspaceCreate) {
      const message = caps?.api.reason ?? "herdr workspace.create unavailable"
      set((state) => withRuntime(state, sessionName, { errorMessage: message }))
      return { ok: false, error: message }
    }

    const previousSession = stateBefore.selectedSessionName
    const previousSpace =
      previousSession != null
        ? stateBefore.selectedSpaceBySession[previousSession] ?? null
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
      if (!proceed) {
        return { ok: false, cancelled: true }
      }
    }

    // 2) Create (+ focus) Herdr Space only after preflight.
    let created
    try {
      created = await herdrWorkspaceCreate({
        sessionName,
        cwd,
        label: label ?? null,
        focus: true
      })
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error)
      set((state) => withRuntime(state, sessionName, { errorMessage: message }))
      return { ok: false, error: message }
    }

    // 3) Preapproved local Yuzora switch (no second unsaved guard).
    if (needsWorkspaceSwitch) {
      try {
        const opened = await openWorkspaceAtPath(cwd, { skipUnsavedGuard: true })
        if (opened === false) {
          if (previousSession && previousSpace) {
            await herdrWorkspaceFocus({
              sessionName: previousSession,
              workspaceId: previousSpace
            }).catch(() => undefined)
          }
          // Do not commit the new Space selection.
          void get().refreshSnapshot(sessionName)
          return { ok: false, cancelled: true }
        }
      } catch (error) {
        if (previousSession && previousSpace) {
          await herdrWorkspaceFocus({
            sessionName: previousSession,
            workspaceId: previousSpace
          }).catch(() => undefined)
        }
        void get().refreshSnapshot(sessionName)
        const message = error instanceof Error ? error.message : String(error)
        set((state) => withRuntime(state, sessionName, { errorMessage: message }))
        return { ok: false, error: message }
      }
    }

    // 4) The workspace-created root terminal inherits the opened folder name.
    if (created.tabId) {
      const terminalLabel =
        label?.trim() || cwd.replace(/[\\/]+$/, "").split(/[\\/]/).pop() || created.label
      await herdrTabRename({
        sessionName,
        tabId: created.tabId,
        label: terminalLabel
      }).catch(() => undefined)
    }

    // 5) Commit selection only after success.
    await get().refreshSnapshot(sessionName)
    get().setSelectedSpaceId(created.workspaceId)
    set((state) => withRuntime(state, sessionName, { errorMessage: null }))
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

  async activateSpace({ sessionName, workspaceId, path }) {
    const stateBefore = get()
    const previousSession = stateBefore.selectedSessionName
    const previousSpace =
      previousSession != null
        ? stateBefore.selectedSpaceBySession[previousSession] ?? null
        : null

    const session = stateBefore.sessions.find((item) => item.name === sessionName)
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

    const currentWorkspace = useWorkspaceStore.getState().workspacePath
    const needsWorkspaceSwitch = Boolean(path && !pathsMatch(path, currentWorkspace))

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

    // 2) Focus Herdr Space on the target running session.
    try {
      await herdrWorkspaceFocus({ sessionName, workspaceId })
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error)
      return { ok: false, error: message }
    }

    // 3) Guarded Yuzora workspace switch when a Space path exists.
    if (needsWorkspaceSwitch && path) {
      try {
        const opened = await openWorkspaceAtPath(path, {
          skipUnsavedGuard: true
        })
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
          ? withRuntime(state, sessionName, {
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
    return { ok: true }
  },

  async activateTab(tab) {
    const sessionName = tab.sessionName ?? get().selectedSessionName ?? HERDR_LIVE_SESSION_ID
    if (!tab.terminalId) {
      return { ok: false, error: "Herdr tab has no terminalId" }
    }

    const stateBefore = get()
    const session = stateBefore.sessions.find((item) => item.name === sessionName)
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

    const space = runtime.snapshot?.spaces.find((item) => item.id === tab.workspaceId)
    const currentWorkspace = useWorkspaceStore.getState().workspacePath
    const needsWorkspaceSwitch = Boolean(space?.path && !pathsMatch(space.path, currentWorkspace))

    // Unsaved preflight must complete before workspace.focus or tab.focus.
    if (needsWorkspaceSwitch) {
      const proceed = await confirmDiscardingUnsaved({
        title: i18n.t("unsavedDialog.switchWorkspaceTitle", { ns: "menus" }),
        description: i18n.t("unsavedDialog.switchWorkspaceDescription", { ns: "menus" }),
        saveLabel: i18n.t("unsavedDialog.saveAll", { ns: "menus" })
      })
      if (!proceed) return { ok: false, cancelled: true }
    }

    const activationGeneration = (tabActivationGeneration.get(sessionName) ?? 0) + 1
    tabActivationGeneration.set(sessionName, activationGeneration)
    const isLatestActivation = () =>
      tabActivationGeneration.get(sessionName) === activationGeneration
    const releaseActivation = await acquireTabActivation(sessionName)

    try {
      if (!isLatestActivation()) return { ok: false, cancelled: true }

      // Capture rollback state only after earlier mutations in this session
      // have settled. The per-session queue prevents a stale RPC from applying
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
        if (previousTab) {
          await herdrTabFocus({
            sessionName,
            tabId: previousTab
          }).catch(() => undefined)
        }
      }

      try {
        await herdrWorkspaceFocus({ sessionName, workspaceId: tab.workspaceId })
        if (!isLatestActivation()) return { ok: false, cancelled: true }
        await herdrTabFocus({ sessionName, tabId: tab.id })
        if (!isLatestActivation()) return { ok: false, cancelled: true }
      } catch (error) {
        if (!isLatestActivation()) return { ok: false, cancelled: true }
        await rollbackFocus()
        const message = error instanceof Error ? error.message : String(error)
        set((state) => withRuntime(state, sessionName, { errorMessage: message }))
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
          ...withRuntime(state, sessionName, {
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
        return get().activateTab({
          ...owningTab,
          paneId: agent.paneId ?? owningTab.paneId,
          terminalId: agent.terminalId,
          sessionName
        })
      }
    }

    const space =
      get().runtimesBySession[sessionName]?.snapshot?.spaces.find(
        (item) => item.id === agent.workspaceId
      ) ?? get().spaces().find((item) => item.id === agent.workspaceId)

    const activation = await get().activateSpace({
      sessionName,
      workspaceId: agent.workspaceId,
      path: space?.path ?? null
    })
    if (!activation.ok) return activation

    useWorkspaceStore.getState().openHerdrTerminalPage({
      herdrSessionId: sessionName,
      terminalId: agent.terminalId,
      title: agent.title ?? agent.name,
      paneId: agent.paneId ?? null,
      herdrTabId: agent.tabId ?? null,
      herdrWorkspaceId: agent.workspaceId
    })
    if (agent.paneId) get().markAttentionSeen(sessionName, agent.paneId)
    useUiStore.getState().setMode("ade")
    return { ok: true }
  },

  async restoreFocusedState(sessionName) {
    const restoreGeneration = sessionSelectionGeneration
    const isCurrentSelection = () =>
      sessionSelectionGeneration === restoreGeneration &&
      get().selectedSessionName === sessionName
    const readTarget = () => {
      if (!isCurrentSelection()) return { kind: "cancelled" as const }
      const state = get()
      const session = state.sessions.find((item) => item.name === sessionName)
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
          skipUnsavedGuard: true
        })
        if (opened === false) return { ok: false, cancelled: true }
      } catch (error) {
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
        ...withRuntime(state, sessionName, { errorMessage: null })
      }
    })
    if (!isCurrentSelection()) return { ok: false, cancelled: true }
    const defaultSessionName =
      get().sessions.find((session) => session.default)?.name ?? null
    useWorkspaceStore
      .getState()
      .hydrateHerdrPagesFromSnapshot(target.snapshot, defaultSessionName)
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
    if (current.selectedSessionName !== sessionName) return
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
      const key = herdrAttentionKey(sessionName, event.paneId)
      set((state) => {
        const attentionByKey = new Map(state.attentionByKey)
        attentionByKey.delete(key)
        return { attentionByKey, eventsHealthy: true }
      })
      return
    }
    if (event.type === "worktree_changed") {
      set({ eventsHealthy: true })
      // Dirty signal only — authoritative recovery is list + snapshot.
      void get().refreshWorktreeInventory(sessionName)
      return
    }
    if (event.type === "topology_changed") {
      set({ eventsHealthy: true })
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
        return { attentionByKey, eventsHealthy: true }
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
      return { attentionByKey, eventsHealthy: true }
    })
  },

  setEventsHealth(sessionName, healthy, subscriptionId = null) {
    set((state) => {
      if (state.selectedSessionName !== sessionName) return state
      return {
        eventsHealthy: healthy,
        eventsSubscriptionId:
          subscriptionId === undefined ? state.eventsSubscriptionId : subscriptionId
      }
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
    const selected = sessionName ?? get().selectedSessionName
    const items = Array.from(get().attentionByKey.values()).filter((item) => {
      if (selected && item.sessionName !== selected) return false
      if (item.kind === "done" && item.seen) return false
      return true
    })
    items.sort((a, b) => b.updatedAt - a.updatedAt)
    return items
  },

  canInspectAgent(sessionName) {
    const state = get()
    const resolved = sessionName ?? state.selectedSessionName
    if (!resolved) return false
    const session = state.sessions.find((item) => item.name === resolved)
    const caps =
      state.runtimesBySession[resolved]?.capabilities ??
      (resolved === state.selectedSessionName ? state.capabilities : null)
    if (!session?.running || !caps?.server.running) return false
    return Boolean(caps.api.agentGet && caps.api.agentRead)
  }
}))
