import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

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
    agentGet: true,
    agentRead: true,
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
  const promise = new Promise<T>((res) => {
    resolve = res
  })
  return { promise, resolve }
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
