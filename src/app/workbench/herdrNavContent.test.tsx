import { beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

import { HerdrNavContent } from "@/app/workbench/HerdrNavContent"
import { herdrInitialState, useHerdrStore } from "@/state/herdrStore"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { useTextInputDialogStore } from "@/state/textInputDialogStore"

vi.mock("@/lib/herdrIpc", () => ({
  herdrSessions: vi.fn(),
  herdrCapabilities: vi.fn(),
  herdrSnapshot: vi.fn(),
  herdrTabFocus: vi.fn(),
  herdrTabRename: vi.fn(),
  herdrTerminalCreate: vi.fn(),
  herdrTerminalRelease: vi.fn(),
  herdrWorkspaceFocus: vi.fn(),
  herdrWorkspaceCreate: vi.fn()
}))

function readyState(overrides: Record<string, unknown> = {}) {
  return {
    ...herdrInitialState,
    attachments: new Map(),
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
        running: false,
        sessionDir: "/tmp/w",
        socketPath: "/tmp/w.sock"
      }
    ],
    selectedSessionName: "default",
    connectionState: "ready" as const,
    selectedSpaceId: "ws-1",
    selectedSpaceBySession: { default: "ws-1" },
    capabilities: {
      binaryPath: "/bin/herdr",
      binarySource: { configured: "global" as const, resolved: "global" as const, available: true, path: "/bin/herdr", reason: null, restartRequired: false },
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
        paneFocus: true,
        paneRename: true,
        paneSplit: true,
        paneZoom: true,
        paneSwap: true,
        paneClose: true,
        layoutExport: true,
        layoutSetSplitRatio: true,
        agentGet: true,
        agentRead: true,
        eventsSubscribe: true,
        worktreeList: true,
        methods: [
          "session.snapshot",
          "workspace.focus",
          "workspace.create",
          "workspace.rename",
          "workspace.close",
          "tab.create",
          "tab.rename",
          "tab.close",
          "tab.focus",
          "pane.focus",
          "pane.rename",
          "pane.split",
          "pane.zoom",
          "pane.swap",
          "pane.close",
          "layout.export",
          "layout.set_split_ratio"
        ],
        schemaProtocol: 19,
        schemaVersion: 1,
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
    },
    snapshot: {
      herdrSessionId: "default",
      protocol: 19,
      version: "0.8.0",
      spaces: [{ id: "ws-1", label: "Main", order: 0, focused: true, status: "working" as const }],
      focusedWorkspaceId: "ws-1",
      focusedTabId: "tab-1",
      focusedPaneId: "pane-1",
      tabs: [
        {
          id: "tab-1",
          label: "Main",
          order: 1,
          workspaceId: "ws-1",
          paneCount: 1,
          status: "working" as const,
          active: true,
          focused: true,
          paneId: "pane-1",
          terminalId: "term-1",
          sessionName: "default"
        }
      ],
      agents: [
        {
          id: "ag-1",
          name: "pi",
          title: "Implementer",
          status: "working" as const,
          workspaceId: "ws-1",
          terminalId: "term-1",
          paneId: "pane-1",
          tabId: "tab-1",
          spaceLabel: "Main",
          sessionName: "default"
        }
      ],
      terminals: [],
      raw: {}
    },
    ...overrides
  }
}

