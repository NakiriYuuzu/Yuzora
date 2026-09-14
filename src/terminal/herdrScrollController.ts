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
  let inFlight = 0
  let reading = false
  let revision = 0
  let generation = 0
  let pendingDelta = 0
  const publish = (next: PaneScrollInfo | null) => {
    state = next
    options.change(next)
  }
  const refresh = async () => {
    if (disposed || inFlight > 0 || reading || !options.allowed()) return
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
  // Send each gesture immediately. Waiting for the prior RPC acknowledgement
  // made a slow bridge feel unlike native overflow scrolling during wheel and
  // track clicks. Revision guards keep stale responses from moving the thumb.
  const dispatch = (offset: number, token: number, activeGeneration: number) => {
    inFlight++
    void Promise.resolve(options.write(offset)).then((next) => {
      if (disposed || activeGeneration !== generation || !next) return
      if (token === revision) publish(next)
      else if (state) publish({
        ...next,
        offsetFromBottom: Math.min(next.maxOffsetFromBottom, state.offsetFromBottom),
      })
    }).catch((error) => {
      if (!disposed && activeGeneration === generation) options.error?.(error)
    }).finally(() => {
      inFlight--
      if (!disposed && activeGeneration !== generation && inFlight === 0) void refresh()
    })
  }
  const move = (offset: number) => {
    if (disposed || !state || !options.allowed() || !Number.isFinite(offset)) return
    const nextOffset = Math.max(0, Math.min(state.maxOffsetFromBottom, Math.round(offset)))
    revision++ // A delayed pre-gesture read cannot overwrite the requested thumb.
    publish({ ...state, offsetFromBottom: nextOffset })
    dispatch(nextOffset, revision, generation)
  }
  return {
    refresh,
    sync(next: PaneScrollInfo | null) {
      if (disposed || !options.allowed()) return
      revision++
      publish(inFlight > 0 && state && next
        ? { ...next, offsetFromBottom: Math.min(next.maxOffsetFromBottom, state.offsetFromBottom) }
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
      generation++; revision++; pendingDelta = 0; publish(null)
    },
    dispose() { disposed = true; revision++; pendingDelta = 0 }
  }
}

export type PaneScrollController = ReturnType<typeof createPaneScrollController>
