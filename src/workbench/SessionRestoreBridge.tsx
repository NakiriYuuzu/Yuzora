import { useEffect, useRef } from "react"

import { getDocument } from "@/editor/documentRegistry"
import { allowWorkspaceAssetScope } from "@/lib/ipc"
import { dismissSplash } from "@/lib/splash"
import { openWorkspaceAtPath } from "@/lib/workspaceActions"
import { isImagePath } from "@/workbench/ImageView"
import {
    clearWorkspaceSession,
    loadWorkspaceSession,
    markWorkspaceSessionActive,
    saveWorkspaceSession
} from "@/state/workspaceSession"
import { PREVIEW_TAB_PATH, useWorkspaceStore } from "@/state/workspaceStore"

/**
 * Restores the last workspace + first-editor-group tabs on launch, then keeps
 * the persisted session in sync as tabs change.
 *
 * The two effects race by construction: the restore effect opens the workspace
 * asynchronously, during which the store passes through an empty state (the
 * openWorkspaceAtPath → setWorkspace resets groups). Without a gate the save
 * subscription would fire on that empty state and clobber the session we just
 * read. `restoredRef` blocks every save until the restore attempt fully
 * settles, so the on-disk session survives the async gap.
 */
export function SessionRestoreBridge() {
    const restoredRef = useRef(false)

    // Restore effect (mount-once): only when no workspace is open yet.
    useEffect(() => {
        const store = useWorkspaceStore.getState()
        const session = loadWorkspaceSession()
        if (store.workspacePath || !session) {
            // Nothing to restore — open the save gate immediately.
            restoredRef.current = true
            dismissSplash()
            return
        }

        let cancelled = false
        // Count distinct workspace opens during restore. Our own
        // openWorkspaceAtPath accounts for exactly one; a second means the user
        // opened a workspace themselves during the canonicalize round-trip, so we
        // must not overwrite their choice with the restored tabs.
        let workspaceOpens = 0
        const unsubscribeGuard = useWorkspaceStore.subscribe((state, prev) => {
            if (state.workspacePath && state.workspacePath !== prev.workspacePath) {
                workspaceOpens += 1
            }
        })
        void (async () => {
            try {
                // restoreSessionTabs: false — 冷啟還原由下面的迴圈逐檔驗證
                // （getDocument 失敗＝檔案已消失，靜默略過）後才開分頁；
                // workspaceActions 的 map 還原不做驗證，必須關掉以免失效
                // 檔案的分頁被搶先開出來（#60 T4c）。
                await openWorkspaceAtPath(session.workspacePath, { restoreSessionTabs: false })
                if (cancelled || workspaceOpens > 1) return
                const ws = useWorkspaceStore.getState()
                // T4 覆核修正（NB-1 回歸）：冷啟還原的圖片分頁同樣不得與 asset
                // scope grant 競速——openWorkspaceAtPath 已 fire-and-forget 發過
                // grant，這裡對 canonical 路徑再等一趟 idempotent µs 級 command，
                // 確保 ImageView 的 <img> 請求不會 403 進永久 loadError。失敗
                // 靜默：圖片分頁屆時顯示載入錯誤。
                if (session.tabs.some(isImagePath)) {
                    await allowWorkspaceAssetScope(
                        ws.workspacePath ?? session.workspacePath
                    ).catch(() => {})
                    if (cancelled || workspaceOpens > 1) return
                }
                const opened: string[] = []
                for (const path of session.tabs) {
                    try {
                        // Warm the document cache and confirm the file still
                        // exists; a since-deleted file rejects and is skipped.
                        await getDocument(path)
                        if (cancelled) return
                        ws.openTab(path)
                        opened.push(path)
                    } catch {
                        // File gone — silently skip this tab.
                    }
                }
                if (session.activePath && opened.includes(session.activePath)) {
                    ws.setActiveTab(0, session.activePath)
                }
            } catch {
                // Workspace folder moved/deleted — drop the stale session.
                clearWorkspaceSession()
            } finally {
                unsubscribeGuard()
                if (!cancelled) restoredRef.current = true
                // The splash lives exactly as long as the restore attempt —
                // success, workspace-gone and cancellation all release it.
                dismissSplash()
            }
        })()

        return () => {
            cancelled = true
            unsubscribeGuard()
        }
    }, [])

    // Save effect: mirror the first group's real-file tabs into the current
    // workspace's session entry on every relevant workspace-store change, once
    // the restore gate is open.
    useEffect(() => {
        return useWorkspaceStore.subscribe((state, prev) => {
            if (!restoredRef.current) return
            const workspacePath = state.workspacePath
            if (!workspacePath) return
            if (workspacePath !== prev.workspacePath) {
                // Workspace switch: the store passes through an empty-groups
                // state here (setWorkspace resets groups before workspaceActions
                // restores the target's tabs), so persisting the entry now would
                // clobber the saved tabs we're about to restore. Only advance
                // the last-workspace pointer for the next cold-start restore;
                // the restored/edited tabs re-save via later group changes.
                markWorkspaceSessionActive(workspacePath)
                return
            }
            if (state.groups[0] === prev.groups[0]) return
            const group = state.groups[0]
            const tabs = group.tabs
                .filter((tab) => tab.path !== PREVIEW_TAB_PATH)
                .map((tab) => tab.path)
            const activePath =
                group.activePath && group.activePath !== PREVIEW_TAB_PATH
                    ? group.activePath
                    : null
            saveWorkspaceSession({ workspacePath, tabs, activePath })
        })
    }, [])

    return null
}
