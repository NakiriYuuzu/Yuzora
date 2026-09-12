import { installTerminalImeHandling } from "@/terminal/terminalImeHandling"
import { Profiler } from "react"
import { useTextInputDialogStore } from "@/state/textInputDialogStore"
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { HerdrCapabilities, HerdrTerminalEvent } from "@/lib/herdrTypes"
import { normalizeHerdrSnapshot } from "@/lib/herdrNormalize"
import { herdrInitialState, useHerdrStore } from "@/state/herdrStore"

const xtermMock = vi.hoisted(() => {
  type DataHandler = (data: string) => void
  type KeyHandler = (event: KeyboardEvent) => boolean

  const state = {
    terminals: [] as TerminalMock[]
  }

  class TerminalMock {
    options: Record<string, unknown>
    cols = 80
    rows = 24
    selection = ""
    dataHandler: DataHandler | null = null
    keyHandler: KeyHandler | null = null
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
    refresh = vi.fn()
    hasSelection = vi.fn(() => this.selection.length > 0)
    getSelection = vi.fn(() => this.selection)
    paste = vi.fn((text: string) => this.dataHandler?.(text))
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
    attachCustomKeyEventHandler = vi.fn((handler: KeyHandler) => {
      this.keyHandler = handler
    })
    emitKey(event: KeyboardEvent) {
      return this.keyHandler?.(event) ?? true
    }
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

const clipboardMock = vi.hoisted(() => ({
  readText: vi.fn(),
  writeText: vi.fn()
}))

const navigatorClipboardMock = vi.hoisted(() => ({
  readText: vi.fn(),
  writeText: vi.fn()
}))

vi.mock("@tauri-apps/plugin-clipboard-manager", () => clipboardMock)

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

const terminalControlCapabilities = {
  binaryPath: "/bin/herdr",
  binarySource: {
    configured: "global",
    resolved: "global",
    available: true,
    path: "/bin/herdr",
    reason: null,
    restartRequired: false
  },
  server: { running: true },
  api: {
    snapshot: true,
    ping: true,
    tabCreate: true,
    workspaceFocus: true,
    workspaceCreate: true,
    workspaceRename: true,
    workspaceClose: true,
    tabRename: true,
    tabClose: true,
    tabFocus: true,
    tabMove: false,
    paneFocus: true,
    paneRename: true,
    paneSplit: true,
    paneZoom: true,
    paneSwap: true,
    paneClose: true,
    layoutExport: false,
    layoutSetSplitRatio: false,
    agentGet: false,
    agentRead: false,
    eventsSubscribe: false,
    worktreeList: false,
    methods: [],
    reason: null
  },
  terminal: {
    observe: true,
    control: true,
    takeover: true,
    input: true,
    resize: true,
    scroll: true,
    release: true,
    create: true,
    reason: null
  },
  events: { status: "deferred" }
} satisfies HerdrCapabilities

function seedSessions(
  sessions: Array<{ name: string; default: boolean; running: boolean }>
) {
  useHerdrStore.setState({
    ...herdrInitialState,
    attachments: new Map(),
    selectedSessionName: sessions.find((session) => session.default)?.name ?? sessions[0]?.name ?? null,
    sessions: sessions.map((s) => ({
      ...s,
      sessionDir: `/tmp/${s.name}`,
      socketPath: `/tmp/${s.name}.sock`
    })),
    capabilities: terminalControlCapabilities
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

beforeEach(() => {
  clipboardMock.readText.mockResolvedValue("")
  clipboardMock.writeText.mockResolvedValue(undefined)
  navigatorClipboardMock.readText.mockResolvedValue("")
  navigatorClipboardMock.writeText.mockResolvedValue(undefined)
  Object.defineProperty(globalThis.navigator, "clipboard", {
    value: navigatorClipboardMock,
    configurable: true
  })
})

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

  it("ignores attachment updates for other terminal pages while keeping its own badge reactive", async () => {
    const renderCommit = vi.fn()
    render(<Profiler id="terminal" onRender={renderCommit}><HerdrTerminalPage herdrSessionId="live" terminalId="term-1" active visible /></Profiler>)
    await waitFor(() => expect(useHerdrStore.getState().attachments.size).toBe(1))
    const [key, record] = [...useHerdrStore.getState().attachments][0]
    renderCommit.mockClear()
    await act(async () => {
      useHerdrStore.getState().registerAttachment("other-page", { ...record, pagePath: "other-page" })
    })
    expect(renderCommit).not.toHaveBeenCalled()
    act(() => useHerdrStore.getState().registerAttachment(key, { ...record, mode: "observe", role: "observer" }))
    expect(renderCommit).toHaveBeenCalled()
    expect(herdrIpcMock.herdrTerminalOpen).toHaveBeenCalledTimes(1)
  })

  it("does not redraw a terminal when only runtime focus changes", async () => {
    const snapshot = normalizeHerdrSnapshot({ protocol: 22, version: "0.9.0", snapshot: {
      tabs: [{ tab_id: "tab-1", workspace_id: "space-1", terminal_id: "term-1", pane_id: "pane-1" }],
      panes: [{ pane_id: "pane-1", terminal_id: "term-1", tab_id: "tab-1", workspace_id: "space-1", cwd: "/demo" }]
    } }, "default")
    const runtime = { connectionState: "ready" as const, capabilities: terminalControlCapabilities, snapshot, baseSnapshot: snapshot, worktreeInventory: null, errorMessage: null }
    useHerdrStore.setState({ snapshot, runtimesBySession: { default: runtime } })
    const renderCommit = vi.fn()
    render(<Profiler id="terminal" onRender={renderCommit}><HerdrTerminalPage herdrSessionId="default" terminalId="term-1" herdrTabId="tab-1" active visible /></Profiler>)
    await waitFor(() => expect(useHerdrStore.getState().attachments.size).toBe(1))
    renderCommit.mockClear()
    act(() => {
      const focused = { ...snapshot, focusedTabId: "other-tab", tabs: snapshot.tabs.map(tab => ({ ...tab, focused: false })) }
      useHerdrStore.setState({ snapshot: focused, runtimesBySession: { default: { ...runtime, snapshot: focused } } })
    })
    expect(renderCommit).not.toHaveBeenCalled()
    expect(herdrIpcMock.herdrTerminalOpen).toHaveBeenCalledTimes(1)
  })

  it("does not steal focus when a naming dialog is pending before its portal mounts", async () => {
    void useTextInputDialogStore.getState().request({ title: "Name", label: "Name", confirmLabel: "Save" })
    try {
      render(<HerdrTerminalPage herdrSessionId="default" terminalId="term-1" active visible />)
      await waitFor(() => expect(xtermMock.state.terminals.length).toBeGreaterThan(0))
      expect(xtermMock.state.terminals[0].focus).not.toHaveBeenCalled()
    } finally {
      useTextInputDialogStore.getState().respond(null)
    }
  })

  it("blocks focus and terminal input while any mounted modal is open", async () => {
    const view = render(<HerdrTerminalPage herdrSessionId="default" terminalId="term-1" active={false} visible />)
    await waitFor(() => expect(herdrIpcMock.herdrTerminalOpen).toHaveBeenCalled())
    const modal = document.createElement("div")
    modal.setAttribute("aria-modal", "true")
    document.body.append(modal)
    try {
      const term = xtermMock.state.terminals[0]
      term.focus.mockClear()
      view.rerender(<HerdrTerminalPage herdrSessionId="default" terminalId="term-1" active visible />)
      expect(term.focus).not.toHaveBeenCalled()
      const calls = vi.mocked(installTerminalImeHandling).mock.calls
      const sendInput = calls[calls.length - 1][1]
      herdrIpcMock.herdrTerminalInput.mockClear()
      sendInput("qa_focus_probe")
      expect(herdrIpcMock.herdrTerminalInput).not.toHaveBeenCalled()
      modal.remove()
      sendInput("after_modal")
      await waitFor(() => expect(herdrIpcMock.herdrTerminalInput).toHaveBeenCalled())
    } finally {
      modal.remove()
    }
  })

  it.each(["ssh-linux", "wsl-ubuntu"])("opens the scoped %s Session even when local default is stopped", async (hostId) => {
    const scope = JSON.stringify([hostId, "default"])
    seedSessions([{ name: "default", default: true, running: false }])
    const local = useHerdrStore.getState().sessions[0]
    useHerdrStore.setState({
      selectedSessionName: scope,
      sessions: [local, { ...local, hostId, runtimeId: scope, running: true }]
    })
    render(<HerdrTerminalPage herdrSessionId={scope} terminalId="same-terminal" active visible />)
    await waitFor(() => expect(herdrIpcMock.herdrTerminalOpen).toHaveBeenCalledWith(expect.objectContaining({ sessionName: scope, target: "same-terminal" })))
    expect(screen.queryByTestId("herdr-terminal-stopped")).toBeNull()
  })

  it("never borrows a running local default for a stopped remote Session", async () => {
    const scope = JSON.stringify(["ssh-stopped", "default"])
    seedSessions([{ name: "default", default: true, running: true }])
    const local = useHerdrStore.getState().sessions[0]
    useHerdrStore.setState({ selectedSessionName: scope, sessions: [local, { ...local, hostId: "ssh-stopped", runtimeId: scope, running: false }] })
    render(<HerdrTerminalPage herdrSessionId={scope} terminalId="same-terminal" active visible />)
    await waitFor(() => expect(screen.getByTestId("herdr-terminal-stopped")).toBeInTheDocument())
    expect(herdrIpcMock.herdrTerminalOpen).not.toHaveBeenCalled()
  })

  it("does not open a control connector before exact runtime capabilities are known", async () => {
    useHerdrStore.setState({ capabilities: null })

    render(
      <HerdrTerminalPage
        herdrSessionId="live"
        terminalId="term-1"
        active
        visible
      />
    )

    await waitFor(() => {
      expect(screen.getByRole("status")).toHaveTextContent(
        "Terminal control is unavailable for this Herdr server."
      )
    })
    expect(herdrIpcMock.herdrTerminalOpen).not.toHaveBeenCalled()
  })

  it("keeps the scoped WSL scrollbar when capabilities are projected globally", async () => {
    const scope = JSON.stringify(["wsl-ubuntu", "default"])
    seedSessions([{ name: "default", default: true, running: false }])
    const local = useHerdrStore.getState().sessions[0]
    const capabilities = {
      ...terminalControlCapabilities,
      api: { ...terminalControlCapabilities.api, methods: ["pane.get", "pane.scroll"] }
    }
    useHerdrStore.setState({
      selectedSessionName: scope,
      capabilities,
      sessions: [local, { ...local, hostId: "wsl-ubuntu", runtimeId: scope, running: true }],
      runtimesBySession: {
        [scope]: {
          connectionState: "ready",
          capabilities: null,
          snapshot: null,
          baseSnapshot: null,
          worktreeInventory: null,
          errorMessage: null
        }
      }
    })
    render(<HerdrTerminalPage herdrSessionId={scope} terminalId="same-terminal" paneId="pane-1" active visible />)
    await waitFor(() => expect(screen.getByTestId("herdr-scroll-proxy")).toBeInTheDocument())
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

  it("does not render terminal pages for unrelated runtime snapshot updates", async () => {
    const renderCommit = vi.fn()
    render(<Profiler id="terminal" onRender={renderCommit}><HerdrTerminalPage herdrSessionId="live" terminalId="term-1" active visible /></Profiler>)
    await waitFor(() => expect(herdrIpcMock.herdrTerminalOpen).toHaveBeenCalledTimes(1))
    renderCommit.mockClear()
    for (let i = 0; i < 20; i++) {
      act(() => useHerdrStore.setState((state) => ({ runtimesBySession: {
        ...state.runtimesBySession,
        other: { ...state.runtimesBySession.default, errorMessage: `update ${i}` }
      } })))
    }
    expect(renderCommit).not.toHaveBeenCalled()
  })

  it("recovers a truncated hidden ANSI stream before painting the selected agent", async () => {
    const { rerender } = render(<HerdrTerminalPage herdrSessionId="live" terminalId="term-1" active visible />)
    await waitFor(() => expect(herdrIpcMock.herdrTerminalOpen).toHaveBeenCalledTimes(1))
    const term = xtermMock.state.terminals[0]
    herdrIpcMock.emit(frame(1, "initial screen", true))
    await waitFor(() => expect(term.write).toHaveBeenCalledTimes(1))
    rerender(<HerdrTerminalPage herdrSessionId="live" terminalId="term-1" active={false} visible={false} />)
    herdrIpcMock.emit(frame(2, "\u001b[?2026h" + "x".repeat(300 * 1024)))
    rerender(<HerdrTerminalPage herdrSessionId="live" terminalId="term-1" active visible />)
    await waitFor(() => expect(herdrIpcMock.herdrTerminalOpen).toHaveBeenCalledTimes(2))
    expect(herdrIpcMock.herdrTerminalRelease).toHaveBeenCalledTimes(1)
    expect(term.write).toHaveBeenCalledTimes(1)
    herdrIpcMock.emit(frame(1, "\u001b[2Jrestored screen", true))
    await waitFor(() => expect(term.write).toHaveBeenCalledTimes(2))
    expect(term.write.mock.calls[1][0]).toContain("restored screen")
    expect(term.write.mock.calls[1][0]).not.toContain("was truncated")
    expect(xtermMock.state.terminals).toHaveLength(1)
    expect(term.refresh).toHaveBeenCalledWith(0, term.rows - 1)
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

describe("HerdrTerminalPage clipboard", () => {
  beforeEach(() => {
    cleanup()
    xtermMock.reset()
    herdrIpcMock.reset()
    herdrIpcMock.herdrTerminalOpen.mockClear()
    herdrIpcMock.herdrTerminalInput.mockClear()
    clipboardMock.readText.mockClear()
    clipboardMock.writeText.mockClear()
    seedSessions([{ name: "default", default: true, running: true }])
  })

  afterEach(() => {
    cleanup()
  })

  it("preserves setup paste when control arrives before open readiness", async () => {
    clipboardMock.readText.mockResolvedValue("setup clipboard payload")
    let completeOpen: () => void = () => {
      throw new Error("Expected a pending Herdr terminal open")
    }
    herdrIpcMock.herdrTerminalOpen.mockImplementationOnce(
      () => new Promise((resolve) => {
        completeOpen = () => resolve({
          sessionId: "sess-1",
          target: "term-1",
          mode: "control",
          role: "controller",
          cols: 80,
          rows: 24,
          takeover: true
        })
      })
    )

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
    const event = new KeyboardEvent("keydown", {
      key: "v",
      ctrlKey: true,
      cancelable: true
    })

    expect(term.emitKey(event)).toBe(false)
    await waitFor(() => expect(clipboardMock.readText).toHaveBeenCalledTimes(1))
    expect(term.paste).not.toHaveBeenCalled()

    await act(async () => completeOpen())

    await waitFor(() => {
      expect(herdrIpcMock.herdrTerminalInput).toHaveBeenCalledWith("sess-1", "\x1b[200~setup clipboard payload\x1b[201~", null)
    })
  })

  it.each([
    ["Ctrl", { ctrlKey: true }],
    ["Cmd", { metaKey: true }]
  ])("copies a selection with %s+C", async (_label, modifier) => {
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
    term.selection = "selected Herdr text"
    const event = new KeyboardEvent("keydown", {
      key: "c",
      cancelable: true,
      ...modifier
    })

    expect(term.emitKey(event)).toBe(false)
    await waitFor(() => {
      expect(clipboardMock.writeText).toHaveBeenCalledWith("selected Herdr text")
    })
    expect(event.defaultPrevented).toBe(true)
    expect(herdrIpcMock.herdrTerminalInput).not.toHaveBeenCalled()
  })

  it.each([
    ["Ctrl", { ctrlKey: true }],
    ["Cmd", { metaKey: true }]
  ])("pastes clipboard text with %s+V", async (_label, modifier) => {
    clipboardMock.readText.mockResolvedValue("Herdr clipboard payload")
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
    const event = new KeyboardEvent("keydown", {
      key: "v",
      cancelable: true,
      ...modifier
    })

    expect(term.emitKey(event)).toBe(false)
    await waitFor(() => {
      expect(herdrIpcMock.herdrTerminalInput).toHaveBeenCalledWith("sess-1", "\x1b[200~Herdr clipboard payload\x1b[201~", null)
    })
    expect(event.defaultPrevented).toBe(true)
  })

  it("drops observer paste rather than replaying it after Take Control", async () => {
    clipboardMock.readText.mockResolvedValue("observer clipboard payload")
    herdrIpcMock.herdrTerminalOpen.mockResolvedValueOnce({
      sessionId: "sess-observer",
      target: "term-1",
      mode: "observe",
      role: "observer",
      cols: 80,
      rows: 24,
      takeover: false
    })
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
    const event = new KeyboardEvent("keydown", {
      key: "v",
      ctrlKey: true,
      cancelable: true
    })

    expect(term.emitKey(event)).toBe(false)
    await act(async () => Promise.resolve())
    expect(clipboardMock.readText).not.toHaveBeenCalled()
    expect(term.paste).not.toHaveBeenCalled()

    fireEvent.click(await screen.findByTestId("herdr-take-control"))
    await waitFor(() => expect(herdrIpcMock.herdrTerminalOpen).toHaveBeenCalledTimes(2))

    expect(term.paste).not.toHaveBeenCalled()
    expect(herdrIpcMock.herdrTerminalInput).not.toHaveBeenCalled()
  })

  it("sends Shift+Enter as one bracketed newline rather than a submit byte", async () => {
    render(<HerdrTerminalPage herdrSessionId="live" terminalId="term-1" active visible />)
    await waitFor(() => expect(herdrIpcMock.herdrTerminalOpen).toHaveBeenCalledOnce())
    const event = new KeyboardEvent("keydown", { key: "Enter", shiftKey: true, cancelable: true })
    expect(xtermMock.state.terminals[0].emitKey(event)).toBe(false)
    await waitFor(() => expect(herdrIpcMock.herdrTerminalInput).toHaveBeenCalledExactlyOnceWith("sess-1", "\x1b[200~\n\x1b[201~", null))
    expect(clipboardMock.readText).not.toHaveBeenCalled()
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

  it("disables xterm local scrollback for server-owned frames", async () => {
    render(
      <HerdrTerminalPage
        herdrSessionId="live"
        terminalId="term-1"
        active
        visible
      />
    )
    await waitFor(() => expect(herdrIpcMock.herdrTerminalOpen).toHaveBeenCalledTimes(1))

    expect(xtermMock.state.terminals[0].options).toMatchObject({
      scrollback: 0,
      scrollOnUserInput: false,
      smoothScrollDuration: 0
    })
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

  it("keeps the fallback connector mounted while a topology refresh awaits layout", async () => {
    const { herdrLayoutExport } = await import("@/lib/herdrIpc")
    seedSessions([{ name: "work", default: false, running: true }])
    render(<HerdrTerminalPage herdrSessionId="work" terminalId="term-1" active visible />)
    await waitFor(() => expect(useHerdrStore.getState().attachments.size).toBe(1))
    const term = xtermMock.state.terminals[0]
    let rejectLayout!: (error: Error) => void
    vi.mocked(herdrLayoutExport).mockImplementationOnce(() => new Promise((_, reject) => { rejectLayout = reject }))
    await act(async () => { useHerdrStore.getState().bumpTopologyRevision() })
    expect(screen.queryByTestId("herdr-layout-loading")).toBeNull()
    expect(term.dispose).not.toHaveBeenCalled()
    expect(herdrIpcMock.herdrTerminalRelease).not.toHaveBeenCalled()
    await act(async () => { rejectLayout(new Error("layout still unavailable")) })
    expect(xtermMock.state.terminals).toEqual([term])
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
      replaceSessionInventory([{ name: "work", default: true, running: false }])
    })

    await waitFor(() => {
      expect(screen.getByTestId("herdr-terminal-stopped")).toBeInTheDocument()
    })
    await waitFor(() => {
      expect(herdrIpcMock.herdrTerminalRelease).toHaveBeenCalledTimes(1)
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

  it("releases exactly once when stale-page registry cleanup races leaf unmount", async () => {
    seedSessions([{ name: "default", default: true, running: true }])
    const { unmount } = render(
      <HerdrTerminalPage
        herdrSessionId="default"
        terminalId="term-1"
        active
        visible
      />
    )
    await waitFor(() => expect(herdrIpcMock.herdrTerminalOpen).toHaveBeenCalledTimes(1))
    const attachmentKey = "yuzora://herdr/default/term-1::term-1"
    await waitFor(() => expect(useHerdrStore.getState().attachments.has(attachmentKey)).toBe(true))

    const staleCleanup = useHerdrStore.getState().releaseAttachment(attachmentKey)
    unmount()
    await staleCleanup

    expect(herdrIpcMock.herdrTerminalRelease).toHaveBeenCalledTimes(1)
    expect(useHerdrStore.getState().attachments.size).toBe(0)
  })

  it("releases exactly once per connector generation across Take Control and unmount", async () => {
    seedSessions([{ name: "default", default: true, running: true }])
    herdrIpcMock.herdrTerminalOpen.mockResolvedValueOnce({
      sessionId: "sess-observer",
      target: "term-1",
      mode: "observe",
      role: "observer",
      cols: 80,
      rows: 24,
      takeover: false
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

    fireEvent.click(await screen.findByTestId("herdr-take-control"))

    await waitFor(() => expect(herdrIpcMock.herdrTerminalOpen).toHaveBeenCalledTimes(2))
    expect(herdrIpcMock.herdrTerminalRelease).toHaveBeenCalledTimes(1)

    unmount()
    await waitFor(() => expect(herdrIpcMock.herdrTerminalRelease).toHaveBeenCalledTimes(2))
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
    expect(herdrIpcMock.herdrTerminalRelease).toHaveBeenCalledTimes(1)
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
        capabilities: terminalControlCapabilities,
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

    await waitFor(() => expect(xtermMock.state.terminals).toHaveLength(1))
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
