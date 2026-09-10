import { openFileSnapshot } from "../lib/ipc"
import type { OpenFileResult } from "../lib/types"
import { useWorkspaceStore } from "../state/workspaceStore"

export interface RegistryEntry {
    result: OpenFileResult
}

const registry = new Map<string, RegistryEntry>()
const generations = new Map<string, number>()
let epoch = 0

function documentKey(path: string, workspace = useWorkspaceStore.getState().workspacePath): string {
    return JSON.stringify([workspace, path])
}

export async function getDocument(path: string): Promise<RegistryEntry> {
    const key = documentKey(path)
    const started = epoch
    const existing = registry.get(key)
    if (existing) return existing
    const accept = () => started === epoch && key === documentKey(path)
    const snapshot = await openFileSnapshot(path)
    if (!accept()) throw new Error("Document workspace changed")
    const current = registry.get(key)
    if (current) return current
    snapshot.accept()
    const entry: RegistryEntry = { result: snapshot.result }
    registry.set(key, entry)
    return entry
}

export function dropDocument(path: string) {
    registry.delete(documentKey(path))
}

// Move a cached document from oldPath to newPath after a rename so a re-opened
// tab hits the cache under the new key instead of the (now-gone) old one.
// `liveContent`, when given, snapshots the live editor buffer — which holds
// newer text than the registry until the pane unmounts — so unsaved edits
// survive the remount. Move the generation with the cached entry so metadata
// hydration can distinguish an ordinary rename remount from a later disk reload.
export function renameDocument(oldPath: string, newPath: string, liveContent?: string) {
    oldPath = documentKey(oldPath)
    newPath = documentKey(newPath)
    const entry = registry.get(oldPath)
    if (!entry) return
    registry.delete(oldPath)
    if (liveContent !== undefined) {
        const r = entry.result
        if (r.kind === "full" || r.kind === "limited") {
            entry.result = { ...r, content: liveContent }
        }
    }
    registry.set(newPath, entry)
    const generation = generations.get(oldPath)
    if (generation !== undefined) {
        generations.delete(oldPath)
        generations.set(newPath, generation)
    }
}

export function updateBuffer(path: string, content: string, generation: number, workspace = useWorkspaceStore.getState().workspacePath) {
    if (workspace !== useWorkspaceStore.getState().workspacePath) return
    if (generation !== documentGeneration(path)) return
    const entry = registry.get(documentKey(path, workspace))
    if (!entry) return
    const r = entry.result
    if (r.kind === "full" || r.kind === "limited") {
        entry.result = { ...r, content }
    }
}

// generations 保留不清：路徑在新 workspace 重疊時，key 的 generation 仍需遞增以強制 EditorPane remount。
export function clearAll() {
    epoch++
    for (const key of registry.keys()) generations.set(key, (generations.get(key) ?? 0) + 1)
    registry.clear()
}

export async function reloadDocument(path: string, canApply: () => boolean = () => true): Promise<RegistryEntry> {
    const key = documentKey(path)
    const started = epoch
    const previous = registry.get(key)
    const previousResult = previous?.result
    const generation = generations.get(key)
    // Keep the buffer cached until the read is accepted. Edits, a newer reload,
    // or workspace changes during slow remote I/O must not remount the editor
    // or silently replace the revision used for the next save.
    const accept = () => started === epoch && key === documentKey(path)
        && registry.get(key) === previous && previous?.result === previousResult
        && generations.get(key) === generation && canApply()
    const snapshot = await openFileSnapshot(path)
    if (!accept()) throw new Error("Document changed during reload")
    snapshot.accept()
    const entry = { result: snapshot.result }
    registry.set(key, entry)
    generations.set(key, (generations.get(key) ?? 0) + 1)
    return entry
}

export function documentGeneration(path: string): number {
    return generations.get(documentKey(path)) ?? 0
}
