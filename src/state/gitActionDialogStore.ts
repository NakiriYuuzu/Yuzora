import { create } from "zustand"

export interface GitResetTarget {
    hash: string
    subject: string
}

/** Open state of the Stash and Reset dialogs. */
interface GitActionDialogState {
    stashOpen: boolean
    resetTarget: GitResetTarget | null
    openStash: () => void
    closeStash: () => void
    openReset: (target: GitResetTarget) => void
    closeReset: () => void
}

export const useGitActionDialogStore = create<GitActionDialogState>((set) => ({
    stashOpen: false,
    resetTarget: null,
    openStash: () => set({ stashOpen: true }),
    closeStash: () => set({ stashOpen: false }),
    openReset: (target) => set({ resetTarget: target }),
    closeReset: () => set({ resetTarget: null })
}))
