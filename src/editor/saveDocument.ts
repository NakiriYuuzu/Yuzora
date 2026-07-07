import { getDocument } from "./documentRegistry"
import { getView } from "./viewRegistry"
import { saveFile } from "../lib/ipc"
import { recentlySaved } from "../lib/saveSuppress"
import { useWorkspaceStore } from "../state/workspaceStore"

/**
 * Persists a dirty tab to disk and clears its dirty / externally-modified flags
 * — the "Save" branch of the unsaved-changes modal (TabBar close, workspace
 * switch). The active tab's freshest text lives in its live CodeMirror view; a
 * dirty non-active tab (only the active pane is mounted) has its latest text
 * mirrored into the document registry on unmount, so read from whichever is
 * present. Marks the path recentlySaved first so the fs watcher doesn't flag our
 * own write as an external change (mirrors EditorPane's save).
 */
export async function saveDirtyTab(path: string): Promise<void> {
    const view = getView(path)
    let content: string
    if (view) {
        content = view.state.doc.toString()
    } else {
        const entry = await getDocument(path)
        const r = entry.result
        if (r.kind !== "full" && r.kind !== "limited") return
        content = r.content
    }
    recentlySaved.mark(path)
    await saveFile(path, content)
    const store = useWorkspaceStore.getState()
    store.markDirty(path, false)
    store.markExternallyModified(path, false)
}
