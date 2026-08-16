import type { GitRollbackTarget } from "@/lib/ipc"
import type { GitFileEntry, GitStatus } from "@/lib/types"

import { badgeChar } from "./fileRows"

export type GitChangeClassification = "tracked" | "added" | "untracked" | "conflicted"
export type GitChangeId = `${"s" | "c"}:${string}`
export type GitSectionKey = "conflicts" | "staged" | "unstaged" | "untracked"

export const GIT_SECTION_ORDER: readonly GitSectionKey[] = [
    "conflicts",
    "staged",
    "unstaged",
    "untracked"
]

export interface GitChangeKey {
    path: string
    staged: boolean
    classification: GitChangeClassification
    stagedStatus: string | null
    unstagedStatus: string | null
    origPath: string | null
}

export interface GitChangeRow extends GitChangeKey {
    badge: string
}

export interface GitChangeSectionBuckets {
    conflicts: GitChangeRow[]
    staged: GitChangeRow[]
    unstaged: GitChangeRow[]
    untracked: GitChangeRow[]
}

export interface GitChangeModel {
    rows: GitChangeRow[]
    buckets: GitChangeSectionBuckets
    visualOrder: GitChangeRow[]
    rowById: ReadonlyMap<GitChangeId, GitChangeRow>
    indexById: ReadonlyMap<GitChangeId, number>
}

const modelCache = new WeakMap<GitStatus, GitChangeModel>()
const modelByRows = new WeakMap<readonly GitChangeRow[], GitChangeModel>()

function byPath(entries: readonly GitFileEntry[]): Map<string, GitFileEntry> {
    return new Map(entries.map((entry) => [entry.path, entry]))
}

function classificationFor(
    path: string,
    staged: Map<string, GitFileEntry>,
    untracked: ReadonlySet<string>,
    conflicted: ReadonlySet<string>
): GitChangeClassification {
    if (conflicted.has(path)) return "conflicted"
    if (untracked.has(path)) return "untracked"
    return staged.get(path)?.status === "A" ? "added" : "tracked"
}

export function gitChangeId(row: Pick<GitChangeKey, "path" | "staged">): GitChangeId {
    return `${row.staged ? "s" : "c"}:${row.path}`
}

export function sameGitChange(a: GitChangeKey | null, b: GitChangeKey | null): boolean {
    return Boolean(a && b && gitChangeId(a) === gitChangeId(b))
}

export function isConflictChange(row: GitChangeKey): boolean {
    return row.classification === "conflicted"
}

export function isStagedChange(row: GitChangeKey): boolean {
    return row.staged
}

export function isUntrackedChange(row: GitChangeKey): boolean {
    return !row.staged && row.classification === "untracked"
}

export function isUnstagedChange(row: GitChangeKey): boolean {
    return !row.staged && row.classification !== "untracked" && row.classification !== "conflicted"
}

export function isStageableChange(row: GitChangeKey): boolean {
    return !row.staged && row.classification !== "conflicted"
}

export function isUnstageableChange(row: GitChangeKey): boolean {
    return row.staged
}

export function isDiscardableChange(row: GitChangeKey): boolean {
    return !row.staged && row.classification !== "conflicted"
}

function createBuckets(): GitChangeSectionBuckets {
    return { conflicts: [], staged: [], unstaged: [], untracked: [] }
}

function addToBuckets(row: GitChangeRow, buckets: GitChangeSectionBuckets) {
    if (isConflictChange(row)) buckets.conflicts.push(row)
    if (isStagedChange(row)) buckets.staged.push(row)
    if (isUnstagedChange(row)) buckets.unstaged.push(row)
    if (isUntrackedChange(row)) buckets.untracked.push(row)
}

const EMPTY_MODEL: GitChangeModel = {
    rows: [],
    buckets: createBuckets(),
    visualOrder: [],
    rowById: new Map(),
    indexById: new Map()
}
modelByRows.set(EMPTY_MODEL.rows, EMPTY_MODEL)

/**
 * Build the canonical status snapshot once. GitStatus objects are immutable store
 * snapshots, so identity caching keeps row objects and all lookup maps stable.
 */
