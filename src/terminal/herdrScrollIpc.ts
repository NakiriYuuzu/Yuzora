import { invokeHerdr } from "@/lib/herdrProvider"
import type { PaneScrollInfo } from "./herdrScrollController"
import { timeHerdrTerminalIpc } from "./herdrTerminalDiagnostics"

export function readPaneScroll(sessionName: string, paneId: string, signal?: AbortSignal): Promise<PaneScrollInfo | null> {
  return timeHerdrTerminalIpc("pane.get", () => invokeHerdr("herdr_pane_scroll_state", { sessionName, paneId }, signal))
}
export function setPaneScroll(sessionName: string, paneId: string, offsetFromBottom: number, signal?: AbortSignal): Promise<PaneScrollInfo | null> {
  if (!Number.isSafeInteger(offsetFromBottom) || offsetFromBottom < 0) return Promise.reject(new Error("invalid pane scroll offset"))
  return timeHerdrTerminalIpc("pane.scroll", () => invokeHerdr("herdr_pane_scroll_to", { sessionName, paneId, offsetFromBottom }, signal))
}

/** Absolute HERDR text point: row 0 is the oldest host-scrollback row; col is inclusive. */
export interface PaneTextPoint { row: number; col: number }
export function readPaneSelection(sessionName: string, paneId: string, anchor: PaneTextPoint, cursor: PaneTextPoint): Promise<string> {
  return invokeHerdr("herdr_pane_selection_read", { sessionName, paneId, anchor, cursor })
}
