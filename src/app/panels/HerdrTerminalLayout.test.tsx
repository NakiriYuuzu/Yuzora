import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { HerdrLayoutDescription } from "@/lib/herdrTypes"
import { herdrInitialState, useHerdrStore } from "@/state/herdrStore"
import { useAgentMruStore } from "@/state/agentMruStore"

const layoutMock = vi.hoisted(() => {
  let layout: HerdrLayoutDescription = {
    workspaceId: "ws-1",
    tabId: "tab-1",
    zoomed: false,
    focusedPaneId: "p1",
    root: {
      type: "split",
      direction: "right",
      ratio: 0.6,
      first: { type: "pane", paneId: "p1", label: "A", cwd: null },
      second: {
        type: "split",
        direction: "down",
        ratio: 0.5,
        first: { type: "pane", paneId: "p2", label: "B", cwd: null },
        second: { type: "pane", paneId: "p3", label: "C", cwd: null }
      }
    }
  }
  return {
    get: () => layout,
    set(next: HerdrLayoutDescription) {
      layout = next
    },
    export: vi.fn(async (_args?: unknown) => layout),
    setRatio: vi.fn(async (args: { path: boolean[]; ratio: number }) => {
      layout = {
        ...layout,
        root: {
          type: "split",
          direction: "right",
          ratio: args.ratio,
          first: { type: "pane", paneId: "p1", label: "A", cwd: null },
          second: {
            type: "split",
            direction: "down",
            ratio: 0.5,
            first: { type: "pane", paneId: "p2", label: "B", cwd: null },
            second: { type: "pane", paneId: "p3", label: "C", cwd: null }
          }
        }
      }
      return layout
    })
  }
})

vi.mock("@xterm/xterm", () => {
  class Terminal {
    options: Record<string, unknown>
    cols = 80
    rows = 24
    constructor(options: Record<string, unknown>) {
      this.options = options
    }
    open = vi.fn()
    write = vi.fn((_d: string, cb?: () => void) => cb?.())
    focus = vi.fn()
    refresh = vi.fn()
    reset = vi.fn()
    resize = vi.fn()
    dispose = vi.fn()
    loadAddon = vi.fn()
    onData = vi.fn(() => ({ dispose: vi.fn() }))
    attachCustomKeyEventHandler = vi.fn()
    attachCustomWheelEventHandler = vi.fn()
    registerLinkProvider = vi.fn(() => ({ dispose: vi.fn() }))
  }
  return { Terminal }
})

vi.mock("@xterm/addon-fit", () => ({
  FitAddon: class {
    fit = vi.fn()
    proposeDimensions = vi.fn(() => ({ cols: 80, rows: 24 }))
    activate = vi.fn()
    dispose = vi.fn()
  }
}))

vi.mock("@/terminal/terminalImeHandling", () => ({
  installTerminalImeHandling: vi.fn(() => ({ dispose: vi.fn() }))
}))

vi.mock("@/terminal/xtermTheme", () => ({
  buildXtermTheme: vi.fn(() => ({}))
}))

// Observe the authoritative percentages passed to the real resizable panels.
// Keyboard handling, registration, and layout callbacks remain the library's.
vi.mock("@/components/ui/resizable", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/components/ui/resizable")>()
  return {
    ...actual,
    ResizablePanel: (props: Parameters<typeof actual.ResizablePanel>[0]) =>
      <actual.ResizablePanel {...props} data-confirmed-size={props.defaultSize} />
  }
})

vi.mock("@/lib/herdrIpc", () => ({
  herdrTerminalOpen: vi.fn(async () => ({
    sessionId: `sess-${Math.random().toString(16).slice(2)}`,
    target: "term",
    mode: "control",
    role: "controller",
    cols: 80,
    rows: 24,
    takeover: true
  })),
  herdrTerminalInput: vi.fn(),
  herdrTerminalResize: vi.fn(),
  herdrTerminalScroll: vi.fn(),
  herdrTerminalRelease: vi.fn().mockResolvedValue(undefined),
  herdrPaneFocus: vi.fn().mockResolvedValue(undefined),
  herdrLayoutExport: (args: unknown) => layoutMock.export(args),
  herdrLayoutSetSplitRatio: vi.fn((args: { path: boolean[]; ratio: number }) =>
    layoutMock.setRatio(args))
}))

import { herdrAttachmentKey } from "@/lib/herdrPages"
import { herdrLayoutSetSplitRatio, herdrPaneFocus, herdrTerminalOpen, herdrTerminalRelease } from "@/lib/herdrIpc"
import { HerdrTerminalPage } from "./HerdrTerminalPage"

