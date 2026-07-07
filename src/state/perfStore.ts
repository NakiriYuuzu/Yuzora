import { create } from "zustand"

import type { PerfSnapshot } from "../lib/types"

interface PerfState {
    // Latest sample from perf_snapshot, or null before the first poll / when the
    // backend returns none. StatusBar hides the chip while null.
    snapshot: PerfSnapshot | null
    setSnapshot: (snapshot: PerfSnapshot | null) => void
    reset: () => void
}

export const perfInitialState = {
    snapshot: null as PerfSnapshot | null
}

export const usePerfStore = create<PerfState>()((set) => ({
    ...perfInitialState,

    setSnapshot: (snapshot) => set({ snapshot }),

    reset: () => set(perfInitialState)
}))