describe("HerdrNavContent", () => {
  beforeEach(() => {
    cleanup()
    useHerdrStore.setState({ ...herdrInitialState, attachments: new Map() })
    useTextInputDialogStore.setState({ pending: null })
    useWorkspaceStore.setState({
      groups: [{ tabs: [], activePath: null }],
      activeGroupIndex: 0
    })
  })

  it("renders Session tabs + Agents only (no Spaces section)", () => {
    useHerdrStore.setState(readyState())
    render(<HerdrNavContent />)

    expect(screen.getByTestId("herdr-session-default")).toBeInTheDocument()
    expect(screen.getByTestId("herdr-session-work")).toBeInTheDocument()
    expect(screen.getByText("Agents")).toBeInTheDocument()
    expect(screen.queryByText("Spaces")).toBeNull()
    expect(screen.queryByTestId("herdr-space-ws-1")).toBeNull()
    expect(screen.getByText("Main")).toBeInTheDocument() // owning Space label
  })

  it("labels Session and Agent surfaces with the selected WSL Runtime", () => {
    const ubuntu = { kind: "wsl" as const, distro: "Ubuntu 開発" }
    const state = readyState({
      selectedRuntimeTarget: ubuntu,
      sessions: [{
        name: "default", default: true, running: true,
        sessionDir: "/tmp/u", socketPath: "/tmp/u.sock", runtimeTarget: ubuntu
      }]
    })
    const snapshot = state.snapshot as { agents: Array<Record<string, unknown>> }
    snapshot.agents[0] = { ...snapshot.agents[0]!, runtimeTarget: ubuntu }
    useHerdrStore.setState(state)
    render(<HerdrNavContent />)

    expect(screen.getByTestId("herdr-runtime-label")).toHaveTextContent("WSL: Ubuntu 開発")
    expect(screen.getByTestId("herdr-session-default")).toHaveAttribute("aria-description", "WSL: Ubuntu 開発")
    expect(screen.getByTestId("herdr-agent-ag-1")).toHaveTextContent("WSL: Ubuntu 開発")
  })

  it("does not add an unrequested per-agent Inspector action", () => {
    useHerdrStore.setState(readyState())

    render(<HerdrNavContent />)

    expect(screen.queryByTestId("herdr-inspect-ag-1")).toBeNull()
  })

  it("opens a page when an agent is activated", async () => {
    const activateAgent = vi.fn().mockResolvedValue({ ok: true })
    useHerdrStore.setState(
      readyState({
        activateAgent
      })
    )

    render(<HerdrNavContent />)
    fireEvent.click(screen.getByTestId("herdr-agent-ag-1"))
    expect(activateAgent).toHaveBeenCalled()
  })

  it("creates a terminal page in the selected Space via + and requests its name", async () => {
    const { herdrTerminalCreate, herdrTabRename } = await import("@/lib/herdrIpc")
    vi.mocked(herdrTerminalCreate).mockResolvedValue({
      terminalId: "term-new",
      paneId: "pane-new",
      tabId: "tab-new",
      workspaceId: "ws-1",
      title: "Shell"
    })
    useHerdrStore.setState(
      readyState({
        createTerminalInSelectedSpace: async () => ({
          herdrSessionId: "default",
          terminalId: "term-new",
          paneId: "pane-new",
          tabId: "tab-new",
          title: "Shell"
        }),
        canCreateTerminal: () => true,
        canMutateSelectedSession: () => true
      })
    )

    render(<HerdrNavContent />)
    fireEvent.click(screen.getByTestId("herdr-create-terminal"))

    await vi.waitFor(() => {
      expect(useTextInputDialogStore.getState().pending).toMatchObject({
        initialValue: "Shell"
      })
    })
    useTextInputDialogStore.getState().respond("Named shell")

    await vi.waitFor(() => {
      const tabs = useWorkspaceStore.getState().groups[0].tabs
      expect(tabs).toHaveLength(1)
      expect(tabs[0].terminalId).toBe("term-new")
      expect(tabs[0].herdrSessionId).toBe("default")
      expect(tabs[0].name).toBe("Named shell")
    })
    expect(herdrTabRename).toHaveBeenCalledWith({
      sessionName: "default",
      tabId: "tab-new",
      label: "Named shell"
    })
  })

  it("shows stopped guidance and disables mutations for stopped session tab", () => {
    useHerdrStore.setState(
      readyState({
        selectedSessionName: "work",
        connectionState: "stopped",
        selectedSpaceId: null,
        snapshot: null,
        canCreateTerminal: () => false,
        canMutateSelectedSession: () => false,
        createTerminalBlockedReason: () => "stopped"
      })
    )
    render(<HerdrNavContent />)
    expect(screen.getByTestId("herdr-session-stopped")).toBeInTheDocument()
    expect(screen.getByTestId("herdr-create-terminal")).toBeDisabled()
  })

  it("disables + with capability reason when create is unavailable", () => {
    useHerdrStore.setState(
      readyState({
        capabilities: {
          binaryPath: "/bin/herdr",
          server: { running: false },
          api: {
            snapshot: true,
            ping: false,
            tabCreate: false,
            workspaceFocus: false,
            workspaceCreate: false,
            workspaceRename: false,
            workspaceClose: false,
            tabRename: false,
            tabClose: false,
            tabFocus: false,
            paneFocus: false,
            paneRename: false,
            paneSplit: false,
            paneZoom: false,
            paneSwap: false,
            paneClose: false,
            layoutExport: false,
            layoutSetSplitRatio: false,
        agentGet: false,
        agentRead: false,
        eventsSubscribe: false,
        worktreeList: false,
            methods: [],
            reason: "herdr server not running or socket unavailable"
          },
          terminal: {
            observe: true,
            control: true,
            takeover: true,
            input: true,
            resize: true,
            scroll: true,
            release: true,
            create: false,
            reason: "herdr server not running or socket unavailable"
          },
          events: { status: "deferred" }
        },
        canCreateTerminal: () => false,
        createTerminalBlockedReason: () =>
          "herdr server not running or socket unavailable"
      })
    )

    render(<HerdrNavContent />)
    const button = screen.getByTestId("herdr-create-terminal")
    expect(button).toBeDisabled()
    expect(button.getAttribute("title") ?? "").toContain("socket")
  })
})
