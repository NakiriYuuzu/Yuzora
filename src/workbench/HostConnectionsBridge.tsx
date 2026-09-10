import { useEffect } from "react"
import { useHostStore } from "@/state/hostStore"
import { useSshStore } from "@/state/sshStore"
import { useRuntimePreferencesStore } from "@/state/runtimePreferencesStore"

export function HostConnectionsBridge() {
  useEffect(() => {
    const reconcile = () => useHostStore.getState().reconcile()
    reconcile()
    const timer=setInterval(reconcile,4000)
    const unsubscribe=useSshStore.subscribe(reconcile)
    const unsubscribePreferences=useRuntimePreferencesStore.subscribe(reconcile)
    return () => {clearInterval(timer);unsubscribe();unsubscribePreferences()}
  },[])
  return null
}
