/**
 * Ring-buffer ceiling per session, in **UTF-8 bytes**.
 *
 * Deliberately equal to `PTY_OUTPUT_PENDING_CAP` in `src-tauri/src/pty_service.rs`
 * so a full backend batch fits exactly, nothing crosses the IPC boundary only
 * to be discarded on arrival, and the two `droppedBytes` counters measure the
 * same threshold.
 *
 * Tradeoff: the unit is UTF-8 bytes rather than characters so these numbers are
 * comparable with the Rust counters. CJK text costs 3 bytes per character, so a
 * hidden Chinese session retains roughly a third of the characters an ASCII one
 * does. Accepted deliberately in exchange for cross-boundary comparability.
 */
const TERMINAL_OUTPUT_BUFFER_LIMIT = 256 * 1024
export const TERMINAL_OUTPUT_TRUNCATED_NOTICE =
  "\u001b[0m\r\n[Yuzora: hidden terminal output was truncated]\r\n"

/**
 * Marker for output that never reached the UI: bytes the Rust pending cap
 * discarded, and whole events missing from the backend sequence.
 *
 * Deliberately additive rather than one marker per event. The queue keeps only
 * the two running totals, so a hidden session accumulates O(1) state and emits
 * a single line when it becomes visible again, instead of one marker per
 * dropped event. Kept in the same hard-coded English form as the truncation
 * notice above because it is written into the xterm buffer, not rendered by
 * React.
 */
export function terminalOutputLossNotice(
  droppedBytes: number,
  missedEvents: number,
): string {
  const parts: string[] = []
  if (droppedBytes > 0) parts.push(`dropped ${droppedBytes} bytes`)
  if (missedEvents > 0) parts.push(`lost ${missedEvents} output event(s)`)
  if (parts.length === 0) return ""
  return `\u001b[0m\r\n[Yuzora: ${parts.join(", ")} of terminal output]\r\n`
}

/**
 * UTF-8 byte length of a JS string. `String.length` counts UTF-16 code units,
 * which disagrees with the Rust `PtyOutputMetrics` byte counters by 3x for CJK
 * text — and both sets of numbers are shown side by side as "bytes".
 */
export function utf8Length(value: string): number {
  let bytes = 0
  for (let index = 0; index < value.length; index += 1) {
    const code = value.charCodeAt(index)
    if (code < 0x80) {
      bytes += 1
    } else if (code < 0x800) {
      bytes += 2
    } else if (code >= 0xd800 && code <= 0xdbff && index + 1 < value.length) {
      const low = value.charCodeAt(index + 1)
      if (low >= 0xdc00 && low <= 0xdfff) {
        // Surrogate pair: one 4-byte code point, so skip the low half.
        bytes += 4
        index += 1
      } else {
        // Lone surrogate; it encodes as the 3-byte replacement character.
        bytes += 3
      }
    } else {
      bytes += 3
    }
  }
  return bytes
}

/** Newest `limitBytes` UTF-8 bytes of `value`, never splitting a code point. */
function utf8Tail(value: string, limitBytes: number): string {
  let bytes = 0
  let index = value.length
  while (index > 0) {
    const code = value.charCodeAt(index - 1)
    let step = 1
    let charBytes: number
    if (code >= 0xdc00 && code <= 0xdfff && index >= 2
      && value.charCodeAt(index - 2) >= 0xd800 && value.charCodeAt(index - 2) <= 0xdbff) {
      charBytes = 4
      step = 2
    } else if (code < 0x80) {
      charBytes = 1
    } else if (code < 0x800) {
      charBytes = 2
    } else {
      charBytes = 3
    }
    if (bytes + charBytes > limitBytes) break
    bytes += charBytes
    index -= step
  }
  return value.slice(index)
}

/**
 * Monotonic, sub-millisecond clock. `Date.now()` has 1 ms resolution and is
 * wall clock, so it reports 0 for a normal xterm write and can go negative
 * across an NTP step — useless for the stall diagnostics this feeds.
 */
function now(): number {
  return typeof globalThis.performance?.now === "function"
    ? globalThis.performance.now()
    : Date.now()
}

