export interface UserActionLogEntry {
  event: string
  message: string
  metadata?: Record<string, unknown>
}

export type LogAction = (entry: UserActionLogEntry) => Promise<void>

export function useUserActionLog(): LogAction {
  // Stub: wired to a Tauri command in a later phase, no-op for now.
  return () => Promise.resolve()
}
