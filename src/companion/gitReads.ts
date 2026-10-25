import { call } from "./client"
import { gitGeneration, spaceKey } from "./lib/git"
import type { DiffSource, GitBranches, GitCommit, GitDiff, GitLog, GitStatus } from "./lib/protocol"
import { RpcError } from "./lib/rpc"
import { notify, useCompanion, type SpaceGit } from "./store"

// Read-only Git of a Space, filled into the store's per-Space cache. A failed
// read throws its `RpcError` for the view to show.

const generationOf = (session: string, workspaceId: string) => gitGeneration(useCompanion.getState().snapshots[session], workspaceId)

/** `generation` is the repository the read was started against: a read that outlived a remap of the Space is dropped, not mixed into the new repository's cache. */
function update(session: string, workspaceId: string, generation: string | undefined, change: (git: SpaceGit) => Partial<SpaceGit>): void {
  if (generationOf(session, workspaceId) !== generation) return
  const key = spaceKey(session, workspaceId)
  useCompanion.setState((state) => {
    const git = state.git[key] ?? { generation, commits: {} }
    return { git: { ...state.git, [key]: { ...git, ...change(git) } } }
  })
}

const cached = (session: string, workspaceId: string) => useCompanion.getState().git[spaceKey(session, workspaceId)]

export async function loadStatus(session: string, workspaceId: string): Promise<GitStatus> {
  const generation = generationOf(session, workspaceId)
  const status = await call<GitStatus>("git.status", { session, workspaceId })
  update(session, workspaceId, generation, () => ({ status }))
  return status
}

export async function loadBranches(session: string, workspaceId: string): Promise<void> {
  const generation = generationOf(session, workspaceId)
  const branches = await call<GitBranches>("git.branches", { session, workspaceId })
  update(session, workspaceId, generation, () => ({ branches }))
}

/** The first page, or with `more` the next one. An expired cursor starts the history again. */
export async function loadLog(session: string, workspaceId: string, more = false): Promise<void> {
  const cursor = more ? cached(session, workspaceId)?.log?.nextCursor ?? null : null
  if (more && cursor === null) return
  const generation = generationOf(session, workspaceId)
  let page: GitLog
  try {
    page = await call<GitLog>("git.log", { session, workspaceId, ...(cursor === null ? {} : { cursor }) })
  } catch (error) {
    if (!more || !(error instanceof RpcError) || error.code !== "git-cursor-invalid") throw error
    // The desktop restarted or the repository moved on: the loaded pages no longer lead anywhere.
    notify({ tone: "info", key: "git.historyReloaded" })
    return loadLog(session, workspaceId)
  }
  // Another reload replaced the history while this page was on its way.
  if (more && cached(session, workspaceId)?.log?.nextCursor !== cursor) return
  update(session, workspaceId, generation, (git) => ({ log: more && git.log ? { ...page, commits: [...git.log.commits, ...page.commits] } : page }))
}

export async function loadCommit(session: string, workspaceId: string, hash: string): Promise<void> {
  if (cached(session, workspaceId)?.commits[hash]) return
  const generation = generationOf(session, workspaceId)
  const commit = await call<GitCommit>("git.commit", { session, workspaceId, hash })
  update(session, workspaceId, generation, (git) => ({ commits: { ...git.commits, [hash]: commit } }))
}

/** Diffs are not kept: a working tree diff goes stale, and a big one is not worth the memory. */
export function loadDiff(session: string, workspaceId: string, source: DiffSource): Promise<GitDiff> {
  const params =
    source.kind === "worktree" ? { kind: source.kind, path: source.path, untracked: source.untracked }
    : source.kind === "staged" ? { kind: source.kind, path: source.path, origPath: source.origPath }
    : { kind: source.kind, path: source.path, origPath: source.origPath, commit: source.hash }
  return call<GitDiff>("git.diff", { session, workspaceId, ...params })
}

/** The refresh button: drop the loaded pages so the history starts over. */
export function dropLog(session: string, workspaceId: string): void {
  update(session, workspaceId, generationOf(session, workspaceId), () => ({ log: undefined }))
}
