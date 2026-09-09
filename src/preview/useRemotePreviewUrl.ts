import { useEffect, useState } from "react"
import { parseRemoteFilePath } from "@/lib/runtimeIdentity"
import { useHostStore } from "@/state/hostStore"
import { acquireRemotePreviewUrl, needsRemotePreviewTunnel } from "./remotePreviewUrl"

/** A mounted preview owns its tunnel; source navigation remains host-relative. */
export function useRemotePreviewUrl(workspace: string | null, sourceUrl: string | null, reloadNonce: number) {
  const hostId = workspace ? parseRemoteFilePath(workspace)?.hostId : undefined
  const connection = useHostStore((state) => hostId ? state.hosts[hostId]?.connection : null)
  const remote = needsRemotePreviewTunnel(workspace, sourceUrl)
  const [result, setResult] = useState<{ key: string; url: string | null; error: string | null } | null>(null)
  const key = JSON.stringify([workspace, sourceUrl, connection?.owner.generation, reloadNonce])
  useEffect(() => {
    if (!remote || !workspace || !sourceUrl) return
    let closed = false
    let lease: Awaited<ReturnType<typeof acquireRemotePreviewUrl>> | undefined
    let timer: ReturnType<typeof setTimeout> | undefined
    let attempts = 0
    const assertLive = () => { if (closed) throw new Error("Preview was closed") }
    const open = async () => {
      try {
        lease = await acquireRemotePreviewUrl(workspace, sourceUrl, assertLive)
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
  }, [remote, workspace, sourceUrl, connection, key])
  if (!remote) return { url: sourceUrl, error: null }
  return result?.key === key ? result : { url: null, error: null }
}