function layoutCapabilities(layoutSetSplitRatio: boolean) {
  return {
    binaryPath: "/bin/herdr",
    binarySource: {
      configured: "global" as const,
      resolved: "global" as const,
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
      layoutExport: true,
      layoutSetSplitRatio,
      eventsSubscribe: false,
      worktreeList: false,
      methods: ["layout.export"],
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
    events: { status: "deferred" as const }
  }
}

function seed(layoutSetSplitRatio = true) {
  useHerdrStore.setState({
    ...herdrInitialState,
    attachments: new Map(),
    topologyRevision: 0,
    sessions: [
      {
        name: "default",
        default: true,
        running: true,
        sessionDir: "/tmp/d",
        socketPath: "/tmp/d.sock"
      }
    ],
    selectedSessionName: "default",
    capabilities: layoutCapabilities(layoutSetSplitRatio),
    snapshot: {
      herdrSessionId: "default",
      protocol: 19,
      version: "0.8.0",
      spaces: [],
      agents: [],
      tabs: [],
      terminals: [
        { terminalId: "t1", paneId: "p1", tabId: "tab-1" },
        { terminalId: "t2", paneId: "p2", tabId: "tab-1" },
        { terminalId: "t3", paneId: "p3", tabId: "tab-1" }
      ],
      raw: {}
    }
  })
}

describe("HerdrTerminalPage BSP layout surface", () => {
  let viewportRect: ReturnType<typeof vi.spyOn>
  let panelWidth: ReturnType<typeof vi.spyOn>
  let panelHeight: ReturnType<typeof vi.spyOn>
  beforeEach(() => {
    viewportRect = vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue({ width: 800, height: 600 } as DOMRect)
    panelWidth = vi.spyOn(HTMLElement.prototype, "offsetWidth", "get").mockReturnValue(400)
    panelHeight = vi.spyOn(HTMLElement.prototype, "offsetHeight", "get").mockReturnValue(300)
    cleanup()
    seed()
    layoutMock.set({ ...layoutMock.get(), zoomed: false, focusedPaneId: "p1" })
    layoutMock.export.mockReset().mockImplementation(async () => layoutMock.get())
    layoutMock.setRatio.mockClear()
    vi.mocked(herdrLayoutSetSplitRatio).mockClear()
    vi.mocked(herdrPaneFocus).mockReset().mockResolvedValue(undefined)
    vi.mocked(herdrTerminalOpen).mockClear()
    vi.mocked(herdrTerminalRelease).mockClear()
  })

  afterEach(() => {
    cleanup()
    viewportRect.mockRestore()
    panelWidth.mockRestore()
    panelHeight.mockRestore()
  })

  it("renders nested BSP splits and opens independent control connectors", async () => {
    render(
      <HerdrTerminalPage
        herdrSessionId="default"
        terminalId="t1"
        herdrTabId="tab-1"
        pagePath="yuzora://herdr/default/t1"
        active
        visible
      />
    )

    await waitFor(() => expect(layoutMock.export).toHaveBeenCalled())
    expect(layoutMock.export).toHaveBeenCalledWith(
      expect.objectContaining({ tabId: "tab-1" })
    )
    await waitFor(() => {
      expect(screen.getByTestId("herdr-split-root")).toBeInTheDocument()
      expect(screen.getByTestId("herdr-split-1")).toBeInTheDocument()
    })
    await waitFor(() => expect(herdrTerminalOpen).toHaveBeenCalledTimes(3))
    for (const call of vi.mocked(herdrTerminalOpen).mock.calls) {
      expect(call[0]).toMatchObject({ mode: "control", takeover: true })
    }

    const attachments = useHerdrStore.getState().attachments
    expect(attachments.size).toBe(3)
    expect(attachments.has(herdrAttachmentKey("yuzora://herdr/default/t1", "p1"))).toBe(true)
    expect(attachments.has(herdrAttachmentKey("yuzora://herdr/default/t1", "p2"))).toBe(true)
    expect(attachments.has(herdrAttachmentKey("yuzora://herdr/default/t1", "p3"))).toBe(true)
  })

  it("marks only the confirmed input pane and clears the marker when the page is inactive or hidden", async () => {
    let confirmFocus!: () => void
    vi.mocked(herdrPaneFocus).mockImplementationOnce(() => new Promise<void>((resolve) => { confirmFocus = resolve }))
    const view = render(<HerdrTerminalPage herdrSessionId="default" terminalId="t1" herdrTabId="tab-1" active visible />)
    await waitFor(() => expect(herdrTerminalOpen).toHaveBeenCalledTimes(3))
    const paneA = screen.getByRole("button", { name: "Focus terminal: A" })
    const paneB = screen.getByRole("button", { name: "Focus terminal: B" })
    expect(paneA).toHaveAttribute("aria-pressed", "true")
    expect(paneB).toHaveAttribute("aria-pressed", "false")
    expect(screen.getAllByText("Focused")).toHaveLength(1)

    fireEvent.click(paneB)
    expect(herdrPaneFocus).toHaveBeenCalledWith({ sessionName: "default", paneId: "p2" })
    expect(paneA).toHaveAttribute("aria-pressed", "true")
    await act(async () => confirmFocus())
    await waitFor(() => expect(paneB).toHaveAttribute("aria-pressed", "true"))
    expect(paneA).toHaveAttribute("aria-pressed", "false")
    expect(screen.getAllByText("Focused")).toHaveLength(1)
    expect(herdrTerminalOpen).toHaveBeenCalledTimes(3)
    expect(herdrTerminalRelease).not.toHaveBeenCalled()

    view.rerender(<HerdrTerminalPage herdrSessionId="default" terminalId="t1" herdrTabId="tab-1" active={false} visible />)
    expect(screen.queryByText("Focused")).not.toBeInTheDocument()
    expect(paneB).toHaveAttribute("aria-pressed", "false")
    view.rerender(<HerdrTerminalPage herdrSessionId="default" terminalId="t1" herdrTabId="tab-1" active visible={false} />)
    expect(screen.queryByText("Focused")).not.toBeInTheDocument()
  })

  it("selects the pane an Agent activation targets without reloading the mounted split", async () => {
    render(<HerdrTerminalPage herdrSessionId="default" terminalId="t1" herdrTabId="tab-1" active visible />)
    await waitFor(() => expect(herdrTerminalOpen).toHaveBeenCalledTimes(3))
    const paneA = screen.getByRole("button", { name: "Focus terminal: A" })
    const paneC = screen.getByRole("button", { name: "Focus terminal: C" })
    const exports = layoutMock.export.mock.calls.length

    // Other sessions and panes outside this tab never move the selection.
    act(() => useHerdrStore.setState({ paneFocusRequest: { sessionName: "work", paneId: "p3", seq: 1 } }))
    act(() => useHerdrStore.setState({ paneFocusRequest: { sessionName: "default", paneId: "elsewhere", seq: 2 } }))
    expect(paneA).toHaveAttribute("aria-pressed", "true")

    act(() => useHerdrStore.setState({ paneFocusRequest: { sessionName: "default", paneId: "p3", seq: 3 } }))
    expect(paneC).toHaveAttribute("aria-pressed", "true")
    expect(paneA).toHaveAttribute("aria-pressed", "false")

    // After a local click elsewhere, re-selecting the same Agent applies again.
    fireEvent.click(paneA)
    await waitFor(() => expect(paneA).toHaveAttribute("aria-pressed", "true"))
    act(() => useHerdrStore.setState({ paneFocusRequest: { sessionName: "default", paneId: "p3", seq: 4 } }))
    expect(paneC).toHaveAttribute("aria-pressed", "true")

    expect(layoutMock.export).toHaveBeenCalledTimes(exports)
    expect(herdrTerminalOpen).toHaveBeenCalledTimes(3)
    expect(herdrTerminalRelease).not.toHaveBeenCalled()
  })

  it.each(["A", "C"])("delivers the last focus intent after B is pending then %s is clicked", async (last) => {
    const pending: { paneId: string; resolve: () => void }[] = []
    let serverFocus = "p1"
    vi.mocked(herdrPaneFocus).mockImplementation(({ paneId }) => new Promise<void>((resolve) => {
      pending.push({ paneId, resolve: () => { serverFocus = paneId; resolve() } })
    }))
    render(<HerdrTerminalPage herdrSessionId="default" terminalId="t1" herdrTabId="tab-1" active visible />)
    await waitFor(() => expect(herdrTerminalOpen).toHaveBeenCalledTimes(3))
    fireEvent.click(screen.getByRole("button", { name: "Focus terminal: B" }))
    fireEvent.click(screen.getByRole("button", { name: `Focus terminal: ${last}` }))
    const sentBeforeConfirmation = pending.length
    await act(async () => pending[0].resolve())
    const requests = pending.map(request => request.paneId)
    if (pending[1]) await act(async () => pending[1].resolve())

    expect(sentBeforeConfirmation).toBe(1)
    expect(requests).toEqual(["p2", last === "A" ? "p1" : "p3"])
    expect(serverFocus).toBe(last === "A" ? "p1" : "p3")
    expect(screen.getByRole("button", { name: `Focus terminal: ${last}` })).toHaveAttribute("aria-pressed", "true")
    expect(herdrTerminalOpen).toHaveBeenCalledTimes(3)
    expect(herdrTerminalRelease).not.toHaveBeenCalled()
  })

  it("coalesces unsent focus intents and keeps only the final request", async () => {
    let finish!: () => void
    vi.mocked(herdrPaneFocus).mockImplementationOnce(() => new Promise<void>(resolve => { finish = resolve }))
    render(<HerdrTerminalPage herdrSessionId="default" terminalId="t1" herdrTabId="tab-1" active visible />)
    await waitFor(() => expect(herdrTerminalOpen).toHaveBeenCalledTimes(3))
    for (const pane of ["B", "C", "A"]) fireEvent.click(screen.getByRole("button", { name: `Focus terminal: ${pane}` }))
    await act(async () => finish())
    expect(vi.mocked(herdrPaneFocus).mock.calls.map(([args]) => args.paneId)).toEqual(["p2", "p1"])
    expect(screen.getByRole("button", { name: "Focus terminal: A" })).toHaveAttribute("aria-pressed", "true")
  })

  it("does not repeat an already successful in-flight focus target", async () => {
    let finish!: () => void
    vi.mocked(herdrPaneFocus).mockImplementationOnce(() => new Promise<void>(resolve => { finish = resolve }))
    render(<HerdrTerminalPage herdrSessionId="default" terminalId="t1" herdrTabId="tab-1" active visible />)
    await waitFor(() => expect(herdrTerminalOpen).toHaveBeenCalledTimes(3))
    const pane = screen.getByRole("button", { name: "Focus terminal: B" })
    fireEvent.click(pane)
    fireEvent.click(pane)
    await act(async () => finish())
    expect(herdrPaneFocus).toHaveBeenCalledTimes(1)
    expect(pane).toHaveAttribute("aria-pressed", "true")
  })

  it.each(["B", "C"])("continues to the requested %s after an in-flight failure", async (last) => {
    let fail!: (error: Error) => void
    vi.mocked(herdrPaneFocus).mockImplementationOnce(() => new Promise<void>((_resolve, reject) => { fail = reject }))
    render(<HerdrTerminalPage herdrSessionId="default" terminalId="t1" herdrTabId="tab-1" active visible />)
    await waitFor(() => expect(herdrTerminalOpen).toHaveBeenCalledTimes(3))
    fireEvent.click(screen.getByRole("button", { name: "Focus terminal: B" }))
    fireEvent.click(screen.getByRole("button", { name: `Focus terminal: ${last}` }))
    const sentBeforeFailure = vi.mocked(herdrPaneFocus).mock.calls.length
    await act(async () => fail(new Error("focus unavailable")))
    expect(sentBeforeFailure).toBe(1)
    expect(vi.mocked(herdrPaneFocus).mock.calls.map(([args]) => args.paneId)).toEqual(["p2", last === "B" ? "p2" : "p3"])
    expect(screen.getByRole("button", { name: `Focus terminal: ${last}` })).toHaveAttribute("aria-pressed", "true")
  })

  it("allows an explicit retry after a failed focus without optimistic confirmation", async () => {
    vi.mocked(herdrPaneFocus).mockRejectedValueOnce(new Error("focus unavailable"))
    render(<HerdrTerminalPage herdrSessionId="default" terminalId="t1" herdrTabId="tab-1" active visible />)
    await waitFor(() => expect(herdrTerminalOpen).toHaveBeenCalledTimes(3))
    const pane = screen.getByRole("button", { name: "Focus terminal: B" })
    await act(async () => fireEvent.click(pane))
    expect(pane).toHaveAttribute("aria-pressed", "false")
    expect(screen.getByRole("button", { name: "Focus terminal: A" })).toHaveAttribute("aria-pressed", "true")
    await act(async () => fireEvent.click(pane))
    expect(pane).toHaveAttribute("aria-pressed", "true")
    expect(herdrPaneFocus).toHaveBeenCalledTimes(2)
  })

  it("keeps the last successful focus when the newer queued request fails", async () => {
    let finishB!: () => void
    let failC!: (error: Error) => void
    vi.mocked(herdrPaneFocus)
      .mockImplementationOnce(() => new Promise<void>(resolve => { finishB = resolve }))
      .mockImplementationOnce(() => new Promise<void>((_resolve, reject) => { failC = reject }))
    render(<HerdrTerminalPage herdrSessionId="default" terminalId="t1" herdrTabId="tab-1" active visible />)
    await waitFor(() => expect(herdrTerminalOpen).toHaveBeenCalledTimes(3))
    fireEvent.click(screen.getByRole("button", { name: "Focus terminal: B" }))
    fireEvent.click(screen.getByRole("button", { name: "Focus terminal: C" }))
    await act(async () => finishB())
    await act(async () => failC(new Error("focus unavailable")))
    expect(screen.getByRole("button", { name: "Focus terminal: B" })).toHaveAttribute("aria-pressed", "true")
    expect(screen.getByRole("button", { name: "Focus terminal: C" })).toHaveAttribute("aria-pressed", "false")
  })

  it("accepts a return to A after B confirms but before React commits B", async () => {
    let finish!: () => void
    vi.mocked(herdrPaneFocus).mockImplementationOnce(() => new Promise<void>(resolve => { finish = resolve }))
    render(<HerdrTerminalPage herdrSessionId="default" terminalId="t1" herdrTabId="tab-1" active visible />)
    await waitFor(() => expect(herdrTerminalOpen).toHaveBeenCalledTimes(3))
    const paneA = screen.getByRole("button", { name: "Focus terminal: A" })
    fireEvent.click(screen.getByRole("button", { name: "Focus terminal: B" }))
    await act(async () => {
      finish()
      await Promise.resolve()
      fireEvent.click(paneA)
    })
    expect(vi.mocked(herdrPaneFocus).mock.calls.map(([args]) => args.paneId)).toEqual(["p2", "p1"])
    expect(paneA).toHaveAttribute("aria-pressed", "true")
  })

  it("keeps confirmed B if topology removes only the queued C", async () => {
    const original = layoutMock.get()
    let finish!: () => void
    vi.mocked(herdrPaneFocus).mockImplementationOnce(() => new Promise<void>(resolve => { finish = resolve }))
    const view = render(<HerdrTerminalPage herdrSessionId="default" terminalId="t1" herdrTabId="tab-1" active visible />)
    try {
      await waitFor(() => expect(herdrTerminalOpen).toHaveBeenCalledTimes(3))
      fireEvent.click(screen.getByRole("button", { name: "Focus terminal: B" }))
      fireEvent.click(screen.getByRole("button", { name: "Focus terminal: C" }))
      act(() => {
        layoutMock.set({ ...original, root: { type: "split", direction: "right", ratio: 0.6,
          first: { type: "pane", paneId: "p1", label: "A", cwd: null },
          second: { type: "pane", paneId: "p2", label: "B", cwd: null } } })
        useHerdrStore.getState().bumpTopologyRevision()
      })
      await waitFor(() => expect(screen.queryByRole("button", { name: "Focus terminal: C" })).not.toBeInTheDocument())
      await act(async () => finish())
      expect(herdrPaneFocus).toHaveBeenCalledTimes(1)
      expect(screen.getByRole("button", { name: "Focus terminal: B" })).toHaveAttribute("aria-pressed", "true")
    } finally {
      view.unmount()
      layoutMock.set(original)
    }
  })

  it("does not touch a removed pane's Agent recency after late confirmation", async () => {
    const original = layoutMock.get()
    const originalMruKeys = useAgentMruStore.getState().keys
    const snapshot = useHerdrStore.getState().snapshot!
    const agentSnapshot = { ...snapshot, agents: [{ id: "agent-b", name: "B", status: "idle" as const,
      workspaceId: "ws-1", paneId: "p2", terminalId: "t2", tabId: "tab-1" }] }
    useHerdrStore.setState({ snapshot: agentSnapshot, runtimesBySession: { default: {
      connectionState: "ready", capabilities: layoutCapabilities(true), snapshot: agentSnapshot,
      worktreeInventory: null, errorMessage: null,
    } } })
    const touch = vi.spyOn(useAgentMruStore.getState(), "touch")
    let finish!: () => void
    vi.mocked(herdrPaneFocus).mockImplementationOnce(() => new Promise<void>(resolve => { finish = resolve }))
    const view = render(<HerdrTerminalPage herdrSessionId="default" terminalId="t1" herdrTabId="tab-1" active visible />)
    try {
      await waitFor(() => expect(herdrTerminalOpen).toHaveBeenCalledTimes(3))
      fireEvent.click(screen.getByRole("button", { name: "Focus terminal: B" }))
      act(() => {
        layoutMock.set({ ...original, root: { type: "pane", paneId: "p1", label: "A", cwd: null } })
        useHerdrStore.getState().bumpTopologyRevision()
      })
      await waitFor(() => expect(screen.queryByRole("button", { name: "Focus terminal: B" })).not.toBeInTheDocument())
      await act(async () => finish())
      expect(touch).not.toHaveBeenCalled()
    } finally {
      view.unmount()
      layoutMock.set(original)
      touch.mockRestore()
      useAgentMruStore.setState({ keys: originalMruKeys })
    }
  })

  it.each(["unmount", "new-page"] as const)("retires queued focus on %s", async (boundary) => {
    let finish!: () => void
    vi.mocked(herdrPaneFocus).mockImplementationOnce(() => new Promise<void>(resolve => { finish = resolve }))
    const view = render(<HerdrTerminalPage herdrSessionId="default" terminalId="t1" herdrTabId="tab-1" active visible />)
    await waitFor(() => expect(herdrTerminalOpen).toHaveBeenCalledTimes(3))
    fireEvent.click(screen.getByRole("button", { name: "Focus terminal: B" }))
    fireEvent.click(screen.getByRole("button", { name: "Focus terminal: C" }))
    if (boundary === "unmount") view.unmount()
    else view.rerender(<HerdrTerminalPage herdrSessionId="default" terminalId="t1" herdrTabId="tab-1" pagePath="yuzora://herdr/default/another-page" active visible />)
    await act(async () => finish())
    expect(herdrPaneFocus).toHaveBeenCalledTimes(1)
    if (boundary === "new-page") expect(screen.getByRole("button", { name: "Focus terminal: A" })).toHaveAttribute("aria-pressed", "true")
  })

  it.each(["inactive", "hidden"] as const)("discards an unsent focus when the page becomes %s", async (boundary) => {
    let finish!: () => void
    vi.mocked(herdrPaneFocus).mockImplementationOnce(() => new Promise<void>(resolve => { finish = resolve }))
    const view = render(<HerdrTerminalPage herdrSessionId="default" terminalId="t1" herdrTabId="tab-1" active visible />)
    await waitFor(() => expect(herdrTerminalOpen).toHaveBeenCalledTimes(3))
    fireEvent.click(screen.getByRole("button", { name: "Focus terminal: B" }))
    fireEvent.click(screen.getByRole("button", { name: "Focus terminal: C" }))
    view.rerender(<HerdrTerminalPage herdrSessionId="default" terminalId="t1" herdrTabId="tab-1" active={boundary !== "inactive"} visible={boundary !== "hidden"} />)
    await act(async () => finish())
    expect(herdrPaneFocus).toHaveBeenCalledTimes(1)
    view.rerender(<HerdrTerminalPage herdrSessionId="default" terminalId="t1" herdrTabId="tab-1" active visible />)
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Focus terminal: C" })))
    expect(vi.mocked(herdrPaneFocus).mock.calls.map(([args]) => args.paneId)).toEqual(["p2", "p3"])
    expect(screen.getByRole("button", { name: "Focus terminal: C" })).toHaveAttribute("aria-pressed", "true")
  })

  it("discards an unsent local focus when activation switches to another tab", async () => {
    let finish!: () => void
    vi.mocked(herdrPaneFocus).mockImplementationOnce(() => new Promise<void>(resolve => { finish = resolve }))
    render(<HerdrTerminalPage herdrSessionId="default" terminalId="t1" herdrTabId="tab-1" active visible />)
    await waitFor(() => expect(herdrTerminalOpen).toHaveBeenCalledTimes(3))
    fireEvent.click(screen.getByRole("button", { name: "Focus terminal: B" }))
    fireEvent.click(screen.getByRole("button", { name: "Focus terminal: C" }))
    act(() => useHerdrStore.setState({ paneFocusRequest: { sessionName: "default", paneId: "other-tab-pane", seq: 1 } }))
    await act(async () => finish())
    expect(herdrPaneFocus).toHaveBeenCalledTimes(1)
  })

  it("does not send a queued focus for a pane removed by topology refresh", async () => {
    const original = layoutMock.get()
    let finish!: () => void
    vi.mocked(herdrPaneFocus).mockImplementationOnce(() => new Promise<void>(resolve => { finish = resolve }))
    const view = render(<HerdrTerminalPage herdrSessionId="default" terminalId="t1" herdrTabId="tab-1" active visible />)
    try {
      await waitFor(() => expect(herdrTerminalOpen).toHaveBeenCalledTimes(3))
      fireEvent.click(screen.getByRole("button", { name: "Focus terminal: B" }))
      fireEvent.click(screen.getByRole("button", { name: "Focus terminal: C" }))
      act(() => {
        layoutMock.set({ ...original, root: { type: "pane", paneId: "p1", label: "A", cwd: null } })
        useHerdrStore.getState().bumpTopologyRevision()
      })
      await waitFor(() => expect(screen.queryByRole("button", { name: "Focus terminal: C" })).not.toBeInTheDocument())
      await act(async () => finish())
      expect(herdrPaneFocus).toHaveBeenCalledTimes(1)
      expect(screen.getByTestId("herdr-terminal-page-t1")).toHaveAttribute("data-pane-count", "1")
    } finally {
      view.unmount()
      layoutMock.set(original)
    }
  })

  it("reasserts a confirmed Agent activation after an older local focus settles", async () => {
    const pending: { paneId: string; resolve: () => void }[] = []
    let serverFocus: string
    vi.mocked(herdrPaneFocus).mockImplementation(({ paneId }) => new Promise<void>((resolve) => {
      pending.push({ paneId, resolve: () => { serverFocus = paneId; resolve() } })
    }))
    render(<HerdrTerminalPage herdrSessionId="default" terminalId="t1" herdrTabId="tab-1" active visible />)
    await waitFor(() => expect(herdrTerminalOpen).toHaveBeenCalledTimes(3))
    fireEvent.click(screen.getByRole("button", { name: "Focus terminal: B" }))
    serverFocus = "p3"
    act(() => useHerdrStore.setState({ paneFocusRequest: { sessionName: "default", paneId: "p3", seq: 1 } }))
    expect(screen.getByRole("button", { name: "Focus terminal: C" })).toHaveAttribute("aria-pressed", "true")
    await act(async () => pending[0].resolve())
    const requests = pending.map(request => request.paneId)
    if (pending[1]) await act(async () => pending[1].resolve())
    expect(requests).toEqual(["p2", "p3"])
    expect(serverFocus).toBe("p3")
    expect(screen.getByRole("button", { name: "Focus terminal: C" })).toHaveAttribute("aria-pressed", "true")
    expect(herdrTerminalOpen).toHaveBeenCalledTimes(3)
    expect(herdrTerminalRelease).not.toHaveBeenCalled()
  })

  it("zooms only the focused pane without releasing connectors and restores the split", async () => {
    render(<HerdrTerminalPage herdrSessionId="default" terminalId="t1" herdrTabId="tab-1" active visible />)
    await waitFor(() => expect(herdrTerminalOpen).toHaveBeenCalledTimes(3))
    act(() => {
      layoutMock.set({ ...layoutMock.get(), zoomed: true, focusedPaneId: "p2" })
      useHerdrStore.getState().bumpTopologyRevision()
    })
    await waitFor(() => expect(screen.getByTestId("herdr-split-handle-root")).not.toBeVisible())
    expect(screen.getByTestId("herdr-terminal-leaf-t1")).not.toBeVisible()
    expect(screen.getByTestId("herdr-terminal-leaf-t2")).toBeVisible()
    expect(screen.getByTestId("herdr-terminal-leaf-t2")).toContainElement(screen.getByText("Focused"))
    expect(screen.getByTestId("herdr-terminal-leaf-t3")).not.toBeVisible()
    expect(herdrTerminalRelease).not.toHaveBeenCalled()
    act(() => {
      layoutMock.set({ ...layoutMock.get(), zoomed: false })
      useHerdrStore.getState().bumpTopologyRevision()
    })
    await waitFor(() => expect(screen.getByTestId("herdr-split-handle-root")).toBeVisible())
    expect(screen.getByTestId("herdr-terminal-leaf-t1")).toBeVisible()
    expect(herdrTerminalOpen).toHaveBeenCalledTimes(3)
  })

  it("uses the page's owning named-session snapshot after sidebar session changes", async () => {
    const defaultSnapshot = useHerdrStore.getState().snapshot
    expect(defaultSnapshot).not.toBeNull()
    useHerdrStore.setState({
      sessions: [
        {
          name: "default",
          default: true,
          running: true,
          sessionDir: "/tmp/d",
          socketPath: "/tmp/d.sock"
        },
        {
          name: "work",
          default: false,
          running: true,
          sessionDir: "/tmp/w",
          socketPath: "/tmp/w.sock"
        }
      ],
      selectedSessionName: "work",
      snapshot: {
        herdrSessionId: "work",
        protocol: 19,
        version: "0.8.0",
        spaces: [],
        agents: [],
        tabs: [],
        terminals: [{ terminalId: "other", paneId: "other-pane", tabId: "other-tab" }],
        raw: {}
      },
      runtimesBySession: {
        default: {
          capabilities: layoutCapabilities(true),
          snapshot: defaultSnapshot,
          worktreeInventory: null,
          connectionState: "ready",
          errorMessage: null
        }
      }
    })

    render(
      <HerdrTerminalPage
        herdrSessionId="default"
        terminalId="t1"
        herdrTabId="tab-1"
        pagePath="yuzora://herdr/default/t1"
        active
        visible
      />
    )

    await waitFor(() => expect(herdrTerminalOpen).toHaveBeenCalledTimes(3))
    expect(screen.queryByTestId("herdr-leaf-missing-terminal")).not.toBeInTheDocument()
  })

  it("keeps multi-pane layout visible but makes split separators inert when this runtime cannot set ratios", async () => {
    seed(false)
    render(
      <HerdrTerminalPage
        herdrSessionId="default"
        terminalId="t1"
        herdrTabId="tab-1"
        pagePath="yuzora://herdr/default/t1"
        active
        visible
      />
    )

    const handle = await screen.findByTestId("herdr-split-handle-root")
    expect(screen.getByTestId("herdr-split-root")).toBeInTheDocument()
    expect(handle).toHaveAttribute("aria-disabled", "true")
    expect(handle).not.toHaveAttribute("tabindex")
    expect(handle).toHaveClass("pointer-events-none")
    expect(screen.getByTestId("herdr-split-resize-unavailable")).toHaveTextContent(
      "Pane resizing is unavailable",
    )

    expect(vi.mocked(herdrLayoutSetSplitRatio)).not.toHaveBeenCalled()
  })

  it("does not open a connector when the exact runtime reports terminal control unavailable", async () => {
    const capabilities = useHerdrStore.getState().capabilities!
    useHerdrStore.setState({
      capabilities: {
        ...capabilities,
        terminal: {
          ...capabilities.terminal,
          control: false,
          takeover: false,
          input: false,
          resize: false,
          scroll: false,
          release: false,
          reason: "verified control plane unavailable"
        }
      }
    })

    render(
      <HerdrTerminalPage
        herdrSessionId="default"
        terminalId="t1"
        herdrTabId="tab-1"
        pagePath="yuzora://herdr/default/t1"
        active
        visible
      />
    )

    await screen.findByTestId("herdr-split-root")
    expect(vi.mocked(herdrTerminalOpen)).not.toHaveBeenCalled()
    expect(screen.getByRole("status")).toHaveTextContent("verified control plane unavailable")
  })

  it("keeps split resizing enabled when the exact runtime supports layout.set_split_ratio", async () => {
    render(
      <HerdrTerminalPage
        herdrSessionId="default"
        terminalId="t1"
        herdrTabId="tab-1"
        pagePath="yuzora://herdr/default/t1"
        active
        visible
      />
    )

    const handle = await screen.findByTestId("herdr-split-handle-root")
    expect(handle).toHaveAttribute("tabindex", "0")
    expect(handle).not.toHaveAttribute("aria-disabled")
    expect(handle).not.toHaveClass("pointer-events-none")
  })

  it("does not write split ratio during initial hydration", async () => {
    render(
      <HerdrTerminalPage
        herdrSessionId="default"
        terminalId="t1"
        herdrTabId="tab-1"
        pagePath="yuzora://herdr/default/t1"
        active
        visible
      />
    )
    await waitFor(() => expect(layoutMock.export).toHaveBeenCalled())
    await act(async () => {
      await Promise.resolve()
      await Promise.resolve()
    })
    expect(layoutMock.setRatio).not.toHaveBeenCalled()
  })

  it.each(["local", "external"])("keeps newer %s focus when a prior ratio snapshot arrives", async (source) => {
    let finishRatio!: () => void
    vi.mocked(herdrLayoutSetSplitRatio).mockImplementationOnce(async () => {
      const response = structuredClone(layoutMock.get())
      await new Promise<void>(resolve => { finishRatio = resolve })
      return response
    })
    render(<HerdrTerminalPage herdrSessionId="default" terminalId="t1" herdrTabId="tab-1" active visible />)
    await waitFor(() => expect(herdrTerminalOpen).toHaveBeenCalledTimes(3))
    const handle = await screen.findByTestId("herdr-split-handle-root")
    fireEvent.keyDown(handle, { key: "ArrowRight" })
    await waitFor(() => expect(herdrLayoutSetSplitRatio).toHaveBeenCalledTimes(1))
    if (source === "local") fireEvent.click(screen.getByRole("button", { name: "Focus terminal: B" }))
    else act(() => useHerdrStore.setState({ paneFocusRequest: { sessionName: "default", paneId: "p2", seq: 1 } }))
    await waitFor(() => expect(screen.getByRole("button", { name: "Focus terminal: B" })).toHaveAttribute("aria-pressed", "true"))
    await act(async () => finishRatio())
    expect(screen.getByRole("button", { name: "Focus terminal: B" })).toHaveAttribute("aria-pressed", "true")
    expect(herdrTerminalOpen).toHaveBeenCalledTimes(3)
    expect(herdrTerminalRelease).not.toHaveBeenCalled()
  })

  describe("split ratio response ordering", () => {
    type Request = Parameters<typeof herdrLayoutSetSplitRatio>[0]
    let server: HerdrLayoutDescription
    let replies: { request: Request; finish: (focus?: string) => void; fail: () => void }[]

    beforeEach(() => {
      server = {
        workspaceId: "ws-1", tabId: "tab-1", zoomed: false, focusedPaneId: "p1",
        root: { type: "split", direction: "right", ratio: 0.6,
          first: { type: "pane", paneId: "p1", label: "A", cwd: null },
          second: { type: "split", direction: "down", ratio: 0.5,
            first: { type: "pane", paneId: "p2", label: "B", cwd: null },
            second: { type: "pane", paneId: "p3", label: "C", cwd: null } } }
      }
      layoutMock.set(server)
      replies = []
      vi.mocked(herdrLayoutSetSplitRatio).mockReset().mockImplementation(request => new Promise((resolve, reject) => {
        replies.push({ request, fail: () => reject(new Error("fixture ratio failure")), finish: (focus) => {
          const next = structuredClone(server)
          let node = next.root
          for (const second of request.path) {
            if (node.type !== "split") throw new Error("fixture invalid path")
            node = second ? node.second : node.first
          }
          if (node.type !== "split") throw new Error("fixture expected split")
          node.ratio = request.ratio
          if (focus) next.focusedPaneId = focus
          server = next
          resolve(next)
        } })
      }))
    })

    afterEach(() => {
      vi.useRealTimers()
      vi.mocked(herdrLayoutSetSplitRatio).mockReset().mockImplementation(args => layoutMock.setRatio(args))
    })

    async function mountRatioPage() {
      const view = render(<HerdrTerminalPage herdrSessionId="default" terminalId="t1" herdrTabId="tab-1" active visible />)
      await waitFor(() => expect(herdrTerminalOpen).toHaveBeenCalledTimes(3))
      await act(async () => { await new Promise(resolve => setTimeout(resolve, 0)) })
      vi.useFakeTimers()
      return view
    }

    async function resize(path = "root", key = "ArrowRight") {
      fireEvent.keyDown(screen.getByTestId(`herdr-split-handle-${path}`), { key })
      await act(async () => vi.advanceTimersByTimeAsync(300))
    }

    function confirmedRatio(path: string) {
      return Number(document.getElementById(`herdr-split-${path}-first`)?.getAttribute("data-confirmed-size")) / 100
    }

    it.each([false, true])("accepts server focus with no newer confirmation (local failure=%s)", async (failedFocus) => {
      await mountRatioPage()
      await resize()
      if (failedFocus) {
        vi.mocked(herdrPaneFocus).mockRejectedValueOnce(new Error("focus failed"))
        await act(async () => fireEvent.click(screen.getByRole("button", { name: "Focus terminal: C" })))
      }
      await act(async () => replies[0].finish("p2"))
      expect(screen.getByRole("button", { name: "Focus terminal: B" })).toHaveAttribute("aria-pressed", "true")
      expect(confirmedRatio("root")).toBeCloseTo(replies[0].request.ratio)
    })

    it.each([false, true])("serializes distinct paths and keeps the first success (second failure=%s)", async (failedSecond) => {
      await mountRatioPage()
      await resize()
      await resize("1", "ArrowDown")
      const sentBeforeFirstReply = replies.length
      await act(async () => replies[0].finish())
      expect(sentBeforeFirstReply).toBe(1)
      expect(replies.map(reply => reply.request.path)).toEqual([[], [true]])
      expect(confirmedRatio("root")).toBeCloseTo(replies[0].request.ratio)
      await act(async () => failedSecond ? replies[1].fail() : replies[1].finish())
      expect(confirmedRatio("root")).toBeCloseTo(replies[0].request.ratio)
      expect(confirmedRatio("1")).toBeCloseTo(failedSecond ? 0.5 : replies[1].request.ratio)
      expect(herdrTerminalOpen).toHaveBeenCalledTimes(3)
      expect(herdrTerminalRelease).not.toHaveBeenCalled()
    })

    it("coalesces the latest unsent same-path ratio and continues after a failed request", async () => {
      await mountRatioPage()
      await resize()
      await resize()
      await resize()
      const sentBeforeFailure = replies.length
      const finalPercent = Number(screen.getByTestId("herdr-split-handle-root").getAttribute("aria-valuenow"))
      await act(async () => replies[0].fail())
      expect(sentBeforeFailure).toBe(1)
      expect(replies).toHaveLength(2)
      expect(replies[1].request.ratio).toBeCloseTo(finalPercent / 100)
      await act(async () => replies[1].finish())
      expect(confirmedRatio("root")).toBeCloseTo(finalPercent / 100)
    })

    it("cancels an unsent ratio when the user returns to the in-flight value", async () => {
      await mountRatioPage()
      await resize()
      await resize()
      await resize("root", "ArrowLeft")
      await act(async () => replies[0].finish())
      expect(replies).toHaveLength(1)
      expect(confirmedRatio("root")).toBeCloseTo(replies[0].request.ratio)
    })

    it.each(["topology", "capability", "tab", "page", "unmount"])(
      "retires queued ratio work after %s changes", async (change) => {
        const view = await mountRatioPage()
        await resize()
        await resize("1", "ArrowDown")
        const sentBeforeRetirement = replies.length
        if (change === "topology") act(() => useHerdrStore.getState().bumpTopologyRevision())
        if (change === "capability") act(() => useHerdrStore.setState({ capabilities: layoutCapabilities(false) }))
        if (change === "unmount") view.unmount()
        if (change === "tab") {
          layoutMock.set({ ...server, tabId: "tab-2" })
          act(() => useHerdrStore.setState(state => ({ snapshot: {
            ...state.snapshot!, terminals: state.snapshot!.terminals.map(terminal => ({ ...terminal, tabId: "tab-2" }))
          } })))
        }
        if (["tab", "page"].includes(change)) view.rerender(
          <HerdrTerminalPage herdrSessionId="default" terminalId="t1" herdrTabId={change === "tab" ? "tab-2" : "tab-1"}
            pagePath={change === "page" ? "yuzora://herdr/another-page" : undefined}
            active visible />
        )
        await act(async () => vi.advanceTimersByTimeAsync(0))
        await act(async () => replies[0].finish())
        expect(sentBeforeRetirement).toBe(1)
        expect(replies).toHaveLength(1)
        if (["topology", "capability", "tab", "page"].includes(change)) expect(confirmedRatio("root")).toBe(0.6)
      }
    )

    it.each(["hidden", "inactive"])("persists pending geometry when the same page becomes %s", async (change) => {
      const view = await mountRatioPage()
      await resize()
      await resize("1", "ArrowDown")
      view.rerender(<HerdrTerminalPage herdrSessionId="default" terminalId="t1" herdrTabId="tab-1"
        active={change !== "inactive"} visible={change !== "hidden"} />)
      await act(async () => replies[0].finish())
      expect(replies).toHaveLength(2)
      await act(async () => replies[1].finish())
      expect(confirmedRatio("root")).toBeCloseTo(replies[0].request.ratio)
      expect(confirmedRatio("1")).toBeCloseTo(replies[1].request.ratio)
      expect(herdrPaneFocus).not.toHaveBeenCalled()
      expect(herdrTerminalOpen).toHaveBeenCalledTimes(3)
      expect(herdrTerminalRelease).not.toHaveBeenCalled()
    })

    it("retires old path intents when a ratio reply changes topology, then accepts new-tree resizing", async () => {
      await mountRatioPage()
      await resize()
      await resize("1", "ArrowDown")
      const next = structuredClone(server)
      if (next.root.type !== "split" || next.root.second.type !== "split") throw new Error("fixture needs nested split")
      const nested = next.root.second
      const first = nested.first
      nested.first = nested.second
      nested.second = first
      server = next
      await act(async () => replies[0].finish())
      expect(replies).toHaveLength(1)
      const secondSplit = screen.getByTestId("herdr-split-1")
      expect(secondSplit.textContent!.indexOf("C")).toBeLessThan(secondSplit.textContent!.indexOf("B"))
      await resize("1", "ArrowDown")
      expect(replies).toHaveLength(2)
      await act(async () => replies[1].finish())
      expect(confirmedRatio("1")).toBeCloseTo(replies[1].request.ratio)
    })

    it("uses newly accepted server focus for a click before React commits the ratio response", async () => {
      await mountRatioPage()
      await resize()
      const paneA = screen.getByRole("button", { name: "Focus terminal: A" })
      await act(async () => {
        replies[0].finish("p2")
        await Promise.resolve()
        fireEvent.click(paneA)
      })
      expect(herdrPaneFocus).toHaveBeenCalledWith({ sessionName: "default", paneId: "p1" })
      expect(paneA).toHaveAttribute("aria-pressed", "true")
    })
  })

  it("reloads layout on topology revision without closing the page", async () => {
    render(
      <HerdrTerminalPage
        herdrSessionId="default"
        terminalId="t1"
        herdrTabId="tab-1"
        pagePath="yuzora://herdr/default/t1"
        active
        visible
      />
    )
    await waitFor(() => expect(layoutMock.export).toHaveBeenCalledTimes(1))
    act(() => {
      useHerdrStore.getState().bumpTopologyRevision()
    })
    await waitFor(() => expect(layoutMock.export).toHaveBeenCalledTimes(2))
    expect(screen.getByTestId("herdr-terminal-page-t1")).toBeInTheDocument()
  })

  it("retains the WSL split and connectors when a background layout request is temporarily busy", async () => {
    const scope = JSON.stringify(["wsl:Ubuntu-26.04", "default"])
    const state = useHerdrStore.getState()
    useHerdrStore.setState({
      selectedSessionName: scope,
      sessions: [{ ...state.sessions[0], hostId: "wsl:Ubuntu-26.04", runtimeId: scope }],
      runtimesBySession: { [scope]: { snapshot: state.snapshot, capabilities: state.capabilities, connectionState: "ready", worktreeInventory: null, errorMessage: null } }
    })
    render(<HerdrTerminalPage herdrSessionId={scope} terminalId="t1" herdrTabId="tab-1" active visible />)
    await waitFor(() => expect(herdrTerminalOpen).toHaveBeenCalledTimes(3))
    layoutMock.export.mockRejectedValueOnce(new Error("host-request-wait-timeout"))
    act(() => useHerdrStore.getState().bumpTopologyRevision())
    await waitFor(() => expect(layoutMock.export).toHaveBeenCalledTimes(2))
    expect(screen.getByTestId("herdr-split-root")).toBeInTheDocument()
    expect(screen.queryByTestId("herdr-layout-fallback")).not.toBeInTheDocument()
    expect(herdrTerminalRelease).not.toHaveBeenCalled()
    await waitFor(() => expect(layoutMock.export).toHaveBeenCalledTimes(3), { timeout: 2500 })
    expect(layoutMock.export).toHaveBeenLastCalledWith({ sessionName: scope, tabId: "tab-1", paneId: null })
  })

  it("recovers multi-pane layout after a transient first WSL request failure", async () => {
    layoutMock.export.mockRejectedValueOnce(new Error("host-request-limit"))
    render(<HerdrTerminalPage herdrSessionId="default" terminalId="t1" herdrTabId="tab-1" active visible />)
    await waitFor(() => expect(screen.getByTestId("herdr-split-root")).toBeInTheDocument(), { timeout: 2500 })
    expect(screen.queryByTestId("herdr-layout-fallback")).not.toBeInTheDocument()
    expect(layoutMock.export).toHaveBeenCalledTimes(2)
  })

  it("uses the live terminal's tab instead of restored stale tab metadata", async () => {
    layoutMock.export.mockImplementationOnce(async (args) => {
      if ((args as { tabId: string }).tabId !== "tab-1") throw new Error("layout_not_found: layout target not found")
      return layoutMock.get()
    })
    render(<HerdrTerminalPage herdrSessionId="default" terminalId="t1" herdrTabId="old-tab" active visible />)
    await waitFor(() => expect(layoutMock.export).toHaveBeenCalled())
    expect(layoutMock.export).toHaveBeenCalledWith({ sessionName: "default", tabId: "tab-1", paneId: null })
    expect(await screen.findByTestId("herdr-split-root")).toBeInTheDocument()
  })

  it("bounds failed retries and lets the user recover without reopening the terminal page", async () => {
    layoutMock.export.mockRejectedValue(new Error("host-request-wait-timeout"))
    render(<HerdrTerminalPage herdrSessionId="default" terminalId="t1" herdrTabId="tab-1" active visible />)
    await screen.findByTestId("herdr-layout-fallback", {}, { timeout: 2500 })
    expect(layoutMock.export).toHaveBeenCalledTimes(3)
    layoutMock.export.mockImplementation(async () => layoutMock.get())
    fireEvent.click(screen.getByRole("button", { name: "Reload layout" }))
    await screen.findByTestId("herdr-split-root")
    expect(screen.queryByTestId("herdr-layout-fallback")).not.toBeInTheDocument()
    expect(layoutMock.export).toHaveBeenCalledTimes(4)
  })

  it("cancels a pending layout retry when the page closes", async () => {
    layoutMock.export.mockRejectedValue(new Error("host-request-limit"))
    const view = render(<HerdrTerminalPage herdrSessionId="default" terminalId="t1" herdrTabId="tab-1" active visible />)
    await waitFor(() => expect(layoutMock.export).toHaveBeenCalledOnce())
    view.unmount()
    await new Promise((resolve) => setTimeout(resolve, 500))
    expect(layoutMock.export).toHaveBeenCalledOnce()
    expect(herdrTerminalOpen).not.toHaveBeenCalled()
  })

  it("keeps the owning tab when the original terminal moved but the tab still has other panes", async () => {
    const snapshot = useHerdrStore.getState().snapshot!
    useHerdrStore.setState({ snapshot: { ...snapshot, terminals: snapshot.terminals.map((term) => term.terminalId === "t1" ? { ...term, tabId: "other-tab" } : term) } })
    render(<HerdrTerminalPage herdrSessionId="default" terminalId="t1" herdrTabId="tab-1" active visible />)
    await screen.findByTestId("herdr-split-root")
    expect(layoutMock.export).toHaveBeenCalledWith({ sessionName: "default", tabId: "tab-1", paneId: null })
  })

  it("releases only the removed leaf connector on unmount of page", async () => {
    const { unmount } = render(
      <HerdrTerminalPage
        herdrSessionId="default"
        terminalId="t1"
        herdrTabId="tab-1"
        pagePath="yuzora://herdr/default/t1"
        active
        visible
      />
    )
    await waitFor(() => expect(herdrTerminalOpen).toHaveBeenCalledTimes(3))
    unmount()
    await waitFor(() => {
      expect(vi.mocked(herdrTerminalRelease)).toHaveBeenCalledTimes(3)
    })
    expect(useHerdrStore.getState().attachments.size).toBe(0)
  })
})
