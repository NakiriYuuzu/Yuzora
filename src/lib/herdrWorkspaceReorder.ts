import type { HerdrSpaceInfo } from './herdrTypes'

/** Legacy HERDR takes the insertion boundary BEFORE removing the source. */
export function herdrWorkspaceInsertIndex(source: number, target: number, after: boolean, length: number): number | null {
  if (![source, target, length].every(Number.isInteger) || source < 0 || target < 0 || source >= length || target >= length) return null
  const boundary = target + Number(after)
  const final = boundary - Number(source < boundary)
  return source === final ? null : boundary
}

export interface HerdrReorderPlan {
  sourceWorkspaceIds: string[]
  beforeWorkspaceId: string | null
  legacyInsertIndex: number
  expectedOrder: string[]
}

/** Only API-owned group identity establishes an atomic move. Unverified
 * presentation grouping must not silently turn into a one-member mutation. */
export function herdrReorderMembers(spaces: HerdrSpaceInfo[], sourceId: string): HerdrSpaceInfo[] | null {
  const source = spaces.find(s => s.id === sourceId)
  if (!source || source.isLinkedWorktree) return null
  if (source.worktreeGroupKey) {
    if (spaces.some(s => s.repoKey === source.worktreeGroupKey && s.worktreeGroupKey !== source.worktreeGroupKey)) return null
    return [source, ...spaces.filter(s => s.id !== source.id && s.worktreeGroupKey === source.worktreeGroupKey)]
  }
  if (source.repoKey && spaces.some(s => s.id !== source.id && s.repoKey === source.repoKey)) return null
  return [source]
}

/** Match the sidebar's root groups, without skipping an unmovable neighbor. */
export function adjacentHerdrWorkspace(spaces: HerdrSpaceInfo[], sourceId: string, direction: 'up' | 'down'): HerdrSpaceInfo | null {
  const groups = new Map<string, HerdrSpaceInfo[]>()
  for (const space of spaces) {
    const key = space.worktreeGroupKey ?? space.repoKey ?? space.id
    const group = groups.get(key) ?? []
    group.push(space)
    groups.set(key, group)
  }
  const roots = [...groups.values()].map(group => group.find(space => space.isLinkedWorktree === false) ?? group[0])
  const index = roots.findIndex(space => space.id === sourceId)
  return index < 0 ? null : roots[index + (direction === 'up' ? -1 : 1)] ?? null
}

export function planHerdrWorkspaceReorder(spaces: HerdrSpaceInfo[], sourceId: string, targetId: string, after: boolean): HerdrReorderPlan | null {
  const members = herdrReorderMembers(spaces, sourceId)
  const target = herdrReorderMembers(spaces, targetId)
  if (!members || !target || members.some(s => target.some(t => t.id === s.id))) return null
  const sourceWorkspaceIds = members.map(s => s.id)
  const moving = new Set(sourceWorkspaceIds)
  const remaining = spaces.filter(s => !moving.has(s.id))
  const targetIds = new Set(target.map(s => s.id))
  const positions = remaining.flatMap((s, i) => targetIds.has(s.id) ? [i] : [])
  const insertion = after ? Math.max(...positions) + 1 : Math.min(...positions)
  const beforeWorkspaceId = remaining[insertion]?.id ?? null
  const expectedOrder = remaining.map(s => s.id)
  expectedOrder.splice(insertion, 0, ...sourceWorkspaceIds)
  if (spaces.every((s, i) => s.id === expectedOrder[i])) return null
  return { sourceWorkspaceIds, beforeWorkspaceId, legacyInsertIndex: beforeWorkspaceId ? spaces.findIndex(s => s.id === beforeWorkspaceId) : spaces.length, expectedOrder }
}
