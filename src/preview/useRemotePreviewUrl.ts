import { useEffect, useState } from "react"
import { parseRemoteFilePath } from "@/lib/runtimeIdentity"
import { useHostStore } from "@/state/hostStore"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { browserTarget, resolveFilePreviewUrl } from "./filePreview"
import { acquireRemotePreviewUrl, needsRemotePreviewTunnel } from "./remotePreviewUrl"

/** A mounted preview owns its tunnel; source navigation remains host-relative. */
export function useRemotePreviewUrl(workspace: string | null, sourceUrl: string | null, reloadNonce: number) {
  const hostId = workspace ? parseRemoteFilePath(workspace)?.hostId : undefined
  const connection = useHostStore((state) => hostId ? state.hosts[hostId]?.connection : null)
  const capability = useWorkspaceStore((state) => state.workspaceCapabilityId)
  const file = !!sourceUrl && browserTarget(sourceUrl).kind === "file"
  const remote = needsRemotePreviewTunnel(workspace, sourceUrl)
  const sourceOrigin = remote && sourceUrl ? new URL(sourceUrl).origin : sourceUrl
  const [result, setResult] = useState<{ key: string; url: string | null; error: string | null } | null>(null)
  const key = JSON.stringify([workspace, sourceOrigin, connection?.owner.generation, file ? capability : null, reloadNonce])
  useEffect(() => {
    if ((!remote && !file) || !workspace || !sourceOrigin) return
    let closed = false
    let lease: Awaited<ReturnType<typeof acquireRemotePreviewUrl>> | undefined
    let timer: ReturnType<typeof setTimeout> | undefined
    let attempts = 0
    const assertLive = () => { if (closed) throw new Error("Preview was closed") }
    const open = async () => {
      try {
        if (file) {
          const url = await resolveFilePreviewUrl(workspace, sourceOrigin)
          if (!closed) setResult({ key, url, error: null })
          return
        }
        lease = await acquireRemotePreviewUrl(workspace, sourceOrigin, assertLive)
        if (closed) { await lease.close(); return }
        setResult({ key, url: lease.url, error: null })
      } catch (error) {
        if (closed) return
        setResult({ key, url: null, error: String(error) })
        // Files capabilities may still be reopening after the host reconnects.
        if (connection) timer = setTimeout(() => void open(), Math.min(30000, 1000 * 2 ** Math.min(attempts++, 5)))
      }
    }
    void open()
    return () => { closed = true; clearTimeout(timer); void lease?.close() }
  }, [remote, file, workspace, sourceOrigin, connection, key])
  if (!remote && !file) return { url: sourceUrl, error: null }
  if (result?.key !== key || !result.url || !sourceUrl) return result?.key === key ? result : { url: null, error: null }
  if (file) return result
  const projected = new URL(sourceUrl)
  const tunnel = new URL(result.url)
  projected.hostname = tunnel.hostname
  projected.port = tunnel.port
  return { ...result, url: projected.href }
}
