// Recent-workspaces list — localStorage-backed, reactive, with optional MRU
// promotion. Powers the workspace rail's RECENT tiles. Hand-written localStorage
// read/write mirrors sshStore's persistence shape (no persist middleware): the
// store is the authoritative in-memory copy, localStorage the durable mirror.

import { create } from "zustand"

import { canonicalPathKey } from "@/lib/paths"

export const RECENT_WORKSPACES_STORAGE_KEY = "yuzora.workspace.recent.v1"
export const RECENT_WORKSPACE_PRESENTATIONS_STORAGE_KEY =
    "yuzora.workspace.presentation.v1"
export const MOVE_OPENED_WORKSPACE_TO_TOP_STORAGE_KEY =
    "yuzora.workspace.move-opened-to-top.v1"

export const RECENT_WORKSPACE_COLOR_IDS = [
    "lime",
    "dusk",
    "sunrise",
    "mint",
    "coral",
    "ocean"
] as const

export type RecentWorkspaceColor = typeof RECENT_WORKSPACE_COLOR_IDS[number]

export interface RecentWorkspacePresentation {
    name?: string
    glyph?: string
    color?: RecentWorkspaceColor
}

export type RecentWorkspacePresentations = Record<string, RecentWorkspacePresentation>

const MAX_RECENT_WORKSPACES = 10

// Strip trailing slashes so "/a/b" and "/a/b/" dedupe to the same entry;
// keep a bare "/" as-is instead of collapsing it to "".
export function normalizeWorkspacePath(path: string): string {
    if (/^[A-Za-z]:[\\/]+$/.test(path) || /^[\\/]{2}\?[\\/][A-Za-z]:[\\/]+$/.test(path)) {
        return path
    }
    const stripped = path.replace(/[/\\]+$/, "")
    return stripped === "" ? "/" : stripped
}

export function loadRecentWorkspaces(): string[] {
    try {
        const raw = localStorage.getItem(RECENT_WORKSPACES_STORAGE_KEY)
        if (!raw) return []
        const parsed = JSON.parse(raw)
        if (!Array.isArray(parsed)) return []
        const seen = new Set<string>()
        return parsed
            .filter((p): p is string => typeof p === "string")
            .filter((path) => {
                const key = canonicalPathKey(path)
                if (seen.has(key)) return false
                seen.add(key)
                return true
            })
    } catch {
        // Malformed JSON (or storage unavailable) — reset to an empty list
        // rather than throw.
        return []
    }
}

export function loadMoveOpenedWorkspaceToTop(): boolean {
    try {
        return localStorage.getItem(MOVE_OPENED_WORKSPACE_TO_TOP_STORAGE_KEY) !== "false"
    } catch {
        return true
    }
}

function sanitizePresentation(value: unknown): RecentWorkspacePresentation | undefined {
    if (!value || typeof value !== "object" || Array.isArray(value)) return undefined

    const candidate = value as Record<string, unknown>
    const presentation: RecentWorkspacePresentation = {}
    if (typeof candidate.name === "string") presentation.name = candidate.name.slice(0, 80)
    if (typeof candidate.glyph === "string") presentation.glyph = candidate.glyph.slice(0, 16)
    if (
        typeof candidate.color === "string"
        && RECENT_WORKSPACE_COLOR_IDS.includes(candidate.color as RecentWorkspaceColor)
    ) {
        presentation.color = candidate.color as RecentWorkspaceColor
    }

    return Object.keys(presentation).length > 0 ? presentation : undefined
}

export function loadRecentWorkspacePresentations(): RecentWorkspacePresentations {
    try {
        const raw = localStorage.getItem(RECENT_WORKSPACE_PRESENTATIONS_STORAGE_KEY)
        if (!raw) return {}
        const parsed = JSON.parse(raw)
        if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return {}

        const presentations: RecentWorkspacePresentations = {}
        for (const [path, value] of Object.entries(parsed)) {
            const presentation = sanitizePresentation(value)
            if (presentation) presentations[canonicalPathKey(path)] = presentation
        }
        return presentations
    } catch {
        return {}
    }
}

function saveRecentWorkspaces(list: string[]): void {
    try {
        localStorage.setItem(RECENT_WORKSPACES_STORAGE_KEY, JSON.stringify(list))
    } catch {
        // private mode / quota — keep the in-memory list authoritative
    }
}

function saveMoveOpenedWorkspaceToTop(enabled: boolean): void {
    try {
        localStorage.setItem(MOVE_OPENED_WORKSPACE_TO_TOP_STORAGE_KEY, String(enabled))
    } catch {
        // private mode / quota — keep the in-memory preference authoritative
    }
}

function saveRecentWorkspacePresentations(presentations: RecentWorkspacePresentations): void {
    try {
        localStorage.setItem(
            RECENT_WORKSPACE_PRESENTATIONS_STORAGE_KEY,
            JSON.stringify(presentations)
        )
    } catch {
        // private mode / quota — keep the in-memory metadata authoritative
    }
}

interface RecentWorkspacesStore {
    list: string[]
    presentations: RecentWorkspacePresentations
    moveOpenedWorkspaceToTop: boolean
    /** Record a successfully-opened workspace (deduped, capped). New paths are
     * added first; existing paths move only when the preference is enabled. */
    record: (path: string) => void
    setMoveOpenedWorkspaceToTop: (enabled: boolean) => void
    presentationFor: (path: string) => RecentWorkspacePresentation | undefined
    updatePresentation: (path: string, patch: Partial<RecentWorkspacePresentation>) => void
    /** Drop `path` and its presentation metadata without touching the folder. */
    remove: (path: string) => void
}

export const useRecentWorkspacesStore = create<RecentWorkspacesStore>()((set, get) => ({
    list: loadRecentWorkspaces(),
    presentations: loadRecentWorkspacePresentations(),
    moveOpenedWorkspaceToTop: loadMoveOpenedWorkspaceToTop(),

    record: (path) => {
        const normalized = normalizeWorkspacePath(path)
        const current = get().list
        const key = canonicalPathKey(normalized)
        const alreadyRecorded = current.some((p) => canonicalPathKey(p) === key)
        if (!get().moveOpenedWorkspaceToTop && alreadyRecorded) return
        const next = [
            normalized,
            ...current.filter((p) => canonicalPathKey(p) !== key)
        ].slice(
            0,
            MAX_RECENT_WORKSPACES
        )
        saveRecentWorkspaces(next)
        set({ list: next })
    },

    setMoveOpenedWorkspaceToTop: (enabled) => {
        saveMoveOpenedWorkspaceToTop(enabled)
        set({ moveOpenedWorkspaceToTop: enabled })
    },

    presentationFor: (path) => get().presentations[canonicalPathKey(path)],

    updatePresentation: (path, patch) => {
        const key = canonicalPathKey(path)
        const presentation = sanitizePresentation({
            ...get().presentations[key],
            ...patch
        })
        const presentations = { ...get().presentations }
        if (presentation) presentations[key] = presentation
        else delete presentations[key]
        saveRecentWorkspacePresentations(presentations)
        set({ presentations })
    },

    remove: (path) => {
        const normalized = normalizeWorkspacePath(path)
        const key = canonicalPathKey(normalized)
        const next = get().list.filter((p) => canonicalPathKey(p) !== key)
        const presentations = { ...get().presentations }
        delete presentations[key]
        saveRecentWorkspaces(next)
        saveRecentWorkspacePresentations(presentations)
        set({ list: next, presentations })
    }
}))
