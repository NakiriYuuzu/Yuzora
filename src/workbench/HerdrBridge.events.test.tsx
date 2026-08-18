import { act, cleanup, render, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { HerdrCapabilities, HerdrSubscriptionEvent } from "@/lib/herdrTypes"
import { herdrInitialState, herdrStoreRuntimeKey, useHerdrStore } from "@/state/herdrStore"
import { useWorkspaceStore } from "@/state/workspaceStore"

const eventIpc = vi.hoisted(() => ({
  subscribe: vi.fn(),
  release: vi.fn()
}))

vi.mock("@/lib/herdrIpc", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/herdrIpc")>()),
  herdrEventsSubscribe: eventIpc.subscribe,
  herdrEventsRelease: eventIpc.release
}))

import { HerdrBridge } from "./HerdrBridge"

const capabilities: HerdrCapabilities = {
  binarySource: {
    configured: "global",
    active: "global",
    resolved: "global",
    available: true,
    configuredAvailable: true,
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
    methods: ["session.snapshot", "events.subscribe", "agent.get", "agent.read"]
  },
  terminal: {
    observe: true,
    control: true,
    takeover: true,
    input: true,
    resize: true,
    scroll: true,
    release: true,
    create: true
  },
  events: { status: "available" }
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
  }
]

beforeEach(() => {
  eventIpc.subscribe.mockReset()
  eventIpc.release.mockReset().mockResolvedValue(undefined)
  useWorkspaceStore.setState({ sessionRestoreReady: false })
  useHerdrStore.setState({
    ...herdrInitialState,
    sessions,
    selectedSessionName: "default",
    connectionState: "ready",
    capabilities,
    runtimesBySession: {
      default: { capabilities, snapshot: null, worktreeInventory: null, connectionState: "ready", errorMessage: null },
      work: { capabilities, snapshot: null, worktreeInventory: null, connectionState: "ready", errorMessage: null }
    },
    refreshSessions: vi.fn(async () => undefined),
    refreshSnapshot: vi.fn(async () => true),
    releaseAllAttachments: vi.fn(async () => undefined),
    selectedSession: () => {
      const state = useHerdrStore.getState()
      return state.sessions.find((item) => item.name === state.selectedSessionName) ?? null
    }
  })
})

afterEach(() => {
  cleanup()
  vi.useRealTimers()
  vi.restoreAllMocks()
})

