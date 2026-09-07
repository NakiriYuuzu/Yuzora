import { useEffect, useRef } from "react"
import { herdrEventsRelease, herdrEventsSubscribe } from "@/lib/herdrIpc"
import { runtimeOwner, sessionScope } from "@/lib/herdrProvider"
import { isHerdrPagePath } from "@/lib/herdrPages"
import { canonicalPathKey } from "@/lib/paths"
import { useHerdrStore } from "@/state/herdrStore"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { HERDR_HEALTHY_SNAPSHOT_FALLBACK_MS, shouldPollHerdrSnapshots, shouldRefreshWorktreeInventory } from "./herdrBridgePolicy"

interface RuntimeSubscription {
  scope: string
  identity: string
  inFlight: boolean
  connecting: boolean
  subscriptionId: string | null
  subscriptionGeneration: number
  paneKey: string | null
  attempts: number
  nextAttempt: number
  lastSnapshot: number
  lastInventory: number
  refreshTimer: ReturnType<typeof setTimeout> | null
  retryTimer: ReturnType<typeof setTimeout> | null
}

/** Each running runtime owns its polling/backoff and event stream. Only the
 * focused runtime restores the visible workspace; all runtimes feed attention. */
export function HerdrBridge() {
  const cancelledRef = useRef(false)
  const restoredFocusRef = useRef(new Map<string, string>())
  const restoringFocusRef = useRef(new Set<string>())

  useEffect(() => {
    cancelledRef.current = false
    const active = new Map<string, RuntimeSubscription>()
    let listing = false
    const current = (entry: RuntimeSubscription) => !cancelledRef.current && active.get(entry.scope) === entry
    const release = (entry: RuntimeSubscription) => {
      entry.subscriptionGeneration++
      if (entry.refreshTimer) clearTimeout(entry.refreshTimer)
      if (entry.retryTimer) clearTimeout(entry.retryTimer)
      entry.refreshTimer = null
      entry.retryTimer = null
      const id = entry.subscriptionId
      entry.subscriptionId = null
      useHerdrStore.getState().setEventsHealth(entry.scope, false, null)
      if (id) void herdrEventsRelease(id).catch(() => undefined)
    }
    const maybeRestoreFocusedView = async (sessionName: string) => {
      if (
        cancelledRef.current ||
        useHerdrStore.getState().selectedSessionName !== sessionName
      ) {
        return
      }
      if (!useWorkspaceStore.getState().sessionRestoreReady) return
      const snapshot = useHerdrStore.getState().runtimesBySession[sessionName]?.snapshot
      if (!snapshot?.focusedWorkspaceId || !snapshot.focusedTabId) return
      const workspace = useWorkspaceStore.getState().workspacePath
      const root = snapshot.spaces.find((space) => space.id === snapshot.focusedWorkspaceId)?.path
      // A reconnect/snapshot cannot replace the restored or user-opened folder.
      // Explicit Space/tab activation owns the guarded workspace switch.
      if (workspace && (!root || canonicalPathKey(workspace) !== canonicalPathKey(root))) return
      const focusKey = `${snapshot.focusedWorkspaceId}:${snapshot.focusedTabId}`
      if (restoredFocusRef.current.get(sessionName) === focusKey) return
      if (restoringFocusRef.current.has(sessionName)) return

      restoringFocusRef.current.add(sessionName)
      try {
        const result = await useHerdrStore.getState().restoreFocusedState(sessionName)
        // Only a committed restore owns this runtime focus key. A cancelled
        // hydration/selection race is retryable; marking it restored here could
        // leave the app permanently on the empty Intro surface.
        if (result.ok) {
          restoredFocusRef.current.set(sessionName, focusKey)
        }
      } finally {
        restoringFocusRef.current.delete(sessionName)
        const latest = useHerdrStore.getState().runtimesBySession[sessionName]?.snapshot
        const latestKey =
          latest?.focusedWorkspaceId && latest.focusedTabId
            ? `${latest.focusedWorkspaceId}:${latest.focusedTabId}`
            : null
        if (
          latestKey &&
          latestKey !== focusKey &&
          useHerdrStore.getState().selectedSessionName === sessionName
        ) {
          void maybeRestoreFocusedView(sessionName)
        }
      }
    }


    const scheduleRefresh = (entry: RuntimeSubscription) => {
      if (!current(entry) || entry.refreshTimer) return
      entry.refreshTimer = setTimeout(() => {
        entry.refreshTimer = null
        if (current(entry)) void refresh(entry, true)
      }, 250)
    }

    const subscribe = async (entry: RuntimeSubscription) => {
      if (!current(entry) || entry.connecting || entry.retryTimer) return
      const paneIds = [...new Set(useHerdrStore.getState().runtimesBySession[entry.scope]?.snapshot?.terminals
        .flatMap((terminal) => terminal.paneId ? [terminal.paneId] : []) ?? [])].sort()
      const paneKey = JSON.stringify(paneIds)
      if (entry.subscriptionId && entry.paneKey === paneKey) return
      const caps = useHerdrStore.getState().runtimesBySession[entry.scope]?.capabilities
      if (!caps?.api.eventsSubscribe || caps.events.status !== "available" || !caps.server.running) return
      entry.connecting = true
      const generation = ++entry.subscriptionGeneration
      const currentSubscription = () => current(entry) && entry.subscriptionGeneration === generation
      let terminated = false
      const retry = () => {
        if (!current(entry) || entry.retryTimer) return
        const delay = Math.min(16000, 1000 * 2 ** Math.min(entry.attempts++, 4))
        entry.retryTimer = setTimeout(() => {
          entry.retryTimer = null
          if (current(entry)) void subscribe(entry)
        }, delay)
      }
      try {
        const previousId = entry.subscriptionId
        entry.subscriptionId = null
        if (previousId) {
          useHerdrStore.getState().setEventsHealth(entry.scope, false, null)
          await herdrEventsRelease(previousId).catch(() => undefined)
          if (!currentSubscription()) return
        }
        entry.paneKey = paneKey
        const id = await herdrEventsSubscribe({
          sessionName: entry.scope,
          paneIds,
          onEvent: (event) => {
            if (!currentSubscription() || terminated) return
            if (event.type === "subscribed") {
              entry.subscriptionId = event.subscriptionId
              entry.attempts = 0
            } else if (!entry.subscriptionId || event.subscriptionId !== entry.subscriptionId) return
            useHerdrStore.getState().applySubscriptionEvent(entry.scope, event)
            if (event.type === "disconnected" || event.type === "error") {
              terminated = true
              entry.subscriptionId = null
              void herdrEventsRelease(event.subscriptionId).catch(() => undefined)
              scheduleRefresh(entry)
              retry()
            } else if (event.type !== "subscribed") {
              if (event.type === "pane_exited" || event.type === "topology_changed") useHerdrStore.getState().bumpTopologyRevision()
              scheduleRefresh(entry)
            }
          }
        })
        if (!currentSubscription() || terminated || (entry.subscriptionId && entry.subscriptionId !== id)) {
          await herdrEventsRelease(id).catch(() => undefined)
          return
        }
        entry.subscriptionId = id
        entry.attempts = 0
        useHerdrStore.getState().setEventsHealth(entry.scope, true, id)
      } catch {
        if (currentSubscription()) {
          entry.subscriptionId = null
          useHerdrStore.getState().setEventsHealth(entry.scope, false, null)
          retry()
        }
      } finally { entry.connecting = false }
    }

    const refresh = async (entry: RuntimeSubscription, force = false) => {
      if (!current(entry) || entry.inFlight || (!force && Date.now() < entry.nextAttempt)) return
      entry.inFlight = true
      try {
        const state = useHerdrStore.getState()
        const runtime = state.runtimesBySession[entry.scope]
        let ok = true
        if (runtime?.connectionState !== "ready") {
          await state.bootstrap(entry.scope)
          ok = useHerdrStore.getState().runtimesBySession[entry.scope]?.connectionState === "ready"
        } else if (force || shouldPollHerdrSnapshots(runtime.capabilities, !!entry.subscriptionId, Date.now() - entry.lastSnapshot, HERDR_HEALTHY_SNAPSHOT_FALLBACK_MS)) {
          ok = await state.refreshSnapshot(entry.scope)
          if (ok) entry.lastSnapshot = Date.now()
        }
        if (!current(entry)) return
        if (!ok) {
          entry.nextAttempt = Date.now() + Math.min(16000, 1000 * 2 ** Math.min(entry.attempts++, 4))
          return
        }
        entry.nextAttempt = 0
        const latest = useHerdrStore.getState().runtimesBySession[entry.scope]
        if (shouldRefreshWorktreeInventory(latest?.capabilities ?? null, Date.now() - entry.lastInventory, 30000)) {
          entry.lastInventory = Date.now()
          void useHerdrStore.getState().refreshWorktreeInventory(entry.scope).catch(() => undefined)
        }
        void subscribe(entry)
        await maybeRestoreFocusedView(entry.scope)
      } finally { entry.inFlight = false }
    }

    const reconcileRuntimes = () => {
      const wanted = new Map<string, string>()
      for (const session of useHerdrStore.getState().sessions) {
        const scope = sessionScope(session)!
        if (session.running) wanted.set(scope, JSON.stringify([runtimeOwner(scope), session.socketPath]))
      }
      for (const [scope, entry] of active) {
        if (wanted.get(scope) !== entry.identity) {
          active.delete(scope)
          release(entry)
          restoredFocusRef.current.delete(scope)
        }
      }
      for (const [scope, identity] of wanted) {
        let entry = active.get(scope)
        if (!entry) {
          entry = {scope, identity, inFlight:false, connecting:false, subscriptionId:null, subscriptionGeneration:0, paneKey:null, attempts:0, nextAttempt:0, lastSnapshot:0, lastInventory:Date.now(), refreshTimer:null, retryTimer:null}
          active.set(scope, entry)
        }
        void refresh(entry).catch(() => undefined)
      }
    }
    const poll = async () => {
      if (listing || cancelledRef.current) return
      listing = true
      try {
        await useHerdrStore.getState().refreshSessions()
        if (!cancelledRef.current) reconcileRuntimes()
      } finally { listing = false }
    }
    void poll()
    const interval = setInterval(() => void poll(), 4000)
    const unsubscribeWorkspaceRestore = useWorkspaceStore.subscribe((state, previous) => {
      if (!state.sessionRestoreReady || previous.sessionRestoreReady) return
      const scope = useHerdrStore.getState().selectedSessionName
      if (scope) void maybeRestoreFocusedView(scope)
    })
    const unsubscribeFocus = useHerdrStore.subscribe((state, previous) => {
      for (const [runtimeScope, entry] of active) {
        if (state.runtimesBySession[runtimeScope]?.snapshot !== previous.runtimesBySession[runtimeScope]?.snapshot) {
          void subscribe(entry)
        }
      }
      const scope = state.selectedSessionName
      if (!scope) return
      if (scope !== previous.selectedSessionName) {
        const entry = active.get(scope)
        state.setEventsHealth(scope, !!entry?.subscriptionId, entry?.subscriptionId ?? null)
      }
      if (scope !== previous.selectedSessionName || state.runtimesBySession[scope]?.snapshot !== previous.runtimesBySession[scope]?.snapshot) {
        void maybeRestoreFocusedView(scope)
      }
    })
    return () => {
      cancelledRef.current = true
      clearInterval(interval)
      unsubscribeWorkspaceRestore()
      unsubscribeFocus()
      const entries = [...active.values()]
      active.clear()
      entries.forEach(release)
      void useHerdrStore.getState().releaseAllAttachments().catch(() => undefined)
    }
  }, [])

  useEffect(() => {
    const reconcile = () => {
      const openPaths = new Set<string>()
      for (const group of useWorkspaceStore.getState().groups) {
        for (const tab of group.tabs) {
          if (tab.kind === "herdr-terminal" || isHerdrPagePath(tab.path)) {
            openPaths.add(tab.path)
          }
        }
      }
      const attachments = useHerdrStore.getState().attachments
      for (const [attachmentKey, record] of attachments) {
        if (!openPaths.has(record.pagePath)) {
          void useHerdrStore.getState().releaseAttachment(attachmentKey)
        }
      }
    }

    reconcile()
    return useWorkspaceStore.subscribe(reconcile)
  }, [])

  return null
}
