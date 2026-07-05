import { create } from "zustand"

export interface TabInfo {
    path: string
    name: string
    dirty: boolean
    externallyModified: boolean
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
    setWorkspace: (path: string) => void
    openTab: (path: string) => void
    closeTab: (groupIndex: number, path: string) => void
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
    setWorkspace: (path) =>
        set({
            workspacePath: path,
            groups: [emptyGroup()],
            activeGroupIndex: 0,
            pendingReveal: null
        }),
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
