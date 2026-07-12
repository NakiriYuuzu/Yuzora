import { create } from "zustand"

export type TerminalSplitDirection = "right" | "down"

export interface TerminalSessionMeta {
    sessionId: string
    title: string
    workspace: string
    shell: string
    cols: number
    rows: number
}

export interface TerminalPane {
    paneId: string
    sessionId: string
}

export interface TerminalWorkspaceLayout {
    panes: TerminalPane[]
    activePaneId: string | null
    splitDirection: TerminalSplitDirection | null
}

interface TerminalState {
    sessions: Record<string, TerminalSessionMeta>
    layouts: Record<string, TerminalWorkspaceLayout>
    addSession: (workspace: string, meta: TerminalSessionMeta, paneId?: string) => void
    removeSession: (workspace: string, sessionId: string) => void
    setActivePane: (workspace: string, paneId: string) => void
    splitFrom: (
        workspace: string,
        paneId: string,
        meta: TerminalSessionMeta,
        direction?: TerminalSplitDirection
    ) => void
    sessionsForWorkspace: (workspace: string) => TerminalSessionMeta[]
    reset: () => void
}

export const MAX_PANES = 2

export const terminalInitialState = {
    sessions: {} as Record<string, TerminalSessionMeta>,
    layouts: {} as Record<string, TerminalWorkspaceLayout>
}

function emptyLayout(): TerminalWorkspaceLayout {
    return {
        panes: [],
        activePaneId: null,
        splitDirection: null
    }
}

export const useTerminalStore = create<TerminalState>()((set, get) => ({
    ...terminalInitialState,

    addSession: (workspace, meta, paneId) =>
        set((s) => {
            const layout = s.layouts[workspace] ?? emptyLayout()
            if (layout.panes.length >= MAX_PANES) return s
            const nextPaneId = paneId ?? meta.sessionId
            return {
                sessions: { ...s.sessions, [meta.sessionId]: meta },
                layouts: {
                    ...s.layouts,
                    [workspace]: {
                        ...layout,
                        panes: [...layout.panes, { paneId: nextPaneId, sessionId: meta.sessionId }],
                        activePaneId: nextPaneId
                    }
                }
            }
        }),

    removeSession: (workspace, sessionId) =>
        set((s) => {
            const session = s.sessions[sessionId]
            if (session && session.workspace !== workspace) return s
            const layout = s.layouts[workspace] ?? emptyLayout()
            const removedIndex = layout.panes.findIndex((pane) => pane.sessionId === sessionId)
            const sessions = { ...s.sessions }
            delete sessions[sessionId]
            const panes = layout.panes.filter((pane) => pane.sessionId !== sessionId)
            const destination = removedIndex >= 0
                ? layout.panes[removedIndex + 1] ?? layout.panes[removedIndex - 1]
                : undefined
            const activePaneId = destination?.paneId
                ?? (panes.some((pane) => pane.paneId === layout.activePaneId)
                    ? layout.activePaneId
                    : null)
            return {
                sessions,
                layouts: {
                    ...s.layouts,
                    [workspace]: {
                        panes,
                        activePaneId,
                        splitDirection: panes.length > 1 ? layout.splitDirection : null
                    }
                }
            }
        }),

    setActivePane: (workspace, paneId) =>
        set((s) => {
            const layout = s.layouts[workspace]
            if (!layout?.panes.some((pane) => pane.paneId === paneId)) return s
            return {
                layouts: {
                    ...s.layouts,
                    [workspace]: { ...layout, activePaneId: paneId }
                }
            }
        }),

    splitFrom: (workspace, paneId, meta, direction = "right") =>
        set((s) => {
            const layout = s.layouts[workspace]
            if (!layout || layout.panes.length >= MAX_PANES) return s
            if (!layout.panes.some((pane) => pane.paneId === paneId)) return s
            const nextPaneId = meta.sessionId
            return {
                sessions: { ...s.sessions, [meta.sessionId]: meta },
                layouts: {
                    ...s.layouts,
                    [workspace]: {
                        panes: [...layout.panes, { paneId: nextPaneId, sessionId: meta.sessionId }],
                        activePaneId: nextPaneId,
                        splitDirection: direction
                    }
                }
            }
        }),

    sessionsForWorkspace: (workspace) =>
        Object.values(get().sessions).filter((session) => session.workspace === workspace),

    reset: () => set(terminalInitialState)
}))
