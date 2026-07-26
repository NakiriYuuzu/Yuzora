import { create } from "zustand"

import { listDir } from "../lib/ipc"
import type { FileNode } from "../lib/types"
import { useWorkspaceStore } from "./workspaceStore"

// Per-workspace file-tree state (#59 T4b, spec Phase 3.2), bucketed by workspace
// root the same way terminalStore buckets layouts: switching workspaces keeps
// every visited tree (root listing, lazily-loaded children, expanded dirs,
// scroll offset) so switching back hydrates instantly, then revalidates in the
// background instead of starting from a blank remount.
export interface WorkspaceTree {
    rootNodes: FileNode[]
    // Lazily-listed children of every dir the user has expanded at least once
    // (kept across collapse so re-expanding is instant; revalidation refreshes it).
    childrenByDir: Record<string, FileNode[]>
    expandedDirs: ReadonlySet<string>
    scrollTop: number
}

// treeRevision (workspaceStore) stays the shared invalidation authority — the
// AgentZone mention index and the explorer "Refresh" menu item key off it. A
// precise invalidation (external fs change, context-menu file op) also bumps it
// for those consumers, but records the bumped revision here so FileTree can
// tell "already applied precisely — skip the full revalidate" apart from an
// unmarked bump (cmRefresh) that should revalidate the whole visible tree.
interface PreciseRevision {
    root: string
    revision: number
}

interface FileTreeState {
    trees: Record<string, WorkspaceTree>
    preciseRevision: PreciseRevision | null
    // Hydrate-or-revalidate: first visit lists the root; a revisit re-lists the
    // root plus every expanded dir (bounded concurrency) and diff-applies.
    ensureTree: (root: string) => Promise<void>
    toggleDir: (root: string, dir: string) => Promise<void>
    setScrollTop: (root: string, scrollTop: number) => void
    // Precise invalidation: re-list only the cached directories that contain the
    // changed paths. Bumps treeRevision (mention-index compat) and marks it.
    invalidatePaths: (root: string, paths: string[]) => Promise<void>
    // One-shot: true (and clears the marker) when `revision` was already handled
    // by a precise invalidation for `root`, so callers skip a full revalidate.
    consumePreciseRevision: (root: string, revision: number) => boolean
}

// Bounded parallelism for background revalidation (spec Phase 3.2: 併發上限 8).
export const REVALIDATE_CONCURRENCY = 8

const emptyTree = (): WorkspaceTree => ({
    rootNodes: [],
    childrenByDir: {},
    expandedDirs: new Set(),
    scrollTop: 0
})

// Paths come from the fs watcher / context menu as absolute paths with either
// separator (Windows keeps backslashes), matching contextMenuStore's handling.
function parentOf(path: string): string {
    const i = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"))
    return i > 0 ? path.slice(0, i) : path
}

function isUnder(path: string, ancestor: string): boolean {
    return path.startsWith(ancestor + "/") || path.startsWith(ancestor + "\\")
}

function sameNodes(a: FileNode[], b: FileNode[]): boolean {
    if (a.length !== b.length) return false
    return a.every(
        (n, i) => n.path === b[i].path && n.name === b[i].name && n.isDir === b[i].isDir
    )
}

async function mapLimit<T>(
    items: readonly T[],
    limit: number,
    fn: (item: T) => Promise<void>
): Promise<void> {
    const queue = [...items]
    const workers = Array.from({ length: Math.min(limit, queue.length) }, async () => {
        for (let item = queue.shift(); item !== undefined; item = queue.shift()) {
            await fn(item)
        }
    })
    await Promise.all(workers)
}

