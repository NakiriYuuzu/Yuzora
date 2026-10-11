import { create } from "zustand"

export type HerdrTask = "worktree" | "startAgent" | "messageAgent" | "movePane" | "sessions" | "integrations" | "plugins"
export interface HerdrToolsSelection {
  /** Omitted = launcher home. */
  task?: HerdrTask
  /** Runtime scope; tools never fall back to the local default Session. */
  sessionName: string
  workspaceId?: string
  paneId?: string
}
export const useHerdrToolsStore = create<{
  selection: HerdrToolsSelection | null
  /** Session the sidebar scope filter targets (null = follow the selected Session); the palette reads it so both open tools on the same Session. */
  sidebarScope: string | null
  open: (selection: HerdrToolsSelection) => void
  close: () => void
  setSidebarScope: (name: string | null) => void
}>(set => ({
  selection: null,
  sidebarScope: null,
  open: selection => set({ selection }),
  close: () => set({ selection: null }),
  setSidebarScope: sidebarScope => set({ sidebarScope }),
}))
