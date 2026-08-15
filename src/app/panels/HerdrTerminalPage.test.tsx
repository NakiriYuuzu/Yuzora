import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { HerdrTerminalEvent } from "@/lib/herdrTypes"
import { herdrInitialState, useHerdrStore } from "@/state/herdrStore"

const xtermMock = vi.hoisted(() => {
  type DataHandler = (data: string) => void

  const state = {
    terminals: [] as TerminalMock[]
  }

  class TerminalMock {
    options: Record<string, unknown>
    cols = 80
    rows = 24
    dataHandler: DataHandler | null = null
    writeParsedHandler: (() => void) | null = null
    writeParsedDisposable = { dispose: vi.fn() }
    linkProvider: { provideLinks: (y: number, cb: (links: unknown) => void) => void } | null = null
    linkProviderDisposable = { dispose: vi.fn() }
    element = document.createElement("div")
    bufferLines: Array<{ text: string; wrapped?: boolean }> = []
    bufferType: "normal" | "alternate" = "normal"
    get buffer() {
      return {
        active: {
          type: this.bufferType,
          length: this.bufferLines.length,
          getLine: (y: number) => {
            const line = this.bufferLines[y]
            if (!line) return undefined
            return {
              isWrapped: Boolean(line.wrapped),
              length: line.text.length,
              getCell: (x: number) => {
                const char = line.text[x]
                if (char === undefined) return undefined
                return {
                  getChars: () => char,
                  getWidth: () => 1
                }
              },
              translateToString: (trimRight?: boolean) =>
                trimRight ? line.text.replace(/\s+$/, "") : line.text
            }
          }
        }
      }
    }
    open = vi.fn()
    // Mirror xterm: optional onProcessed callback after the write is applied.
    write = vi.fn((_data: string, onProcessed?: () => void) => onProcessed?.())
    focus = vi.fn()
    reset = vi.fn()
    dispose = vi.fn()
    loadAddon = vi.fn((addon: { activate?: (terminal: TerminalMock) => void }) => {
      addon.activate?.(this)
    })
    onWriteParsed = vi.fn((handler: () => void) => {
      this.writeParsedHandler = handler
      return this.writeParsedDisposable
    })
    onData = vi.fn((handler: DataHandler) => {
      this.dataHandler = handler
      return { dispose: vi.fn() }
    })
    customWheelEventHandler: ((event: WheelEvent) => boolean) | null = null
    attachCustomWheelEventHandler = vi.fn((handler: (event: WheelEvent) => boolean) => {
      this.customWheelEventHandler = handler
    })
    registerLinkProvider = vi.fn((provider: { provideLinks: (y: number, cb: (links: unknown) => void) => void }) => {
      this.linkProvider = provider
      return this.linkProviderDisposable
    })

    constructor(options: Record<string, unknown>) {
      this.options = options
      state.terminals.push(this)
    }
  }

  class FitAddonMock {
    terminal: TerminalMock | null = null
    activate = vi.fn((terminal: TerminalMock) => {
      this.terminal = terminal
    })
    dispose = vi.fn()
    fit = vi.fn()
  }

  return {
    state,
    Terminal: TerminalMock,
    FitAddon: FitAddonMock,
    reset() {
      state.terminals = []
    }
  }
})

vi.mock("@xterm/xterm", () => ({
  Terminal: xtermMock.Terminal
}))

vi.mock("@xterm/addon-fit", () => ({
  FitAddon: xtermMock.FitAddon
}))

vi.mock("@/terminal/terminalImeHandling", () => ({
  installTerminalImeHandling: vi.fn(() => ({ dispose: vi.fn() }))
}))

vi.mock("@/terminal/xtermTheme", () => ({
  buildXtermTheme: vi.fn(() => ({}))
}))

