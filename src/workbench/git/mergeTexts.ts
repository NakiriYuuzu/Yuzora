import type { GitConflictSides, GradedText } from "@/lib/types"

export interface MergeTexts {
    base: string
    ours: string
    theirs: string
    crlf: boolean
}

function text(side: GradedText | null): string | null {
    return side && (side.kind === "full" || side.kind === "limited") ? side.content : null
}

/** Merge input, or the reason the file must be resolved by accepting a side. */
export function mergeTexts(sides: GitConflictSides): MergeTexts | "binary" | "deleted" {
    if (sides.ours === null || sides.theirs === null) return "deleted"
    const ours = text(sides.ours)
    const theirs = text(sides.theirs)
    // Both-added files have no base: everything differs from the empty text.
    const base = sides.base === null ? "" : text(sides.base)
    if (ours === null || theirs === null || base === null) return "binary"
    const worktree = text(sides.worktree) ?? ""
    return { base, ours, theirs, crlf: worktree.includes("\r\n") }
}
