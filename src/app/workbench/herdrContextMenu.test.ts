import { beforeEach, describe, expect, it, vi } from "vitest"

vi.mock("@tauri-apps/plugin-dialog", () => ({
  confirm: vi.fn(),
  message: vi.fn()
}))

vi.mock("@/lib/herdrIpc", () => ({
  herdrWorkspaceRename: vi.fn(),
  herdrWorkspaceClose: vi.fn(),
  herdrTabCreate: vi.fn(),
  herdrTabRename: vi.fn(),
  herdrTabClose: vi.fn(),
  herdrPaneRename: vi.fn(),
  herdrPaneSplit: vi.fn(),
  herdrPaneZoom: vi.fn(),
  herdrPaneSwap: vi.fn(),
  herdrPaneClose: vi.fn()
}))

import {
  commandFor,
  resolveContextMenuEntries
} from "@/app/workbench/contextMenuDefs"
import {
  herdrPaneClose,
  herdrPaneRename,
  herdrPaneSplit,
  herdrTabClose,
  herdrTabCreate,
  herdrTabRename,
  herdrWorkspaceRename
} from "@/lib/herdrIpc"
import { useAppDialogStore } from "@/state/appDialogStore"
import { herdrInitialState, herdrStoreRuntimeKey, useHerdrStore } from "@/state/herdrStore"
import { useTextInputDialogStore } from "@/state/textInputDialogStore"
import { useWorkspaceStore } from "@/state/workspaceStore"

const fullApi = {
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
    "workspace.rename",
    "workspace.close",
    "tab.create",
    "tab.rename",
    "tab.close",
    "pane.rename",
    "pane.split",
    "pane.zoom",
    "pane.swap",
    "pane.close"
  ],
  reason: null
}

function seedHerdr() {
  useHerdrStore.setState({
    ...herdrInitialState,
    attachments: new Map(),
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
    connectionState: "ready",
    capabilities: {
      binaryPath: "/bin/herdr",
      binarySource: { configured: "global" as const, resolved: "global" as const, available: true, path: "/bin/herdr", reason: null, restartRequired: false },
  server: { running: true },
      api: fullApi,
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
    }
  })
}

beforeEach(() => {
  seedHerdr()
  useAppDialogStore.setState({ pending: null })
  vi.mocked(herdrWorkspaceRename).mockReset()
  vi.mocked(herdrPaneRename).mockReset()
  vi.mocked(herdrPaneSplit).mockReset()
  vi.mocked(herdrPaneClose).mockReset()
  vi.mocked(herdrTabCreate).mockReset()
  vi.mocked(herdrTabRename).mockReset()
  vi.mocked(herdrTabClose).mockReset()
  useTextInputDialogStore.setState({ pending: null })
})

