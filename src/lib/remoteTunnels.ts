import { invoke } from "./ipc"
import type { ConnectionOwner } from "./runtimeIdentity"

export interface TunnelEndpoint { host: string; port: number }
export async function openRemoteTunnel(owner: ConnectionOwner, resourceOwner: string, endpoint: TunnelEndpoint, assertCurrent: () => void) {
  assertCurrent()
  const result = await invoke<{ tunnelId: string; localPort: number }>("host_tunnel_open", { owner, resourceOwner, endpoint })
  let closed = false
  const close = async () => {
    if (closed) return
    closed = true
    await invoke("host_tunnel_close", { owner, resourceOwner, tunnelId: result.tunnelId }).catch(() => {})
  }
  try {
    assertCurrent()
    if (!Number.isInteger(result.localPort) || result.localPort < 1 || result.localPort > 65535) throw new Error("Invalid tunnel port")
  } catch (error) { await close(); throw error }
  return { port: result.localPort, close }
}
