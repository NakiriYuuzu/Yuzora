import { invokeHerdr } from "@/lib/herdrProvider"
import type { PaneScrollInfo } from "./herdrScrollController"

export function readPaneScroll(sessionName: string, paneId: string, signal?: AbortSignal): Promise<PaneScrollInfo | null> {
  return invokeHerdr("herdr_pane_scroll_state", { sessionName, paneId }, signal)
}
export function setPaneScroll(sessionName: string, paneId: string, offsetFromBottom: number, signal?: AbortSignal): Promise<PaneScrollInfo | null> {
  if (!Number.isSafeInteger(offsetFromBottom) || offsetFromBottom < 0) return Promise.reject(new Error("invalid pane scroll offset"))
  return invokeHerdr("herdr_pane_scroll_to", { sessionName, paneId, offsetFromBottom }, signal)
}
