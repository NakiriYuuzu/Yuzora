import { parseRemoteFilePath } from "@/lib/runtimeIdentity"
import { useHostStore } from "@/state/hostStore"

/** The distro of a WSL host-runtime workspace, or null for local and SSH workspaces. */
export function wslDistroOf(workspacePath: string): string | null {
  const remote = parseRemoteFilePath(workspacePath)
  if (!remote) return null
  const config = useHostStore.getState().configs[remote.hostId]
  return config?.kind === "wsl" && config.distro ? config.distro : null
}

/** Whether Finder / Explorer files can be copied into the workspace: local folders and WSL host workspaces. */
export function canImportOsFiles(workspacePath: string): boolean {
  return !parseRemoteFilePath(workspacePath) || wslDistroOf(workspacePath) !== null
}
