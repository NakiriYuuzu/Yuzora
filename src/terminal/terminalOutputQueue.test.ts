import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import {
  TERMINAL_OUTPUT_TRUNCATED_NOTICE,
  TerminalOutputQueue,
  registerTerminalOutputQueue,
  terminalOutputLossNotice,
  terminalOutputMetrics,
  terminalOutputMetricsSnapshot,
  unregisterTerminalOutputQueue,
  utf8Length,
} from "./terminalOutputQueue"

let nextFrameId = 1
let frames = new Map<number, FrameRequestCallback>()

function flushFrame(): void {
  const entry = frames.entries().next().value as [number, FrameRequestCallback] | undefined
  if (!entry) throw new Error("No output frame scheduled")
  frames.delete(entry[0])
  entry[1](0)
}

/** Runs every frame scheduled so far without draining the ones they queue. */
function flushScheduledFrames(): void {
  const scheduled = [...frames.values()]
  frames.clear()
  for (const callback of scheduled) callback(0)
}

beforeEach(() => {
  nextFrameId = 1
  frames = new Map()
  vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => {
    const id = nextFrameId++
    frames.set(id, callback)
    return id
  })
  vi.stubGlobal("cancelAnimationFrame", (id: number) => {
    frames.delete(id)
  })
})

afterEach(() => {
  vi.unstubAllGlobals()
})

describe("TerminalOutputQueue", () => {
  it("batches visible output and waits for xterm before scheduling the next write", () => {
    const writes: Array<{ data: string; done: () => void }> = []
    const queue = new TerminalOutputQueue(
      (data, done) => writes.push({ data, done }),
      true,
    )

    queue.push("a")
    queue.push("b")
    expect(writes).toEqual([])
    expect(frames.size).toBe(1)

    flushFrame()
    expect(writes[0]?.data).toBe("ab")

    queue.push("c")
    expect(frames.size).toBe(0)
    writes[0]?.done()
    expect(frames.size).toBe(1)
    flushFrame()
    expect(writes[1]?.data).toBe("c")
  })

  it("does not write while hidden and replays a bounded buffer with a truncation notice", () => {
    const writes: string[] = []
    const queue = new TerminalOutputQueue(
      (data, done) => {
        writes.push(data)
        done()
      },
      false,
      5,
    )

    queue.push("abc")
    queue.push("def")
    expect(writes).toEqual([])
    expect(frames.size).toBe(0)

    queue.setVisible(true)
    flushFrame()
    expect(writes).toEqual([`${TERMINAL_OUTPUT_TRUNCATED_NOTICE}def`])
  })

  it("keeps the newest bounded tail for one oversized chunk", () => {
    const writes: string[] = []
    const queue = new TerminalOutputQueue(
      (data, done) => {
        writes.push(data)
        done()
      },
      false,
      4,
    )

    queue.push("123456")
    queue.setVisible(true)
    flushFrame()

    expect(writes).toEqual([`${TERMINAL_OUTPUT_TRUNCATED_NOTICE}3456`])
  })
})

describe("TerminalOutputQueue telemetry", () => {
  it("reports pending, hidden, dropped, flush count and flush latency", () => {
    const now = vi.spyOn(performance, "now").mockReturnValue(1_000)
    const writes: Array<{ data: string; done: () => void }> = []
    const queue = new TerminalOutputQueue(
      (data, done) => writes.push({ data, done }),
      true,
      8,
    )

    expect(queue.pendingBytes).toBe(0)
    expect(queue.hiddenBytes).toBe(0)
    expect(queue.droppedBytes).toBe(0)
    expect(queue.flushCount).toBe(0)
    expect(queue.lastFlushLatencyMs).toBe(0)

    queue.push("abcd")
    expect(queue.pendingBytes).toBe(4)

    // Overflowing the ring buffer counts the discarded bytes.
    queue.push("efghij")
    expect(queue.pendingBytes).toBe(6)
    expect(queue.droppedBytes).toBe(4)

    flushFrame()
    expect(queue.flushCount).toBe(1)
    expect(queue.pendingBytes).toBe(0)

    // A sub-millisecond write must not report a flat 0: #40 reads this to
    // decide whether the UI is stalling.
    now.mockReturnValue(1_000.35)
    writes[0].done()
    expect(queue.lastFlushLatencyMs).toBeCloseTo(0.35, 5)

    queue.setVisible(false)
    queue.push("hidden")
    expect(queue.hiddenBytes).toBe(6)
    expect(queue.pendingBytes).toBe(0)
    expect(queue.flushCount).toBe(1)

    now.mockRestore()
  })
})

