/** One admitted scroll read/write per host; FIFO fairness between panes. The
 * controller owns latest-only coalescing, so this queue never contains gestures. */
export function createHerdrScrollScheduler() {
  const queue: Array<{ run: () => Promise<unknown>; resolve: (value: unknown) => void; reject: (reason: unknown) => void; signal?: AbortSignal; remove?: () => void }> = []
  let active = false
  const drain = () => {
    if (active) return
    const item = queue.shift()
    if (!item) return
    item.remove?.()
    if (item.signal?.aborted) { item.reject(new Error('scroll-cancelled')); drain(); return }
    active = true
    void Promise.resolve().then(item.run).then(item.resolve, item.reject).finally(() => { active = false; drain() })
  }
  return {
    get depth() { return queue.length + Number(active) },
    run<T>(run: () => Promise<T>, signal?: AbortSignal): Promise<T> {
      if (signal?.aborted) return Promise.reject(new Error('scroll-cancelled'))
      if (queue.length >= 32) return Promise.reject(new Error('host-request-limit'))
      return new Promise<T>((resolve, reject) => {
        const item = { run, resolve: resolve as (value: unknown) => void, reject, signal, remove: () => signal?.removeEventListener('abort', abort) }
        const abort = () => {
          const index = queue.indexOf(item)
          if (index < 0) return
          queue.splice(index, 1); item.remove(); reject(new Error('scroll-cancelled'))
        }
        signal?.addEventListener('abort', abort, { once: true })
        queue.push(item); drain()
      })
    }
  }
}
