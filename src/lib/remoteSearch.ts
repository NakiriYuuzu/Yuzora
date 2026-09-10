import { Channel } from "@tauri-apps/api/core"
import { invoke } from "./ipc"
import { runtimeWorkspaceService } from "./remoteFiles"
import { parseRemoteFilePath, remoteFilePath, sameConnection, type ConnectionOwner } from "./runtimeIdentity"
import type { SearchEvent } from "./types"

interface Search { service: ReturnType<typeof runtimeWorkspaceService>; streamId?: string; done: boolean }
let current: Search | null = null

export async function stopRemoteSearch(): Promise<void> {
  const previous = current
  current = null
  if (previous?.streamId) await invoke("host_stream_close", { owner: previous.service.owner, streamId: previous.streamId }).catch(() => undefined)
}

export async function searchRemoteWorkspace(root: string, query: string, caseSensitive: boolean, onEvent: (event: SearchEvent) => void): Promise<void> {
  const previous = current
  const service = runtimeWorkspaceService(root)
  const search: Search = { service, done: false }
  current = search
  if (previous?.streamId) await invoke("host_stream_close", { owner: previous.service.owner, streamId: previous.streamId }).catch(() => undefined)
  if (current !== search) return
  if (!query) { search.done = true; current = null; onEvent({ type: "done", truncated: false, fileCount: 0 }); return }
  type Message =
    | { type: "frame"; frame: { version: number; owner: ConnectionOwner; payload: { type: string; event?: SearchEvent } } }
    | { type: "closed"; owner: ConnectionOwner }
  const channel = new Channel<Message>()
  channel.onmessage = (message) => {
    if (current !== search || search.done) return
    try { service.assertCurrent() } catch { void stopRemoteSearch(); return }
    const owner = message.type === "frame" ? message.frame.owner : message.owner
    if (!sameConnection(owner, service.owner)) return
    if (message.type === "closed") {
      search.done = true
      onEvent({ type: "done", truncated: true, fileCount: 0 })
      return
    }
    const event = message.frame.payload.event
    if (message.frame.version !== 1 || message.frame.payload.type !== "search" || !event) return
    if (event.type === "match") {
      if (event.path !== service.root && !event.path.startsWith(service.root.replace(/\/$/, "") + "/")) return
      onEvent({ ...event, path: remoteFilePath(service.owner.hostId, event.path, service.root) })
    } else { search.done = true; onEvent(event) }
  }
  service.assertCurrent()
  const opened = await invoke<{ streamId: string }>("host_stream_open", { owner: service.owner, config: { kind: "search", path: parseRemoteFilePath(root)!.path, query, caseSensitive }, onEvent: channel })
  try { service.assertCurrent() } catch { search.done = true }
  if (current !== search || search.done) {
    await invoke("host_stream_close", { owner: service.owner, streamId: opened.streamId }).catch(() => undefined)
  } else search.streamId = opened.streamId
}
