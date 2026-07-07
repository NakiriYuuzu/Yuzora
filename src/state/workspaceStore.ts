import { create } from "zustand"

// Singleton preview tab: a reserved sentinel path keeps preview inside the
// path-keyed tab model without ever being mistaken for a real file. Only one
// preview tab exists app-wide (see openPreviewTab).
export const PREVIEW_TAB_PATH = "yuzora://preview"
export const PREVIEW_TAB_NAME = "Preview"

export interface TabInfo {
    path: string
    name: string
    dirty: boolean
    externallyModified: boolean
    // Absent ⇒ a normal file tab. "preview" marks the singleton preview tab so
    // EditorArea/TabBar can special-case it without touching file-path logic.
    kind?: "file" | "preview"
}

export interface EditorGroup {
    tabs: TabInfo[]
    activePath: string | null
}

export interface PendingReveal {
    path: string
    line: number
    // Whether revealing should also steal editor focus. Omitted (undefined) means
    // the default — navigations (go-to-definition, symbol jump) focus the editor;
    // search-result clicks pass false to reveal-only, preserving M2 behaviour (A4).
    // Consumers apply `?? true`.
    focus?: boolean
}

interface WorkspaceState {
    workspacePath: string | null
    groups: EditorGroup[]
    activeGroupIndex: number
    pendingReveal: PendingReveal | null
    // Bumped after a file-tree mutation (new/rename/delete via the context menu)
    // to force FileTree to re-list. The FileTree doesn't subscribe to the fs
    // watcher, so these in-app operations need an explicit refresh signal.
    treeRevision: number
    setWorkspace: (path: string) => void
    openTab: (path: string) => void
    refreshTree: () => void
    closeTab: (groupIndex: number, path: string) => void
    closeOtherTabs: (groupIndex: number, keepPath: string) => void
    closeAllTabs: (groupIndex: number) => void
    closeTabsByPath: (paths: string[]) => void
    updateTabPath: (fromPath: string, toPath: string) => void
    openPreviewTab: () => void
    closePreviewTab: () => void
    togglePreviewTab: () => void
    setActiveTab: (groupIndex: number, path: string) => void
    setActiveGroup: (groupIndex: number) => void
    markDirty: (path: string, dirty: boolean) => void
    markExternallyModified: (path: string, modified: boolean) => void
    splitRight: () => void
    closeSplit: () => void
    requestReveal: (path: string, line: number, focus?: boolean) => void
    consumeReveal: () => void
}

const emptyGroup = (): EditorGroup => ({ tabs: [], activePath: null })