/**
 * Read-only terminal output queue metrics (issue #39 AC 6).
 *
 * Issue #40 §3.4 consumes exactly this shape through
 * `terminalOutputMetrics(sessionId)` below, alongside the Rust
 * `PtyOutputMetrics` from the `pty_output_metrics` command. Byte counts on both
 * sides are UTF-8 and use the same ring-buffer threshold, so they are directly
 * comparable. Extend this shape rather than adding a second one.
 */
export interface TerminalOutputMetrics {
  /** UTF-8 bytes buffered for the next visible flush. */
  pendingBytes: number
  /** UTF-8 bytes buffered while the session is hidden. */
  hiddenBytes: number
  /** Cumulative UTF-8 bytes of terminal output this queue discarded. */
  droppedBytes: number
  /** Milliseconds the last flush took, from a monotonic sub-ms clock. */
  lastFlushLatencyMs: number
  /** Cumulative flushes handed to xterm. */
  flushCount: number
}

type TerminalWriter = (data: string, onProcessed: () => void) => void

interface ScheduledFrame {
  id: number
  cancel: (id: number) => void
}

function scheduleFrame(run: () => void): ScheduledFrame {
  if (typeof globalThis.requestAnimationFrame === "function") {
    return {
      id: globalThis.requestAnimationFrame(run),
      cancel: (id) => globalThis.cancelAnimationFrame(id),
    }
  }
  return {
    id: globalThis.setTimeout(run, 0) as unknown as number,
    cancel: (id) => globalThis.clearTimeout(id),
  }
}

export class TerminalOutputQueue {
  private visible: boolean
  private disposed = false
  private writing = false
  private scheduled: ScheduledFrame | null = null
  private hiddenChunks: string[] = []
  // Per-chunk UTF-8 sizes, so trimming the ring buffer never rescans a chunk.
  private hiddenSizes: number[] = []
  private hiddenSize = 0
  private hiddenTruncated = false
  private pendingChunks: string[] = []
  private pendingSizes: number[] = []
  private pendingSize = 0
  private pendingTruncated = false
  private replacement: {
    data: string
    size: number
    beforeWrite?: () => void
  } | null = null
  private droppedTotal = 0
  // Backend loss reported since the last flush, kept as running totals rather
  // than accumulated marker text. Out of the ring buffers so a notice about
  // dropped output can never itself be dropped, and O(1) so a hidden session
  // cannot grow them without bound.
  private noticeDroppedBytes = 0
  private noticeMissedEvents = 0
  private flushes = 0
  private flushLatencyMs = 0
  private flushStartedAt = 0

  constructor(
    private readonly write: TerminalWriter,
    visible: boolean,
    private readonly limit = TERMINAL_OUTPUT_BUFFER_LIMIT,
  ) {
    this.visible = visible
  }

  // Read-only telemetry (issue #39 AC 6), also surfaced next to the Rust
  // `PtyOutputMetrics` counters from `pty_output_metrics`. Byte counts are
  // UTF-8 so both sides agree on the unit. Extend this set rather than adding
  // a parallel metrics shape. Reading never mutates the queue or changes when
  // a flush happens.

  /** UTF-8 bytes buffered for the next visible flush. */
  get pendingBytes(): number {
    return this.pendingSize + (this.visible ? this.replacement?.size ?? 0 : 0)
  }

  /** UTF-8 bytes buffered by the ring buffer while the session is hidden. */
  get hiddenBytes(): number {
    return this.hiddenSize + (!this.visible ? this.replacement?.size ?? 0 : 0)
  }

  /** Cumulative UTF-8 bytes this queue's ring buffers discarded. */
  get droppedBytes(): number {
    return this.droppedTotal
  }

  /**
   * Milliseconds the last flush took from hand-off to the xterm callback,
   * from a monotonic sub-millisecond clock (so a fast write reports a small
   * fraction rather than a misleading 0).
   */
  get lastFlushLatencyMs(): number {
    return this.flushLatencyMs
  }

  /** Cumulative flushes handed to xterm. */
  get flushCount(): number {
    return this.flushes
  }