const herdrIpcMock = vi.hoisted(() => {
  let onEvent: ((event: HerdrTerminalEvent) => void) | null = null
  return {
    herdrTerminalOpen: vi.fn(
      async (args: {
        onEvent: (event: HerdrTerminalEvent) => void
      }) => {
        onEvent = args.onEvent
        return {
          sessionId: "sess-1",
          target: "term-1",
          mode: (args as { mode?: "observe" | "control" }).mode ?? "control",
          role: ((args as { mode?: "observe" | "control" }).mode ?? "control") === "control"
            ? ("controller" as const)
            : ("observer" as const),
          cols: 80,
          rows: 24,
          takeover: ((args as { mode?: "observe" | "control" }).mode ?? "control") === "control"
        }
      }
    ),
    herdrTerminalInput: vi.fn(),
    herdrTerminalResize: vi.fn(),
    herdrTerminalScroll: vi.fn(),
    herdrTerminalRelease: vi.fn().mockResolvedValue(undefined),
    emit(event: HerdrTerminalEvent) {
      onEvent?.(event)
    },
    reset() {
      onEvent = null
    }
  }
})

vi.mock("@/lib/herdrIpc", () => ({
  herdrTerminalOpen: herdrIpcMock.herdrTerminalOpen,
  herdrTerminalInput: herdrIpcMock.herdrTerminalInput,
  herdrTerminalResize: herdrIpcMock.herdrTerminalResize,
  herdrTerminalScroll: herdrIpcMock.herdrTerminalScroll,
  herdrTerminalRelease: herdrIpcMock.herdrTerminalRelease,
  herdrPaneFocus: vi.fn().mockResolvedValue(undefined),
  herdrLayoutExport: vi.fn(async () => {
    throw new Error("layout unavailable in test")
  }),
  herdrLayoutSetSplitRatio: vi.fn()
}))

const documentMock = vi.hoisted(() => ({
  getDocument: vi.fn()
}))
const feedbackMock = vi.hoisted(() => ({
  showActionError: vi.fn(async (_action?: string, _error?: unknown) => undefined)
}))

vi.mock("@/editor/documentRegistry", () => ({
  getDocument: (path: string) => documentMock.getDocument(path)
}))
vi.mock("@/lib/ipc", () => ({
  isOpenableFile: vi.fn(async () => true)
}))

vi.mock("@/lib/actionFeedback", () => ({
  showActionError: (action: string, error: unknown) => feedbackMock.showActionError(action, error)
}))

import { HerdrTerminalPage } from "./HerdrTerminalPage"
import { isMacPlatform } from "@/lib/platform"
import { useContextMenuStore } from "@/state/contextMenuStore"
import { useWorkspaceStore } from "@/state/workspaceStore"

function seedSessions(
  sessions: Array<{ name: string; default: boolean; running: boolean }>
) {
  useHerdrStore.setState({
    ...herdrInitialState,
    attachments: new Map(),
    sessions: sessions.map((s) => ({
      ...s,
      sessionDir: `/tmp/${s.name}`,
      socketPath: `/tmp/${s.name}.sock`
    }))
  })
}

function replaceSessionInventory(
  sessions: Array<{ name: string; default: boolean; running: boolean }>
) {
  useHerdrStore.setState({
    sessions: sessions.map((s) => ({
      ...s,
      sessionDir: `/tmp/${s.name}`,
      socketPath: `/tmp/${s.name}.sock`
    }))
  })
}

function frame(seq: number, text: string, full = false): HerdrTerminalEvent {
  return {
    type: "frame",
    sessionId: "sess-1",
    seq,
    full,
    encoding: "ansi",
    width: 80,
    height: 24,
    bytesBase64: btoa(text)
  }
}