export const useFileTreeStore = create<FileTreeState>((set, get) => {
    // Diff-apply a fresh listing of `dir`: keep the old array reference when
    // nothing changed (no re-render churn), otherwise swap it in and prune the
    // cached subtrees of subdirectories that no longer exist under `dir`.
    function applyListing(root: string, dir: string, nodes: FileNode[]): void {
        set((s) => {
            const tree = s.trees[root]
            if (!tree) return s
            const prev = dir === root ? tree.rootNodes : tree.childrenByDir[dir]
            if (prev !== undefined && sameNodes(prev, nodes)) return s
            const liveDirs = new Set(nodes.filter((n) => n.isDir).map((n) => n.path))
            const removed = Object.keys(tree.childrenByDir).filter(
                (key) => parentOf(key) === dir && !liveDirs.has(key)
            )
            const childrenByDir = { ...tree.childrenByDir }
            const expandedDirs = new Set(tree.expandedDirs)
            for (const gone of removed) {
                for (const key of Object.keys(childrenByDir)) {
                    if (key === gone || isUnder(key, gone)) delete childrenByDir[key]
                }
                for (const key of [...expandedDirs]) {
                    if (key === gone || isUnder(key, gone)) expandedDirs.delete(key)
                }
            }
            if (dir !== root) childrenByDir[dir] = nodes
            return {
                trees: {
                    ...s.trees,
                    [root]: {
                        ...tree,
                        rootNodes: dir === root ? nodes : tree.rootNodes,
                        childrenByDir,
                        expandedDirs
                    }
                }
            }
        })
    }

    // Listing failed (the directory was deleted out from under us): drop its
    // cached subtree + expansion flags. The parent's own re-list removes the
    // node from view.
    function dropSubtree(root: string, dir: string): void {
        set((s) => {
            const tree = s.trees[root]
            if (!tree) return s
            const childrenByDir = { ...tree.childrenByDir }
            const expandedDirs = new Set(tree.expandedDirs)
            for (const key of Object.keys(childrenByDir)) {
                if (key === dir || isUnder(key, dir)) delete childrenByDir[key]
            }
            for (const key of [...expandedDirs]) {
                if (key === dir || isUnder(key, dir)) expandedDirs.delete(key)
            }
            return { trees: { ...s.trees, [root]: { ...tree, childrenByDir, expandedDirs } } }
        })
    }

    async function relistDir(root: string, dir: string): Promise<void> {
        try {
            const nodes = await listDir(dir)
            applyListing(root, dir, Array.isArray(nodes) ? nodes : [])
        } catch {
            // Keep the last-known root listing on a root failure (transient IO
            // errors shouldn't blank the whole tree); drop deleted subdirs.
            if (dir !== root) dropSubtree(root, dir)
        }
    }

    return {
        trees: {},
        preciseRevision: null,
        ensureTree: async (root) => {
            const tree = get().trees[root]
            if (!tree) {
                set((s) => ({ trees: { ...s.trees, [root]: emptyTree() } }))
                await relistDir(root, root)
                return
            }
            await mapLimit(
                [root, ...tree.expandedDirs],
                REVALIDATE_CONCURRENCY,
                (dir) => relistDir(root, dir)
            )
        },
        toggleDir: async (root, dir) => {
            const tree = get().trees[root]
            if (!tree) return
            const expanding = !tree.expandedDirs.has(dir)
            set((s) => {
                const cur = s.trees[root]
                if (!cur) return s
                const expandedDirs = new Set(cur.expandedDirs)
                if (expanding) expandedDirs.add(dir)
                else expandedDirs.delete(dir)
                return { trees: { ...s.trees, [root]: { ...cur, expandedDirs } } }
            })
            if (expanding && tree.childrenByDir[dir] === undefined) {
                await relistDir(root, dir)
            }
        },
        setScrollTop: (root, scrollTop) =>
            set((s) => {
                const tree = s.trees[root]
                if (!tree || tree.scrollTop === scrollTop) return s
                return { trees: { ...s.trees, [root]: { ...tree, scrollTop } } }
            }),
        invalidatePaths: async (root, paths) => {
            // Bump first (synchronously) so mention-index consumers and existing
            // treeRevision expectations behave exactly like refreshTree() did.
            useWorkspaceStore.getState().refreshTree()
            const tree = get().trees[root]
            // No tree yet → nothing cached to invalidate; leave the revision
            // unmarked so a mounted FileTree falls back to a full ensureTree.
            if (!tree) return
            set({
                preciseRevision: { root, revision: useWorkspaceStore.getState().treeRevision }
            })
            const dirs = new Set<string>()
            for (const path of paths) {
                const dir = path === root ? root : parentOf(path)
                // Only re-list what is visible/cached: the root listing, or a dir
                // whose children we have. Changes inside never-expanded dirs get
                // listed fresh on first expand anyway.
                if (dir === root || tree.childrenByDir[dir] !== undefined) dirs.add(dir)
            }
            await mapLimit([...dirs], REVALIDATE_CONCURRENCY, (dir) => relistDir(root, dir))
        },
        consumePreciseRevision: (root, revision) => {
            const marker = get().preciseRevision
            if (!marker || marker.root !== root || marker.revision !== revision) return false
            set({ preciseRevision: null })
            return true
        }
    }
})
