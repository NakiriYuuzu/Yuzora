import { requestHost } from "./hostIpc"
import { runtimeWorkspaceService } from "./remoteFiles"
import { openRemoteTunnel } from "./remoteTunnels"
import { parseRemoteFilePath } from "./runtimeIdentity"
import type { PreviewSession } from "./ipc"

const sessions = new Map<string, () => Promise<void>>()
const PREFIX = "host-preview:"
export function isRemotePreviewToken(token: string): boolean { return token.startsWith(PREFIX) }

export async function createRemotePreview(path: string): Promise<PreviewSession> {
  if (sessions.size >= 64) throw new Error("Too many remote previews")
  const service = runtimeWorkspaceService(path)
  const file = parseRemoteFilePath(path)!
  const relative = file.path.slice(service.root.length).replace(/^\//, "")
  service.assertCurrent()
  const remote = await requestHost<PreviewSession>(service.owner, { method: "previewCreate", params: { workspace: service.capabilityId, path: relative } })
  let tunnel: Awaited<ReturnType<typeof openRemoteTunnel>> | undefined
  const release = async () => {
    await tunnel?.close()
    await requestHost(service.owner, { method: "previewRevoke", params: { workspace: service.capabilityId, token: remote.token } }).catch(() => {})
  }
  try {
    service.assertCurrent()
    const url = new URL(remote.url)
    if (!/^[a-f0-9]{64}$/i.test(remote.token) || url.protocol !== "http:" || url.hostname !== "127.0.0.1" || url.username || url.password || !url.pathname.startsWith(`/${remote.token}/`)) throw new Error("Invalid host preview session")
    const token = PREFIX + crypto.randomUUID()
    tunnel = await openRemoteTunnel(service.owner, token, { host: "127.0.0.1", port: Number(url.port) }, service.assertCurrent)
    service.assertCurrent()
    url.port = String(tunnel.port)
    sessions.set(token, release)
    return { token, url: url.href, sourcePath: path }
  } catch (error) { await release(); throw error }
}

export async function revokeRemotePreview(token: string): Promise<void> {
  const release = sessions.get(token)
  sessions.delete(token)
  await release?.()
}
export async function stopRemotePreviews(): Promise<void> {
  await Promise.all([...sessions.keys()].map(revokeRemotePreview))
}

export async function restoreRemotePreview(workspace: string): Promise<void> {
  const { usePreviewStore } = await import("@/state/previewStore")
  const previous = usePreviewStore.getState().staticPreview
  if (previous?.workspace !== workspace || !previous.sourcePath) return
  const session = await createRemotePreview(previous.sourcePath)
  if (usePreviewStore.getState().staticPreview !== previous) { await revokeRemotePreview(session.token); return }
  usePreviewStore.getState().openStaticPreview(workspace, session)
}
