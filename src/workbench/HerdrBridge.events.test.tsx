import { act, cleanup, render, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { HerdrCapabilities, HerdrSubscriptionEvent } from "@/lib/herdrTypes"
import type { ConnectedHost } from "@/lib/hostIpc"
import { registerRuntimeHost, unregisterRuntimeHost } from "@/lib/herdrProvider"
import { useHostStore } from "@/state/hostStore"
import { herdrInitialState, useHerdrStore } from "@/state/herdrStore"
import { useWorkspaceStore } from "@/state/workspaceStore"

const eventIpc = vi.hoisted(() => ({
  subscribe: vi.fn(),
  release: vi.fn(),
  sessions: vi.fn(),
  snapshot: vi.fn(),
  capabilities: vi.fn()
}))

vi.mock("@/lib/herdrIpc", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/herdrIpc")>()),
  herdrEventsSubscribe: eventIpc.subscribe,
  herdrEventsRelease: eventIpc.release,
  herdrSessions: eventIpc.sessions,
  herdrSnapshot: eventIpc.snapshot,
  herdrCapabilities: eventIpc.capabilities
}))

import { HerdrBridge } from "./HerdrBridge"

const initialHerdrState = useHerdrStore.getState()
const initialHostState = useHostStore.getState()
const initialWorkspaceState = useWorkspaceStore.getState()

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
    sessions: sessions.slice(0, 1),
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
  vi.restoreAllMocks()
})

describe("HerdrBridge event ownership", () => {
  it("takes a fresh snapshot after the live-only subscription is acknowledged", async () => {
    let callback: ((event: HerdrSubscriptionEvent) => void) | undefined
    eventIpc.subscribe.mockImplementation(async ({ onEvent }: { onEvent: (event: HerdrSubscriptionEvent) => void }) => {
      callback = onEvent
      return "sub-live-only"
    })
    render(<HerdrBridge />)
    await waitFor(() => expect(callback).toBeDefined())
    const refresh = useHerdrStore.getState().refreshSnapshot
    const before = vi.mocked(refresh).mock.calls.length
    act(() => callback?.({ type: "subscribed", subscriptionId: "sub-live-only" }))
    await waitFor(() => expect(refresh).toHaveBeenCalledTimes(before + 1), { timeout: 700 })
  })

  it("replaces per-pane selectors only when pane membership changes and drops the old callback", async () => {
    const callbacks: Array<(event: HerdrSubscriptionEvent) => void> = []
    eventIpc.subscribe.mockImplementation(async ({ onEvent }: { onEvent: (event: HerdrSubscriptionEvent) => void }) => {
      callbacks.push(onEvent)
      const id = `sub-${callbacks.length}`
      onEvent({ type: "subscribed", subscriptionId: id })
      return id
    })
    const snapshot = {
      herdrSessionId: "default", protocol: 20, version: "0.8.2",
      spaces: [], agents: [], tabs: [], terminals: [{ terminalId: "term-1", paneId: "w1:p1" }], raw: {}
    }
    const setSnapshot = (value: typeof snapshot) => {
      const state = useHerdrStore.getState()
      useHerdrStore.setState({ runtimesBySession: {
        ...state.runtimesBySession,
        default: { ...state.runtimesBySession.default!, snapshot: value }
      } })
    }
    setSnapshot(snapshot)
    render(<HerdrBridge />)
    await waitFor(() => expect(callbacks).toHaveLength(1))
    expect(eventIpc.subscribe.mock.calls[0][0].paneIds).toEqual(["w1:p1"])
    await act(async () => { setSnapshot({ ...snapshot, raw: { revision: 2 } }) })
    expect(callbacks).toHaveLength(1)
    await act(async () => { setSnapshot({ ...snapshot, terminals: [...snapshot.terminals, { terminalId: "term-2", paneId: "w1:p2" }] }) })
    await waitFor(() => expect(callbacks).toHaveLength(2))
    expect(eventIpc.release).toHaveBeenCalledWith("sub-1")
    expect(eventIpc.subscribe.mock.calls[1][0].paneIds).toEqual(["w1:p1", "w1:p2"])
    const revision = useHerdrStore.getState().topologyRevision
    await act(async () => { callbacks[0]({ type: "pane_exited", subscriptionId: "sub-1", paneId: "w1:p1", workspaceId: "w1" }) })
    expect(useHerdrStore.getState().topologyRevision).toBe(revision)
  })

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
    await waitFor(() => expect(refreshSnapshot).toHaveBeenCalledWith("default"))
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
    await waitFor(() => expect(refreshSnapshot).toHaveBeenCalledWith("default"))
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
    await waitFor(() => expect(refreshSnapshot).toHaveBeenCalledWith("default"))

    refreshSnapshot.mockClear()
    act(() => {
      callback?.({
        type: "topology_changed",
        subscriptionId: "sub-default",
        kind: "workspace.reordered",
        workspaceId: "w2"
      })
    })
    await waitFor(() => expect(useHerdrStore.getState().topologyRevision).toBe(before + 2))
    await waitFor(() => expect(refreshSnapshot).toHaveBeenCalledWith("default"))
  })

  it("keeps background sessions subscribed and isolates their attention", async () => {
    useHerdrStore.setState({ sessions })
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
    expect(useHerdrStore.getState().attentionItems("default")).toHaveLength(1)

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
    expect(useHerdrStore.getState().attentionItems()).toHaveLength(2)
    expect(eventIpc.release).not.toHaveBeenCalledWith("sub-default")
    cleanup()
    expect(eventIpc.release).toHaveBeenCalledWith("sub-default")
    expect(eventIpc.release).toHaveBeenCalledWith("sub-work")
  })
})

