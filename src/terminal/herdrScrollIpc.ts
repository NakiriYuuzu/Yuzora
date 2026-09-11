import { invokeHerdr } from "@/lib/herdrProvider"
import type { PaneScrollInfo } from "./herdrScrollController"

export function readPaneScroll(sessionName: string, paneId: string): Promise<PaneScrollInfo | null> {
  return invokeHerdr("herdr_pane_scroll_state", { sessionName, paneId })
}
export function setPaneScroll(sessionName: string, paneId: string, offsetFromBottom: number): Promise<PaneScrollInfo | null> {
  if (!Number.isSafeInteger(offsetFromBottom) || offsetFromBottom < 0) return Promise.reject(new Error("invalid pane scroll offset"))
  return invokeHerdr("herdr_pane_scroll_to", { sessionName, paneId, offsetFromBottom })
}