describe("TerminalOutputQueue soak", () => {
  it("keeps 12 concurrent sessions bounded and batches sub-linearly", () => {
    const limit = 1024
    const sessions = Array.from({ length: 12 }, (_, index) => {
      const writes: string[] = []
      const visible = index % 2 === 0
      const queue = new TerminalOutputQueue(
        (data, done) => {
          writes.push(data)
          done()
        },
        visible,
        limit,
      )
      return { queue, writes, visible }
    })

    const chunk = "x".repeat(256)
    const rounds = 400
    const framesPerRound = 20
    let maxPendingBytes = 0
    let maxHiddenBytes = 0

    for (let round = 0; round < rounds; round += 1) {
      for (const session of sessions) {
        session.queue.push(chunk)
        maxPendingBytes = Math.max(maxPendingBytes, session.queue.pendingBytes)
        maxHiddenBytes = Math.max(maxHiddenBytes, session.queue.hiddenBytes)
      }
      if (round % framesPerRound === framesPerRound - 1) flushScheduledFrames()
    }

    // (a) neither buffer may grow past the configured cap
    expect(maxPendingBytes).toBeLessThanOrEqual(limit)
    expect(maxHiddenBytes).toBeLessThanOrEqual(limit)

    for (const session of sessions) {
      if (session.visible) {
        // (b) flushes stay proportional to frames, not to the write count
        expect(session.queue.flushCount).toBeGreaterThan(0)
        expect(session.queue.flushCount).toBeLessThanOrEqual(rounds / framesPerRound)
        expect(session.writes.length).toBe(session.queue.flushCount)
      } else {
        // (c) hidden sessions never touch xterm
        expect(session.writes).toEqual([])
        expect(session.queue.flushCount).toBe(0)
      }
      expect(session.queue.droppedBytes).toBeGreaterThan(0)
    }
  })
})

describe("TerminalOutputQueue UTF-8 accounting", () => {
  it("counts UTF-8 bytes, not UTF-16 code units", () => {
    expect(utf8Length("abc")).toBe(3)
    expect(utf8Length("編譯完成")).toBe(12)
    expect(utf8Length("é")).toBe(2)
    expect(utf8Length("😀")).toBe(4)
    expect(utf8Length("→中文😀é─")).toBe(3 + 6 + 4 + 2 + 3)
    // Lone surrogates encode as the 3-byte replacement character.
    expect(utf8Length("\ud83d")).toBe(3)
    expect(utf8Length("\ude00")).toBe(3)
  })

  it("bounds the buffer by UTF-8 bytes so telemetry matches the Rust counters", () => {
    const writes: string[] = []
    const queue = new TerminalOutputQueue(
      (data, done) => {
        writes.push(data)
        done()
      },
      true,
      64,
    )

    queue.push("編譯完成")
    // 4 chars but 12 bytes — a code-unit count would report 4 here.
    expect(queue.pendingBytes).toBe(12)

    queue.push("😀")
    expect(queue.pendingBytes).toBe(16)
  })

  it("trims an oversized chunk on a code point boundary, never mid-character", () => {
    const writes: string[] = []
    const queue = new TerminalOutputQueue(
      (data, done) => {
        writes.push(data)
        done()
      },
      false,
      7,
    )

    // 12 bytes into a 7-byte buffer: the newest whole characters are "完成"
    // (6 bytes); taking a 7th byte would split "譯".
    queue.push("編譯完成")
    expect(queue.hiddenBytes).toBe(6)
    expect(queue.droppedBytes).toBe(6)

    queue.setVisible(true)
    flushFrame()
    expect(writes).toEqual([`${TERMINAL_OUTPUT_TRUNCATED_NOTICE}完成`])
  })

  it("keeps a surrogate pair intact when trimming", () => {
    const queue = new TerminalOutputQueue((_data, done) => done(), false, 5)

    // "a😀" is 5 bytes; adding another 4-byte emoji must drop "a" and keep
    // the whole emoji rather than half a surrogate pair.
    queue.push("a😀😀")
    expect(queue.hiddenBytes).toBe(4)
    expect(queue.droppedBytes).toBe(5)
  })
})

