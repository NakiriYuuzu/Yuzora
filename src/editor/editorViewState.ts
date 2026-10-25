import { EditorSelection, type EditorState, type StateEffect, type Text } from "@codemirror/state"
import { historyField, redoDepth, undoDepth } from "@codemirror/commands"
import { ViewPlugin, type EditorView, type ViewUpdate } from "@codemirror/view"

import { getView } from "./viewRegistry"
import {
    clearRetainedViewStatesForTest,
    renameRetainedViewStates,
    retainedViewState,
    setRetainedFacetLimit
} from "@/workbench/viewRetention"

// Per-document CodeMirror view state (scroll + selection + undo history) that
// outlives the EditorView. Panes are destroyed on every tab / Space /
// workspace switch, so the last known state is recorded while the view is
// live (the DOM is already detached by the time React's passive cleanup runs,
// so scrollTop reads 0 there) and replayed when a pane for the same document
// mounts again. Storage is the shared view-state registry (viewRetention).
interface SavedEditorViewState {
    docLength: number
    selection: EditorSelection
    scroll: StateEffect<unknown> | null
}

interface SavedEditorHistory {
    // The undo history is a list of changes against this exact text; it is
    // only replayed onto a document that is still identical to it.
    doc: Text
    history: unknown
}

export interface RestoredEditorViewState {
    selection?: EditorSelection
    scrollTo?: StateEffect<unknown>
}

const VIEW_FACET = "editor"
const HISTORY_FACET = "editor-history"
// Histories pin their document text and can hold whole-document replacements
// (rich Markdown edits), so keep fewer of them and bound their total size, in
// UTF-16 code units of serialized history. One over the budget is not kept.
const HISTORY_WEIGHT_BUDGET = 4_000_000
setRetainedFacetLimit(HISTORY_FACET, 32, HISTORY_WEIGHT_BUDGET)

// Size of serialized history, giving up (Infinity) once it passes `limit`.
function historyWeight(value: unknown, limit: number): number {
    let weight = 0
    const stack: unknown[] = [value]
    while (stack.length > 0) {
        const item = stack.pop()
        if (typeof item === "string") weight += item.length
        else if (Array.isArray(item)) {
            weight += item.length
            for (const child of item) stack.push(child)
        } else if (item && typeof item === "object") {
            for (const child of Object.values(item)) stack.push(child)
        } else weight += 1
        if (weight > limit) return Infinity
    }
    return weight
}

function viewState(workspacePath: string | null, path: string) {
    return retainedViewState<SavedEditorViewState>(workspacePath, path, VIEW_FACET)
}

/** Saved state for a freshly created document, or empty when nothing is recorded. */
export function restoreEditorViewState(workspacePath: string | null, path: string, doc: Text): RestoredEditorViewState {
    const saved = viewState(workspacePath, path).get()
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
    const state = viewState(workspacePath, path)
    return ViewPlugin.define((view: EditorView) => {
        const onScroll = () => state.set({
            docLength: view.state.doc.length,
            selection: view.state.selection,
            scroll: view.scrollSnapshot()
        })
        view.scrollDOM.addEventListener("scroll", onScroll, { passive: true })
        return {
            update(update: ViewUpdate) {
                if (!update.selectionSet && !update.docChanged) return
                const previous = state.get()?.scroll ?? null
                // Keep the scroll anchor on the same text when edits shift it.
                const scroll = previous && update.docChanged ? previous.map(update.changes) ?? null : previous
                state.set({ docLength: update.state.doc.length, selection: update.state.selection, scroll })
            },
            destroy() {
                view.scrollDOM.removeEventListener("scroll", onScroll)
            }
        }
    })
}

/**
 * Records the undo history of a view that is about to be destroyed. Call it
 * only while the document's buffer is kept (see EditorPane): a closed tab or a
 * reloaded document starts over with no history.
 */
export function rememberEditorHistory(workspacePath: string | null, path: string, state: EditorState): void {
    const saved = retainedViewState<SavedEditorHistory>(workspacePath, path, HISTORY_FACET)
    if (state.field(historyField, false) === undefined || (undoDepth(state) === 0 && redoDepth(state) === 0)) {
        saved.delete()
        return
    }
    const { history } = state.toJSON({ history: historyField }) as { history?: unknown }
    const weight = historyWeight(history, HISTORY_WEIGHT_BUDGET)
    if (history === undefined || weight === Infinity) {
        saved.delete()
        return
    }
    saved.set({ doc: state.doc, history }, weight)
}

/**
 * Drops a document's recorded undo history. Its own pane skips recording when
 * the buffer is gone, but a history recorded earlier, when the document was
 * switched away from, has to be dropped with the buffer explicitly.
 */
export function forgetEditorHistory(workspacePath: string | null, path: string): void {
    retainedViewState<SavedEditorHistory>(workspacePath, path, HISTORY_FACET).delete()
}

/**
 * Serialized undo history for `doc`, for EditorState.fromJSON with
 * `{ history: historyField }`, or undefined when none was recorded or the text
 * changed since (an external reload, or an edit made in another group).
 */
export function restoreEditorHistory(workspacePath: string | null, path: string, doc: Text): unknown {
    const saved = retainedViewState<SavedEditorHistory>(workspacePath, path, HISTORY_FACET).get()
    if (!saved || saved.doc.length !== doc.length || !saved.doc.eq(doc)) return undefined
    return saved.history
}

/**
 * Moves a renamed document's view state. The open pane is remounted under the
 * new path afterwards and its own unmount no longer finds the old buffer, so
 * take its live undo history now, like renameDocument takes its live text.
 */
export function renameEditorViewState(workspacePath: string | null, fromPath: string, toPath: string) {
    const live = getView(fromPath)
    if (live) rememberEditorHistory(workspacePath, fromPath, live.state)
    renameRetainedViewStates(workspacePath, fromPath, toPath)
}

export function clearEditorViewStatesForTest() {
    clearRetainedViewStatesForTest(VIEW_FACET)
    clearRetainedViewStatesForTest(HISTORY_FACET)
}
