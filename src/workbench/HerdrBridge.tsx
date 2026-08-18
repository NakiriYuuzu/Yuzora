import { useEffect, useRef } from "react"

import { herdrEventsRelease, herdrEventsSubscribe } from "@/lib/herdrIpc"
import { isHerdrPagePath } from "@/lib/herdrPages"
import { herdrSessionKey, sameHerdrRuntimeTarget } from "@/lib/herdrRuntime"
import type { HerdrRuntimeTarget } from "@/lib/herdrTypes"
import { useHerdrStore } from "@/state/herdrStore"
import { useWorkspaceStore } from "@/state/workspaceStore"
import {
  HERDR_HEALTHY_SNAPSHOT_FALLBACK_MS,
  shouldPollHerdrSnapshots,
  shouldRefreshWorktreeInventory
} from "@/workbench/herdrBridgePolicy"

const SNAPSHOT_POLL_MS = 4000
const WORKTREE_INVENTORY_FALLBACK_MS = 30_000
const RETRY_BASE_MS = 1000
const RETRY_MAX_MS = 16000
const EVENT_REFRESH_DEBOUNCE_MS = 250

/**
 * Headless Herdr bridge.
 * - Refreshes named sessions every 4s.
 * - Bootstraps/polls snapshot only for the selected running Runtime Session.
 * - Owns one events.subscribe stream per selected Runtime Session.
 * - Never starts stopped sessions / TUI / server.
 */
