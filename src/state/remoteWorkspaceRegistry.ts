import { parseRemoteFilePath } from "@/lib/runtimeIdentity"

export const REMOTE_WORKSPACES_STORAGE_KEY = "yuzora.workspaces.remote.v1"
export type RemoteWorkspaceAccess = "runtime" | "sftp"

/** Durable identity and access only; capabilities and connection generations are never persisted. */
export function loadRemoteWorkspaces(): Record<string, RemoteWorkspaceAccess> {
  try {
    const rows: unknown = JSON.parse(localStorage.getItem(REMOTE_WORKSPACES_STORAGE_KEY) ?? "{}")
    if (!rows || typeof rows !== "object" || Array.isArray(rows)) return {}
    const result: Record<string, RemoteWorkspaceAccess> = {}
    for (const [uri, access] of Object.entries(rows).slice(-100)) {
      try {
        const resource = parseRemoteFilePath(uri)
        if (resource?.workspaceRoot === resource?.path && resource && (access === "runtime" || access === "sftp")) result[uri] = access
      } catch { /* Malformed identities never acquire filesystem authority. */ }
    }
    return result
  } catch { return {} }
}

export function rememberRemoteWorkspace(uri: string, access: RemoteWorkspaceAccess): void {
  const resource = parseRemoteFilePath(uri)
  if (!resource || resource.workspaceRoot !== resource.path) throw new Error("Expected a scoped workspace identity")
  const records = loadRemoteWorkspaces()
  delete records[uri]
  records[uri] = access
  try { localStorage.setItem(REMOTE_WORKSPACES_STORAGE_KEY, JSON.stringify(Object.fromEntries(Object.entries(records).slice(-100)))) }
  catch { /* Current session stays usable when local storage is unavailable. */ }
}
