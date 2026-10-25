import { useEffect } from "react"
import { listen } from "@tauri-apps/api/event"
import { runEditorHistory } from "@/editor/editHistory"
import { editHistoryNative, type EditHistoryAction } from "@/lib/ipc"

// On macOS the Edit menu's Undo/Redo (⌘Z / ⇧⌘Z) arrive here instead of as key
// presses (src-tauri/src/edit_menu.rs). WebKit's own undo only knows the edits
// it recorded in the current DOM, so a focused CodeMirror editor runs its own
// history, which also survives tab switches; everything else (inputs, the rich
// Markdown editor, terminals, the Browser) gets the native action as before.
export function EditHistoryBridge() {
    useEffect(() => {
        const unlisten = listen<EditHistoryAction>("edit:history", (event) => {
            if (!runEditorHistory(event.payload)) void editHistoryNative(event.payload).catch(() => {})
        })
        return () => {
            void unlisten.then((fn) => fn())
        }
    }, [])
    return null
}
