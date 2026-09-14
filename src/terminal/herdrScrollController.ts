import { herdrErrorKind } from "@/lib/herdrErrors"
import type { HerdrScrollMetric } from "./herdrScrollTelemetry"

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
  read: (signal?: AbortSignal) => Promise<PaneScrollInfo | null>
  write: (offset: number, signal?: AbortSignal) => Promise<PaneScrollInfo | null>
  allowed: () => boolean
  change: (state: PaneScrollInfo | null) => void
  error?: (error: unknown) => void
  metric?: (metric: Omit<HerdrScrollMetric, "session" | "pane">) => void
}) {
  let state: PaneScrollInfo | null = null
  let disposed = false
  let blocked = false
  let generation = 0
  let revision = 0
  let serial = 0
  let writing = false
  let reading = false
  let pending: number | null = null
  let pendingDelta = 0
  let lastSent = -Infinity
  let attempts = 0
  let busySince: number | null = null
  let retryAt = 0
  let frameStart: number | null = null
  let timer: ReturnType<typeof setTimeout> | undefined
  let reconcile: ReturnType<typeof setTimeout> | undefined
  let abort = new AbortController()
  const metric = (phase: HerdrScrollMetric['phase'], extra = {}) => options.metric?.({ phase, serial, at: performance.now(), pending: Number(pending !== null), attempt: attempts, ...extra })
  const publish = (next: PaneScrollInfo | null) => { state = next; options.change(next) }
  const clearTimers = () => { clearTimeout(timer); clearTimeout(reconcile); timer = undefined; reconcile = undefined }
  const current = (g: number) => !disposed && g === generation && options.allowed()
  const fail = (error: unknown) => {
    const kind = herdrErrorKind(error)
    metric('error', { errorClass: kind })
    pending = null; pendingDelta = 0; busySince = null; attempts = 0
    if (kind === 'unsupported' || kind === 'permission-denied') { blocked = true; publish(null) }
    if (kind === 'disconnected') { blocked = true; generation++; abort.abort(); abort = new AbortController(); publish(null) }
    options.error?.(error)
  }
  const refresh = async () => {
    if (disposed || blocked || writing || reading || pending !== null || !options.allowed()) return
    clearTimeout(reconcile); reconcile = undefined
    reading = true
    const g = generation, token = revision
    try {
      const next = await options.read(abort.signal)
      if (current(g) && token === revision) {
        busySince = null; attempts = 0; retryAt = 0
        publish(next)
        const delta = pendingDelta; pendingDelta = 0
        if (next && delta) move(next.offsetFromBottom - delta)
      }
    } catch (error) {
      if (current(g) && token === revision) {
        if (herdrErrorKind(error) === 'busy') {
          busySince ??= performance.now()
          if (performance.now() - busySince < 750) {
            reconcile = setTimeout(() => { reconcile = undefined; void refresh() }, Math.min(128, 16 * 2 ** attempts++))
          } else fail(error)
        } else fail(error)
      }
    } finally {
      reading = false
      if (!disposed && g !== generation) void refresh()
    }
  }
  const scheduleReconcile = () => {
    clearTimeout(reconcile)
    reconcile = setTimeout(() => { reconcile = undefined; void refresh() }, 150)
  }
  const drain = () => {
    if (disposed || blocked || writing || pending === null || !options.allowed()) return
    clearTimeout(timer); timer = undefined
    const delay = Math.max(lastSent + 33, retryAt) - performance.now()
    if (delay > 0) { timer = setTimeout(drain, delay); return }
    const offset = pending, token = revision, g = generation
    pending = null; writing = true; serial++; lastSent = performance.now(); frameStart = lastSent
    const start = lastSent
    metric('dispatch')
    // A synchronous adapter throw must settle the same state machine too.
    let result: Promise<PaneScrollInfo | null>
    try { result = options.write(offset, abort.signal) } catch (error) { result = Promise.reject(error) }
    void result.then(next => {
      if (!current(g)) return
      busySince = null; attempts = 0; retryAt = 0
      metric('ack', { elapsedMs: performance.now() - start })
      if (token === revision && pending === null) publish(next)
      else if (next && state) {
        // A newer intent owns position. Metadata may grow, but a stale reply
        // cannot shrink the range and clamp an unsent target.
        publish({ ...next, maxOffsetFromBottom: Math.max(next.maxOffsetFromBottom, state.maxOffsetFromBottom), offsetFromBottom: state.offsetFromBottom })
      }
    }).catch(error => {
      if (!current(g)) return
      if (herdrErrorKind(error) === 'busy') {
        busySince ??= start
        if (performance.now() - busySince < 750) {
          pending ??= offset
          retryAt = performance.now() + Math.min(128, 16 * 2 ** attempts++)
          return
        }
      }
      fail(error)
    }).finally(() => {
      writing = false
      if (disposed) return
      if (g !== generation) { void refresh(); return }
      if (pending !== null) drain()
      else if (!blocked) scheduleReconcile()
    })
  }
  const move = (offset: number) => {
    if (disposed || blocked || !state || !options.allowed() || !Number.isFinite(offset)) return
    const next = Math.max(0, Math.min(state.maxOffsetFromBottom, Math.round(offset)))
    if (next === state.offsetFromBottom) return
    revision++; pending = next
    clearTimeout(reconcile); reconcile = undefined
    publish({ ...state, offsetFromBottom: next }); metric('gesture'); drain()
  }
  return {
    refresh, move,
    frame() {
      // This is the next received frame, not proof that the requested offset
      // was painted; native acceptance correlates visible fixture rows too.
      if (frameStart !== null) { metric('next-frame', { elapsedMs: performance.now() - frameStart }); frameStart = null }
    },
    sync(next: PaneScrollInfo | null) {
      if (disposed || blocked || !options.allowed()) return
      if (writing || pending !== null) {
        if (state && next) publish({ ...next, maxOffsetFromBottom: Math.max(state.maxOffsetFromBottom, next.maxOffsetFromBottom), offsetFromBottom: state.offsetFromBottom })
      } else { revision++; publish(next) }
    },
    scroll(delta: number) {
      if (disposed || blocked || !options.allowed() || !Number.isFinite(delta) || !Math.trunc(delta)) return
      if (state) move(state.offsetFromBottom - Math.trunc(delta))
      else { pendingDelta += Math.trunc(delta); void refresh() }
    },
    reset() {
      if (disposed) return
      generation++; revision++; pendingDelta = 0; pending = null; blocked = false; frameStart = null
      attempts = 0; busySince = null; retryAt = 0; lastSent = -Infinity
      clearTimers(); abort.abort(); abort = new AbortController(); publish(null)
    },
    dispose() { disposed = true; generation++; pending = null; pendingDelta = 0; clearTimers(); abort.abort() }
  }
}

export type PaneScrollController = ReturnType<typeof createPaneScrollController>
