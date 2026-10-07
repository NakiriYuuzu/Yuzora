import { useEffect } from "react"
import { useHostStore } from "@/state/hostStore"
import { useSshStore } from "@/state/sshStore"
import { useRuntimePreferencesStore } from "@/state/runtimePreferencesStore"
import { startHerdrVisibilityPolling } from "./herdrBridgePolicy"

export function HostConnectionsBridge() {
  useEffect(() => {
    const reconcile = () => useHostStore.getState().reconcile()
    reconcile()
    const stopPolling = startHerdrVisibilityPolling(reconcile)
    const unsubscribe=useSshStore.subscribe((state, previous) => {
      if (state.hosts !== previous.hosts || state.sessions !== previous.sessions) reconcile()
    })
    const unsubscribePreferences=useRuntimePreferencesStore.subscribe((state, previous) => {
      if (state.wslEnabled !== previous.wslEnabled) reconcile()
    })
    return () => {stopPolling();unsubscribe();unsubscribePreferences()}
  },[])
  return null
}
