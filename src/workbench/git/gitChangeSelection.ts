import type { GitRollbackTarget } from "@/lib/ipc"
import type { GitFileEntry, GitStatus } from "@/lib/types"

import { badgeChar } from "./fileRows"

export type GitChangeClassification = "tracked" | "added" | "untracked" | "conflicted"

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

/** The one shared flat order used by both Local Changes surfaces. */
export function gitChangeRows(status: GitStatus | null): GitChangeRow[] {
    if (!status) return []
    const staged = byPath(status.staged)
    const unstaged = byPath(status.unstaged)
    const untracked = new Set(status.untracked)
    const conflicted = new Set(status.conflicted.map((entry) => entry.path))
    const snapshot = (path: string) => ({
        stagedStatus: staged.get(path)?.status ?? null,
        unstagedStatus: unstaged.get(path)?.status ?? null,
        origPath: staged.get(path)?.origPath ?? unstaged.get(path)?.origPath ?? null
    })
    const classify = (path: string): GitChangeClassification => {
        const base = classificationFor(path, staged, untracked, conflicted)
        return base === "tracked" && unstaged.get(path)?.status === "A" ? "added" : base
    }
    return [
        ...status.staged.map((entry) => ({
            path: entry.path,
            badge: badgeChar(entry.status),
            staged: true,
            classification: classify(entry.path),
            ...snapshot(entry.path)
        })),
        ...status.unstaged.map((entry) => ({
            path: entry.path,
            badge: badgeChar(entry.status),
            staged: false,
            classification: classify(entry.path),
            ...snapshot(entry.path)
        })),
        ...status.untracked.map((path) => ({
            path,
            badge: "?",
            staged: false,
            classification: "untracked" as const,
            ...snapshot(path)
        })),
        ...status.conflicted.map((entry) => ({
            path: entry.path,
            badge: "!",
            staged: false,
            classification: "conflicted" as const,
            ...snapshot(entry.path)
        }))
    ]
}

export function sameGitChange(a: GitChangeKey | null, b: GitChangeKey | null): boolean {
    return Boolean(a && b && a.path === b.path && a.staged === b.staged)
}

/** Exact command snapshot equality; deliberately stricter than selection identity. */
export function sameGitChangeSnapshot(a: GitChangeKey, b: GitChangeKey): boolean {
    return sameGitChange(a, b)
        && a.classification === b.classification
        && a.stagedStatus === b.stagedStatus
        && a.unstagedStatus === b.unstagedStatus
        && a.origPath === b.origPath
}

export function currentGitChange(
    key: GitChangeKey,
    rows: readonly GitChangeRow[]
): GitChangeRow | null {
    return rows.find((row) => sameGitChange(row, key)) ?? null
}

export function currentGitChanges(
    keys: readonly GitChangeKey[],
    rows: readonly GitChangeRow[]
): GitChangeRow[] {
    return keys.flatMap((key) => {
        const row = currentGitChange(key, rows)
        return row ? [row] : []
    })
}

export function exactGitChanges(
    keys: readonly GitChangeKey[],
    rows: readonly GitChangeRow[]
): GitChangeRow[] {
    return keys.flatMap((key) => {
        const row = currentGitChange(key, rows)
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
