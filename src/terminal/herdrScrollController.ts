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
}) {
  let state: PaneScrollInfo | null = null
  let disposed = false
  let writing = false
  let reading = false
  let revision = 0
  let pending: number | null = null
  const publish = (next: PaneScrollInfo | null) => {
    state = next
    options.change(next)
  }
  const refresh = async () => {
    if (disposed || writing || reading) return
    reading = true
    const token = revision
    try {
      const next = await options.read()
      if (!disposed && token === revision) publish(next)
    } catch {
      if (!disposed && token === revision) publish(null)
    } finally { reading = false }
  }
  const drain = async () => {
    writing = true
    try {
      while (!disposed && options.allowed() && pending !== null) {
        const offset = pending
        pending = null
        const next = await options.write(offset)
        if (!disposed && pending === null) publish(next)
      }
    } catch {
      if (!disposed) publish(null)
    } finally { pending = null; writing = false }
  }
  return {
    refresh,
    move(offset: number) {
      if (disposed || !state || !options.allowed() || !Number.isFinite(offset)) return
      pending = Math.max(0, Math.min(state.maxOffsetFromBottom, Math.round(offset)))
      revision++ // A delayed pre-drag read cannot overwrite the requested thumb.
      publish({ ...state, offsetFromBottom: pending })
      if (!writing) void drain()
    },
    dispose() { disposed = true; revision++; pending = null }
  }
}