describe("HerdrBridge event ownership", () => {
  it("refreshes snapshot and BSP topology when Herdr reports pane exit", async () => {
    let callback: ((event: HerdrSubscriptionEvent) => void) | undefined
    const refreshSnapshot = vi.fn(async () => true)
    eventIpc.subscribe.mockImplementation(
      async ({ onEvent }: { onEvent: (event: HerdrSubscriptionEvent) => void }) => {
        callback = onEvent
        onEvent({ type: "subscribed", subscriptionId: "sub-default" })
        return "sub-default"
      }
    )
    useHerdrStore.setState({ refreshSnapshot })

    render(<HerdrBridge />)
    await waitFor(() => expect(callback).toBeDefined())
    const before = useHerdrStore.getState().topologyRevision

    act(() => {
      callback?.({
        type: "pane_exited",
        subscriptionId: "sub-default",
        paneId: "w1:p2",
        workspaceId: "w1"
      })
    })

    await waitFor(() => expect(useHerdrStore.getState().topologyRevision).toBe(before + 1))
    await waitFor(() => expect(refreshSnapshot).toHaveBeenCalledWith("default", { kind: "native" }))
  })

  it("lets the store own worktree inventory refresh and only schedules snapshot recovery", async () => {
    let callback: ((event: HerdrSubscriptionEvent) => void) | undefined
    const refreshSnapshot = vi.fn(async () => true)
    const refreshWorktreeInventory = vi.fn(async () => undefined)
    eventIpc.subscribe.mockImplementation(
      async ({ onEvent }: { onEvent: (event: HerdrSubscriptionEvent) => void }) => {
        callback = onEvent
        onEvent({ type: "subscribed", subscriptionId: "sub-default" })
        return "sub-default"
      }
    )
    useHerdrStore.setState({ refreshSnapshot, refreshWorktreeInventory })

    render(<HerdrBridge />)
    await waitFor(() => expect(callback).toBeDefined())
    act(() => {
      callback?.({
        type: "worktree_changed",
        subscriptionId: "sub-default",
        kind: "created",
        workspaceId: "w1"
      })
    })

    await waitFor(() => expect(refreshWorktreeInventory).toHaveBeenCalledTimes(1))
    await waitFor(() => expect(refreshSnapshot).toHaveBeenCalledWith("default", { kind: "native" }))
  })

  it("refreshes after tab.closed and workspace topology events", async () => {
    let callback: ((event: HerdrSubscriptionEvent) => void) | undefined
    const refreshSnapshot = vi.fn(async () => true)
    eventIpc.subscribe.mockImplementation(
      async ({ onEvent }: { onEvent: (event: HerdrSubscriptionEvent) => void }) => {
        callback = onEvent
        onEvent({ type: "subscribed", subscriptionId: "sub-default" })
        return "sub-default"
      }
    )
    useHerdrStore.setState({ refreshSnapshot })

    render(<HerdrBridge />)
    await waitFor(() => expect(callback).toBeDefined())
    const before = useHerdrStore.getState().topologyRevision

    act(() => {
      callback?.({
        type: "topology_changed",
        subscriptionId: "sub-default",
        kind: "tab.closed",
        workspaceId: "w1",
        tabId: "t9"
      })
    })
    await waitFor(() => expect(useHerdrStore.getState().topologyRevision).toBe(before + 1))
    await waitFor(() => expect(refreshSnapshot).toHaveBeenCalledWith("default", { kind: "native" }))

    refreshSnapshot.mockClear()
    act(() => {
      callback?.({
        type: "topology_changed",
        subscriptionId: "sub-default",
        kind: "workspace.moved",
        workspaceId: "w2"
      })
    })
    await waitFor(() => expect(useHerdrStore.getState().topologyRevision).toBe(before + 2))
    await waitFor(() => expect(refreshSnapshot).toHaveBeenCalledWith("default", { kind: "native" }))
  })

  it("re-probes, re-snapshots, then resubscribes after a WSL proxy disconnect", async () => {
    vi.useFakeTimers()
    const ubuntu = { kind: "wsl" as const, distro: "Ubuntu" }
    const calls: string[] = []
    let callback: ((event: HerdrSubscriptionEvent) => void) | undefined
    const bootstrap = vi.fn(async () => { calls.push("probe") })
    const refreshSnapshot = vi.fn(async () => {
      calls.push("snapshot")
      return true
    })
    eventIpc.subscribe.mockImplementation(async ({ onEvent }: {
      onEvent: (event: HerdrSubscriptionEvent) => void
    }) => {
      calls.push("subscribe")
      callback = onEvent
      const id = calls.filter((call) => call === "subscribe").length === 1
        ? "sub-1"
        : "sub-2"
      onEvent({ type: "subscribed", subscriptionId: id })
      return id
    })
    useHerdrStore.setState((state) => ({
      sessions: [{ ...sessions[0]!, runtimeTarget: ubuntu }],
      selectedSessionName: "default",
      selectedRuntimeTarget: ubuntu,
      connectionState: "ready",
      capabilities,
      runtimesBySession: {
        ...state.runtimesBySession,
        [herdrStoreRuntimeKey("default", ubuntu)]: {
          runtimeTarget: ubuntu,
          capabilities,
          snapshot: null,
          baseSnapshot: null,
          worktreeInventory: null,
          connectionState: "ready",
          errorMessage: null
        }
      },
      bootstrap,
      refreshSnapshot
    }))

    render(<HerdrBridge />)
    await act(async () => {
      await Promise.resolve()
      await Promise.resolve()
    })
    expect(callback).toBeDefined()
    act(() => {
      callback?.({ type: "disconnected", subscriptionId: "sub-1", reason: "proxy EOF" })
    })
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1_000)
    })
    expect(bootstrap).toHaveBeenCalledWith("default", ubuntu)
    expect(refreshSnapshot).toHaveBeenCalledWith("default", ubuntu)
    expect(calls.indexOf("probe")).toBeLessThan(calls.lastIndexOf("snapshot"))
    expect(calls.lastIndexOf("snapshot")).toBeLessThan(calls.lastIndexOf("subscribe"))
  })

  it("bounds repeated WSL proxy disconnects to one reconnect generation", async () => {
    vi.useFakeTimers()
    const ubuntu = { kind: "wsl" as const, distro: "Ubuntu" }
    let callback: ((event: HerdrSubscriptionEvent) => void) | undefined
    const bootstrap = vi.fn(async () => undefined)
    const refreshSnapshot = vi.fn(async () => true)
    eventIpc.subscribe.mockImplementation(async ({ onEvent }: {
      onEvent: (event: HerdrSubscriptionEvent) => void
    }) => {
      callback = onEvent
      const id = eventIpc.subscribe.mock.calls.length === 1 ? "sub-1" : "sub-2"
      onEvent({ type: "subscribed", subscriptionId: id })
      return id
    })
    useHerdrStore.setState((state) => ({
      sessions: [{ ...sessions[0]!, runtimeTarget: ubuntu }],
      selectedSessionName: "default",
      selectedRuntimeTarget: ubuntu,
      connectionState: "ready",
      capabilities,
      runtimesBySession: {
        ...state.runtimesBySession,
        [herdrStoreRuntimeKey("default", ubuntu)]: {
          runtimeTarget: ubuntu,
          capabilities,
          snapshot: null,
          baseSnapshot: null,
          worktreeInventory: null,
          connectionState: "ready",
          errorMessage: null
        }
      },
      bootstrap,
      refreshSnapshot
    }))

    render(<HerdrBridge />)
    await act(async () => {
      await Promise.resolve()
      await Promise.resolve()
    })
    expect(callback).toBeDefined()
    const beforeReconnectSnapshots = refreshSnapshot.mock.calls.length
    const beforeReconnectSubscribes = eventIpc.subscribe.mock.calls.length
    act(() => {
      for (let index = 0; index < 16; index += 1) {
        callback?.({ type: "disconnected", subscriptionId: "sub-1", reason: "fixture crash" })
      }
    })
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1_000)
    })
    expect(bootstrap).toHaveBeenCalledTimes(1)
    // One event-driven recovery snapshot may race the one post-probe snapshot,
    // but repeated disconnect notices must not multiply proxy reconnects.
    expect(refreshSnapshot.mock.calls.length).toBeLessThanOrEqual(beforeReconnectSnapshots + 2)
    expect(eventIpc.subscribe).toHaveBeenCalledTimes(beforeReconnectSubscribes + 1)
  })

  it("rejects late events from the previous named session", async () => {
    const callbacks = new Map<string, (event: HerdrSubscriptionEvent) => void>()
    eventIpc.subscribe.mockImplementation(
      async ({
        sessionName,
        onEvent
      }: {
        sessionName: string
        onEvent: (event: HerdrSubscriptionEvent) => void
      }) => {
        callbacks.set(sessionName, onEvent)
        const subscriptionId = `sub-${sessionName}`
        onEvent({ type: "subscribed", subscriptionId })
        return subscriptionId
      }
    )

    render(<HerdrBridge />)
    await waitFor(() => expect(callbacks.has("default")).toBe(true))

    act(() => {
      useHerdrStore.setState({
        selectedSessionName: "work",
        capabilities,
        connectionState: "ready"
      })
    })
    await waitFor(() => expect(callbacks.has("work")).toBe(true))
    expect(useHerdrStore.getState().eventsSubscriptionId).toBe("sub-work")

    act(() => {
      callbacks.get("default")?.({
        type: "agent_status_changed",
        subscriptionId: "sub-default",
        paneId: "w1:p1",
        workspaceId: "w1",
        agentStatus: "done",
        title: "Old",
        stateLabels: {}
      })
    })
    expect(useHerdrStore.getState().attentionItems("work")).toHaveLength(0)
    expect(useHerdrStore.getState().attentionItems("default")).toHaveLength(0)

    act(() => {
      callbacks.get("work")?.({
        type: "agent_status_changed",
        subscriptionId: "sub-work",
        paneId: "w2:p2",
        workspaceId: "w2",
        agentStatus: "blocked",
        title: "Current",
        stateLabels: {}
      })
    })
    expect(useHerdrStore.getState().attentionItems("work")[0]?.title).toBe("Current")
    expect(eventIpc.release).toHaveBeenCalledWith("sub-default", { kind: "native" })
  })
})

