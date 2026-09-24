/**
 * Last-opened branch (Herdr workspace id) for each sidebar Space group.
 * Keys are the tree's session-scoped project keys, so hosts and named Sessions
 * never share memory. Stale ids (for example after a Herdr server restart)
 * are ignored by the caller, which falls back to the first branch.
 */
export const SPACE_BRANCH_MEMORY_KEY = "yuzora.sidebar.lastBranch.v1"
export const SPACE_BRANCH_MEMORY_LIMIT = 200

type Entry = [projectKey: string, workspaceId: string]
let memory: Entry[] | null = null

function load(): Entry[] {
  if (memory) return memory
  try {
    const raw: unknown = JSON.parse(localStorage.getItem(SPACE_BRANCH_MEMORY_KEY) ?? "[]")
    memory = Array.isArray(raw)
      ? raw.filter((item): item is Entry =>
        Array.isArray(item) && item.length === 2 && typeof item[0] === "string" && typeof item[1] === "string",
      ).slice(-SPACE_BRANCH_MEMORY_LIMIT)
      : []
  } catch {
    memory = []
  }
  return memory
}

export function rememberedSpaceBranch(projectKey: string): string | null {
  return load().find(([key]) => key === projectKey)?.[1] ?? null
}

export function rememberSpaceBranch(projectKey: string, workspaceId: string): void {
  const entries = load()
  const last = entries.at(-1)
  if (last?.[0] === projectKey && last[1] === workspaceId) return
  // Most recent last; the oldest entries fall off first.
  memory = [...entries.filter(([key]) => key !== projectKey), [projectKey, workspaceId] as Entry]
    .slice(-SPACE_BRANCH_MEMORY_LIMIT)
  try {
    localStorage.setItem(SPACE_BRANCH_MEMORY_KEY, JSON.stringify(memory))
  } catch {
    // Keep the in-memory value when storage is unavailable.
  }
}

/** Test hook: forget the in-memory copy so the next read reloads storage. */
export function resetSpaceBranchMemoryCache(): void {
  memory = null
}
