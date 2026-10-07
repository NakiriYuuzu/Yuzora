import { openFileSnapshot, saveFile } from "../lib/ipc"
import { parseRemoteFilePath } from "../lib/runtimeIdentity"
import type { OpenFileResult } from "../lib/types"
import { useWorkspaceStore } from "../state/workspaceStore"
import { renameEditorViewState } from "./editorViewState"

export interface RegistryEntry {
    result: OpenFileResult
    // Unlike result (the unmounted editor buffer), this never includes unsaved edits.
    diskResult?: OpenFileResult
}

const registry = new Map<string, RegistryEntry>()
const generations = new Map<string, number>()
let generationBase = 0
let highestGeneration = 0
let epoch = 0
const pendingReads = new Set<{ key: string }>()
// Key by path, not workspace: overlapping native workspaces share the same file.
const pendingSaves = new Map<string, Promise<void>>()
const pendingRemoteSaves = new Map<string, { next: number; applied: number; pending: number }>()

function documentKey(path: string, workspace = useWorkspaceStore.getState().workspacePath): string {
    return JSON.stringify([workspace, path])
}

export async function getDocument(path: string): Promise<RegistryEntry> {
    const key = documentKey(path)
    const started = epoch
    const existing = registry.get(key)
    if (existing) return existing
    const read = { key }
    pendingReads.add(read)
    const accept = () => pendingReads.has(read) && started === epoch && key === documentKey(path)
    try {
        const snapshot = await openFileSnapshot(path)
        if (!accept()) throw new Error("Document workspace changed")
        const current = registry.get(key)
        if (current) return current
        snapshot.accept()
        const entry: RegistryEntry = { result: snapshot.result, diskResult: snapshot.result }
        registry.set(key, entry)
        return entry
    } finally { pendingReads.delete(read) }
}

export function dropDocument(path: string, workspace = useWorkspaceStore.getState().workspacePath) {
    const key = documentKey(path, workspace)
    registry.delete(key)
    for (const read of pendingReads) if (read.key === key) pendingReads.delete(read)
}