describe("HerdrBridge polling while Host discovery is pending", () => {
  const connected: ConnectedHost = {
    owner: { hostId: "ssh:polling", generation: 1 },
    hello: { protocol: 1, version: "0.0.16", os: "linux", arch: "x86_64", home: "/home/test", methods: ["herdrCall"] }
  }
  const remote = { ...sessions[0], hostId: connected.owner.hostId, runtimeId: JSON.stringify([connected.owner.hostId, "default"]) }
  const snapshot = { protocol: 22, version: "0.9.1", workspaces: [], tabs: [], panes: [] }
  const pending: Array<() => void> = []
  const deferred = <T,>(value: T) => {
    let resolve!: () => void
    const promise = new Promise<T>(done => { resolve = () => done(value) })
    pending.push(resolve)
    return { promise, resolve }
  }
  const tick = async (ms = 0) => {
    await act(async () => { await vi.advanceTimersByTimeAsync(ms) })
  }
  const seed = (isRemote: boolean, events = false) => {
    const session = isRemote ? remote : sessions[0]
    const scope = isRemote ? remote.runtimeId : session.name
    const caps = { ...capabilities, binaryPath: "/bin/herdr", api: { ...capabilities.api, eventsSubscribe: events, worktreeList: false } }
    if (isRemote) registerRuntimeHost(connected, "/bin/herdr", "Polling Host")
    eventIpc.capabilities.mockResolvedValue(caps)
    useHerdrStore.setState({
      sessions: [session], selectedSessionName: scope,
      runtimesBySession: { [scope]: { capabilities: caps, snapshot: null, worktreeInventory: null, connectionState: "ready", errorMessage: null } }
    })
    return scope
  }

  beforeEach(() => {
    vi.useFakeTimers()
    vi.setSystemTime(new Date("2026-09-26T12:00:00Z"))
    useHerdrStore.setState({ ...initialHerdrState, ...herdrInitialState }, true)
    eventIpc.sessions.mockReset()
    eventIpc.snapshot.mockReset().mockResolvedValue(snapshot)
    eventIpc.capabilities.mockReset()
  })

  afterEach(async () => {
    cleanup()
    await act(async () => { pending.splice(0).forEach(resolve => resolve()) })
    unregisterRuntimeHost(connected.owner)
    unregisterRuntimeHost({ ...connected.owner, generation: 2 })
    useHerdrStore.setState(initialHerdrState, true)
    useHostStore.setState(initialHostState, true)
    useWorkspaceStore.setState(initialWorkspaceState, true)
    vi.useRealTimers()
  })

  it.each([false, true])("keeps four-second snapshots single-flight through 20s discovery (remote: %s)", async (isRemote) => {
    const scope = seed(isRemote)
    const discovery = deferred(useHerdrStore.getState().sessions)
    eventIpc.sessions.mockReturnValue(discovery.promise)
    render(<HerdrBridge />)
    await tick()
    expect(eventIpc.snapshot).toHaveBeenCalledTimes(1)
    for (let count = 2; count <= 6; count++) {
      await tick(4000)
      expect(eventIpc.snapshot).toHaveBeenCalledTimes(count)
      expect(eventIpc.snapshot).toHaveBeenLastCalledWith(scope)
    }
    expect(eventIpc.sessions).toHaveBeenCalledTimes(1)

    const heldSnapshot = deferred(snapshot)
    eventIpc.snapshot.mockReturnValueOnce(heldSnapshot.promise)
    // Observe the store boundary too: store coalescing must not hide overlapping Bridge refreshes.
    const refresh = vi.spyOn(useHerdrStore.getState(), "refreshSnapshot")
    await tick(4000)
    expect(refresh).toHaveBeenCalledTimes(1)
    expect(eventIpc.snapshot).toHaveBeenCalledTimes(7)
    await tick(8000)
    expect(refresh).toHaveBeenCalledTimes(1)
    expect(eventIpc.snapshot).toHaveBeenCalledTimes(7)
    await act(async () => { heldSnapshot.resolve() })
    await tick(4000)
    expect(refresh).toHaveBeenCalledTimes(2)
    expect(eventIpc.snapshot).toHaveBeenCalledTimes(8)
    expect(eventIpc.sessions).toHaveBeenCalledTimes(1)
    await act(async () => { discovery.resolve() })
  })

  it.each([false, true])("does not poll existing runtimes again when discovery settles after a tick (remote: %s)", async (isRemote) => {
    const scope = seed(isRemote)
    const discovered = useHerdrStore.getState().sessions
    eventIpc.sessions.mockImplementation(() => new Promise(resolve => {
      setTimeout(() => resolve(discovered), 100)
    }))
    render(<HerdrBridge />)
    await tick()
    expect(eventIpc.snapshot).toHaveBeenCalledExactlyOnceWith(scope)
    await tick(100)
    expect(eventIpc.snapshot).toHaveBeenCalledTimes(1)
    for (let count = 2; count <= 4; count++) {
      await tick(3900)
      expect(eventIpc.snapshot).toHaveBeenCalledTimes(count)
      await tick(100)
      expect(eventIpc.snapshot).toHaveBeenCalledTimes(count)
      expect(eventIpc.sessions).toHaveBeenCalledTimes(count)
    }
  })

  it("starts newly discovered runtimes without repeating existing snapshots", async () => {
    seed(false)
    const discovery = deferred(sessions)
    eventIpc.sessions.mockReturnValueOnce(discovery.promise).mockResolvedValue(sessions)
    render(<HerdrBridge />)
    await tick()
    expect(eventIpc.snapshot).toHaveBeenCalledExactlyOnceWith("default")
    await act(async () => { discovery.resolve() })
    await tick()
    expect(eventIpc.snapshot.mock.calls.map(([scope]) => scope)).toEqual(["default", "work"])
    expect(eventIpc.capabilities).toHaveBeenCalledExactlyOnceWith("work")
    await tick(4000)
    expect(eventIpc.snapshot.mock.calls.map(([scope]) => scope)).toEqual(["default", "work", "default", "work"])
  })

  it("keeps healthy-event fallback at 12s and releases a stopped Session as soon as discovery settles", async () => {
    seed(false, true)
    const discovery = deferred([{ ...sessions[0], running: false }])
    eventIpc.sessions.mockReturnValue(discovery.promise)
    let callback!: (event: HerdrSubscriptionEvent) => void
    eventIpc.subscribe.mockImplementation(async ({ onEvent }: { onEvent: typeof callback }) => {
      callback = onEvent
      return "sub-healthy"
    })
    render(<HerdrBridge />)
    await tick()
    expect(eventIpc.snapshot).toHaveBeenCalledTimes(1)
    expect(eventIpc.subscribe).toHaveBeenCalledTimes(1)
    for (let seconds = 4; seconds <= 24; seconds += 4) {
      await tick(4000)
      expect(eventIpc.snapshot).toHaveBeenCalledTimes(1 + Math.floor(seconds / 12))
    }
    expect(eventIpc.sessions).toHaveBeenCalledTimes(1)
    await act(async () => { discovery.resolve() })
    expect(eventIpc.release).toHaveBeenCalledExactlyOnceWith("sub-healthy")
    const revision = useHerdrStore.getState().topologyRevision
    act(() => callback({ type: "pane_exited", subscriptionId: "sub-healthy", workspaceId: "w1", paneId: "p1" }))
    await tick(4000)
    expect(useHerdrStore.getState().topologyRevision).toBe(revision)
    expect(eventIpc.snapshot).toHaveBeenCalledTimes(3)
    expect(eventIpc.subscribe).toHaveBeenCalledTimes(1)
  })

  it("ignores late discovery and subscription callbacks after unmount", async () => {
    seed(false, true)
    const discovery = deferred(sessions)
    const subscription = deferred("sub-late")
    eventIpc.sessions.mockReturnValue(discovery.promise)
    let callback!: (event: HerdrSubscriptionEvent) => void
    eventIpc.subscribe.mockImplementation(({ onEvent }: { onEvent: typeof callback }) => {
      callback = onEvent
      return subscription.promise
    })
    const view = render(<HerdrBridge />)
    await tick()
    expect(eventIpc.snapshot).toHaveBeenCalledTimes(1)
    expect(eventIpc.subscribe).toHaveBeenCalledTimes(1)
    view.unmount()
    const revision = useHerdrStore.getState().topologyRevision
    await act(async () => {
      discovery.resolve()
      subscription.resolve()
      callback({ type: "subscribed", subscriptionId: "sub-late" })
      callback({ type: "pane_exited", subscriptionId: "sub-late", workspaceId: "w1", paneId: "p1" })
    })
    await tick(20000)
    expect(eventIpc.release).toHaveBeenCalledExactlyOnceWith("sub-late")
    expect(useHerdrStore.getState().topologyRevision).toBe(revision)
    expect(eventIpc.snapshot).toHaveBeenCalledTimes(1)
    expect(eventIpc.sessions).toHaveBeenCalledTimes(1)
    expect(eventIpc.subscribe).toHaveBeenCalledTimes(1)
  })

  it("rebootstraps a replacement owner and ignores its predecessor's callback during discovery", async () => {
    const scope = seed(true, true)
    const discovery = deferred([remote])
    eventIpc.sessions.mockReturnValue(discovery.promise)
    const callbacks: Array<(event: HerdrSubscriptionEvent) => void> = []
    eventIpc.subscribe.mockImplementation(async ({ onEvent }: { onEvent: (event: HerdrSubscriptionEvent) => void }) => {
      callbacks.push(onEvent)
      return `sub-owner-${callbacks.length}`
    })
    render(<HerdrBridge />)
    await tick()
    expect(callbacks).toHaveLength(1)
    expect(eventIpc.capabilities).toHaveBeenCalledExactlyOnceWith(scope)
    const replacement = { ...connected, owner: { ...connected.owner, generation: 2 } }
    await act(async () => {
      registerRuntimeHost(replacement, "/bin/herdr", "Replacement Host")
      useHostStore.setState({ hosts: { [connected.owner.hostId]: {
        connection: replacement, connecting: false, error: null,
        target: { kind: "ssh", sessionId: "ssh-connection" }, attempt: 0, retryAt: 0
      } } })
    })
    expect(eventIpc.release).toHaveBeenCalledExactlyOnceWith("sub-owner-1")
    expect(eventIpc.capabilities).toHaveBeenCalledTimes(2)
    expect(eventIpc.capabilities).toHaveBeenLastCalledWith(scope)
    expect(callbacks).toHaveLength(2)
    const revision = useHerdrStore.getState().topologyRevision
    act(() => callbacks[0]({ type: "pane_exited", subscriptionId: "sub-owner-1", workspaceId: "w1", paneId: "p1" }))
    await tick(250)
    expect(useHerdrStore.getState().topologyRevision).toBe(revision)
    expect(eventIpc.snapshot).toHaveBeenCalledTimes(2)
    expect(eventIpc.sessions).toHaveBeenCalledTimes(1)
    act(() => callbacks[1]({ type: "pane_exited", subscriptionId: "sub-owner-2", workspaceId: "w1", paneId: "p1" }))
    await tick(250)
    expect(useHerdrStore.getState().topologyRevision).toBe(revision + 1)
    expect(eventIpc.snapshot).toHaveBeenCalledTimes(3)
  })
})
