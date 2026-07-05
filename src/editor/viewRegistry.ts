import type { EditorView } from "@codemirror/view"

const views = new Map<string, EditorView>()

export function registerView(path: string, view: EditorView): void {
    views.set(path, view)
}

export function unregisterView(path: string, view?: EditorView): void {
    // When a view is given, only remove the entry if it is still the one
    // registered — a later split group that overwrote the path must not be
    // clobbered by an earlier pane unmounting (m4). No view = unconditional.
    if (view !== undefined && views.get(path) !== view) return
    views.delete(path)
}

export function getView(path: string): EditorView | undefined {
    return views.get(path)
}
