import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

vi.mock("@/lib/herdrIpc", () => ({
  herdrTerminalOpen: vi.fn(),
  herdrTerminalInput: vi.fn(),
  herdrTerminalResize: vi.fn(),
  herdrTerminalScroll: vi.fn(),
  herdrTerminalMouse: vi.fn(),
  herdrTerminalRelease: vi.fn()
}))
vi.mock("./herdrScrollIpc", () => ({
  readPaneScroll: vi.fn(),
  setPaneScroll: vi.fn()
}))

import {
  herdrTerminalInput,
  herdrTerminalMouse,
  herdrTerminalOpen,
  herdrTerminalRelease,
  herdrTerminalResize,
  herdrTerminalScroll
} from "@/lib/herdrIpc"
import type { HerdrTerminalEvent } from "@/lib/herdrTypes"
import {
  createHerdrTerminalTransport,
  normalizeTerminalWheelRows,
  terminalWheelCell,
  type TerminalTransportEvent
} from "./terminalTransport"
import { createPaneScrollController } from "./herdrScrollController"
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

describe("Herdr frame decoding", () => {
  it.each([0, -1, 1001, 2 ** 32, Number.NaN, 1.5])("rejects geometry %s before rendering and releases the connector", async width => {
    vi.mocked(herdrTerminalRelease).mockResolvedValue(undefined)
    vi.mocked(herdrTerminalOpen).mockResolvedValue({ sessionId: "geometry-session", target: "t1", mode: "control", role: "controller", takeover: true, cols: 80, rows: 24 })
    const event = vi.fn()
    const transport = createHerdrTerminalTransport({ terminalId: "t1" })
    await transport.open({ cols: 80, rows: 24, onEvent: event })
    const { onEvent } = vi.mocked(herdrTerminalOpen).mock.calls.at(-1)![0]
    onEvent({ type: "frame", sessionId: "geometry-session", seq: 1, full: true, encoding: "ansi", width, height: 24, bytesBase64: "" })
    expect(event).toHaveBeenCalledWith(expect.objectContaining({ type: "error", code: "invalid-terminal-geometry" }))
    expect(event.mock.calls.some(([value]) => value.type === "output")).toBe(false)
    expect(herdrTerminalRelease).toHaveBeenCalledWith("geometry-session")
  })
  afterEach(() => {
    vi.unstubAllGlobals()
    vi.resetModules()
  })

  async function decodeFrames(payloads: string[], createTransport = createHerdrTerminalTransport) {
    vi.mocked(herdrTerminalOpen).mockResolvedValue({
      sessionId: "decode-session", target: "t1", mode: "control", role: "controller",
      takeover: true, cols: 80, rows: 24
    })
    const output: string[] = []
    const transport = createTransport({ terminalId: "t1" })
    await transport.open({ cols: 80, rows: 24, onEvent: (event) => {
      if (event.type === "output") output.push(event.data)
    } })
    const { onEvent } = vi.mocked(herdrTerminalOpen).mock.calls.at(-1)![0]
    payloads.forEach((bytesBase64, index) => onEvent({
      type: "frame", sessionId: "decode-session", seq: index + 1, full: true,
      encoding: "ansi", width: 80, height: 24, bytesBase64
    }))
    return output
  }

  it.each(["native", "atob"])("decodes ASCII, UTF-8, invalid sequences and empty frames via %s", async (path) => {
    const fromBase64 = vi.fn((value: string) => Uint8Array.from(atob(value), (char) => char.charCodeAt(0)))
    class FrameBytes extends Uint8Array {}
    Object.defineProperty(FrameBytes, "fromBase64", { value: path === "native" ? fromBase64 : undefined })
    vi.stubGlobal("Uint8Array", FrameBytes)
    const utf8 = (text: string) => btoa(String.fromCharCode(...new TextEncoder().encode(text)))
    const payloads = [b64("hello\x1b[0m"), utf8("中文 😀"), b64("\xe4\xb8"), b64("\x80"), ""]
    expect(await decodeFrames(payloads)).toEqual(["hello\x1b[0m", "中文 😀", "�", "�", ""])
    expect(fromBase64).toHaveBeenCalledTimes(path === "native" ? payloads.length : 0)
  })

  it("uses native base64 decoding without atob", async () => {
    class FrameBytes extends Uint8Array {
      static fromBase64 = vi.fn(() => new Uint8Array([0xe4, 0xb8, 0xad]))
    }
    vi.stubGlobal("Uint8Array", FrameBytes)
    vi.stubGlobal("atob", undefined)
    expect(await decodeFrames(["5Lit"])).toEqual(["中"])
    expect(FrameBytes.fromBase64).toHaveBeenCalledWith("5Lit")
  })

  it("keeps the byte-string fallback when TextDecoder is unavailable", async () => {
    vi.stubGlobal("TextDecoder", undefined)
    vi.resetModules()
    const { createHerdrTerminalTransport: createTransport } = await import("./terminalTransport")
    expect(await decodeFrames([b64("ASCII"), b64("\xe4\xb8\xad"), ""], createTransport))
      .toEqual(["ASCII", "\xe4\xb8\xad", ""])
  })

  it("returns the original payload when base64 decoding fails", async () => {
    expect(await decodeFrames(["!invalid-base64!"])).toEqual(["!invalid-base64!"])
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

  it("retires a dead remote stream as a connector loss, not a pane exit", async () => {
    let handleEvent: ((event: HerdrTerminalEvent) => void) | undefined
    vi.mocked(herdrTerminalOpen).mockImplementation(async (args) => {
      handleEvent = args.onEvent
      return { sessionId: "sess-remote", target: args.target, mode: "control", role: "controller", cols: args.cols, rows: args.rows, takeover: true }
    })
    const events: TerminalTransportEvent[] = []
    const transport = createHerdrTerminalTransport({ terminalId: "t-remote" })
    await transport.open({ cols: 80, rows: 24, onEvent: (event) => events.push(event) })

    handleEvent?.({ type: "error", sessionId: "sess-remote", code: "host-stream-closed", message: "stream-request-timeout" })

    expect(events).toContainEqual({ type: "error", message: "stream-request-timeout", code: "host-stream-closed" })
    expect(events.some((event) => event.type === "exit")).toBe(false)
    expect(transport.getSessionId?.()).toBeNull()
    expect(transport.canWrite()).toBe(false)
  })

  it("keeps routing wheels to HERDR when a remote stream is momentarily busy", async () => {
    vi.mocked(herdrTerminalOpen).mockResolvedValue({
      sessionId: "sess-busy", target: "t1", mode: "control", role: "controller", cols: 80, rows: 24, takeover: true
    })
    let reject!: (error: Error) => void
    vi.mocked(herdrTerminalScroll)
      .mockReturnValueOnce(new Promise<void>((_, fail) => { reject = fail }))
      .mockResolvedValue(undefined)
    const transport = createHerdrTerminalTransport({ terminalId: "t1", applicationWheelEnabled: () => true })
    await transport.open({ cols: 80, rows: 24, onEvent: () => undefined })

    const first = transport.scroll?.(-1, { column: 1, row: 1 })
    await vi.waitFor(() => expect(herdrTerminalScroll).toHaveBeenCalledOnce())
    // Wheels queued behind the busy request are dropped with it.
    for (let notch = 0; notch < 3; notch++) void transport.scroll?.(-1, { column: 1, row: 1 })
    reject(new Error("stream-closed-or-busy"))
    await expect(first).resolves.toBeUndefined()

    await transport.scroll?.(-1, { column: 7, row: 2 })
    expect(herdrTerminalScroll).toHaveBeenCalledTimes(2)
    expect(herdrTerminalScroll).toHaveBeenLastCalledWith("sess-busy", "up", 1, { column: 7, row: 2 })
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
      sessionName: "default",
      paneScrollEnabled: () => true
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

  it("keeps native scrolling on the connector fast path when pane metrics are available", async () => {
    vi.mocked(herdrTerminalOpen).mockResolvedValue({
      sessionId: "sess-native",
      target: "t1",
      mode: "control",
      role: "controller",
      cols: 80,
      rows: 24,
      takeover: true
    })
    vi.mocked(herdrTerminalScroll).mockResolvedValue(undefined)

    const transport = createHerdrTerminalTransport({
      terminalId: "t1",
      paneId: "pane-1",
      sessionName: "[native,default]",
      paneScrollEnabled: () => true,
      terminalScrollEnabled: () => true
    })
    await transport.open({ cols: 80, rows: 24, onEvent: () => undefined })
    await transport.scroll?.(-3)

    expect(herdrTerminalScroll).toHaveBeenCalledWith("sess-native", "up", 3)
    expect(readPaneScroll).not.toHaveBeenCalled()
    expect(setPaneScroll).not.toHaveBeenCalled()
  })

  it("reuses a short-lived pane snapshot during wheel bursts", async () => {
    vi.mocked(herdrTerminalOpen).mockResolvedValue({
      sessionId: "sess-pane-cache",
      target: "t1",
      mode: "control",
      role: "controller",
      cols: 80,
      rows: 24,
      takeover: true
    })
    vi.mocked(readPaneScroll).mockResolvedValue({
      offsetFromBottom: 10,
      maxOffsetFromBottom: 40,
      viewportRows: 24
    })
    vi.mocked(setPaneScroll).mockImplementation(async (_session, _pane, offset) => ({
      offsetFromBottom: offset,
      maxOffsetFromBottom: 40,
      viewportRows: 24
    }))

    const onPaneScroll = vi.fn()
    const transport = createHerdrTerminalTransport({
      terminalId: "t1",
      paneId: "pane-1",
      sessionName: "default",
      paneScrollEnabled: () => true,
      onPaneScroll
    })
    await transport.open({ cols: 80, rows: 24, onEvent: () => undefined })
    await transport.scroll?.(-2)
    await transport.scroll?.(-2)
    await transport.scroll?.(-2)

    expect(readPaneScroll).toHaveBeenCalledOnce()
    expect(setPaneScroll).toHaveBeenNthCalledWith(1, "default", "pane-1", 12)
    expect(setPaneScroll).toHaveBeenNthCalledWith(3, "default", "pane-1", 16)
    expect(onPaneScroll).toHaveBeenLastCalledWith({ offsetFromBottom: 16, maxOffsetFromBottom: 40, viewportRows: 24 })
  })

  it("falls back to terminal.scroll when pane metadata is temporarily unavailable", async () => {
    vi.mocked(herdrTerminalOpen).mockResolvedValue({
      sessionId: "sess-pane-null",
      target: "t1",
      mode: "control",
      role: "controller",
      cols: 80,
      rows: 24,
      takeover: true
    })
    vi.mocked(readPaneScroll).mockResolvedValue(null)
    vi.mocked(herdrTerminalScroll).mockResolvedValue(undefined)

    const transport = createHerdrTerminalTransport({
      terminalId: "t1",
      paneId: "pane-1",
      sessionName: "default",
      paneScrollEnabled: () => true
    })
    await transport.open({ cols: 80, rows: 24, onEvent: () => undefined })
    await transport.scroll?.(-3)

    expect(herdrTerminalScroll).toHaveBeenCalledWith("sess-pane-null", "up", 3)
    expect(setPaneScroll).not.toHaveBeenCalled()
  })

  it("keeps pane-only runtimes safe when the pane probe has no range yet", async () => {
    vi.mocked(herdrTerminalOpen).mockResolvedValue({
      sessionId: "sess-pane-probe",
      target: "t1",
      mode: "control",
      role: "controller",
      cols: 80,
      rows: 24,
      takeover: true
    })
    vi.mocked(readPaneScroll).mockResolvedValue(null)
    const transport = createHerdrTerminalTransport({
      terminalId: "t1",
      paneId: "pane-1",
      sessionName: "[wsl:Debian,default]",
      paneScrollEnabled: () => true,
      scrollEnabled: () => true,
      terminalScrollEnabled: () => false
    })
    await transport.open({ cols: 80, rows: 24, onEvent: () => undefined })
    await expect(transport.scroll?.(-3)).rejects.toThrow("pane-scroll-state-unavailable")

    expect(herdrTerminalScroll).not.toHaveBeenCalled()
    expect(readPaneScroll).toHaveBeenCalledWith("[wsl:Debian,default]", "pane-1")
  })

  it("does not call an unverified scroll transport", async () => {
    vi.mocked(herdrTerminalOpen).mockResolvedValue({
      sessionId: "sess-no-scroll",
      target: "t1",
      mode: "control",
      role: "controller",
      cols: 80,
      rows: 24,
      takeover: true
    })
    const transport = createHerdrTerminalTransport({
      terminalId: "t1",
      scrollEnabled: () => false
    })
    await transport.open({ cols: 80, rows: 24, onEvent: () => undefined })
    await transport.scroll?.(-3)

    expect(herdrTerminalScroll).not.toHaveBeenCalled()
    expect(readPaneScroll).not.toHaveBeenCalled()
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

  it("paces connector scrolls by rendered frames instead of IPC acknowledgements", async () => {
    // macOS debug log: 274 wheel events in 5 s became 270 terminal.scroll
    // commands (IPC ack p50 1 ms) but only 141 frames, so the viewport kept
    // replaying a backlog after the wheel stopped.
    let emit!: (event: HerdrTerminalEvent) => void
    vi.mocked(herdrTerminalOpen).mockImplementation(async (args) => {
      emit = args.onEvent
      return { sessionId: "sess-paced", target: "t1", mode: "control", role: "controller", cols: 80, rows: 24, takeover: true }
    })
    vi.mocked(herdrTerminalScroll).mockResolvedValue(undefined)
    const transport = createHerdrTerminalTransport({ terminalId: "t1" })
    await transport.open({ cols: 80, rows: 24, onEvent: () => undefined })
    const frame = (seq: number) => emit({
      type: "frame", sessionId: "sess-paced", seq, full: seq === 1, encoding: "ansi", width: 80, height: 24, bytesBase64: ""
    })
    frame(1)
    const settle = () => new Promise((resolve) => setTimeout(resolve, 5))

    const drains = [transport.scroll?.(-1)]
    for (let i = 0; i < 4; i++) {
      await settle()
      drains.push(transport.scroll?.(-1))
    }
    expect(herdrTerminalScroll).toHaveBeenCalledTimes(1)

    frame(2)
    await settle()
    expect(herdrTerminalScroll).toHaveBeenCalledTimes(2)
    expect(herdrTerminalScroll).toHaveBeenLastCalledWith("sess-paced", "up", 4)
    frame(3)
    await Promise.all(drains)
  })

  it("does not stall a connector scroll when its frame never arrives", async () => {
    vi.mocked(herdrTerminalOpen).mockResolvedValue({ sessionId: "sess-edge", target: "t1", mode: "control", role: "controller", cols: 80, rows: 24, takeover: true })
    vi.mocked(herdrTerminalScroll).mockResolvedValue(undefined)
    const transport = createHerdrTerminalTransport({ terminalId: "t1" })
    await transport.open({ cols: 80, rows: 24, onEvent: () => undefined })

    // Already at the top: Herdr has nothing to redraw, so no frame follows.
    const first = transport.scroll?.(-1)
    await new Promise((resolve) => setTimeout(resolve, 5))
    const second = transport.scroll?.(-2)
    await Promise.all([first, second])

    expect(herdrTerminalScroll).toHaveBeenCalledTimes(2)
    expect(herdrTerminalScroll).toHaveBeenLastCalledWith("sess-edge", "up", 2)
  })

  it("does not write a delayed pane scroll after the session is detached", async () => {
    vi.mocked(herdrTerminalOpen).mockResolvedValue({
      sessionId: "sess-delayed",
      target: "t1",
      mode: "control",
      role: "controller",
      cols: 80,
      rows: 24,
      takeover: true
    })
    let resolveRead!: (state: { offsetFromBottom: number; maxOffsetFromBottom: number; viewportRows: number }) => void
    vi.mocked(readPaneScroll).mockImplementation(
      () => new Promise((resolve) => { resolveRead = resolve })
    )
    vi.mocked(setPaneScroll).mockResolvedValue(null)
    const transport = createHerdrTerminalTransport({
      terminalId: "t1",
      paneId: "pane-1",
      sessionName: "[wsl-host,default]",
      paneScrollEnabled: () => true
    })
    await transport.open({ cols: 80, rows: 24, onEvent: () => undefined })
    const pending = transport.scroll?.(-3)
    await Promise.resolve()
    transport.detachSession?.()
    resolveRead({ offsetFromBottom: 4, maxOffsetFromBottom: 20, viewportRows: 24 })
    await pending

    expect(setPaneScroll).not.toHaveBeenCalled()
  })

  it("drops stale wheel delta after a scroll error", async () => {
    vi.mocked(herdrTerminalOpen).mockResolvedValue({
      sessionId: "sess-error",
      target: "t1",
      mode: "control",
      role: "controller",
      cols: 80,
      rows: 24,
      takeover: true
    })
    vi.mocked(herdrTerminalScroll).mockRejectedValueOnce(new Error("scroll unavailable"))
    const transport = createHerdrTerminalTransport({ terminalId: "t1" })
    await transport.open({ cols: 80, rows: 24, onEvent: () => undefined })
    const failed = transport.scroll?.(-3)?.catch((error) => error)
    const queued = transport.scroll?.(-2)?.catch((error) => error)
    await Promise.all([failed, queued])
    await transport.scroll?.(-1)

    expect(herdrTerminalScroll).toHaveBeenCalledOnce()
  })

  it("does not retry a rejected terminal scroll on the same attachment", async () => {
    vi.mocked(herdrTerminalOpen).mockResolvedValue({
      sessionId: "sess-breaker",
      target: "t1",
      mode: "control",
      role: "controller",
      cols: 80,
      rows: 24,
      takeover: true
    })
    vi.mocked(herdrTerminalScroll).mockRejectedValue(new Error("connector closed"))
    const transport = createHerdrTerminalTransport({ terminalId: "t1" })
    await transport.open({ cols: 80, rows: 24, onEvent: () => undefined })

    await expect(transport.scroll?.(-1)).rejects.toThrow("connector closed")
    await transport.scroll?.(-1)
    expect(herdrTerminalScroll).toHaveBeenCalledOnce()
  })

  it("ignores fractional scroll deltas without poisoning the drain", async () => {
    vi.mocked(herdrTerminalOpen).mockResolvedValue({
      sessionId: "sess-fraction",
      target: "t1",
      mode: "control",
      role: "controller",
      cols: 80,
      rows: 24,
      takeover: true
    })
    vi.mocked(herdrTerminalScroll).mockResolvedValue(undefined)
    const transport = createHerdrTerminalTransport({ terminalId: "t1" })
    await transport.open({ cols: 80, rows: 24, onEvent: () => undefined })
    await transport.scroll?.(-0.5)
    await transport.scroll?.(-2)

    expect(herdrTerminalScroll).toHaveBeenCalledWith("sess-fraction", "up", 2)
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

  it("counts ASCII keystrokes without encoding and encodes larger or Unicode inputs once", async () => {
    const { transport } = await open()
    const encode = vi.spyOn(TextEncoder.prototype, "encode")
    try {
      await Promise.all([transport.write("中"), transport.write("😀"), transport.paste("文"), transport.write("a"), transport.write("end")])
      expect(encode.mock.calls.map(([text]) => text)).toEqual(["中", "😀", "\x1b[200~文\x1b[201~", "end"])
      expect(vi.mocked(herdrTerminalInput).mock.calls.map(([, text]) => text))
        .toEqual(["中😀", "\x1b[200~文\x1b[201~", "aend"])
    } finally {
      encode.mockRestore()
    }
  })

  it("reclaims all merged UTF-8 bytes on dequeue while excluding the in-flight frame", async () => {
    const first = delayed()
    const second = delayed()
    vi.mocked(herdrTerminalInput).mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise)
    const { transport, onEvent } = await open()
    const head = transport.write("pending")
    await vi.waitFor(() => expect(herdrTerminalInput).toHaveBeenCalledOnce())
    const cjk = "中".repeat(87_380) // 262140 bytes; emoji fills the 256 KiB queue.
    const tail = [transport.write(cjk), transport.write("😀")]
    first.resolve()
    await vi.waitFor(() => expect(herdrTerminalInput).toHaveBeenCalledTimes(2))
    const next = transport.write("a".repeat(256 * 1024))
    second.resolve()
    await Promise.all([head, ...tail, next])
    expect(herdrTerminalInput).toHaveBeenCalledTimes(3)
    expect(vi.mocked(herdrTerminalInput).mock.calls[1][1]).toBe(cjk + "😀")
    expect(onEvent).not.toHaveBeenCalledWith(expect.objectContaining({ type: "error" }))
  })

  it.each([false, true])("counts paste brackets at the exact byte limit (overflow=%s)", async (overflow) => {
    const { transport, onEvent } = await open()
    const payload = "a".repeat(256 * 1024 - 12 + Number(overflow))
    if (overflow) {
      await expect(transport.paste(payload)).rejects.toThrow("terminal-input-limit")
      expect(herdrTerminalInput).not.toHaveBeenCalled()
      expect(onEvent).toHaveBeenCalledWith({ type: "error", message: "terminal-input-limit" })
    } else {
      await transport.paste(payload)
      expect(herdrTerminalInput).toHaveBeenCalledWith("input-session", "\x1b[200~" + payload + "\x1b[201~", null)
    }
  })

  it("rejects one byte beyond a merged UTF-8 queue limit", async () => {
    const first = delayed()
    vi.mocked(herdrTerminalInput).mockReturnValueOnce(first.promise)
    const { transport } = await open()
    const head = transport.write("pending")
    await vi.waitFor(() => expect(herdrTerminalInput).toHaveBeenCalledOnce())
    const tail = [transport.write("中".repeat(87_380)), transport.write("😀")]
    await expect(transport.write("x")).rejects.toThrow("terminal-input-limit")
    first.resolve()
    await Promise.all([head, ...tail])
    expect(herdrTerminalInput).toHaveBeenCalledOnce()
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

describe("terminalWheelCell", () => {
  it("maps the pointer to a zero-based cell and clamps to the grid", () => {
    const screen = { left: 10, top: 20, width: 800, height: 480 }
    expect(terminalWheelCell(screen, 100, 30, 10, 20)).toEqual({ column: 0, row: 0 })
    expect(terminalWheelCell(screen, 100, 30, 10 + 8 * 10.5, 20 + 16 * 5.5)).toEqual({ column: 10, row: 5 })
    expect(terminalWheelCell(screen, 100, 30, 5000, 5000)).toEqual({ column: 99, row: 29 })
    expect(terminalWheelCell(screen, 100, 30, -5, -5)).toEqual({ column: 0, row: 0 })
    expect(terminalWheelCell({ left: 0, top: 0, width: 0, height: 0 }, 100, 30, 1, 1)).toBeUndefined()
    expect(terminalWheelCell(screen, 100, 30, Number.NaN, 1)).toBeUndefined()
  })
})

// Claude Code fullscreen (and vim/less) runs on the alternate screen, where
// HERDR reports no host scrollback. `pane.scroll` cannot move it; only the
// connector command lets HERDR route the wheel to the application.
describe("alternate-screen wheel routing", () => {
  const altScreen = { offsetFromBottom: 0, maxOffsetFromBottom: 0, viewportRows: 30 }
  const history = { offsetFromBottom: 0, maxOffsetFromBottom: 172, viewportRows: 30 }
  const cell = { column: 10, row: 5 }

  beforeEach(() => {
    vi.mocked(herdrTerminalOpen).mockReset().mockResolvedValue({
      sessionId: "sess-alt", target: "t1", mode: "control", role: "controller", cols: 100, rows: 30, takeover: true
    })
    vi.mocked(herdrTerminalScroll).mockReset().mockResolvedValue(undefined)
    vi.mocked(readPaneScroll).mockReset()
    vi.mocked(setPaneScroll).mockReset()
  })

  async function openWithController(state: typeof altScreen, options: {
    terminalScrollEnabled: () => boolean
    applicationWheelEnabled: () => boolean
  }) {
    const write = vi.fn(async (offset: number) => ({ ...state, offsetFromBottom: offset }))
    const shared = createPaneScrollController({ read: async () => state, write, allowed: () => true, change: () => undefined })
    const transport = createHerdrTerminalTransport({
      terminalId: "t1",
      paneId: "pane-1",
      sessionName: "default",
      paneScrollEnabled: () => true,
      paneScrollController: () => shared,
      ...options
    })
    await transport.open({ cols: 100, rows: 30, onEvent: () => undefined })
    // Opening resets the shared controller; the first frame's refresh restores it.
    await shared.refresh()
    return { shared, transport, write }
  }

  it("sends a native fullscreen wheel to the application with its pointer cell", async () => {
    const { transport, write } = await openWithController(altScreen, {
      terminalScrollEnabled: () => false,
      applicationWheelEnabled: () => true
    })
    await transport.scroll?.(-3, cell)

    expect(herdrTerminalScroll).toHaveBeenCalledWith("sess-alt", "up", 3, cell)
    expect(write).not.toHaveBeenCalled()
    expect(readPaneScroll).not.toHaveBeenCalled()
    expect(setPaneScroll).not.toHaveBeenCalled()
  })

  it("lets HERDR route a wheel over host scrollback to a mouse-reporting application", async () => {
    // A normal-buffer TUI can enable mouse reporting while the pane still has
    // host history. Only HERDR knows the child modes; pane.scroll would move
    // the host viewport and the application would never see the wheel.
    const { shared, transport, write } = await openWithController(history, {
      terminalScrollEnabled: () => false,
      applicationWheelEnabled: () => true
    })
    await transport.scroll?.(-3, cell)

    expect(herdrTerminalScroll).toHaveBeenCalledWith("sess-alt", "up", 3, cell)
    expect(write).not.toHaveBeenCalled()
    expect(setPaneScroll).not.toHaveBeenCalled()
    shared.dispose()
  })

  it("asks the scrollbar to follow frames produced by a connector wheel", async () => {
    const { shared, transport } = await openWithController(history, {
      terminalScrollEnabled: () => false,
      applicationWheelEnabled: () => true
    })
    const follow = vi.spyOn(shared, "follow")
    await transport.scroll?.(-3, cell)

    expect(follow).toHaveBeenCalledOnce()
    shared.dispose()
  })

  it("keeps host scrollback on the pane path where the runtime forbids the connector command", async () => {
    const { shared, transport, write } = await openWithController(history, {
      terminalScrollEnabled: () => false,
      applicationWheelEnabled: () => false
    })
    const follow = vi.spyOn(shared, "follow")
    await transport.scroll?.(-3, cell)

    await vi.waitFor(() => expect(write).toHaveBeenCalledWith(3, expect.anything()))
    expect(herdrTerminalScroll).not.toHaveBeenCalled()
    // The pane path already publishes an optimistic position.
    expect(follow).not.toHaveBeenCalled()
    shared.dispose()
  })

  it("moves host scrollback through the pane path after the connector rejects", async () => {
    vi.mocked(herdrTerminalScroll).mockRejectedValueOnce(new Error("connector rejected"))
    vi.mocked(readPaneScroll).mockResolvedValue(history)
    vi.mocked(setPaneScroll).mockResolvedValue({ ...history, offsetFromBottom: 3 })
    const { shared, transport, write } = await openWithController(history, {
      terminalScrollEnabled: () => false,
      applicationWheelEnabled: () => true
    })
    await transport.scroll?.(-3, cell)
    await transport.scroll?.(-3, cell)

    expect(herdrTerminalScroll).toHaveBeenCalledOnce()
    await vi.waitFor(() => expect(write).toHaveBeenCalled())
    shared.dispose()
  })

  it("routes a protocol-22 WSL fullscreen wheel through the connector", async () => {
    // WSL prefers the pane API for the scrollbar; protocol 22 wheels go to HERDR.
    const { transport } = await openWithController(altScreen, {
      terminalScrollEnabled: () => false,
      applicationWheelEnabled: () => true
    })
    await transport.scroll?.(2, cell)

    expect(herdrTerminalScroll).toHaveBeenCalledWith("sess-alt", "down", 2, cell)
  })

  it("never sends the connector command where the runtime forbids it", async () => {
    const { transport } = await openWithController(altScreen, {
      terminalScrollEnabled: () => false,
      applicationWheelEnabled: () => false
    })
    await transport.scroll?.(-3, cell)

    expect(herdrTerminalScroll).not.toHaveBeenCalled()
  })

  it("stops routing to the connector after it rejects on this attachment", async () => {
    vi.mocked(herdrTerminalScroll).mockRejectedValueOnce(new Error("connector closed"))
    vi.mocked(readPaneScroll).mockResolvedValue(altScreen)
    vi.mocked(setPaneScroll).mockResolvedValue(altScreen)
    const { transport } = await openWithController(altScreen, {
      terminalScrollEnabled: () => false,
      applicationWheelEnabled: () => true
    })
    await transport.scroll?.(-3, cell)
    await transport.scroll?.(-3, cell)

    expect(herdrTerminalScroll).toHaveBeenCalledOnce()
  })

  it("routes a standalone wheel to HERDR without probing the pane range first", async () => {
    vi.mocked(readPaneScroll).mockResolvedValue(history)
    const transport = createHerdrTerminalTransport({
      terminalId: "t1", paneId: "pane-1", sessionName: "default",
      paneScrollEnabled: () => true,
      terminalScrollEnabled: () => false,
      applicationWheelEnabled: () => true
    })
    await transport.open({ cols: 100, rows: 30, onEvent: () => undefined })
    await transport.scroll?.(-3, cell)

    expect(readPaneScroll).not.toHaveBeenCalled()
    expect(setPaneScroll).not.toHaveBeenCalled()
    expect(herdrTerminalScroll).toHaveBeenCalledWith("sess-alt", "up", 3, cell)
  })

  it("routes the first wheel after attach before pane metadata arrives", async () => {
    // transport.open() resets the shared controller; its first read is still pending.
    const write = vi.fn()
    const shared = createPaneScrollController({ read: () => new Promise(() => undefined), write, allowed: () => true, change: () => undefined })
    const transport = createHerdrTerminalTransport({
      terminalId: "t1", paneId: "pane-1", sessionName: "default",
      paneScrollEnabled: () => true, paneScrollController: () => shared,
      terminalScrollEnabled: () => false, applicationWheelEnabled: () => true
    })
    await transport.open({ cols: 100, rows: 30, onEvent: () => undefined })
    await transport.scroll?.(-3, cell)

    expect(herdrTerminalScroll).toHaveBeenCalledWith("sess-alt", "up", 3, cell)
    expect(write).not.toHaveBeenCalled()
  })

  it("sends one application report per wheel event queued behind an in-flight one", async () => {
    // HERDR ignores `lines` for mouse reports, so coalescing would drop notches.
    let release!: () => void
    vi.mocked(herdrTerminalScroll).mockReturnValueOnce(new Promise<void>((done) => { release = done }))
    const { transport } = await openWithController(altScreen, {
      terminalScrollEnabled: () => false,
      applicationWheelEnabled: () => true
    })
    const first = transport.scroll?.(-1, cell)
    await vi.waitFor(() => expect(herdrTerminalScroll).toHaveBeenCalledOnce())
    for (let notch = 0; notch < 4; notch++) void transport.scroll?.(-1, cell)
    release()
    await first

    const calls = vi.mocked(herdrTerminalScroll).mock.calls
    expect(calls).toHaveLength(5)
    expect(calls.every(([, direction, lines]) => direction === "up" && lines === 1)).toBe(true)
  })

  it("bounds application reports per frame window and keeps the scrolled rows", async () => {
    let release!: () => void
    vi.mocked(herdrTerminalScroll).mockReturnValueOnce(new Promise<void>((done) => { release = done }))
    const { transport } = await openWithController(altScreen, {
      terminalScrollEnabled: () => false,
      applicationWheelEnabled: () => true
    })
    const first = transport.scroll?.(-1, cell)
    await vi.waitFor(() => expect(herdrTerminalScroll).toHaveBeenCalledOnce())
    for (let notch = 0; notch < 8; notch++) void transport.scroll?.(-1, cell)
    release()
    await first

    const burst = vi.mocked(herdrTerminalScroll).mock.calls.slice(1)
    expect(burst).toHaveLength(6)
    expect(burst.reduce((rows, [, , lines]) => rows + lines, 0)).toBe(8)
  })
})

describe("terminal mouse", () => {
  beforeEach(() => {
    vi.mocked(herdrTerminalOpen).mockReset().mockResolvedValue({
      sessionId: "sess-mouse", target: "t1", mode: "control", role: "controller", cols: 80, rows: 24, takeover: true
    })
    vi.mocked(herdrTerminalMouse).mockReset().mockResolvedValue(undefined)
  })

  async function open(mouseEnabled: () => boolean, mode: "control" | "observe" = "control") {
    if (mode === "observe") {
      vi.mocked(herdrTerminalOpen).mockResolvedValue({
        sessionId: "sess-mouse", target: "t1", mode: "observe", role: "observer", cols: 80, rows: 24, takeover: false
      })
    }
    const transport = createHerdrTerminalTransport({ terminalId: "t1", mode, mouseEnabled })
    await transport.open({ cols: 80, rows: 24, onEvent: () => undefined })
    return transport
  }

  it("sends one event at a time in order and keeps only the latest queued drag", async () => {
    let release!: () => void
    vi.mocked(herdrTerminalMouse).mockReturnValueOnce(new Promise<void>((done) => { release = done }))
    const transport = await open(() => true)
    const first = transport.mouse?.("down", { column: 1, row: 2 }, 0)
    await vi.waitFor(() => expect(herdrTerminalMouse).toHaveBeenCalledOnce())
    void transport.mouse?.("drag", { column: 2, row: 2 }, 0)
    void transport.mouse?.("drag", { column: 3, row: 2 }, 0)
    void transport.mouse?.("up", { column: 3, row: 2 }, 4)
    expect(herdrTerminalMouse).toHaveBeenCalledOnce()
    release()
    await first

    expect(vi.mocked(herdrTerminalMouse).mock.calls).toEqual([
      ["sess-mouse", "down", { column: 1, row: 2 }, 0],
      ["sess-mouse", "drag", { column: 3, row: 2 }, 0],
      ["sess-mouse", "up", { column: 3, row: 2 }, 4]
    ])
  })

  it("never sends to connectors without terminal.mouse or without control", async () => {
    await (await open(() => false)).mouse?.("down", { column: 0, row: 0 }, 0)
    await (await open(() => true, "observe")).mouse?.("down", { column: 0, row: 0 }, 0)
    expect(herdrTerminalMouse).not.toHaveBeenCalled()
  })

  it("drops the rest of a gesture after a failed send instead of replaying it", async () => {
    vi.mocked(herdrTerminalMouse).mockRejectedValueOnce(new Error("busy"))
    const transport = await open(() => true)
    const first = transport.mouse?.("down", { column: 1, row: 1 }, 0)
    void transport.mouse?.("up", { column: 1, row: 1 }, 0)
    await expect(first).resolves.toBeUndefined()
    expect(herdrTerminalMouse).toHaveBeenCalledOnce()

    await transport.mouse?.("down", { column: 4, row: 4 }, 0)
    expect(vi.mocked(herdrTerminalMouse).mock.calls.at(-1)).toEqual(["sess-mouse", "down", { column: 4, row: 4 }, 0])
  })
})
