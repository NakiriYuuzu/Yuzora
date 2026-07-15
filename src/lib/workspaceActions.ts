import { open } from "@tauri-apps/plugin-dialog"

import { clearAll } from "@/editor/documentRegistry"
import { saveDirtyTab } from "@/editor/saveDocument"
import { logUserAction } from "@/features/logs/userAction"
import i18n from "@/lib/i18n"
import { allowWorkspaceAssetScope, openWorkspace, startWatch } from "@/lib/ipc"
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
async function openWorkspaceAtPathWithOutcome(path: string): Promise<boolean> {
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
        if (decision === "cancel") return false
        if (decision === "save") {
            for (const dirtyPath of dirtyPaths) {
                const outcome = await saveDirtyTab(dirtyPath)
                if (outcome.kind !== "saved") return false
            }
        }
    }

    const canonical = await openWorkspace(path)
    clearAll()
    useWorkspaceStore.getState().setWorkspace(canonical)
    // Image tabs load through the asset protocol; await the grant so restored
    // image tabs never race it, but a grant failure must not block opening the
    // workspace — affected images surface a load error instead.
    await allowWorkspaceAssetScope(canonical).catch((err) => {
        console.warn("allow_workspace_asset_scope failed:", err)
    })
    void startWatch(canonical)
    void logUserAction("open_workspace", `open workspace ${canonical}`)
    useRecentWorkspacesStore.getState().record(canonical)
    return true
}

export async function openWorkspaceAtPath(path: string): Promise<void> {
    await openWorkspaceAtPathWithOutcome(path)
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
    return openWorkspaceAtPathWithOutcome(selected)
}
