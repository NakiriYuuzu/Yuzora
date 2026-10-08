// HERDR machines contract (Rust: src-tauri/src/herdr_machines.rs). Kept apart from
// herdrTypes.ts: machines are always local-manager commands, never runtime-scope routed.
export type HerdrMachineStatusKind = "reachable" | "auth-required" | "error" | "disabled"
export type HerdrMachineAgentStatus = "idle" | "working" | "blocked" | "done" | "unknown"

export interface HerdrMachinesCapabilities {
  binaryPath: string
  version: string | null
  supported: boolean
  hasStatus: boolean
  hasReconnect: boolean
  source: "default" | "global" | "custom"
  reason: string | null
}

export interface HerdrMachine {
  id: string
  label: string
  target: string
  session: string
  enabled: boolean
  selected: boolean
}

export interface HerdrMachineStatus {
  id: string
  label: string
  status: HerdrMachineStatusKind
  error: string | null
}

export interface HerdrMachineWorkspace {
  workspaceId: string
  label: string | null
  repoName: string | null
  checkoutPath: string | null
  agentStatus: HerdrMachineAgentStatus
}

export interface HerdrMachineAgent {
  terminalId: string
  paneId: string
  tabId: string
  workspaceId: string
  workspaceLabel: string | null
  agent: string | null
  name: string | null
  title: string | null
  cwd: string | null
  folder: string | null
  status: HerdrMachineAgentStatus
  focused: boolean
}

export interface HerdrMachineSnapshot {
  machineId: string
  fetchedAt: number
  serverVersion: string | null
  workspaces: HerdrMachineWorkspace[]
  agents: HerdrMachineAgent[]
}

export type HerdrMachineInteractiveSpec =
  | { kind: "add"; target: string; remoteSession?: string; label?: string }
  | { kind: "reconnect"; machineId: string }
  | { kind: "client" }

export interface HerdrMachineInteractiveOpened {
  sessionId: string
}
