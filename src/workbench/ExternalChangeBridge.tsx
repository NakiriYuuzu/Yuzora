import { useEffect } from "react"
import { listen } from "@tauri-apps/api/event"
import { useWorkspaceStore } from "../state/workspaceStore"
import { handleExternalChange } from "../lib/externalChange"
import { recentlySaved } from "../lib/saveSuppress"
import { reloadDocument } from "../editor/documentRegistry"

export function ExternalChangeBridge() {
    useEffect(() => {
        const unlisten = listen<string[]>("fs:external-change", (e) => {
            const s = useWorkspaceStore.getState()
            const allTabs = s.groups.flatMap((g) => g.tabs)
            const plan = handleExternalChange(e.payload, allTabs, recentlySaved.snapshot())
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
