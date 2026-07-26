import { create } from "zustand"

export type UnsavedDecision = "save" | "discard" | "cancel"

interface UnsavedDecisionRequest {
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
    // Imperative gate shared by every flow that can discard unsaved work: the
    // TabBar close flow (TabBar.tsx) and confirmDiscardingUnsaved
    // (lib/unsavedGuard.ts), which in turn serves the workspace switch and the
    // window-close guard (issue #21). All await the user's decision; the
    // returned promise resolves once ConfirmDialogHost's button (or
    // Escape/overlay dismiss → "cancel") calls respond().
    requestUnsavedDecision: (request: UnsavedDecisionRequest) => Promise<UnsavedDecision>
    respond: (decision: UnsavedDecision) => void
}

export const useConfirmDialogStore = create<ConfirmDialogState>((set, get) => ({
    pending: null,
    requestUnsavedDecision: (request) =>
        new Promise<UnsavedDecision>((resolve) => {
            // A still-open prior request is cancelled so its promise never leaks.
            // The in-app call sites await sequentially behind the modal, but the
            // native window-close button / Alt+F4 is outside the webview and can
            // fire again while the modal is up (issue #21): the superseded close
            // handler then sees "cancel" and prevents its own close, which is the
            // safe outcome — the newest dialog owns the decision.
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
