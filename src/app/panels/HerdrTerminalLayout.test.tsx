import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { HerdrLayoutDescription } from "@/lib/herdrTypes"
import { herdrInitialState, useHerdrStore } from "@/state/herdrStore"

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
      agentGet: false,
      agentRead: false,
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
  beforeEach(() => {
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
