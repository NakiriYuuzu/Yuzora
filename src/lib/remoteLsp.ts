import { Channel } from "@tauri-apps/api/core"
import { emit } from "@tauri-apps/api/event"
import { invoke } from "./ipc"
import { runtimeWorkspaceService } from "./remoteFiles"
import { requestWorkspace } from "./remoteTrust"
import { parseRemoteFilePath, sameConnection, type ConnectionOwner } from "./runtimeIdentity"
import { mapRemoteLspMessage } from "@/lsp/remoteUris"
import type { LspServerInfo } from "./types"

interface Server {
  service: ReturnType<typeof runtimeWorkspaceService>
  workspace: string
  language: string
  streamId?: string
  info?: LspServerInfo
  closed: boolean
  opening: Promise<LspServerInfo>
}
const servers = new Map<string, Server>()
const tracing = new Map<string, boolean>()
export function remoteLspTraceEnabled(context: string): boolean {
  return tracing.get(parseRemoteFilePath(context)!.hostId) ?? false
}
export async function setRemoteLspTrace(context: string, enabled: boolean): Promise<void> {
  const service = runtimeWorkspaceService(context)
  for (const server of servers.values()) {
    if (server.service.owner.hostId !== service.owner.hostId || !current(server)) continue
    await server.opening
    if (!current(server)) continue
    await invoke("host_stream_command", { owner: server.service.owner, streamId: server.streamId, operation: { command: "lspTrace", enabled } })
  }
  service.assertCurrent()
  tracing.set(service.owner.hostId, enabled)
}
const keyOf = (workspace: string, language: string) => JSON.stringify([workspace, language])

function current(server: Server): boolean {
  if (server.closed || servers.get(keyOf(server.workspace, server.language)) !== server) return false
  try { server.service.assertCurrent(); return true } catch { return false }
}
async function close(server: Server): Promise<void> {
  server.closed = true
  if (server.streamId) await invoke("host_stream_close", { owner: server.service.owner, streamId: server.streamId }).catch(() => undefined)
}
function fail(server: Server, reason: string): void {
  if (!current(server)) return
  const key = keyOf(server.workspace, server.language)
  void close(server)
  // Stop the frontend client too: closing only the helper leaves diagnostics,
  // formatting and initialized state attached to an unusable connection.
  void import("@/lsp/lspManager").then((lsp) => {
    if (servers.get(key) !== server) return
    lsp.stopWorkspace(server.workspace)
    if (server.info) void emit("lsp:server-status", { ...server.info, status: { status: "crashed", reason } }).catch(() => {})
  })
}
export async function stopRemoteLsp(workspace: string): Promise<void> {
  const pending: Promise<void>[] = []
  for (const [key, server] of servers) if (server.workspace === workspace) {
    servers.delete(key)
    pending.push(close(server))
  }
  await Promise.all(pending)
}
export async function restartConfiguredRemoteLsp(context: string, global: boolean, language: string): Promise<void> {
  const service = runtimeWorkspaceService(context)
  const hostId = parseRemoteFilePath(context)!.hostId
  const workspaces = new Set([context])
  if (global) for (const server of servers.values()) {
    if (server.language === language && server.service.owner.hostId === hostId) workspaces.add(server.workspace)
  }
  const manager = await import("@/lsp/lspManager")
  for (const workspace of workspaces) {
    await manager.restartWorkspace(workspace, () => { try { service.assertCurrent(); return true } catch { return false } })
  }
}
export function remoteLspStatus(workspace: string): LspServerInfo[] {
  return [...servers.values()].filter((server) => server.workspace === workspace && current(server)).flatMap((server) => server.info ? [server.info] : [])
}
export async function detectRemoteLsp(workspace: string, language: string): Promise<LspServerInfo> {
  const service = runtimeWorkspaceService(workspace)
  const info = await requestWorkspace<LspServerInfo>(service, { method: "lspDetect", params: { workspace: service.capabilityId, language } })
  return { ...info, workspace }
}

export function startRemoteLsp(workspace: string, language: string, onMessage: (message: string) => void): Promise<LspServerInfo> {
  const key = keyOf(workspace, language)
  const existing = servers.get(key)
  if (existing && current(existing)) return existing.opening
  if (existing) void close(existing)
  if (!existing && servers.size >= 128) return Promise.reject(new Error("LSP server limit exceeded"))
  const service = runtimeWorkspaceService(workspace)
  const server = { service, workspace, language, closed: false } as Server
  servers.set(key, server)
  server.opening = (async () => {
    await requestWorkspace(service, { method: "workspaceAuthorize", params: { workspace: service.capabilityId } })
    if (!current(server)) throw new Error("LSP startup was superseded")
    type Message =
      | { type: "frame"; frame: { version: number; owner: ConnectionOwner; payload: { type: string; message?: string; info?: LspServerInfo } } }
      | { type: "closed"; owner: ConnectionOwner; reason: string }
    const channel = new Channel<Message>()
    channel.onmessage = (message) => {
      if (!current(server)) return
      const owner = message.type === "frame" ? message.frame.owner : message.owner
      if (!sameConnection(owner, service.owner)) return
      if (message.type === "closed") {
        fail(server, message.reason)
        return
      }
      if (message.frame.version !== 1) return
      const payload = message.frame.payload
      if (payload.type === "lsp" && payload.message) {
        try { onMessage(mapRemoteLspMessage(payload.message, workspace, "fromHost")) }
        catch (error) { fail(server, String(error)) }
      }
      if (payload.type === "lspStatus" && payload.info) {
        server.info = { ...payload.info, workspace }
        void emit("lsp:server-status", server.info).catch(() => {})
      }
    }
    let opened: { streamId: string; value: LspServerInfo }
    try {
      opened = await invoke("host_stream_open", { owner: service.owner, config: { kind: "lsp", path: service.root, language }, onEvent: channel })
    } catch (error) {
      // Reissue any raced trust failure through the primary helper's registry.
      await requestWorkspace(service, { method: "workspaceAuthorize", params: { workspace: service.capabilityId } })
      throw error
    }
    server.streamId = opened.streamId
    if (!current(server)) { await close(server); throw new Error("LSP startup was superseded") }
    server.info = { ...opened.value, workspace }
    if (server.info.status.status === "starting" && remoteLspTraceEnabled(workspace)) {
      await invoke("host_stream_command", { owner: service.owner, streamId: server.streamId, operation: { command: "lspTrace", enabled: true } })
      if (!current(server)) throw new Error("LSP startup was superseded")
    }
    if (server.info.status.status === "missing" || server.info.status.status === "crashed" || server.info.status.status === "stopped") {
      if (servers.get(key) === server) servers.delete(key)
      await close(server)
    }
    return server.info
  })().catch((error) => {
    if (servers.get(key) === server) servers.delete(key)
    void close(server)
    throw error
  })
  return server.opening
}

export async function sendRemoteLsp(workspace: string, language: string, message: string): Promise<void> {
  const server = servers.get(keyOf(workspace, language))
  if (!server || !current(server)) throw new Error("LSP connection is unavailable")
  await server.opening
  if (!current(server)) throw new Error("LSP connection changed")
  const body = mapRemoteLspMessage(message, workspace, "toHost")
  await invoke("host_stream_command", { owner: server.service.owner, streamId: server.streamId, operation: { command: "lspMessage", message: body } })
  if (!current(server)) throw new Error("LSP response belongs to a previous connection")
}
