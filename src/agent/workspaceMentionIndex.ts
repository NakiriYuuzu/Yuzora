import { workspacePathIndex } from "@/lib/ipc"
import { canonicalPathKey } from "@/lib/paths"
import type { WorkspacePathIndexEntry, WorkspacePathIndexResult } from "@/lib/types"

export { canonicalPathKey } from "@/lib/paths"

export const MAX_WORKSPACE_MENTION_RESULTS = 100

export interface WorkspaceMentionIndexSnapshot {
  workspace: string
  entries: WorkspacePathIndexEntry[]
  truncated: boolean
  revision: number
}

export interface RankedWorkspaceMention extends WorkspacePathIndexEntry {
  score: number
}

type WorkspacePathIndexLoader = (workspace: string) => Promise<WorkspacePathIndexResult>

interface CacheRecord {
  revision: number
  generation: number
  promise?: Promise<WorkspaceMentionIndexSnapshot | null>
  value?: WorkspaceMentionIndexSnapshot
}

function isCanonicalPathWithin(workspace: string, candidate: string): boolean {
  const rootKey = canonicalPathKey(workspace)
  const candidateKey = canonicalPathKey(candidate)
  if (candidateKey === rootKey) return false
  const prefix = rootKey.endsWith("/") ? rootKey : `${rootKey}/`
  return candidateKey.startsWith(prefix)
}

function normalizedRelativePath(path: string): string | null {
  const normalized = path.replace(/\\/g, "/")
  if (!normalized || normalized.startsWith("/") || /^[A-Za-z]:\//.test(normalized)) return null
  const segments = normalized.split("/")
  if (segments.some((segment) => !segment || segment === "." || segment === "..")) return null
  return segments.join("/")
}

function validateWorkspaceIndexResult(
  requestedWorkspace: string,
  revision: number,
  result: WorkspacePathIndexResult
): WorkspaceMentionIndexSnapshot {
  if (canonicalPathKey(result.workspace) !== canonicalPathKey(requestedWorkspace)) {
    throw new Error("Workspace path index returned a different canonical workspace")
  }

  const seen = new Set<string>()
  const entries: WorkspacePathIndexEntry[] = []
  for (const entry of result.entries) {
    const relativePath = normalizedRelativePath(entry.relativePath)
    if (!relativePath || !isCanonicalPathWithin(result.workspace, entry.canonicalPath)) continue
    const canonicalKey = canonicalPathKey(entry.canonicalPath)
    if (seen.has(canonicalKey)) continue
    seen.add(canonicalKey)
    entries.push({ relativePath, canonicalPath: entry.canonicalPath })
  }
  entries.sort((left, right) => compareText(left.relativePath, right.relativePath)
    || compareText(canonicalPathKey(left.canonicalPath), canonicalPathKey(right.canonicalPath)))

  return {
    workspace: result.workspace,
    entries,
    truncated: result.truncated,
    revision,
  }
}

function compareText(left: string, right: string): number {
  if (left < right) return -1
  if (left > right) return 1
  return 0
}

function fuzzyScore(path: string, query: string): number | null {
  const haystack = path.toLowerCase()
  const needle = query.toLowerCase()
  if (!needle) return 0

  let previous = -1
  let score = 0
  for (const character of needle) {
    const index = haystack.indexOf(character, previous + 1)
    if (index === -1) return null
    const gap = index - previous - 1
    score += Math.max(1, 18 - gap)
    if (index === previous + 1) score += 12
    if (index === 0 || "/._- ".includes(haystack[index - 1] ?? "")) score += 18
    previous = index
  }

  const basename = haystack.slice(haystack.lastIndexOf("/") + 1)
  if (basename.startsWith(needle)) score += 300
  if (haystack.startsWith(needle)) score += 180
  if (haystack === needle || basename === needle) score += 500
  score += Math.max(0, 80 - haystack.length)
  return score
}

export function rankWorkspaceMentions(
  entries: readonly WorkspacePathIndexEntry[],
  query: string,
  limit = MAX_WORKSPACE_MENTION_RESULTS
): RankedWorkspaceMention[] {
  const boundedLimit = Math.max(0, Math.min(Math.trunc(limit), MAX_WORKSPACE_MENTION_RESULTS))
  return entries
    .flatMap((entry) => {
      const score = fuzzyScore(entry.relativePath, query)
      return score === null ? [] : [{ ...entry, score }]
    })
    .sort((left, right) => right.score - left.score
      || compareText(left.relativePath, right.relativePath)
      || compareText(canonicalPathKey(left.canonicalPath), canonicalPathKey(right.canonicalPath)))
    .slice(0, boundedLimit)
}

export class WorkspaceMentionIndexCache {
  private readonly records = new Map<string, CacheRecord>()
  private nextGeneration = 0

  constructor(private readonly loader: WorkspacePathIndexLoader = workspacePathIndex) {}

  load(workspace: string, revision: number): Promise<WorkspaceMentionIndexSnapshot | null> {
    const workspaceKey = canonicalPathKey(workspace)
    const existing = this.records.get(workspaceKey)
    if (existing?.revision === revision) {
      if (existing.promise) return existing.promise
      if (existing.value) return Promise.resolve(existing.value)
    }

    const generation = ++this.nextGeneration
    const promise: Promise<WorkspaceMentionIndexSnapshot | null> = this.loader(workspace)
      .then((result) => {
        const current = this.records.get(workspaceKey)
        if (current?.generation !== generation || current.revision !== revision) return null
        const value = validateWorkspaceIndexResult(workspace, revision, result)
        current.value = value
        current.promise = undefined
        return value
      })
      .catch((error: unknown) => {
        const current = this.records.get(workspaceKey)
        if (current?.generation !== generation || current.revision !== revision) return null
        this.records.delete(workspaceKey)
        throw error
      })

    this.records.set(workspaceKey, { revision, generation, promise })
    return promise
  }

  /**
   * Starts a new UI lifetime for a workspace. Reopening the same path must not
   * inherit a value or in-flight walk retained by an earlier watcher lifetime.
   * Call this once when the workspace becomes active; query-time `load` calls
   * within that lifetime still share their promise/value normally.
   */
  activateWorkspace(workspace: string): void {
    this.invalidate(workspace)
  }

  invalidate(workspace: string): void {
    this.nextGeneration += 1
    this.records.delete(canonicalPathKey(workspace))
  }

  clear(): void {
    this.nextGeneration += 1
    this.records.clear()
  }
}

export const workspaceMentionIndex = new WorkspaceMentionIndexCache()
