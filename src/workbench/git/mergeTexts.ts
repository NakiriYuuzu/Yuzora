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
export function mergeTexts(sides: GitConflictSides): MergeTexts | "binary" | "deleted" | "encoding" | "symlink" {
    // A link's text is its target: saving a merge would write through the link.
    if (sides.symlink) return "symlink"
    if (sides.ours === null || sides.theirs === null) return "deleted"
    const ours = readableText(sides.ours)
    const theirs = readableText(sides.theirs)
    // Both-added files have no base: everything differs from the empty text.
    const base = sides.base === null ? "" : readableText(sides.base)
    if (ours === null || theirs === null || base === null) return "binary"
    // The result is written back as UTF-8 text: only lossless for UTF-8 sources.
    if (sides.lossyEncoding) return "encoding"
    const worktree = readableText(sides.worktree)
    // Model offsets and inserted lengths must use CodeMirror's logical LF
    // document coordinates. Keep the worktree raw for save/overwrite checks.
    return {
        base: base.replace(/\r\n?/g, "\n"),
        ours: ours.replace(/\r\n?/g, "\n"),
        theirs: theirs.replace(/\r\n?/g, "\n"),
        crlf: (worktree ?? "").includes("\r\n"),
        worktree
    }
}
