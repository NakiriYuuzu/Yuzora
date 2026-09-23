import { create } from "zustand"

export type HerdrTool = "worktrees" | "agents" | "panes" | "sessions" | "integrations" | "plugins" | "notifications"
export interface HerdrToolsSelection {
  tool: HerdrTool
  /** Runtime scope; tools never fall back to the local default Session. */
  sessionName: string
  workspaceId?: string
  paneId?: string
}
export const useHerdrToolsStore = create<{
  selection: HerdrToolsSelection | null
  open: (selection: HerdrToolsSelection) => void
  close: () => void
}>(set => ({ selection: null, open: selection => set({ selection }), close: () => set({ selection: null }) }))
