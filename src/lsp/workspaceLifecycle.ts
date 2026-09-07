const listeners = new Map<string, Set<(restart: boolean) => void>>()

export function subscribeWorkspaceLsp(workspace: string, listener: (restart: boolean) => void): () => void {
  const group = listeners.get(workspace) ?? new Set()
  group.add(listener)
  listeners.set(workspace, group)
  return () => { group.delete(listener); if (!group.size) listeners.delete(workspace) }
}

export function notifyWorkspaceLsp(workspace: string, restart: boolean): void {
  for (const listener of listeners.get(workspace) ?? []) listener(restart)
}
