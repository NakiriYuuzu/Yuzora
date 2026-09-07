import { loadWorkspaceSessionEntry, type WorkspaceSessionEntry } from "@/state/workspaceSession"
import { useHostStore } from "@/state/hostStore"
import { requestHost, wslPath } from "./hostIpc"
import { nativePathJoin, relativePathWithin } from "./paths"
import { runtimeWorkspaceService } from "./remoteFiles"
import type { WorkspaceOpenResult } from "./types"

/** Copy layout only after the selected distro proves the original folder mapping. */
export async function bindWindowsWorkspace(legacyRoot: string, target: string): Promise<WorkspaceSessionEntry | null> {
  const service = runtimeWorkspaceService(target)
  const host = useHostStore.getState().hosts[service.owner.hostId]
  if (host?.target.kind !== "wsl") throw new Error("Select a WSL2 workspace")
  const path = await wslPath(service.owner.hostId, host.target.distro, legacyRoot)
  service.assertCurrent()
  const opened = await requestHost<WorkspaceOpenResult>(service.owner, { method: "workspaceOpen", params: { path } })
  try {
    service.assertCurrent()
    if (opened.canonicalPath !== service.root) throw new Error("The selected folder does not match the original Windows workspace")
  } finally {
    if (opened.capabilityId !== service.capabilityId) await requestHost(service.owner, { method: "workspaceClose", params: { workspace: opened.capabilityId } }).catch(() => undefined)
  }
  const entry = loadWorkspaceSessionEntry(legacyRoot)
  return entry ? mapWindowsSession(legacyRoot, target, entry) : null
}

export function mapWindowsSession(legacyRoot: string, target: string, entry: WorkspaceSessionEntry): WorkspaceSessionEntry {
  const mapped = new Map<string, string>()
  for (const path of entry.tabs) {
    const relative = relativePathWithin(legacyRoot, path)
    if (!relative || relative.split(/[\\/]/).some((part) => part === "." || part === "..")) continue
    mapped.set(path, nativePathJoin(target, relative.replace(/\\/g, "/")))
  }
  return { tabs: [...new Set(mapped.values())], activePath: entry.activePath ? mapped.get(entry.activePath) ?? null : null }
}
