import { act, cleanup, render, screen, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { HerdrLayoutDescription } from "@/lib/herdrTypes"
import { herdrInitialState, herdrStoreRuntimeKey, useHerdrStore } from "@/state/herdrStore"

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
import { herdrLayoutSetSplitRatio, herdrTerminalOpen, herdrTerminalRelease } from "@/lib/herdrIpc"
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
    layoutMock.export.mockClear()
    layoutMock.setRatio.mockClear()
    vi.mocked(herdrLayoutSetSplitRatio).mockClear()
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

  it("does not borrow selected Native resize capability for an unselected WSL page", async () => {
    const ubuntu = { kind: "wsl" as const, distro: "Ubuntu" }
    const snapshot = useHerdrStore.getState().snapshot
    useHerdrStore.setState({
      sessions: [
        { ...useHerdrStore.getState().sessions[0]!, runtimeTarget: { kind: "native" } },
        { ...useHerdrStore.getState().sessions[0]!, runtimeTarget: ubuntu }
      ],
      selectedRuntimeTarget: { kind: "native" },
      capabilities: layoutCapabilities(true),
      runtimesBySession: {
        [herdrStoreRuntimeKey("default", ubuntu)]: {
          capabilities: layoutCapabilities(false),
          snapshot,
          worktreeInventory: null,
          connectionState: "ready",
          errorMessage: null
        }
      }
    })
    render(
      <HerdrTerminalPage
        herdrSessionId="default"
        runtimeTarget={ubuntu}
        terminalId="t1"
        herdrTabId="tab-1"
        pagePath="yuzora://herdr/wsl/ubuntu/t1"
        active
        visible
      />
    )

    const handle = await screen.findByTestId("herdr-split-handle-root")
    expect(handle).toHaveAttribute("aria-disabled", "true")
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
