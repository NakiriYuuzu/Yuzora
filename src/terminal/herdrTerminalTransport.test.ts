import { beforeEach, describe, expect, it, vi } from "vitest"

vi.mock("@/lib/herdrIpc", () => ({
  herdrTerminalOpen: vi.fn(),
  herdrTerminalInput: vi.fn(),
  herdrTerminalResize: vi.fn(),
  herdrTerminalScroll: vi.fn(),
  herdrTerminalRelease: vi.fn()
}))
vi.mock("./herdrScrollIpc", () => ({
  readPaneScroll: vi.fn(),
  setPaneScroll: vi.fn()
}))

import {
  herdrTerminalInput,
  herdrTerminalOpen,
  herdrTerminalRelease,
  herdrTerminalResize,
  herdrTerminalScroll
} from "@/lib/herdrIpc"
import type { HerdrTerminalEvent } from "@/lib/herdrTypes"
import {
  createHerdrTerminalTransport,
  normalizeTerminalWheelRows
} from "./terminalTransport"
import { readPaneScroll, setPaneScroll } from "./herdrScrollIpc"

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
  it("keeps a multiline paste in one HERDR input frame between surrounding keystrokes", async () => {
    vi.mocked(herdrTerminalOpen).mockResolvedValue({ sessionId: "sess-1", target: "t1", mode: "control", role: "controller", takeover: true, cols: 80, rows: 24 })
    const transport = createHerdrTerminalTransport({ terminalId: "t1" })
    await transport.open({ cols: 80, rows: 24, onEvent: () => undefined })
    await Promise.all([transport.write("before"), transport.paste("first\r\nsecond\n"), transport.write("after")])
    expect(vi.mocked(herdrTerminalInput).mock.calls.map((call) => call[1])).toEqual([
      "before", "\x1b[200~first\nsecond\n\x1b[201~", "after"
    ])
  })

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

  it("keeps adjacent pastes separate and removes embedded paste delimiters", async () => {
    vi.mocked(herdrTerminalOpen).mockResolvedValue({ sessionId: "sess-1", target: "t1", mode: "control", role: "controller", takeover: true, cols: 80, rows: 24 })
    const transport = createHerdrTerminalTransport({ terminalId: "t1" })
    await transport.open({ cols: 80, rows: 24, onEvent: () => undefined })
    await Promise.all([transport.paste("a\x1b[201~\rb\x1b[200~"), transport.paste("c\nd")])
    expect(vi.mocked(herdrTerminalInput).mock.calls.map((call) => call[1])).toEqual([
      "\x1b[200~a\nb\x1b[201~", "\x1b[200~c\nd\x1b[201~"
    ])
  })

  beforeEach(() => {
    vi.mocked(herdrTerminalOpen).mockReset()
    vi.mocked(herdrTerminalInput).mockReset()
    vi.mocked(herdrTerminalResize).mockReset()
    vi.mocked(herdrTerminalScroll).mockReset()
    vi.mocked(herdrTerminalRelease).mockReset()
    vi.mocked(readPaneScroll).mockReset()
    vi.mocked(setPaneScroll).mockReset()
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

  it("falls back to the official pane scroll API when terminal scroll is rejected", async () => {
    vi.mocked(herdrTerminalOpen).mockResolvedValue({
      sessionId: "sess-windows",
      target: "t1",
      mode: "control",
      role: "controller",
      cols: 80,
      rows: 24,
      takeover: true
    })
    vi.mocked(herdrTerminalScroll).mockRejectedValue(new Error("terminal.scroll unavailable"))
    vi.mocked(readPaneScroll).mockResolvedValue({
      offsetFromBottom: 4,
      maxOffsetFromBottom: 20,
      viewportRows: 24
    })
    vi.mocked(setPaneScroll).mockResolvedValue({
      offsetFromBottom: 7,
      maxOffsetFromBottom: 20,
      viewportRows: 24
    })

    const transport = createHerdrTerminalTransport({
      terminalId: "t1",
      paneId: "pane-1",
      sessionName: "default"
    })
    await transport.open({ cols: 80, rows: 24, onEvent: () => undefined })
    await transport.scroll?.(-3)

    expect(readPaneScroll).toHaveBeenCalledWith("default", "pane-1")
    expect(setPaneScroll).toHaveBeenCalledWith("default", "pane-1", 7)
  })

  it("uses pane scrolling directly when the official pane API is available", async () => {
    vi.mocked(herdrTerminalOpen).mockResolvedValue({
      sessionId: "sess-pane",
      target: "t1",
      mode: "control",
      role: "controller",
      cols: 80,
      rows: 24,
      takeover: true
    })
    vi.mocked(readPaneScroll).mockResolvedValue({
      offsetFromBottom: 4,
      maxOffsetFromBottom: 20,
      viewportRows: 24
    })
    vi.mocked(setPaneScroll).mockResolvedValue(null)

    const transport = createHerdrTerminalTransport({
      terminalId: "t1",
      paneId: "pane-1",
      sessionName: "[wsl-host,default]",
      paneScrollEnabled: () => true
    })
    await transport.open({ cols: 80, rows: 24, onEvent: () => undefined })
    await transport.scroll?.(-3)

    expect(herdrTerminalScroll).not.toHaveBeenCalled()
    expect(readPaneScroll).toHaveBeenCalledWith("[wsl-host,default]", "pane-1")
    expect(setPaneScroll).toHaveBeenCalledWith("[wsl-host,default]", "pane-1", 7)
  })

  it("coalesces concurrent wheel requests into one remote scroll at a time", async () => {
    vi.mocked(herdrTerminalOpen).mockResolvedValue({
      sessionId: "sess-burst",
      target: "t1",
      mode: "control",
      role: "controller",
      cols: 80,
      rows: 24,
      takeover: true
    })
    let release!: () => void
    vi.mocked(herdrTerminalScroll).mockImplementationOnce(
      () => new Promise<void>((resolve) => { release = resolve })
    ).mockResolvedValue(undefined)
    const transport = createHerdrTerminalTransport({ terminalId: "t1" })
    await transport.open({ cols: 80, rows: 24, onEvent: () => undefined })

    const first = transport.scroll?.(-2)
    const second = transport.scroll?.(-3)
    expect(herdrTerminalScroll).toHaveBeenCalledTimes(1)
    release()
    await Promise.all([first, second])

    expect(herdrTerminalScroll).toHaveBeenCalledTimes(2)
    expect(herdrTerminalScroll).toHaveBeenLastCalledWith("sess-burst", "up", 3)
  })
})


  it("does not mutate server scrollback while observing", async () => {
    vi.mocked(herdrTerminalScroll).mockReset()
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

describe("bounded Herdr input delivery", () => {
  beforeEach(() => {
    vi.mocked(herdrTerminalOpen).mockReset().mockResolvedValue({
      sessionId: "input-session", target: "input-terminal", mode: "control",
      role: "controller", cols: 80, rows: 24, takeover: true
    })
    vi.mocked(herdrTerminalInput).mockReset().mockResolvedValue(undefined)
    vi.mocked(herdrTerminalRelease).mockReset().mockResolvedValue(undefined)
  })
  function delayed() {
    let resolve!: () => void
    let reject!: (error: Error) => void
    const promise = new Promise<void>((done, fail) => { resolve = done; reject = fail })
    return { promise, resolve, reject }
  }
  async function open() {
    const transport = createHerdrTerminalTransport({ terminalId: "input-terminal" })
    const onEvent = vi.fn()
    await transport.open({ cols: 80, rows: 24, onEvent })
    return { transport, onEvent }
  }

  it("coalesces a typing burst behind one in-flight request without dropping or reordering text", async () => {
    const first = delayed()
    vi.mocked(herdrTerminalInput).mockReturnValueOnce(first.promise)
    const { transport } = await open()
    const head = transport.write("node ")
    await vi.waitFor(() => expect(herdrTerminalInput).toHaveBeenCalledOnce())
    const text = "/home/ubuntu/中文 workspace/" + "path/".repeat(100) + "server.cjs\r"
    const tail = [...text].map((character) => transport.write(character))
    expect(herdrTerminalInput).toHaveBeenCalledOnce()
    first.resolve()
    await Promise.all([head, ...tail])
    expect(herdrTerminalInput).toHaveBeenCalledTimes(2)
    expect(vi.mocked(herdrTerminalInput).mock.calls.map((args) => args[1]).join("")).toBe("node " + text)
  })

  it("surfaces an uncertain delivery and never sends the queued Enter", async () => {
    const first = delayed()
    vi.mocked(herdrTerminalInput).mockReturnValueOnce(first.promise)
    const { transport, onEvent } = await open()
    const head = transport.write("pending command")
    await vi.waitFor(() => expect(herdrTerminalInput).toHaveBeenCalledOnce())
    const tail = transport.write("\r")
    const settled = Promise.allSettled([head, tail])
    first.reject(new Error("stream-response-unavailable"))
    expect((await settled).every((result) => result.status === "rejected")).toBe(true)
    expect(herdrTerminalInput).toHaveBeenCalledOnce()
    expect(transport.canWrite()).toBe(false)
    expect(onEvent).toHaveBeenCalledWith({ type: "error", message: "terminal-input-failed" })
    await transport.write("ignored")
    expect(herdrTerminalInput).toHaveBeenCalledOnce()
  })

  it("discards old unsent input and ignores a late failure after opening another connector", async () => {
    const first = delayed()
    vi.mocked(herdrTerminalInput).mockReturnValueOnce(first.promise)
    const { transport, onEvent } = await open()
    const head = transport.write("old command")
    await vi.waitFor(() => expect(herdrTerminalInput).toHaveBeenCalledOnce())
    const tail = transport.write("\r")
    const old = Promise.allSettled([head, tail])
    await transport.release()
    vi.mocked(herdrTerminalOpen).mockResolvedValueOnce({
      sessionId: "new-session", target: "input-terminal", mode: "control",
      role: "controller", cols: 80, rows: 24, takeover: true
    })
    await transport.open({ cols: 80, rows: 24, onEvent })
    await transport.write("new command")
    first.reject(new Error("old connection closed"))
    await old
    expect(herdrTerminalInput).toHaveBeenCalledTimes(2)
    expect(herdrTerminalInput).toHaveBeenLastCalledWith("new-session", "new command", null)
    expect(transport.canWrite()).toBe(true)
    expect(onEvent).not.toHaveBeenCalledWith(expect.objectContaining({ type: "error" }))
  })

  it("bounds UTF-8 bytes and discards the unsent tail on overflow", async () => {
    const first = delayed()
    vi.mocked(herdrTerminalInput).mockReturnValueOnce(first.promise)
    const { transport, onEvent } = await open()
    const head = transport.write("pending")
    await vi.waitFor(() => expect(herdrTerminalInput).toHaveBeenCalledOnce())
    const tail = transport.write("\r")
    await expect(transport.write("中".repeat(90_000))).rejects.toThrow("terminal-input-limit")
    expect(transport.canWrite()).toBe(false)
    expect(onEvent).toHaveBeenCalledWith({ type: "error", message: "terminal-input-limit" })
    first.resolve()
    await Promise.all([head, tail])
    expect(herdrTerminalInput).toHaveBeenCalledOnce()
  })

  it("drops input when the connector is disposed before its first send", async () => {
    const { transport } = await open()
    const pending = transport.write("never execute\r")
    await transport.dispose?.()
    await pending
    expect(herdrTerminalInput).not.toHaveBeenCalled()
  })
})
