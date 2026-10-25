import type { Text } from "@codemirror/state"
import { useEditorSettingsStore, type MarkdownViewMode } from "@/state/editorSettingsStore"
import { clearRetainedViewStatesForTest, retainedViewState } from "@/workbench/viewRetention"

// Match source-editor state lifetime: per workspace/document, bounded in memory
// by the shared view-state registry (viewRetention).
interface SavedMarkdownViewState {
    mode: MarkdownViewMode
    selection?: { doc: Text; anchor: number; head: number }
    scroll?: { doc: Text; top: number; left: number }
}

export interface MarkdownViewState {
    get: () => Readonly<SavedMarkdownViewState>
    set: (patch: Partial<SavedMarkdownViewState>) => void
}

type StoredMarkdownViewState = Omit<SavedMarkdownViewState, "mode"> & { mode?: MarkdownViewMode }

const FACET = "markdown"

export function markdownViewState(workspace: string | null, path: string): MarkdownViewState {
    const state = retainedViewState<StoredMarkdownViewState>(workspace, path, FACET)
    // Touch the entry so a document being opened counts as recently used.
    const snapshot = (): StoredMarkdownViewState => state.get() ?? {}
    state.set(snapshot())
    return {
        // Until this file's mode is chosen, it follows the editor setting.
        get: () => {
            const current = snapshot()
            return { ...current, mode: current.mode ?? useEditorSettingsStore.getState().markdownDefaultMode }
        },
        set: patch => state.set({ ...snapshot(), ...patch })
    }
}

export function clearMarkdownViewStatesForTest() {
    clearRetainedViewStatesForTest(FACET)
}
