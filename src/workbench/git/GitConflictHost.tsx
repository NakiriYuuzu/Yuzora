import { GitConflictsDialog } from "./GitConflictsDialog"
import { GitMergeTool } from "./GitMergeTool"
import { GitResetDialog } from "./GitResetDialog"
import { GitStashDialog } from "./GitStashDialog"

/** Mounts the Git operation dialogs (conflicts, merge tool, stash, reset) once. */
export function GitConflictHost() {
    return (
        <>
            <GitConflictsDialog />
            <GitMergeTool />
            <GitStashDialog />
            <GitResetDialog />
        </>
    )
}