describe("HerdrTerminalPage TerminalOutputQueue writer contract", () => {
  beforeEach(() => {
    cleanup()
    xtermMock.reset()
    herdrIpcMock.reset()
    herdrIpcMock.herdrTerminalOpen.mockClear()
    herdrIpcMock.herdrTerminalRelease.mockClear()
    seedSessions([{ name: "default", default: true, running: true }])
  })

  afterEach(() => {
    cleanup()
  })

  it("honors onProcessed so two separate flushes both reach xterm", async () => {
    render(
      <HerdrTerminalPage
        herdrSessionId="live"
        terminalId="term-1"
        active
        visible
      />
    )

    await waitFor(() => expect(herdrIpcMock.herdrTerminalOpen).toHaveBeenCalled())
    await waitFor(() => expect(xtermMock.state.terminals.length).toBe(1))

    const term = xtermMock.state.terminals[0]
    // Force the queue to wait on onProcessed for the first flush; if Herdr
    // never calls it, the second push stays stuck behind writing=true.
    let firstProcessed: (() => void) | undefined
    term.write.mockImplementationOnce((_data: string, onProcessed?: () => void) => {
      firstProcessed = onProcessed
    })

    herdrIpcMock.emit(frame(1, "first\n", true))

    await waitFor(() => expect(term.write).toHaveBeenCalledTimes(1))
    expect(term.write.mock.calls[0][0]).toContain("first\n")
    expect(typeof term.write.mock.calls[0][1]).toBe("function")

    // Second push while first write is still in-flight.
    herdrIpcMock.emit(frame(2, "second\n"))
    // Without onProcessed, writing stays true and this never flushes.
    expect(term.write).toHaveBeenCalledTimes(1)

    // Complete the first write — queue should schedule the second flush.
    firstProcessed?.()
    term.write.mockImplementation((_data: string, onProcessed?: () => void) => {
      onProcessed?.()
    })

    await waitFor(() => expect(term.write).toHaveBeenCalledTimes(2))
    expect(term.write.mock.calls[1][0]).toContain("second\n")
    expect(typeof term.write.mock.calls[1][1]).toBe("function")
  })

  it("applies an authoritative full frame atomically without resetting xterm first", async () => {
    render(
      <HerdrTerminalPage
        herdrSessionId="live"
        terminalId="term-1"
        active
        visible
      />
    )

    await waitFor(() => expect(herdrIpcMock.herdrTerminalOpen).toHaveBeenCalledTimes(1))
    const term = xtermMock.state.terminals[0]
    const fullFrame = "\u001b[?2026h\u001b[2J" + "x".repeat(300 * 1024) + "\u001b[?2026l"
    herdrIpcMock.emit(frame(1, fullFrame, true))

    await waitFor(() => expect(term.write).toHaveBeenCalledTimes(1))
    expect(term.reset).not.toHaveBeenCalled()
    expect(term.write.mock.calls[0][0]).toBe(fullFrame)
    expect(term.write.mock.calls[0][0]).not.toContain(
      "[Yuzora: hidden terminal output was truncated]"
    )
  })

  it("does not resize Herdr again when a hidden tab returns at the same dimensions", async () => {
    const { rerender } = render(
      <HerdrTerminalPage
        herdrSessionId="live"
        terminalId="term-1"
        active
        visible
      />
    )

    await waitFor(() => expect(herdrIpcMock.herdrTerminalOpen).toHaveBeenCalledTimes(1))
    await act(async () => {
      await new Promise((resolve) => requestAnimationFrame(resolve))
    })
    herdrIpcMock.herdrTerminalResize.mockClear()

    rerender(
      <HerdrTerminalPage
        herdrSessionId="live"
        terminalId="term-1"
        active={false}
        visible={false}
      />
    )
    rerender(
      <HerdrTerminalPage
        herdrSessionId="live"
        terminalId="term-1"
        active
        visible
      />
    )

    expect(herdrIpcMock.herdrTerminalResize).not.toHaveBeenCalled()
  })

  it("keeps one connector and xterm instance across visible tab switches", async () => {
    const { rerender } = render(
      <HerdrTerminalPage
        herdrSessionId="live"
        terminalId="term-1"
        active
        visible
      />
    )

    await waitFor(() => expect(herdrIpcMock.herdrTerminalOpen).toHaveBeenCalledTimes(1))
    const term = xtermMock.state.terminals[0]

    rerender(
      <HerdrTerminalPage
        herdrSessionId="live"
        terminalId="term-1"
        active={false}
        visible={false}
      />
    )
    rerender(
      <HerdrTerminalPage
        herdrSessionId="live"
        terminalId="term-1"
        active
        visible
      />
    )

    expect(herdrIpcMock.herdrTerminalOpen).toHaveBeenCalledTimes(1)
    expect(herdrIpcMock.herdrTerminalRelease).not.toHaveBeenCalled()
    expect(xtermMock.state.terminals).toEqual([term])
    expect(term.dispose).not.toHaveBeenCalled()
  })
})