export function HerdrBridge() {
  const cancelledRef = useRef(false)
  const inFlightRef = useRef(false)
  const attemptRef = useRef(0)
  const retryTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const pollTimerRef = useRef<ReturnType<typeof setInterval> | null>(null)
  const restoredFocusRef = useRef(new Map<string, string>())
  const restoringFocusRef = useRef(new Set<string>())
  const eventOwnerRef = useRef<{
    sessionName: string
    runtimeTarget: HerdrRuntimeTarget
    generation: number
    subscriptionId: string | null
    terminating: boolean
  } | null>(null)
  const eventGenerationRef = useRef(0)
  const eventDesiredRef = useRef<{ sessionName: string; runtimeTarget: HerdrRuntimeTarget } | null>(null)
  const eventRefreshTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const eventRetryTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const eventRetryAttemptRef = useRef(0)
  const eventConnectInFlightRef = useRef(false)
  /** Separate from subscribe-in-flight: a failed proxy must re-probe/schema,
   * resnapshot, then subscribe once without racing the polling loop. */
  const eventReconnectInFlightRef = useRef(false)
  const lastWorktreeInventoryRefreshRef = useRef(new Map<string, number>())
  const lastSnapshotSuccessRef = useRef(new Map<string, number>())

  useEffect(() => {
    cancelledRef.current = false
    attemptRef.current = 0
    const keyFor = (sessionName: string, runtimeTarget: HerdrRuntimeTarget) =>
      herdrSessionKey(runtimeTarget, sessionName)
    const currentSelectionMatches = (sessionName: string, runtimeTarget: HerdrRuntimeTarget) => {
      const state = useHerdrStore.getState()
      return state.selectedSessionName === sessionName &&
        sameHerdrRuntimeTarget(state.selectedRuntimeTarget, runtimeTarget)
    }
    const clearRetryTimer = () => {
      if (retryTimerRef.current !== null) {
        clearTimeout(retryTimerRef.current)
        retryTimerRef.current = null
      }
    }
    const clearEventRefreshTimer = () => {
      if (eventRefreshTimerRef.current !== null) {
        clearTimeout(eventRefreshTimerRef.current)
        eventRefreshTimerRef.current = null
      }
    }
    const clearEventRetryTimer = () => {
      if (eventRetryTimerRef.current !== null) {
        clearTimeout(eventRetryTimerRef.current)
        eventRetryTimerRef.current = null
      }
    }

    const scheduleEventDrivenRefresh = (sessionName: string, runtimeTarget: HerdrRuntimeTarget) => {
      clearEventRefreshTimer()
      eventRefreshTimerRef.current = setTimeout(() => {
        eventRefreshTimerRef.current = null
        if (cancelledRef.current || !currentSelectionMatches(sessionName, runtimeTarget)) return
        void useHerdrStore.getState().refreshSnapshot(sessionName, runtimeTarget).then((ok) => {
          if (ok) lastSnapshotSuccessRef.current.set(keyFor(sessionName, runtimeTarget), Date.now())
        })
      }, EVENT_REFRESH_DEBOUNCE_MS)
    }

    let ensureEventSubscription: (sessionName: string, runtimeTarget: HerdrRuntimeTarget) => Promise<void>
    const scheduleEventReconnect = (sessionName: string, runtimeTarget: HerdrRuntimeTarget) => {
      const desired = eventDesiredRef.current
      if (
        cancelledRef.current ||
        !desired ||
        desired.sessionName !== sessionName ||
        !sameHerdrRuntimeTarget(desired.runtimeTarget, runtimeTarget) ||
        !currentSelectionMatches(sessionName, runtimeTarget)
      ) return
      clearEventRetryTimer()
      const exp = Math.min(eventRetryAttemptRef.current, 4)
      const delay = Math.min(RETRY_MAX_MS, RETRY_BASE_MS * 2 ** exp)
      eventRetryAttemptRef.current += 1
      eventRetryTimerRef.current = setTimeout(() => {
        eventRetryTimerRef.current = null
        void (async () => {
          if (eventReconnectInFlightRef.current || !currentSelectionMatches(sessionName, runtimeTarget)) return
          eventReconnectInFlightRef.current = true
          try {
            // `bootstrap` re-runs WSL status/schema/proxy ping. It must finish
            // before the authoritative snapshot and subscription restart so an
            // old proxy generation cannot publish an identity-drifted cache.
            const state = useHerdrStore.getState()
            await state.bootstrap(sessionName, runtimeTarget)
            if (!currentSelectionMatches(sessionName, runtimeTarget)) return
            const afterProbe = useHerdrStore.getState()
            if (afterProbe.connectionState !== "ready") return
            const refreshed = await afterProbe.refreshSnapshot(sessionName, runtimeTarget)
            if (!refreshed || !currentSelectionMatches(sessionName, runtimeTarget)) return
            lastSnapshotSuccessRef.current.set(keyFor(sessionName, runtimeTarget), Date.now())
            await ensureEventSubscription(sessionName, runtimeTarget)
          } finally {
            eventReconnectInFlightRef.current = false
          }
        })()
      }, delay)
    }

    const releaseEventSubscription = async () => {
      const owner = eventOwnerRef.current
      if (owner) {
        owner.terminating = true
        if (eventGenerationRef.current === owner.generation) eventGenerationRef.current += 1
        eventOwnerRef.current = null
        useHerdrStore.getState().setEventsHealth(
          owner.sessionName,
          false,
          null,
          owner.runtimeTarget
        )
      }
      if (owner?.subscriptionId) {
        await herdrEventsRelease(owner.subscriptionId, owner.runtimeTarget).catch(() => undefined)
      }
    }

    ensureEventSubscription = async (sessionName, runtimeTarget) => {
      eventDesiredRef.current = { sessionName, runtimeTarget }
      if (cancelledRef.current || eventConnectInFlightRef.current) return
      const state = useHerdrStore.getState()
      if (!currentSelectionMatches(sessionName, runtimeTarget)) return
      const runtime = state.runtimesBySession[keyFor(sessionName, runtimeTarget)] ??
        (runtimeTarget.kind === "native" ? state.runtimesBySession[sessionName] : undefined)
      const caps = runtime?.capabilities
      if (!caps?.api.eventsSubscribe || caps.events.status !== "available" || !caps.server.running) {
        if (eventOwnerRef.current) await releaseEventSubscription()
        state.setEventsHealth(sessionName, false, null, runtimeTarget)
        return
      }
      if (
        eventOwnerRef.current?.sessionName === sessionName &&
        sameHerdrRuntimeTarget(eventOwnerRef.current.runtimeTarget, runtimeTarget) &&
        !eventOwnerRef.current.terminating
      ) return

      eventConnectInFlightRef.current = true
      if (eventOwnerRef.current) await releaseEventSubscription()
      const owner = {
        sessionName,
        runtimeTarget,
        generation: ++eventGenerationRef.current,
        subscriptionId: null as string | null,
        terminating: false
      }
      eventOwnerRef.current = owner
      state.setEventsHealth(sessionName, false, null, runtimeTarget)
      try {
        const subscriptionId = await herdrEventsSubscribe({
          runtimeTarget,
          sessionName,
          onEvent: (event) => {
            const desired = eventDesiredRef.current
            if (
              cancelledRef.current || owner.terminating ||
              owner.generation !== eventGenerationRef.current ||
              eventOwnerRef.current !== owner ||
              !desired || desired.sessionName !== sessionName ||
              !sameHerdrRuntimeTarget(desired.runtimeTarget, runtimeTarget) ||
              !currentSelectionMatches(sessionName, runtimeTarget)
            ) return
            if (event.type === "subscribed") {
              owner.subscriptionId = event.subscriptionId
              eventRetryAttemptRef.current = 0
              clearEventRetryTimer()
              useHerdrStore.getState().applySubscriptionEvent(sessionName, event, runtimeTarget)
              return
            }
            if (!owner.subscriptionId || event.subscriptionId !== owner.subscriptionId) return
            useHerdrStore.getState().applySubscriptionEvent(sessionName, event, runtimeTarget)
            if (
              event.type === "agent_status_changed" || event.type === "pane_exited" ||
              event.type === "worktree_changed" || event.type === "topology_changed"
            ) {
              if (event.type === "pane_exited" || event.type === "topology_changed") {
                useHerdrStore.getState().bumpTopologyRevision()
              }
              scheduleEventDrivenRefresh(sessionName, runtimeTarget)
              return
            }
            if (event.type === "disconnected" || event.type === "error") {
              owner.terminating = true
              if (eventOwnerRef.current === owner) eventOwnerRef.current = null
              scheduleEventDrivenRefresh(sessionName, runtimeTarget)
              void herdrEventsRelease(event.subscriptionId, runtimeTarget)
                .catch(() => undefined)
                .finally(() => scheduleEventReconnect(sessionName, runtimeTarget))
            }
          }
        })
        const desired = eventDesiredRef.current
        const stillOwner = !cancelledRef.current && !owner.terminating &&
          eventOwnerRef.current === owner && desired?.sessionName === sessionName &&
          Boolean(desired && sameHerdrRuntimeTarget(desired.runtimeTarget, runtimeTarget)) &&
          currentSelectionMatches(sessionName, runtimeTarget)
        if (!stillOwner || (owner.subscriptionId && owner.subscriptionId !== subscriptionId)) {
          owner.terminating = true
          if (eventOwnerRef.current === owner) eventOwnerRef.current = null
          await herdrEventsRelease(subscriptionId, runtimeTarget).catch(() => undefined)
          return
        }
        owner.subscriptionId = subscriptionId
        eventRetryAttemptRef.current = 0
        clearEventRetryTimer()
        useHerdrStore.getState().setEventsHealth(sessionName, true, subscriptionId, runtimeTarget)
      } catch {
        owner.terminating = true
        if (eventOwnerRef.current === owner) eventOwnerRef.current = null
        useHerdrStore.getState().setEventsHealth(sessionName, false, null, runtimeTarget)
        scheduleEventDrivenRefresh(sessionName, runtimeTarget)
        scheduleEventReconnect(sessionName, runtimeTarget)
      } finally {
        eventConnectInFlightRef.current = false
        const desired = eventDesiredRef.current
        if (desired &&
          (desired.sessionName !== sessionName || !sameHerdrRuntimeTarget(desired.runtimeTarget, runtimeTarget)) &&
          currentSelectionMatches(desired.sessionName, desired.runtimeTarget)
        ) void ensureEventSubscription(desired.sessionName, desired.runtimeTarget)
      }
    }

    const scheduleRetry = () => {
      if (cancelledRef.current) return
      clearRetryTimer()
      const exp = Math.min(attemptRef.current, 4)
      const delay = Math.min(RETRY_MAX_MS, RETRY_BASE_MS * 2 ** exp)
      attemptRef.current += 1
      retryTimerRef.current = setTimeout(() => {
        retryTimerRef.current = null
        void ensureConnected()
      }, delay)
    }

    const maybeRestoreFocusedView = async (sessionName: string, runtimeTarget: HerdrRuntimeTarget) => {
      const runtimeSessionKey = keyFor(sessionName, runtimeTarget)
      if (cancelledRef.current || !currentSelectionMatches(sessionName, runtimeTarget)) return
      if (!useWorkspaceStore.getState().sessionRestoreReady) return
      const runtime = useHerdrStore.getState().runtimesBySession[runtimeSessionKey] ??
        (runtimeTarget.kind === "native"
          ? useHerdrStore.getState().runtimesBySession[sessionName]
          : undefined)
      const snapshot = runtime?.snapshot
      if (!snapshot?.focusedWorkspaceId || !snapshot.focusedTabId) return
      const focusKey = `${snapshot.focusedWorkspaceId}:${snapshot.focusedTabId}`
      if (restoredFocusRef.current.get(runtimeSessionKey) === focusKey) return
      if (restoringFocusRef.current.has(runtimeSessionKey)) return
      restoringFocusRef.current.add(runtimeSessionKey)
      try {
        const result = await useHerdrStore.getState().restoreFocusedState(sessionName, runtimeTarget)
        if (result.ok) restoredFocusRef.current.set(runtimeSessionKey, focusKey)
      } finally {
        restoringFocusRef.current.delete(runtimeSessionKey)
        const latest = useHerdrStore.getState().runtimesBySession[runtimeSessionKey] ??
          (runtimeTarget.kind === "native"
            ? useHerdrStore.getState().runtimesBySession[sessionName]
            : undefined)
        const latestKey = latest?.snapshot?.focusedWorkspaceId && latest.snapshot.focusedTabId
          ? `${latest.snapshot.focusedWorkspaceId}:${latest.snapshot.focusedTabId}`
          : null
        if (latestKey && latestKey !== focusKey && currentSelectionMatches(sessionName, runtimeTarget)) {
          void maybeRestoreFocusedView(sessionName, runtimeTarget)
        }
      }
    }

    const ensureConnected = async () => {
      if (cancelledRef.current || inFlightRef.current) return
      inFlightRef.current = true
      try {
        const before = useHerdrStore.getState()
        await before.refreshSessions(before.selectedRuntimeTarget)
        if (cancelledRef.current) return
        const state = useHerdrStore.getState()
        const selected = state.selectedSession()
        const runtimeTarget = state.selectedRuntimeTarget
        if (!selected) return
        eventDesiredRef.current = selected.running ? { sessionName: selected.name, runtimeTarget } : null
        if (!selected.running) {
          clearEventRetryTimer()
          await releaseEventSubscription()
          await state.selectSession(selected.name, runtimeTarget)
          attemptRef.current = 0
          return
        }
        if (state.connectionState === "ready") {
          attemptRef.current = 0
          await state.refreshSnapshot(selected.name, runtimeTarget)
          if (!currentSelectionMatches(selected.name, runtimeTarget)) return
          await ensureEventSubscription(selected.name, runtimeTarget)
          await maybeRestoreFocusedView(selected.name, runtimeTarget)
          if (!cancelledRef.current && useHerdrStore.getState().connectionState === "error") scheduleRetry()
          return
        }
        await state.bootstrap(selected.name, runtimeTarget)
        if (cancelledRef.current || !currentSelectionMatches(selected.name, runtimeTarget)) return
        const next = useHerdrStore.getState().connectionState
        if (next === "ready" || next === "stopped") {
          attemptRef.current = 0
          if (next === "ready") {
            await ensureEventSubscription(selected.name, runtimeTarget)
            await maybeRestoreFocusedView(selected.name, runtimeTarget)
          } else await releaseEventSubscription()
          return
        }
        if (next === "error") scheduleRetry()
      } finally {
        inFlightRef.current = false
      }
    }

    void ensureConnected()
    const unsubscribeWorkspaceRestore = useWorkspaceStore.subscribe((state, previous) => {
      if (!state.sessionRestoreReady || previous.sessionRestoreReady) return
      const herdr = useHerdrStore.getState()
      if (herdr.selectedSessionName) {
        void maybeRestoreFocusedView(herdr.selectedSessionName, herdr.selectedRuntimeTarget)
      }
    })
    const unsubscribeHerdrFocus = useHerdrStore.subscribe((state, previous) => {
      const sessionName = state.selectedSessionName
      const runtimeTarget = state.selectedRuntimeTarget
      if (
        sessionName !== previous.selectedSessionName ||
        !sameHerdrRuntimeTarget(runtimeTarget, previous.selectedRuntimeTarget)
      ) {
        eventDesiredRef.current = sessionName ? { sessionName, runtimeTarget } : null
        eventRetryAttemptRef.current = 0
        clearEventRetryTimer()
        if (sessionName) state.setEventsHealth(sessionName, false, null, runtimeTarget)
        void releaseEventSubscription().then(() => {
          if (sessionName) void ensureEventSubscription(sessionName, runtimeTarget)
        })
      }
      if (!sessionName) return
      const key = keyFor(sessionName, runtimeTarget)
      const snapshot = state.runtimesBySession[key]?.snapshot ??
        (runtimeTarget.kind === "native" ? state.runtimesBySession[sessionName]?.snapshot : null)
      const previousSnapshot = previous.runtimesBySession[key]?.snapshot ??
        (runtimeTarget.kind === "native" ? previous.runtimesBySession[sessionName]?.snapshot : null)
      if (sessionName !== previous.selectedSessionName ||
        !sameHerdrRuntimeTarget(runtimeTarget, previous.selectedRuntimeTarget) || snapshot !== previousSnapshot
      ) void maybeRestoreFocusedView(sessionName, runtimeTarget)
    })

    pollTimerRef.current = setInterval(() => {
      if (cancelledRef.current || inFlightRef.current) return
      const state = useHerdrStore.getState()
      void state.refreshSessions(state.selectedRuntimeTarget).then(() => {
        if (cancelledRef.current) return
        const latest = useHerdrStore.getState()
        const selected = latest.selectedSession()
        const runtimeTarget = latest.selectedRuntimeTarget
        if (!selected) return
        if (!selected.running) {
          eventDesiredRef.current = null
          clearEventRetryTimer()
          void releaseEventSubscription()
          return
        }
        if (latest.connectionState === "ready") {
          const runtimeSessionKey = keyFor(selected.name, runtimeTarget)
          void ensureEventSubscription(selected.name, runtimeTarget)
          const now = Date.now()
          const lastInventoryRefresh = lastWorktreeInventoryRefreshRef.current.get(runtimeSessionKey) ?? 0
          if (shouldRefreshWorktreeInventory(latest.capabilities, now - lastInventoryRefresh, WORKTREE_INVENTORY_FALLBACK_MS)) {
            lastWorktreeInventoryRefreshRef.current.set(runtimeSessionKey, now)
            void latest.refreshWorktreeInventory(selected.name, runtimeTarget)
          }
          const lastSnapshotSuccess = lastSnapshotSuccessRef.current.get(runtimeSessionKey) ?? 0
          if (!shouldPollHerdrSnapshots(latest.capabilities, latest.eventsHealthy, now - lastSnapshotSuccess, HERDR_HEALTHY_SNAPSHOT_FALLBACK_MS)) return
          void latest.refreshSnapshot(selected.name, runtimeTarget).then((ok) => {
            if (ok) lastSnapshotSuccessRef.current.set(runtimeSessionKey, Date.now())
            return maybeRestoreFocusedView(selected.name, runtimeTarget)
          })
          return
        }
        if (["error", "unsupported", "idle", "stopped"].includes(latest.connectionState)) void ensureConnected()
      })
    }, SNAPSHOT_POLL_MS)

    return () => {
      cancelledRef.current = true
      inFlightRef.current = false
      clearRetryTimer()
      clearEventRefreshTimer()
      clearEventRetryTimer()
      eventDesiredRef.current = null
      unsubscribeWorkspaceRestore()
      unsubscribeHerdrFocus()
      if (pollTimerRef.current !== null) clearInterval(pollTimerRef.current)
      pollTimerRef.current = null
      void releaseEventSubscription()
      void useHerdrStore.getState().releaseAllAttachments().catch(() => undefined)
    }
  }, [])

  useEffect(() => {
    const reconcile = () => {
      const openPaths = new Set<string>()
      for (const group of useWorkspaceStore.getState().groups) {
        for (const tab of group.tabs) {
          if (tab.kind === "herdr-terminal" || isHerdrPagePath(tab.path)) openPaths.add(tab.path)
        }
      }
      for (const [attachmentKey, record] of useHerdrStore.getState().attachments) {
        if (!openPaths.has(record.pagePath)) void useHerdrStore.getState().releaseAttachment(attachmentKey)
      }
    }
    reconcile()
    return useWorkspaceStore.subscribe(reconcile)
  }, [])
  return null
}
