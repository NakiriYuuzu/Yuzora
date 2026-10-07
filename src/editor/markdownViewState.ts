import type { Text } from "@codemirror/state"

// Match source-editor state lifetime: per workspace/document, bounded in memory.
interface SavedMarkdownViewState {
    mode: "document" | "source"
    selection?: { doc: Text; anchor: number; head: number }
    scroll?: { doc: Text; top: number; left: number }
}

export interface MarkdownViewState {
    get: () => Readonly<SavedMarkdownViewState>
    set: (patch: Partial<SavedMarkdownViewState>) => void
}

const states = new Map<string, MarkdownViewState>()

export function markdownViewState(workspace: string | null, path: string): MarkdownViewState {
    const key = JSON.stringify([workspace, path])
    let state = states.get(key)
    if (!state) {
        let snapshot: SavedMarkdownViewState = { mode: "document" }
        state = { get: () => snapshot, set: patch => { snapshot = { ...snapshot, ...patch } } }
    }
    states.delete(key)
    states.set(key, state)
    if (states.size > 200) states.delete(states.keys().next().value!)
    return state
}

export function clearMarkdownViewStatesForTest() {
    states.clear()
}
