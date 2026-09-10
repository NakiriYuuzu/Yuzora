import { beforeEach, describe, expect, it } from "vitest"

import { normalizeHerdrSnapshot } from "@/lib/herdrNormalize"
import { herdrAttentionKey, herdrInitialState, useHerdrStore } from "./herdrStore"

const rawCustomPi = {
  protocol: 20,
  version: "0.8.2",
  snapshot: {
    version: "0.8.2",
    protocol: 20,
    focused_workspace_id: "w1",
    focused_tab_id: "w1:t1",
    focused_pane_id: "w1:p1",
    workspaces: [
      {
        workspace_id: "w1",
        number: 1,
        label: "Yuzora",
        focused: true,
        pane_count: 1,
        tab_count: 1,
        active_tab_id: "w1:t1",
        agent_status: "unknown"
      }
    ],
    tabs: [
      {
        tab_id: "w1:t1",
        workspace_id: "w1",
        number: 1,
        label: "WSL Pi",
        focused: true,
        pane_count: 1,
        agent_status: "unknown"
      }
    ],
    panes: [
      {
        pane_id: "w1:p1",
        terminal_id: "term-wsl-pi",
        workspace_id: "w1",
        tab_id: "w1:t1",
        focused: true,
        agent_status: "unknown",
        title: "pi"
      }
    ],
    agents: [
      {
        terminal_id: "term-wsl-pi",
        agent_status: "unknown",
        workspace_id: "w1",
        tab_id: "w1:t1",
        pane_id: "w1:p1",
        focused: true,
        display_agent: "pi",
        agent: "pi",
        title: "pi",
        source: "yuzora:wsl:pi",
        agent_session: { id: "pi-sess-secret" },
        tokens: { "yuzora:wsl:pi": { session: "pi-sess-secret" } },
        execution_origin: "wsl"
      }
    ]
  }
}

describe("herdrWslPluginCompatibility store projection", () => {
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
      ]
    })
  })

  it("keeps unknown Attention for custom-source Pi without session DTOs", () => {
    const snapshot = normalizeHerdrSnapshot(rawCustomPi, "default")
    useHerdrStore.getState().applySnapshot("default", snapshot)
    const agent = useHerdrStore.getState().snapshot?.agents[0]
    expect(agent?.status).toBe("unknown")
    expect(agent?.displayAgent).toBe("pi")
    expect(agent).not.toHaveProperty("agentSession")
    expect(agent).not.toHaveProperty("agent_session")
    expect(agent).not.toHaveProperty("tokens")
    expect(agent).not.toHaveProperty("execution_origin")
    const items = useHerdrStore.getState().attentionItems("default")
    expect(items).toHaveLength(1)
    expect(items[0]).toMatchObject({
      key: herdrAttentionKey("default", "w1:p1"),
      paneId: "w1:p1",
      kind: "unknown",
      agentStatus: "unknown",
      displayAgent: "pi"
    })
    expect(items[0]).not.toHaveProperty("agentSession")
    expect(items[0]).not.toHaveProperty("tokens")
  })

  it("updates live Pi state from events and drops extras that are not on the event DTO", () => {
    useHerdrStore.getState().applySubscriptionEvent("default", {
      type: "subscribed",
      subscriptionId: "sub-wsl"
    })
    useHerdrStore.getState().applySubscriptionEvent("default", {
      type: "agent_status_changed",
      subscriptionId: "sub-wsl",
      paneId: "w1:p1",
      workspaceId: "w1",
      agentStatus: "blocked",
      agent: "pi",
      displayAgent: "pi",
      title: "pi",
      stateLabels: { source: "yuzora:wsl:pi" }
    })
    const blocked = useHerdrStore.getState().attentionItems("default")[0]
    expect(blocked?.kind).toBe("blocked")
    expect(blocked?.displayAgent).toBe("pi")
    expect(blocked).not.toHaveProperty("stateLabels")
    expect(blocked).not.toHaveProperty("agentSession")

    useHerdrStore.getState().applySubscriptionEvent("default", {
      type: "agent_status_changed",
      subscriptionId: "sub-wsl",
      paneId: "w1:p1",
      workspaceId: "w1",
      agentStatus: "idle",
      displayAgent: "pi",
      title: "pi",
      stateLabels: {}
    })
    expect(useHerdrStore.getState().attentionItems("default")).toHaveLength(0)
  })

  it("clears Attention on pane exit without inventing a session identity", () => {
    useHerdrStore.getState().applySubscriptionEvent("default", {
      type: "subscribed",
      subscriptionId: "sub-wsl"
    })
    useHerdrStore.getState().applySubscriptionEvent("default", {
      type: "agent_status_changed",
      subscriptionId: "sub-wsl",
      paneId: "w1:p1",
      workspaceId: "w1",
      agentStatus: "unknown",
      displayAgent: "pi",
      title: "pi",
      stateLabels: {}
    })
    useHerdrStore.getState().applySubscriptionEvent("default", {
      type: "pane_exited",
      subscriptionId: "sub-wsl",
      paneId: "w1:p1",
      workspaceId: "w1"
    })
    expect(useHerdrStore.getState().attentionItems("default")).toHaveLength(0)
  })
})
