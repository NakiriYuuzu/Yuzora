import { Channel } from "@tauri-apps/api/core"
import { emit } from "@tauri-apps/api/event"
import { invoke } from "./ipc"
import { runtimeWorkspaceService } from "./remoteFiles"
import { consumeRemoteExecutionChallenge, requestWorkspace } from "./remoteTrust"
import { sameConnection, type ConnectionOwner } from "./runtimeIdentity"
import type { DevServerDetect, DevServerInfo } from "./types"

interface Running { service: ReturnType<typeof runtimeWorkspaceService>; streamId?: string; closed: boolean; info?: DevServerInfo; terminal: boolean }
const running = new Map<string, Running>()
const close = async (server: Running) => {
  server.closed = true
  if (server.streamId) await invoke("host_stream_close", { owner: server.service.owner, streamId: server.streamId }).catch(() => {})
}
export async function detectRemoteDevServer(workspace: string, extraPorts?: number[]): Promise<DevServerDetect> {
  const service = runtimeWorkspaceService(workspace)
  return requestWorkspace(service, { method: "devServerDetect", params: { workspace: service.capabilityId, extra_ports: extraPorts ?? null } })
}
export async function stopRemoteDevServer(workspace: string): Promise<void> {
  const server = running.get(workspace)
  if (!server) return
  running.delete(workspace)
  await close(server)
}
export async function startRemoteDevServer(workspace: string, command: string, port: number | null, onOutput: (text: string) => void, challengeId: string): Promise<DevServerInfo> {
  const previous = running.get(workspace)
  if (previous) {
    try { previous.service.assertCurrent() } catch {
      running.delete(workspace)
      await close(previous)
    }
  }
  if (running.has(workspace) || running.size >= 32) throw new Error("Dev server is already running or host limit reached")
  const { raw, service } = consumeRemoteExecutionChallenge(challengeId, workspace)
  const server: Running = { service, closed: false, terminal: false }
  running.set(workspace, server)
  const current = () => {
    if (server.closed || running.get(workspace) !== server) return false
    try { service.assertCurrent(); return true } catch { return false }
  }
  type Message =
    | { type: "closed"; owner: ConnectionOwner; reason: string }
    | { type: "frame"; frame: { version: number; owner: ConnectionOwner; payload: { type: "devServerStatus"; info: DevServerInfo } | { type: "devServerOutput"; text: string } } }
  const channel = new Channel<Message>()
  channel.onmessage = (message) => {
    if (!sameConnection(message.type === "frame" ? message.frame.owner : message.owner, service.owner)) return
    if (server.closed || running.get(workspace) !== server) return
    if (!current()) {
      running.delete(workspace)
      void close(server)
      return
    }
    if (message.type === "closed") {
      if (!server.terminal) void emit("dev-server:status", { workspace, command, port: server.info?.port ?? port, status: { status: "failed", reason: message.reason } }).catch(() => {})
      server.closed = true
      running.delete(workspace)
      return
    }
    if (message.frame.version !== 1) return
    const payload = message.frame.payload
    if (payload.type === "devServerOutput") onOutput(payload.text)
    else if (payload.info.workspace === service.root && payload.info.command === command) {
      server.info = { ...payload.info, workspace }
      server.terminal = payload.info.status.status === "exited" || payload.info.status.status === "failed"
      void emit("dev-server:status", server.info).catch(() => {})
    }
  }
  try {
    const opened = await invoke<{ streamId: string; value: DevServerInfo }>("host_stream_open", { owner: service.owner, config: { kind: "devServer", workspace: service.capabilityId, path: service.root, command, port, challengeId: raw }, onEvent: channel })
    server.streamId = opened.streamId
    service.assertCurrent()
    if (opened.value.workspace !== service.root || opened.value.command !== command) throw new Error("Dev server response identity mismatch")
    // A short-lived command may exit before the open reply reaches the UI.
    if (server.closed && server.terminal && server.info) return server.info
    if (!current()) throw new Error("Dev server startup was superseded")
    return server.info ?? { ...opened.value, workspace }
  } catch (error) {
    if (running.get(workspace) === server) running.delete(workspace)
    await close(server)
    throw error
  }
}
