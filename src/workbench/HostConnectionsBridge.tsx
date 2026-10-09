import { useEffect } from "react"
import { useHostStore } from "@/state/hostStore"
import { useSshStore } from "@/state/sshStore"
import { useRuntimePreferencesStore } from "@/state/runtimePreferencesStore"
import { startHerdrVisibilityPolling } from "./herdrBridgePolicy"

export function HostConnectionsBridge() {
  useEffect(() => {
    let disposed = false
    let teardown: (() => void) | null = null
    // The WSL preference lives in Rust; until hydrated it reads as disabled, so wait before the first reconcile.
    const start = () => {
      if (disposed) return
      const reconcile = () => useHostStore.getState().reconcile()
      reconcile()
      const stopPolling = startHerdrVisibilityPolling(reconcile)
      const unsubscribe=useSshStore.subscribe((state, previous) => {
        if (state.hosts !== previous.hosts || state.sessions !== previous.sessions) reconcile()
      })
      const unsubscribePreferences=useRuntimePreferencesStore.subscribe((state, previous) => {
        if (state.wslEnabled !== previous.wslEnabled) reconcile()
      })
      teardown = () => {stopPolling();unsubscribe();unsubscribePreferences()}
    }
    const preferences = useRuntimePreferencesStore.getState()
    if (preferences.hydrated) start()
    else void preferences.hydrate().then(start)
    return () => {disposed = true; teardown?.()}
  },[])
  return null
}