describe("HerdrTerminalPage server-owned scrolling", () => {
  beforeEach(() => {
    cleanup()
    xtermMock.reset()
    herdrIpcMock.reset()
    herdrIpcMock.herdrTerminalOpen.mockClear()
    herdrIpcMock.herdrTerminalScroll.mockClear()
    herdrIpcMock.herdrTerminalScroll.mockResolvedValue(undefined)
    seedSessions([{ name: "default", default: true, running: true }])
  })

  afterEach(() => {
    cleanup()
  })

  it("forwards normalized wheel rows to the Herdr control connector", async () => {
    render(
      <HerdrTerminalPage
        herdrSessionId="live"
        terminalId="term-1"
        active
        visible
      />
    )
    await waitFor(() => expect(herdrIpcMock.herdrTerminalOpen).toHaveBeenCalledTimes(1))
    const term = xtermMock.state.terminals[0]
    const handleWheel = term.customWheelEventHandler
    expect(handleWheel).toBeTypeOf("function")

    const pixelWheel = {
      deltaY: -48,
      deltaMode: WheelEvent.DOM_DELTA_PIXEL,
      preventDefault: vi.fn(),
      stopPropagation: vi.fn()
    } as unknown as WheelEvent
    expect(handleWheel?.(pixelWheel)).toBe(false)
    expect(pixelWheel.preventDefault).toHaveBeenCalledTimes(1)
    expect(pixelWheel.stopPropagation).toHaveBeenCalledTimes(1)
    await waitFor(() => {
      expect(herdrIpcMock.herdrTerminalScroll).toHaveBeenCalledWith("sess-1", "up", 3)
    })

    const lineWheel = {
      deltaY: 2,
      deltaMode: WheelEvent.DOM_DELTA_LINE,
      preventDefault: vi.fn(),
      stopPropagation: vi.fn()
    } as unknown as WheelEvent
    expect(handleWheel?.(lineWheel)).toBe(false)
    expect(lineWheel.preventDefault).toHaveBeenCalledTimes(1)
    expect(lineWheel.stopPropagation).toHaveBeenCalledTimes(1)
    await waitFor(() => {
      expect(herdrIpcMock.herdrTerminalScroll).toHaveBeenLastCalledWith("sess-1", "down", 2)
    })

    term.bufferType = "alternate"
    const alternateWheel = {
      deltaY: -48,
      deltaMode: WheelEvent.DOM_DELTA_PIXEL,
      preventDefault: vi.fn(),
      stopPropagation: vi.fn()
    } as unknown as WheelEvent
    expect(handleWheel?.(alternateWheel)).toBe(true)
    expect(alternateWheel.preventDefault).not.toHaveBeenCalled()
    expect(alternateWheel.stopPropagation).not.toHaveBeenCalled()
    expect(herdrIpcMock.herdrTerminalScroll).toHaveBeenCalledTimes(2)
  })

  it("does not mutate server scroll when the connector is unavailable", async () => {
    seedSessions([{ name: "work", default: false, running: false }])
    render(
      <HerdrTerminalPage
        herdrSessionId="work"
        terminalId="term-1"
        active
        visible
      />
    )
    await screen.findByTestId("herdr-terminal-page-term-1")
    const handleWheel = xtermMock.state.terminals[0]?.customWheelEventHandler
    expect(handleWheel?.({ deltaY: -80, deltaMode: WheelEvent.DOM_DELTA_PIXEL } as WheelEvent)).toBe(true)
    expect(herdrIpcMock.herdrTerminalScroll).not.toHaveBeenCalled()
  })
})

