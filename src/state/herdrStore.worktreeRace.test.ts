import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { registerRuntimeHost, runtimeOwner, StaleRuntimeResponse, unregisterRuntimeHost } from "@/lib/herdrProvider"
import { runtimeKey } from "@/lib/runtimeIdentity"
import type {
  HerdrCapabilities,
  HerdrSnapshot,
  HerdrWorktreeListResult
} from "@/lib/herdrTypes"
import { herdrInitialState, useHerdrStore } from "@/state/herdrStore"

const ipc = vi.hoisted(() => ({ worktreeList: vi.fn() }))

vi.mock("@/lib/herdrIpc", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/herdrIpc")>()),
  herdrWorktreeList: (...args: unknown[]) => ipc.worktreeList(...args)
}))

const capabilities = {
  binaryPath: "/bin/herdr",
  binarySource: {
    configured: "global",
    resolved: "global",
    available: true,
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
    methods: ["session.snapshot", "worktree.list"]
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
} satisfies HerdrCapabilities

const snapshot: HerdrSnapshot = {
  herdrSessionId: "default",
  protocol: 19,
  version: "0.8.0",
  spaces: [
    {
      id: "ws-source",
      label: "Yuzora",
      order: 0,
      focused: true,
      path: "/repo",
      repoKey: "repo-a",
      repoName: "repo",
      isLinkedWorktree: false
    },
    {
      id: "ws-linked",
      label: "Feature",
      order: 1,
      focused: false,
      path: "/feature",
      repoKey: "repo-a",
      repoName: "repo",
      isLinkedWorktree: true
    }
  ],
  agents: [],
  tabs: [],
  terminals: [],
  raw: null
}

function result(branch = "main", includeLinked = true): HerdrWorktreeListResult {
  return {
    source: {
      repoKey: "repo-a",
      repoName: "repo",
      repoRoot: "/repo",
      sourceCheckoutPath: "/repo",
      sourceWorkspaceId: "ws-source"
    },
    worktrees: [
      {
        path: "/repo",
        branch,
        isBare: false,
        isDetached: false,
        isPrunable: false,
        isLinkedWorktree: false,
        label: "repo",
        openWorkspaceId: "ws-source"
      },
      ...(includeLinked
        ? [
            {
              path: "/feature",
              branch: "feature/x",
              isBare: false,
              isDetached: false,
              isPrunable: false,
              isLinkedWorktree: true,
              label: "feature",
              openWorkspaceId: "ws-linked"
            }
          ]
        : [])
    ]
  }
}

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (reason: unknown) => void
  const promise = new Promise<T>((res, rej) => {
    resolve = res
    reject = rej
  })
  return { promise, resolve, reject }
}

const remoteScope = runtimeKey({ hostId: "inventory-host", sessionName: "default" })

function registerRemote(generation: number) {
  registerRuntimeHost({
    owner: { hostId: "inventory-host", generation },
    hello: { protocol: 1, version: "test", os: "linux", arch: "aarch64", home: "/home/test", methods: [] }
  }, "/bin/herdr", "Inventory Host")
}

function setupRemote() {
  registerRemote(1)
  useHerdrStore.setState((state) => ({
    sessions: state.sessions.map((session) => ({
      ...session,
      hostId: "inventory-host",
      runtimeId: remoteScope
    })),
    selectedSessionName: remoteScope,
    runtimesBySession: { [remoteScope]: state.runtimesBySession.default! }
  }))
  useHerdrStore.getState().applySnapshot(remoteScope, { ...snapshot, herdrSessionId: remoteScope })
}

