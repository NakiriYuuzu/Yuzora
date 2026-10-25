import { redo, undo } from "@codemirror/commands"
import { EditorView } from "@codemirror/view"
import type { EditHistoryAction } from "@/lib/ipc"

/** Runs undo/redo in the focused CodeMirror editor; false when none has focus. */
export function runEditorHistory(
    action: EditHistoryAction,
    focused: Element | null = document.activeElement
): boolean {
    const editor = focused?.closest(".cm-editor")
    const view = editor instanceof HTMLElement ? EditorView.findFromDOM(editor) : null
    // Fields in an editor's panels (search, replace) keep their native undo.
    if (!view || !view.contentDOM.contains(focused)) return false
    if (action === "undo") undo(view)
    else redo(view)
    return true
}
