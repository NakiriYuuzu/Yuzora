import { create } from "zustand"

interface TextInputDialogRequest {
    title: string
    description?: string
    label: string
    initialValue?: string
    placeholder?: string
    confirmLabel: string
}

interface PendingTextInputDialog extends TextInputDialogRequest {
    requestId: number
    resolve: (value: string | null) => void
}

let nextTextInputDialogRequestId = 0

interface TextInputDialogState {
    pending: PendingTextInputDialog | null
    request: (request: TextInputDialogRequest) => Promise<string | null>
    respond: (value: string | null) => void
}

export const useTextInputDialogStore = create<TextInputDialogState>((set, get) => ({
    pending: null,
    request: (request) =>
        new Promise<string | null>((resolve) => {
            const previous = get().pending
            if (previous) previous.resolve(null)
            nextTextInputDialogRequestId += 1
            set({ pending: { ...request, requestId: nextTextInputDialogRequestId, resolve } })
        }),
    respond: (value) => {
        const pending = get().pending
        if (!pending) return
        set({ pending: null })
        pending.resolve(value)
    }
}))

export function requestTextInputDialog(
    request: TextInputDialogRequest
): Promise<string | null> {
    return useTextInputDialogStore.getState().request(request)
}
