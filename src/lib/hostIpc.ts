import { invoke } from "./ipc"
import type { ConnectionOwner } from "./runtimeIdentity"
import type { HerdrRuntimeSelection, RuntimeBinaryCheck } from "./herdrTypes"

export type HostTarget = { kind: "local" } | { kind: "wsl"; distro: string } | { kind: "ssh"; sessionId: string }
export interface ConnectedHost {
  owner: ConnectionOwner
  hello: { protocol: number; version: string; os: string; arch: string; home: string; methods: string[] }
}
export interface PreparedHost { connection: ConnectedHost; binary: string; helper: string; artifactIdentity: string }
export interface HostRuntimeCheck {
  binary: string
  installedBinary: string | null
  managedVersion: string
  managedProtocol: number
  artifactIdentity: string
  requiresInstall: boolean
  check: RuntimeBinaryCheck | null
}
export interface WslDistribution { hostId: string; name: string; version: number }
export function prepareHost(hostId: string, target: HostTarget, selection: HerdrRuntimeSelection = { source: "default" }): Promise<PreparedHost> {
  return invoke("host_prepare", { hostId, target, source: selection.source, customPath: selection.customPath ?? null })
}
export function checkHostRuntime(hostId: string, target: HostTarget, selection: HerdrRuntimeSelection): Promise<HostRuntimeCheck> {
  return invoke("host_runtime_check", { hostId, target, source: selection.source, customPath: selection.customPath ?? null })
}
export function wslDistributions(): Promise<WslDistribution[]> { return invoke("host_wsl_distributions") }
export function wslPath(hostId: string, distro: string, path: string): Promise<string> { return invoke("host_wsl_path", { hostId, distro, path }) }

export type HostOperation =
  | { method: "hello" }
  | { method: "workspaceAuthorize"; params: { workspace: string } }
  | { method: "trust"; params: { call:
      | { action: "status"; workspace: string }
      | { action: "list" }
      | { action: "grant"; challenge: string }
      | { action: "revoke"; path: string }
    } }
  | { method: "git"; params: { workspace: string; repository_root: string | null; call: { command: string; args?: Record<string, unknown> } } }
  | { method: "workspaceOpen"; params: { path: string } }
  | { method: "workspaceClose"; params: { workspace: string } }
  | { method: "filesList" | "filesRead"; params: { workspace: string; path: string } }
  | { method: "filesWrite"; params: { workspace: string; path: string; content: string; revision: string } }
  | { method: "filesCreate"; params: { workspace: string; path: string; directory: boolean } }
  | { method: "filesRename"; params: { workspace: string; from: string; to: string } }
  | { method: "filesDelete"; params: { workspace: string; path: string } }
  | { method: "filesReadBase64"; params: { workspace: string; path: string; max_bytes: number } }
  | { method: "herdrDiscover"; params: { binary: string } }
  | { method: "herdrRequest"; params: { socket: string; request: unknown } }

export function connectHost(hostId: string, target: HostTarget, helper: string): Promise<ConnectedHost> {
  return invoke("host_connect", { hostId, target, helper })
}
export function requestHost<T>(owner: ConnectionOwner, operation: HostOperation): Promise<T> {
  return invoke("host_request", { owner, operation })
}
export function disconnectHost(owner: ConnectionOwner): Promise<void> {
  return invoke("host_disconnect", { owner })
}
