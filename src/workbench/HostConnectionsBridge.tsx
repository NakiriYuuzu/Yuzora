import { useEffect } from "react"
import { useHostStore } from "@/state/hostStore"
import { useSshStore } from "@/state/sshStore"

export function HostConnectionsBridge() {
  useEffect(() => {
    const reconcile = () => useHostStore.getState().reconcile()
    reconcile()
    const timer=setInterval(reconcile,4000)
    const unsubscribe=useSshStore.subscribe(reconcile)
    return () => {clearInterval(timer);unsubscribe()}
  },[])
  return null
}
