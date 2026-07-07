// Persisted workspace session — the last open workspace plus its first editor
// group's real-file tabs, restored on the next launch by SessionRestoreBridge.
// Only genuine file paths are stored; pseudo-tabs (the singleton preview tab)
// are filtered out before persisting.

export const WORKSPACE_SESSION_STORAGE_KEY = "yuzora.workspace.session.v1"

export interface WorkspaceSession {
    workspacePath: string
    tabs: string[]
    activePath: string | null
}

function isWorkspaceSession(value: unknown): value is WorkspaceSession {
    if (typeof value !== "object" || value === null) return false
    const v = value as Record<string, unknown>
    return (
        typeof v.workspacePath === "string" &&
        Array.isArray(v.tabs) &&
        v.tabs.every((t) => typeof t === "string") &&
        (v.activePath === null || typeof v.activePath === "string")
    )
}

export function loadWorkspaceSession(): WorkspaceSession | null {
    try {
        const raw = localStorage.getItem(WORKSPACE_SESSION_STORAGE_KEY)
        if (!raw) return null
        const parsed = JSON.parse(raw)
        return isWorkspaceSession(parsed) ? parsed : null
    } catch {
        return null
    }
}

export function saveWorkspaceSession(session: WorkspaceSession): void {
    try {
        localStorage.setItem(WORKSPACE_SESSION_STORAGE_KEY, JSON.stringify(session))
    } catch {
        // private mode / quota — session simply won't persist this run
    }
}

export function clearWorkspaceSession(): void {
    try {
        localStorage.removeItem(WORKSPACE_SESSION_STORAGE_KEY)
    } catch {
        // ignore
    }
}
