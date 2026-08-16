import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { HerdrCapabilities, HerdrSnapshot } from "@/lib/herdrTypes"
import { herdrInitialState, useHerdrStore } from "@/state/herdrStore"

const ipc = vi.hoisted(() => ({
  worktreeList: vi.fn()
}))

vi.mock("@/lib/herdrIpc", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/herdrIpc")>()),
  herdrWorktreeList: (...args: unknown[]) => ipc.worktreeList(...args)
}))

const capabilities: HerdrCapabilities = {
  binaryPath: "/bin/herdr",
  binarySource: {
    configured: "global",
    resolved: "global",
    available: true,
    path: "/bin/herdr",
    reason: null,
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
}

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
      path: "/Users/me/yuzora",
      isLinkedWorktree: false,
      repoName: "yuzora"
    },
    {
      id: "ws-linked",
      label: "Feature",
      order: 1,
      focused: false,
      path: "/Users/me/feature",
      isLinkedWorktree: true,
      repoName: "yuzora"
    }
  ],
  agents: [],
  tabs: [],
  terminals: [],
  raw: null
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
        sessionDir: "/tmp/d",
        socketPath: "/tmp/d.sock"
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

describe("herdrStore worktree inventory", () => {
  it("reconciles inventory by open workspace id onto Space projection", async () => {
    ipc.worktreeList.mockImplementation(async (args: { workspaceId?: string | null }) => {
      if (args.workspaceId === "ws-linked") {
        return {
          source: {
            repoKey: "/Users/me/yuzora/.git",
            repoName: "yuzora",
            repoRoot: "/Users/me/yuzora",
            sourceCheckoutPath: "/Users/me/yuzora",
            sourceWorkspaceId: "ws-source"
          },
          worktrees: [
            {
              path: "/Users/me/yuzora",
              branch: "main",
              isBare: false,
              isDetached: false,
              isPrunable: false,
              isLinkedWorktree: false,
              label: "yuzora",
              openWorkspaceId: "ws-source"
            },
            {
              path: "/Users/me/feature",
              branch: "feature/x",
              isBare: false,
              isDetached: false,
              isPrunable: false,
              isLinkedWorktree: true,
              label: "feature",
              openWorkspaceId: "ws-linked"
            }
          ]
        }
      }
      return {
        source: {
          repoKey: "/Users/me/yuzora/.git",
          repoName: "yuzora",
          repoRoot: "/Users/me/yuzora",
          sourceCheckoutPath: "/Users/me/yuzora",
          sourceWorkspaceId: "ws-source"
        },
        worktrees: [
          {
            path: "/Users/me/yuzora",
            branch: "main",
            isBare: false,
            isDetached: false,
            isPrunable: false,
            isLinkedWorktree: false,
            label: "yuzora",
            openWorkspaceId: "ws-source"
          },
          {
            path: "/Users/me/feature",
            branch: "feature/x",
            isBare: false,
            isDetached: false,
            isPrunable: false,
            isLinkedWorktree: true,
            label: "feature",
            openWorkspaceId: "ws-linked"
          }
        ]
      }
    })

    await useHerdrStore.getState().refreshWorktreeInventory("default")
    const spaces = useHerdrStore.getState().snapshot?.spaces ?? []
    expect(spaces.find((s) => s.id === "ws-source")).toEqual(
      expect.objectContaining({
        branch: "main",
        isLinkedWorktree: false,
        sourceCheckoutPath: "/Users/me/yuzora"
      })
    )
    expect(spaces.find((s) => s.id === "ws-linked")).toEqual(
      expect.objectContaining({
        branch: "feature/x",
        isLinkedWorktree: true
      })
    )
    // Same repo should only list once after first success (dedupe by repo_key).
    expect(ipc.worktreeList).toHaveBeenCalled()
  })

  it("treats worktree_changed subscription events as inventory dirty signals", async () => {
    ipc.worktreeList.mockResolvedValue({
      source: {
        repoKey: "k",
        repoName: "r",
        repoRoot: "/r",
        sourceCheckoutPath: "/r",
        sourceWorkspaceId: "ws-source"
      },
      worktrees: [
        {
          path: "/r",
          branch: "main",
          isBare: false,
          isDetached: false,
          isPrunable: false,
          isLinkedWorktree: false,
          label: "r",
          openWorkspaceId: "ws-source"
        }
      ]
    })
    useHerdrStore.getState().applySubscriptionEvent("default", {
      type: "worktree_changed",
      subscriptionId: "sub-1",
      kind: "opened",
      workspaceId: "ws-linked"
    })
    await vi.waitFor(() => {
      expect(ipc.worktreeList).toHaveBeenCalled()
    })
  })
})
