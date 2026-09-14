/** The viewport belongs to HERDR. xterm only renders its current ANSI frame. */
export interface PaneScrollInfo {
  offsetFromBottom: number
  maxOffsetFromBottom: number
  viewportRows: number
}

/** The proxy is a coordinate surface only. Its extent comes from server rows. */
export function scrollProxyContentHeight(state: PaneScrollInfo, viewportHeight: number) {
  return viewportHeight * (1 + state.maxOffsetFromBottom / state.viewportRows)
}
export function offsetFromProxyScroll(state: PaneScrollInfo, scrollTop: number, scrollRange: number) {
  if (scrollRange <= 0) return 0
  return Math.round(state.maxOffsetFromBottom * (1 - Math.min(1, Math.max(0, scrollTop / scrollRange))))
}
export function proxyScrollTop(state: PaneScrollInfo, scrollRange: number) {
  return state.maxOffsetFromBottom ? scrollRange * (1 - state.offsetFromBottom / state.maxOffsetFromBottom) : 0
}

export function createPaneScrollController(options: {
  read: () => Promise<PaneScrollInfo | null>
  write: (offset: number) => Promise<PaneScrollInfo | null>
  allowed: () => boolean
  change: (state: PaneScrollInfo | null) => void
  error?: (error: unknown) => void
}) {
  let state: PaneScrollInfo | null = null
  let disposed = false
  let writing = false
  let reading = false
  let revision = 0
  let generation = 0
  let pending: number | null = null
  let pendingDelta = 0
  const publish = (next: PaneScrollInfo | null) => {
    state = next
    options.change(next)
  }
  const refresh = async () => {
    if (disposed || writing || reading || !options.allowed()) return
    reading = true
    const token = revision
    try {
      const next = await options.read()
      if (!disposed && token === revision) {
        publish(next)
        const delta = pendingDelta
        pendingDelta = 0
        if (next && delta !== 0) move(next.offsetFromBottom - delta)
      }
    } catch (error) {
      if (!disposed && token === revision) {
        pendingDelta = 0
        publish(null)
        options.error?.(error)
      }
    } finally { reading = false }
  }
  const drain = async () => {
    writing = true
    const activeGeneration = generation
    try {
      while (!disposed && activeGeneration === generation && options.allowed() && pending !== null) {
        const offset = pending
        pending = null
        const token = revision
        const next = await options.write(offset)
        if (disposed || activeGeneration !== generation) return
        if (token === revision) publish(next)
        else if (next && pending !== null && state) {
          // Acknowledgements carry fresh overflow even when a newer wheel or
          // drag has superseded their offset. Keep that latest target.
          pending = Math.min(next.maxOffsetFromBottom, pending)
          publish({ ...next, offsetFromBottom: pending })
        }
      }
    } catch (error) {
      if (!disposed && activeGeneration === generation) { publish(null); options.error?.(error) }
    } finally {
      pending = null
      writing = false
      if (!disposed && activeGeneration !== generation) void refresh()
    }
  }
  const move = (offset: number) => {
    if (disposed || !state || !options.allowed() || !Number.isFinite(offset)) return
    pending = Math.max(0, Math.min(state.maxOffsetFromBottom, Math.round(offset)))
    revision++ // A delayed pre-gesture read cannot overwrite the requested thumb.
    publish({ ...state, offsetFromBottom: pending })
    if (!writing) void drain()
  }
  return {
    refresh,
    sync(next: PaneScrollInfo | null) {
      if (disposed || !options.allowed()) return
      revision++
      publish(writing && state && next
        ? { ...next, offsetFromBottom: Math.min(next.maxOffsetFromBottom, pending ?? state.offsetFromBottom) }
        : next)
    },
    move,
    scroll(delta: number) {
      if (disposed || !options.allowed() || !Number.isFinite(delta) || !Math.trunc(delta)) return
      if (state) move(state.offsetFromBottom - Math.trunc(delta))
      else {
        // Only initial hydration waits for pane.get. Subsequent gestures use
        // the same position as dragging, independent of remote latency.
        pendingDelta += Math.trunc(delta)
        void refresh()
      }
    },
    reset() {
      if (disposed) return
      generation++; revision++; pending = null; pendingDelta = 0; publish(null)
    },
    dispose() { disposed = true; revision++; pending = null; pendingDelta = 0 }
  }
}

export type PaneScrollController = ReturnType<typeof createPaneScrollController>
