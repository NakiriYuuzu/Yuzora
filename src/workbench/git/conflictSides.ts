export type SideChange = "modified" | "added" | "deleted"

/** Porcelain unmerged XY: X describes ours, Y theirs (U modified, A added, D deleted). */
export function conflictSideChanges(code: string): { ours: SideChange; theirs: SideChange } {
    const side = (letter: string | undefined): SideChange => letter === "A" ? "added" : letter === "D" ? "deleted" : "modified"
    return { ours: side(code[0]), theirs: side(code[1]) }
}

/** The merge tool needs text on both sides; a deletion can only be accepted or rejected. */
export function canMergeConflict(code: string): boolean {
    const { ours, theirs } = conflictSideChanges(code)
    return ours !== "deleted" && theirs !== "deleted"
}