it("owns event subscribe and release by RuntimeTarget for same-name WSL sessions", async () => {
  const ubuntu = { kind: "wsl" as const, distro: "Ubuntu" }
  let callback: ((event: HerdrSubscriptionEvent) => void) | undefined
  eventIpc.subscribe.mockImplementation(async ({ onEvent }: {
    onEvent: (event: HerdrSubscriptionEvent) => void
  }) => {
    callback = onEvent
    onEvent({ type: "subscribed", subscriptionId: "sub-ubuntu" })
    return "sub-ubuntu"
  })
  useHerdrStore.setState((state) => ({
    sessions: [{
      name: "default",
      default: true,
      running: true,
      sessionDir: "/tmp/ubuntu-default",
      socketPath: "/tmp/ubuntu-default.sock",
      runtimeTarget: ubuntu
    }],
    selectedSessionName: "default",
    selectedRuntimeTarget: ubuntu,
    connectionState: "ready",
    capabilities,
    runtimesBySession: {
      ...state.runtimesBySession,
      [herdrStoreRuntimeKey("default", ubuntu)]: {
        runtimeTarget: ubuntu,
        capabilities,
        snapshot: null,
        baseSnapshot: null,
        worktreeInventory: null,
        connectionState: "ready",
        errorMessage: null
      }
    }
  }))

  const view = render(<HerdrBridge />)
  await waitFor(() => expect(callback).toBeDefined())
  expect(eventIpc.subscribe).toHaveBeenCalledWith(expect.objectContaining({
    runtimeTarget: ubuntu,
    sessionName: "default"
  }))
  view.unmount()
  await waitFor(() => expect(eventIpc.release).toHaveBeenCalledWith("sub-ubuntu", ubuntu))
})
