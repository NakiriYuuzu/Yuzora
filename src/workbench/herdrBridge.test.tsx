import { act, cleanup, render, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { herdrInitialState, useHerdrStore } from "@/state/herdrStore"
import { uiInitialState, useUiStore } from "@/state/uiStore"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { HerdrBridge } from "./HerdrBridge"
import { shouldPollHerdrSnapshots } from "./herdrBridgePolicy"
import { remoteFilePath } from "@/lib/runtimeIdentity"

const initialWorkspaceState = useWorkspaceStore.getState()
const initialHerdrState = useHerdrStore.getState()

describe("HerdrBridge attachment reconciliation", () => {
  beforeEach(() => {
    cleanup()
    useWorkspaceStore.setState(initialWorkspaceState, true)
  })

  afterEach(() => {
    cleanup()
    useWorkspaceStore.setState(initialWorkspaceState, true)
    useHerdrStore.setState(initialHerdrState, true)
    useUiStore.setState(uiInitialState)
    vi.restoreAllMocks()
  })

  it("restores the focused Herdr tab after file-session hydration settles", async () => {
    const restoreFocusedState = vi.fn(async () => ({ ok: true as const }))
    const refreshSessions = vi.fn(async () => undefined)
    const refreshSnapshot = vi.fn(async () => true)
    const releaseAllAttachments = vi.fn(async () => undefined)
    const session = {
      name: "default",
      default: true,
      running: true,
      sessionDir: "/tmp/default",
      socketPath: "/tmp/default.sock"
    }
    const snapshot = {
      herdrSessionId: "default",
      protocol: 19,
      version: "0.8.0",
      spaces: [{ id: "ws-1", label: "Main", order: 1, focused: true }],
      agents: [],
      tabs: [
        {
          id: "tab-1",
          label: "Agent",
          order: 1,
          workspaceId: "ws-1",
          paneCount: 1,
          status: "idle" as const,
          active: true,
          focused: true,
          paneId: "pane-1",
          terminalId: "term-1",
          sessionName: "default"
        }
      ],
      terminals: [],
      focusedWorkspaceId: "ws-1",
      focusedTabId: "tab-1",
      focusedPaneId: "pane-1",
      raw: {}
    }

    useWorkspaceStore.setState({ sessionRestoreReady: true })
    // Focus restoration is runtime state recovery, not conditional presentation.
    useUiStore.setState({ mode: "files" })
    useHerdrStore.setState({
      ...herdrInitialState,
      sessions: [session],
      selectedSessionName: "default",
      connectionState: "ready",
      snapshot,
      runtimesBySession: {
        default: {
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
                "tab.focus",
                "agent.get",
                "agent.read",
                "events.subscribe"
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
            events: { status: "unavailable" }
          },
          snapshot,
          worktreeInventory: null,
          connectionState: "ready",
          errorMessage: null
        }
      },
      refreshSessions,
      refreshSnapshot,
      restoreFocusedState,
      releaseAllAttachments,
      selectedSession: () => session
    })

    render(<HerdrBridge />)
    await act(async () => {
      for (let index = 0; index < 8; index++) await Promise.resolve()
    })

    expect(restoreFocusedState).toHaveBeenCalledWith("default")
    expect(restoreFocusedState).toHaveBeenCalledTimes(1)
    expect(refreshSnapshot).toHaveBeenCalledWith("default")
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0))
    })

    // A same-focus snapshot refresh must not reopen a user-closed page.
    act(() => {
      const current = useHerdrStore.getState()
      const refreshed = { ...snapshot, raw: { revision: 2 } }
      useHerdrStore.setState({
        snapshot: refreshed,
        runtimesBySession: {
          ...current.runtimesBySession,
          default: { ...current.runtimesBySession.default!, snapshot: refreshed }
        }
      })
    })
    await act(async () => {
      await Promise.resolve()
    })
    expect(restoreFocusedState).toHaveBeenCalledTimes(1)

    // Until a real events subscriber exists, even `unavailable` must poll.
    expect(
      shouldPollHerdrSnapshots(
        useHerdrStore.getState().runtimesBySession.default!.capabilities,
        false
      )
    ).toBe(true)
  })

  it("retries a cancelled cold-start focus restore instead of accepting an empty Intro result", async () => {
    const restoreFocusedState = vi
      .fn()
      .mockResolvedValueOnce({ ok: false as const, cancelled: true as const })
      .mockResolvedValueOnce({ ok: true as const })
    const session = {
      name: "default",
      default: true,
      running: true,
      sessionDir: "/tmp/default",
      socketPath: "/tmp/default.sock"
    }
    const snapshot = {
      herdrSessionId: "default",
      protocol: 19,
      version: "0.8.0",
      spaces: [{ id: "ws-1", label: "Main", order: 1, focused: true }],
      agents: [],
      tabs: [{
        id: "tab-1",
        label: "Agent",
        order: 1,
        workspaceId: "ws-1",
        paneCount: 1,
        status: "idle" as const,
        active: true,
        focused: true,
        paneId: "pane-1",
        terminalId: "term-1",
        sessionName: "default"
      }],
      terminals: [],
      focusedWorkspaceId: "ws-1",
      focusedTabId: "tab-1",
      focusedPaneId: "pane-1",
      raw: {}
    }
    useWorkspaceStore.setState({ sessionRestoreReady: true })
    useHerdrStore.setState({
      ...herdrInitialState,
      sessions: [session],
      selectedSessionName: "default",
      connectionState: "ready",
      snapshot,
      runtimesBySession: {
        default: {
          capabilities: null,
          snapshot,
          worktreeInventory: null,
          connectionState: "ready",
          errorMessage: null
        }
      },
      refreshSessions: vi.fn(async () => undefined),
      refreshSnapshot: vi.fn(async () => true),
      restoreFocusedState,
      releaseAllAttachments: vi.fn(async () => undefined),
      selectedSession: () => session
    })

    render(<HerdrBridge />)
    await waitFor(() => expect(restoreFocusedState).toHaveBeenCalledTimes(1))

    act(() => {
      const current = useHerdrStore.getState()
      const retrySnapshot = { ...snapshot, raw: { revision: 2 } }
      useHerdrStore.setState({
        snapshot: retrySnapshot,
        runtimesBySession: {
          ...current.runtimesBySession,
          default: { ...current.runtimesBySession.default!, snapshot: retrySnapshot }
        }
      })
    })

    await waitFor(() => expect(restoreFocusedState).toHaveBeenCalledTimes(2))
  })

  it.each([
    ["another native folder", "/tmp/chosen", "/tmp/agent-cwd", false],
    ["missing Space root", "/tmp/chosen", undefined, false],
    ["the same native folder", "/tmp/chosen", "/tmp/chosen", true],
    ["another host with the same path", remoteFilePath("host-a", "/tmp/chosen"), remoteFilePath("host-b", "/tmp/chosen"), false],
    ["the same remote folder", remoteFilePath("host-a", "/tmp/chosen"), remoteFilePath("host-a", "/tmp/chosen"), true]
  ])("preserves hydrated workspace identity when snapshot targets %s", async (_label, workspacePath, root, shouldRestore) => {
    const restoreFocusedState = vi.fn(async () => ({ ok: true as const }))
    const snapshot = {
      herdrSessionId: "default", protocol: 20, version: "0.8.2",
      spaces: [{ id: "w1", label: "Test", order: 1, focused: true, path: root }],
      agents: [], tabs: [], terminals: [],
      focusedWorkspaceId: "w1", focusedTabId: "w1:t1", raw: {}
    }
    useWorkspaceStore.setState({ sessionRestoreReady: false, workspacePath: null })
    useHerdrStore.setState({
      ...herdrInitialState,
      selectedSessionName: "default",
      runtimesBySession: {
        default: { capabilities: null, snapshot, worktreeInventory: null, connectionState: "ready", errorMessage: null }
      },
      refreshSessions: vi.fn(async () => undefined),
      restoreFocusedState,
      releaseAllAttachments: vi.fn(async () => undefined)
    })
    render(<HerdrBridge />)
    await act(async () => {
      useWorkspaceStore.setState({ sessionRestoreReady: true, workspacePath })
    })
    expect(restoreFocusedState).toHaveBeenCalledTimes(shouldRestore ? 1 : 0)
    // Reconnect snapshots must apply the same workspace guard.
    await act(async () => {
      const state = useHerdrStore.getState()
      useHerdrStore.setState({ runtimesBySession: {
        default: { ...state.runtimesBySession.default!, snapshot: { ...snapshot, focusedTabId: "w1:t2" } }
      } })
    })
    expect(restoreFocusedState).toHaveBeenCalledTimes(shouldRestore ? 2 : 0)
    expect(useWorkspaceStore.getState().workspacePath).toBe(workspacePath)
  })

  it("keeps composite leaf attachments while their owning page is open", async () => {
    const pagePath = "yuzora://herdr/default/term-1"
    const attachmentKey = `${pagePath}::pane-1`
    const releaseAttachment = vi.fn(async () => undefined)
    const releaseAllAttachments = vi.fn(async () => undefined)
    const refreshSessions = vi.fn(async () => undefined)

    useWorkspaceStore.setState({
      groups: [
        {
          activePath: pagePath,
          tabs: [
            {
              path: pagePath,
              name: "Shell",
              dirty: false,
              externallyModified: false,
              kind: "herdr-terminal",
              herdrSessionId: "default",
              terminalId: "term-1",
              herdrTabId: "tab-1",
              paneId: "pane-1"
            }
          ]
        }
      ],
      activeGroupIndex: 0
    })
    useHerdrStore.setState({
      ...herdrInitialState,
      attachments: new Map([
        [
          attachmentKey,
          {
            sessionId: "connector-1",
            pagePath,
            paneKey: "pane-1",
            herdrSessionId: "default",
            terminalId: "term-1",
            target: "term-1",
            paneId: "pane-1",
            mode: "control",
            role: "controller",
            takeover: true
          }
        ]
      ]),
      refreshSessions,
      releaseAttachment,
      releaseAllAttachments,
      selectedSession: () => null
    })

    render(<HerdrBridge />)
    await act(async () => {
      await Promise.resolve()
    })
    expect(releaseAttachment).not.toHaveBeenCalled()

    act(() => {
      useWorkspaceStore.setState({
        groups: [{ activePath: null, tabs: [] }]
      })
    })
    await waitFor(() => {
      expect(releaseAttachment).toHaveBeenCalledWith(attachmentKey)
    })
  })
})
