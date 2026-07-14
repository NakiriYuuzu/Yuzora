// Recent-workspaces list — localStorage-backed, reactive, with optional MRU
// promotion. Powers the workspace rail's RECENT tiles. Hand-written localStorage
// read/write mirrors sshStore's persistence shape (no persist middleware): the
// store is the authoritative in-memory copy, localStorage the durable mirror.

import { create } from "zustand"

export const RECENT_WORKSPACES_STORAGE_KEY = "yuzora.workspace.recent.v1"
export const MOVE_OPENED_WORKSPACE_TO_TOP_STORAGE_KEY =
    "yuzora.workspace.move-opened-to-top.v1"

const MAX_RECENT_WORKSPACES = 10

// Strip trailing slashes so "/a/b" and "/a/b/" dedupe to the same entry;
// keep a bare "/" as-is instead of collapsing it to "".
export function normalizeWorkspacePath(path: string): string {
    const stripped = path.replace(/[/\\]+$/, "")
    return stripped === "" ? "/" : stripped
}

export function loadRecentWorkspaces(): string[] {
    try {
        const raw = localStorage.getItem(RECENT_WORKSPACES_STORAGE_KEY)
        if (!raw) return []
        const parsed = JSON.parse(raw)
        if (!Array.isArray(parsed)) return []
        return parsed.filter((p): p is string => typeof p === "string")
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

interface RecentWorkspacesStore {
    list: string[]
    moveOpenedWorkspaceToTop: boolean
    /** Record a successfully-opened workspace (deduped, capped). New paths are
     * added first; existing paths move only when the preference is enabled. */
    record: (path: string) => void
    setMoveOpenedWorkspaceToTop: (enabled: boolean) => void
    /** Drop `path` — used when opening a recent entry fails (folder moved/deleted). */
    remove: (path: string) => void
}

export const useRecentWorkspacesStore = create<RecentWorkspacesStore>()((set, get) => ({
    list: loadRecentWorkspaces(),
    moveOpenedWorkspaceToTop: loadMoveOpenedWorkspaceToTop(),

    record: (path) => {
        const normalized = normalizeWorkspacePath(path)
        const current = get().list
        if (!get().moveOpenedWorkspaceToTop && current.includes(normalized)) return
        const next = [normalized, ...current.filter((p) => p !== normalized)].slice(
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

    remove: (path) => {
        const normalized = normalizeWorkspacePath(path)
        const next = get().list.filter((p) => p !== normalized)
        saveRecentWorkspaces(next)
        set({ list: next })
    }
}))
