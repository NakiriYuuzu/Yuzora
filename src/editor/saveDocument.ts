import { documentGeneration, getDocument, saveDocumentContent, type RegistryEntry } from "./documentRegistry"
import { getView } from "./viewRegistry"
import { recentlySaved } from "../lib/saveSuppress"
import { useWorkspaceStore } from "../state/workspaceStore"
import { serializeDocumentLineEndings } from "./lineEndings"
import { showActionError } from "../lib/actionFeedback"
import i18n from "../lib/i18n"

export type SaveDirtyTabOutcome =
    | { kind: "saved" }
    | { kind: "blocked"; reason: "mixed" }
    | { kind: "notEditable" }
    | { kind: "failed" }
    /** The buffer, document or workspace changed before or during the write: still dirty. */
    | { kind: "changed" }

function saveActionLabel(): string {
    return i18n.t("statusBar.saveAction", { ns: "workbench" })
}

export function showMixedLineEndingSaveError(): Promise<void> {
    return showActionError(
        saveActionLabel(),
        new Error(i18n.t("statusBar.mixedSaveError", { ns: "workbench" }))
    )
}

export function showDocumentSaveError(error: unknown): Promise<void> {
    return showActionError(saveActionLabel(), error)
}

/**
 * Persists a dirty tab to disk and clears its externally-modified flag, and its
 * dirty flag unless the text was edited while the write ran — the "Save" branch
 * of the unsaved-changes modal (TabBar close, workspace switch). The active tab's freshest text lives in its live CodeMirror view; a
 * dirty non-active tab (only the active pane is mounted) has its latest text
 * mirrored into the document registry on unmount, so read from whichever is
 * present. Marks the path recentlySaved first so the fs watcher doesn't flag our
 * own write as an external change (mirrors EditorPane's save).
 */
export async function saveDirtyTab(path: string, canContinue: () => boolean = () => true): Promise<SaveDirtyTabOutcome> {
    const view = getView(path)
    const document = view?.state.doc
    const generation = documentGeneration(path)
    const store = useWorkspaceStore.getState()
    const storedLineEnding = store.getLineEnding(path)
    let content: string
    let lineEnding = storedLineEnding
    let entry: RegistryEntry | undefined

    if (view && lineEnding) {
        // A failed external reload deliberately leaves the live pane mounted but
        // clears the document cache. Prefer that live buffer so Save can recreate
        // a file deleted on disk without another failing openFile call.
        content = view.state.doc.toString()
    } else {
        try {
            entry = await getDocument(path)
        } catch (error) {
            await showDocumentSaveError(error)
            return { kind: "failed" }
        }
        const result = entry.result
        if (result.kind !== "full" && result.kind !== "limited") {
            return { kind: "notEditable" }
        }
        content = view ? view.state.doc.toString() : result.content
        lineEnding ??= result.lineEnding
    }

    const cachedResult = entry?.result
    const unchanged = () => canContinue()
        && useWorkspaceStore.getState().workspacePath === store.workspacePath
        && useWorkspaceStore.getState().getLineEnding(path) === storedLineEnding
        && documentGeneration(path) === generation
        && getView(path) === view
        && (view ? view.state.doc === document : entry?.result === cachedResult)
    if (!unchanged()) return { kind: "changed" }

    const serialized = serializeDocumentLineEndings(
        content,
        lineEnding
    )
    if (serialized.kind === "blocked") {
        await showMixedLineEndingSaveError()
        return serialized
    }

    recentlySaved.mark(path)
    try {
        await saveDocumentContent(path, serialized.content)
    } catch (error) {
        await showDocumentSaveError(error)
        return { kind: "failed" }
    }
    // A completed write belongs to its captured buffer, not edits or a new
    // workspace that arrived while it was in flight.
    if (!unchanged()) return { kind: "changed" }
    store.markDirty(path, false)
    store.markExternallyModified(path, false)
    return { kind: "saved" }
}
