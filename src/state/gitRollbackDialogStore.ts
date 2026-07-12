import { create } from "zustand"

import type { GitChangeKey } from "@/workbench/git/gitChangeSelection"

export interface GitRollbackDialogRequest {
    repositoryRoot: string
    targets: GitChangeKey[]
}

interface PendingGitRollback extends GitRollbackDialogRequest {
    requestId: number
    error: string | null
    resolve: (confirmed: boolean) => void
}

interface GitRollbackDialogState {
    pending: PendingGitRollback | null
    request: (request: GitRollbackDialogRequest) => Promise<boolean>
    setError: (error: string | null) => void
    respond: (confirmed: boolean) => void
}

let requestSequence = 0

export const useGitRollbackDialogStore = create<GitRollbackDialogState>((set, get) => ({
    pending: null,
    request: (request) => new Promise<boolean>((resolve) => {
        const previous = get().pending
        if (previous) previous.resolve(false)
        set({
            pending: {
                ...request,
                requestId: ++requestSequence,
                error: null,
                resolve
            }
        })
    }),
    setError: (error) => set((state) => state.pending
        ? { pending: { ...state.pending, error } }
        : state),
    respond: (confirmed) => {
        const pending = get().pending
        if (!pending) return
        set({ pending: null })
        pending.resolve(confirmed)
    }
}))
