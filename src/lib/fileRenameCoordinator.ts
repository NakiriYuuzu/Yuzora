import i18n from "./i18n"
import { isSameOrDescendantPath } from "./paths"
import { parseRemoteFilePath, relativeRemoteHostPath } from "./runtimeIdentity"

// A rename changes the physical file even when another workspace owns its UI.
// Remote capability roots must not make overlapping host paths independent.
export function containsFilePath(root: string, path: string): boolean {
    const remoteRoot = parseRemoteFilePath(root), remotePath = parseRemoteFilePath(path)
    if (remoteRoot || remotePath) {
        return !!remoteRoot && !!remotePath && remoteRoot.hostId === remotePath.hostId
            && relativeRemoteHostPath(remoteRoot.path, remotePath.path) !== null
    }
    return isSameOrDescendantPath(root, path)
}

export function fileRenameError(reason: "busy" | "destinationOpen"): Error {
    return new Error(i18n.t(`fileRename.${reason}`, { ns: "menus" }))
}

interface Rename {
    from: string
    to: string
    settled: Promise<void>
}
const renames = new Set<Rename>()
const reads = new Set<{ path: string }>()
const writes = new Set<{ path: string }>()
const touches = (rename: Pick<Rename, "from" | "to">, path: string) =>
    containsFilePath(rename.from, path) || containsFilePath(rename.to, path)

export function pendingFileRename(path: string): Promise<void> | undefined {
    const pending = [...renames].filter(rename => touches(rename, path))
    return pending.length ? Promise.all(pending.map(rename => rename.settled)).then(() => {}) : undefined
}

export function reserveFileRename(from: string, to: string): () => void {
    const scope = { from, to }
    if ([...reads, ...writes].some(operation => touches(scope, operation.path))
        || [...renames].some(other => touches(scope, other.from) || touches(scope, other.to)
            || touches(other, from) || touches(other, to))) throw fileRenameError("busy")
    let settle!: () => void
    const reservation = { ...scope, settled: new Promise<void>(resolve => { settle = resolve }) }
    renames.add(reservation)
    return () => { renames.delete(reservation); settle() }
}

export async function withFileRead<T>(path: string, read: () => Promise<T>): Promise<T> {
    let pending: Promise<void> | undefined
    while ((pending = pendingFileRename(path))) await pending
    const operation = { path }
    reads.add(operation)
    try { return await read() } finally { reads.delete(operation) }
}

export async function withFileWrite<T>(path: string, write: () => Promise<T>): Promise<T> {
    // Do not queue a write to the old name: after rename that could recreate it.
    if (pendingFileRename(path)) throw fileRenameError("busy")
    const operation = { path }
    writes.add(operation)
    try { return await write() } finally { writes.delete(operation) }
}
