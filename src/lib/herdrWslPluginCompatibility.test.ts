import { describe, expect, it } from "vitest"

import { normalizeHerdrSnapshot } from "./herdrNormalize"
import type { HerdrAgentInfo } from "./herdrTypes"

const PROJECTED_AGENT_KEYS = [
  "displayAgent",
  "focused",
  "id",
  "name",
  "paneId",
  "sessionName",
  "spaceLabel",
  "status",
  "tabId",
  "terminalId",
  "title",
  "workspaceId"
] as const

const FORBIDDEN_AGENT_KEYS = [
  "agentSession",
  "agent_session",
  "agent_session_id",
  "agent_session_path",
  "executionOrigin",
  "execution_origin",
  "source",
  "tokens"
] as const

function customSourcePiSnapshot(status: string) {
  return {
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
          agent_status: status
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
          agent_status: status
        }
      ],
      panes: [
        {
          pane_id: "w1:p1",
          terminal_id: "term-wsl-pi",
          workspace_id: "w1",
          tab_id: "w1:t1",
          focused: true,
          agent_status: status,
          title: "pi",
          cwd: "/home/yuuzu/yuzora",
          source: "yuzora:wsl:pi",
          tokens: { "yuzora:wsl:pi": { session: "pi-sess-secret" } },
          agent_session: {
            id: "pi-sess-secret",
            path: "/home/yuuzu/.pi/agent/sessions/secret"
          }
        }
      ],
      agents: [
        {
          terminal_id: "term-wsl-pi",
          agent_status: status,
          workspace_id: "w1",
          tab_id: "w1:t1",
          pane_id: "w1:p1",
          focused: true,
          display_agent: "pi",
          agent: "pi",
          title: "pi",
          source: "yuzora:wsl:pi",
          tokens: { "yuzora:wsl:pi": { session: "pi-sess-secret" } },
          agent_session: {
            id: "pi-sess-secret",
            path: "/home/yuuzu/.pi/agent/sessions/secret"
          },
          execution_origin: "wsl",
          executionOrigin: "wsl",
          agent_session_id: "pi-sess-secret",
          agent_session_path: "/home/yuuzu/.pi/agent/sessions/secret"
        }
      ]
    }
  }
}

function assertThinAgentProjection(agent: HerdrAgentInfo | undefined) {
  expect(agent).toBeDefined()
  expect(Object.keys(agent!).sort()).toEqual([...PROJECTED_AGENT_KEYS].sort())
  for (const key of FORBIDDEN_AGENT_KEYS) {
    expect(agent).not.toHaveProperty(key)
  }
}

describe("herdrWslPluginCompatibility consumer projection", () => {
  it("projects live Pi identity/state and ignores session/token/origin extras", () => {
    const normalized = normalizeHerdrSnapshot(customSourcePiSnapshot("working"), "default")
    const agent = normalized.agents[0]
    expect(agent?.name).toBe("pi")
    expect(agent?.displayAgent).toBe("pi")
    expect(agent?.status).toBe("working")
    expect(agent?.paneId).toBe("w1:p1")
    expect(agent?.terminalId).toBe("term-wsl-pi")
    expect(agent?.sessionName).toBe("default")
    assertThinAgentProjection(agent)
    expect(normalized.raw).toEqual(customSourcePiSnapshot("working").snapshot)
  })

  it("keeps unknown live state without synthesizing session fields", () => {
    const normalized = normalizeHerdrSnapshot(customSourcePiSnapshot("unknown"), "work")
    expect(normalized.agents[0]?.status).toBe("unknown")
    assertThinAgentProjection(normalized.agents[0])
  })

  it("retains raw snapshot extras without promoting them onto terminals", () => {
    const normalized = normalizeHerdrSnapshot(customSourcePiSnapshot("blocked"))
    const terminal = normalized.terminals[0]
    expect(terminal?.status).toBe("blocked")
    expect(terminal).not.toHaveProperty("agent_session")
    expect(terminal).not.toHaveProperty("tokens")
    expect(terminal).not.toHaveProperty("execution_origin")
    expect(terminal).not.toHaveProperty("source")
  })
})
