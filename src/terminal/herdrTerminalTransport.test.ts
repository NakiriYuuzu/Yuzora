import { beforeEach, describe, expect, it, vi } from "vitest"

vi.mock("@/lib/herdrIpc", () => ({
  herdrTerminalOpen: vi.fn(),
  herdrTerminalInput: vi.fn(),
  herdrTerminalResize: vi.fn(),
  herdrTerminalScroll: vi.fn(),
  herdrTerminalRelease: vi.fn()
}))

vi.mock("@/lib/ipc", () => ({
  ptyOpen: vi.fn(),
  ptyWrite: vi.fn(),
  ptyResize: vi.fn(),
  ptyClose: vi.fn()
}))

import {
  herdrTerminalInput,
  herdrTerminalOpen,
  herdrTerminalRelease,
  herdrTerminalResize,
  herdrTerminalScroll
} from "@/lib/herdrIpc"
import { ptyClose, ptyOpen, ptyWrite } from "@/lib/ipc"
import type { HerdrTerminalEvent } from "@/lib/herdrTypes"
import {
  createHerdrTerminalTransport,
  createLocalPtyTransport,
  normalizeTerminalWheelRows
} from "./terminalTransport"

function b64(text: string): string {
  return btoa(text)
}

describe("normalizeTerminalWheelRows", () => {
  it("normalizes pixel, line, and page deltas within the visible row bound", () => {
    expect(normalizeTerminalWheelRows(0, 0, 24)).toBe(0)
    expect(normalizeTerminalWheelRows(1, 0, 24)).toBe(1)
    expect(normalizeTerminalWheelRows(48, 0, 24)).toBe(3)
    expect(normalizeTerminalWheelRows(5, 1, 24)).toBe(5)
    expect(normalizeTerminalWheelRows(2, 2, 24)).toBe(24)
    expect(normalizeTerminalWheelRows(Number.NaN, 0, 24)).toBe(0)
  })
})