describe("HerdrTerminalPage stopped session gate", () => {
  beforeEach(() => {
    cleanup()
    xtermMock.reset()
    herdrIpcMock.reset()
    herdrIpcMock.herdrTerminalOpen.mockClear()
    herdrIpcMock.herdrTerminalRelease.mockClear()
    useHerdrStore.setState({ ...herdrInitialState, attachments: new Map() })
  })

  afterEach(() => {
    cleanup()
  })

  it("does not open connector when existing page session is stopped at mount", async () => {
    seedSessions([{ name: "work", default: false, running: false }])
    render(
      <HerdrTerminalPage
        herdrSessionId="work"
        terminalId="term-1"
        active
        visible
      />
    )

    await waitFor(() => {
      expect(screen.getByTestId("herdr-terminal-stopped")).toBeInTheDocument()
    })
    expect(herdrIpcMock.herdrTerminalOpen).not.toHaveBeenCalled()
    expect(screen.getByTestId("herdr-terminal-page-term-1")).toHaveAttribute(
      "data-session-stopped",
      "true"
    )
  })

  it("keeps the connected terminal and last frame visible during a running→inventory-unknown transition", async () => {
    seedSessions([{ name: "work", default: false, running: true }])
    render(
      <HerdrTerminalPage
        herdrSessionId="work"
        terminalId="term-1"
        active
        visible
      />
    )

    await waitFor(() => expect(herdrIpcMock.herdrTerminalOpen).toHaveBeenCalledTimes(1))
    await waitFor(() => expect(useHerdrStore.getState().attachments.size).toBe(1))
    const term = xtermMock.state.terminals[0]
    await act(async () => {
      herdrIpcMock.emit(frame(1, "preserved output", true))
      await Promise.resolve()
    })
    await waitFor(() => expect(term.write).toHaveBeenCalled())

    act(() => {
      replaceSessionInventory([])
    })

    expect(screen.queryByTestId("herdr-layout-loading")).toBeNull()
    expect(xtermMock.state.terminals).toEqual([term])
    expect(term.dispose).not.toHaveBeenCalled()
    expect(herdrIpcMock.herdrTerminalRelease).not.toHaveBeenCalled()
    expect(herdrIpcMock.herdrTerminalOpen).toHaveBeenCalledTimes(1)
  })

  it("preserves an authoritative BSP layout while session inventory is temporarily unknown", async () => {
    const { herdrLayoutExport } = await import("@/lib/herdrIpc")
    vi.mocked(herdrLayoutExport).mockResolvedValueOnce({
      workspaceId: "ws-1",
      tabId: "tab-1",
      focusedPaneId: "pane-1",
      zoomed: false,
      root: { type: "pane", paneId: "pane-1", label: "Agent" }
    })
    useHerdrStore.setState({
      ...herdrInitialState,
      attachments: new Map(),
      sessions: [{
        name: "work",
        default: false,
        running: true,
        sessionDir: "/tmp/work",
        socketPath: "/tmp/work.sock"
      }],
      selectedSessionName: "work",
      snapshot: {
        herdrSessionId: "work",
        protocol: 19,
        version: "0.8.0",
        spaces: [],
        agents: [],
        tabs: [],
        terminals: [{
          terminalId: "term-1",
          paneId: "pane-1",
          tabId: "tab-1",
          workspaceId: "ws-1"
        }],
        raw: {}
      }
    })
    render(
      <HerdrTerminalPage
        herdrSessionId="work"
        terminalId="term-1"
        herdrTabId="tab-1"
        active
        visible
      />
    )

    await waitFor(() => expect(screen.getByTestId("herdr-terminal-leaf-term-1")).toBeInTheDocument())
    const term = xtermMock.state.terminals[0]

    act(() => {
      replaceSessionInventory([])
    })

    expect(screen.getByTestId("herdr-terminal-leaf-term-1")).toBeInTheDocument()
    expect(xtermMock.state.terminals).toEqual([term])
    expect(term.dispose).not.toHaveBeenCalled()
  })

  it("keeps the terminal surface visible while session inventory is temporarily unknown", async () => {
    render(
      <HerdrTerminalPage
        herdrSessionId="work"
        terminalId="term-1"
        active
        visible
      />
    )

    await act(async () => {
      await Promise.resolve()
    })
    expect(herdrIpcMock.herdrTerminalOpen).not.toHaveBeenCalled()
    expect(screen.queryByTestId("herdr-layout-loading")).toBeNull()
    expect(xtermMock.state.terminals).toHaveLength(1)

    act(() => {
      seedSessions([{ name: "work", default: false, running: true }])
    })
    await waitFor(() => expect(herdrIpcMock.herdrTerminalOpen).toHaveBeenCalledTimes(1))
  })

  it("releases connector and does not reopen when session transitions running→stopped", async () => {
    seedSessions([{ name: "work", default: true, running: true }])
    render(
      <HerdrTerminalPage
        herdrSessionId="work"
        terminalId="term-1"
        active
        visible
      />
    )

    await waitFor(() => expect(herdrIpcMock.herdrTerminalOpen).toHaveBeenCalledTimes(1))

    act(() => {
      seedSessions([{ name: "work", default: true, running: false }])
    })

    await waitFor(() => {
      expect(screen.getByTestId("herdr-terminal-stopped")).toBeInTheDocument()
    })
    await waitFor(() => {
      expect(herdrIpcMock.herdrTerminalRelease).toHaveBeenCalled()
    })
    // No additional open after stop.
    expect(herdrIpcMock.herdrTerminalOpen).toHaveBeenCalledTimes(1)
  })
})

