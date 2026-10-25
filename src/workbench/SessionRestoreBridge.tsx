import { useEffect, useRef } from "react"

import { documentWorkspaceEpoch, getDocument } from "@/editor/documentRegistry"
import { allowWorkspaceAssetScope } from "@/lib/ipc"
import { launchOpenRequests, openRequestBatchesAccepted } from "@/lib/openRequests"
import { dismissSplash } from "@/lib/splash"
import { openWorkspaceAtPath } from "@/lib/workspaceActions"
import { isImagePath } from "@/workbench/ImageView"
import { isHerdrPagePath } from "@/lib/herdrPages"
import { parseRemoteFilePath } from "@/lib/runtimeIdentity"
import { useHostStore } from "@/state/hostStore"
import { useSshStore } from "@/state/sshStore"
import { loadRemoteWorkspaces } from "@/state/remoteWorkspaceRegistry"
import {
    clearWorkspaceSession,
    isPersistableSessionPath,
    loadWorkspaceSession,
    markWorkspaceSessionActive,
    saveWorkspaceSession,
    type WorkspaceSession
} from "@/state/workspaceSession"
import { PREVIEW_TAB_PATH, useWorkspaceStore } from "@/state/workspaceStore"

function persistEditorSession(state: ReturnType<typeof useWorkspaceStore.getState>): void {
    const workspacePath = state.workspacePath
    const group = state.groups[0]
    if (!workspacePath || !group) return
    const tabs = group.tabs
        .filter((tab) => {
            if (tab.kind === "preview" || tab.kind === "markdown-preview" || tab.kind === "herdr-terminal") return false
            if (tab.path === PREVIEW_TAB_PATH || isHerdrPagePath(tab.path)) return false
            return isPersistableSessionPath(tab.path)
        })
        .map((tab) => tab.path)
    const activePath = group.activePath && isPersistableSessionPath(group.activePath)
        ? group.activePath : null
    saveWorkspaceSession({ workspacePath, tabs, activePath, pinnedPaths: group.tabs.filter((tab) => tab.pinned && tabs.includes(tab.path)).map((tab) => tab.path) })
}

/** Nothing to restore: open the save gate immediately and release the splash. */
function openRestoreGate(restoredRef: { current: boolean }): void {
    restoredRef.current = true
    useWorkspaceStore.getState().markSessionRestoreReady()
    dismissSplash()
}