export function buildGitChangeModel(status: GitStatus | null): GitChangeModel {
    if (!status) return EMPTY_MODEL
    const cached = modelCache.get(status)
    if (cached) return cached

    const staged = byPath(status.staged)
    const unstaged = byPath(status.unstaged)
    const untracked = new Set(status.untracked)
    const conflicted = new Set(status.conflicted.map((entry) => entry.path))
    const rows: GitChangeRow[] = []
    const buckets = createBuckets()
    const rowById = new Map<GitChangeId, GitChangeRow>()
    const visualOrder: GitChangeRow[] = []
    const indexById = new Map<GitChangeId, number>()
    const snapshot = (path: string) => ({
        stagedStatus: staged.get(path)?.status ?? null,
        unstagedStatus: unstaged.get(path)?.status ?? null,
        origPath: staged.get(path)?.origPath ?? unstaged.get(path)?.origPath ?? null
    })
    const classify = (path: string): GitChangeClassification => {
        const base = classificationFor(path, staged, untracked, conflicted)
        return base === "tracked" && unstaged.get(path)?.status === "A" ? "added" : base
    }
    const addRow = (row: GitChangeRow) => {
        rows.push(row)
        addToBuckets(row, buckets)
        rowById.set(gitChangeId(row), row)
    }

    for (const entry of status.staged) {
        addRow({
            path: entry.path,
            badge: badgeChar(entry.status),
            staged: true,
            classification: classify(entry.path),
            ...snapshot(entry.path)
        })
    }
    for (const entry of status.unstaged) {
        addRow({
            path: entry.path,
            badge: badgeChar(entry.status),
            staged: false,
            classification: classify(entry.path),
            ...snapshot(entry.path)
        })
    }
    for (const path of status.untracked) {
        addRow({
            path,
            badge: "?",
            staged: false,
            classification: "untracked",
            ...snapshot(path)
        })
    }
    for (const entry of status.conflicted) {
        addRow({
            path: entry.path,
            badge: "!",
            staged: false,
            classification: "conflicted",
            ...snapshot(entry.path)
        })
    }

    for (const section of GIT_SECTION_ORDER) {
        for (const row of buckets[section]) {
            indexById.set(gitChangeId(row), visualOrder.length)
            visualOrder.push(row)
        }
    }
    const model = { rows, buckets, visualOrder, rowById, indexById }
    modelCache.set(status, model)
    modelByRows.set(rows, model)
    return model
}

/** The one shared flat order used by both Local Changes surfaces. */
export function gitChangeRows(status: GitStatus | null): GitChangeRow[] {
    return buildGitChangeModel(status).rows
}

/** Visible sidebar order: conflicts → staged → unstaged → untracked. */
export function gitChangeSectionBuckets(rows: readonly GitChangeRow[]): GitChangeSectionBuckets {
    const cached = modelByRows.get(rows)
    if (cached) return cached.buckets
    const buckets = createBuckets()
    for (const row of rows) addToBuckets(row, buckets)
    return buckets
}

export function gitChangeVisualOrder(rows: readonly GitChangeRow[]): GitChangeRow[] {
    const cached = modelByRows.get(rows)
    if (cached) return cached.visualOrder
    const buckets = gitChangeSectionBuckets(rows)
    return GIT_SECTION_ORDER.flatMap((section) => buckets[section])
}

/** Visible sidebar rows after excluding collapsed section buckets. */
export function gitChangeVisibleOrder(
    rows: readonly GitChangeRow[],
    openSections: Readonly<Record<GitSectionKey, boolean>>
): GitChangeRow[] {
    const buckets = gitChangeSectionBuckets(rows)
    return GIT_SECTION_ORDER.flatMap((section) => openSections[section] ? buckets[section] : [])
}

/** Stable HTML/ARIA id that never embeds a raw Git path. */
export function gitChangeDomId(
    prefix: string,
    row: Pick<GitChangeKey, "path" | "staged">
): string {
    let encoded = ""
    for (const ch of row.path) {
        if (/[A-Za-z0-9.-]/.test(ch)) {
            encoded += ch
            continue
        }
        const bytes = new TextEncoder().encode(ch)
        for (const byte of bytes) {
            encoded += `_${byte.toString(16).padStart(2, "0")}`
        }
    }
    return `${prefix}-${row.staged ? "s" : "c"}-${encoded}`
}

export type SectionTriState = "unchecked" | "mixed" | "checked"

export function gitChangeIdSet(rows: readonly Pick<GitChangeKey, "path" | "staged">[]): Set<GitChangeId> {
    return new Set(rows.map(gitChangeId))
}

export function sectionSelectionState(
    section: readonly GitChangeKey[],
    selection: readonly GitChangeKey[] | ReadonlySet<GitChangeId>
): SectionTriState {
    if (section.length === 0) return "unchecked"
    const selectedIds: ReadonlySet<GitChangeId> = Array.isArray(selection)
        ? gitChangeIdSet(selection)
        : selection as ReadonlySet<GitChangeId>
    let selectedCount = 0
    for (const row of section) if (selectedIds.has(gitChangeId(row))) selectedCount += 1
    if (selectedCount === 0) return "unchecked"
    if (selectedCount === section.length) return "checked"
    return "mixed"
}

