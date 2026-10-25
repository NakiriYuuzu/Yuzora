import { beforeEach, describe, expect, it } from "vitest"

import { herdrAttentionKey, herdrInitialState, useHerdrStore } from "@/state/herdrStore"
import type { HerdrAgentStatus, HerdrSnapshot } from "@/lib/herdrTypes"
import { newHerdrAttention } from "@/lib/herdrNotifications"

function statusEvent(sessionName: string, agentStatus: HerdrAgentStatus) {
  useHerdrStore.getState().applySubscriptionEvent(sessionName, {
    type: "agent_status_changed", subscriptionId: `sub-${sessionName}`,
    paneId: "w1:p1", workspaceId: "w1", agentStatus, stateLabels: {}
  })
}

function statusSnapshot(sessionName: string, status: HerdrAgentStatus): HerdrSnapshot {
  return {
    herdrSessionId: sessionName, protocol: 22, version: "0.9.1", raw: {},
    spaces: [], tabs: [], terminals: [],
    agents: [{ id: "agent", name: "Agent", paneId: "w1:p1", workspaceId: "w1", status }]
  }
}

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

  it.each(["blocked", "unknown"] as const)("makes a completion after seen %s unread, including its next snapshot", status => {
    useHerdrStore.getState().setEventsHealth("default", true, "sub-default")
    statusEvent("default", status)
    useHerdrStore.getState().markAttentionSeen("default", "w1:p1")
    const previous = useHerdrStore.getState().attentionByKey
    expect(previous.get(herdrAttentionKey("default", "w1:p1"))?.seen).toBe(true)
    useHerdrStore.setState({ selectedSessionName: "work" })
    statusEvent("default", "done")
    expect(useHerdrStore.getState().attentionItems("default")).toMatchObject([{ kind: "done", seen: false }])
    expect(newHerdrAttention(previous, useHerdrStore.getState().attentionByKey)).toMatchObject([{ kind: "done", seen: false }])
    useHerdrStore.getState().applySnapshot("default", statusSnapshot("default", "done"))
    expect(useHerdrStore.getState().attentionItems("default")).toMatchObject([{ kind: "done", seen: false }])
  })

  it.each([false, true])("preserves duplicate completion seen=%s across events and snapshots", seen => {
    useHerdrStore.getState().setEventsHealth("default", true, "sub-default")
    statusEvent("default", "done")
    if (seen) useHerdrStore.getState().markAttentionSeen("default", "w1:p1")
    const previous = useHerdrStore.getState().attentionByKey
    statusEvent("default", "done")
    useHerdrStore.getState().applySnapshot("default", statusSnapshot("default", "done"))
    expect(useHerdrStore.getState().attentionByKey.get(herdrAttentionKey("default", "w1:p1"))?.seen).toBe(seen)
    expect(useHerdrStore.getState().attentionItems("default")).toHaveLength(seen ? 0 : 1)
    expect(newHerdrAttention(previous, useHerdrStore.getState().attentionByKey)).toEqual([])
  })

  it("treats a completion after working as a new turn", () => {
    useHerdrStore.getState().setEventsHealth("default", true, "sub-default")
    statusEvent("default", "done")
    useHerdrStore.getState().markAttentionSeen("default", "w1:p1")
    statusEvent("default", "working")
    expect(useHerdrStore.getState().attentionItems("default")).toEqual([])
    const previous = useHerdrStore.getState().attentionByKey
    statusEvent("default", "done")
    expect(useHerdrStore.getState().attentionItems("default")).toMatchObject([{ kind: "done", seen: false }])
    expect(newHerdrAttention(previous, useHerdrStore.getState().attentionByKey)).toHaveLength(1)
  })

  it("does not transfer read completion state across Sessions with identical pane IDs", () => {
    for (const session of ["default", "work"]) {
      useHerdrStore.getState().setEventsHealth(session, true, `sub-${session}`)
      statusEvent(session, "blocked")
      useHerdrStore.getState().markAttentionSeen(session, "w1:p1")
    }
    const workBefore = useHerdrStore.getState().attentionByKey.get(herdrAttentionKey("work", "w1:p1"))
    statusEvent("default", "done")
    expect(useHerdrStore.getState().attentionItems("default")).toMatchObject([{ kind: "done", seen: false }])
    expect(useHerdrStore.getState().attentionByKey.get(herdrAttentionKey("work", "w1:p1"))).toBe(workBefore)
    useHerdrStore.getState().markAttentionSeen("default", "w1:p1")
    statusEvent("work", "done")
    expect(useHerdrStore.getState().attentionItems("default")).toEqual([])
    expect(useHerdrStore.getState().attentionItems("work")).toMatchObject([{ kind: "done", seen: false }])
    const current = useHerdrStore.getState().attentionByKey
    useHerdrStore.getState().applySubscriptionEvent("work", {
      type: "agent_status_changed", subscriptionId: "sub-default", paneId: "w1:p1",
      workspaceId: "w1", agentStatus: "working", stateLabels: {}
    })
    expect(useHerdrStore.getState().attentionByKey).toBe(current)
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
})