/** Restores `session` and returns the cleanup that cancels it. */
function restoreSession(session: WorkspaceSession, restoredRef: { current: boolean }): () => void {
    const remote = parseRemoteFilePath(session.workspacePath)
    if (remote) {
        // A network connection never owns the startup splash. Subscribe to
        // this exact host, then restore once per connection generation.
        restoredRef.current = true
        useWorkspaceStore.getState().markSessionRestoreReady()
        dismissSplash()
        let cancelled = false
        let busy = false
        let attempted: string | null = null
        // Files the OS opens meanwhile do not set a workspace, so the restore
        // would replace their tabs; once the user opened something, it yields.
        const openRequestBatches = openRequestBatchesAccepted()
        const superseded = () => useWorkspaceStore.getState().workspacePath !== null
            || openRequestBatchesAccepted() !== openRequestBatches
        const access = loadRemoteWorkspaces()[session.workspacePath]
        const connectionKey = () => {
            if (access === "runtime") {
                const owner = useHostStore.getState().hosts[remote.hostId]?.connection?.owner
                return owner ? `runtime:${owner.generation}` : null
            }
            if (access === "sftp") return useSshStore.getState().sessions[remote.hostId]?.sessionId ?? null
            return null
        }
        const retry = () => {
            const key = connectionKey()
            if (cancelled || busy || !key || attempted === key || superseded()) return
            attempted = key
            busy = true
            void openWorkspaceAtPath(session.workspacePath, {
                // Keep recorded remote tabs even when one file cannot be
                // read yet. The editor shows its source error on demand.
                shouldOpen: () => !cancelled && connectionKey() === key && !superseded()
            }).catch((error) => console.warn("remote session restore deferred", error)).finally(() => {
                busy = false
                retry()
            })
        }
        const stopRuntime = useHostStore.subscribe(retry)
        const stopSsh = useSshStore.subscribe(retry)
        retry()
        return () => { cancelled = true; stopRuntime(); stopSsh() }
    }

    let cancelled = false
    const initialEpoch = documentWorkspaceEpoch()
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
    let isCurrent = () => !cancelled && workspaceOpens === 0
        && documentWorkspaceEpoch() === initialEpoch && !useWorkspaceStore.getState().workspacePath
    let preserveUserSelection = false
    void (async () => {
        try {
            // restoreSessionTabs: false — 冷啟還原由下面的迴圈逐檔驗證
            // （getDocument 失敗＝檔案已消失，靜默略過）後才開分頁；
            // workspaceActions 的 map 還原不做驗證，必須關掉以免失效
            // 檔案的分頁被搶先開出來（#60 T4c）。
            const didOpen = await openWorkspaceAtPath(session.workspacePath, {
                restoreSessionTabs: false, shouldOpen: () => isCurrent()
            })
            if (!didOpen || cancelled || workspaceOpens > 1) return
            const ws = useWorkspaceStore.getState()
            const epoch = documentWorkspaceEpoch()
            const groupId = ws.groups[0]?.id
            let activeGroupIndex = ws.activeGroupIndex
            let activePath = ws.groups[0]?.activePath
            isCurrent = () => {
                const current = useWorkspaceStore.getState()
                return !cancelled && workspaceOpens <= 1 && documentWorkspaceEpoch() === epoch
                    && current.workspacePath === ws.workspacePath && current.workspaceCapabilityId === ws.workspaceCapabilityId
                    && current.groups[0]?.id === groupId
            }
            // T4 覆核修正（NB-1 回歸）：冷啟還原的圖片分頁同樣不得與 asset
            // scope grant 競速——openWorkspaceAtPath 已 fire-and-forget 發過
            // grant，這裡對 canonical 路徑再等一趟 idempotent µs 級 command，
            // 確保 ImageView 的 <img> 請求不會 403 進永久 loadError。失敗
            // 靜默：圖片分頁屆時顯示載入錯誤。
            if (session.tabs.some(isImagePath)) {
                await allowWorkspaceAssetScope(
                    ws.workspacePath ?? session.workspacePath
                ).catch(() => {})
                if (!isCurrent()) return
            }
            const opened: string[] = []
            for (const path of session.tabs) {
                if (!isCurrent()) return
                // Defense in depth: never restore preview / Herdr pseudo paths.
                if (!isPersistableSessionPath(path)) continue
                try {
                    // Warm the document cache and confirm the file still
                    // exists; a since-deleted file rejects and is skipped.
                    await getDocument(path)
                    if (!isCurrent()) return
                    const before = useWorkspaceStore.getState()
                    preserveUserSelection ||= before.activeGroupIndex !== activeGroupIndex
                        || before.groups[0]?.activePath !== activePath
                    ws.openTab(path, 0)
                    if (session.pinnedPaths?.includes(path)) ws.toggleTabPinned(0, path)
                    if (preserveUserSelection) {
                        const previous = before.groups[before.activeGroupIndex]?.activePath
                        if (previous) ws.setActiveTab(before.activeGroupIndex, previous)
                        else ws.setActiveGroup(before.activeGroupIndex)
                    }
                    opened.push(path)
                    activeGroupIndex = useWorkspaceStore.getState().activeGroupIndex
                    activePath = useWorkspaceStore.getState().groups[0]?.activePath
                } catch {
                    // File gone — silently skip this tab.
                }
            }
            // The last read may reject after the user has chosen another
            // tab or split, without reaching the success-path check above.
            const current = useWorkspaceStore.getState()
            preserveUserSelection ||= current.activeGroupIndex !== activeGroupIndex
                || current.groups[0]?.activePath !== activePath
            if (isCurrent() && !preserveUserSelection && session.activePath && opened.includes(session.activePath)) {
                ws.setActiveTab(0, session.activePath)
                activeGroupIndex = useWorkspaceStore.getState().activeGroupIndex
                activePath = useWorkspaceStore.getState().groups[0]?.activePath
            }
        } catch {
            // A remote host can still be connecting during startup. Keep
            // its tabs for a later reopen; absence of a connection is not
            // evidence that the source folder was deleted.
            if (isCurrent() && !parseRemoteFilePath(session.workspacePath)) clearWorkspaceSession()
        } finally {
            unsubscribeGuard()
            if (!cancelled) {
                restoredRef.current = true
                const current = useWorkspaceStore.getState()
                // A shortcut can replace the restore while the splash is
                // still up. Persist that selection once the save gate opens.
                if (!isCurrent() || preserveUserSelection) persistEditorSession(current)
                current.markSessionRestoreReady()
            }
            // The splash lives exactly as long as the restore attempt —
            // success, workspace-gone and cancellation all release it.
            dismissSplash()
        }
    })()

    return () => {
        cancelled = true
        unsubscribeGuard()
    }
}

/**
 * Restores the last workspace + first-editor-group tabs on launch, then keeps
 * the persisted session in sync as tabs change.
 *
 * For local cold restore, the two effects race: restoration opens the workspace
 * asynchronously, during which the store passes through an empty state (the
 * openWorkspaceAtPath → setWorkspace resets groups). Without a gate the save
 * subscription would fire on that empty state and clobber the session we just
 * read. `restoredRef` blocks every save until the restore attempt fully
 * settles, so the on-disk session survives the async gap. Remote restoration
 * leaves saving active because the splash is already dismissed and the user
 * can open another workspace while the connection attempt is pending.
 */
export function SessionRestoreBridge() {
    const restoredRef = useRef(false)

    // Restore effect (mount-once): only when no workspace is open yet, and not
    // when the OS launched Yuzora to open a folder — that folder replaces the
    // last session. Launch files still open into the restored workspace.
    useEffect(() => {
        const session = loadWorkspaceSession()
        if (useWorkspaceStore.getState().workspacePath || !session) {
            openRestoreGate(restoredRef)
            return
        }
        let disposed = false
        let stop: (() => void) | undefined
        void launchOpenRequests().then((requests) => {
            if (disposed) return
            if (useWorkspaceStore.getState().workspacePath || requests.some((request) => request.kind === "directory")) {
                openRestoreGate(restoredRef)
                return
            }
            stop = restoreSession(session, restoredRef)
        })
        return () => {
            disposed = true
            stop?.()
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
            persistEditorSession(state)
        })
    }, [])

    return null
}
