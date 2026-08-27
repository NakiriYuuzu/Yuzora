import { beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"

import { HerdrNavContent } from "@/app/workbench/HerdrNavContent"
import { herdrInitialState, useHerdrStore } from "@/state/herdrStore"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { useTextInputDialogStore } from "@/state/textInputDialogStore"
import { useUiStore } from "@/state/uiStore"
import { open } from "@tauri-apps/plugin-dialog"
import { pickWorkspace } from "@/lib/workspaceActions"

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn()
}))

vi.mock("@/lib/workspaceActions", () => ({
  pickWorkspace: vi.fn()
}))

vi.mock("@/lib/herdrIpc", () => ({
  herdrSessions: vi.fn(),
  herdrCapabilities: vi.fn(),
  herdrSnapshot: vi.fn(),
  herdrTabFocus: vi.fn(),
  herdrTabRename: vi.fn(),
  herdrTerminalCreate: vi.fn(),
  herdrTerminalRelease: vi.fn(),
  herdrWorkspaceFocus: vi.fn(),
  herdrWorkspaceCreate: vi.fn(),
  herdrAgentGet: vi.fn(async () => ({
    terminalId: "term-1",
    agentStatus: "working",
    workspaceId: "ws-1",
    tabId: "tab-1",
    paneId: "pane-1",
    focused: true,
    revision: 1,
    title: "Implementer",
    stateLabels: {}
  })),
  herdrAgentRead: vi.fn(async () => ({
    paneId: "pane-1",
    workspaceId: "ws-1",
    tabId: "tab-1",
    source: "recent",
    format: "text",
    text: "ready",
    revision: 1,
    truncated: false
  }))
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
    useUiStore.setState({ mode: "ade" })
    vi.mocked(open).mockReset()
    vi.mocked(pickWorkspace).mockReset()
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

  it("projects Herdr-reported WSL origin as a compact Agent badge", () => {
    const state = readyState()
    useHerdrStore.setState(
      readyState({
        snapshot: {
          ...state.snapshot,
          agents: [
            {
              ...state.snapshot.agents[0],
              executionOrigin: { kind: "wsl", distribution: "Ubuntu" }
            }
          ]
        }
      })
    )

    render(<HerdrNavContent />)

    expect(screen.getByTestId("herdr-agent-origin-ag-1")).toHaveTextContent("WSL · Ubuntu")
  })

  it("exposes a discoverable per-agent Inspector action without replacing row focus", async () => {
    useHerdrStore.setState(readyState())

    render(<HerdrNavContent />)

    const inspect = screen.getByTestId("herdr-inspect-ag-1")
    expect(inspect).toHaveAccessibleName("Inspect Implementer")
    expect(inspect).not.toHaveAttribute("tabindex", "-1")
    fireEvent.click(inspect)
    const dialog = await screen.findByRole("dialog", { name: "Agent Inspector" })
    expect(dialog).toBeInTheDocument()

    fireEvent.keyDown(document, { key: "Escape" })
    await waitFor(() => expect(inspect).toHaveFocus())
  })

  it("disables attention rows when the selected runtime cannot mutate", () => {
    const state = readyState()
    useHerdrStore.setState(
      readyState({
        capabilities: {
          ...state.capabilities,
          api: {
            ...state.capabilities.api,
            workspaceFocus: false,
            reason: "Herdr workspace.focus unavailable"
          }
        },
        attentionByKey: new Map([
          [
            "native::default::pane-1",
            {
              key: "native::default::pane-1",
              sessionName: "default",
              paneId: "pane-1",
              workspaceId: "ws-1",
              agentStatus: "blocked",
              kind: "blocked",
              title: "Needs input",
              seen: false,
              updatedAt: Date.now()
            }
          ]
        ])
      })
    )

    render(<HerdrNavContent />)

    const attention = screen.getByTestId("herdr-attention-pane-1")
    expect(attention).toHaveAttribute("data-slot", "button")
    expect(attention).toBeDisabled()
    expect(attention).toHaveAttribute("title", "Herdr workspace.focus unavailable")
  })

  it("onboards a connected zero-Space session with one scoped Space create action", async () => {
    const createSpaceFromFolder = vi.fn().mockResolvedValue({ ok: true })
    useHerdrStore.setState(
      readyState({
        selectedSpaceId: null,
        selectedSpaceBySession: { default: null },
        snapshot: {
          ...readyState().snapshot,
          spaces: [],
          tabs: [],
          agents: [],
          terminals: []
        },
        canCreateSpace: () => true,
        createSpaceFromFolder
      })
    )
    vi.mocked(open).mockResolvedValue("/Users/tester/first-space")

    render(<HerdrNavContent />)

    expect(screen.getByTestId("herdr-zero-space-onboarding")).toBeInTheDocument()
    fireEvent.click(screen.getByTestId("herdr-create-space-from-folder"))
    fireEvent.click(screen.getByTestId("herdr-create-space-from-folder"))

    await vi.waitFor(() => {
      expect(createSpaceFromFolder).toHaveBeenCalledTimes(1)
    })
    expect(createSpaceFromFolder).toHaveBeenCalledWith("/Users/tester/first-space", "first-space")
    expect(vi.mocked(open)).toHaveBeenCalledTimes(1)
  })

  it("opens a local folder from zero-Space onboarding and visibly switches to Files", async () => {
    useHerdrStore.setState(
      readyState({
        selectedSpaceId: null,
        selectedSpaceBySession: { default: null },
        snapshot: {
          ...readyState().snapshot,
          spaces: [],
          tabs: [],
          agents: [],
          terminals: []
        }
      })
    )
    vi.mocked(pickWorkspace).mockResolvedValue(true)

    render(<HerdrNavContent />)
    fireEvent.click(screen.getByTestId("herdr-open-local-folder"))

    await vi.waitFor(() => expect(vi.mocked(pickWorkspace)).toHaveBeenCalledTimes(1))
    expect(useUiStore.getState().mode).toBe("files")
  })

  it("keeps the local-folder escape available while Herdr is connecting", async () => {
    useHerdrStore.setState({
      ...herdrInitialState,
      attachments: new Map(),
      connectionState: "connecting"
    })
    vi.mocked(pickWorkspace).mockResolvedValue(true)

    render(<HerdrNavContent />)
    fireEvent.click(screen.getByTestId("herdr-connecting-open-local-folder"))

    await vi.waitFor(() => expect(vi.mocked(pickWorkspace)).toHaveBeenCalledTimes(1))
    expect(useUiStore.getState().mode).toBe("files")
  })

  it("keeps the local-folder escape available when Herdr itself is unavailable", async () => {
    useHerdrStore.setState({
      ...herdrInitialState,
      attachments: new Map(),
      connectionState: "unsupported",
      errorMessage: "Herdr is unavailable"
    })
    vi.mocked(pickWorkspace).mockResolvedValue(true)

    render(<HerdrNavContent />)
    fireEvent.click(screen.getByTestId("herdr-unavailable-open-local-folder"))

    await vi.waitFor(() => expect(vi.mocked(pickWorkspace)).toHaveBeenCalledTimes(1))
    expect(useUiStore.getState().mode).toBe("files")
  })

  it("keeps the local-folder escape available for a stopped session without a snapshot", async () => {
    useHerdrStore.setState({
      ...herdrInitialState,
      attachments: new Map(),
      connectionState: "stopped",
      errorMessage: "Herdr session is stopped"
    })
    vi.mocked(pickWorkspace).mockResolvedValue(true)

    render(<HerdrNavContent />)
    fireEvent.click(screen.getByTestId("herdr-unavailable-open-local-folder"))

    await vi.waitFor(() => expect(vi.mocked(pickWorkspace)).toHaveBeenCalledTimes(1))
    expect(useUiStore.getState().mode).toBe("files")
  })

  it.each(["stopped", "error"] as const)(
    "keeps Session tabs usable when the selected runtime is %s without a snapshot",
    (connectionState) => {
      const selectSession = vi.fn().mockResolvedValue(undefined)
      useHerdrStore.setState(
        readyState({
          selectedSessionName: "work",
          connectionState,
          selectedSpaceId: null,
          snapshot: null,
          errorMessage: `Herdr session is ${connectionState}`,
          selectSession
        })
      )

      render(<HerdrNavContent />)

      expect(screen.getByTestId("herdr-session-default")).toBeInTheDocument()
      expect(screen.getByTestId("herdr-session-work")).toHaveAttribute("aria-selected", "true")
      fireEvent.click(screen.getByTestId("herdr-session-default"))
      expect(selectSession).toHaveBeenCalledWith("default")
      expect(screen.getByTestId("herdr-unavailable-open-local-folder")).toBeEnabled()
    }
  )

  it("renders the first-Space blocked reason instead of hiding it in a tooltip", () => {
    useHerdrStore.setState(
      readyState({
        selectedSpaceId: null,
        selectedSpaceBySession: { default: null },
        snapshot: {
          ...readyState().snapshot,
          spaces: [],
          tabs: [],
          agents: [],
          terminals: []
        },
        canCreateSpace: () => false,
        createSpaceBlockedReason: () => "Herdr workspace.create unavailable"
      })
    )

    render(<HerdrNavContent />)

    expect(screen.getByTestId("herdr-create-space-from-folder")).toBeDisabled()
    expect(screen.getByTestId("herdr-create-space-blocked-reason")).toHaveTextContent(
      "Herdr workspace.create unavailable"
    )
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

  it("shows the local-folder escape for a stopped session without a snapshot", () => {
    useHerdrStore.setState(
      readyState({
        selectedSessionName: "work",
        connectionState: "stopped",
        selectedSpaceId: null,
        snapshot: null,
        errorMessage: "stopped"
      })
    )
    render(<HerdrNavContent />)
    expect(screen.getByTestId("herdr-unavailable-open-local-folder")).toBeEnabled()
    expect(screen.queryByTestId("herdr-create-terminal")).toBeNull()
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
