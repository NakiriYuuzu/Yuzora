import { create } from "zustand"
import type { TerminalCwdStrategy } from "@/lib/types"
import type { TerminalImeAnchorMode } from "@/terminal/terminalImePositioning"

type TerminalLaunchStatus = "opening" | "running" | "failed"

export interface TerminalSessionMeta {
    sessionId: string
    title: string
    manualTitle?: string
    shellTitle?: string
    launchStatus: TerminalLaunchStatus
    workspace: string
    shell: string
    shellArgs?: string[]
    profileName?: string
    cwdStrategy?: TerminalCwdStrategy
    imeAnchorMode?: TerminalImeAnchorMode
    cols: number
    rows: number
}

interface TerminalPane {
    paneId: string
    sessionId: string
}

export interface TerminalWorkspaceLayout {
    tabIds: string[]
    panes: TerminalPane[]
    activePaneId: string | null
    splitRatio: number
    nextTerminalNumber: number
    renamingSessionId: string | null
}

interface TerminalState {
    sessions: Record<string, TerminalSessionMeta>
    layouts: Record<string, TerminalWorkspaceLayout>
    addSession: (workspace: string, meta: TerminalSessionMeta, paneId?: string) => void
    removeSession: (workspace: string, sessionId: string) => void
    selectTab: (workspace: string, sessionId: string) => void
    reorderTab: (workspace: string, sessionId: string, destinationIndex: number) => void
    setActivePane: (workspace: string, paneId: string) => void
    setSplitRatio: (workspace: string, ratio: number) => void
    splitFrom: (workspace: string, paneId: string, meta: TerminalSessionMeta) => void
    allocateTerminalNumber: (workspace: string) => number
    beginRename: (workspace: string, sessionId: string) => void
    finishRename: (workspace: string, sessionId: string) => void
    setManualTitle: (sessionId: string, title: string) => void
    setShellTitle: (sessionId: string, title: string) => void
    setLaunchStatus: (sessionId: string, status: TerminalLaunchStatus) => void
    sessionsForWorkspace: (workspace: string) => TerminalSessionMeta[]
    reset: () => void
}

export const MAX_VISIBLE_TERMINAL_PANES = 2
const DEFAULT_TERMINAL_PANE_SPLIT_RATIO = 0.5
export const MIN_TERMINAL_PANE_SPLIT_RATIO = 0.2
export const MAX_TERMINAL_PANE_SPLIT_RATIO = 0.8
export const MAX_TERMINAL_TITLE_LENGTH = 128

export const terminalInitialState = {
    sessions: {} as Record<string, TerminalSessionMeta>,
    layouts: {} as Record<string, TerminalWorkspaceLayout>
}

function emptyLayout(): TerminalWorkspaceLayout {
    return {
        tabIds: [],
        panes: [],
        activePaneId: null,
        splitRatio: DEFAULT_TERMINAL_PANE_SPLIT_RATIO,
        nextTerminalNumber: 1,
        renamingSessionId: null
    }
}

function clampSplitRatio(ratio: number): number {
    return Math.min(
        MAX_TERMINAL_PANE_SPLIT_RATIO,
        Math.max(MIN_TERMINAL_PANE_SPLIT_RATIO, ratio)
    )
}

function normalizeTerminalTitle(value: string): string {
    const singleLine = Array.from(value.replace(/[\r\n\u2028\u2029]+/g, " "))
        .filter((character) => {
            const codePoint = character.codePointAt(0) ?? 0
            return !(
                (codePoint >= 0 && codePoint <= 0x1f)
                || (codePoint >= 0x7f && codePoint <= 0x9f)
            )
        })
        .join("")
        .trim()
    return Array.from(singleLine).slice(0, MAX_TERMINAL_TITLE_LENGTH).join("")
}

export function terminalDisplayTitle(session: TerminalSessionMeta): string {
    return session.manualTitle || session.shellTitle || session.title
}