describe("HerdrTerminalPage dispose races", () => {
  beforeEach(() => {
    cleanup()
    xtermMock.reset()
    herdrIpcMock.reset()
    herdrIpcMock.herdrTerminalOpen.mockClear()
    herdrIpcMock.herdrTerminalRelease.mockClear()
    useHerdrStore.setState({ ...herdrInitialState, attachments: new Map() })
  })

  afterEach(() => {
    cleanup()
  })

  it("resync release completing after unmount cannot reopen a hidden connector", async () => {
    seedSessions([{ name: "default", default: true, running: true }])
    let finishResyncRelease: (() => void) | null = null
    let releaseCall = 0
    herdrIpcMock.herdrTerminalRelease.mockImplementation(() => {
      releaseCall += 1
      if (releaseCall !== 1) return Promise.resolve()
      return new Promise<void>((resolve) => {
        finishResyncRelease = resolve
      })
    })

    const { unmount } = render(
      <HerdrTerminalPage
        herdrSessionId="default"
        terminalId="term-1"
        active
        visible
      />
    )
    await waitFor(() => expect(herdrIpcMock.herdrTerminalOpen).toHaveBeenCalledTimes(1))

    herdrIpcMock.emit({
      type: "resync",
      sessionId: "sess-1",
      message: "gap"
    })
    await waitFor(() => expect(herdrIpcMock.herdrTerminalRelease).toHaveBeenCalledTimes(1))
    unmount()

    await act(async () => {
      finishResyncRelease?.()
      await Promise.resolve()
      await Promise.resolve()
    })

    expect(herdrIpcMock.herdrTerminalOpen).toHaveBeenCalledTimes(1)
    expect(useHerdrStore.getState().attachments.size).toBe(0)
  })
})

