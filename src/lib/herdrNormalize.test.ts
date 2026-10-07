import { describe, expect, it } from "vitest"

import { HERDR_LIVE_SESSION_ID, normalizeHerdrSnapshot } from "./herdrNormalize"

describe("normalizeHerdrSnapshot", () => {
  it("never adopts agent or pane cwd as the workspace root", () => {
    const normalized = normalizeHerdrSnapshot({
      protocol: 20,
      version: "0.8.2",
      snapshot: {
        workspaces: [{ workspace_id: "ws" }],
        agents: [{ workspace_id: "ws", pane_id: "p", cwd: "C:/plugins/yuzora-wsl-agents" }],
        panes: [{ workspace_id: "ws", pane_id: "p", cwd: "/home/yuuzu", foreground_cwd: "/tmp" }]
      }
    })
    expect(normalized.spaces[0].path).toBeNull()
    expect(normalized.terminals[0].cwd).toBe("/home/yuuzu")
  })

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

  it("preserves representative precedence, explicit counts and first duplicate tab metadata", () => {
    const snapshot = normalizeHerdrSnapshot({
      protocol: 22, version: "0.9.3", snapshot: {
        focused_pane_id: "focused",
        tabs: [
          { tab_id: "wire", workspace_id: "ws", label: "First", pane_count: 9 },
          { tab_id: "wire", workspace_id: "ws", label: "Duplicate", pane_count: 1 },
          { tab_id: "derived", workspace_id: "ws" },
          { tab_id: "empty", workspace_id: "ws" }
        ],
        panes: [
          { tab_id: "wire", workspace_id: "ws", pane_id: "first", terminal_id: "first-terminal" },
          { tab_id: "wire", workspace_id: "ws", pane_id: "focused", terminal_id: "focused-terminal" },
          { tab_id: "wire", workspace_id: "ws", pane_id: "focused", terminal_id: "duplicate-focus" },
          { tab_id: "derived", workspace_id: "ws", pane_id: "p1", terminal_id: "t1" },
          { tab_id: "derived", workspace_id: "ws", pane_id: "p2", terminal_id: "t2" }
        ],
        agents: [
          { tab_id: "agent-only", workspace_id: "ws", pane_id: "a1", terminal_id: "at1" },
          { tab_id: "agent-only", workspace_id: "ws", pane_id: "a2", terminal_id: "at2", focused: true }
        ]
      }
    })
    expect(snapshot.tabs).toEqual([
      expect.objectContaining({ id: "wire", label: "First", paneCount: 9, terminalId: "focused-terminal" }),
      expect.objectContaining({ id: "derived", paneCount: 2, terminalId: "t1" }),
      expect.objectContaining({ id: "empty", paneCount: 0, terminalId: null }),
      expect.objectContaining({ id: "agent-only", paneCount: 1, terminalId: "at1" })
    ])
  })

  it("retains partial snapshots' terminal-only representative when no focused pane is supplied", () => {
    const snapshot = normalizeHerdrSnapshot({
      protocol: 22, version: "0.9.3", snapshot: {
        panes: [
          { tab_id: "partial", workspace_id: "ws", pane_id: "p1", terminal_id: "t1" },
          { tab_id: "partial", workspace_id: "ws", terminal_id: "terminal-only" },
          { tab_id: "partial", workspace_id: "ws", terminal_id: "second-terminal-only" }
        ]
      }
    })
    expect(snapshot.tabs).toEqual([
      expect.objectContaining({ id: "partial", paneCount: 3, paneId: null, terminalId: "terminal-only" })
    ])
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