  /**
   * Records output the backend dropped or lost before it reached this queue.
   *
   * The totals are coalesced into a single marker emitted on the next flush.
   * That marker bypasses the ring buffers, so (a) an oversized data chunk
   * arriving in the same frame cannot evict it — which is exactly the storm
   * case it exists for — and (b) it is never counted in `droppedBytes`, which
   * must measure terminal output only. Because only two numbers are retained,
   * a session that stays hidden through an output storm neither grows its
   * memory nor builds up marker text to dump into xterm when it returns.
   *
   * Use `push` instead for text with in-stream ordering semantics (an exit
   * notice belongs after the output that preceded it, not ahead of it).
   */
  noteBackendLoss(droppedBytes: number, missedEvents: number): void {
    if (this.disposed) return
    if (droppedBytes <= 0 && missedEvents <= 0) return
    if (droppedBytes > 0) this.noticeDroppedBytes += droppedBytes
    if (missedEvents > 0) this.noticeMissedEvents += missedEvents
    this.schedule()
  }

  /** Pending backend-loss marker text, or "" when there is nothing to report. */
  private pendingLossNotice(): string {
    return terminalOutputLossNotice(this.noticeDroppedBytes, this.noticeMissedEvents)
  }

  /** Read-only snapshot of every getter below, for the metrics registry. */
  metrics(): TerminalOutputMetrics {
    return {
      pendingBytes: this.pendingBytes,
      hiddenBytes: this.hiddenBytes,
      droppedBytes: this.droppedBytes,
      lastFlushLatencyMs: this.lastFlushLatencyMs,
      flushCount: this.flushCount,
    }
  }

  push(data: string): void {
    if (this.disposed || data.length === 0) return
    if (!this.visible) {
      this.appendHidden(data)
      return
    }
    this.appendPending(data)
    this.schedule()
  }

  /**
   * Replaces all queued output with one authoritative terminal snapshot.
   *
   * Full Herdr frames are bounded screen-state snapshots, not an output burst.
   * They must bypass the incremental ring-buffer ceiling: trimming the ANSI
   * prefix corrupts the reconstructed screen and emits a misleading hidden
   * output warning while a tab is merely opening. Later incremental output is
   * still bounded normally.
   */
  replace(data: string, beforeWrite?: () => void): void {
    if (this.disposed) return
    this.hiddenChunks = []
    this.hiddenSizes = []
    this.hiddenSize = 0
    this.hiddenTruncated = false
    this.pendingChunks = []
    this.pendingSizes = []
    this.pendingSize = 0
    this.pendingTruncated = false
    this.noticeDroppedBytes = 0
    this.noticeMissedEvents = 0
    this.replacement = {
      data,
      size: utf8Length(data),
      beforeWrite
    }
    this.schedule()
  }

  setVisible(visible: boolean): void {
    if (this.disposed || this.visible === visible) return
    this.visible = visible
    if (!visible) {
      if (this.scheduled) {
        this.scheduled.cancel(this.scheduled.id)
        this.scheduled = null
      }
      if (this.pendingTruncated) this.hiddenTruncated = true
      for (const chunk of this.pendingChunks) this.appendHidden(chunk)
      this.pendingChunks = []
      this.pendingSizes = []
      this.pendingSize = 0
      this.pendingTruncated = false
      return
    }

    if (this.hiddenTruncated) this.appendPending(TERMINAL_OUTPUT_TRUNCATED_NOTICE)
    for (const chunk of this.hiddenChunks) this.appendPending(chunk)
    this.hiddenChunks = []
    this.hiddenSizes = []
    this.hiddenSize = 0
    this.hiddenTruncated = false
    this.schedule()
  }

  /** Flushes already-buffered output before the browser paints a newly visible tab. */
  flushNow(): void {
    if (this.disposed || !this.visible || this.writing) return
    if (this.scheduled) {
      this.scheduled.cancel(this.scheduled.id)
      this.scheduled = null
    }
    this.flush()
  }

  dispose(): void {
    this.disposed = true
    if (this.scheduled) this.scheduled.cancel(this.scheduled.id)
    this.scheduled = null
    this.noticeDroppedBytes = 0
    this.noticeMissedEvents = 0
    this.hiddenChunks = []
    this.hiddenSizes = []
    this.pendingChunks = []
    this.pendingSizes = []
    this.replacement = null
    this.hiddenSize = 0
    this.pendingSize = 0
  }

