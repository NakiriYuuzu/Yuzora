import { documentGeneration } from "../editor/documentRegistry"
import { subscribeView } from "../editor/viewRegistry"
import { useWorkspaceStore } from "../state/workspaceStore"

// Keep expensive full-document previews at the old 400ms budget during typing,
// without waking idle previews. View lifecycle changes bypass the typing budget.
export function subscribePreviewView(path: string, refresh: () => void): () => void {
    let lastChange = -Infinity
    let trailing: ReturnType<typeof setTimeout> | undefined
    const emit = () => {
        trailing = undefined
        lastChange = performance.now()
        refresh()
    }
    const refreshView = () => {
        clearTimeout(trailing)
        trailing = undefined
        lastChange = -Infinity
        refresh()
    }
    const unsubscribe = subscribeView(path, (change) => {
        if (change === "view") {
            refreshView()
            return
        }
        const remaining = 400 - (performance.now() - lastChange)
        if (remaining <= 0) {
            clearTimeout(trailing)
            emit()
        } else if (trailing === undefined) {
            trailing = setTimeout(emit, remaining)
        }
    })
    // Accepted external reloads bump documentGeneration before updating the
    // workspace tabs. Observe that revision even when the source tab is inactive
    // (no EditorView to unregister/register). Unrelated store writes are O(1).
    let generation = documentGeneration(path)
    const unsubscribeWorkspace = useWorkspaceStore.subscribe(() => {
        const next = documentGeneration(path)
        if (next === generation) return
        generation = next
        refreshView()
    })
    return () => {
        unsubscribe()
        unsubscribeWorkspace()
        clearTimeout(trailing)
    }
}
