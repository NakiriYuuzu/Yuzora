import { useEffect, useRef } from "react"

import { getDocument } from "@/editor/documentRegistry"
import { openWorkspaceAtPath } from "@/lib/workspaceActions"
import {
    clearWorkspaceSession,
    loadWorkspaceSession,
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
                await openWorkspaceAtPath(session.workspacePath)
                if (cancelled || workspaceOpens > 1) return
                const ws = useWorkspaceStore.getState()
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
            }
        })()

        return () => {
            cancelled = true
            unsubscribeGuard()
        }
    }, [])

    // Save effect: mirror the first group's real-file tabs into localStorage on
    // every relevant workspace-store change, once the restore gate is open.
    useEffect(() => {
        return useWorkspaceStore.subscribe((state, prev) => {
            if (!restoredRef.current) return
            if (state.workspacePath === prev.workspacePath && state.groups[0] === prev.groups[0]) {
                return
            }
            const workspacePath = state.workspacePath
            if (!workspacePath) return
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
