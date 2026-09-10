import { runtimeWorkspaceService } from "@/lib/remoteFiles"
import { openRemoteTunnel } from "@/lib/remoteTunnels"
import { parseRemoteFilePath } from "@/lib/runtimeIdentity"

interface Lease { source: string; port: number; assertCurrent: () => void }
const active = new Map<string, Lease>()
export function needsRemotePreviewTunnel(workspace: string | null, value: string | null): boolean {
  if (!workspace || !value || !parseRemoteFilePath(workspace)) return false
  try { const url = new URL(value); return url.protocol === "http:" && ["localhost", "127.0.0.1"].includes(url.hostname) } catch { return false }
}
function project(url: string, port: number): string {
  const parsed = new URL(url)
  parsed.hostname = "127.0.0.1"
  parsed.port = String(port)
  return parsed.href
}
export function remotePreviewDisplayUrl(workspace: string, url: string): string {
  if (!needsRemotePreviewTunnel(workspace, url)) return url
  const lease = active.get(workspace)
  if (!lease || lease.source !== new URL(url).origin) throw new Error("Remote preview is not connected")
  lease.assertCurrent()
  return project(url, lease.port)
}

export async function acquireRemotePreviewUrl(workspace: string, url: string, assertLive: () => void) {
  const source = new URL(url)
  if (!needsRemotePreviewTunnel(workspace, url)) throw new Error("Invalid remote preview URL")
  const service = runtimeWorkspaceService(workspace)
  const assertCurrent = () => { service.assertCurrent(); assertLive() }
  const tunnel = await openRemoteTunnel(service.owner, `preview:${workspace}:${crypto.randomUUID()}`, { host: source.hostname, port: source.port ? Number(source.port) : 80 }, assertCurrent)
  const lease = { source: source.origin, port: tunnel.port, assertCurrent }
  active.set(workspace, lease)
  return {
    url: project(url, tunnel.port),
    close: async () => { if (active.get(workspace) === lease) active.delete(workspace); await tunnel.close() },
  }
}