describe("TerminalOutputQueue loss notices", () => {
  it("keeps the marker when an oversized chunk arrives in the same frame", () => {
    const writes: string[] = []
    const limit = 32
    const queue = new TerminalOutputQueue(
      (data, done) => {
        writes.push(data)
        done()
      },
      true,
      limit,
    )

    queue.noteBackendLoss(4096, 2)
    // Larger than the whole buffer: the ring buffer keeps only its tail, which
    // used to take the marker down with it.
    queue.push("y".repeat(limit * 4))
    flushFrame()

    expect(writes).toHaveLength(1)
    expect(writes[0]).toContain(terminalOutputLossNotice(4096, 2))
    expect(writes[0].endsWith("y".repeat(limit))).toBe(true)
  })

  it("does not count its own marker as dropped terminal output", () => {
    const queue = new TerminalOutputQueue((_data, done) => done(), true, 32)

    queue.noteBackendLoss(999999, 999999)
    expect(queue.droppedBytes).toBe(0)
    expect(queue.pendingBytes).toBe(0)

    flushFrame()
    expect(queue.droppedBytes).toBe(0)
  })

  it("emits a marker even with no terminal output pending", () => {
    const writes: string[] = []
    const queue = new TerminalOutputQueue(
      (data, done) => {
        writes.push(data)
        done()
      },
      true,
    )

    queue.noteBackendLoss(512, 0)
    flushFrame()
    expect(writes).toEqual([terminalOutputLossNotice(512, 0)])
  })

  it("ignores a report with nothing lost", () => {
    const writes: string[] = []
    const queue = new TerminalOutputQueue(
      (data, done) => {
        writes.push(data)
        done()
      },
      true,
    )

    queue.noteBackendLoss(0, 0)
    expect(frames.size).toBe(0)
    expect(writes).toEqual([])
  })

  it("coalesces repeated reports into one marker carrying the totals", () => {
    const writes: string[] = []
    const queue = new TerminalOutputQueue(
      (data, done) => {
        writes.push(data)
        done()
      },
      true,
    )

    queue.noteBackendLoss(100, 1)
    queue.noteBackendLoss(250, 2)
    queue.noteBackendLoss(0, 3)
    flushFrame()

    expect(writes).toEqual([terminalOutputLossNotice(350, 6)])

    // Totals reset with the flush rather than being reported twice.
    queue.noteBackendLoss(7, 0)
    flushFrame()
    expect(writes[1]).toBe(terminalOutputLossNotice(7, 0))
  })

  it("holds the marker until the session becomes visible again", () => {
    const writes: string[] = []
    const queue = new TerminalOutputQueue(
      (data, done) => {
        writes.push(data)
        done()
      },
      false,
    )

    queue.noteBackendLoss(64, 1)
    expect(frames.size).toBe(0)
    expect(writes).toEqual([])

    queue.setVisible(true)
    flushFrame()
    expect(writes).toEqual([terminalOutputLossNotice(64, 1)])
  })

  it("stays bounded while hidden through a sustained storm", () => {
    const writes: string[] = []
    const queue = new TerminalOutputQueue(
      (data, done) => {
        writes.push(data)
        done()
      },
      false,
    )

    // Roughly an hour of the measured ~62 backend events/s, every one of them
    // reporting loss. The old text-accumulating design grew without bound here.
    const reports = 62 * 60 * 60
    for (let index = 0; index < reports; index += 1) {
      queue.noteBackendLoss(4096, 1)
    }
    expect(writes).toEqual([])

    // Retained state must not scale with the number of reports: two numbers.
    const retained = JSON.stringify(queue).length
    expect(retained).toBeLessThan(1024)

    queue.setVisible(true)
    flushFrame()

    // And the catch-up write is one line, not one marker per report.
    expect(writes).toHaveLength(1)
    expect(writes[0].length).toBeLessThan(256)
    expect(writes[0]).toBe(terminalOutputLossNotice(4096 * reports, reports))
  })
})

describe("terminal output metrics registry", () => {
  afterEach(() => {
    unregisterTerminalOutputQueue("session-a")
    unregisterTerminalOutputQueue("session-b")
  })

  it("resolves live metrics by session id and forgets unregistered sessions", () => {
    expect(terminalOutputMetrics("session-a")).toBeNull()
    expect(terminalOutputMetricsSnapshot()).toEqual({})

    const queue = new TerminalOutputQueue((_data, done) => done(), true, 8)
    registerTerminalOutputQueue("session-a", queue)

    queue.push("abcd")
    expect(terminalOutputMetrics("session-a")).toMatchObject({
      pendingBytes: 4,
      hiddenBytes: 0,
      droppedBytes: 0,
      flushCount: 0,
    })

    queue.push("efghij")
    flushFrame()
    const metrics = terminalOutputMetrics("session-a")
    expect(metrics?.droppedBytes).toBe(4)
    expect(metrics?.flushCount).toBe(1)
    expect(Object.keys(terminalOutputMetricsSnapshot())).toEqual(["session-a"])

    unregisterTerminalOutputQueue("session-a")
    expect(terminalOutputMetrics("session-a")).toBeNull()
    expect(terminalOutputMetricsSnapshot()).toEqual({})
  })

  it("does not grow when the same session registers repeatedly", () => {
    // The registry is the other container this issue added; keyed by session
    // id so a re-registering session replaces its entry instead of stacking.
    for (let index = 0; index < 500; index += 1) {
      registerTerminalOutputQueue(
        "session-a",
        new TerminalOutputQueue((_data, done) => done(), true),
      )
    }
    expect(Object.keys(terminalOutputMetricsSnapshot())).toEqual(["session-a"])

    unregisterTerminalOutputQueue("session-a")
    expect(terminalOutputMetricsSnapshot()).toEqual({})
  })
})
