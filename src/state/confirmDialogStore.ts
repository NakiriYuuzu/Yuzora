import { create } from "zustand"

export type UnsavedDecision = "save" | "discard" | "cancel"

export interface UnsavedDecisionRequest {
    title: string
    description: string
    // Label for the save-and-continue button — "Save" for a single tab close,
    // "Save all" when switching workspace with dirty files. Discard/Cancel are
    // fixed and owned by ConfirmDialogHost.
    saveLabel: string
}

interface PendingRequest extends UnsavedDecisionRequest {
    resolve: (decision: UnsavedDecision) => void
}

interface ConfirmDialogState {
    pending: PendingRequest | null
    // Imperative gate shared by the TabBar close flow and the workspace switch
    // flow: both await the user's decision. The returned promise resolves once
    // ConfirmDialogHost's button (or Escape/overlay dismiss → "cancel") calls
    // respond().
    requestUnsavedDecision: (request: UnsavedDecisionRequest) => Promise<UnsavedDecision>
    respond: (decision: UnsavedDecision) => void
}

export const useConfirmDialogStore = create<ConfirmDialogState>((set, get) => ({
    pending: null,
    requestUnsavedDecision: (request) =>
        new Promise<UnsavedDecision>((resolve) => {
            // A still-open prior request (shouldn't happen — both call sites await
            // sequentially behind the modal) is cancelled so its promise never leaks.
            const prev = get().pending
            if (prev) prev.resolve("cancel")
            set({ pending: { ...request, resolve } })
        }),
    respond: (decision) => {
        const pending = get().pending
        if (!pending) return
        set({ pending: null })
        pending.resolve(decision)
    }
}))