beforeEach(() => {
  ipc.worktreeList.mockReset()
  useHerdrStore.setState({
    ...herdrInitialState,
    attachments: new Map(),
    attentionByKey: new Map(),
    sessions: [
      {
        name: "default",
        default: true,
        running: true,
        sessionDir: "/tmp/default",
        socketPath: "/tmp/default.sock"
      }
    ],
    selectedSessionName: "default",
    connectionState: "ready",
    capabilities,
    snapshot,
    runtimesBySession: {
      default: {
        capabilities,
        snapshot,
        baseSnapshot: snapshot,
        worktreeInventory: null,
        connectionState: "ready",
        errorMessage: null
      }
    },
    eventsHealthy: true,
    eventsSubscriptionId: "sub-1"
  })
})

afterEach(() => {
  const owner = runtimeOwner(remoteScope)
  if (owner) unregisterRuntimeHost(owner)
  useHerdrStore.setState({
    ...herdrInitialState,
    attachments: new Map(),
    attentionByKey: new Map()
  })
})

describe("worktree inventory reconciliation races", () => {
  it("queries one representative per known repository", async () => {
    ipc.worktreeList.mockResolvedValue(result())
    await useHerdrStore.getState().refreshWorktreeInventory("default")
    expect(ipc.worktreeList).toHaveBeenCalledTimes(1)
    expect(ipc.worktreeList).toHaveBeenCalledWith({
      sessionName: "default",
      workspaceId: "ws-source"
    })
  })

  it("commits a pending list across ten status-only snapshots without relisting", async () => {
    const first = deferred<HerdrWorktreeListResult>()
    ipc.worktreeList.mockReturnValueOnce(first.promise).mockResolvedValue(result("unexpected-retry"))

    const refresh = useHerdrStore.getState().refreshWorktreeInventory("default")
    for (let revision = 1; revision <= 10; revision += 1) {
      useHerdrStore.getState().applySnapshot("default", {
        ...snapshot,
        spaces: snapshot.spaces.map((space) => ({ ...space, agentStatus: revision % 2 ? "working" : "idle" }))
      })
    }
    first.resolve(result("accepted"))
    await refresh

    expect(ipc.worktreeList).toHaveBeenCalledTimes(1)
    expect(useHerdrStore.getState().snapshot?.spaces[0]).toMatchObject({
      branch: "accepted",
      agentStatus: "idle"
    })
    expect(useHerdrStore.getState().runtimesBySession.default?.worktreeInventory?.failedScopes).toEqual([])
  })

  it.each(["resolve", "reject"])("stops the old Host generation on %s and lets its replacement refresh independently", async (outcome) => {
    setupRemote()
    const first = deferred<HerdrWorktreeListResult>()
    const replacement = deferred<HerdrWorktreeListResult>()
    ipc.worktreeList.mockReturnValueOnce(first.promise).mockReturnValueOnce(replacement.promise).mockResolvedValue(result("unexpected-retry"))
    const refresh = useHerdrStore.getState().refreshWorktreeInventory(remoteScope)
    registerRemote(2)
    const nextRefresh = useHerdrStore.getState().refreshWorktreeInventory(remoteScope)
    const callsAtReplacementLaunch = ipc.worktreeList.mock.calls.length
    const published: Array<string | null | undefined> = []
    const unsubscribe = useHerdrStore.subscribe((state) => published.push(state.snapshot?.spaces[0]?.branch))
    // Settle both even against the old implementation, so failed tests leave no pending tasks.
    if (outcome === "resolve") first.resolve(result("stale"))
    else first.reject(new StaleRuntimeResponse())
    replacement.resolve(result("current"))
    await Promise.all([refresh, nextRefresh])
    unsubscribe()

    expect(callsAtReplacementLaunch).toBe(2)
    expect(published).toEqual(["current"])
    expect(ipc.worktreeList).toHaveBeenCalledTimes(2)
    expect(useHerdrStore.getState().snapshot?.spaces[0]?.branch).toBe("current")
  })

  it.each(["resolve", "reject"])("does not retry or publish after Host disconnect on %s", async (outcome) => {
    setupRemote()
    const first = deferred<HerdrWorktreeListResult>()
    ipc.worktreeList.mockReturnValueOnce(first.promise).mockResolvedValue(result("unexpected-retry"))
    const refresh = useHerdrStore.getState().refreshWorktreeInventory(remoteScope)
    unregisterRuntimeHost(runtimeOwner(remoteScope)!)
    if (outcome === "resolve") first.resolve(result("stale"))
    else first.reject(new StaleRuntimeResponse())
    await refresh
    await useHerdrStore.getState().refreshWorktreeInventory(remoteScope)

    expect(ipc.worktreeList).toHaveBeenCalledTimes(1)
    expect(useHerdrStore.getState().runtimesBySession[remoteScope]?.worktreeInventory).toBeNull()
  })

  it("does not recreate a removed runtime when its pending list settles", async () => {
    const first = deferred<HerdrWorktreeListResult>()
    ipc.worktreeList.mockReturnValueOnce(first.promise).mockResolvedValue(result("unexpected-retry"))
    const refresh = useHerdrStore.getState().refreshWorktreeInventory("default")
    useHerdrStore.setState({ sessions: [], runtimesBySession: {} })
    first.resolve(result("stale"))
    await refresh

    expect(ipc.worktreeList).toHaveBeenCalledTimes(1)
    expect(useHerdrStore.getState().runtimesBySession).toEqual({})
  })

  it("stops rather than retrying a stale-runtime error", async () => {
    ipc.worktreeList.mockRejectedValueOnce(new StaleRuntimeResponse()).mockResolvedValue(result("unexpected-retry"))
    await useHerdrStore.getState().refreshWorktreeInventory("default")

    expect(ipc.worktreeList).toHaveBeenCalledTimes(1)
    expect(useHerdrStore.getState().runtimesBySession.default?.worktreeInventory).toBeNull()
  })

  it("clears stale list-owned fields for omission and exhausted failure", async () => {
    ipc.worktreeList.mockResolvedValueOnce(result())
    await useHerdrStore.getState().refreshWorktreeInventory("default")
    expect(
      useHerdrStore.getState().snapshot?.spaces.find((space) => space.id === "ws-linked")
        ?.branch
    ).toBe("feature/x")

    ipc.worktreeList.mockResolvedValueOnce(result("main", false))
    await useHerdrStore.getState().refreshWorktreeInventory("default")
    expect(
      useHerdrStore.getState().snapshot?.spaces.find((space) => space.id === "ws-linked")
        ?.branch
    ).toBeUndefined()

    ipc.worktreeList.mockRejectedValue(new Error("unavailable"))
    await useHerdrStore.getState().refreshWorktreeInventory("default")
    expect(
      useHerdrStore.getState().snapshot?.spaces.find((space) => space.id === "ws-source")
        ?.branch
    ).toBeUndefined()
    expect(ipc.worktreeList).toHaveBeenCalledTimes(4)
    expect(
      useHerdrStore.getState().runtimesBySession.default?.worktreeInventory
        ?.failedScopes
    ).toEqual(["repo-a"])
  })

  it("retries a transient list failure once before exposing a failed scope", async () => {
    ipc.worktreeList
      .mockRejectedValueOnce(new Error("temporary"))
      .mockResolvedValueOnce(result("recovered"))

    await useHerdrStore.getState().refreshWorktreeInventory("default")

    expect(ipc.worktreeList).toHaveBeenCalledTimes(2)
    expect(
      useHerdrStore.getState().snapshot?.spaces.find((space) => space.id === "ws-source")
        ?.branch
    ).toBe("recovered")
    expect(
      useHerdrStore.getState().runtimesBySession.default?.worktreeInventory
        ?.failedScopes
    ).toEqual([])
  })

  it("runs one follow-up pass when a dirty event arrives in flight", async () => {
    const first = deferred<HerdrWorktreeListResult>()
    ipc.worktreeList
      .mockReturnValueOnce(first.promise)
      .mockResolvedValueOnce(result("new"))

    const refresh = useHerdrStore.getState().refreshWorktreeInventory("default")
    useHerdrStore.getState().applySubscriptionEvent("default", {
      type: "worktree_changed",
      subscriptionId: "sub-1",
      kind: "opened",
      workspaceId: "ws-linked"
    })
    first.resolve(result("old"))
    await refresh

    expect(ipc.worktreeList).toHaveBeenCalledTimes(2)
    expect(
      useHerdrStore.getState().snapshot?.spaces.find((space) => space.id === "ws-source")
        ?.branch
    ).toBe("new")
  })

  it.each(["repository", "membership"])("rejects old inventory before delivery when %s changes", async (change) => {
    const first = deferred<HerdrWorktreeListResult>()
    const second = deferred<HerdrWorktreeListResult>()
    ipc.worktreeList.mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise)
    const refresh = useHerdrStore.getState().refreshWorktreeInventory("default")
    const nextSnapshot = {
      ...snapshot,
      spaces: change === "repository"
        ? snapshot.spaces.map((space) => ({ ...space, repoKey: "repo-b" }))
        : snapshot.spaces.filter((space) => space.id === "ws-linked")
    }
    useHerdrStore.getState().applySnapshot("default", nextSnapshot)
    const published: Array<string | null | undefined> = []
    const unsubscribe = useHerdrStore.subscribe((state) => published.push(state.snapshot?.spaces[0]?.branch))
    first.resolve(result("stale"))
    const next = result("current")
    if (change === "repository") next.source.repoKey = "repo-b"
    else next.worktrees = [{ ...next.worktrees[1]!, branch: "current" }]
    second.resolve(next)
    await refresh
    unsubscribe()

    expect(ipc.worktreeList).toHaveBeenCalledTimes(2)
    expect(ipc.worktreeList).toHaveBeenLastCalledWith({
      sessionName: "default",
      workspaceId: change === "repository" ? "ws-source" : "ws-linked"
    })
    expect(published).toEqual(["current"])
    expect(useHerdrStore.getState().snapshot?.spaces).toHaveLength(nextSnapshot.spaces.length)
    expect(useHerdrStore.getState().runtimesBySession.default?.worktreeInventory?.lists).toEqual([next])
  })

  it("retires an empty inventory pass so a later topology can refresh", async () => {
    useHerdrStore.getState().applySnapshot("default", { ...snapshot, spaces: [] })
    await useHerdrStore.getState().refreshWorktreeInventory("default")
    expect(ipc.worktreeList).not.toHaveBeenCalled()
    useHerdrStore.getState().applySnapshot("default", snapshot)
    ipc.worktreeList.mockResolvedValue(result("current"))
    await useHerdrStore.getState().refreshWorktreeInventory("default")

    expect(ipc.worktreeList).toHaveBeenCalledTimes(1)
    expect(useHerdrStore.getState().snapshot?.spaces[0]?.branch).toBe("current")
  })

  it("does not overlay a list response captured before a newer snapshot", async () => {
    const first = deferred<HerdrWorktreeListResult>()
    const next = result("new")
    next.worktrees[0] = { ...next.worktrees[0]!, path: "/repo-new" }
    ipc.worktreeList.mockReturnValueOnce(first.promise).mockResolvedValueOnce(next)

    const refresh = useHerdrStore.getState().refreshWorktreeInventory("default")
    useHerdrStore.getState().applySnapshot("default", {
      ...snapshot,
      spaces: snapshot.spaces.map((space) =>
        space.id === "ws-source" ? { ...space, path: "/repo-new" } : space
      )
    })
    first.resolve(result("old"))
    await refresh

    expect(ipc.worktreeList).toHaveBeenCalledTimes(2)
    const current = useHerdrStore
      .getState()
      .snapshot?.spaces.find((space) => space.id === "ws-source")
    expect(current?.branch).toBe("new")
    expect(current?.path).toBe("/repo-new")
  })
})
