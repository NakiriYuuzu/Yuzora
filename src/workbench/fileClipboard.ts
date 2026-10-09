import { logUserAction } from "@/features/logs/userAction"
import i18n from "@/lib/i18n"
import {
    clipboardReadFileList,
    clipboardWriteWorkspaceFiles,
    fsCopyPaths,
    fsMovePaths,
    fsPasteClipboardFiles
} from "@/lib/ipc"
import { canonicalPathKey, isSameOrDescendantPath, nativePathParent } from "@/lib/paths"
import { parseRemoteFilePath } from "@/lib/runtimeIdentity"
import { showAppMessage } from "@/state/appDialogStore"
import { retargetOpenDocuments } from "@/state/contextMenuStore"
import { useFileClipboardStore } from "@/state/fileClipboardStore"
import { useFileTreeStore } from "@/state/fileTreeStore"
import { useWorkspaceStore } from "@/state/workspaceStore"

const t = (key: string, options?: Record<string, unknown>) => i18n.t(key, { ns: "menus", ...options })

const KNOWN_ERRORS = [
    "copy-into-itself",
    "move-into-itself",
    "copy-too-large",
    "copy-symlink-unsupported",
    "copy-unsupported-sftp",
    "move-unsupported-sftp",
    "clipboard-files-unsupported",
    "clipboard-import-remote-unsupported"
]

async function reportError(error: unknown, title = t("fileClipboard.errorTitle")): Promise<void> {
    const message = String(error)
    const known = KNOWN_ERRORS.find((code) => message.includes(code))
    await showAppMessage({
        title,
        description: known ? t(`fileClipboard.error.${known}`) : message,
        kind: "error"
    })
}

function currentWorkspace(workspacePath: string): boolean {
    return useWorkspaceStore.getState().workspacePath === workspacePath
}

/** The folder a paste lands in: the clicked folder, the clicked file's folder, or the workspace root. */
export function pasteTargetDir(workspacePath: string, target: { path: string; isDirectory: boolean } | null): string {
    if (!target) return workspacePath
    if (target.isDirectory) return target.path
    const parent = nativePathParent(target.path)
    return parent && isSameOrDescendantPath(workspacePath, parent) ? parent : workspacePath
}

function samePaths(a: readonly string[], b: readonly string[]): boolean {
    if (a.length !== b.length) return false
    const keys = new Set(a.map((path) => canonicalPathKey(path)))
    return b.every((path) => keys.has(canonicalPathKey(path)))
}

/**
 * Copy or cut tree items. Local files also go onto the OS clipboard so Finder
 * or Explorer can paste them; that is best-effort and never blocks the copy.
 */
export async function copyFilesToClipboard(workspacePath: string, paths: string[], mode: "copy" | "cut"): Promise<void> {
    if (!paths.length || !currentWorkspace(workspacePath)) return
    useFileClipboardStore.getState().setClipboard({ workspacePath, paths, mode })
    void logUserAction(mode === "cut" ? "file_cut" : "file_copy", `${mode} ${paths.length} item(s)`)
    if (parseRemoteFilePath(workspacePath)) return
    await clipboardWriteWorkspaceFiles(workspacePath, paths).catch(() => undefined)
}

async function landed(workspacePath: string, targetDir: string, changed: string[]): Promise<void> {
    const fileTree = useFileTreeStore.getState()
    await fileTree.invalidatePaths(workspacePath, changed)
    if (!currentWorkspace(workspacePath)) return
    if (targetDir !== workspacePath && !useFileTreeStore.getState().trees[workspacePath]?.expandedDirs.has(targetDir)) {
        await fileTree.toggleDir(workspacePath, targetDir)
    }
}

/**
 * Paste into the folder of `target`. Sources, in order of precedence:
 * 1. this app's copy/cut while the OS clipboard still holds it (or holds nothing),
 * 2. files copied in Finder / Explorer (local workspaces),
 * 3. nothing to paste.
 */
export async function pasteFiles(
    workspacePath: string,
    target: { path: string; isDirectory: boolean } | null
): Promise<string[]> {
    if (!currentWorkspace(workspacePath)) return []
    const targetDir = pasteTargetDir(workspacePath, target)
    const remote = !!parseRemoteFilePath(workspacePath)
    const stored = useFileClipboardStore.getState().clipboard
    const internal = stored?.workspacePath === workspacePath ? stored : null
    const osPaths = remote ? [] : await clipboardReadFileList().catch(() => [] as string[])
    const useInternal = internal !== null && (osPaths.length === 0 || samePaths(osPaths, internal.paths))
    try {
        if (useInternal) {
            if (internal.mode === "cut") {
                const moved = await fsMovePaths(workspacePath, internal.paths, targetDir)
                internal.paths.forEach((from, index) => {
                    if (moved[index] && moved[index] !== from) retargetOpenDocuments(from, moved[index])
                })
                // A cut is pasted once.
                useFileClipboardStore.getState().setClipboard(null)
                await landed(workspacePath, targetDir, [...internal.paths, ...moved])
                void logUserAction("file_paste", `move ${moved.length} item(s)`)
                return moved
            }
            const created = await fsCopyPaths(workspacePath, internal.paths, targetDir)
            await landed(workspacePath, targetDir, created)
            void logUserAction("file_paste", `copy ${created.length} item(s)`)
            return created
        }
        if (osPaths.length) {
            if (remote) throw new Error("clipboard-import-remote-unsupported")
            const created = await fsPasteClipboardFiles(workspacePath, targetDir)
            await landed(workspacePath, targetDir, created)
            void logUserAction("file_paste", `import ${created.length} item(s)`)
            return created
        }
        return []
    } catch (error) {
        // A failed batch keeps the entries it finished; show them so a retry
        // is not mistaken for a first paste.
        const touched = useInternal && internal.mode === "cut" ? [targetDir, ...internal.paths] : [targetDir]
        await useFileTreeStore.getState().invalidatePaths(workspacePath, touched).catch(() => undefined)
        await reportError(error)
        return []
    }
}

/** Drag-and-drop move into `targetDir`. Unlike a cut-paste it never touches the clipboard store. */
export async function moveFilesTo(workspacePath: string, paths: string[], targetDir: string): Promise<string[]> {
    if (!paths.length || !currentWorkspace(workspacePath)) return []
    try {
        const moved = await fsMovePaths(workspacePath, paths, targetDir)
        paths.forEach((from, index) => {
            if (moved[index] && moved[index] !== from) retargetOpenDocuments(from, moved[index])
        })
        await landed(workspacePath, targetDir, [...paths, ...moved])
        void logUserAction("file_move", `move ${moved.length} item(s)`)
        return moved
    } catch (error) {
        await useFileTreeStore.getState().invalidatePaths(workspacePath, [targetDir, ...paths]).catch(() => undefined)
        await reportError(error, t("fileClipboard.moveErrorTitle"))
        return []
    }
}

/** JetBrains / VS Code "Duplicate": copy next to the original with a "copy" name. */
export async function duplicatePath(workspacePath: string, path: string): Promise<string[]> {
    if (!currentWorkspace(workspacePath)) return []
    const targetDir = pasteTargetDir(workspacePath, { path, isDirectory: false })
    try {
        const created = await fsCopyPaths(workspacePath, [path], targetDir)
        await landed(workspacePath, targetDir, created)
        return created
    } catch (error) {
        await reportError(error)
        return []
    }
}

/** True when the OS or this app holds something pasteable for the workspace. */
export function hasInternalClipboard(workspacePath: string): boolean {
    return useFileClipboardStore.getState().clipboard?.workspacePath === workspacePath
}
