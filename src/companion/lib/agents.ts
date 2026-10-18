import type { AgentStatus, Snapshot } from "./protocol"

export type AgentRow = {
  session: string
  paneId: string
  name: string
  kind: string | null
  status: AgentStatus
  tabLabel: string | null
  paneLabel: string | null
  /** Tells apart agents that share a name in one Space. */
  hint: string | null
}
export type SpaceGroup = { session: string; workspaceId: string | null; label: string | null; status: AgentStatus; agents: AgentRow[] }

// Same order as the desktop sidebar (src/lib/herdrAgents.ts).
const URGENCY: Record<AgentStatus, number> = { blocked: 0, done: 1, working: 2, unknown: 3, idle: 4 }
const collator = new Intl.Collator(undefined, { numeric: true, sensitivity: "base" })

export function compareAgents(left: AgentRow, right: AgentRow): number {
  return URGENCY[left.status] - URGENCY[right.status] || collator.compare(left.name, right.name) || left.paneId.localeCompare(right.paneId)
}

/** Session → Space → Agents, agents by urgency, Spaces in the desktop's order. */
export function projectSpaces(snapshot: Snapshot): SpaceGroup[] {
  const groups = new Map<string | null, SpaceGroup>()
  for (const workspace of snapshot.workspaces) {
    groups.set(workspace.id, { session: snapshot.session, workspaceId: workspace.id, label: workspace.label, status: workspace.status, agents: [] })
  }
  for (const agent of snapshot.agents) {
    if (!agent.paneId) continue
    const pane = snapshot.panes.find((item) => item.id === agent.paneId)
    const tab = snapshot.tabs.find((item) => item.id === pane?.tabId)
    const workspaceId = pane?.workspaceId ?? null
    let group = groups.get(workspaceId)
    if (!group) {
      group = { session: snapshot.session, workspaceId, label: null, status: "unknown", agents: [] }
      groups.set(workspaceId, group)
    }
    group.agents.push({
      session: snapshot.session,
      paneId: agent.paneId,
      name: agent.name ?? agent.agent ?? pane?.label ?? agent.paneId,
      kind: agent.agent,
      status: agent.status,
      tabLabel: tab?.label ?? null,
      paneLabel: pane?.label ?? null,
      hint: null,
    })
  }
  return [...groups.values()].filter((group) => group.agents.length > 0).map((group) => {
    const agents = group.agents.sort(compareAgents)
    for (const agent of agents) {
      if (agents.filter((other) => other.name === agent.name).length > 1) agent.hint = agent.paneLabel ?? agent.paneId.split(":").pop() ?? agent.paneId
    }
    return { ...group, agents }
  })
}

/**
 * Applies `agent.status`. Returns null for a pane the snapshot does not know:
 * events never announce new agents, so the caller fetches a fresh snapshot.
 */
export function withAgentStatus(snapshot: Snapshot, pane: string, status: AgentStatus): Snapshot | null {
  if (!snapshot.agents.some((agent) => agent.paneId === pane)) return null
  return {
    ...snapshot,
    agents: snapshot.agents.map((agent) => (agent.paneId === pane ? { ...agent, status } : agent)),
    panes: snapshot.panes.map((item) => (item.id === pane ? { ...item, status } : item)),
  }
}

export function withoutPane(snapshot: Snapshot, pane: string): Snapshot {
  return {
    ...snapshot,
    agents: snapshot.agents.filter((agent) => agent.paneId !== pane),
    panes: snapshot.panes.filter((item) => item.id !== pane),
  }
}

export function findAgent(snapshot: Snapshot | undefined, pane: string): AgentRow | null {
  if (!snapshot) return null
  for (const group of projectSpaces(snapshot)) {
    const agent = group.agents.find((item) => item.paneId === pane)
    if (agent) return agent
  }
  return null
}
