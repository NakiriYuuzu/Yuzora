const queues = new Map<string, Promise<void>>()

/**
 * Runs one session's drag-initiated layout mutations (and their refreshes) in
 * the order they were asked for: concurrent requests to a slow or remote
 * runtime could reach the server reordered and leave a different layout.
 */
export function queueHerdrMutation<T>(sessionName: string, run: () => Promise<T>): Promise<T> {
  const result = (queues.get(sessionName) ?? Promise.resolve()).then(run)
  const settled = result.then(() => undefined, () => undefined)
  queues.set(sessionName, settled)
  void settled.then(() => {
    if (queues.get(sessionName) === settled) queues.delete(sessionName)
  })
  return result
}
