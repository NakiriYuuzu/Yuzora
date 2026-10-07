import type { GitConflictSides, GradedText } from "@/lib/types"

export interface MergeTexts {
    base: string
    ours: string
    theirs: string
    crlf: boolean
    /** The working file as Git left it (with conflict markers), when readable. */
    worktree: string | null
}

export function readableText(side: GradedText | null): string | null {
    return side && (side.kind === "full" || side.kind === "limited") ? side.content : null
}

/** Conflict blocks still marked in a working file (`<<<<<<<` lines). */
export function conflictMarkerCount(content: string): number {
    return content.match(/^<{7}(?:[ \r]|$)/gm)?.length ?? 0
}

/** Merge input, or the reason the file must be resolved by accepting a side. */
export function mergeTexts(sides: GitConflictSides): MergeTexts | "binary" | "deleted" {
    if (sides.ours === null || sides.theirs === null) return "deleted"
    const ours = readableText(sides.ours)
    const theirs = readableText(sides.theirs)
    // Both-added files have no base: everything differs from the empty text.
    const base = sides.base === null ? "" : readableText(sides.base)
    if (ours === null || theirs === null || base === null) return "binary"
    const worktree = readableText(sides.worktree)
    return { base, ours, theirs, crlf: (worktree ?? "").includes("\r\n"), worktree }
}
