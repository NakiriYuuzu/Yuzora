import type { HerdrSnapshot } from "./herdrTypes"
import { parseRuntimeScope } from "./herdrProvider"
import { LOCAL_HOST_ID, parseRemoteFilePath, remoteFilePath, runtimeKey } from "./runtimeIdentity"

const STORAGE_KEY = "yuzora.runtime.workspace-roots.v1"
const MAX_BINDINGS = 2048

function read(): Record<string, string> {
  try {
    const value: unknown = JSON.parse(window.localStorage.getItem(STORAGE_KEY) ?? "{}")
    if (!value || typeof value !== "object" || Array.isArray(value)) return {}
    return Object.fromEntries(Object.entries(value).filter(([key, path]) => key.length < 2048 && typeof path === "string" && path.length < 32768).slice(-MAX_BINDINGS))
  } catch { return {} }
}

function bindingKey(scope: string, workspaceId: string): string {
  return JSON.stringify([runtimeKey(parseRuntimeScope(scope)), workspaceId])
}

/** Call only with a root canonicalized by its file host, chosen explicitly. */
export function bindWorkspaceRoot(scope: string, workspaceId: string, canonicalPath: string): void {
  const hostId = parseRuntimeScope(scope).hostId
  const remote = parseRemoteFilePath(canonicalPath)
  if ((remote?.hostId ?? LOCAL_HOST_ID) !== hostId) throw new Error("Workspace root host mismatch")
  const entries = read()
  const key = bindingKey(scope, workspaceId)
  delete entries[key]
  entries[key] = canonicalPath
  window.localStorage.setItem(STORAGE_KEY, JSON.stringify(Object.fromEntries(Object.entries(entries).slice(-MAX_BINDINGS))))
}

export function projectWorkspaceRoots(scope: string, snapshot: HerdrSnapshot): HerdrSnapshot {
  const { hostId } = parseRuntimeScope(scope)
  const bindings = read()
  const pathOnHost = (path: string | null | undefined) => {
    if (!path || hostId === LOCAL_HOST_ID) return path
    const remote = parseRemoteFilePath(path)
    if (remote) return remote.hostId === hostId ? path : null
    return remoteFilePath(hostId, path)
  }
  return {
    ...snapshot,
    spaces: snapshot.spaces.map((space) => ({
      ...space,
      path: pathOnHost(bindings[bindingKey(scope, space.id)] ?? space.path),
      repoRoot: pathOnHost(space.repoRoot),
      sourceCheckoutPath: pathOnHost(space.sourceCheckoutPath)
    })),
    agents: snapshot.agents.map((agent) => ({ ...agent, sessionName: scope }))
  }
}
