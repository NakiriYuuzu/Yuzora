// Companion WS v1 shapes (src-tauri/companion/READ_PROTOCOL.md).

export type Scope = "view" | "control" | "full"
export type AgentStatus = "idle" | "working" | "blocked" | "done" | "unknown"

export type SessionInfo = { name: string; default: boolean }
export type Snapshot = {
  session: string
  workspaces: { id: string | null; label: string | null; status: AgentStatus }[]
  tabs: { id: string | null; workspaceId: string | null; label: string | null }[]
  panes: { id: string | null; tabId: string | null; workspaceId: string | null; status: AgentStatus; label: string | null; layout: { cols: number; rows: number } | null }[]
  agents: { paneId: string | null; name: string | null; agent: string | null; status: AgentStatus }[]
}
export type Hello = { protocolVersion: number; serverTime: string; scopes: Scope[] }
export type StreamOpen = { stream: number; cols: number; rows: number }
export type ControlOpen = StreamOpen & { until: string; target: string }

export type ServerEvent =
  | { event: "agent.status"; data: { session: string; pane: string; workspaceId: string | null; status: AgentStatus } }
  | { event: "pane.exited"; data: { session: string; pane: string; workspaceId: string | null } }
  | { event: "stream.closed"; data: { stream: number; reason: string } }

export const READ_METHODS = new Set(["sessions.list", "snapshot.get", "events.subscribe", "terminal.observe"])

/** Keys `pane.sendKeys` accepts. */
export const AGENT_KEYS = ["Enter", "Escape", "Tab", "BackTab", "ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "Ctrl-C", "Ctrl-D"] as const
export type AgentKey = (typeof AGENT_KEYS)[number]