export const useWorkspaceStore = create<WorkspaceState>((set, get) => ({
    workspacePath: null,
    groups: [emptyGroup()],
    activeGroupIndex: 0,
    pendingReveal: null,
    treeRevision: 0,
    setWorkspace: (path) =>
        set({
            workspacePath: path,
            groups: [emptyGroup()],
            activeGroupIndex: 0,
            pendingReveal: null
        }),
    refreshTree: () => set((s) => ({ treeRevision: s.treeRevision + 1 })),
    openTab: (path) =>
        set((s) => {
            const groups = s.groups.map((g) => ({ ...g, tabs: [...g.tabs] }))
            const g = groups[s.activeGroupIndex]
            if (!g.tabs.some((t) => t.path === path)) {
                g.tabs.push({
                    path,
                    name: path.split("/").pop() ?? path,
                    dirty: false,
                    externallyModified: false
                })
            }
            g.activePath = path
            return { groups }
        }),
    closeTab: (groupIndex, path) =>
        set((s) => {
            const groups = s.groups.map((g) => ({ ...g, tabs: [...g.tabs] }))
            const g = groups[groupIndex]
            g.tabs = g.tabs.filter((t) => t.path !== path)
            if (g.activePath === path) g.activePath = g.tabs.at(-1)?.path ?? null
            return { groups }
        }),
    // Pure tab-list mutations for the tab context menu's "Close others" /
    // "Close all" — the confirm-dialog + document/preview cleanup side effects
    // (mirroring TabBar's onClose) live in contextMenuStore, which calls these
    // after resolving any dirty-tab confirmation.
    closeOtherTabs: (groupIndex, keepPath) =>
        set((s) => ({
            groups: s.groups.map((g, i) => {
                if (i !== groupIndex) return g
                const tabs = g.tabs.filter((t) => t.path === keepPath)
                return { tabs, activePath: tabs.length > 0 ? keepPath : null }
            })
        })),
    closeAllTabs: (groupIndex) =>
        set((s) => ({
            groups: s.groups.map((g, i) => (i === groupIndex ? { tabs: [], activePath: null } : g))
        })),
    // Bulk close every tab (across ALL groups) whose path is in `paths`. Used
    // after a file/folder delete: a tab left pointing at a now-gone path would
    // let its EditorPane recreate the file on the next save. activePath falls
    // back to the last surviving tab, mirroring closeTab's rule.
    closeTabsByPath: (paths) =>
        set((s) => {
            const drop = new Set(paths)
            return {
                groups: s.groups.map((g) => {
                    const tabs = g.tabs.filter((t) => !drop.has(t.path))
                    if (tabs.length === g.tabs.length) return g
                    const activePath =
                        g.activePath !== null && drop.has(g.activePath)
                            ? tabs.at(-1)?.path ?? null
                            : g.activePath
                    return { tabs, activePath }
                })
            }
        }),
    // Re-point tabs (across ALL groups) after a file/folder rename: a tab at
    // exactly `fromPath` moves to `toPath`; a tab under `fromPath/` (folder
    // rename) has its prefix rewritten. path + name (+ any matching activePath)
    // are updated in place, preserving the tab's dirty flag and position.
    updateTabPath: (fromPath, toPath) =>
        set((s) => {
            const remap = (p: string): string | null => {
                if (p === fromPath) return toPath
                if (p.startsWith(fromPath + "/")) return toPath + p.slice(fromPath.length)
                return null
            }
            return {
                groups: s.groups.map((g) => {
                    let changed = false
                    const tabs = g.tabs.map((t) => {
                        const np = remap(t.path)
                        if (np === null) return t
                        changed = true
                        return { ...t, path: np, name: np.split("/").pop() ?? np }
                    })
                    const activePath =
                        g.activePath !== null ? remap(g.activePath) ?? g.activePath : null
                    if (!changed && activePath === g.activePath) return g
                    return { tabs, activePath }
                })
            }
        }),
    openPreviewTab: () =>
        set((s) => {
            // Singleton: if a preview tab already exists in any group, just
            // focus it (and its group) rather than opening a second one.
            const existing = s.groups.findIndex((g) =>
                g.tabs.some((t) => t.path === PREVIEW_TAB_PATH)
            )
            if (existing !== -1) {
                return {
                    groups: s.groups.map((g, i) =>
                        i === existing ? { ...g, activePath: PREVIEW_TAB_PATH } : g
                    ),
                    activeGroupIndex: existing
                }
            }
            const groups = s.groups.map((g) => ({ ...g, tabs: [...g.tabs] }))
            const g = groups[s.activeGroupIndex]
            g.tabs.push({
                path: PREVIEW_TAB_PATH,
                name: PREVIEW_TAB_NAME,
                dirty: false,
                externallyModified: false,
                kind: "preview"
            })
            g.activePath = PREVIEW_TAB_PATH
            return { groups }
        }),
    closePreviewTab: () =>
        set((s) => ({
            groups: s.groups.map((g) => {
                if (!g.tabs.some((t) => t.path === PREVIEW_TAB_PATH)) return g
                const tabs = g.tabs.filter((t) => t.path !== PREVIEW_TAB_PATH)
                return {
                    tabs,
                    activePath:
                        g.activePath === PREVIEW_TAB_PATH
                            ? tabs.at(-1)?.path ?? null
                            : g.activePath
                }
            })
        })),
    togglePreviewTab: () => {
        // Focused preview ⇒ close; otherwise open-or-focus (rail toggle semantics).
        const s = get()
        if (s.groups[s.activeGroupIndex]?.activePath === PREVIEW_TAB_PATH) {
            get().closePreviewTab()
        } else {
            get().openPreviewTab()
        }
    },
    setActiveTab: (groupIndex, path) =>
        set((s) => {
            const groups = s.groups.map((g, i) =>
                i === groupIndex ? { ...g, activePath: path } : g
            )
            return { groups, activeGroupIndex: groupIndex }
        }),
    setActiveGroup: (groupIndex) =>
        set((s) => (s.groups[groupIndex] ? { activeGroupIndex: groupIndex } : s)),
    markDirty: (path, dirty) =>
        set((s) => ({
            groups: s.groups.map((g) => ({
                ...g,
                tabs: g.tabs.map((t) => (t.path === path ? { ...t, dirty } : t))
            }))
        })),
    markExternallyModified: (path, modified) =>
        set((s) => ({
            groups: s.groups.map((g) => ({
                ...g,
                tabs: g.tabs.map((t) =>
                    t.path === path ? { ...t, externallyModified: modified } : t
                )
            }))
        })),
    splitRight: () =>
        set((s) => (s.groups.length >= 2 ? s : { groups: [...s.groups, emptyGroup()] })),
    closeSplit: () =>
        set((s) =>
            s.groups.length < 2
                ? s
                : { groups: [s.groups[0]], activeGroupIndex: 0 }
        ),
    requestReveal: (path, line, focus) => {
        get().openTab(path)
        // Store focus only when specified so callers that omit it keep the default
        // (consumed as `?? true`) and the pendingReveal shape stays minimal.
        set({ pendingReveal: focus === undefined ? { path, line } : { path, line, focus } })
    },
    consumeReveal: () => set({ pendingReveal: null })
}))
