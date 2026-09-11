import { useLayoutEffect, useMemo, useRef, useState } from "react"
import { listDir } from "@/lib/ipc"
import { canonicalPathKey, relativePathWithin } from "@/lib/paths"
import type { FileNode } from "@/lib/types"

export interface FileNameSearchResult {
    files: FileNode[]
    loading: boolean
    incomplete: boolean
    error: string | null
}

const EMPTY: FileNameSearchResult = { files: [], loading: false, incomplete: false, error: null }
const MAX_FILES = 200
const MAX_DIRECTORIES = 2000
const CONCURRENCY = 4
const DEBOUNCE_MS = 250

/** Independent filename/path traversal; never reads file contents or expands the UI tree. */
export function useFileNameSearch(root: string | null, query: string, revision: number, active: boolean): FileNameSearchResult {
    const needle = query.trim().replaceAll("\\", "/").toLowerCase()
    const request = useMemo(() => active && root && needle ? { root, needle, revision } : null, [root, needle, revision, active])
    const [completed, setCompleted] = useState<{ request: typeof request; result: FileNameSearchResult } | null>(null)
    // Retiring scans share the same four slots. listDir has no native abort API.
    const pending = useRef(new Set<Promise<unknown>>())

    useLayoutEffect(() => {
        if (!request) return
        const { root, needle } = request
        let cancelled = false
        const list = async (directory: string) => {
            while (pending.current.size >= CONCURRENCY) {
                await Promise.race(pending.current)
                if (cancelled) return null
            }
            if (cancelled) return null
            const listingRequest = (async () => {
                try { return { ok: true as const, nodes: await listDir(directory) } }
                catch (error) { return { ok: false as const, error: error instanceof Error ? error.message : String(error) } }
            })()
            pending.current.add(listingRequest)
            try { return await listingRequest }
            finally { pending.current.delete(listingRequest) }
        }
        const scan = async () => {
            const queue = [root]
            const visited = new Set([canonicalPathKey(root)])
            const found = new Map<string, FileNode>()
            let incomplete = false
            let error: string | null = null
            while (!cancelled && queue.length && found.size < MAX_FILES) {
                const directories = queue.splice(0, CONCURRENCY)
                const listings = await Promise.all(directories.map(list))
                if (cancelled) return
                for (let index = 0; index < listings.length && found.size < MAX_FILES; index++) {
                    const listing = listings[index]
                    if (!listing) continue
                    if (!listing.ok) {
                        if (directories[index] === root) error = listing.error
                        else incomplete = true
                        continue
                    }
                    for (const node of listing.nodes) {
                        if (node.name === ".git" || node.kind === "symlink" || node.kind === "other") continue
                        const relative = relativePathWithin(root, node.path)
                        const child = relativePathWithin(directories[index], node.path)
                        if (!relative || !child || relative.split("/").some((part) => part === "." || part === ".." || part === ".git")) continue
                        const identity = canonicalPathKey(node.path)
                        if (node.isDir && (!node.kind || node.kind === "directory")) {
                            if (visited.has(identity)) continue
                            if (visited.size >= MAX_DIRECTORIES) { incomplete = true; continue }
                            visited.add(identity)
                            queue.push(node.path)
                        } else if (!node.isDir && (!node.kind || node.kind === "file") && relative.toLowerCase().includes(needle)) {
                            found.set(identity, node)
                            if (found.size === MAX_FILES) { incomplete = true; break }
                        }
                    }
                }
            }
            if (!cancelled) setCompleted({ request, result: {
                files: [...found.values()].sort((a, b) => a.path.localeCompare(b.path)),
                loading: false, incomplete, error,
            } })
        }
        const timer = setTimeout(() => { void scan() }, DEBOUNCE_MS)
        return () => { cancelled = true; clearTimeout(timer) }
    }, [request])

    if (!request) return EMPTY
    return completed?.request === request ? completed.result : { ...EMPTY, loading: true }
}