  private appendHidden(data: string): void {
    const bytes = utf8Length(data)
    if (bytes === 0) return
    this.hiddenChunks.push(data)
    this.hiddenSizes.push(bytes)
    this.hiddenSize += bytes
    while (this.hiddenSize > this.limit) {
      this.hiddenTruncated = true
      if (this.hiddenChunks.length > 1) {
        this.hiddenChunks.shift()
        const removed = this.hiddenSizes.shift()!
        this.hiddenSize -= removed
        this.droppedTotal += removed
        continue
      }
      // A single chunk larger than the buffer keeps its newest tail, matching
      // the Rust `OutputBuffer` rule.
      const tail = utf8Tail(this.hiddenChunks[0], this.limit)
      const tailBytes = utf8Length(tail)
      this.droppedTotal += this.hiddenSize - tailBytes
      this.hiddenChunks[0] = tail
      this.hiddenSizes[0] = tailBytes
      this.hiddenSize = tailBytes
    }
  }

  private appendPending(data: string): void {
    const bytes = utf8Length(data)
    if (bytes === 0) return
    this.pendingChunks.push(data)
    this.pendingSizes.push(bytes)
    this.pendingSize += bytes
    while (this.pendingSize > this.limit) {
      this.pendingTruncated = true
      if (this.pendingChunks.length > 1) {
        this.pendingChunks.shift()
        const removed = this.pendingSizes.shift()!
        this.pendingSize -= removed
        this.droppedTotal += removed
        continue
      }
      const tail = utf8Tail(this.pendingChunks[0], this.limit)
      const tailBytes = utf8Length(tail)
      this.droppedTotal += this.pendingSize - tailBytes
      this.pendingChunks[0] = tail
      this.pendingSizes[0] = tailBytes
      this.pendingSize = tailBytes
    }
  }

  private schedule(): void {
    if (
      this.disposed
      || !this.visible
      || this.writing
      || this.scheduled
      || (
        this.replacement === null
        && this.pendingChunks.length === 0
        && this.pendingLossNotice().length === 0
      )
    ) {
      return
    }
    this.scheduled = scheduleFrame(() => {
      this.scheduled = null
      this.flush()
    })
  }

  private flush(): void {
    if (
      this.disposed
      || !this.visible
      || this.writing
      || (
        this.replacement === null
        && this.pendingChunks.length === 0
        && this.pendingLossNotice().length === 0
      )
    ) {
      return
    }
    const replacement = this.replacement
    this.replacement = null
    const truncationPrefix = this.pendingTruncated ? TERMINAL_OUTPUT_TRUNCATED_NOTICE : ""
    const noticePrefix = this.pendingLossNotice()
    this.noticeDroppedBytes = 0
    this.noticeMissedEvents = 0
    const data = (replacement?.data ?? "") + truncationPrefix + noticePrefix + this.pendingChunks.join("")
    this.pendingChunks = []
    this.pendingSizes = []
    this.pendingSize = 0
    this.pendingTruncated = false
    this.writing = true
    this.flushes += 1
    this.flushStartedAt = now()
    try {
      replacement?.beforeWrite?.()
      this.write(data, () => {
        this.flushLatencyMs = now() - this.flushStartedAt
        this.writing = false
        this.schedule()
      })
    } catch {
      this.flushLatencyMs = now() - this.flushStartedAt
      this.writing = false
      this.schedule()
    }
  }
}

// --- Metrics registry -------------------------------------------------------
// `TerminalOutputQueue` instances live inside `TerminalSession`'s refs, so the
// getters above are unreachable from anywhere else. This registry is the seam
// issue #40 §3.4 reads: `TerminalSession` registers its queue for the lifetime
// of the pty session, and any consumer resolves metrics by session id without
// touching the component tree.

const queueRegistry = new Map<string, TerminalOutputQueue>()

export function registerTerminalOutputQueue(
  sessionId: string,
  queue: TerminalOutputQueue,
): void {
  queueRegistry.set(sessionId, queue)
}

export function unregisterTerminalOutputQueue(sessionId: string): void {
  queueRegistry.delete(sessionId)
}

/** Metrics for one live terminal session, or null if it has no open queue. */
export function terminalOutputMetrics(sessionId: string): TerminalOutputMetrics | null {
  return queueRegistry.get(sessionId)?.metrics() ?? null
}

/** Metrics for every live terminal session, keyed by session id. */
export function terminalOutputMetricsSnapshot(): Record<string, TerminalOutputMetrics> {
  const snapshot: Record<string, TerminalOutputMetrics> = {}
  for (const [sessionId, queue] of queueRegistry) snapshot[sessionId] = queue.metrics()
  return snapshot
}
