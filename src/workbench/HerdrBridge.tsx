import { useEffect, useRef } from "react"

import { herdrEventsRelease, herdrEventsSubscribe } from "@/lib/herdrIpc"
import { isHerdrPagePath } from "@/lib/herdrPages"
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
 * - Bootstraps/polls snapshot only for the selected running session.
 * - Owns one events.subscribe stream per selected running session when available.
 * - Recovers from transient failures with bounded backoff.
 * - Reconciles attachments against open herdr-terminal pages.
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
    generation: number
    subscriptionId: string | null
    terminating: boolean
  } | null>(null)
  const eventGenerationRef = useRef(0)
  const eventDesiredSessionRef = useRef<string | null>(null)
  const eventRefreshTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const eventRetryTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const eventRetryAttemptRef = useRef(0)
  const eventConnectInFlightRef = useRef(false)
  const lastWorktreeInventoryRefreshRef = useRef(new Map<string, number>())
  const lastSnapshotSuccessRef = useRef(new Map<string, number>())

  useEffect(() => {
    cancelledRef.current = false
    attemptRef.current = 0

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

    const scheduleEventDrivenRefresh = (sessionName: string) => {
      clearEventRefreshTimer()
      eventRefreshTimerRef.current = setTimeout(() => {
        eventRefreshTimerRef.current = null
        if (
          cancelledRef.current ||
          useHerdrStore.getState().selectedSessionName !== sessionName
        ) {
          return
        }
        void useHerdrStore.getState().refreshSnapshot(sessionName).then((ok) => {
          if (ok) lastSnapshotSuccessRef.current.set(sessionName, Date.now())
        })
      }, EVENT_REFRESH_DEBOUNCE_MS)
    }

    let ensureEventSubscription: (sessionName: string) => Promise<void>

    const scheduleEventReconnect = (sessionName: string) => {
      if (
        cancelledRef.current ||
        eventDesiredSessionRef.current !== sessionName ||
        useHerdrStore.getState().selectedSessionName !== sessionName
      ) {
        return
      }
      clearEventRetryTimer()
      const exp = Math.min(eventRetryAttemptRef.current, 4)
      const delay = Math.min(RETRY_MAX_MS, RETRY_BASE_MS * 2 ** exp)
      eventRetryAttemptRef.current += 1
      eventRetryTimerRef.current = setTimeout(() => {
        eventRetryTimerRef.current = null
        void ensureEventSubscription(sessionName)
      }, delay)
    }

    const releaseEventSubscription = async () => {
      const owner = eventOwnerRef.current
      if (owner) {
        owner.terminating = true
        if (eventGenerationRef.current === owner.generation) {
          eventGenerationRef.current += 1
        }
        eventOwnerRef.current = null
        useHerdrStore.getState().setEventsHealth(owner.sessionName, false, null)
      }
      if (owner?.subscriptionId) {
        await herdrEventsRelease(owner.subscriptionId).catch(() => undefined)
      }
    }

    ensureEventSubscription = async (sessionName: string) => {
      eventDesiredSessionRef.current = sessionName
      if (cancelledRef.current || eventConnectInFlightRef.current) return
      const state = useHerdrStore.getState()
      if (state.selectedSessionName !== sessionName) return
      const caps = state.runtimesBySession[sessionName]?.capabilities
      if (
        !caps?.api.eventsSubscribe ||
        caps.events.status !== "available" ||
        !caps.server.running
      ) {
        if (eventOwnerRef.current) await releaseEventSubscription()
        state.setEventsHealth(sessionName, false, null)
        return
      }
      if (
        eventOwnerRef.current?.sessionName === sessionName &&
        !eventOwnerRef.current.terminating
      ) {
        return
      }

      eventConnectInFlightRef.current = true
      if (eventOwnerRef.current) await releaseEventSubscription()
      const owner = {
        sessionName,
        generation: ++eventGenerationRef.current,
        subscriptionId: null as string | null,
        terminating: false
      }
      eventOwnerRef.current = owner
      state.setEventsHealth(sessionName, false, null)
      try {
        const subscriptionId = await herdrEventsSubscribe({
          sessionName,
          onEvent: (event) => {
            if (
              cancelledRef.current ||
              owner.terminating ||
              owner.generation !== eventGenerationRef.current ||
              eventOwnerRef.current !== owner ||
              eventDesiredSessionRef.current !== sessionName ||
              useHerdrStore.getState().selectedSessionName !== sessionName
            ) {
              return
            }
            if (event.type === "subscribed") {
              owner.subscriptionId = event.subscriptionId
              eventRetryAttemptRef.current = 0
              clearEventRetryTimer()
              useHerdrStore.getState().applySubscriptionEvent(sessionName, event)
              return
            }
            if (!owner.subscriptionId || event.subscriptionId !== owner.subscriptionId) return
            useHerdrStore.getState().applySubscriptionEvent(sessionName, event)
            if (
              event.type === "agent_status_changed" ||
              event.type === "pane_exited" ||
              event.type === "worktree_changed" ||
              event.type === "topology_changed"
            ) {
              if (event.type === "pane_exited" || event.type === "topology_changed") {
                useHerdrStore.getState().bumpTopologyRevision()
              }
              // Store owns worktree inventory dirty reconciliation. This bridge
              // only schedules the authoritative snapshot recovery pass.
              scheduleEventDrivenRefresh(sessionName)
              return
            }
            if (event.type === "disconnected" || event.type === "error") {
              owner.terminating = true
              if (eventOwnerRef.current === owner) eventOwnerRef.current = null
              scheduleEventDrivenRefresh(sessionName)
              void herdrEventsRelease(event.subscriptionId)
                .catch(() => undefined)
                .finally(() => scheduleEventReconnect(sessionName))
            }
          }
        })
        const stillOwner =
          !cancelledRef.current &&
          !owner.terminating &&
          eventOwnerRef.current === owner &&
          eventDesiredSessionRef.current === sessionName &&
          useHerdrStore.getState().selectedSessionName === sessionName
        if (!stillOwner || (owner.subscriptionId && owner.subscriptionId !== subscriptionId)) {
          owner.terminating = true
          if (eventOwnerRef.current === owner) eventOwnerRef.current = null
          await herdrEventsRelease(subscriptionId).catch(() => undefined)
          return
        }
        owner.subscriptionId = subscriptionId
        eventRetryAttemptRef.current = 0
        clearEventRetryTimer()
        useHerdrStore.getState().setEventsHealth(sessionName, true, subscriptionId)
      } catch {
        owner.terminating = true
        if (eventOwnerRef.current === owner) eventOwnerRef.current = null
        useHerdrStore.getState().setEventsHealth(sessionName, false, null)
        scheduleEventDrivenRefresh(sessionName)
        scheduleEventReconnect(sessionName)
      } finally {
        eventConnectInFlightRef.current = false
        const desired = eventDesiredSessionRef.current
        if (
          desired &&
          desired !== sessionName &&
          useHerdrStore.getState().selectedSessionName === desired
        ) {
          void ensureEventSubscription(desired)
        }
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

    const ensureConnected = async () => {
      if (cancelledRef.current || inFlightRef.current) return
      inFlightRef.current = true
      try {
        await useHerdrStore.getState().refreshSessions()
        if (cancelledRef.current) return

        const state = useHerdrStore.getState()
        const selected = state.selectedSession()
        if (!selected) return
        eventDesiredSessionRef.current = selected.running ? selected.name : null

        if (!selected.running) {
          clearEventRetryTimer()
          await releaseEventSubscription()
          if (state.selectedSessionName) {
            await state.selectSession(selected.name)
          }
          attemptRef.current = 0
          return
        }

        if (state.connectionState === "ready" && state.selectedSessionName === selected.name) {
          attemptRef.current = 0
          await state.refreshSnapshot(selected.name)
          if (useHerdrStore.getState().selectedSessionName !== selected.name) return
          await ensureEventSubscription(selected.name)
          await maybeRestoreFocusedView(selected.name)
          if (
            !cancelledRef.current &&
            useHerdrStore.getState().connectionState === "error"
          ) {
            scheduleRetry()
          }
          return
        }

        await state.bootstrap(selected.name)
        if (
          cancelledRef.current ||
          useHerdrStore.getState().selectedSessionName !== selected.name
        ) {
          return
        }

        const next = useHerdrStore.getState().connectionState
        if (next === "ready" || next === "stopped") {
          attemptRef.current = 0
          if (next === "ready") {
            await ensureEventSubscription(selected.name)
            await maybeRestoreFocusedView(selected.name)
          } else {
            await releaseEventSubscription()
          }
          return
        }
        if (next === "error") {
          scheduleRetry()
        }
      } finally {
        inFlightRef.current = false
      }
    }

    void ensureConnected()

    const unsubscribeWorkspaceRestore = useWorkspaceStore.subscribe((state, previous) => {
      if (!state.sessionRestoreReady || previous.sessionRestoreReady) return
      const sessionName = useHerdrStore.getState().selectedSessionName
      if (sessionName) void maybeRestoreFocusedView(sessionName)
    })
    const unsubscribeHerdrFocus = useHerdrStore.subscribe((state, previous) => {
      const sessionName = state.selectedSessionName
      if (sessionName !== previous.selectedSessionName) {
        eventDesiredSessionRef.current = sessionName
        eventRetryAttemptRef.current = 0
        clearEventRetryTimer()
        if (sessionName) state.setEventsHealth(sessionName, false, null)
        void releaseEventSubscription().then(() => {
          if (sessionName) void ensureEventSubscription(sessionName)
        })
      }
      if (!sessionName) return
      const snapshot = state.runtimesBySession[sessionName]?.snapshot
      const previousSnapshot = previous.runtimesBySession[sessionName]?.snapshot
      if (sessionName !== previous.selectedSessionName || snapshot !== previousSnapshot) {
        void maybeRestoreFocusedView(sessionName)
      }
    })

    pollTimerRef.current = setInterval(() => {
      if (cancelledRef.current || inFlightRef.current) return
      const state = useHerdrStore.getState()
      void state.refreshSessions().then(() => {
        if (cancelledRef.current) return
        const latest = useHerdrStore.getState()
        const selected = latest.selectedSession()
        if (!selected) return
        if (!selected.running) {
          eventDesiredSessionRef.current = null
          clearEventRetryTimer()
          void releaseEventSubscription()
          return
        }
        if (latest.connectionState === "ready") {
          void ensureEventSubscription(selected.name)
          const now = Date.now()
          const lastInventoryRefresh =
            lastWorktreeInventoryRefreshRef.current.get(selected.name) ?? 0
          if (
            shouldRefreshWorktreeInventory(
              latest.capabilities,
              now - lastInventoryRefresh,
              WORKTREE_INVENTORY_FALLBACK_MS
            )
          ) {
            lastWorktreeInventoryRefreshRef.current.set(selected.name, now)
            void latest.refreshWorktreeInventory(selected.name)
          }
          const lastSnapshotSuccess =
            lastSnapshotSuccessRef.current.get(selected.name) ?? 0
          if (
            !shouldPollHerdrSnapshots(
              latest.capabilities,
              latest.eventsHealthy,
              now - lastSnapshotSuccess,
              HERDR_HEALTHY_SNAPSHOT_FALLBACK_MS
            )
          ) {
            return
          }
          void latest.refreshSnapshot(selected.name).then((ok) => {
            if (ok) lastSnapshotSuccessRef.current.set(selected.name, Date.now())
            return maybeRestoreFocusedView(selected.name)
          })
          return
        }
        if (
          latest.connectionState === "error" ||
          latest.connectionState === "unsupported" ||
          latest.connectionState === "idle" ||
          latest.connectionState === "stopped"
        ) {
          void ensureConnected()
        }
      })
    }, SNAPSHOT_POLL_MS)

    return () => {
      cancelledRef.current = true
      inFlightRef.current = false
      clearRetryTimer()
      clearEventRefreshTimer()
      clearEventRetryTimer()
      eventDesiredSessionRef.current = null
      unsubscribeWorkspaceRestore()
      unsubscribeHerdrFocus()
      if (pollTimerRef.current !== null) {
        clearInterval(pollTimerRef.current)
        pollTimerRef.current = null
      }
      void releaseEventSubscription()
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
