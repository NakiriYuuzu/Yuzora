// Recent-workspaces MRU list — localStorage-backed, reactive. Powers the
// workspace rail's RECENT tiles. Hand-written localStorage read/write mirrors
// sshStore's persistence shape (no persist middleware): the store is the
// authoritative in-memory copy, localStorage the durable mirror.

import { create } from "zustand"

export const RECENT_WORKSPACES_STORAGE_KEY = "yuzora.workspace.recent.v1"

const MAX_RECENT_WORKSPACES = 10

// Strip trailing slashes so "/a/b" and "/a/b/" dedupe to the same entry;
// keep a bare "/" as-is instead of collapsing it to "".
export function normalizeWorkspacePath(path: string): string {
    const stripped = path.replace(/\/+$/, "")
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

function saveRecentWorkspaces(list: string[]): void {
    try {
        localStorage.setItem(RECENT_WORKSPACES_STORAGE_KEY, JSON.stringify(list))
    } catch {
        // private mode / quota — keep the in-memory list authoritative
    }
}

interface RecentWorkspacesStore {
    list: string[]
    /** Move `path` to the front of the MRU list (deduped, capped). Call once a
     * workspace has actually opened successfully. */
    record: (path: string) => void
    /** Drop `path` — used when opening a recent entry fails (folder moved/deleted). */
    remove: (path: string) => void
}

export const useRecentWorkspacesStore = create<RecentWorkspacesStore>()((set, get) => ({
    list: loadRecentWorkspaces(),

    record: (path) => {
        const normalized = normalizeWorkspacePath(path)
        const next = [normalized, ...get().list.filter((p) => p !== normalized)].slice(
            0,
            MAX_RECENT_WORKSPACES
        )
        saveRecentWorkspaces(next)
        set({ list: next })
    },

    remove: (path) => {
        const normalized = normalizeWorkspacePath(path)
        const next = get().list.filter((p) => p !== normalized)
        saveRecentWorkspaces(next)
        set({ list: next })
    }
}))
