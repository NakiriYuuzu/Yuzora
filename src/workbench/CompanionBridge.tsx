import { useEffect } from "react"
import { listen } from "@tauri-apps/api/event"
import { COMPANION_EVENT, type CompanionEvent } from "@/lib/companionIpc"
import { useCompanionStore } from "@/state/companionStore"
import { reportCompanionEvent } from "./companionAudit"

/** How often the gateway's own list of phones corrects what the events built. */
export const PEER_RECONCILE_MS = 20_000

/**
 * Folds the Companion supervisor's events into the store; pairing claims
 * reach the approval host from here. Phone actions go to Logs and notices.
 */
export function CompanionBridge() {
  const running = useCompanionStore(s => s.overview?.status.state === "running")
  // `session.*` events are best effort: while the gateway runs, ask it who is connected.
  useEffect(() => {
    if (!running) return
    const timer = setInterval(() => void useCompanionStore.getState().reconcilePeers(), PEER_RECONCILE_MS)
    return () => clearInterval(timer)
  }, [running])
  useEffect(() => {
    const unlisten = listen<CompanionEvent>(COMPANION_EVENT, event => {
      useCompanionStore.getState().applyEvent(event.payload)
      reportCompanionEvent(event.payload)
    })
    return () => {
      void unlisten.then(fn => fn())
    }
  }, [])
  return null
}
