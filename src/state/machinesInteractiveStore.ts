import { create } from "zustand"
import type { HerdrMachineInteractiveSpec } from "@/lib/machinesTypes"

export interface MachineInteractiveSelection {
  spec: HerdrMachineInteractiveSpec
  /** Machine label for the client hint ("select <label> in the sidebar"). */
  machineLabel?: string
}

interface MachinesInteractiveState {
  selection: MachineInteractiveSelection | null
  /** Only one interactive dialog at a time; returns false when one is already open. */
  open: (selection: MachineInteractiveSelection) => boolean
  close: () => void
}

export const useMachinesInteractiveStore = create<MachinesInteractiveState>((set, get) => ({
  selection: null,
  open: (selection) => {
    if (get().selection) return false
    set({ selection })
    return true
  },
  close: () => set({ selection: null })
}))
