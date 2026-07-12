import type { EditorView } from "@codemirror/view"

export type FormatterState = "checking" | "available" | "unsupported"

export interface EditorViewMetadata {
    groupIndex: number
    readonly: boolean
    formatter: FormatterState
    formatDocument?: () => Promise<boolean>
}

export interface RegisteredEditorView extends EditorViewMetadata {
    // The EditorView object is also the registry identity. All metadata updates
    // and unregisters are identity-guarded so a replaced pane cannot mutate the
    // newer view registered for the same path.
    view: EditorView
}

const views = new Map<string, RegisteredEditorView>()

export function registerView(
    path: string,
    view: EditorView,
    metadata: Partial<EditorViewMetadata> = {}
): void {
    views.set(path, {
        view,
        groupIndex: metadata.groupIndex ?? -1,
        readonly: metadata.readonly ?? false,
        formatter: metadata.formatter ?? "unsupported",
        formatDocument: metadata.formatDocument
    })
}

export function updateViewMetadata(
    path: string,
    view: EditorView,
    metadata: Partial<EditorViewMetadata>
): void {
    const current = views.get(path)
    if (!current || current.view !== view) return
    views.set(path, { ...current, ...metadata, view })
}

export function unregisterView(path: string, view?: EditorView): void {
    // When a view is given, only remove the entry if it is still the one
    // registered — a later split group that overwrote the path must not be
    // clobbered by an earlier pane unmounting (m4). No view = unconditional.
    if (view !== undefined && views.get(path)?.view !== view) return
    views.delete(path)
}

export function getView(path: string): EditorView | undefined {
    return views.get(path)?.view
}

export function getViewEntry(path: string): RegisteredEditorView | undefined {
    return views.get(path)
}