describe("createHerdrTerminalTransport", () => {
  it("passes sessionName to herdrTerminalOpen", async () => {
    vi.mocked(herdrTerminalOpen).mockResolvedValue({
      sessionId: "sess-1",
      target: "t1",
      mode: "control",
      role: "controller",
      cols: 80,
      rows: 24,
      takeover: true
    })
    const transport = createHerdrTerminalTransport({
      terminalId: "t1",
      sessionName: "work"
    })
    await transport.open({ cols: 80, rows: 24, onEvent: () => undefined })
    expect(herdrTerminalOpen).toHaveBeenCalledWith(
      expect.objectContaining({ target: "t1", sessionName: "work" })
    )
  })

  beforeEach(() => {
    vi.mocked(herdrTerminalOpen).mockReset()
    vi.mocked(herdrTerminalInput).mockReset()
    vi.mocked(herdrTerminalResize).mockReset()
    vi.mocked(herdrTerminalScroll).mockReset()
    vi.mocked(herdrTerminalRelease).mockReset()
  })

  it("opens as control+takeover by default and allows write", async () => {
    vi.mocked(herdrTerminalOpen).mockImplementation(async (args) => {
      expect(args.target).toBe("t1")
      return {
        sessionId: "sess-1",
        target: args.target,
        mode: "control",
        role: "controller",
        cols: args.cols,
        rows: args.rows,
        takeover: true
      }
    })
    vi.mocked(herdrTerminalRelease).mockResolvedValue(undefined)

    const transport = createHerdrTerminalTransport({
      terminalId: "t1"
    })
    await transport.open({
      cols: 80,
      rows: 24,
      onEvent: () => undefined
    })

    expect(herdrTerminalOpen).toHaveBeenCalledWith(
      expect.objectContaining({
        mode: "control",
        takeover: true,
        target: "t1"
      })
    )
    expect(transport.canWrite()).toBe(true)
    await transport.write("hello")
    expect(herdrTerminalInput).toHaveBeenCalledWith("sess-1", "hello", null)
  })

  it("observe mode still blocks write until takeControl", async () => {
    vi.mocked(herdrTerminalOpen).mockImplementation(async (args) => {
      if ((args.mode ?? "observe") === "observe") {
        return {
          sessionId: "sess-1",
          target: args.target,
          mode: "observe",
          role: "observer",
          cols: args.cols,
          rows: args.rows,
          takeover: false
        }
      }
      return {
        sessionId: "sess-2",
        target: args.target,
        mode: "control",
        role: "controller",
        cols: args.cols,
        rows: args.rows,
        takeover: true
      }
    })
    vi.mocked(herdrTerminalRelease).mockResolvedValue(undefined)

    const transport = createHerdrTerminalTransport({
      terminalId: "t1",
      mode: "observe",
      takeover: false
    })
    await transport.open({
      cols: 80,
      rows: 24,
      onEvent: () => undefined
    })

    expect(transport.canWrite()).toBe(false)
    await transport.write("hello")
    expect(herdrTerminalInput).not.toHaveBeenCalled()

    await transport.takeControl?.()
    expect(herdrTerminalRelease).toHaveBeenCalledWith("sess-1")
    expect(herdrTerminalOpen).toHaveBeenLastCalledWith(
      expect.objectContaining({
        mode: "control",
        takeover: true,
        target: "t1"
      })
    )
    expect(transport.canWrite()).toBe(true)
  })

  it("decodes frame bytes and surfaces resync", async () => {
    const handlers: Array<(event: HerdrTerminalEvent) => void> = []
    vi.mocked(herdrTerminalOpen).mockImplementation(async (args) => {
      handlers.push(args.onEvent)
      return {
        sessionId: "sess-1",
        target: args.target,
        mode: "observe",
        role: "observer",
        cols: 80,
        rows: 24,
        takeover: false
      }
    })

    const transport = createHerdrTerminalTransport({
      terminalId: "t1"
    })
    const events: Array<{ type: string; message?: string; data?: string; seq?: number }> = []
    await transport.open({
      cols: 80,
      rows: 24,
      onEvent: (e) => {
        if (e.type === "output") events.push({ type: e.type, seq: e.seq, data: e.data })
        if (e.type === "resync") events.push({ type: e.type, message: e.message })
        if (e.type === "error") events.push({ type: e.type, message: e.message })
      }
    })

    handlers[0]({
      type: "frame",
      sessionId: "sess-1",
      seq: 1,
      full: true,
      encoding: "ansi",
      width: 80,
      height: 24,
      bytesBase64: b64("a")
    })
    handlers[0]({
      type: "resync",
      sessionId: "sess-1",
      expectedSeq: 2,
      receivedSeq: 4,
      message: "gap"
    })

    expect(events.some((e) => e.type === "output" && e.data === "a")).toBe(true)
    expect(events.some((e) => e.type === "resync" && e.message === "gap")).toBe(true)
  })

  it("marks a closed connector non-writable before surfacing exit", async () => {
    let handleEvent: ((event: HerdrTerminalEvent) => void) | undefined
    vi.mocked(herdrTerminalOpen).mockImplementation(async (args) => {
      handleEvent = args.onEvent
      return {
        sessionId: "sess-closed",
        target: args.target,
        mode: "control",
        role: "controller",
        cols: args.cols,
        rows: args.rows,
        takeover: true
      }
    })
    const events: string[] = []
    const transport = createHerdrTerminalTransport({ terminalId: "t-closed" })
    await transport.open({
      cols: 80,
      rows: 24,
      onEvent: (event) => events.push(event.type)
    })

    handleEvent?.({ type: "closed", sessionId: "sess-closed" })

    expect(events).toContain("exit")
    expect(transport.getSessionId?.()).toBeNull()
    expect(transport.canWrite()).toBe(false)
    await transport.write("ignored")
    await transport.scroll?.(-5)
    expect(herdrTerminalInput).not.toHaveBeenCalled()
    expect(herdrTerminalScroll).not.toHaveBeenCalled()
  })

  it("release calls herdr_terminal_release and is idempotent", async () => {
    vi.mocked(herdrTerminalOpen).mockResolvedValue({
      sessionId: "sess-9",
      target: "t9",
      mode: "observe",
      role: "observer",
      cols: 80,
      rows: 24,
      takeover: false
    })
    vi.mocked(herdrTerminalRelease).mockResolvedValue(undefined)

    const transport = createHerdrTerminalTransport({
      terminalId: "t9"
    })
    await transport.open({ cols: 80, rows: 24, onEvent: () => undefined })
    await transport.release()
    await transport.release()
    expect(herdrTerminalRelease).toHaveBeenCalledTimes(1)
    expect(herdrTerminalRelease).toHaveBeenCalledWith("sess-9")
  })

  it("resize is a no-op for observe", async () => {
    vi.mocked(herdrTerminalOpen).mockResolvedValue({
      sessionId: "sess-1",
      target: "t1",
      mode: "observe",
      role: "observer",
      cols: 80,
      rows: 24,
      takeover: false
    })
    const transport = createHerdrTerminalTransport({
      terminalId: "t1"
    })
    await transport.open({ cols: 80, rows: 24, onEvent: () => undefined })
    await transport.resize(100, 40)
    expect(herdrTerminalResize).not.toHaveBeenCalled()
  })
})


  it("does not mutate server scrollback while observing", async () => {
    vi.mocked(herdrTerminalOpen).mockResolvedValue({
      sessionId: "session-observe",
      target: "term-1",
      mode: "observe",
      role: "observer",
      cols: 80,
      rows: 24,
      takeover: false
    })
    const transport = createHerdrTerminalTransport({
      terminalId: "term-1",
      mode: "observe"
    })
    await transport.open({ cols: 80, rows: 24, onEvent: vi.fn() })

    await transport.scroll?.(-5)

    expect(herdrTerminalScroll).not.toHaveBeenCalled()
  })

  it("dispose makes later open/takeControl no-op and releases late open results", async () => {
    type OpenResult = {
      sessionId: string
      target: string
      mode: "observe" | "control"
      role: "observer" | "controller"
      cols: number
      rows: number
      takeover: boolean
    }
    let resolveOpen: ((value: OpenResult) => void) | undefined
    vi.mocked(herdrTerminalOpen).mockImplementation(
      () =>
        new Promise<OpenResult>((resolve) => {
          resolveOpen = resolve
        })
    )
    vi.mocked(herdrTerminalRelease).mockResolvedValue(undefined)

    const attachments: string[] = []
    const transport = createHerdrTerminalTransport({
      terminalId: "t-race",
      onAttachment: (info) => attachments.push(info.sessionId)
    })

    const openPromise = transport.open({
      cols: 80,
      rows: 24,
      onEvent: () => undefined
    })
    // Dispose while open is in flight.
    await transport.dispose?.()
    resolveOpen?.({
      sessionId: "late-sess",
      target: "t-race",
      mode: "observe",
      role: "observer",
      cols: 80,
      rows: 24,
      takeover: false
    })
    await openPromise

    expect(transport.isDisposed?.()).toBe(true)
    expect(herdrTerminalRelease).toHaveBeenCalledWith("late-sess")
    expect(attachments).toEqual([])
    expect(transport.getSessionId?.()).toBeNull()

    // Later reopen paths no-op.
    vi.mocked(herdrTerminalOpen).mockClear()
    await transport.open({ cols: 80, rows: 24, onEvent: () => undefined })
    await transport.takeControl?.()
    expect(herdrTerminalOpen).not.toHaveBeenCalled()
  })

  it("dispose during Take Control release prevents controller reopen", async () => {
    vi.mocked(herdrTerminalOpen).mockResolvedValue({
      sessionId: "sess-obs",
      target: "t1",
      mode: "observe",
      role: "observer",
      cols: 80,
      rows: 24,
      takeover: false
    })
    let finishObserverRelease!: () => void
    vi.mocked(herdrTerminalRelease).mockImplementation(
      () =>
        new Promise<void>((resolve) => {
          finishObserverRelease = resolve
        })
    )

    const transport = createHerdrTerminalTransport({ terminalId: "t1" })
    await transport.open({ cols: 80, rows: 24, onEvent: () => undefined })
    vi.mocked(herdrTerminalOpen).mockClear()

    const takeControl = transport.takeControl?.()
    await Promise.resolve()
    expect(herdrTerminalRelease).toHaveBeenCalledWith("sess-obs")
    await transport.dispose?.()
    finishObserverRelease()
    await takeControl

    expect(transport.isDisposed?.()).toBe(true)
    expect(herdrTerminalOpen).not.toHaveBeenCalled()
    expect(transport.canWrite()).toBe(false)
  })

describe("createLocalPtyTransport", () => {
  beforeEach(() => {
    vi.mocked(ptyOpen).mockReset()
    vi.mocked(ptyWrite).mockReset()
    vi.mocked(ptyClose).mockReset()
  })

  it("wraps existing pty open/write/close", async () => {
    vi.mocked(ptyOpen).mockResolvedValue({
      sessionId: "pty-1",
      shell: "/bin/zsh",
      cols: 80,
      rows: 24
    } as never)
    vi.mocked(ptyWrite).mockResolvedValue(undefined)
    vi.mocked(ptyClose).mockResolvedValue(undefined)

    const transport = createLocalPtyTransport({
      workspace: "/w",
      sessionId: "pty-1"
    })
    await transport.open({ cols: 80, rows: 24, onEvent: () => undefined })
    expect(transport.canWrite()).toBe(true)
    await transport.write("x")
    expect(ptyWrite).toHaveBeenCalledWith("pty-1", "x")
    await transport.release()
    expect(ptyClose).toHaveBeenCalledWith("pty-1")
    expect(ptyOpen).toHaveBeenCalled()
  })
})
