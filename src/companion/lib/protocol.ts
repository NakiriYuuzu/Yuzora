// Companion WS v1 shapes (src-tauri/companion/READ_PROTOCOL.md).

export type Scope = "view" | "control" | "full"
export type AgentStatus = "idle" | "working" | "blocked" | "done" | "unknown"

export type SessionInfo = { name: string; default: boolean }
export type Snapshot = {
  session: string
  /**
   * `git`: the desktop shares this Space's repository read-only. `gitGeneration`
   * (only with `git`) is an opaque token that changes whenever the desktop maps the Space to another folder.
   */
  workspaces: { id: string | null; label: string | null; status: AgentStatus; git: boolean; gitGeneration?: string }[]
  tabs: { id: string | null; workspaceId: string | null; label: string | null }[]
  panes: { id: string | null; tabId: string | null; workspaceId: string | null; status: AgentStatus; label: string | null; layout: { cols: number; rows: number } | null }[]
  agents: { paneId: string | null; name: string | null; agent: string | null; status: AgentStatus }[]
}
/** `capabilities.limits.streams`: the streams one connection may hold, event subscriptions and terminals together. */
export type Hello = { protocolVersion: number; serverTime: string; scopes: Scope[]; push: { vapidPublicKey: string } | null; capabilities?: { limits: { streams: number } } }
export type StreamOpen = { stream: number; cols: number; rows: number }
export type ControlOpen = StreamOpen & { until: string; target: string }

export type ServerEvent =
  | { event: "agent.status"; data: { session: string; pane: string; workspaceId: string | null; status: AgentStatus } }
  | { event: "pane.exited"; data: { session: string; pane: string; workspaceId: string | null } }
  | { event: "stream.closed"; data: { stream: number; reason: string } }

// Git reads (-U3 diffs capped at 256 KiB, never whole files) are view-scope only.
export type GitFile = { path: string; origPath: string | null; status: string }
export type GitStatus = {
  repo: { name: string }
  status: {
    branch: string | null
    headOid: string | null
    detached: boolean
    upstream: string | null
    ahead: number
    behind: number
    staged: GitFile[]
    unstaged: GitFile[]
    untracked: string[]
    conflicted: GitFile[]
    inProgress: string | null
  }
  /** Some list was capped at 1000 entries. */
  truncated: boolean
}
export type GitBranch = { name: string; upstream: string | null; ahead: number; behind: number; isCurrent: boolean; gone: boolean }
export type GitBranches = { local: GitBranch[]; remote: string[]; tags: { name: string; date: string }[]; truncated: boolean }
export type GitRef = { name: string; kind: "head" | "local" | "remote" | "tag" }
export type GitLogCommit = {
  hash: string
  shortHash: string
  subject: string
  authorName: string
  authorEmail: string
  /** Unix seconds. */
  timestamp: number
  parents: string[]
  refs: GitRef[]
}
export type GitLog = { commits: GitLogCommit[]; hasMore: boolean; nextCursor: string | null }
export type GitCommitFile = { status: string; path: string; oldPath: string | null; additions: number; deletions: number; binary: boolean }
export type GitCommit = {
  hash: string
  subject: string
  body: string
  authorName: string
  authorEmail: string
  timestamp: number
  parents: string[]
  files: GitCommitFile[]
  totalAdditions: number
  totalDeletions: number
  /** The file list was capped at 1000 entries. */
  truncated: boolean
}
export type GitDiff = { text: string; truncated: boolean; binary: boolean }
/**
 * What a diff screen compares: the working tree, the index, or one commit against its first parent.
 * `conflicted` is for the screen only (the file was opened from the Conflicted group); `loadDiff` never sends it.
 */
export type DiffSource =
  | { kind: "worktree"; path: string; untracked: boolean; conflicted?: boolean }
  | { kind: "staged"; path: string; origPath: string | null }
  | { kind: "commit"; hash: string; path: string; origPath: string | null }

export const READ_METHODS = new Set([
  "sessions.list", "snapshot.get", "events.subscribe", "terminal.observe",
  "git.status", "git.branches", "git.log", "git.commit", "git.diff", "push.status",
])

/**
 * Calls that hold one of the gateway's two per-peer pending slots. `push.subscribe`
 * waits there for the desktop to store the subscription; it is not a read, so a
 * lost answer stays `outcome-unknown`.
 */
export const SLOT_METHODS = new Set([...READ_METHODS, "push.subscribe"])

/** Keys `pane.sendKeys` accepts. */
export const AGENT_KEYS = ["Enter", "Escape", "Tab", "BackTab", "ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "Ctrl-C", "Ctrl-D"] as const
export type AgentKey = (typeof AGENT_KEYS)[number]
