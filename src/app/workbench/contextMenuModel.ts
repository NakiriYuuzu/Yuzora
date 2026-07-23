export interface GeneralContextMenuRequest {
  kind: "general"
}

export interface RailContextMenuRequest {
  kind: "rail"
}

export interface RecentWorkspaceContextMenuRequest {
  kind: "recentWorkspace"
  path: string
}

export interface ExplorerContextMenuRequest {
  kind: "explorer"
  workspacePath: string | null
}

export interface FileContextMenuRequest {
  kind: "file"
  workspacePath: string
  path: string
  isDirectory: boolean
  sourceGroupIndex: number
}

export interface TabContextMenuRequest {
  kind: "tab"
  workspacePath: string | null
  path: string
  groupIndex: number
}

export interface EditorContextMenuRequest {
  kind: "editor"
  workspacePath: string | null
  path: string
  groupIndex: number
}

export interface TerminalTabContextMenuRequest {
  kind: "terminalTab"
  workspacePath: string
  sessionId: string
}

export interface AgentSessionContextMenuRequest {
  kind: "agentSession"
  sessionId: string
}

export interface GitContextMenuRequest {
  kind: "git"
  repositoryRoot: string | null
}

export interface GitChangeTarget {
  path: string
  staged: boolean
  classification: "tracked" | "added" | "untracked" | "conflicted"
  stagedStatus: string | null
  unstagedStatus: string | null
  origPath: string | null
}

export interface GitChangeContextMenuRequest {
  kind: "gitChange"
  repositoryRoot: string
  clicked: GitChangeTarget
  selected: readonly GitChangeTarget[]
}

export interface StatusContextMenuRequest {
  kind: "status"
  repositoryRoot: string | null
}

export interface SshHostContextMenuRequest {
  kind: "sshhost"
  hostId: string
  address: string
}

export interface DbConnectionContextMenuRequest {
  kind: "dbconn"
  descriptorId: string
  address: string
}

export interface PreviewContextMenuRequest {
  kind: "preview"
  workspacePath: string
  url: string | null
  serverAttempt: number
}

export type ContextMenuRequest =
  | GeneralContextMenuRequest
  | RailContextMenuRequest
  | RecentWorkspaceContextMenuRequest
  | ExplorerContextMenuRequest
  | FileContextMenuRequest
  | TabContextMenuRequest
  | EditorContextMenuRequest
  | TerminalTabContextMenuRequest
  | AgentSessionContextMenuRequest
  | GitContextMenuRequest
  | GitChangeContextMenuRequest
  | StatusContextMenuRequest
  | SshHostContextMenuRequest
  | DbConnectionContextMenuRequest
  | PreviewContextMenuRequest

export type ContextMenuKind = ContextMenuRequest["kind"]
export type ContextMenuRequestFor<K extends ContextMenuKind> = Extract<ContextMenuRequest, { kind: K }>

export interface ContextMenuAvailability {
  visible: boolean
  enabled: boolean
  disabledReasonKey?: string
}

export type ContextMenuCommandOutcome = "completed" | "cancelled"
export type ContextMenuRunOutcome = ContextMenuCommandOutcome | "error"

export interface ContextMenuCommandDefinition {
  id: string
  label: (request: ContextMenuRequest) => string
  availability: (request: ContextMenuRequest) => ContextMenuAvailability
  danger: boolean
  executor: (request: ContextMenuRequest) => ContextMenuCommandOutcome | Promise<ContextMenuCommandOutcome>
}

export type ContextMenuRegistry = {
  [K in ContextMenuKind]: readonly (ContextMenuCommandDefinition | "separator")[]
}

export const CONTEXT_MENU_COMPLETED: ContextMenuCommandOutcome = "completed"
export const CONTEXT_MENU_CANCELLED: ContextMenuCommandOutcome = "cancelled"
