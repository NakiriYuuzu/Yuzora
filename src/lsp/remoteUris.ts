import { parseRemoteFilePath, remoteFilePath } from "@/lib/runtimeIdentity"

const uriFields = new Set(["uri", "rootUri", "targetUri", "oldUri", "newUri", "target"])

/** Translate protocol URI fields; source text and diagnostics remain untouched. */
export function mapRemoteLspMessage(message: string, workspace: string, direction: "toHost" | "fromHost"): string {
  const context = parseRemoteFilePath(workspace)
  if (!context?.workspaceRoot) throw new Error("LSP requires a workspace identity")
  const { hostId, workspaceRoot } = context
  const owned = (path: NonNullable<ReturnType<typeof parseRemoteFilePath>>) => {
    if (path.hostId !== hostId) throw new Error("LSP document belongs to another host")
    if (path.workspaceRoot !== workspaceRoot) throw new Error("LSP document belongs to another workspace")
  }
  const uri = (value: string): string => {
    const resource = parseRemoteFilePath(value)
    if (resource) owned(resource)
    if (direction === "toHost") {
      if (value.startsWith("file://")) throw new Error("LSP file URI requires workspace identity")
      if (!resource) return value
      return "file://" + resource.path.split("/").map(encodeURIComponent).join("/")
    }
    if (!value.startsWith("file://")) return value
    const parsed = new URL(value)
    if (parsed.hostname && parsed.hostname !== "localhost") throw new Error("Unexpected LSP file authority")
    let path = parsed.pathname
    try { path = decodeURIComponent(path) } catch { /* tolerate servers returning literal percent */ }
    return remoteFilePath(hostId, path, workspaceRoot)
  }
  const visit = (value: unknown, field: string, depth: number): unknown => {
    if (depth > 64) throw new Error("LSP message nesting limit exceeded")
    if (typeof value === "string") {
      if (field === "rootPath") {
        if (direction === "fromHost") return value.startsWith("/") ? remoteFilePath(hostId, value, workspaceRoot) : value
        const path = parseRemoteFilePath(value)
        if (path) owned(path)
        return path?.path ?? value
      }
      return uriFields.has(field) ? uri(value) : value
    }
    if (Array.isArray(value)) return value.map((item) => visit(item, field, depth + 1))
    if (!value || typeof value !== "object") return value
    return Object.fromEntries(Object.entries(value).map(([key, item]) => [
      field === "changes" ? uri(key) : key, visit(item, key, depth + 1)
    ]))
  }
  return JSON.stringify(visit(JSON.parse(message), "", 0))
}
