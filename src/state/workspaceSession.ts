// Persisted workspace sessions (v2) — a per-workspace map of the first editor
// group's real-file tabs plus a last-workspace pointer. The pointer drives
// cold-start restore (SessionRestoreBridge); the map lets workspaceActions
// restore a workspace's tabs when switching back to it (#60 T4c, spec
// docs/specs/workspace-switch-async-and-cache.md §Phase 3.3). Only genuine
// file paths are stored; pseudo-tabs (preview + Herdr terminal pages) are
// filtered out before persisting and again on load. The legacy v1
// single-session format is migrated on read; the first write persists v2 and
// removes the v1 key.

import { isHerdrPagePath } from "../lib/herdrPages"

export const WORKSPACE_SESSION_V1_STORAGE_KEY = "yuzora.workspace.session.v1"
export const WORKSPACE_SESSION_STORAGE_KEY = "yuzora.workspace.session.v2"

// Keep this local so session storage never depends on the workspace store module.
const PREVIEW_TAB_PATH = "yuzora://preview"

// LRU cap on tracked workspaces; the least recently saved entry is evicted
// first. Entries are tab-path lists only (no buffers), so this is generous.
export const WORKSPACE_SESSION_MAX_WORKSPACES = 20

export interface WorkspaceSession {
    workspacePath: string
    tabs: string[]
    activePath: string | null
}

export interface WorkspaceSessionEntry {
    tabs: string[]
    activePath: string | null
}

// Object.keys insertion order doubles as LRU order: saves re-insert their key
// at the end, so the front of `workspaces` is always the eviction candidate.
interface WorkspaceSessionFileV2 {
    version: 2
    lastWorkspacePath: string | null
    workspaces: Record<string, WorkspaceSessionEntry>
}

function isEntry(value: unknown): value is WorkspaceSessionEntry {
    if (typeof value !== "object" || value === null) return false
    const v = value as Record<string, unknown>
    return (
        Array.isArray(v.tabs) &&
        v.tabs.every((t) => typeof t === "string") &&
        (v.activePath === null || typeof v.activePath === "string")
    )
}

function isV1Session(value: unknown): value is WorkspaceSession {
    if (typeof value !== "object" || value === null) return false
    const v = value as Record<string, unknown>
    return typeof v.workspacePath === "string" && isEntry(v)
}

function isV2File(value: unknown): value is WorkspaceSessionFileV2 {
    if (typeof value !== "object" || value === null) return false
    const v = value as Record<string, unknown>
    if (v.version !== 2) return false
    if (v.lastWorkspacePath !== null && typeof v.lastWorkspacePath !== "string") return false
    const workspaces = v.workspaces
    if (typeof workspaces !== "object" || workspaces === null || Array.isArray(workspaces)) {
        return false
    }
    return Object.values(workspaces).every(isEntry)
}

function emptyFile(): WorkspaceSessionFileV2 {
    return { version: 2, lastWorkspacePath: null, workspaces: {} }
}

/** Only real file paths are restorable — drop preview / Herdr pseudo paths. */
export function isPersistableSessionPath(path: string): boolean {
    if (!path || path === PREVIEW_TAB_PATH) return false
    if (isHerdrPagePath(path)) return false
    // Defensive: any other yuzora:// pseudo scheme stays out of session storage.
    if (path.startsWith("yuzora://")) return false
    return true
}

export function sanitizeSessionPaths(
    tabs: string[],
    activePath: string | null
): WorkspaceSessionEntry {
    const cleaned = tabs.filter(isPersistableSessionPath)
    const nextActive =
        activePath && cleaned.includes(activePath) ? activePath : null
    return { tabs: cleaned, activePath: nextActive }
}

function sanitizeEntry(entry: WorkspaceSessionEntry): WorkspaceSessionEntry {
    return sanitizeSessionPaths(entry.tabs, entry.activePath)
}

function sanitizeFile(file: WorkspaceSessionFileV2): WorkspaceSessionFileV2 {
    const workspaces: Record<string, WorkspaceSessionEntry> = {}
    for (const [path, entry] of Object.entries(file.workspaces)) {
        workspaces[path] = sanitizeEntry(entry)
    }
    return {
        version: 2,
        lastWorkspacePath: file.lastWorkspacePath,
        workspaces
    }
}

