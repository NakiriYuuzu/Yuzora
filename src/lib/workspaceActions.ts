import { chooseWorkspaceFolder } from "@/state/folderPickerStore"

import { clearAll } from "@/editor/documentRegistry"
import { logUserAction } from "@/features/logs/userAction"
import i18n from "@/lib/i18n"
import { allowWorkspaceAssetScope, openWorkspace, startWatch } from "@/lib/ipc"
import { confirmDiscardingUnsaved } from "@/lib/unsavedGuard"
import { useRecentWorkspacesStore } from "@/state/recentWorkspaces"
import { loadWorkspaceSessionEntry } from "@/state/workspaceSession"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { isImagePath } from "@/workbench/ImageView"

export interface OpenWorkspaceOptions {
    /** Drop a delayed restore before it replaces a newer user workspace. */
    shouldOpen?: () => boolean
    // #60 T4c：預設從 per-workspace session map 立即還原 tabs。冷啟還原
    // （SessionRestoreBridge）自己帶逐檔存在性驗證再開分頁，傳 false 關掉
    // 這裡的還原，避免失效檔案的分頁被搶先開出來。
    restoreSessionTabs?: boolean
    /**
     * When true, skip the unsaved-documents dialog. Only for callers that have
     * already completed an unsaved preflight in the same transaction.
     */
    skipUnsavedGuard?: boolean
}

/**
 * Opens `path` as the active workspace: canonicalizes it through the backend,
 * drops cached document content from any previous workspace so it can't leak
 * into the new one, restores the workspace's recorded editor tabs, starts the
 * fs watcher, and records the workspace in the recent-workspaces MRU list.
 * Shared by the Files panel empty state and the workspace rail's Open/Recent
 * popover. Rejects (backend error, e.g. the folder was moved or deleted)
 * without recording anything.
 */
async function openWorkspaceAtPathWithOutcome(
    path: string,
    options?: OpenWorkspaceOptions
): Promise<boolean> {
    if (options?.shouldOpen && !options.shouldOpen()) return false
    // Guard unsaved work before discarding the current workspace's buffers.
    // Restore-on-launch runs with no workspace and no tabs open (SessionRestore
    // only fires when workspacePath is null), so there are never dirty tabs then
    // and this is naturally skipped — no modal on auto-restore.
    if (!options?.skipUnsavedGuard) {
        const proceed = await confirmDiscardingUnsaved({
            title: i18n.t("unsavedDialog.switchWorkspaceTitle", { ns: "menus" }),
            description: i18n.t("unsavedDialog.switchWorkspaceDescription", { ns: "menus" }),
            saveLabel: i18n.t("unsavedDialog.saveAll", { ns: "menus" })
        })
        if (!proceed) return false
    }

    const opened = await openWorkspace(path)
    if (options?.shouldOpen && !options.shouldOpen()) return false
    const canonical = opened.canonicalPath
    // #60 T4c：切回曾開過的 workspace 要還原它的 tabs。entry 必須在
    // setWorkspace 之前讀出——SessionRestoreBridge 的存檔訂閱會對 store 轉場
    // 做出反應，先讀確保不受任何寫入競態影響。
    const sessionEntry =
        options?.restoreSessionTabs === false ? null : loadWorkspaceSessionEntry(canonical)
    // Grant image access before publishing a restored workspace. Waiting after
    // setWorkspace would leave a half-switched UI if a newer selection wins.
    const assetScopeGrant = allowWorkspaceAssetScope(canonical).catch((err) => {
        console.warn("allow_workspace_asset_scope failed:", err)
    })
    if (sessionEntry?.tabs.some((tabPath) => isImagePath(tabPath))) {
        await assetScopeGrant
    }
    if (options?.shouldOpen && !options.shouldOpen()) return false
    clearAll()
    const workspace = useWorkspaceStore.getState()
    workspace.setWorkspace(canonical, opened.capabilityId)
    if (sessionEntry) {
        // 立即還原 tabs 與 active tab（切回 <100ms 顯示）；檔案內容由
        // EditorPane 掛載時經 documentRegistry / async open_file 背景載入，
        // 不在這裡 await（unsavedGuard 已保證切換時沒有 dirty buffer）。
        for (const tabPath of sessionEntry.tabs) {
            workspace.openTab(tabPath, 0)
            if (sessionEntry.pinnedPaths?.includes(tabPath)) workspace.toggleTabPinned(0, tabPath)
        }
        if (sessionEntry.activePath && sessionEntry.tabs.includes(sessionEntry.activePath)) {
            workspace.setActiveTab(0, sessionEntry.activePath)
        }
    }
    void startWatch(canonical).catch((error) => console.warn("workspace watcher failed", error))
    void logUserAction("open_workspace", `open workspace ${canonical}`)
    useRecentWorkspacesStore.getState().record(canonical)
    return true
}

export async function openWorkspaceAtPath(
    path: string,
    options?: OpenWorkspaceOptions
): Promise<boolean> {
    return openWorkspaceAtPathWithOutcome(path, options)
}

/**
 * Opens the native directory picker and, if a folder was chosen, opens it as
 * the workspace. Returns whether a workspace was actually opened (false when
 * the user cancels the picker) so callers can decide whether to close any
 * surrounding UI (e.g. a popover).
 */
export async function pickWorkspace(): Promise<boolean> {
    const selected = await chooseWorkspaceFolder()
    if (typeof selected !== "string") return false
    return openWorkspaceAtPathWithOutcome(selected)
}

export async function pickRemoteWorkspace(): Promise<boolean> {
    const selected = await chooseWorkspaceFolder({ initialLocation: "remote" })
    return typeof selected === "string" && openWorkspaceAtPathWithOutcome(selected)
}
