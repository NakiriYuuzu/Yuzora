import { Channel } from "@tauri-apps/api/core"
import { invoke } from "./ipc"
import { runtimeWorkspaceService } from "./remoteFiles"
import { requestWorkspace } from "./remoteTrust"
import { sameConnection, type ConnectionOwner } from "./runtimeIdentity"
import type { LspInstallProgress, LspServerInfo } from "./types"

const pending = new Map<string, () => void>()
const keyOf = (context: string, language: string) => JSON.stringify([context, language])
export function cancelRemoteLspInstall(context: string, language: string): void {
  pending.get(keyOf(context, language))?.()
}

export async function installRemoteLsp(context: string, workspace: string | null, language: string, onProgress?: (event: LspInstallProgress) => void): Promise<LspServerInfo> {
  if (workspace && workspace !== context) throw new Error("LSP installation belongs to another workspace")
  const key = keyOf(context, language)
  if (pending.has(key) || pending.size >= 8) throw new Error("LSP installation is already running")
  const service = runtimeWorkspaceService(context)
  let streamId: string | undefined
  let done = false
  let resolve!: (info: LspServerInfo) => void
  let reject!: (reason: unknown) => void
  const completed = new Promise<LspServerInfo>((ok, fail) => { resolve = ok; reject = fail })
  void completed.catch(() => {})
  const close = () => {
    if (streamId) void invoke("host_stream_close", { owner: service.owner, streamId }).catch(() => {})
  }
  const finish = (error?: unknown, info?: LspServerInfo) => {
    if (done) return
    done = true
    clearTimeout(timer)
    pending.delete(key)
    close()
    if (info) resolve({ ...info, workspace: workspace ?? "" })
    else reject(error)
  }
  const timer = setTimeout(() => finish(new Error("LSP installation timed out")), 10 * 60 * 1000)
  pending.set(key, () => finish(new Error("LSP installation cancelled")))
  try {
    await requestWorkspace(service, { method: "workspaceAuthorize", params: { workspace: service.capabilityId } })
    if (done) return await completed
    type Message =
      | { type: "closed"; owner: ConnectionOwner; reason: string }
      | { type: "frame"; frame: { version: number; owner: ConnectionOwner; payload:
          | { type: "lspInstallProgress"; event: LspInstallProgress }
          | { type: "lspInstalled"; outcome: { status: "ok"; value: LspServerInfo } | { status: "error"; message: string } }
        } }
    const channel = new Channel<Message>()
    channel.onmessage = (message) => {
      if (done) return
      const owner = message.type === "frame" ? message.frame.owner : message.owner
      if (!sameConnection(owner, service.owner)) return
      try { service.assertCurrent() } catch (error) { finish(error); return }
      if (message.type === "closed") { finish(new Error(message.reason)); return }
      if (message.frame.version !== 1) return
      const payload = message.frame.payload
      if (payload.type === "lspInstallProgress" && payload.event.language === language) onProgress?.(payload.event)
      if (payload.type === "lspInstalled") {
        if (payload.outcome.status === "error") finish(new Error(payload.outcome.message))
        else if (payload.outcome.value.language !== language) finish(new Error("LSP installation identity mismatch"))
        else finish(undefined, payload.outcome.value)
      }
    }
    const opened = await invoke<{ streamId: string }>("host_stream_open", { owner: service.owner, config: { kind: "lspInstall", path: service.root, language, global: workspace === null }, onEvent: channel })
    streamId = opened.streamId
    if (done) close()
    else { service.assertCurrent() }
  } catch (error) { finish(error) }
  const info = await completed
  service.assertCurrent()
  const remote = await import("./remoteLsp")
  service.assertCurrent()
  await remote.restartConfiguredRemoteLsp(context, workspace === null, language)
  service.assertCurrent()
  return info
}
