export const TERMINAL_OUTPUT_BUFFER_LIMIT = 256 * 1024
export const TERMINAL_OUTPUT_TRUNCATED_NOTICE =
  "\u001b[0m\r\n[Yuzora: hidden terminal output was truncated]\r\n"

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
  private hiddenSize = 0
  private hiddenTruncated = false
  private pendingChunks: string[] = []
  private pendingSize = 0
  private pendingTruncated = false

  constructor(
    private readonly write: TerminalWriter,
    visible: boolean,
    private readonly limit = TERMINAL_OUTPUT_BUFFER_LIMIT,
  ) {
    this.visible = visible
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
      this.pendingSize = 0
      this.pendingTruncated = false
      return
    }

    if (this.hiddenTruncated) this.appendPending(TERMINAL_OUTPUT_TRUNCATED_NOTICE)
    for (const chunk of this.hiddenChunks) this.appendPending(chunk)
    this.hiddenChunks = []
    this.hiddenSize = 0
    this.hiddenTruncated = false
    this.schedule()
  }

  dispose(): void {
    this.disposed = true
    if (this.scheduled) this.scheduled.cancel(this.scheduled.id)
    this.scheduled = null
    this.hiddenChunks = []
    this.pendingChunks = []
    this.hiddenSize = 0
    this.pendingSize = 0
  }

  private appendHidden(data: string): void {
    if (data.length > this.limit) {
      this.hiddenChunks = [data.slice(-this.limit)]
      this.hiddenSize = this.limit
      this.hiddenTruncated = true
      return
    }
    this.hiddenChunks.push(data)
    this.hiddenSize += data.length
    while (this.hiddenSize > this.limit && this.hiddenChunks.length > 0) {
      const removed = this.hiddenChunks.shift()!
      this.hiddenSize -= removed.length
      this.hiddenTruncated = true
    }
  }

  private appendPending(data: string): void {
    if (data.length > this.limit) {
      this.pendingChunks = [data.slice(-this.limit)]
      this.pendingSize = this.limit
      this.pendingTruncated = true
      return
    }
    this.pendingChunks.push(data)
    this.pendingSize += data.length
    while (this.pendingSize > this.limit && this.pendingChunks.length > 0) {
      const removed = this.pendingChunks.shift()!
      this.pendingSize -= removed.length
      this.pendingTruncated = true
    }
  }

  private schedule(): void {
    if (
      this.disposed
      || !this.visible
      || this.writing
      || this.scheduled
      || this.pendingChunks.length === 0
    ) {
      return
    }
    this.scheduled = scheduleFrame(() => {
      this.scheduled = null
      this.flush()
    })
  }

  private flush(): void {
    if (this.disposed || !this.visible || this.writing || this.pendingChunks.length === 0) {
      return
    }
    const prefix = this.pendingTruncated ? TERMINAL_OUTPUT_TRUNCATED_NOTICE : ""
    const data = prefix + this.pendingChunks.join("")
    this.pendingChunks = []
    this.pendingSize = 0
    this.pendingTruncated = false
    this.writing = true
    try {
      this.write(data, () => {
        this.writing = false
        this.schedule()
      })
    } catch {
      this.writing = false
      this.schedule()
    }
  }
}
