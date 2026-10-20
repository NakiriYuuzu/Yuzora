import type { GitRef } from "./protocol"

/** A Space's key in the per-Space caches. */
export const spaceKey = (session: string, workspaceId: string) => `${session}\0${workspaceId}`

/** A repository-relative path as its directory (with the trailing slash) and file name. */
export function splitPath(path: string): { dir: string; name: string } {
  const slash = path.lastIndexOf("/")
  return { dir: path.slice(0, slash + 1), name: path.slice(slash + 1) }
}

/** A git timestamp (Unix seconds) in the phone's language. */
export function formatWhen(seconds: number, language: string, withTime = false): string {
  return new Date(seconds * 1000).toLocaleString(language, withTime ? { dateStyle: "medium", timeStyle: "short" } : { dateStyle: "medium" })
}

/** A commit's refs as chips. HEAD is implied by the local branch it points at, so it only shows when detached. */
export function visibleRefs(refs: GitRef[]): GitRef[] {
  return refs.some((ref) => ref.kind === "local") ? refs.filter((ref) => ref.kind !== "head") : refs
}
