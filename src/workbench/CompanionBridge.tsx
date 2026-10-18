import { useEffect } from "react"
import { listen } from "@tauri-apps/api/event"
import { COMPANION_EVENT, type CompanionEvent } from "@/lib/companionIpc"
import { useCompanionStore } from "@/state/companionStore"
import { reportCompanionEvent } from "./companionAudit"

/**
 * Folds the Companion supervisor's events into the store; pairing claims
 * reach the approval host from here. Phone actions go to Logs and notices.
 */
export function CompanionBridge() {
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
