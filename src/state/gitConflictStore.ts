import { create } from "zustand"

/** Open state of the JetBrains-style Conflicts dialog and three-pane merge tool. */
interface GitConflictUiState {
    conflictsOpen: boolean
    /** Repository-relative path open in the merge tool. */
    mergePath: string | null
    openConflicts: () => void
    closeConflicts: () => void
    openMerge: (path: string) => void
    closeMerge: () => void
}

export const useGitConflictStore = create<GitConflictUiState>((set) => ({
    conflictsOpen: false,
    mergePath: null,
    openConflicts: () => set({ conflictsOpen: true }),
    closeConflicts: () => set({ conflictsOpen: false }),
    openMerge: (path) => set({ mergePath: path }),
    closeMerge: () => set({ mergePath: null })
}))
