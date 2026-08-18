import { beforeEach, describe, expect, it } from "vitest"

import { herdrAttentionKey, herdrInitialState, herdrStoreRuntimeKey, useHerdrStore } from "@/state/herdrStore"

describe("herdr attention model", () => {
  beforeEach(() => {
    useHerdrStore.setState({
      ...herdrInitialState,
      attachments: new Map(),
      attentionByKey: new Map(),
      selectedSessionName: "default",
      sessions: [
        {
          name: "default",
          default: true,
          running: true,
          sessionDir: "/tmp/d",
          socketPath: "/tmp/d.sock"
        }
      ],
      capabilities: {
        binaryPath: "/bin/herdr",
        binarySource: {
          configured: "global" as const,
          resolved: "global" as const,
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
          methods: ["agent.get", "agent.read", "events.subscribe"]
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
    })
  })

  it("tracks blocked/done/unknown by sessionName::paneId and does not clear on read", () => {
    useHerdrStore.getState().applySubscriptionEvent("default", {
      type: "subscribed",
      subscriptionId: "sub-1"
    })
    useHerdrStore.getState().applySubscriptionEvent("default", {
      type: "agent_status_changed",
      subscriptionId: "sub-1",
      paneId: "w1:p1",
      workspaceId: "w1",
      agentStatus: "blocked",
      title: "Reviewer",
      stateLabels: {}
    })
    useHerdrStore.getState().applySubscriptionEvent("default", {
      type: "agent_status_changed",
      subscriptionId: "sub-1",
      paneId: "w1:p1",
      workspaceId: "w1",
      agentStatus: "blocked",
      title: "Reviewer",
      stateLabels: {}
    })
    expect(useHerdrStore.getState().attentionItems()).toHaveLength(1)
    expect(useHerdrStore.getState().attentionItems()[0].key).toBe(
      herdrAttentionKey("default", "w1:p1")
    )

    useHerdrStore.getState().applySubscriptionEvent("default", {
      type: "agent_status_changed",
      subscriptionId: "sub-1",
      paneId: "w1:p1",
      workspaceId: "w1",
      agentStatus: "done",
      title: "Reviewer",
      stateLabels: {}
    })
    const done = useHerdrStore.getState().attentionItems()[0]
    expect(done.kind).toBe("done")
    expect(done.seen).toBe(false)

    // Reading does not mark seen — only markAttentionSeen does.
    expect(useHerdrStore.getState().attentionItems()).toHaveLength(1)
    useHerdrStore.getState().markAttentionSeen("default", "w1:p1")
    expect(useHerdrStore.getState().attentionItems()).toHaveLength(0)
  })

  it("isolates attention across named sessions", () => {
    useHerdrStore.getState().applySubscriptionEvent("default", {
      type: "subscribed",
      subscriptionId: "sub-1"
    })
    useHerdrStore.getState().applySubscriptionEvent("default", {
      type: "agent_status_changed",
      subscriptionId: "sub-1",
      paneId: "w1:p1",
      workspaceId: "w1",
      agentStatus: "blocked",
      stateLabels: {}
    })
    useHerdrStore.setState({ selectedSessionName: "work" })
    useHerdrStore.getState().applySubscriptionEvent("work", {
      type: "subscribed",
      subscriptionId: "sub-2"
    })
    useHerdrStore.getState().applySubscriptionEvent("work", {
      type: "agent_status_changed",
      subscriptionId: "sub-2",
      paneId: "w1:p1",
      workspaceId: "w1",
      agentStatus: "unknown",
      stateLabels: {}
    })
    expect(useHerdrStore.getState().attentionItems("default")).toHaveLength(1)
    expect(useHerdrStore.getState().attentionItems("work")).toHaveLength(1)
    expect(useHerdrStore.getState().attentionItems("work")[0].kind).toBe("unknown")
  })

  it("reconciles missed attention transitions from snapshots without a cursor", () => {
    useHerdrStore.getState().applySnapshot("default", {
      herdrSessionId: "default",
      protocol: 19,
      version: "0.8.0",
      spaces: [{ id: "w1", label: "Main", order: 0, focused: true }],
      tabs: [],
      terminals: [],
      agents: [
        {
          id: "w1:p1",
          name: "Reviewer",
          status: "blocked",
          workspaceId: "w1",
          paneId: "w1:p1",
          sessionName: "default"
        }
      ],
      focusedWorkspaceId: "w1",
      raw: {}
    })
    expect(useHerdrStore.getState().attentionItems("default")[0]?.kind).toBe("blocked")

    const snapshot = useHerdrStore.getState().runtimesBySession.default!.snapshot!
    useHerdrStore.getState().applySnapshot("default", { ...snapshot, agents: [] })
    expect(useHerdrStore.getState().attentionItems("default")).toHaveLength(0)
  })

  it("keeps Native/default and WSL Ubuntu/default snapshots and attention isolated", () => {
    const makeSnapshot = (runtimeTarget: { kind: "native" } | { kind: "wsl"; distro: string }) => ({
      herdrSessionId: "default",
      runtimeTarget,
      protocol: 19,
      version: "0.8.0",
      spaces: [{ id: "w1", label: "Main", order: 0, focused: true }],
      tabs: [],
      terminals: [],
      agents: [
        {
          id: "w1:p1",
          name: "Reviewer",
          status: "blocked" as const,
          workspaceId: "w1",
          paneId: "w1:p1",
          sessionName: "default",
          runtimeTarget
        }
      ],
      focusedWorkspaceId: "w1",
      raw: {}
    })
    const native = { kind: "native" } as const
    const ubuntu = { kind: "wsl", distro: "Ubuntu" } as const

    useHerdrStore.getState().applySnapshot("default", makeSnapshot(native), native)
    useHerdrStore.getState().applySnapshot("default", makeSnapshot(ubuntu), ubuntu)

    const state = useHerdrStore.getState()
    expect(state.runtimesBySession[herdrStoreRuntimeKey("default", native)]).toBeDefined()
    expect(state.runtimesBySession[herdrStoreRuntimeKey("default", ubuntu)]).toBeDefined()
    expect(state.attentionItems("default", native)).toHaveLength(1)
    expect(state.attentionItems("default", ubuntu)).toHaveLength(1)
    expect(herdrAttentionKey("default", "w1:p1", native)).not.toBe(
      herdrAttentionKey("default", "w1:p1", ubuntu)
    )
  })

  it("exposes inspect capability from agent.get/read flags", () => {
    expect(useHerdrStore.getState().canInspectAgent()).toBe(true)
    useHerdrStore.setState({
      capabilities: {
        ...useHerdrStore.getState().capabilities!,
        api: {
          ...useHerdrStore.getState().capabilities!.api,
          agentGet: false,
          agentRead: false
        }
      }
    })
    expect(useHerdrStore.getState().canInspectAgent()).toBe(false)
  })
})
