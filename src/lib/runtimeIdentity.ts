/** Host identity is independent of display names and transient connections. */
export const LOCAL_HOST_ID = "local"

export interface RuntimeKey {
  hostId: string
  sessionName: string
}

export interface ConnectionOwner {
  hostId: string
  generation: number
}

export function runtimeKey(key: RuntimeKey): string {
  return JSON.stringify([key.hostId, key.sessionName])
}

export function sameConnection(left: ConnectionOwner, right: ConnectionOwner): boolean {
  return left.hostId === right.hostId && left.generation === right.generation
}

const REMOTE_FILE_PREFIX = "yuzora-fs://"

/** Stable document key; remote paths must never be passed to native I/O. */
export function remoteFilePath(hostId: string, path: string, workspaceRoot = path): string {
  if (!hostId || [path, workspaceRoot].some((value) => !value.startsWith("/") || value.includes("\0") || value.split("/").some((part) => part === "." || part === ".."))) {
    throw new Error("Invalid remote file identity")
  }
  // The authority includes the workspace root. Path joins still work, while
  // overlapping folders on one host never share dirty buffers or capabilities.
  return `${REMOTE_FILE_PREFIX}${encodeURIComponent(hostId)}@${encodeURIComponent(workspaceRoot)}${path.split("/").map(encodeURIComponent).join("/")}`
}

export function parseRemoteFilePath(value: string): { hostId: string; path: string; workspaceRoot: string | null } | null {
  if (!value.startsWith(REMOTE_FILE_PREFIX)) return null
  const match = value.slice(REMOTE_FILE_PREFIX.length).match(/^([^/]+)(\/.*)$/)
  if (!match) throw new Error("Invalid remote file identity")
  const authority = match[1].split("@")
  if (authority.length > 2) throw new Error("Invalid remote file identity")
  const hostId = decodeURIComponent(authority[0])
  const workspaceRoot = authority.length === 2 ? decodeURIComponent(authority[1]) : null
  const path = match[2].split("/").map(decodeURIComponent).join("/")
  const canonical = remoteFilePath(hostId, path, workspaceRoot ?? path)
  const expected = workspaceRoot === null ? canonical.replace(`@${encodeURIComponent(path)}`, "") : canonical
  if (expected !== value) throw new Error("Non-canonical remote file identity")
  // Old unscoped identities remain readable for migration/display; providers
  // require an explicit workspace binding before any I/O.
  return { hostId, path, workspaceRoot }
}
