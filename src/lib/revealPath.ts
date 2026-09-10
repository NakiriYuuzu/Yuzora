import { revealItemInDir } from "@tauri-apps/plugin-opener"
import { useHostStore } from "@/state/hostStore"
import { isWindowsPlatform } from "./platform"
import { parseRemoteFilePath } from "./runtimeIdentity"

/** Resolve document identity against the connected host, never its display label. */
export function systemRevealPath(path: string): string | null {
  const remote = parseRemoteFilePath(path)
  if (!remote) return path
  const host = useHostStore.getState().hosts[remote.hostId]
  if (!isWindowsPlatform() || !host?.connection || host.target.kind !== "wsl") return null
  const distro = host.target.distro
  if (!distro || /[\\/\0\r\n]/.test(distro) || /[\\\0\r\n]/.test(remote.path)) return null
  return `\\\\wsl.localhost\\${distro}${remote.path.replaceAll("/", "\\")}`
}

export async function revealPathInSystem(path: string): Promise<void> {
  const nativePath = systemRevealPath(path)
  if (!nativePath) throw new Error("File manager is unavailable for this remote host")
  await revealItemInDir(nativePath)
}
