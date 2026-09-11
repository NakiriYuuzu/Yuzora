import { openWorkspace, openWorkspaceDirectory } from "./ipc"
import { canonicalPathKey, isSameOrDescendantPath } from "./paths"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { revealHostPath } from "./hostIpc"
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

const capabilityRenewals = new Map<string, Promise<string>>()

function renewDirectoryCapability(root: string, previousCapability: string | null): Promise<string> {
  const key = JSON.stringify([root, previousCapability])
  const pending = capabilityRenewals.get(key)
  if (pending) return pending
  const request = openWorkspace(root).then(opened => {
    const current = useWorkspaceStore.getState()
    if (current.workspacePath !== root || current.workspaceCapabilityId !== previousCapability ||
        canonicalPathKey(opened.canonicalPath) !== canonicalPathKey(root)) {
      throw new Error("Workspace changed while refreshing its directory capability")
    }
    // Refresh only the ephemeral grant. Reopening the UI workspace would clear
    // live editors, selection and unsaved buffers merely to open a directory.
    useWorkspaceStore.setState({ workspaceCapabilityId: opened.capabilityId })
    return opened.capabilityId
  }).finally(() => capabilityRenewals.delete(key))
  capabilityRenewals.set(key, request)
  return request
}

export async function revealPathInSystem(path: string, isDirectory = false): Promise<void> {
  const nativePath = systemRevealPath(path)
  if (!nativePath) throw new Error("File manager is unavailable for this remote host")
  const remote = parseRemoteFilePath(path)
  if (remote) {
    const owner = useHostStore.getState().hosts[remote.hostId]?.connection?.owner
    if (!owner) throw new Error("File manager is unavailable for this remote host")
    await revealHostPath(owner, remote.path, isDirectory)
    return
  }
  if (isDirectory) {
    const { workspacePath: root, workspaceCapabilityId: capability } = useWorkspaceStore.getState()
    if (!root || parseRemoteFilePath(root) || !isSameOrDescendantPath(root, nativePath)) {
      throw new Error("Current workspace directory capability is unavailable")
    }
    if (capability) {
      try {
        await openWorkspaceDirectory(capability, nativePath)
        return
      } catch (error) {
        // Retry only an expired process-local grant, never an ordinary I/O or
        // containment failure. Native validation remains the authority.
        if ((error instanceof Error ? error.message : String(error)) !== "workspace-capability-missing") throw error
      }
    }
    const current = useWorkspaceStore.getState()
    if (current.workspacePath !== root) throw new Error("Workspace changed before opening its directory")
    const renewed = current.workspaceCapabilityId && current.workspaceCapabilityId !== capability
      ? current.workspaceCapabilityId
      : await renewDirectoryCapability(root, capability)
    if (useWorkspaceStore.getState().workspacePath !== root || useWorkspaceStore.getState().workspaceCapabilityId !== renewed) {
      throw new Error("Workspace changed before opening its directory")
    }
    await openWorkspaceDirectory(renewed, nativePath)
  } else {
    await revealItemInDir(nativePath)
  }
}
