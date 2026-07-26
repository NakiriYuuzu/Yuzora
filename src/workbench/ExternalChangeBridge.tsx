import { useEffect } from "react"
import { listen } from "@tauri-apps/api/event"
import type { ExternalChangePayload } from "../lib/types"
import { useFileTreeStore } from "../state/fileTreeStore"
import { useWorkspaceStore } from "../state/workspaceStore"
import { handleExternalChange } from "../lib/externalChange"
import { recentlySaved } from "../lib/saveSuppress"
import { reloadDocument } from "../editor/documentRegistry"

export function ExternalChangeBridge() {
    useEffect(() => {
        const unlisten = listen<ExternalChangePayload>("fs:external-change", (e) => {
            const s = useWorkspaceStore.getState()
            // #57 T3：事件帶 workspaceRoot——切換 gap 內舊 workspace watcher 的
            // 殘留事件不得刷新新 workspace 的樹或 reload buffer（防串場，比照
            // LspBridge 以事件當下的 live 值比對）。
            if (e.payload.workspaceRoot !== s.workspacePath) return
            // #59 T4b：精準失效——只 re-list payload 路徑對應的已快取目錄，
            // 不再以 treeRevision 整樹 remount。invalidatePaths 內部仍會 bump
            // treeRevision：workspace mention index 與其他相容消費者靠它失效。
            void useFileTreeStore
                .getState()
                .invalidatePaths(e.payload.workspaceRoot, e.payload.paths)
            const allTabs = s.groups.flatMap((g) => g.tabs)
            const plan = handleExternalChange(e.payload.paths, allTabs, recentlySaved.snapshot())
            for (const path of plan.markModified) s.markExternallyModified(path, true)
            for (const path of plan.reload) {
                // Settle the external-modified flag on BOTH outcomes. A reload
                // whose getDocument→openFile rejects (the file was deleted out
                // from under the tab) must still flip a workspaceStore field so
                // subscribers (e.g. StatusBar) re-render and converge; the trailing
                // catch also keeps the chain from floating an unhandled rejection.
                void reloadDocument(path)
                    .then(() => s.markExternallyModified(path, false))
                    .catch(() => s.markExternallyModified(path, false))
            }
        })
        return () => {
            void unlisten.then((fn) => fn())
        }
    }, [])
    return null
}
