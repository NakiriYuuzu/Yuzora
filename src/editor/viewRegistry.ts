import type { EditorView } from "@codemirror/view"

import { canonicalPathKey, isWindowsPath } from "../lib/paths"

type FormatterState = "checking" | "available" | "unsupported"

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

// Operational Windows paths register only an explicit Windows alias key. An
// LSP UNC URI decodes to `//host/share/...`, which is
// ambiguous with a case-sensitive POSIX double-slash path: exact POSIX lookup
// wins, and only a view registered from unambiguous Windows syntax participates
// in the fallback Windows alias lookup.
const views = new Map<string, RegisteredEditorView>()

function automaticViewKey(path: string): string {
    return `auto:${canonicalPathKey(path)}`
}

function windowsAliasKey(path: string): string {
    return `windows:${canonicalPathKey(path, "windows")}`
}

function registrationKeys(path: string): string[] {
    // Unambiguous Windows operational paths live only in the Windows namespace.
    // Otherwise a lower-case UNC alias such as `\\server\share\file.ts` would
    // overwrite the exact, case-sensitive POSIX registration
    // `//server/share/file.ts` in the automatic namespace.
    return isWindowsPath(path)
        ? [windowsAliasKey(path)]
        : [automaticViewKey(path)]
}

function lookupKeys(path: string): string[] {
    // Unambiguous Windows syntax must never consult the exact POSIX namespace.
    if (isWindowsPath(path)) return [windowsAliasKey(path)]

    const keys = [automaticViewKey(path)]
    // A forward-slash `//host/share/...` path can come from an LSP UNC URI.
    // Prefer an exact POSIX registration, then fall back to a registered
    // Windows UNC operational path only when no exact POSIX entry exists.
    if (path.startsWith("//")) keys.push(windowsAliasKey(path))
    return keys
}

function findViewEntry(path: string): RegisteredEditorView | undefined {
    for (const key of lookupKeys(path)) {
        const entry = views.get(key)
        if (entry) return entry
    }
    return undefined
}

export function registerView(
    path: string,
    view: EditorView,
    metadata: Partial<EditorViewMetadata> = {}
): void {
    const entry: RegisteredEditorView = {
        view,
        groupIndex: metadata.groupIndex ?? -1,
        readonly: metadata.readonly ?? false,
        formatter: metadata.formatter ?? "unsupported",
        formatDocument: metadata.formatDocument
    }
    for (const key of registrationKeys(path)) views.set(key, entry)
}

export function updateViewMetadata(
    path: string,
    view: EditorView,
    metadata: Partial<EditorViewMetadata>
): void {
    const current = findViewEntry(path)
    if (!current || current.view !== view) return
    const updated = { ...current, ...metadata, view }
    for (const [key, entry] of views) {
        if (entry === current) views.set(key, updated)
    }
}

export function unregisterView(path: string, view?: EditorView): void {
    // When a view is given, only remove the entry if it is still the one
    // registered — a later split group that overwrote the path must not be
    // clobbered by an earlier pane unmounting (m4). No view = unconditional.
    const current = findViewEntry(path)
    if (!current || (view !== undefined && current.view !== view)) return
    for (const [key, entry] of views) {
        if (entry === current) views.delete(key)
    }
}

export function getView(path: string): EditorView | undefined {
    return findViewEntry(path)?.view
}

export function getViewEntry(path: string): RegisteredEditorView | undefined {
    return findViewEntry(path)
}
