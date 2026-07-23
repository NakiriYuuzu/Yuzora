import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import {
  TERMINAL_OUTPUT_TRUNCATED_NOTICE,
  TerminalOutputQueue,
} from "./terminalOutputQueue"

let nextFrameId = 1
let frames = new Map<number, FrameRequestCallback>()

function flushFrame(): void {
  const entry = frames.entries().next().value as [number, FrameRequestCallback] | undefined
  if (!entry) throw new Error("No output frame scheduled")
  frames.delete(entry[0])
  entry[1](0)
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
