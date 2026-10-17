import { useEffect } from "react"
import { listen } from "@tauri-apps/api/event"
import { COMPANION_EVENT, type CompanionEvent } from "@/lib/companionIpc"
import { useCompanionStore } from "@/state/companionStore"

/** Folds the Companion supervisor's events into the store; pairing claims reach the approval host from here. */
export function CompanionBridge() {
  useEffect(() => {
    const unlisten = listen<CompanionEvent>(COMPANION_EVENT, event => {
      useCompanionStore.getState().applyEvent(event.payload)
    })
    return () => {
      void unlisten.then(fn => fn())
    }
  }, [])
  return null
}
