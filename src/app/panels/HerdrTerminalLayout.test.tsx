import { act, cleanup, render, screen, waitFor } from "@testing-library/react"
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
  herdrLayoutSetSplitRatio: (args: { path: boolean[]; ratio: number }) =>
    layoutMock.setRatio(args)
}))

import { herdrAttachmentKey } from "@/lib/herdrPages"
import { herdrTerminalOpen, herdrTerminalRelease } from "@/lib/herdrIpc"
import { HerdrTerminalPage } from "./HerdrTerminalPage"

function seed() {
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
          capabilities: null,
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
      expect(vi.mocked(herdrTerminalRelease).mock.calls.length).toBeGreaterThanOrEqual(3)
    })
    expect(useHerdrStore.getState().attachments.size).toBe(0)
  })
})