describe("Herdr context menu registry", () => {
  it("exposes Space/Tab/Pane entries and gates unavailable methods", () => {
    const space = resolveContextMenuEntries({
      kind: "herdrSpace",
      sessionName: "default",
      workspaceId: "ws-1",
      label: "Yuzora"
    })
    expect(space.map((e) => (e.type === "command" ? e.command.id : "|"))).toEqual([
      "cmHerdrRenameSpace",
      "cmHerdrNewTab",
      "cmHerdrCloseSpace"
    ])

    useHerdrStore.setState((state) => ({
      capabilities: state.capabilities
        ? {
            ...state.capabilities,
            api: {
              ...state.capabilities.api,
              paneSplit: false,
              methods: state.capabilities.api.methods.filter((m) => m !== "pane.split")
            }
          }
        : null
    }))
    const pane = resolveContextMenuEntries({
      kind: "herdrPane",
      sessionName: "default",
      paneId: "p1",
      label: "Shell",
      focusedPaneId: "p2"
    })
    const split = pane.find(
      (entry) => entry.type === "command" && entry.command.id === "cmHerdrSplitRight"
    )
    expect(split?.type).toBe("command")
    if (split?.type === "command") {
      expect(split.availability.enabled).toBe(false)
    }
  })

  it("uses capabilities from the targeted named session, not the selected sidebar session", () => {
    const capabilities = useHerdrStore.getState().capabilities
    if (!capabilities) throw new Error("expected seeded Herdr capabilities")
    useHerdrStore.setState((state) => ({
      sessions: [
        ...state.sessions,
        {
          name: "work",
          default: false,
          running: true,
          sessionDir: "/tmp/w",
          socketPath: "/tmp/w.sock"
        }
      ],
      runtimesBySession: {
        work: {
          capabilities: {
            ...capabilities,
            api: {
              ...capabilities.api,
              paneSplit: false,
              methods: capabilities.api.methods.filter((method) => method !== "pane.split")
            }
          },
          snapshot: null, worktreeInventory: null, connectionState: "ready",
          errorMessage: null
        }
      }
    }))

    const pane = resolveContextMenuEntries({
      kind: "herdrPane",
      sessionName: "work",
      paneId: "p-work",
      focusedPaneId: "p-other"
    })
    const split = pane.find(
      (entry) => entry.type === "command" && entry.command.id === "cmHerdrSplitRight"
    )
    expect(split?.type).toBe("command")
    if (split?.type === "command") {
      expect(split.availability.enabled).toBe(false)
    }
  })

  it("uses the targeted Runtime Environment for same-name session capability gating", () => {
    const capabilities = useHerdrStore.getState().capabilities
    if (!capabilities) throw new Error("expected seeded Herdr capabilities")
    const ubuntu = { kind: "wsl" as const, distro: "Ubuntu" }
    useHerdrStore.setState((state) => ({
      sessions: [
        ...state.sessions,
        {
          name: "default",
          default: true,
          running: true,
          sessionDir: "/tmp/ubuntu-default",
          socketPath: "/tmp/ubuntu-default.sock",
          runtimeTarget: ubuntu
        }
      ],
      runtimesBySession: {
        ...state.runtimesBySession,
        [herdrStoreRuntimeKey("default", ubuntu)]: {
          runtimeTarget: ubuntu,
          capabilities,
          snapshot: null,
          worktreeInventory: null,
          connectionState: "ready",
          errorMessage: null
        }
      },
      capabilities: {
        ...capabilities,
        api: { ...capabilities.api, paneSplit: false }
      }
    }))

    const pane = resolveContextMenuEntries({
      kind: "herdrPane",
      runtimeTarget: ubuntu,
      sessionName: "default",
      paneId: "p-ubuntu",
      focusedPaneId: "p-other"
    })
    const split = pane.find(
      (entry) => entry.type === "command" && entry.command.id === "cmHerdrSplitRight"
    )
    expect(split?.type).toBe("command")
    if (split?.type === "command") expect(split.availability.enabled).toBe(true)
  })

  it("fails closed for WSL when only same-name Native capabilities exist", () => {
    const ubuntu = { kind: "wsl" as const, distro: "Ubuntu" }
    useHerdrStore.setState((state) => ({
      sessions: [
        ...state.sessions,
        {
          name: "default",
          default: true,
          running: true,
          sessionDir: "/tmp/ubuntu-default",
          socketPath: "/tmp/ubuntu-default.sock",
          runtimeTarget: ubuntu
        }
      ]
    }))

    const pane = resolveContextMenuEntries({
      kind: "herdrPane",
      runtimeTarget: ubuntu,
      sessionName: "default",
      paneId: "p-ubuntu",
      focusedPaneId: "p-other"
    })
    const split = pane.find(
      (entry) => entry.type === "command" && entry.command.id === "cmHerdrSplitRight"
    )
    expect(split?.type).toBe("command")
    if (split?.type === "command") expect(split.availability.enabled).toBe(false)
  })

  it("rename uses the in-app text dialog and cancel performs no IPC", async () => {
    const request = {
      kind: "herdrSpace" as const,
      sessionName: "default",
      workspaceId: "ws-1",
      label: "Yuzora"
    }
    const action = commandFor(request, "cmHerdrRenameSpace")?.executor(request)
    await vi.waitFor(() => expect(useTextInputDialogStore.getState().pending).toMatchObject({
      initialValue: "Yuzora"
    }))
    useTextInputDialogStore.getState().respond(null)
    await expect(action).resolves.toBe("cancelled")
    expect(herdrWorkspaceRename).not.toHaveBeenCalled()
  })

  it("tab rename uses the in-app dialog and synchronizes the targeted Herdr page", async () => {
    const pagePath = "yuzora://herdr/default/term-1"
    useWorkspaceStore.setState({
      workspacePath: "/w",
      activeGroupIndex: 0,
      groups: [{
        activePath: pagePath,
        tabs: [{
          path: pagePath,
          name: "Old",
          dirty: false,
          externallyModified: false,
          kind: "herdr-terminal",
          herdrSessionId: "default",
          terminalId: "term-1",
          herdrTabId: "tab-1",
          paneId: "pane-1"
        }]
      }]
    })
    const request = {
      kind: "herdrTab" as const,
      sessionName: "default",
      tabId: "tab-1",
      label: "Old",
      pagePath
    }

    const action = commandFor(request, "cmHerdrRenameTab")?.executor(request)
    await vi.waitFor(() => {
      expect(useTextInputDialogStore.getState().pending).toMatchObject({
        initialValue: "Old"
      })
    })
    useTextInputDialogStore.getState().respond("Renamed")

    await expect(action).resolves.toBe("completed")
    expect(herdrTabRename).toHaveBeenCalledWith({
      sessionName: "default",
      tabId: "tab-1",
      label: "Renamed"
    })
    expect(useWorkspaceStore.getState().groups[0].tabs[0].name).toBe("Renamed")
  })

  it("tab close removes a stale local page when runtime reports that exact tab already absent", async () => {
    vi.mocked(herdrTabClose).mockRejectedValue(
      new Error("tab_not_found: tab tab-stale not found")
    )
    const pagePath = "yuzora://herdr/default/term-stale"
    useWorkspaceStore.setState({
      workspacePath: "/w",
      activeGroupIndex: 0,
      groups: [{
        activePath: pagePath,
        tabs: [{
          path: pagePath,
          name: "Renamed stale page",
          dirty: false,
          externallyModified: false,
          kind: "herdr-terminal",
          herdrSessionId: "default",
          terminalId: "term-stale",
          herdrTabId: "tab-stale",
          paneId: "pane-stale"
        }]
      }]
    })
    const request = {
      kind: "herdrTab" as const,
      sessionName: "default",
      tabId: "tab-stale",
      label: "Renamed stale page",
      pagePath
    }

    const action = commandFor(request, "cmHerdrCloseTab")?.executor(request)
    await vi.waitFor(() => expect(useAppDialogStore.getState().pending?.type).toBe("confirm"))
    useAppDialogStore.getState().respond(true)
    await expect(action).resolves.toBe("completed")
    expect(herdrTabClose).toHaveBeenCalledWith({
      sessionName: "default",
      tabId: "tab-stale"
    })
    expect(useWorkspaceStore.getState().groups[0].tabs).toHaveLength(0)
  })

  it("tab close preserves the local page when tab_not_found names a different tab", async () => {
    vi.mocked(herdrTabClose).mockRejectedValue(
      new Error("tab_not_found: tab another-tab not found")
    )
    const pagePath = "yuzora://herdr/default/term-close"
    useWorkspaceStore.setState({
      workspacePath: "/w",
      activeGroupIndex: 0,
      groups: [{
        activePath: pagePath,
        tabs: [{
          path: pagePath,
          name: "Keep me",
          dirty: false,
          externallyModified: false,
          kind: "herdr-terminal",
          herdrSessionId: "default",
          terminalId: "term-close",
          herdrTabId: "tab-close",
          paneId: "pane-close"
        }]
      }]
    })
    const request = {
      kind: "herdrTab" as const,
      sessionName: "default",
      tabId: "tab-close",
      label: "Keep me",
      pagePath
    }

    const action = commandFor(request, "cmHerdrCloseTab")?.executor(request)
    await vi.waitFor(() => expect(useAppDialogStore.getState().pending?.type).toBe("confirm"))
    useAppDialogStore.getState().respond(true)
    await expect(action).rejects.toThrow("another-tab")
    expect(useWorkspaceStore.getState().groups[0].tabs).toHaveLength(1)
  })

  it("destructive close cancel performs no API mutation", async () => {
    const request = {
      kind: "herdrPane" as const,
      sessionName: "default",
      paneId: "p1",
      label: "Shell"
    }
    const action = commandFor(request, "cmHerdrClosePane")?.executor(request)
    await vi.waitFor(() => expect(useAppDialogStore.getState().pending?.type).toBe("confirm"))
    useAppDialogStore.getState().respond(false)
    await expect(action).resolves.toBe("cancelled")
    expect(herdrPaneClose).not.toHaveBeenCalled()
  })

  it("Agent/Space handlers carry session/tab/pane IDs into split action", async () => {
    vi.mocked(herdrPaneSplit).mockResolvedValue({
      paneId: "p2",
      terminalId: "t2",
      tabId: "tab-1",
      workspaceId: "ws-1",
      title: null
    })
    const request = {
      kind: "herdrPane" as const,
      sessionName: "work",
      paneId: "pane-a",
      tabId: "tab-1",
      workspaceId: "ws-1",
      terminalId: "term-a",
      label: "Agent"
    }
    // Mark work session running for availability; executor still uses request.sessionName.
    useHerdrStore.setState((state) => ({
      sessions: [
        ...state.sessions,
        {
          name: "work",
          default: false,
          running: true,
          sessionDir: "/tmp/w",
          socketPath: "/tmp/w.sock"
        }
      ]
    }))
    const outcome = await commandFor(request, "cmHerdrSplitRight")?.executor(request)
    expect(outcome).toBe("completed")
    expect(herdrPaneSplit).toHaveBeenCalledWith({
      sessionName: "work",
      direction: "right",
      targetPaneId: "pane-a",
      workspaceId: "ws-1",
      focus: true
    })
  })

  it("fails closed for a WSL menu when only Native same-name capabilities exist", () => {
    const capabilities = useHerdrStore.getState().capabilities
    if (!capabilities) throw new Error("expected seeded Herdr capabilities")
    const ubuntu = { kind: "wsl" as const, distro: "Ubuntu" }
    useHerdrStore.setState((state) => ({
      selectedSessionName: "default",
      selectedRuntimeTarget: { kind: "native" },
      sessions: [
        ...state.sessions,
        {
          name: "default",
          default: true,
          running: true,
          sessionDir: "/tmp/ubuntu-default",
          socketPath: "/tmp/ubuntu-default.sock",
          runtimeTarget: ubuntu
        }
      ],
      capabilities: {
        ...capabilities,
        api: { ...capabilities.api, paneSplit: true, methods: ["pane.split"] }
      }
    }))
    const pane = resolveContextMenuEntries({
      kind: "herdrPane",
      runtimeTarget: ubuntu,
      sessionName: "default",
      paneId: "p-ubuntu",
      focusedPaneId: "p-other"
    })
    const split = pane.find(
      (entry) => entry.type === "command" && entry.command.id === "cmHerdrSplitRight"
    )
    expect(split?.type).toBe("command")
    if (split?.type === "command") expect(split.availability.enabled).toBe(false)
  })

})
