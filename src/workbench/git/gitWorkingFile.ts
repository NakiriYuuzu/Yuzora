import type { GitChangeTarget } from "@/app/workbench/contextMenuModel"
import { isWindowsPath, nativePathJoin } from "@/lib/paths"
import { useGitStore } from "@/state/gitStore"
import { useUiStore } from "@/state/uiStore"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { exactGitChanges, gitChangeRows } from "./gitChangeSelection"

/** Resolve only a current changed-file target, never a historical revision. */
export function gitWorkingFilePath(repositoryRoot: string, clicked: GitChangeTarget): string | null {
    const state = useGitStore.getState()
    if (state.environment?.status !== "ready" || state.environment.root !== repositoryRoot || state.snapshotStale) return null
    const [row] = exactGitChanges([clicked], gitChangeRows(state.status))
    if (!row || row.stagedStatus === "D" || row.unstagedStatus === "D") return null
    if (!row.path || row.path.startsWith("/") || row.path.split("/").some((part) => part === ".." || part === ".")) return null
    return nativePathJoin(repositoryRoot, isWindowsPath(repositoryRoot) ? row.path.replace(/\//g, "\\") : row.path)
}

export function openGitWorkingFile(repositoryRoot: string, clicked: GitChangeTarget): boolean {
    const path = gitWorkingFilePath(repositoryRoot, clicked)
    if (!path) return false
    useWorkspaceStore.getState().openTab(path)
    useUiStore.getState().setMode("files")
    return true
}
