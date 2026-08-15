import type {
  HerdrSpaceInfo,
  HerdrWorktreeInfo,
  HerdrWorktreeInventory,
  HerdrWorktreeListResult,
  HerdrWorktreeSourceInfo
} from "./herdrTypes"

/**
 * Build a session-scoped worktree inventory indexed only by open workspace id.
 * Path identity is never used as a key — protocol-19 `open_workspace_id` is.
 */
export function buildWorktreeInventory(
  sessionName: string,
  lists: HerdrWorktreeListResult[],
  failedScopes: string[] = []
): HerdrWorktreeInventory {
  const byOpenWorkspaceId: HerdrWorktreeInventory["byOpenWorkspaceId"] =
    Object.create(null) as HerdrWorktreeInventory["byOpenWorkspaceId"]
  for (const list of lists) {
    for (const worktree of list.worktrees) {
      const openId = worktree.openWorkspaceId
      if (typeof openId !== "string" || openId.length === 0) continue
      // Last writer wins when the same open workspace appears in multiple list
      // results (same-repo dedupe callers may still pass overlapping lists).
      byOpenWorkspaceId[openId] = {
        worktree,
        source: list.source
      }
    }
  }
  return {
    sessionName,
    lists: [...lists],
    failedScopes: [...failedScopes],
    byOpenWorkspaceId
  }
}

/**
 * Merge inventory provenance onto normalized Space projection.
 * Matching is exclusively by `open_workspace_id === space.id`.
 * Existing path / snapshot-derived fields are preserved when inventory is partial.
 */
export function mergeSpaceWorktreeProvenance(
  spaces: HerdrSpaceInfo[],
  inventory: HerdrWorktreeInventory | null | undefined
): HerdrSpaceInfo[] {
  if (!inventory) return spaces
  return spaces.map((space) => {
    const entry = Object.prototype.hasOwnProperty.call(
      inventory.byOpenWorkspaceId,
      space.id
    )
      ? inventory.byOpenWorkspaceId[space.id]
      : undefined
    if (!entry) return space
    return applyWorktreeEntryToSpace(space, entry.worktree, entry.source)
  })
}

export function applyWorktreeEntryToSpace(
  space: HerdrSpaceInfo,
  worktree: HerdrWorktreeInfo,
  source: HerdrWorktreeSourceInfo
): HerdrSpaceInfo {
  // Prefer inventory checkout path when present; otherwise keep existing path
  // fallback chain (snapshot worktree.checkout_path / cwd / agent pane cwd).
  const path =
    typeof worktree.path === "string" && worktree.path.length > 0
      ? worktree.path
      : space.path

  return {
    ...space,
    path,
    repoKey: source.repoKey,
    repoName: source.repoName,
    repoRoot: source.repoRoot,
    sourceCheckoutPath: source.sourceCheckoutPath,
    branch: worktree.branch ?? null,
    isLinkedWorktree: worktree.isLinkedWorktree,
    isDetached: worktree.isDetached,
    isPrunable: worktree.isPrunable,
    isBare: worktree.isBare,
    worktreeLabel: worktree.label
  }
}

/** Snapshot nested `workspace.worktree` (WorkspaceWorktreeInfo) → Space fields. */
export function spaceProvenanceFromSnapshotWorktree(
  worktree: Record<string, unknown> | null | undefined
): Partial<HerdrSpaceInfo> {
  if (!worktree) return {}
  const asString = (value: unknown): string | null =>
    typeof value === "string" && value.length > 0 ? value : null
  const isLinked = worktree.is_linked_worktree
  return {
    path: asString(worktree.checkout_path) ?? undefined,
    repoKey: asString(worktree.repo_key),
    repoName: asString(worktree.repo_name),
    repoRoot: asString(worktree.repo_root),
    isLinkedWorktree: typeof isLinked === "boolean" ? isLinked : null
  }
}
