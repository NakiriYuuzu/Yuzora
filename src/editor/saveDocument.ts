import { getDocument } from "./documentRegistry"
import { getView } from "./viewRegistry"
import { saveFile } from "../lib/ipc"
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
 * Persists a dirty tab to disk and clears its dirty / externally-modified flags
 * — the "Save" branch of the unsaved-changes modal (TabBar close, workspace
 * switch). The active tab's freshest text lives in its live CodeMirror view; a
 * dirty non-active tab (only the active pane is mounted) has its latest text
 * mirrored into the document registry on unmount, so read from whichever is
 * present. Marks the path recentlySaved first so the fs watcher doesn't flag our
 * own write as an external change (mirrors EditorPane's save).
 */
export async function saveDirtyTab(path: string): Promise<SaveDirtyTabOutcome> {
    const view = getView(path)
    const store = useWorkspaceStore.getState()
    const storedLineEnding = store.getLineEnding(path)
    let content: string
    let lineEnding = storedLineEnding

    if (view && lineEnding) {
        // A failed external reload deliberately leaves the live pane mounted but
        // clears the document cache. Prefer that live buffer so Save can recreate
        // a file deleted on disk without another failing openFile call.
        content = view.state.doc.toString()
    } else {
        let entry
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
        await saveFile(path, serialized.content)
    } catch (error) {
        await showDocumentSaveError(error)
        return { kind: "failed" }
    }
    store.markDirty(path, false)
    store.markExternallyModified(path, false)
    return { kind: "saved" }
}