export function toggleSectionSelection<T extends GitChangeKey>(
    section: readonly T[],
    selection: readonly T[]
): T[] {
    const sectionIds = gitChangeIdSet(section)
    const selectedIds = gitChangeIdSet(selection)
    if (sectionSelectionState(section, selectedIds) === "checked") {
        return selection.filter((row) => !sectionIds.has(gitChangeId(row)))
    }
    const next = [...selection]
    for (const row of section) {
        const id = gitChangeId(row)
        if (!selectedIds.has(id)) {
            next.push(row)
            selectedIds.add(id)
        }
    }
    return next
}

export function selectedMutationSubsets(selection: readonly GitChangeRow[]): {
    conflicts: GitChangeRow[]
    stageable: GitChangeRow[]
    unstageable: GitChangeRow[]
    discardable: GitChangeRow[]
} {
    return {
        conflicts: selection.filter(isConflictChange),
        stageable: selection.filter(isStageableChange),
        unstageable: selection.filter(isUnstageableChange),
        discardable: selection.filter(isDiscardableChange)
    }
}

/** Exact command snapshot equality; deliberately stricter than selection identity. */
function sameGitChangeSnapshot(a: GitChangeKey, b: GitChangeKey): boolean {
    return sameGitChange(a, b)
        && a.classification === b.classification
        && a.stagedStatus === b.stagedStatus
        && a.unstagedStatus === b.unstagedStatus
        && a.origPath === b.origPath
}

function isRowArray(
    rows: readonly GitChangeRow[] | ReadonlyMap<GitChangeId, GitChangeRow>
): rows is readonly GitChangeRow[] {
    return Array.isArray(rows)
}

function rowMap(rows: readonly GitChangeRow[]): ReadonlyMap<GitChangeId, GitChangeRow> {
    const cached = modelByRows.get(rows)
    if (cached) return cached.rowById
    return new Map(rows.map((row) => [gitChangeId(row), row]))
}

export function currentGitChange(
    key: GitChangeKey,
    rows: readonly GitChangeRow[] | ReadonlyMap<GitChangeId, GitChangeRow>
): GitChangeRow | null {
    const byId = isRowArray(rows) ? rowMap(rows) : rows
    return byId.get(gitChangeId(key)) ?? null
}

export function currentGitChanges(
    keys: readonly GitChangeKey[],
    rows: readonly GitChangeRow[] | ReadonlyMap<GitChangeId, GitChangeRow>
): GitChangeRow[] {
    const byId = isRowArray(rows) ? rowMap(rows) : rows
    return keys.flatMap((key) => {
        const row = byId.get(gitChangeId(key))
        return row ? [row] : []
    })
}

export function exactGitChanges(
    keys: readonly GitChangeKey[],
    rows: readonly GitChangeRow[] | ReadonlyMap<GitChangeId, GitChangeRow>
): GitChangeRow[] {
    const byId = isRowArray(rows) ? rowMap(rows) : rows
    return keys.flatMap((key) => {
        const row = byId.get(gitChangeId(key))
        return row && sameGitChangeSnapshot(row, key) ? [row] : []
    })
}

export function uniquePaths(rows: readonly GitChangeKey[]): string[] {
    return [...new Set(rows.map((row) => row.path))]
}

/** Convert exact request snapshots to one backend target per path. */
export function rollbackTargetsFromKeys(keys: readonly GitChangeKey[]): GitRollbackTarget[] {
    const snapshots = new Map<string, GitChangeKey>()
    for (const key of keys) if (!snapshots.has(key.path)) snapshots.set(key.path, key)
    return [...snapshots.values()].map((key): GitRollbackTarget => {
        const { path } = key
        if (key.classification === "conflicted") {
            return { path, classification: { kind: "conflicted" } }
        }
        if (key.classification === "untracked") {
            return { path, classification: { kind: "untracked" } }
        }
        if (key.classification === "added") {
            return {
                path,
                classification: {
                    kind: "added",
                    stagedStatus: key.stagedStatus,
                    unstagedStatus: key.unstagedStatus
                }
            }
        }
        return {
            path,
            classification: {
                kind: "tracked",
                stagedStatus: key.stagedStatus,
                unstagedStatus: key.unstagedStatus,
                origPath: key.origPath
            }
        }
    })
}

export function isGitToggleModifier(event: { metaKey: boolean; ctrlKey: boolean }): boolean {
    const platform = typeof navigator === "undefined" ? "" : navigator.platform
    if (/Mac|iPhone|iPad|iPod/i.test(platform)) return event.metaKey
    return event.ctrlKey
}
