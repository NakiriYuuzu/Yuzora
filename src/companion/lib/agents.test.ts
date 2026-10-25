import { describe, expect, it } from "vitest"
import { findAgent, otherSpaces, projectSpaces, withAgentStatus, withoutPane } from "./agents"
import { keyBytes } from "./keys"
import type { Snapshot } from "./protocol"
import { clockOffset, formatCountdown, secondsUntil } from "./time"

const snapshot: Snapshot = {
  session: "work",
  workspaces: [
    { id: "w1", label: "Yuzora", status: "working", git: true },
    { id: "w2", label: "Empty", status: "idle", git: false },
    { id: "w3", label: "Site", status: "blocked", git: false },
  ],
  tabs: [
    { id: "w1:t", workspaceId: "w1", label: "main" },
    { id: "w3:t", workspaceId: "w3", label: "dev" },
  ],
  panes: [
    { id: "w1:p1", tabId: "w1:t", workspaceId: "w1", status: "idle", label: null, layout: { cols: 120, rows: 40 } },
    { id: "w1:p2", tabId: "w1:t", workspaceId: "w1", status: "blocked", label: "review", layout: null },
    { id: "w1:p3", tabId: "w1:t", workspaceId: "w1", status: "working", label: null, layout: null },
    { id: "w3:p", tabId: "w3:t", workspaceId: "w3", status: "done", label: null, layout: null },
    { id: "w1:shell", tabId: "w1:t", workspaceId: "w1", status: "unknown", label: "zsh", layout: null },
  ],
  agents: [
    { paneId: "w1:p1", name: "Agent 10", agent: "claude", status: "idle" },
    { paneId: "w1:p2", name: null, agent: "codex", status: "blocked" },
    { paneId: "w1:p3", name: "Agent 9", agent: "claude", status: "working" },
    { paneId: "w3:p", name: "Site agent", agent: null, status: "done" },
    { paneId: null, name: "Orphan", agent: null, status: "blocked" },
  ],
}

describe("otherSpaces", () => {
  it("lists the Spaces projectSpaces leaves out for having no agent, with a usable id", () => {
    expect(otherSpaces(snapshot)).toEqual([{ session: "work", workspaceId: "w2", label: "Empty" }])
  })

  it("is empty when every Space has an agent, and skips a Space without an id", () => {
    const busy: Snapshot = { ...snapshot, workspaces: [snapshot.workspaces[0], { id: null, label: "Loose", status: "idle", git: false }] }
    expect(otherSpaces(busy)).toEqual([])
  })
})

describe("projectSpaces", () => {
  it("groups agents by Space in desktop order and sorts them by urgency", () => {
    const groups = projectSpaces(snapshot)
    expect(groups.map((group) => group.label)).toEqual(["Yuzora", "Site"])
    expect(groups[0].agents.map((agent) => [agent.name, agent.status])).toEqual([
      ["codex", "blocked"],
      ["Agent 9", "working"],
      ["Agent 10", "idle"],
    ])
    expect(groups[0].agents[0]).toMatchObject({ paneId: "w1:p2", kind: "codex", tabLabel: "main", paneLabel: "review", session: "work", hint: null })
  })

  it("tells apart agents with the same name", () => {
    const twins: Snapshot = { ...snapshot, agents: snapshot.agents.map((agent) => (agent.paneId === "w1:p3" || agent.paneId === "w1:p1" ? { ...agent, name: "pi" } : agent)) }
    const agents = projectSpaces(twins)[0].agents
    expect(agents.filter((agent) => agent.name === "pi").map((agent) => agent.hint).sort()).toEqual(["p1", "p3"])
    expect(agents.find((agent) => agent.paneId === "w1:p2")?.hint).toBeNull()
  })

  it("applies status events and asks for a snapshot for unknown panes", () => {
    const next = withAgentStatus(snapshot, "w1:p1", "blocked")!
    expect(findAgent(next, "w1:p1")?.status).toBe("blocked")
    expect(next.panes.find((pane) => pane.id === "w1:p1")?.status).toBe("blocked")
    expect(findAgent(snapshot, "w1:p1")?.status).toBe("idle")
    expect(withAgentStatus(snapshot, "w9:new", "working")).toBeNull()
    expect(withAgentStatus(snapshot, "w1:shell", "working")).toBeNull()
  })

  it("drops exited panes", () => {
    const next = withoutPane(snapshot, "w3:p")
    expect(projectSpaces(next).map((group) => group.label)).toEqual(["Yuzora"])
    expect(findAgent(next, "w3:p")).toBeNull()
  })
})

describe("keyBytes", () => {
  it("maps the key bar to terminal bytes, following the cursor-key mode", () => {
    expect(keyBytes("Enter", false)).toBe("\r")
    expect(keyBytes("Escape", false)).toBe("\x1b")
    expect(keyBytes("BackTab", false)).toBe("\x1b[Z")
    expect(keyBytes("Ctrl-C", true)).toBe("\x03")
    expect(keyBytes("Ctrl-D", true)).toBe("\x04")
    expect(keyBytes("ArrowUp", false)).toBe("\x1b[A")
    expect(keyBytes("ArrowUp", true)).toBe("\x1bOA")
    expect(keyBytes("ArrowLeft", true)).toBe("\x1bOD")
  })
})

describe("server time", () => {
  it("counts down against the server clock", () => {
    const offset = clockOffset("2026-10-18T10:00:00Z", Date.parse("2026-10-18T09:59:00Z"))
    expect(offset).toBe(60_000)
    expect(secondsUntil("2026-10-18T10:02:05Z", offset, Date.parse("2026-10-18T09:59:00Z"))).toBe(125)
    expect(secondsUntil("2026-10-18T09:00:00Z", offset, Date.parse("2026-10-18T09:59:00Z"))).toBe(0)
    expect(formatCountdown(125)).toBe("2:05")
    expect(clockOffset("nonsense")).toBe(0)
  })
})
