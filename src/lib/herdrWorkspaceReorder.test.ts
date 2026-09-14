import { describe, expect, it } from "vitest"
import { herdrWorkspaceInsertIndex } from "./herdrWorkspaceReorder"

describe("herdrWorkspaceInsertIndex", () => {
  it("maps before and after drops to HERDR original insertion boundaries", () => {
    expect(herdrWorkspaceInsertIndex(0, 2, false, 4)).toBe(2)
    expect(herdrWorkspaceInsertIndex(0, 2, true, 4)).toBe(3)
    expect(herdrWorkspaceInsertIndex(3, 1, false, 4)).toBe(1)
    expect(herdrWorkspaceInsertIndex(3, 1, true, 4)).toBe(2)
    expect(herdrWorkspaceInsertIndex(0, 1, false, 4)).toBeNull()
  })

  it("rejects invalid or self drops", () => {
    expect(herdrWorkspaceInsertIndex(-1, 0, false, 2)).toBeNull()
    expect(herdrWorkspaceInsertIndex(0, 2, false, 2)).toBeNull()
    expect(herdrWorkspaceInsertIndex(1, 1, false, 2)).toBeNull()
    expect(herdrWorkspaceInsertIndex(1, 0, false, 2)).toBe(0)
  })
})

import { planHerdrWorkspaceReorder, herdrReorderMembers } from './herdrWorkspaceReorder'
import type { HerdrSpaceInfo } from './herdrTypes'
const space = (id: string, key?: string, linked = false): HerdrSpaceInfo => ({ id, label: id, order: 0, focused: false, worktreeGroupKey: key, isLinkedWorktree: linked })
it('moves a complete noncontiguous worktree group as an atomic block', () => {
  const spaces = [space('a', 'repo'), space('b'), space('a-child', 'repo', true), space('c')]
  expect(planHerdrWorkspaceReorder(spaces, 'a', 'c', true)).toEqual({ sourceWorkspaceIds: ['a', 'a-child'], beforeWorkspaceId: null, legacyInsertIndex: 4, expectedOrder: ['b', 'c', 'a', 'a-child'] })
  expect(planHerdrWorkspaceReorder(spaces, 'a-child', 'c', false)).toBeNull()
})
it('uses the boundary after the entire target group and preserves downward legacy semantics', () => {
  expect(planHerdrWorkspaceReorder([space('a'), space('b', 'r'), space('bc', 'r', true), space('c')], 'a', 'b', true)).toEqual({ sourceWorkspaceIds: ['a'], beforeWorkspaceId: 'c', legacyInsertIndex: 3, expectedOrder: ['b', 'bc', 'a', 'c'] })
  expect(herdrWorkspaceInsertIndex(0, 1, true, 3)).toBe(2)
})
it('refuses ambiguous inventory-only grouping and same-group drops', () => {
  expect(herdrReorderMembers([{ ...space('a'), repoKey: 'r' }, { ...space('b'), repoKey: 'r' }], 'a')).toBeNull()
  expect(planHerdrWorkspaceReorder([space('a', 'r'), space('b', 'r', true)], 'a', 'b', false)).toBeNull()
})
it('refuses a partly identified group instead of dropping its unknown members', () => {
  expect(herdrReorderMembers([space('a', 'r'), { ...space('b'), repoKey: 'r' }], 'a')).toBeNull()
})