// Move a cached document from oldPath to newPath after a rename so a re-opened
// tab hits the cache under the new key instead of the (now-gone) old one.
// `liveContent`, when given, snapshots the live editor buffer — which holds
// newer text than the registry until the pane unmounts — so unsaved edits
// survive the remount. Move the generation with the cached entry so metadata
// hydration can distinguish an ordinary rename remount from a later disk reload.
export function renameDocument(oldPath: string, newPath: string, liveContent?: string) {
    renameEditorViewState(useWorkspaceStore.getState().workspacePath, oldPath, newPath)
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

// Retire every old token, including dropped documents, without retaining their
// paths forever. A new base exceeds all issued generations so overlapping paths
// remount and late pane flushes cannot match a reopened document's token.
export function clearAll() {
    epoch++
    pendingReads.clear()
    generationBase = ++highestGeneration
    generations.clear()
    registry.clear()
}

export async function reloadDocument(
    path: string,
    canApply: () => boolean = () => true,
    mode: "explicit" | "reconcile" = "explicit"
): Promise<RegistryEntry> {
    const key = documentKey(path)
    const started = epoch
    const previous = registry.get(key)
    const previousResult = previous?.result
    const generation = generations.get(key)
    const previousDisk = previous?.diskResult
    const read = { key }
    pendingReads.add(read)
    // Keep the buffer cached until the read is accepted. Edits, a newer reload,
    // or workspace changes during slow remote I/O must not remount the editor
    // or silently replace the revision used for the next save.
    const accept = () => pendingReads.has(read) && started === epoch && key === documentKey(path)
        && registry.get(key) === previous && previous?.result === previousResult
        && previous?.diskResult === previousDisk
        && generations.get(key) === generation && canApply()
    try {
        const snapshot = await openFileSnapshot(path)
        if (!accept()) throw new Error("Document changed during reload")
        snapshot.accept()
        // Explicit take-disk must replace even a dirty buffer whose baseline is unchanged.
        if (mode === "reconcile" && previous && sameDiskSnapshot(previousDisk, snapshot.result)) return previous
        const entry = { result: snapshot.result, diskResult: snapshot.result }
        registry.set(key, entry)
        const nextGeneration = (generations.get(key) ?? generationBase) + 1
        generations.set(key, nextGeneration)
        highestGeneration = Math.max(highestGeneration, nextGeneration)
        return entry
    } finally { pendingReads.delete(read) }
}

// Non-text grades have no complete content snapshot; remain conservative there.
export function sameDiskSnapshot(previous: OpenFileResult | undefined, fresh: OpenFileResult): boolean {
    if (!previous || previous.kind !== fresh.kind || previous.size !== fresh.size) return false
    if (!("content" in previous) || !("content" in fresh) || previous.content !== fresh.content) return false
    if ("lineEnding" in previous && "lineEnding" in fresh) return previous.lineEnding === fresh.lineEnding
    return "encoding" in previous && "encoding" in fresh && previous.encoding === fresh.encoding
}

export async function documentChangedOnDisk(path: string, onRenamed?: (path: string) => void): Promise<boolean> {
    const workspace = useWorkspaceStore.getState().workspacePath
    const key = documentKey(path, workspace)
    const previous = registry.get(key)
    const disk = previous?.diskResult
    const started = epoch
    const generation = generations.get(key)
    const read = { key }
    pendingReads.add(read)
    try {
        const snapshot = await openFileSnapshot(path)
        if (!pendingReads.has(read) || started !== epoch || key !== documentKey(path)
            || registry.get(key) !== previous || generations.get(key) !== generation || previous?.diskResult !== disk) {
            throw new Error("Document changed during disk comparison")
        }
        if (!sameDiskSnapshot(disk, snapshot.result)) return true
        snapshot.accept()
        return false
    } catch (error) {
        // Rename moves the same entry. Preserve the pending conflict at its
        // new path, but never follow it across a workspace lifecycle boundary.
        if (previous && started === epoch && workspace === useWorkspaceStore.getState().workspacePath) {
            for (const [currentKey, entry] of registry) {
                if (entry !== previous || currentKey === key) continue
                const [currentWorkspace, currentPath] = JSON.parse(currentKey) as [string | null, string]
                if (currentWorkspace === workspace) onRenamed?.(currentPath)
            }
        }
        throw error
    } finally { pendingReads.delete(read) }
}

/** Editor saves update the disk baseline only after a successful write. */
export async function saveDocumentContent(path: string, content: string): Promise<number> {
    const key = documentKey(path)
    const previous = registry.get(key)
    const generation = generations.get(key)
    const started = epoch
    const remoteSave = parseRemoteFilePath(path)
        ? pendingRemoteSaves.get(path) ?? { next: 0, applied: 0, pending: 0 }
        : undefined
    const seq = remoteSave ? ++remoteSave.next : 0
    if (remoteSave) {
        remoteSave.pending++
        pendingRemoteSaves.set(path, remoteSave)
    }
    const save = async () => {
        const result = await saveFile(path, content)
        if (previous && started === epoch && key === documentKey(path)
            && registry.get(key) === previous && generations.get(key) === generation
            && (!remoteSave || seq > remoteSave.applied)) {
            const disk = previous.diskResult
            if (disk?.kind === "full" || disk?.kind === "limited") {
                const withoutCrlf = content.replace(/\r\n/g, "")
                const crlf = content.includes("\r\n")
                previous.diskResult = { ...disk, content, size: new TextEncoder().encode(content).length,
                    lineEnding: withoutCrlf.includes("\r") || (crlf && withoutCrlf.includes("\n")) ? "mixed" : crlf ? "crlf" : "lf" }
                if (remoteSave) remoteSave.applied = seq
            }
        }
        return result
    }
    // Remote saves must enter their backend-bound queue at request time, not after
    // another save settles. The highest-sequence successful write owns the baseline.
    if (remoteSave) return save().finally(() => {
        if (--remoteSave.pending === 0) pendingRemoteSaves.delete(path)
    })
    const pending = pendingSaves.get(path)
    const result = pending ? pending.then(save) : save()
    // The queue tail always settles successfully; callers still receive the real rejection.
    const tail = result.then(() => {}, () => {})
    pendingSaves.set(path, tail)
    void tail.then(() => { if (pendingSaves.get(path) === tail) pendingSaves.delete(path) })
    return result
}

export function documentGeneration(path: string): number {
    return generations.get(documentKey(path)) ?? generationBase
}
