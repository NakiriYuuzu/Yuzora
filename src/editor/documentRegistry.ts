import { openFile } from "../lib/ipc"
import type { OpenFileResult } from "../lib/types"

export interface RegistryEntry {
    result: OpenFileResult
}

const registry = new Map<string, RegistryEntry>()
const generations = new Map<string, number>()

export async function getDocument(path: string): Promise<RegistryEntry> {
    const existing = registry.get(path)
    if (existing) return existing
    const result = await openFile(path)
    const entry: RegistryEntry = { result }
    registry.set(path, entry)
    return entry
}

export function dropDocument(path: string) {
    registry.delete(path)
}

// Move a cached document from oldPath to newPath after a rename so a re-opened
// tab hits the cache under the new key instead of the (now-gone) old one.
// `liveContent`, when given, snapshots the live editor buffer — which holds
// newer text than the registry until the pane unmounts — so unsaved edits
// survive the remount. Move the generation with the cached entry so metadata
// hydration can distinguish an ordinary rename remount from a later disk reload.
export function renameDocument(oldPath: string, newPath: string, liveContent?: string) {
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

export function updateBuffer(path: string, content: string, generation: number) {
    if (generation !== documentGeneration(path)) return
    const entry = registry.get(path)
    if (!entry) return
    const r = entry.result
    if (r.kind === "full" || r.kind === "limited") {
        entry.result = { ...r, content }
    }
}

// generations 保留不清：路徑在新 workspace 重疊時，key 的 generation 仍需遞增以強制 EditorPane remount。
export function clearAll() {
    registry.clear()
}

export async function reloadDocument(path: string): Promise<RegistryEntry> {
    registry.delete(path)
    // Bump the generation only AFTER a successful re-fetch. The generation drives
    // the EditorArea's keyed remount, so a remount must happen only when there is
    // genuinely new content: a reload whose openFile rejects (the file was deleted)
    // must leave the generation — and thus the live pane and its unsaved buffer —
    // untouched. The await-resume and the set run in one microtask, so no
    // updateBuffer can slip in between; this also closes the old race where a
    // re-render during the fetch remounted into a registry miss.
    const entry = await getDocument(path)
    generations.set(path, (generations.get(path) ?? 0) + 1)
    return entry
}

export function documentGeneration(path: string): number {
    return generations.get(path) ?? 0
}
