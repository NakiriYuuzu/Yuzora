import { describe, expect, it } from "vitest"

import { HERDR_LIVE_SESSION_ID, normalizeHerdrSnapshot } from "./herdrNormalize"

describe("normalizeHerdrSnapshot", () => {
  it("maps workspaces/agents/panes and ignores unknown fields", () => {
    const normalized = normalizeHerdrSnapshot({
      protocol: 19,
      version: "0.8.0",
      snapshot: {
        version: "0.8.0",
        protocol: 19,
        focused_workspace_id: "ws_1",
        focused_tab_id: "tab_1",
        focused_pane_id: "pane_1",
        workspaces: [
          {
            workspace_id: "ws_1",
            number: 1,
            label: "Yuzora",
            focused: true,
            pane_count: 1,
            tab_count: 1,
            active_tab_id: "tab_1",
            agent_status: "working",
            worktree: {
              checkout_path: "/Users/me/yuzora",
              is_linked_worktree: false,
              repo_key: "k",
              repo_name: "yuzora",
              repo_root: "/Users/me/yuzora"
            },
            future_field: { nested: true }
          }
        ],
        tabs: [
          {
            tab_id: "tab_1",
            workspace_id: "ws_1",
            number: 1,
            label: "Agent",
            focused: true,
            pane_count: 1,
            agent_status: "working"
          }
        ],
        panes: [
          {
            pane_id: "pane_1",
            terminal_id: "term_1",
            workspace_id: "ws_1",
            tab_id: "tab_1",
            focused: true,
            agent_status: "working",
            revision: 3,
            title: "Implementer",
            cwd: "/Users/me/yuzora",
            unknown_pane_meta: 1
          }
        ],
        layouts: [],
        agents: [
          {
            terminal_id: "term_1",
            agent_status: "working",
            workspace_id: "ws_1",
            tab_id: "tab_1",
            pane_id: "pane_1",
            focused: true,
            revision: 3,
            display_agent: "pi",
            title: "Implementer",
            mystery: "ok"
          }
        ]
      }
    })

    expect(normalized.herdrSessionId).toBe(HERDR_LIVE_SESSION_ID)
    expect(normalized.agents[0]?.spaceLabel).toBe("Yuzora")
    expect(normalized.agents[0]?.sessionName).toBe(HERDR_LIVE_SESSION_ID)
    expect(normalized.protocol).toBe(19)
    expect(normalized.spaces).toEqual([
      expect.objectContaining({
        id: "ws_1",
        label: "Yuzora",
        focused: true,
        path: "/Users/me/yuzora",
        status: "working",
        agentCount: 1,
        tabCount: 1,
        repoKey: "k",
        repoName: "yuzora",
        repoRoot: "/Users/me/yuzora",
        isLinkedWorktree: false
      })
    ])
    expect(normalized.agents[0]).toEqual(
      expect.objectContaining({
        id: "term_1",
        name: "pi",
        terminalId: "term_1",
        paneId: "pane_1",
        workspaceId: "ws_1",
        status: "working",
        title: "Implementer"
      })
    )
    expect(normalized.terminals[0]).toEqual(
      expect.objectContaining({
        terminalId: "term_1",
        paneId: "pane_1",
        title: "Implementer"
      })
    )
    expect(normalized.tabs).toEqual([
      expect.objectContaining({
        id: "tab_1",
        label: "Agent",
        workspaceId: "ws_1",
        paneCount: 1,
        active: true,
        focused: true,
        paneId: "pane_1",
        terminalId: "term_1",
        sessionName: HERDR_LIVE_SESSION_ID
      })
    ])
    expect(normalized.focusedTerminalId).toBe("term_1")
  })

  it("normalizes only truthful WSL execution origins without changing Agent identity", () => {
    const overlong = "x".repeat(129)
    const normalized = normalizeHerdrSnapshot({
      protocol: 19,
      version: "0.8.2",
      snapshot: {
        agents: [
          { terminal_id: "native", pane_id: "p-native", workspace_id: "w", agent_status: "idle", execution_origin: { kind: "native" } },
          { terminal_id: "absent", pane_id: "p-absent", workspace_id: "w", agent_status: "idle" },
          { terminal_id: "valid", pane_id: "p-valid", workspace_id: "w", agent_status: "idle", execution_origin: { kind: "wsl", distribution: "  Ubuntu  " } },
          { terminal_id: "missing", pane_id: "p-missing", workspace_id: "w", agent_status: "idle", execution_origin: { kind: "wsl" } },
          { terminal_id: "hostile", pane_id: "p-hostile", workspace_id: "w", agent_status: "idle", execution_origin: { kind: "wsl", distribution: "Ubuntu\nspoof" } },
          { terminal_id: "overlong", pane_id: "p-overlong", workspace_id: "w", agent_status: "idle", execution_origin: { kind: "wsl", distribution: overlong } },
          { terminal_id: "pane-origin", pane_id: "p-pane", workspace_id: "w", agent_status: "idle" }
        ],
        panes: [
          { terminal_id: "pane-term", pane_id: "p-pane", workspace_id: "w", agent_status: "idle", execution_origin: { kind: "wsl", distribution: "Debian" } }
        ]
      }
    })
    const byId = new Map(normalized.agents.map((agent) => [agent.id, agent]))

    expect(byId.get("native")?.executionOrigin).toBeUndefined()
    expect(byId.get("absent")?.executionOrigin).toBeUndefined()
    expect(byId.get("valid")).toMatchObject({
      id: "valid",
      paneId: "p-valid",
      workspaceId: "w",
      executionOrigin: { kind: "wsl", distribution: "Ubuntu" }
    })
    expect(byId.get("missing")?.executionOrigin).toEqual({ kind: "wsl" })
    expect(byId.get("hostile")?.executionOrigin).toEqual({ kind: "wsl" })
    expect(byId.get("overlong")?.executionOrigin).toEqual({ kind: "wsl" })
    expect(byId.get("pane-origin")?.executionOrigin).toEqual({
      kind: "wsl",
      distribution: "Debian"
    })
    expect(normalized.terminals[0]?.executionOrigin).toEqual({
      kind: "wsl",
      distribution: "Debian"
    })
  })

  it("uses pane WSL origin only when the Agent origin is absent", () => {
    const normalized = normalizeHerdrSnapshot({
      protocol: 19,
      version: "0.8.2",
      snapshot: {
        agents: [
          { terminal_id: "absent", pane_id: "p-absent", workspace_id: "w", agent_status: "idle" },
          { terminal_id: "native", pane_id: "p-native", workspace_id: "w", agent_status: "idle", execution_origin: { kind: "native" } },
          { terminal_id: "unknown", pane_id: "p-unknown", workspace_id: "w", agent_status: "idle", execution_origin: { kind: "other" } },
          { terminal_id: "malformed", pane_id: "p-malformed", workspace_id: "w", agent_status: "idle", execution_origin: "wsl" }
        ],
        panes: [
          { terminal_id: "absent-pane", pane_id: "p-absent", workspace_id: "w", agent_status: "idle", execution_origin: { kind: "wsl", distribution: "Ubuntu" } },
          { terminal_id: "native-pane", pane_id: "p-native", workspace_id: "w", agent_status: "idle", execution_origin: { kind: "wsl", distribution: "Debian" } },
          { terminal_id: "unknown-pane", pane_id: "p-unknown", workspace_id: "w", agent_status: "idle", execution_origin: { kind: "wsl", distribution: "Fedora" } },
          { terminal_id: "malformed-pane", pane_id: "p-malformed", workspace_id: "w", agent_status: "idle", execution_origin: { kind: "wsl", distribution: "Alpine" } }
        ]
      }
    })
    const byId = new Map(normalized.agents.map((agent) => [agent.id, agent]))

    expect(byId.get("absent")?.executionOrigin).toEqual({
      kind: "wsl",
      distribution: "Ubuntu"
    })
    expect(byId.get("native")?.executionOrigin).toBeUndefined()
    expect(byId.get("unknown")?.executionOrigin).toBeUndefined()
    expect(byId.get("malformed")?.executionOrigin).toBeUndefined()
  })

  it("tolerates empty or malformed payload", () => {
    const empty = normalizeHerdrSnapshot({
      protocol: 19,
      version: "0.8.0",
      snapshot: null
    })
    expect(empty.spaces).toEqual([])
    expect(empty.agents).toEqual([])
    expect(empty.tabs).toEqual([])
    expect(empty.terminals).toEqual([])

    const partial = normalizeHerdrSnapshot({
      protocol: 19,
      version: "0.8.0",
      snapshot: {
        workspaces: [{ label: "missing id" }, { workspace_id: "ws_x", number: 0, label: "X" }],
        agents: [{ pane_id: "p", workspace_id: "ws_x", agent_status: "idle" }],
        panes: "nope"
      }
    })
    expect(partial.spaces.map((s) => s.id)).toEqual(["ws_x"])
    expect(partial.agents[0]?.id).toBe("p")
    expect(partial.tabs).toEqual([])
    expect(partial.terminals).toEqual([])
  })
})
