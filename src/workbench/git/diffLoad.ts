import { gitDiffContent, gitFileAtRev } from "@/lib/ipc"
import { lineDiffCounts } from "@/lib/lineDiff"
import { languageFromPath } from "@/lib/types"
import type { CommitFileChange, DiffContent, FileAtRevResult, GradedText } from "@/lib/types"

// Shared diff-loading + header helpers for the two diff surfaces (Local changes
// tab and the Diff modal). Extracted from LocalChangesTab so both compute the
// header language label / add-delete stats and load worktree text the same way,
// and so the modal can load commit-side text without duplicating the mapping.

// name + dir split for a file-row label (dir shown as a dimmed suffix, §2.5 L883).
export function splitPath(path: string): { name: string; dir: string } {
    const idx = path.lastIndexOf("/")
    if (idx < 0) return { name: path, dir: "" }
    return { name: path.slice(idx + 1), dir: path.slice(0, idx + 1) }
}

// §2.5 L897 diff-header language label. Reuse languageFromPath (types.ts) for the
// known extensions; for anything it maps to "Plain Text", fall back to the
// uppercased extension so e.g. `foo.lock` reads "LOCK" and extensionless files
// show nothing.
export function langLabel(path: string): string {
    const known = languageFromPath(path)
    if (known !== "Plain Text") return known
    const dot = path.lastIndexOf(".")
    const slash = path.lastIndexOf("/")
    if (dot <= slash + 1) return ""
    return path.slice(dot + 1).toUpperCase()
}

// tooLarge/binary sides carry no content, so per-line stats can't be computed;
// callers treat that as "no stats" (large/binary don't show +N/−N).
function docOrNull(side: GradedText): string | null {
    return "content" in side ? side.content : null
}

// §2.5 L898-899 header stats. Null when either side is undisplayable.
export function diffStats(diff: DiffContent): { added: number; deleted: number } | null {
    const original = docOrNull(diff.original)
    const modified = docOrNull(diff.modified)
    if (original == null || modified == null) return null
    return lineDiffCounts(original, modified)
}

// Worktree diff (§2.5): same mechanism the Local changes tab uses. `staged`
// selects which side (staged↔working) git diffs.
export function loadWorktreeDiff(path: string, staged: boolean): Promise<DiffContent> {
    return gitDiffContent(path, staged)
}

// A FileAtRevResult carries content only for full/limited; the tooLarge/binary
// kinds map straight onto the matching GradedText kind. `missing` (rev has no
// such file — normal for the added side of an A file or deleted side of a D
// file) is treated as empty text so the diff shows a clean add/delete.
function revToGraded(res: FileAtRevResult): GradedText {
    switch (res.kind) {
        case "full":
            return { kind: "full", content: res.content }
        case "limited":
            return { kind: "limited", content: res.content }
        case "tooLarge":
            return { kind: "tooLarge" }
        case "binary":
            return { kind: "binary" }
        case "missing":
            return { kind: "full", content: "" }
    }
}

// Commit-file diff. old = the file at the first parent (root commit → no parent
// → empty text); new = the file at this commit. Rename/copy resolves the old
// side against `oldPath` when present. Both sides load in parallel.
export async function loadCommitDiff(
    hash: string,
    parents: string[],
    file: CommitFileChange
): Promise<DiffContent> {
    const oldPath = file.oldPath ?? file.path
    const parent = parents[0]
    const [original, modified] = await Promise.all([
        parent
            ? gitFileAtRev(parent, oldPath).then(revToGraded)
            : Promise.resolve<GradedText>({ kind: "full", content: "" }),
        gitFileAtRev(hash, file.path).then(revToGraded)
    ])
    return { original, modified }
}
