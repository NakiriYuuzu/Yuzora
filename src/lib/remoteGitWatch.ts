import { Channel } from "@tauri-apps/api/core"
import { invoke } from "./ipc"
import { runtimeWorkspaceService } from "./remoteFiles"
import { requestWorkspace } from "./remoteTrust"
import { parseRemoteFilePath, sameConnection, type ConnectionOwner } from "./runtimeIdentity"

interface Attempt { service: ReturnType<typeof runtimeWorkspaceService>; streamId?: string; ended: boolean }

/** One effect owns one watcher; late open/close/event results cannot revive it. */
export function watchRemoteGit(workspace: string, repository: string, onChange: () => void): () => void {
  let stopped = false
  let active: Attempt | null = null
  let retry: ReturnType<typeof setTimeout> | undefined
  let failures = 0
  const close = (attempt: Attempt) => {
    attempt.ended = true
    if (attempt.streamId) void invoke("host_stream_close", { owner: attempt.service.owner, streamId: attempt.streamId }).catch(() => {})
  }
  const schedule = () => {
    if (stopped || retry) return
    retry = setTimeout(() => { retry = undefined; void open() }, Math.min(30000, 1000 * 2 ** Math.min(failures++, 5)))
  }
  const live = (attempt: Attempt) => {
    if (stopped || active !== attempt || attempt.ended) return false
    try { attempt.service.assertCurrent(); return true }
    catch { close(attempt); schedule(); return false }
  }
  async function open() {
    if (stopped) return
    if (active) close(active)
    active = null
    let attempt: Attempt | undefined
    try {
      const service = runtimeWorkspaceService(workspace)
      const root = parseRemoteFilePath(repository)
      if (!root || root.hostId !== service.owner.hostId) throw new Error("Git repository host mismatch")
      attempt = { service, ended: false }
      const opening = attempt
      active = opening
      await requestWorkspace(service, { method: "workspaceAuthorize", params: { workspace: service.capabilityId } })
      if (!live(opening)) return
      type Message =
        | { type: "frame"; frame: { version: number; owner: ConnectionOwner; payload: { type: string; workspaceRoot?: string } } }
        | { type: "closed"; owner: ConnectionOwner }
      const channel = new Channel<Message>()
      channel.onmessage = (message) => {
        if (!live(opening)) return
        const owner = message.type === "frame" ? message.frame.owner : message.owner
        if (!sameConnection(owner, service.owner)) return
        if (message.type === "closed") { close(opening); schedule(); return }
        if (message.frame.version === 1 && message.frame.payload.type === "git" && message.frame.payload.workspaceRoot === service.root) onChange()
      }
      const result = await invoke<{ streamId: string }>("host_stream_open", { owner: service.owner, config: { kind: "git", path: service.root, repositoryRoot: root.path }, onEvent: channel })
      opening.streamId = result.streamId
      if (!live(opening)) { close(opening); return }
      failures = 0
      // Catch metadata changes during reconnect/the watcher opening gap.
      onChange()
    } catch {
      if (attempt) close(attempt)
      if (!attempt || active === attempt) schedule()
    }
  }
  void open()
  return () => {
    stopped = true
    if (retry) clearTimeout(retry)
    if (active) close(active)
    active = null
  }
}