function seedNamedRuntime() {
  seedSessions([
    { name: "work", default: false, running: true },
    { name: "other", default: true, running: true }
  ])
  useHerdrStore.setState({
    selectedSessionName: "other",
    snapshot: {
      herdrSessionId: "other",
      protocol: 19,
      version: "0.8.0",
      spaces: [{ id: "space-sel", label: "Sel", order: 0, focused: true, path: "/selected/space" }],
      agents: [],
      tabs: [],
      terminals: [{ terminalId: "term-1", paneId: "pane-1", cwd: "/selected/wrong" }],
      raw: {}
    },
    runtimesBySession: {
      work: {
        capabilities: null,
        snapshot: {
          herdrSessionId: "work",
          protocol: 19,
          version: "0.8.0",
          spaces: [{ id: "space-a", label: "A", order: 0, focused: true, path: "/spaces/a" }],
          agents: [],
          tabs: [],
          terminals: [
            { terminalId: "term-1", paneId: "pane-1", workspaceId: "space-a", cwd: "/pane/exact" }
          ],
          raw: {}
        },
        worktreeInventory: null,
        connectionState: "ready",
        errorMessage: null
      }
    }
  })
}

describe("HerdrTerminalPage target opening", () => {
  const workspaceSnapshot = useWorkspaceStore.getState()

  beforeEach(() => {
    cleanup()
    xtermMock.reset()
    herdrIpcMock.reset()
    herdrIpcMock.herdrTerminalOpen.mockClear()
    documentMock.getDocument.mockReset()
    documentMock.getDocument.mockResolvedValue({
      result: { kind: "full", content: "ok", size: 2, lineEnding: "lf" }
    })
    feedbackMock.showActionError.mockClear()
    useContextMenuStore.setState({ request: null, x: 0, y: 0, availabilityRevision: 0 })
    useWorkspaceStore.setState({
      ...workspaceSnapshot,
      workspacePath: "/ws",
      groups: [{ tabs: [], activePath: null }],
      activeGroupIndex: 0,
      pendingReveal: null
    })
  })

  afterEach(() => {
    cleanup()
    useWorkspaceStore.setState(workspaceSnapshot, true)
    useContextMenuStore.setState({ request: null, x: 0, y: 0, availabilityRevision: 0 })
  })

  it("installs a safe OSC 8 handler instead of xterm external navigation", async () => {
    seedNamedRuntime()
    render(
      <HerdrTerminalPage
        herdrSessionId="work"
        terminalId="term-1"
        paneId="pane-1"
        active
        visible
      />
    )
    await waitFor(() => expect(xtermMock.state.terminals.length).toBe(1))
    expect(xtermMock.state.terminals[0].options.linkHandler).toMatchObject({
      activate: expect.any(Function),
      hover: expect.any(Function),
      leave: expect.any(Function),
      allowNonHttpProtocols: false
    })
  })

  it("opens a relative file from the named-session pane cwd, not the selected session", async () => {
    seedNamedRuntime()
    render(
      <HerdrTerminalPage
        herdrSessionId="work"
        terminalId="term-1"
        paneId="pane-1"
        active
        visible
      />
    )
    await waitFor(() => expect(xtermMock.state.terminals.length).toBe(1))
    const term = xtermMock.state.terminals[0]
    term.bufferLines = [{ text: "src/app.ts" }]
    let links: Array<{ activate: (event: MouseEvent, text: string) => void }> | undefined
    term.linkProvider?.provideLinks(1, (next) => {
      links = next as typeof links
    })
    await waitFor(() => expect(links).toHaveLength(1))
    const modifier = isMacPlatform() ? { metaKey: true } : { ctrlKey: true }
    links?.[0]?.activate(
      { button: 0, altKey: false, ctrlKey: false, metaKey: false, ...modifier } as MouseEvent,
      "src/app.ts"
    )
    await waitFor(() => {
      expect(documentMock.getDocument).toHaveBeenCalledWith("/pane/exact/src/app.ts")
    })
    expect(documentMock.getDocument).not.toHaveBeenCalledWith("/selected/wrong/src/app.ts")
    expect(useWorkspaceStore.getState().groups[0]?.activePath).toBe("/pane/exact/src/app.ts")
  })

  it("keeps the pane menu on right click even when a target is hovered", async () => {
    seedNamedRuntime()
    render(
      <HerdrTerminalPage
        herdrSessionId="work"
        terminalId="term-1"
        paneId="pane-1"
        active
        visible
      />
    )
    await waitFor(() => expect(screen.getByTestId("herdr-terminal-leaf-term-1")).toBeInTheDocument())
    const leaf = screen.getByTestId("herdr-terminal-leaf-term-1")

    fireEvent.contextMenu(leaf, { button: 2 })
    expect(useContextMenuStore.getState().request?.kind).toBe("herdrPane")
    useContextMenuStore.setState({ request: null, x: 0, y: 0, availabilityRevision: 0 })

    const modifier = isMacPlatform() ? { metaKey: true } : { ctrlKey: true }
    fireEvent.contextMenu(leaf, { button: 2, ...modifier })
    expect(useContextMenuStore.getState().request?.kind).toBe("herdrPane")
    useContextMenuStore.setState({ request: null, x: 0, y: 0, availabilityRevision: 0 })

    const term = xtermMock.state.terminals[0]
    term.bufferLines = [{ text: "https://example.com/docs" }]
    let links: Array<{
      activate: (event: MouseEvent, text: string) => void
      hover?: (event: MouseEvent, text: string) => void
    }> | undefined
    term.linkProvider?.provideLinks(1, (next) => {
      links = next as typeof links
    })
    await waitFor(() => expect(links).toHaveLength(1))
    const gesture = {
      button: 2,
      altKey: false,
      ctrlKey: false,
      metaKey: false,
      ...modifier
    } as MouseEvent
    links?.[0]?.hover?.(gesture, "https://example.com/docs")
    links?.[0]?.activate(gesture, "https://example.com/docs")
    fireEvent.contextMenu(leaf, gesture)
    expect(useContextMenuStore.getState().request?.kind).toBe("herdrPane")
  })

  it("disposes the shared provider without recreating a persistent xterm", async () => {
    seedNamedRuntime()
    const { unmount, rerender } = render(
      <HerdrTerminalPage
        herdrSessionId="work"
        terminalId="term-1"
        paneId="pane-1"
        active
        visible
      />
    )
    await waitFor(() => expect(xtermMock.state.terminals.length).toBe(1))
    const term = xtermMock.state.terminals[0]
    expect(term.registerLinkProvider).toHaveBeenCalledTimes(1)
    term.writeParsedHandler?.()
    fireEvent.mouseLeave(screen.getByTestId("herdr-terminal-leaf-term-1").lastElementChild!)
    fireEvent.blur(window)
    expect(term.onWriteParsed).toHaveBeenCalledTimes(1)

    rerender(
      <HerdrTerminalPage
        herdrSessionId="work"
        terminalId="term-1"
        paneId="pane-1"
        active={false}
        visible={false}
      />
    )
    rerender(
      <HerdrTerminalPage
        herdrSessionId="work"
        terminalId="term-1"
        paneId="pane-1"
        active
        visible
      />
    )
    expect(xtermMock.state.terminals).toEqual([term])
    expect(term.dispose).not.toHaveBeenCalled()
    expect(term.linkProviderDisposable.dispose).not.toHaveBeenCalled()

    unmount()
    expect(term.linkProviderDisposable.dispose).toHaveBeenCalledTimes(1)
    expect(term.writeParsedDisposable.dispose).toHaveBeenCalledTimes(1)
  })
})
