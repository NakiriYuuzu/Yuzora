import { useCallback, useEffect, useRef, useState } from "react"
import { herdrFeature, type HerdrFeatureRequest, type HerdrFeatureResult } from "@/lib/herdrFeatures"
import { sessionScope } from "@/lib/herdrProvider"
import { useHerdrStore } from "@/state/herdrStore"
import { closeRemovedHerdrPages, activateHerdrFeatureResult } from "@/lib/herdrFeatureNavigation"

export function useHerdrOperation(sessionName: string) {
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [refreshError, setRefreshError] = useState<string | null>(null)
  const [result, setResult] = useState<HerdrFeatureResult | null>(null)
  const [feedbackScope, setFeedbackScope] = useState(sessionName)
  if (feedbackScope !== sessionName) {
    setFeedbackScope(sessionName); setError(null); setResult(null); setRefreshError(null)
  }
  const inFlight = useRef(false)
  const current = useRef(sessionName)
  useEffect(() => {
    current.current = sessionName
    return () => { current.current = "" }
  }, [sessionName])
  const run = useCallback(async (request: HerdrFeatureRequest, scope = sessionName): Promise<HerdrFeatureResult | null> => {
    if (inFlight.current) return null
    inFlight.current = true
    setBusy(true); setError(null); setResult(null); setRefreshError(null)
    try {
      const value = await herdrFeature(scope, request)
      if (current.current === sessionName) setResult(value)
      const store = useHerdrStore.getState()
      try {
        closeRemovedHerdrPages(scope, request, value)
        // A failed refresh must never turn a successful mutation into a retry.
        await store.refreshSessions()
        if (useHerdrStore.getState().sessions.some(s => sessionScope(s) === scope && s.running)) {
          await store.bootstrap(scope)
          store.bumpTopologyRevision()
          const refreshed = useHerdrStore.getState().runtimesBySession[scope]
          if (refreshed?.connectionState !== "ready") throw new Error(refreshed?.errorMessage ?? "Session refresh failed")
          if (current.current === sessionName) await activateHerdrFeatureResult(scope, request, value)
        }
      } catch (cause) {
        if (current.current === sessionName) setRefreshError(String(cause))
      }
      return value
    } catch (cause) {
      if (current.current === sessionName) setError(cause instanceof Error ? cause.message : String(cause))
      return null
    } finally {
      inFlight.current = false
      if (current.current) setBusy(false)
    }
  }, [sessionName])
  return { busy, error, refreshError, result, run }
}
export type HerdrOperation = ReturnType<typeof useHerdrOperation>