function readFile(): WorkspaceSessionFileV2 {
    try {
        const rawV2 = localStorage.getItem(WORKSPACE_SESSION_STORAGE_KEY)
        if (rawV2 !== null) {
            // A v2 store (even a corrupt one) wins over any lingering v1 key.
            const parsed: unknown = JSON.parse(rawV2)
            return isV2File(parsed) ? sanitizeFile(parsed) : emptyFile()
        }
        const rawV1 = localStorage.getItem(WORKSPACE_SESSION_V1_STORAGE_KEY)
        if (rawV1 !== null) {
            const parsed: unknown = JSON.parse(rawV1)
            if (isV1Session(parsed)) {
                return sanitizeFile({
                    version: 2,
                    lastWorkspacePath: parsed.workspacePath,
                    workspaces: {
                        [parsed.workspacePath]: {
                            tabs: parsed.tabs,
                            activePath: parsed.activePath
                        }
                    }
                })
            }
        }
    } catch {
        // Malformed JSON / storage unavailable — treat as no session.
    }
    return emptyFile()
}

function writeFile(file: WorkspaceSessionFileV2): void {
    localStorage.setItem(WORKSPACE_SESSION_STORAGE_KEY, JSON.stringify(file))
    // v1 is now migrated (or superseded); drop it so it can't resurface.
    localStorage.removeItem(WORKSPACE_SESSION_V1_STORAGE_KEY)
}

/** The session to restore on cold start: the last active workspace and its
 *  recorded tabs (empty when the workspace has no entry yet). */
export function loadWorkspaceSession(): WorkspaceSession | null {
    const file = readFile()
    if (file.lastWorkspacePath === null) return null
    const entry = file.workspaces[file.lastWorkspacePath]
    return {
        workspacePath: file.lastWorkspacePath,
        tabs: entry?.tabs ?? [],
        activePath: entry?.activePath ?? null
    }
}

/** The recorded tabs for one workspace — used when switching back to it. */
export function loadWorkspaceSessionEntry(workspacePath: string): WorkspaceSessionEntry | null {
    const entry = readFile().workspaces[workspacePath]
    return entry ? { tabs: entry.tabs, activePath: entry.activePath } : null
}

export function saveWorkspaceSession(session: WorkspaceSession): void {
    try {
        const file = readFile()
        // Delete-then-set moves the key to the end of insertion order (LRU touch).
        delete file.workspaces[session.workspacePath]
        file.workspaces[session.workspacePath] = sanitizeSessionPaths(
            session.tabs,
            session.activePath
        )
        file.lastWorkspacePath = session.workspacePath
        const keys = Object.keys(file.workspaces)
        while (keys.length > WORKSPACE_SESSION_MAX_WORKSPACES) {
            delete file.workspaces[keys.shift()!]
        }
        writeFile(file)
    } catch {
        // private mode / quota — session simply won't persist this run
    }
}

/**
 * Advances the last-workspace pointer without touching any entry. Used on
 * workspace switches, where the store passes through an empty-groups state:
 * persisting that transient state would clobber the target workspace's saved
 * tabs right before workspaceActions restores them.
 */
export function markWorkspaceSessionActive(workspacePath: string): void {
    try {
        const file = readFile()
        const entry = file.workspaces[workspacePath]
        if (entry) {
            // LRU touch: becoming active counts as recency.
            delete file.workspaces[workspacePath]
            file.workspaces[workspacePath] = entry
        }
        file.lastWorkspacePath = workspacePath
        writeFile(file)
    } catch {
        // ignore
    }
}

/** Drops the stale last workspace (folder moved/deleted on cold-start restore)
 *  while keeping every other workspace's recorded session. */
export function clearWorkspaceSession(): void {
    try {
        const file = readFile()
        if (file.lastWorkspacePath !== null) {
            delete file.workspaces[file.lastWorkspacePath]
            file.lastWorkspacePath = null
        }
        writeFile(file)
    } catch {
        // ignore
    }
}
