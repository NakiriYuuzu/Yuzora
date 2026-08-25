import { beforeEach, describe, expect, it, vi } from "vitest"

vi.mock("@/lib/herdrIpc", () => ({
  herdrAgentCreate: vi.fn(),
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

vi.mock("@/lib/workspaceActions", () => ({
  openWorkspaceAtPath: vi.fn()
}))

vi.mock("@/lib/unsavedGuard", () => ({
  confirmDiscardingUnsaved: vi.fn()
}))

import {
  herdrAgentCreate,
  herdrCapabilities,
  herdrSessions,
  herdrSnapshot,
  herdrTabFocus,
  herdrTabRename,
  herdrTerminalCreate,
  herdrTerminalRelease,
  herdrWorkspaceCreate,
  herdrWorkspaceFocus
} from "@/lib/herdrIpc"
import { confirmDiscardingUnsaved } from "@/lib/unsavedGuard"
import { openWorkspaceAtPath } from "@/lib/workspaceActions"
import { herdrInitialState, useHerdrStore } from "./herdrStore"
import { useWorkspaceStore } from "./workspaceStore"
import { useUiStore } from "./uiStore"

const caps = {
  binaryPath: "/bin/herdr",
  binaryVersion: "0.8.0",
  binaryProtocol: 19,
  binarySource: { configured: "global" as const, resolved: "global" as const, available: true, path: "/bin/herdr", reason: null, restartRequired: false },
  channel: "stable",
  server: {
    running: true,
    version: "0.8.0",
    protocol: 19,
    compatible: true,
    socketPath: "/tmp/herdr.sock"
  },
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
        agentManifests: true,
        agentStart: true,
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
          "layout.set_split_ratio",
          "server.agent_manifests",
          "agent.start"
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
  events: {
    status: "deferred" as const,
    reason: "events.subscribe deferred"
  }
}

const sessions = [
  {
    name: "default",
    default: true,
    running: true,
    sessionDir: "/tmp/default",
    socketPath: "/tmp/default.sock"
  },
  {
    name: "work",
    default: false,
    running: true,
    sessionDir: "/tmp/work",
    socketPath: "/tmp/work.sock"
  },
  {
    name: "stopped",
    default: false,
    running: false,
    sessionDir: "/tmp/stopped",
    socketPath: "/tmp/stopped.sock"
  }
]

const rawSnapshot = {
  protocol: 19,
  version: "0.8.0",
  snapshot: {
    version: "0.8.0",
    protocol: 19,
    focused_workspace_id: "ws-1",
    focused_tab_id: "tab-1",
    focused_pane_id: "pane-1",
    workspaces: [
      {
        workspace_id: "ws-1",
        number: 0,
        label: "Yuzora",
        focused: true,
        pane_count: 1,
        tab_count: 1,
        active_tab_id: "tab-1",
        agent_status: "working",
        worktree: {
          checkout_path: "/Users/me/yuzora",
          is_linked_worktree: false,
          repo_key: "k",
          repo_name: "yuzora",
          repo_root: "/Users/me/yuzora"
        }
      },
      {
        workspace_id: "ws-2",
        number: 1,
        label: "feature-x",
        focused: false,
        pane_count: 0,
        tab_count: 0,
        active_tab_id: "tab-2",
        agent_status: "idle",
        worktree: {
          checkout_path: "/Users/me/feature-x",
          is_linked_worktree: true,
          repo_key: "k",
          repo_name: "yuzora",
          repo_root: "/Users/me/yuzora"
        }
      }
    ],
    tabs: [
      {
        tab_id: "tab-1",
        workspace_id: "ws-1",
        number: 1,
        label: "Main",
        focused: true,
        pane_count: 1,
        agent_status: "working"
      },
      {
        tab_id: "tab-2",
        workspace_id: "ws-2",
        number: 1,
        label: "Feature",
        focused: false,
        pane_count: 1,
        agent_status: "idle"
      }
    ],
    panes: [
      {
        pane_id: "pane-1",
        terminal_id: "term-1",
        workspace_id: "ws-1",
        tab_id: "tab-1",
        focused: true,
        agent_status: "working",
        revision: 1
      }
    ],
    layouts: [],
    agents: [
      {
        terminal_id: "term-1",
        agent_status: "working",
        workspace_id: "ws-1",
        tab_id: "tab-1",
        pane_id: "pane-1",
        focused: true,
        revision: 1,
        name: "pi"
      },
      {
        terminal_id: "term-2",
        agent_status: "idle",
        workspace_id: "ws-2",
        tab_id: "tab-2",
        pane_id: "pane-2",
        focused: false,
        revision: 1,
        name: "claude"
      }
    ]
  }
}

function snapshotWithWorkspace(workspaceId: string, label: string, path: string) {
  return {
    ...rawSnapshot,
    snapshot: {
      ...rawSnapshot.snapshot,
      focused_workspace_id: workspaceId,
      workspaces: [
        ...rawSnapshot.snapshot.workspaces.map((workspace) => ({ ...workspace, focused: false })),
        {
          ...rawSnapshot.snapshot.workspaces[0],
          workspace_id: workspaceId,
          number: rawSnapshot.snapshot.workspaces.length,
          label,
          focused: true,
          pane_count: 0,
          tab_count: 0,
          active_tab_id: null,
          worktree: {
            ...rawSnapshot.snapshot.workspaces[0].worktree,
            checkout_path: path
          }
        }
      ]
    }
  }
}

describe("herdrStore", () => {
  beforeEach(() => {
    useHerdrStore.setState({ ...herdrInitialState, attachments: new Map() })
    useWorkspaceStore.setState({
      workspacePath: "/Users/me/yuzora",
      groups: [{ tabs: [], activePath: null }],
      activeGroupIndex: 0
    })
    vi.mocked(herdrSessions).mockReset().mockResolvedValue(sessions)
    vi.mocked(herdrCapabilities).mockReset().mockResolvedValue(caps)
    vi.mocked(herdrSnapshot).mockReset().mockResolvedValue(rawSnapshot)
    vi.mocked(herdrTabFocus).mockReset().mockResolvedValue(undefined)
    vi.mocked(herdrTabRename).mockReset().mockResolvedValue(undefined)
    vi.mocked(herdrTerminalCreate).mockReset()
    vi.mocked(herdrTerminalRelease).mockReset()
    vi.mocked(herdrWorkspaceFocus).mockReset().mockResolvedValue(undefined)
    vi.mocked(herdrWorkspaceCreate).mockReset()
    vi.mocked(confirmDiscardingUnsaved).mockReset().mockResolvedValue(true)
    vi.mocked(openWorkspaceAtPath).mockReset().mockResolvedValue(true)
  })

  it("refreshSessions selects default and keeps session maps", async () => {
    await useHerdrStore.getState().refreshSessions()
    const state = useHerdrStore.getState()
    expect(state.sessions).toHaveLength(3)
    expect(state.selectedSessionName).toBe("default")
  })

  it("bounds authoritative snapshot retries after deterministic failures", async () => {
    useHerdrStore.setState({ sessions, selectedSessionName: "default" })
    vi.mocked(herdrSnapshot).mockRejectedValue(new Error("snapshot unavailable"))

    await expect(useHerdrStore.getState().refreshSnapshot("default")).resolves.toBe(false)

    expect(herdrSnapshot).toHaveBeenCalledTimes(3)
    expect(herdrSnapshot).toHaveBeenNthCalledWith(1, "default")
    expect(herdrSnapshot).toHaveBeenNthCalledWith(3, "default")
    expect(useHerdrStore.getState().connectionState).toBe("error")
    expect(useHerdrStore.getState().errorMessage).toBe("snapshot unavailable")
  })

  it("coalesces an in-flight refresh and completes one trailing authoritative pass", async () => {
    useHerdrStore.setState({ sessions, selectedSessionName: "default" })
    let resolveFirst!: (value: typeof rawSnapshot) => void
    const firstSnapshot = new Promise<typeof rawSnapshot>((resolve) => {
      resolveFirst = resolve
    })
    const trailingSnapshot = { ...rawSnapshot, version: "0.8.1" }
    vi.mocked(herdrSnapshot)
      .mockImplementationOnce(() => firstSnapshot)
      .mockResolvedValueOnce(trailingSnapshot)

    const first = useHerdrStore.getState().refreshSnapshot("default")
    const coalesced = useHerdrStore.getState().refreshSnapshot("default")
    expect(herdrSnapshot).toHaveBeenCalledTimes(1)

    resolveFirst(rawSnapshot)
    await expect(Promise.all([first, coalesced])).resolves.toEqual([true, true])

    expect(herdrSnapshot).toHaveBeenCalledTimes(2)
    expect(useHerdrStore.getState().snapshot?.version).toBe("0.8.1")
  })

  it("bootstraps capabilities + normalized snapshot for selected session", async () => {
    await useHerdrStore.getState().refreshSessions()
    await useHerdrStore.getState().bootstrap("default")

    const state = useHerdrStore.getState()
    expect(state.connectionState).toBe("ready")
    expect(state.selectedSpaceId).toBe("ws-1")
    expect(state.agents()).toHaveLength(2)
    expect(state.spaces()).toHaveLength(2)
    expect(state.tabs()).toHaveLength(2)
    expect(state.tabsInSpace("ws-1")[0]).toMatchObject({
      id: "tab-1",
      label: "Main",
      terminalId: "term-1",
      focused: true
    })
    expect(state.snapshot?.herdrSessionId).toBe("default")
    expect(state.runtimesBySession.default?.snapshot?.herdrSessionId).toBe("default")
    expect(herdrCapabilities).toHaveBeenCalledWith("default")
    expect(herdrSnapshot).toHaveBeenCalledWith("default")
  })

  it("mirrors an explicit Herdr focused workspace over stale local Space selection", async () => {
    await useHerdrStore.getState().refreshSessions()
    await useHerdrStore.getState().bootstrap("default")
    useHerdrStore.getState().setSelectedSpaceId("ws-2")

    const snapshot = useHerdrStore.getState().snapshot!
    useHerdrStore.getState().applySnapshot("default", snapshot)

    expect(useHerdrStore.getState().selectedSpaceId).toBe("ws-1")
  })

  it("marks stopped sessions without launching anything", async () => {
    await useHerdrStore.getState().refreshSessions()
    await useHerdrStore.getState().selectSession("stopped")
    const state = useHerdrStore.getState()
    expect(state.selectedSessionName).toBe("stopped")
    expect(state.connectionState).toBe("stopped")
    expect(state.canMutateSelectedSession()).toBe(false)
    expect(state.canCreateTerminal()).toBe(false)
    expect(herdrSnapshot).not.toHaveBeenCalled()
    expect(herdrWorkspaceFocus).not.toHaveBeenCalled()
  })

  it("releaseAttachment drops map entry and calls herdr_terminal_release only", async () => {
    vi.mocked(herdrTerminalRelease).mockResolvedValue(undefined)
    useHerdrStore.getState().registerAttachment("yuzora://herdr/default/term-1::term-1", {
      sessionId: "herdr-term-1",
      pagePath: "yuzora://herdr/default/term-1",
      paneKey: "term-1",
      herdrSessionId: "default",
      terminalId: "term-1",
      target: "term-1",
      mode: "control",
      role: "controller",
      takeover: true
    })

    await useHerdrStore.getState().releaseAttachment("yuzora://herdr/default/term-1::term-1")

    expect(useHerdrStore.getState().attachments.size).toBe(0)
    expect(herdrTerminalRelease).toHaveBeenCalledWith("herdr-term-1")
  })

  it("preserves a non-takeover controller attachment when its mode is refreshed", () => {
    const attachmentKey = "yuzora://herdr/default/term-1::term-1"
    useHerdrStore.getState().registerAttachment(attachmentKey, {
      sessionId: "herdr-term-1",
      pagePath: "yuzora://herdr/default/term-1",
      paneKey: "term-1",
      herdrSessionId: "default",
      terminalId: "term-1",
      target: "term-1",
      mode: "control",
      role: "controller",
      takeover: false
    })

    useHerdrStore.getState().updateAttachmentMode(attachmentKey, "control", "controller")

    expect(useHerdrStore.getState().attachments.get(attachmentKey)?.takeover).toBe(false)
  })

  it("createTerminalInSelectedSpace passes sessionName and returns namespaced identity", async () => {
    await useHerdrStore.getState().refreshSessions()
    await useHerdrStore.getState().bootstrap("default")
    vi.mocked(herdrTerminalCreate).mockResolvedValue({
      terminalId: "term-new",
      paneId: "pane-new",
      tabId: "tab-new",
      workspaceId: "ws-1",
      title: "Shell"
    })

    const created = await useHerdrStore.getState().createTerminalInSelectedSpace()

    expect(created).toEqual({
      herdrSessionId: "default",
      workspaceId: "ws-1",
      terminalId: "term-new",
      paneId: "pane-new",
      tabId: "tab-new",
      title: "Shell"
    })
    expect(herdrTerminalCreate).toHaveBeenCalledWith({
      sessionName: "default",
      workspaceId: "ws-1",
      title: "yuzora"
    })
  })

  it("createAgentInSelectedSpace passes only the allowlisted bypass opt-in and refreshes", async () => {
    await useHerdrStore.getState().refreshSessions()
    await useHerdrStore.getState().bootstrap("default")
    vi.mocked(herdrAgentCreate).mockResolvedValue({
      name: "codex",
      kind: "codex",
      terminalId: "term-agent",
      paneId: "pane-agent",
      tabId: "tab-agent",
      workspaceId: "ws-1",
      title: "codex"
    })

    const created = await useHerdrStore
      .getState()
      .createAgentInSelectedSpace("codex", true)

    expect(created).toEqual({
      herdrSessionId: "default",
      workspaceId: "ws-1",
      terminalId: "term-agent",
      paneId: "pane-agent",
      tabId: "tab-agent",
      title: "codex",
      name: "codex",
      kind: "codex"
    })
    expect(herdrAgentCreate).toHaveBeenCalledWith({
      sessionName: "default",
      workspaceId: "ws-1",
      kind: "codex",
      bypassPermissions: true
    })
    expect(useHerdrStore.getState().canCreateAgent()).toBe(true)
  })

  it("keeps the folder basename when Herdr returns no created title", async () => {
    await useHerdrStore.getState().refreshSessions()
    await useHerdrStore.getState().bootstrap("default")
    vi.mocked(herdrTerminalCreate).mockResolvedValue({
      terminalId: "term-new",
      paneId: "pane-new",
      tabId: "tab-new",
      workspaceId: "ws-1",
      title: null
    })

    await expect(useHerdrStore.getState().createTerminalInSelectedSpace()).resolves.toMatchObject({
      title: "yuzora"
    })
  })

  it("uses the shared basename contract for a root Space", async () => {
    await useHerdrStore.getState().refreshSessions()
    await useHerdrStore.getState().bootstrap("default")
    useHerdrStore.setState((state) => {
      const snapshot = state.runtimesBySession.default.snapshot
      const nextSnapshot = snapshot
        ? {
            ...snapshot,
            spaces: snapshot.spaces.map((space) =>
              space.id === "ws-1" ? { ...space, path: "/", label: "Root Space" } : space
            )
          }
        : null
      return {
        snapshot: nextSnapshot,
        runtimesBySession: {
          ...state.runtimesBySession,
          default: {
            ...state.runtimesBySession.default,
            snapshot: nextSnapshot
          }
        }
      }
    })
    vi.mocked(herdrTerminalCreate).mockResolvedValue({
      terminalId: "term-root",
      paneId: "pane-root",
      tabId: "tab-root",
      workspaceId: "ws-1",
      title: null
    })

    await expect(useHerdrStore.getState().createTerminalInSelectedSpace()).resolves.toMatchObject({
      title: "/"
    })
    expect(herdrTerminalCreate).toHaveBeenCalledWith(expect.objectContaining({ title: "/" }))
  })

  it("identical terminalId is namespaced per session in page open", async () => {
    await useHerdrStore.getState().refreshSessions()
    await useHerdrStore.getState().bootstrap("default")
    await useHerdrStore.getState().bootstrap("work")
    const agent = useHerdrStore.getState().agents()[0]
    expect(agent.terminalId).toBe("term-1")

    await useHerdrStore.getState().activateAgent({
      ...agent,
      sessionName: "default"
    })
    await useHerdrStore.getState().activateAgent({
      ...agent,
      sessionName: "work",
      workspaceId: "ws-1"
    })

    const tabs = useWorkspaceStore.getState().groups[0].tabs
    const paths = tabs.map((tab) => tab.path)
    expect(paths).toContain("yuzora://herdr/default/term-1")
    expect(paths).toContain("yuzora://herdr/work/term-1")
    expect(new Set(paths).size).toBe(paths.length)
  })

  it("switches Yuzora workspace when protocol-19 exposes Space cwd only on agents", async () => {
    const snapshotWithoutWorkspacePaths = structuredClone(rawSnapshot)
    for (const workspace of snapshotWithoutWorkspacePaths.snapshot.workspaces) {
      Object.assign(workspace, { worktree: undefined })
    }
    Object.assign(snapshotWithoutWorkspacePaths.snapshot.agents[0], {
      cwd: "/Users/me/yuzora",
      foreground_cwd: "/Users/me/yuzora"
    })
    Object.assign(snapshotWithoutWorkspacePaths.snapshot.agents[1], {
      cwd: "/Users/me/YuStock",
      foreground_cwd: "/Users/me/YuStock"
    })
    vi.mocked(herdrSnapshot).mockResolvedValue(snapshotWithoutWorkspacePaths)

    await useHerdrStore.getState().refreshSessions()
    await useHerdrStore.getState().bootstrap("default")
    const yuStock = useHerdrStore
      .getState()
      .agents()
      .find((item) => item.workspaceId === "ws-2")!

    expect(useHerdrStore.getState().spaces().find((space) => space.id === "ws-2")?.path).toBe(
      "/Users/me/YuStock"
    )
    const result = await useHerdrStore.getState().activateAgent(yuStock)

    expect(result).toEqual({ ok: true })
    expect(openWorkspaceAtPath).toHaveBeenCalledWith("/Users/me/YuStock", {
      skipUnsavedGuard: true
    })
  })

  it("Space activation shows its active terminal before the bridge poll", async () => {
    await useHerdrStore.getState().refreshSessions()
    await useHerdrStore.getState().bootstrap("default")
    const targetSpace = useHerdrStore.getState().spaces().find((space) => space.id === "ws-2")!

    const result = await useHerdrStore.getState().activateSpace({
      sessionName: "default",
      workspaceId: targetSpace.id,
      path: targetSpace.path
    })

    expect(result).toEqual({ ok: true })
    expect(useHerdrStore.getState().snapshot?.focusedTabId).toBe("tab-2")
    expect(useWorkspaceStore.getState().groups[0].activePath).toBe(
      "yuzora://herdr/default/term-2"
    )
    expect(herdrSnapshot).toHaveBeenCalledTimes(1)
  })

  it("cross-Space activation focuses Herdr, switches workspace, commits selection", async () => {
    await useHerdrStore.getState().refreshSessions()
    await useHerdrStore.getState().bootstrap("default")
    const agent = useHerdrStore
      .getState()
      .agents()
      .find((item) => item.workspaceId === "ws-2")!
    expect(agent.spaceLabel).toBe("feature-x")

    const result = await useHerdrStore.getState().activateAgent(agent)
    expect(result).toEqual({ ok: true })
    expect(confirmDiscardingUnsaved).toHaveBeenCalled()
    expect(herdrWorkspaceFocus).toHaveBeenCalledWith({
      sessionName: "default",
      workspaceId: "ws-2"
    })
    expect(herdrTabFocus).toHaveBeenCalledWith({
      sessionName: "default",
      tabId: "tab-2"
    })
    expect(openWorkspaceAtPath).toHaveBeenCalledWith("/Users/me/feature-x", {
      skipUnsavedGuard: true
    })
    expect(useHerdrStore.getState().selectedSpaceId).toBe("ws-2")
    expect(useWorkspaceStore.getState().groups[0].tabs[0].terminalId).toBe("term-2")
    expect(useUiStore.getState().mode).toBe("ade")
  })

  it("serializes rapid Tab activations so no stale RPC can apply after the latest", async () => {
    await useHerdrStore.getState().refreshSessions()
    await useHerdrStore.getState().bootstrap("default")
    const [first, rawSecond] = useHerdrStore.getState().tabs()
    const second = { ...rawSecond, workspaceId: first.workspaceId }
    let finishFirstWorkspaceFocus!: () => void
    vi.mocked(herdrWorkspaceFocus)
      .mockImplementationOnce(
        () =>
          new Promise<void>((resolve) => {
            finishFirstWorkspaceFocus = resolve
          })
      )
      .mockResolvedValue(undefined)

    const firstActivation = useHerdrStore.getState().activateTab(first)
    await vi.waitFor(() => {
      expect(herdrWorkspaceFocus).toHaveBeenCalledTimes(1)
    })
    const secondActivation = useHerdrStore.getState().activateTab(second)
    await Promise.resolve()

    // The second mutation is queued rather than racing the first spawn_blocking
    // request. It cannot complete first and then be overwritten by stale focus.
    expect(herdrWorkspaceFocus).toHaveBeenCalledTimes(1)
    finishFirstWorkspaceFocus()
    await expect(firstActivation).resolves.toEqual({ ok: false, cancelled: true })
    await expect(secondActivation).resolves.toEqual({ ok: true })

    expect(herdrWorkspaceFocus).toHaveBeenCalledTimes(2)
    expect(herdrTabFocus).toHaveBeenCalledTimes(1)
    expect(herdrTabFocus).toHaveBeenCalledWith({
      sessionName: "default",
      tabId: "tab-2"
    })
    expect(useWorkspaceStore.getState().groups[0].activePath).toBe(
      "yuzora://herdr/default/term-2"
    )
    expect(useHerdrStore.getState().snapshot).toMatchObject({
      focusedWorkspaceId: "ws-1",
      focusedTabId: "tab-2",
      focusedPaneId: "pane-2"
    })
  })

  it("restores the Yuzora page from Herdr focus without mutating Herdr focus", async () => {
    useWorkspaceStore.setState({
      workspacePath: "/Users/me/feature-x",
      groups: [{ tabs: [], activePath: null }],
      activeGroupIndex: 0
    })
    await useHerdrStore.getState().refreshSessions()
    await useHerdrStore.getState().bootstrap("default")

    const result = await useHerdrStore.getState().restoreFocusedState("default")

    expect(result).toEqual({ ok: true })
    expect(openWorkspaceAtPath).toHaveBeenCalledWith("/Users/me/yuzora", {
      skipUnsavedGuard: true
    })
    expect(herdrWorkspaceFocus).not.toHaveBeenCalled()
    expect(herdrTabFocus).not.toHaveBeenCalled()
    expect(useWorkspaceStore.getState().groups[0].tabs[0]).toMatchObject({
      name: "Main",
      terminalId: "term-1",
      herdrTabId: "tab-1",
      paneId: "pane-1"
    })
    expect(useWorkspaceStore.getState().groups[0].activePath).toBe(
      "yuzora://herdr/default/term-1"
    )
  })

  it("hydrates every usable focused-Space tab on restore and keeps the focused tab active", async () => {
    useWorkspaceStore.setState({
      workspacePath: "/Users/me/yuzora",
      groups: [{
        tabs: [
          { path: "/Users/me/yuzora/src/a.ts", name: "a.ts", dirty: false, externallyModified: false },
          {
            path: "yuzora://herdr/default/hidden",
            name: "Hidden",
            dirty: false,
            externallyModified: false,
            kind: "herdr-terminal",
            herdrSessionId: "default",
            herdrTabId: "tab-hidden",
            herdrWorkspaceId: "ws-2",
            terminalId: "term-hidden"
          }
        ],
        activePath: "/Users/me/yuzora/src/a.ts"
      }],
      activeGroupIndex: 0
    })
    await useHerdrStore.getState().refreshSessions()
    await useHerdrStore.getState().bootstrap("default")
    const runtime = useHerdrStore.getState().runtimesBySession.default!
    const snapshot = runtime.snapshot!
    const focused = snapshot.tabs.find((tab) => tab.id === "tab-1")!
    const otherSpace = snapshot.tabs.find((tab) => tab.id === "tab-2")!
    useHerdrStore.setState({
      runtimesBySession: {
        ...useHerdrStore.getState().runtimesBySession,
        default: {
          ...runtime,
          snapshot: {
            ...snapshot,
            focusedTabId: "tab-1b",
            tabs: [
              { ...focused, id: "tab-1a", label: "One", order: 0, terminalId: "term-1a", focused: false, active: false },
              { ...focused, id: "tab-1b", label: "Two", order: 1, terminalId: "term-1b", focused: true, active: true, paneCount: 3 },
              { ...focused, id: "tab-1c", label: "Three", order: 2, terminalId: "term-1c", focused: false, active: false },
              otherSpace
            ]
          }
        }
      }
    })

    const result = await useHerdrStore.getState().restoreFocusedState("default")

    expect(result).toEqual({ ok: true })
    expect(herdrWorkspaceFocus).not.toHaveBeenCalled()
    expect(herdrTabFocus).not.toHaveBeenCalled()
    const tabs = useWorkspaceStore.getState().groups[0].tabs
    expect(tabs.map((tab) => tab.path)).toEqual([
      "/Users/me/yuzora/src/a.ts",
      "yuzora://herdr/default/hidden",
      "yuzora://herdr/default/term-1a",
      "yuzora://herdr/default/term-1b",
      "yuzora://herdr/default/term-1c"
    ])
    expect(useWorkspaceStore.getState().groups[0].activePath).toBe(
      "yuzora://herdr/default/term-1b"
    )
    expect(tabs.filter((tab) => tab.herdrTabId === "tab-2")).toHaveLength(0)
    expect(tabs.filter((tab) => tab.kind === "herdr-terminal")).toHaveLength(4)
  })

  it("does not hydrate stale Space siblings when focus changes during workspace open", async () => {
    useWorkspaceStore.setState({
      workspacePath: "/Users/me/feature-x",
      groups: [{ tabs: [], activePath: null }],
      activeGroupIndex: 0
    })
    await useHerdrStore.getState().refreshSessions()
    await useHerdrStore.getState().bootstrap("default")
    const runtime = useHerdrStore.getState().runtimesBySession.default!
    const snapshot = runtime.snapshot!
    const focused = snapshot.tabs.find((tab) => tab.id === "tab-1")!
    const otherSpace = snapshot.tabs.find((tab) => tab.id === "tab-2")!
    const staleFocusedSnapshot = {
      ...snapshot,
      focusedWorkspaceId: "ws-1",
      focusedTabId: "tab-1b",
      tabs: [
        { ...focused, id: "tab-1a", label: "One", order: 0, terminalId: "term-1a", focused: false, active: false },
        { ...focused, id: "tab-1b", label: "Two", order: 1, terminalId: "term-1b", focused: true, active: true },
        { ...focused, id: "tab-1c", label: "Three", order: 2, terminalId: "term-1c", focused: false, active: false },
        otherSpace
      ]
    }
    useHerdrStore.setState({
      snapshot: staleFocusedSnapshot,
      runtimesBySession: {
        ...useHerdrStore.getState().runtimesBySession,
        default: { ...runtime, snapshot: staleFocusedSnapshot }
      }
    })

    let finishOpen!: (value: boolean) => void
    vi.mocked(openWorkspaceAtPath).mockImplementationOnce(
      () =>
        new Promise<boolean>((resolve) => {
          finishOpen = resolve
        })
    )
    const restoration = useHerdrStore.getState().restoreFocusedState("default")
    await vi.waitFor(() => {
      expect(openWorkspaceAtPath).toHaveBeenCalledWith("/Users/me/yuzora", {
        skipUnsavedGuard: true
      })
    })

    const latestRuntime = useHerdrStore.getState().runtimesBySession.default!
    const movedFocusSnapshot = {
      ...staleFocusedSnapshot,
      focusedWorkspaceId: "ws-2",
      focusedTabId: "tab-2",
      focusedPaneId: "pane-2",
      tabs: staleFocusedSnapshot.tabs.map((tab) => ({
        ...tab,
        focused: tab.id === "tab-2",
        active: tab.id === "tab-2"
      }))
    }
    useHerdrStore.setState({
      snapshot: movedFocusSnapshot,
      runtimesBySession: {
        ...useHerdrStore.getState().runtimesBySession,
        default: { ...latestRuntime, snapshot: movedFocusSnapshot }
      }
    })
    finishOpen(true)

    await expect(restoration).resolves.toEqual({ ok: false, cancelled: true })
    expect(herdrWorkspaceFocus).not.toHaveBeenCalled()
    expect(herdrTabFocus).not.toHaveBeenCalled()
    expect(useWorkspaceStore.getState().groups[0].tabs).toEqual([])
    expect(
      useWorkspaceStore.getState().groups[0].tabs.some((tab) =>
        ["tab-1a", "tab-1b", "tab-1c"].includes(tab.herdrTabId ?? "")
      )
    ).toBe(false)
  })

  it("cancels stale focus restoration after the user selects another session", async () => {
    useWorkspaceStore.setState({
      workspacePath: "/Users/me/feature-x",
      groups: [{ tabs: [], activePath: null }],
      activeGroupIndex: 0
    })
    await useHerdrStore.getState().refreshSessions()
    await useHerdrStore.getState().bootstrap("default")

    let finishConfirmation!: (value: boolean) => void
    vi.mocked(confirmDiscardingUnsaved).mockImplementationOnce(
      () =>
        new Promise<boolean>((resolve) => {
          finishConfirmation = resolve
        })
    )
    const restoration = useHerdrStore.getState().restoreFocusedState("default")
    await Promise.resolve()
    expect(confirmDiscardingUnsaved).toHaveBeenCalled()

    await useHerdrStore.getState().selectSession("work")
    finishConfirmation(true)

    await expect(restoration).resolves.toEqual({ ok: false, cancelled: true })
    expect(useHerdrStore.getState().selectedSessionName).toBe("work")
    expect(openWorkspaceAtPath).not.toHaveBeenCalled()
  })

  it("unsaved cancel causes zero session/Space/focus/page mutation", async () => {
    await useHerdrStore.getState().refreshSessions()
    await useHerdrStore.getState().bootstrap("default")
    vi.mocked(confirmDiscardingUnsaved).mockResolvedValueOnce(false)
    const before = {
      session: useHerdrStore.getState().selectedSessionName,
      space: useHerdrStore.getState().selectedSpaceId,
      tabs: useWorkspaceStore.getState().groups[0].tabs.length
    }
    const agent = useHerdrStore
      .getState()
      .agents()
      .find((item) => item.workspaceId === "ws-2")!

    const result = await useHerdrStore.getState().activateAgent(agent)
    expect(result).toEqual({ ok: false, cancelled: true })
    expect(herdrWorkspaceFocus).not.toHaveBeenCalled()
    expect(openWorkspaceAtPath).not.toHaveBeenCalled()
    expect(useHerdrStore.getState().selectedSessionName).toBe(before.session)
    expect(useHerdrStore.getState().selectedSpaceId).toBe(before.space)
    expect(useWorkspaceStore.getState().groups[0].tabs).toHaveLength(before.tabs)
  })

  it("local workspace switch failure rolls back Herdr focus", async () => {
    await useHerdrStore.getState().refreshSessions()
    await useHerdrStore.getState().bootstrap("default")
    useHerdrStore.getState().setSelectedSpaceId("ws-1")
    vi.mocked(openWorkspaceAtPath).mockRejectedValueOnce(new Error("switch failed"))
    const agent = useHerdrStore
      .getState()
      .agents()
      .find((item) => item.workspaceId === "ws-2")!

    const result = await useHerdrStore.getState().activateAgent(agent)
    expect(result.ok).toBe(false)
    expect(herdrWorkspaceFocus).toHaveBeenNthCalledWith(1, {
      sessionName: "default",
      workspaceId: "ws-2"
    })
    expect(herdrWorkspaceFocus).toHaveBeenNthCalledWith(2, {
      sessionName: "default",
      workspaceId: "ws-1"
    })
    expect(useHerdrStore.getState().selectedSpaceId).toBe("ws-1")
    expect(useWorkspaceStore.getState().groups[0].tabs).toHaveLength(0)
  })

  it("cross-session tab failure rolls back the mutated target session", async () => {
    await useHerdrStore.getState().refreshSessions()
    await useHerdrStore.getState().bootstrap("default")
    await useHerdrStore.getState().bootstrap("work")
    vi.mocked(herdrWorkspaceFocus).mockClear()
    vi.mocked(herdrTabFocus).mockClear()
    vi.mocked(openWorkspaceAtPath).mockRejectedValueOnce(new Error("switch failed"))

    const target = useHerdrStore
      .getState()
      .runtimesBySession.work!.snapshot!.tabs.find((tab) => tab.id === "tab-2")!
    const result = await useHerdrStore.getState().activateTab(target)

    expect(result.ok).toBe(false)
    expect(herdrWorkspaceFocus).toHaveBeenNthCalledWith(1, {
      sessionName: "work",
      workspaceId: "ws-2"
    })
    expect(herdrWorkspaceFocus).toHaveBeenNthCalledWith(2, {
      sessionName: "work",
      workspaceId: "ws-1"
    })
    expect(herdrTabFocus).toHaveBeenNthCalledWith(1, {
      sessionName: "work",
      tabId: "tab-2"
    })
    expect(herdrTabFocus).toHaveBeenNthCalledWith(2, {
      sessionName: "work",
      tabId: "tab-1"
    })
    expect(useHerdrStore.getState().selectedSessionName).toBe("default")
  })

  it("honestly gates Space and Tab focus capabilities", async () => {
    await useHerdrStore.getState().refreshSessions()
    await useHerdrStore.getState().bootstrap("default")
    const runtime = useHerdrStore.getState().runtimesBySession.default!
    const withoutTabFocus = {
      ...runtime.capabilities!,
      api: { ...runtime.capabilities!.api, tabFocus: false }
    }
    useHerdrStore.setState({
      capabilities: withoutTabFocus,
      runtimesBySession: {
        ...useHerdrStore.getState().runtimesBySession,
        default: { ...runtime, capabilities: withoutTabFocus }
      }
    })

    expect(useHerdrStore.getState().canMutateSelectedSession()).toBe(true)
    expect(useHerdrStore.getState().canFocusSelectedTab()).toBe(false)
    const target = useHerdrStore.getState().tabs()[0]!
    expect(await useHerdrStore.getState().activateTab(target)).toEqual({
      ok: false,
      error: "herdr workspace.focus/tab.focus unavailable"
    })
    expect(herdrWorkspaceFocus).not.toHaveBeenCalled()

    const withoutWorkspaceFocus = {
      ...withoutTabFocus,
      api: { ...withoutTabFocus.api, workspaceFocus: false }
    }
    useHerdrStore.setState({ capabilities: withoutWorkspaceFocus })
    expect(useHerdrStore.getState().canMutateSelectedSession()).toBe(false)
  })

  it("allows first-Space creation without workspace.focus", () => {
    const createOnlyCaps = {
      ...caps,
      api: { ...caps.api, workspaceFocus: false, workspaceCreate: true }
    }
    useHerdrStore.setState({
      ...herdrInitialState,
      attachments: new Map(),
      sessions: [sessions[0]!],
      selectedSessionName: "default",
      selectedSpaceId: null,
      selectedSpaceBySession: { default: null },
      capabilities: createOnlyCaps,
      snapshot: {
        herdrSessionId: "default",
        protocol: 19,
        version: "0.8.0",
        spaces: [],
        tabs: [],
        terminals: [],
        agents: [],
        raw: {}
      }
    })

    expect(useHerdrStore.getState().canCreateSpace()).toBe(true)
    expect(useHerdrStore.getState().createSpaceBlockedReason()).toBeNull()
    expect(useHerdrStore.getState().canMutateSelectedSession()).toBe(false)
  })

  it("reports the dedicated workspace.create reason when first-Space creation is unavailable", () => {
    const unavailableCaps = {
      ...caps,
      api: {
        ...caps.api,
        workspaceFocus: false,
        workspaceCreate: false,
        reason: "Herdr workspace.create unavailable"
      }
    }
    useHerdrStore.setState({
      ...herdrInitialState,
      attachments: new Map(),
      sessions: [sessions[0]!],
      selectedSessionName: "default",
      capabilities: unavailableCaps
    })

    expect(useHerdrStore.getState().canCreateSpace()).toBe(false)
    expect(useHerdrStore.getState().createSpaceBlockedReason()).toBe(
      "Herdr workspace.create unavailable"
    )
  })

  it("session switch preserves mixed pages and does not clear attachments map entries for other pages", async () => {
    await useHerdrStore.getState().refreshSessions()
    await useHerdrStore.getState().bootstrap("default")
    useWorkspaceStore.setState({
      groups: [
        {
          tabs: [
            { path: "/Users/me/yuzora/src/a.ts", name: "a.ts", kind: "file", dirty: false, externallyModified: false },
            {
              path: "yuzora://herdr/default/term-1",
              name: "term",
              kind: "herdr-terminal",
              dirty: false,
              externallyModified: false,
              herdrSessionId: "default",
              terminalId: "term-1"
            },
            {
              path: "yuzora://preview/http://localhost",
              name: "preview",
              kind: "preview",
              dirty: false,
              externallyModified: false
            }
          ],
          activePath: "/Users/me/yuzora/src/a.ts"
        }
      ],
      activeGroupIndex: 0
    })
    useHerdrStore.getState().registerAttachment("yuzora://herdr/default/term-1::term-1", {
      sessionId: "c1",
      pagePath: "yuzora://herdr/default/term-1",
      paneKey: "term-1",
      herdrSessionId: "default",
      terminalId: "term-1",
      target: "term-1",
      mode: "control",
      role: "controller",
      takeover: true
    })

    await useHerdrStore.getState().selectSession("work")
    expect(useHerdrStore.getState().selectedSessionName).toBe("work")
    expect(useWorkspaceStore.getState().groups[0].tabs).toHaveLength(3)
    expect(useHerdrStore.getState().attachments.has("yuzora://herdr/default/term-1::term-1")).toBe(
      true
    )
  })

  it("createSpaceFromFolder uses workspace.create for selected session after preflight", async () => {
    await useHerdrStore.getState().refreshSessions()
    await useHerdrStore.getState().bootstrap("default")
    vi.mocked(herdrWorkspaceCreate).mockResolvedValue({
      workspaceId: "ws-new",
      label: "new",
      path: "/tmp/new",
      tabId: "tab-new",
      terminalId: "term-new",
      paneId: "pane-new"
    })
    const refreshed = structuredClone(rawSnapshot)
    refreshed.snapshot.workspaces.push({
      workspace_id: "ws-new",
      number: 2,
      label: "new",
      focused: true,
      pane_count: 1,
      tab_count: 1,
      active_tab_id: "tab-new",
      agent_status: "idle",
      worktree: {
        checkout_path: "/tmp/new",
        is_linked_worktree: false,
        repo_key: "k",
        repo_name: "yuzora",
        repo_root: "/tmp/new"
      }
    })
    refreshed.snapshot.tabs.push({
      tab_id: "tab-new",
      workspace_id: "ws-new",
      number: 3,
      label: "new",
      focused: true,
      pane_count: 1,
      agent_status: "idle"
    })
    vi.mocked(herdrSnapshot).mockResolvedValueOnce(refreshed)

    const result = await useHerdrStore.getState().createSpaceFromFolder("/tmp/new", "new")
    expect(confirmDiscardingUnsaved).toHaveBeenCalled()
    expect(herdrWorkspaceCreate).toHaveBeenCalledWith({
      sessionName: "default",
      cwd: "/tmp/new",
      label: "new",
      focus: true
    })
    expect(openWorkspaceAtPath).toHaveBeenCalledWith("/tmp/new", {
      skipUnsavedGuard: true
    })
    expect(herdrTabRename).toHaveBeenCalledWith({
      sessionName: "default",
      tabId: "tab-new",
      label: "new"
    })
    expect(result.ok).toBe(true)
    if (result.ok) expect(result.space?.id).toBe("ws-new")
    expect(useHerdrStore.getState().selectedSpaceId).toBe("ws-new")
  })

  it("serializes first-Space creation across callers for one native session", async () => {
    await useHerdrStore.getState().refreshSessions()
    await useHerdrStore.getState().bootstrap("default")
    let finishCreate!: (value: {
      workspaceId: string
      label: string
      path: string
      tabId: null
      terminalId: null
      paneId: null
    }) => void
    vi.mocked(herdrWorkspaceCreate).mockImplementation(
      () => new Promise((resolve) => { finishCreate = resolve })
    )
    vi.mocked(herdrSnapshot).mockResolvedValue(snapshotWithWorkspace("ws-new", "new", "/tmp/new"))

    const first = useHerdrStore.getState().createSpaceFromFolder("/tmp/new", "new")
    await vi.waitFor(() => expect(herdrWorkspaceCreate).toHaveBeenCalledTimes(1))
    const second = await useHerdrStore.getState().createSpaceFromFolder("/tmp/other", "other")
    finishCreate({
      workspaceId: "ws-new",
      label: "new",
      path: "/tmp/new",
      tabId: null,
      terminalId: null,
      paneId: null
    })

    await expect(first).resolves.toMatchObject({ ok: true })
    expect(second).toEqual({
      ok: false,
      error: "Space creation is already in progress for this Herdr session."
    })
    expect(herdrWorkspaceCreate).toHaveBeenCalledTimes(1)
  })

  it("does not report success when an applied workspace.create cannot refresh", async () => {
    await useHerdrStore.getState().refreshSessions()
    await useHerdrStore.getState().bootstrap("default")
    vi.mocked(herdrWorkspaceCreate).mockResolvedValue({
      workspaceId: "ws-new", label: "new", path: "/tmp/new", tabId: null, terminalId: null, paneId: null
    })
    vi.mocked(herdrSnapshot).mockRejectedValue(new Error("snapshot offline"))

    const result = await useHerdrStore.getState().createSpaceFromFolder("/tmp/new", "new")

    expect(result.ok).toBe(false)
    if (!result.ok) expect(result.error).toContain("may have created the Space")
    expect(useHerdrStore.getState().errorMessage).toContain("do not create it again")
  })

  it("does not report success when the refreshed snapshot omits the created Space", async () => {
    await useHerdrStore.getState().refreshSessions()
    await useHerdrStore.getState().bootstrap("default")
    vi.mocked(herdrWorkspaceCreate).mockResolvedValue({
      workspaceId: "ws-new", label: "new", path: "/tmp/new", tabId: null, terminalId: null, paneId: null
    })
    vi.mocked(herdrSnapshot).mockResolvedValue(rawSnapshot)

    const result = await useHerdrStore.getState().createSpaceFromFolder("/tmp/new", "new")

    expect(result.ok).toBe(false)
    if (!result.ok) expect(result.error).toContain("not present in the refreshed snapshot")
  })

  it("createSpaceFromFolder cancel causes zero create/focus/selection/page mutation", async () => {
    await useHerdrStore.getState().refreshSessions()
    await useHerdrStore.getState().bootstrap("default")
    useHerdrStore.getState().setSelectedSpaceId("ws-1")
    const before = {
      session: useHerdrStore.getState().selectedSessionName,
      space: useHerdrStore.getState().selectedSpaceId,
      tabs: useWorkspaceStore.getState().groups[0].tabs.length,
      workspace: useWorkspaceStore.getState().workspacePath
    }
    vi.mocked(confirmDiscardingUnsaved).mockResolvedValueOnce(false)

    const result = await useHerdrStore.getState().createSpaceFromFolder(
      "/Users/me/feature-x",
      "feature-x"
    )

    expect(result).toEqual({ ok: false, cancelled: true })
    expect(herdrWorkspaceCreate).not.toHaveBeenCalled()
    expect(herdrWorkspaceFocus).not.toHaveBeenCalled()
    expect(openWorkspaceAtPath).not.toHaveBeenCalled()
    expect(useHerdrStore.getState().selectedSessionName).toBe(before.session)
    expect(useHerdrStore.getState().selectedSpaceId).toBe(before.space)
    expect(useWorkspaceStore.getState().groups[0].tabs).toHaveLength(before.tabs)
    expect(useWorkspaceStore.getState().workspacePath).toBe(before.workspace)
  })
})