export const useTerminalStore = create<TerminalState>()((set, get) => ({
    ...terminalInitialState,

    addSession: (workspace, meta, paneId) =>
        set((state) => {
            if (meta.workspace !== workspace || state.sessions[meta.sessionId]) return state
            const layout = state.layouts[workspace] ?? emptyLayout()
            const nextPaneId = paneId ?? meta.sessionId
            let panes: TerminalPane[]
            let activePaneId: string

            if (layout.panes.length === 0) {
                panes = [{ paneId: nextPaneId, sessionId: meta.sessionId }]
                activePaneId = nextPaneId
            } else {
                const activeIndex = Math.max(
                    0,
                    layout.panes.findIndex((pane) => pane.paneId === layout.activePaneId)
                )
                const activePane = layout.panes[activeIndex]
                panes = layout.panes.map((pane, index) =>
                    index === activeIndex ? { ...pane, sessionId: meta.sessionId } : pane
                )
                activePaneId = activePane.paneId
            }

            return {
                sessions: { ...state.sessions, [meta.sessionId]: meta },
                layouts: {
                    ...state.layouts,
                    [workspace]: {
                        ...layout,
                        tabIds: [...layout.tabIds, meta.sessionId],
                        panes,
                        activePaneId
                    }
                }
            }
        }),

    removeSession: (workspace, sessionId) =>
        set((state) => {
            const session = state.sessions[sessionId]
            if (!session || session.workspace !== workspace) return state
            const layout = state.layouts[workspace]
            if (!layout?.tabIds.includes(sessionId)) return state

            const sessions = { ...state.sessions }
            delete sessions[sessionId]

            const tabIndex = layout.tabIds.indexOf(sessionId)
            const tabIds = layout.tabIds.filter((tabId) => tabId !== sessionId)
            const visibleIndex = layout.panes.findIndex((pane) => pane.sessionId === sessionId)
            let panes = layout.panes
            let activePaneId = layout.activePaneId

            if (visibleIndex >= 0 && layout.panes.length > 1) {
                panes = layout.panes.filter((pane) => pane.sessionId !== sessionId)
                activePaneId = panes[0]?.paneId ?? null
            } else if (visibleIndex >= 0) {
                const destination = layout.tabIds[tabIndex + 1] ?? layout.tabIds[tabIndex - 1]
                const pane = layout.panes[visibleIndex]
                panes = destination ? [{ ...pane, sessionId: destination }] : []
                activePaneId = panes[0]?.paneId ?? null
            }

            return {
                sessions,
                layouts: {
                    ...state.layouts,
                    [workspace]: {
                        ...layout,
                        tabIds,
                        panes,
                        activePaneId,
                        renamingSessionId: layout.renamingSessionId === sessionId
                            ? null
                            : layout.renamingSessionId
                    }
                }
            }
        }),

    selectTab: (workspace, sessionId) =>
        set((state) => {
            const layout = state.layouts[workspace]
            if (!layout?.tabIds.includes(sessionId)) return state
            const visiblePane = layout.panes.find((pane) => pane.sessionId === sessionId)
            if (visiblePane) {
                if (layout.activePaneId === visiblePane.paneId) return state
                return {
                    layouts: {
                        ...state.layouts,
                        [workspace]: { ...layout, activePaneId: visiblePane.paneId }
                    }
                }
            }

            if (layout.panes.length === 0) {
                const paneId = sessionId
                return {
                    layouts: {
                        ...state.layouts,
                        [workspace]: {
                            ...layout,
                            panes: [{ paneId, sessionId }],
                            activePaneId: paneId
                        }
                    }
                }
            }

            const activeIndex = Math.max(
                0,
                layout.panes.findIndex((pane) => pane.paneId === layout.activePaneId)
            )
            return {
                layouts: {
                    ...state.layouts,
                    [workspace]: {
                        ...layout,
                        panes: layout.panes.map((pane, index) =>
                            index === activeIndex ? { ...pane, sessionId } : pane
                        ),
                        activePaneId: layout.panes[activeIndex].paneId
                    }
                }
            }
        }),

    reorderTab: (workspace, sessionId, destinationIndex) =>
        set((state) => {
            const layout = state.layouts[workspace]
            const sourceIndex = layout?.tabIds.indexOf(sessionId) ?? -1
            if (!layout || sourceIndex < 0) return state
            const clampedIndex = Math.max(0, Math.min(layout.tabIds.length - 1, destinationIndex))
            if (sourceIndex === clampedIndex) return state
            const tabIds = [...layout.tabIds]
            tabIds.splice(sourceIndex, 1)
            tabIds.splice(clampedIndex, 0, sessionId)
            return {
                layouts: {
                    ...state.layouts,
                    [workspace]: { ...layout, tabIds }
                }
            }
        }),

    setActivePane: (workspace, paneId) =>
        set((state) => {
            const layout = state.layouts[workspace]
            if (!layout?.panes.some((pane) => pane.paneId === paneId)) return state
            if (layout.activePaneId === paneId) return state
            return {
                layouts: {
                    ...state.layouts,
                    [workspace]: { ...layout, activePaneId: paneId }
                }
            }
        }),

    setSplitRatio: (workspace, ratio) =>
        set((state) => {
            const layout = state.layouts[workspace]
            if (!layout) return state
            const splitRatio = clampSplitRatio(ratio)
            if (layout.splitRatio === splitRatio) return state
            return {
                layouts: {
                    ...state.layouts,
                    [workspace]: { ...layout, splitRatio }
                }
            }
        }),

    splitFrom: (workspace, paneId, meta) =>
        set((state) => {
            const layout = state.layouts[workspace]
            if (
                !layout
                || layout.panes.length !== 1
                || !layout.panes.some((pane) => pane.paneId === paneId)
                || meta.workspace !== workspace
                || state.sessions[meta.sessionId]
            ) return state
            const nextPaneId = meta.sessionId
            return {
                sessions: { ...state.sessions, [meta.sessionId]: meta },
                layouts: {
                    ...state.layouts,
                    [workspace]: {
                        ...layout,
                        tabIds: [...layout.tabIds, meta.sessionId],
                        panes: [
                            layout.panes[0],
                            { paneId: nextPaneId, sessionId: meta.sessionId }
                        ],
                        activePaneId: nextPaneId
                    }
                }
            }
        }),

    allocateTerminalNumber: (workspace) => {
        let allocated = 1
        set((state) => {
            const layout = state.layouts[workspace] ?? emptyLayout()
            allocated = layout.nextTerminalNumber
            return {
                layouts: {
                    ...state.layouts,
                    [workspace]: {
                        ...layout,
                        nextTerminalNumber: allocated + 1
                    }
                }
            }
        })
        return allocated
    },

    beginRename: (workspace, sessionId) =>
        set((state) => {
            const layout = state.layouts[workspace]
            if (!layout?.tabIds.includes(sessionId)) return state
            return {
                layouts: {
                    ...state.layouts,
                    [workspace]: { ...layout, renamingSessionId: sessionId }
                }
            }
        }),

    finishRename: (workspace, sessionId) =>
        set((state) => {
            const layout = state.layouts[workspace]
            if (!layout || layout.renamingSessionId !== sessionId) return state
            return {
                layouts: {
                    ...state.layouts,
                    [workspace]: { ...layout, renamingSessionId: null }
                }
            }
        }),

    setManualTitle: (sessionId, title) =>
        set((state) => {
            const session = state.sessions[sessionId]
            if (!session) return state
            const manualTitle = normalizeTerminalTitle(title) || undefined
            if (session.manualTitle === manualTitle) return state
            return {
                sessions: {
                    ...state.sessions,
                    [sessionId]: { ...session, manualTitle }
                }
            }
        }),

    setShellTitle: (sessionId, title) =>
        set((state) => {
            const session = state.sessions[sessionId]
            if (!session) return state
            const shellTitle = normalizeTerminalTitle(title) || undefined
            if (session.shellTitle === shellTitle) return state
            return {
                sessions: {
                    ...state.sessions,
                    [sessionId]: { ...session, shellTitle }
                }
            }
        }),

    setLaunchStatus: (sessionId, launchStatus) =>
        set((state) => {
            const session = state.sessions[sessionId]
            if (!session || session.launchStatus === launchStatus) return state
            return {
                sessions: {
                    ...state.sessions,
                    [sessionId]: { ...session, launchStatus }
                }
            }
        }),

    sessionsForWorkspace: (workspace) => {
        const state = get()
        return (state.layouts[workspace]?.tabIds ?? [])
            .map((sessionId) => state.sessions[sessionId])
            .filter((session): session is TerminalSessionMeta => Boolean(session))
    },

    reset: () => set({ sessions: {}, layouts: {} })
}))
