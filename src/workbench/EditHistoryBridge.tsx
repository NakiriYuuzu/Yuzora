import { useEffect } from "react"
import { listen } from "@tauri-apps/api/event"
import { runEditorHistory } from "@/editor/editHistory"
import { editHistoryNative, type EditMenuAction } from "@/lib/ipc"
import { keepsSelectAllFromPage } from "@/lib/selectAll"

// On macOS the Edit menu's Undo/Redo (⌘Z / ⇧⌘Z) arrive here instead of as key
// presses (src-tauri/src/edit_menu.rs). WebKit's own undo only knows the edits
// it recorded in the current DOM, so a focused CodeMirror editor runs its own
// history, which also survives tab switches; everything else (inputs, the rich
// Markdown editor, terminals, the Browser) gets the native action as before.
// Select All (⌘A) arrives here too and is dropped where it would only highlight
// the page behind a dialog; everywhere else it stays native.
// A focused child Browser webview can leave this document's activeElement on
// the editor (see WorkbenchFocusBridge), so the document must own focus too.
export function EditHistoryBridge() {
    useEffect(() => {
        const unlisten = listen<EditMenuAction>("edit:history", (event) => {
            const action = event.payload
            const handled = document.hasFocus() && (action === "selectAll"
                ? keepsSelectAllFromPage(document.activeElement)
                : runEditorHistory(action))
            if (!handled) void editHistoryNative(action).catch(() => {})
        })
        return () => {
            void unlisten.then((fn) => fn())
        }
    }, [])
    return null
}
