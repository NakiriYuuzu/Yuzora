import { create } from "zustand"

export type AppDialogKind = "info" | "warning" | "error"

interface ConfirmDialogRequest {
  title: string
  description: string
  confirmLabel?: string
  cancelLabel?: string
  kind?: AppDialogKind
  destructive?: boolean
}

interface MessageDialogRequest {
  title: string
  description: string
  confirmLabel?: string
  kind?: AppDialogKind
}

type PendingAppDialog =
  | ({ type: "confirm" } & ConfirmDialogRequest & { resolve: (value: boolean) => void })
  | ({ type: "message" } & MessageDialogRequest & { resolve: () => void })

interface AppDialogState {
  pending: PendingAppDialog | null
  confirm: (request: ConfirmDialogRequest) => Promise<boolean>
  message: (request: MessageDialogRequest) => Promise<void>
  respond: (accepted: boolean) => void
}

export const useAppDialogStore = create<AppDialogState>((set, get) => ({
  pending: null,
  confirm: (request) =>
    new Promise<boolean>((resolve) => {
      const previous = get().pending
      if (previous?.type === "confirm") previous.resolve(false)
      if (previous?.type === "message") previous.resolve()
      set({ pending: { type: "confirm", ...request, resolve } })
    }),
  message: (request) =>
    new Promise<void>((resolve) => {
      const previous = get().pending
      if (previous?.type === "confirm") previous.resolve(false)
      if (previous?.type === "message") previous.resolve()
      set({ pending: { type: "message", ...request, resolve } })
    }),
  respond: (accepted) => {
    const pending = get().pending
    if (!pending) return
    set({ pending: null })
    if (pending.type === "confirm") pending.resolve(accepted)
    else pending.resolve()
  }
}))

export function requestAppConfirmation(request: ConfirmDialogRequest): Promise<boolean> {
  return useAppDialogStore.getState().confirm(request)
}

export function showAppMessage(request: MessageDialogRequest): Promise<void> {
  return useAppDialogStore.getState().message(request)
}
