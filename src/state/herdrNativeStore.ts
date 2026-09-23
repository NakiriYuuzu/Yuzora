import { create } from "zustand"
import type { HerdrFeatureRequest } from "@/lib/herdrFeatures"

export interface HerdrNativeSelection {
  sessionName: string
  paneId?: string
  request?: HerdrFeatureRequest
}
export const useHerdrNativeStore = create<{
  selection: HerdrNativeSelection | null
  open: (selection: HerdrNativeSelection) => void
  close: () => void
}>(set => ({ selection: null, open: selection => set({ selection }), close: () => set({ selection: null }) }))
