import { EditorSelection, type StateEffect, type Text } from "@codemirror/state"
import { ViewPlugin, type EditorView, type ViewUpdate } from "@codemirror/view"

// Per-document CodeMirror view state (scroll + selection) that outlives the
// EditorView. Panes are destroyed on every tab / Space / workspace switch, so
// the last known state is recorded while the view is live (the DOM is already
// detached by the time React's passive cleanup runs, so scrollTop reads 0
// there) and replayed when a pane for the same document mounts again.
interface SavedEditorViewState {
    docLength: number
    selection: EditorSelection
    scroll: StateEffect<unknown> | null
}

export interface RestoredEditorViewState {
    selection?: EditorSelection
    scrollTo?: StateEffect<unknown>
}

// Remember recently viewed documents only; enough for normal tab churn
// without letting a long session grow the map without bound.
const MAX_ENTRIES = 200
const states = new Map<string, SavedEditorViewState>()

function stateKey(workspacePath: string | null, path: string): string {
    return JSON.stringify([workspacePath, path])
}

function remember(key: string, state: SavedEditorViewState) {
    states.delete(key)
    states.set(key, state)
    if (states.size > MAX_ENTRIES) states.delete(states.keys().next().value!)
}

/** Saved state for a freshly created document, or empty when nothing is recorded. */
export function restoreEditorViewState(workspacePath: string | null, path: string, doc: Text): RestoredEditorViewState {
    const saved = states.get(stateKey(workspacePath, path))
    if (!saved) return {}
    const selection = saved.selection.ranges.every((range) => range.to <= doc.length)
        ? saved.selection
        : EditorSelection.single(Math.min(saved.selection.main.head, doc.length))
    // A scroll snapshot is anchored to positions of the document it was taken
    // from; only replay it when the reloaded text still has the same length.
    const scrollTo = saved.scroll && saved.docLength === doc.length ? saved.scroll : undefined
    return scrollTo ? { selection, scrollTo } : { selection }
}

/** Records scroll position and selection of a live view for later restore. */
export function editorViewStateTracker(workspacePath: string | null, path: string) {
    const key = stateKey(workspacePath, path)
    return ViewPlugin.define((view: EditorView) => {
        const onScroll = () => remember(key, {
            docLength: view.state.doc.length,
            selection: view.state.selection,
            scroll: view.scrollSnapshot()
        })
        view.scrollDOM.addEventListener("scroll", onScroll, { passive: true })
        return {
            update(update: ViewUpdate) {
                if (!update.selectionSet && !update.docChanged) return
                const previous = states.get(key)?.scroll ?? null
                // Keep the scroll anchor on the same text when edits shift it.
                const scroll = previous && update.docChanged ? previous.map(update.changes) ?? null : previous
                remember(key, { docLength: update.state.doc.length, selection: update.state.selection, scroll })
            },
            destroy() {
                view.scrollDOM.removeEventListener("scroll", onScroll)
            }
        }
    })
}

export function renameEditorViewState(workspacePath: string | null, fromPath: string, toPath: string) {
    const fromKey = stateKey(workspacePath, fromPath)
    const saved = states.get(fromKey)
    if (!saved) return
    states.delete(fromKey)
    remember(stateKey(workspacePath, toPath), saved)
}

export function clearEditorViewStatesForTest() {
    states.clear()
}
