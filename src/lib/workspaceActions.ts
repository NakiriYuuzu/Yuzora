import { open } from "@tauri-apps/plugin-dialog"

import { clearAll } from "@/editor/documentRegistry"
import { saveDirtyTab } from "@/editor/saveDocument"
import { logUserAction } from "@/features/logs/userAction"
import i18n from "@/lib/i18n"
import { openWorkspace, startWatch } from "@/lib/ipc"
import { useConfirmDialogStore } from "@/state/confirmDialogStore"
import { useRecentWorkspacesStore } from "@/state/recentWorkspaces"
import { useWorkspaceStore } from "@/state/workspaceStore"

/**
 * Opens `path` as the active workspace: canonicalizes it through the backend,
 * drops cached document content from any previous workspace so it can't leak
 * into the new one, starts the fs watcher, and records the workspace in the
 * recent-workspaces MRU list. Shared by the Files panel empty state and the
 * workspace rail's Open/Recent popover. Rejects (backend error, e.g. the
 * folder was moved or deleted) without recording anything.
 */
export async function openWorkspaceAtPath(path: string): Promise<void> {
    // Guard unsaved work before discarding the current workspace's buffers.
    // Restore-on-launch runs with no workspace and no tabs open (SessionRestore
    // only fires when workspacePath is null), so there are never dirty tabs then
    // and this is naturally skipped — no modal on auto-restore.
    const dirtyPaths = [
        ...new Set(
            useWorkspaceStore
                .getState()
                .groups.flatMap((g) => g.tabs)
                .filter((tab) => tab.kind !== "preview" && tab.dirty)
                .map((tab) => tab.path)
        )
    ]
    if (dirtyPaths.length > 0) {
        const decision = await useConfirmDialogStore.getState().requestUnsavedDecision({
            title: i18n.t("unsavedDialog.switchWorkspaceTitle", { ns: "menus" }),
            description: i18n.t("unsavedDialog.switchWorkspaceDescription", { ns: "menus" }),
            saveLabel: i18n.t("unsavedDialog.saveAll", { ns: "menus" })
        })
        if (decision === "cancel") return
        if (decision === "save") await Promise.all(dirtyPaths.map((p) => saveDirtyTab(p)))
    }

    const canonical = await openWorkspace(path)
    clearAll()
    useWorkspaceStore.getState().setWorkspace(canonical)
    void startWatch(canonical)
    void logUserAction("open_workspace", `open workspace ${canonical}`)
    useRecentWorkspacesStore.getState().record(canonical)
}

/**
 * Opens the native directory picker and, if a folder was chosen, opens it as
 * the workspace. Returns whether a workspace was actually opened (false when
 * the user cancels the picker) so callers can decide whether to close any
 * surrounding UI (e.g. a popover).
 */
export async function pickWorkspace(): Promise<boolean> {
    const selected = await open({ directory: true, multiple: false })
    if (typeof selected !== "string") return false
    await openWorkspaceAtPath(selected)
    return true
}
