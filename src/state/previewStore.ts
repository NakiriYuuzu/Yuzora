import { create } from "zustand"

import { previewClose } from "../lib/ipc"
import { enqueueNativePreviewOperation } from "../preview/nativePreviewQueue"
import { PREVIEW_TAB_PATH, useWorkspaceStore } from "./workspaceStore"

type ResponsiveFrame = "full" | "mobile"

export interface PreviewNavState {
    url: string | null
    backStack: string[]
    forwardStack: string[]
    reloadNonce: number
    frame: ResponsiveFrame
}

export interface PreviewNativeNavigationSync {
    url: string
    token: number
}

export interface PreviewNativeNavigationSnapshot {
    sessionId: string
    url: string
    canGoBack: boolean
    canGoForward: boolean
}

export interface PreviewNativeSession {
    sessionId?: string
    canGoBack?: boolean
    canGoForward?: boolean
    outerBackStack?: string[]
    outerForwardStack?: string[]
    workspacePath: string
    currentUrl: string
    backStack: string[]
    forwardStack: string[]
}

export type PreviewNativeRequest =
    | { token: number; kind: "open"; workspacePath: string; url: string }
    | { token: number; kind: "close"; workspacePath: string | null }

interface PreviewState {
    nav: Record<string, PreviewNavState>
    nativeNavigationSyncs: Record<string, PreviewNativeNavigationSync>
    nativeNavigationSyncToken: number
    nativeSession: PreviewNativeSession | null
    nativeRequestToken: number
    nativeRequest: PreviewNativeRequest | null
    navForWorkspace: (workspace: string) => PreviewNavState
    navigate: (workspace: string, url: string) => boolean
    goBack: (workspace: string) => void
    goForward: (workspace: string) => void
    syncNativeBack: (workspace: string) => boolean
    syncNativeForward: (workspace: string) => boolean
    consumeNativeNavigationSync: (workspace: string, token: number) => void
    recordNativeOpen: (workspace: string, url: string, sessionId?: string) => void
    receiveNativeNavigation: (event: PreviewNativeNavigationSnapshot) => void
    closeNativeSession: (workspace?: string) => void
    beginNativeOpenRequest: (workspace: string, url: string) => number
    beginNativeCloseRequest: (workspace: string | null) => number
    nativeRequestIsCurrent: (token: number) => boolean
    settleNativeRequest: (token: number) => boolean
    reload: (workspace: string) => void
    setFrame: (workspace: string, frame: ResponsiveFrame) => void
    reset: () => void
}

export const previewInitialState = {
    nav: {} as Record<string, PreviewNavState>,
    nativeNavigationSyncs: {} as Record<string, PreviewNativeNavigationSync>,
    nativeNavigationSyncToken: 0,
    nativeSession: null as PreviewNativeSession | null,
    nativeRequestToken: 0,
    nativeRequest: null as PreviewNativeRequest | null,
}

// The navigation boundary admits only web URLs; renderer selection remains in
// PreviewPanel so remote frames keep their tunnel boundary.
function isAllowedPreviewUrl(rawUrl: string): boolean {
    try {
        const url = new URL(rawUrl)
        return url.protocol === "http:" || url.protocol === "https:"
    } catch {
        return false
    }
}

export function isLocalPreviewUrl(rawUrl: string | null): boolean {
    if (!rawUrl) return false
    try {
        const url = new URL(rawUrl)
        return url.protocol === "http:" && (url.hostname === "localhost" || url.hostname === "127.0.0.1")
    } catch {
        return false
    }
}

function defaultNav(): PreviewNavState {
    return {
        url: null,
        backStack: [],
        forwardStack: [],
        reloadNonce: 0,
        frame: "full"
    }
}

export const usePreviewStore = create<PreviewState>()((set, get) => ({
    ...previewInitialState,

    navForWorkspace: (workspace) => get().nav[workspace] ?? defaultNav(),

    navigate: (workspace, url) => {
        if (!isAllowedPreviewUrl(url)) return false
        set((s) => {
            const nav = s.nav[workspace] ?? defaultNav()
            if (nav.url === url) return s
            const nativeNavigationSyncs = { ...s.nativeNavigationSyncs }
            delete nativeNavigationSyncs[workspace]
            return {
                nav: {
                    ...s.nav,
                    [workspace]: {
                        ...nav,
                        url,
                        backStack: nav.url ? [...nav.backStack, nav.url] : nav.backStack,
                        forwardStack: []
                    }
                },
                nativeNavigationSyncs
            }
        })
        return true
    },

    goBack: (workspace) =>
        set((s) => {
            const nav = s.nav[workspace] ?? defaultNav()
            if (!nav.url || nav.backStack.length === 0) return s
            const url = nav.backStack[nav.backStack.length - 1]
            const nativeNavigationSyncs = { ...s.nativeNavigationSyncs }
            delete nativeNavigationSyncs[workspace]
            return {
                nav: {
                    ...s.nav,
                    [workspace]: {
                        ...nav,
                        url,
                        backStack: nav.backStack.slice(0, -1),
                        forwardStack: [nav.url, ...nav.forwardStack]
                    }
                },
                nativeNavigationSyncs
            }
        }),

    goForward: (workspace) =>
        set((s) => {
            const nav = s.nav[workspace] ?? defaultNav()
            if (nav.forwardStack.length === 0) return s
            const [url, ...forwardStack] = nav.forwardStack
            const nativeNavigationSyncs = { ...s.nativeNavigationSyncs }
            delete nativeNavigationSyncs[workspace]
            return {
                nav: {
                    ...s.nav,
                    [workspace]: {
                        ...nav,
                        url,
                        backStack: nav.url ? [...nav.backStack, nav.url] : nav.backStack,
                        forwardStack
                    }
                },
                nativeNavigationSyncs
            }
        }),

    syncNativeBack: (workspace) => {
        let moved = false
        set((s) => {
            const nav = s.nav[workspace] ?? defaultNav()
            if (!nav.url || nav.backStack.length === 0) return s
            const url = nav.backStack[nav.backStack.length - 1]
            const nativeSession = s.nativeSession
            if (
                nativeSession?.workspacePath !== workspace
                || nativeSession.currentUrl !== nav.url
                || nativeSession.backStack.at(-1) !== url
            ) return s
            const token = s.nativeNavigationSyncToken + 1
            moved = true
            return {
                nav: {
                    ...s.nav,
                    [workspace]: {
                        ...nav,
                        url,
                        backStack: nav.backStack.slice(0, -1),
                        forwardStack: [nav.url, ...nav.forwardStack]
                    }
                },
                nativeNavigationSyncs: {
                    ...s.nativeNavigationSyncs,
                    [workspace]: { url, token }
                },
                nativeNavigationSyncToken: token,
                nativeSession: {
                    ...nativeSession,
                    currentUrl: url,
                    backStack: nativeSession.backStack.slice(0, -1),
                    forwardStack: [nativeSession.currentUrl, ...nativeSession.forwardStack]
                }
            }
        })
        return moved
    },

    syncNativeForward: (workspace) => {
        let moved = false
        set((s) => {
            const nav = s.nav[workspace] ?? defaultNav()
            if (nav.forwardStack.length === 0) return s
            const [url, ...forwardStack] = nav.forwardStack
            const nativeSession = s.nativeSession
            if (
                nativeSession?.workspacePath !== workspace
                || nativeSession.currentUrl !== nav.url
                || nativeSession.forwardStack[0] !== url
            ) return s
            const token = s.nativeNavigationSyncToken + 1
            moved = true
            return {
                nav: {
                    ...s.nav,
                    [workspace]: {
                        ...nav,
                        url,
                        backStack: nav.url ? [...nav.backStack, nav.url] : nav.backStack,
                        forwardStack
                    }
                },
                nativeNavigationSyncs: {
                    ...s.nativeNavigationSyncs,
                    [workspace]: { url, token }
                },
                nativeNavigationSyncToken: token,
                nativeSession: {
                    ...nativeSession,
                    currentUrl: url,
                    backStack: [...nativeSession.backStack, nativeSession.currentUrl],
                    forwardStack: nativeSession.forwardStack.slice(1)
                }
            }
        })
        return moved
    },

    consumeNativeNavigationSync: (workspace, token) =>
        set((s) => {
            if (s.nativeNavigationSyncs[workspace]?.token !== token) return s
            const nativeNavigationSyncs = { ...s.nativeNavigationSyncs }
            delete nativeNavigationSyncs[workspace]
            return { nativeNavigationSyncs }
        }),

    recordNativeOpen: (workspace, url, sessionId) =>
        set((s) => {
            const current = s.nativeSession
            if (current?.workspacePath !== workspace || current.sessionId !== sessionId) {
                return {
                    nativeSession: {
                        workspacePath: workspace,
                        sessionId,
                        ...(sessionId ? { canGoBack: false, canGoForward: false,
                            outerBackStack: [...(s.nav[workspace]?.backStack ?? [])],
                            outerForwardStack: [...(s.nav[workspace]?.forwardStack ?? [])] } : {}),
                        currentUrl: url,
                        backStack: [],
                        forwardStack: []
                    }
                }
            }
            if (current.currentUrl === url) return s
            return {
                nativeSession: {
                    ...current,
                    currentUrl: url,
                    outerForwardStack: [],
                    backStack: [...current.backStack, current.currentUrl],
                    forwardStack: []
                }
            }
        }),

    receiveNativeNavigation: ({ sessionId, url, canGoBack, canGoForward }) => {
        if (!isAllowedPreviewUrl(url)) return
        set((s) => {
            const session = s.nativeSession
            if (!session || session.sessionId !== sessionId || s.nativeRequest !== null
                || useWorkspaceStore.getState().workspacePath !== session.workspacePath) return s
            const workspace = session.workspacePath
            const nav = s.nav[workspace]
            if (!nav || nav.url !== session.currentUrl) return s
            const changedUrl = nav.url !== url
            if (!changedUrl && session.canGoBack === canGoBack && session.canGoForward === canGoForward) return s
            const token = s.nativeNavigationSyncToken + 1
            return {
                // The native browser owns its actual history, including redirects,
                // replaceState and traversal. A snapshot never invents entries.
                nav: { ...s.nav, [workspace]: { ...nav, url } },
                nativeSession: { ...session, currentUrl: url, canGoBack, canGoForward,
                    outerForwardStack: canGoBack ? [] : session.outerForwardStack },
                ...(changedUrl ? {
                    nativeNavigationSyncs: { ...s.nativeNavigationSyncs, [workspace]: { url, token } },
                    nativeNavigationSyncToken: token
                } : {})
            }
        })
    },

    closeNativeSession: (workspace) =>
        set((s) => {
            if (workspace && s.nativeSession?.workspacePath !== workspace) return s
            return { nativeSession: null }
        }),

    beginNativeOpenRequest: (workspace, url) => {
        const token = get().nativeRequestToken + 1
        set({
            nativeRequestToken: token,
            nativeRequest: { token, kind: "open", workspacePath: workspace, url }
        })
        return token
    },

    beginNativeCloseRequest: (workspace) => {
        const token = get().nativeRequestToken + 1
        set({
            nativeRequestToken: token,
            nativeRequest: { token, kind: "close", workspacePath: workspace }
        })
        return token
    },

    nativeRequestIsCurrent: (token) => get().nativeRequest?.token === token,

    settleNativeRequest: (token) => {
        if (get().nativeRequest?.token !== token) return false
        set({ nativeRequest: null })
        return true
    },

    reload: (workspace) =>
        set((s) => {
            const nav = s.nav[workspace] ?? defaultNav()
            return {
                nav: {
                    ...s.nav,
                    [workspace]: { ...nav, reloadNonce: nav.reloadNonce + 1 }
                }
            }
        }),

    setFrame: (workspace, frame) =>
        set((s) => {
            const nav = s.nav[workspace] ?? defaultNav()
            return {
                nav: {
                    ...s.nav,
                    [workspace]: { ...nav, frame }
                }
            }
        }),

    reset: () => set(previewInitialState)
}))

function previewTabIsOpen(groups: { tabs: { path: string; kind?: string }[] }[]): boolean {
    return groups.some((group) =>
        group.tabs.some((tab) => tab.kind === "preview" || tab.path === PREVIEW_TAB_PATH)
    )
}

function closeNativePreviewForOwner(workspacePath: string | null): void {
    const state = usePreviewStore.getState()
    const owner = state.nativeSession?.workspacePath
        ?? (state.nativeRequest?.kind === "open" ? state.nativeRequest.workspacePath : null)
    if (!owner || (workspacePath !== null && owner !== workspacePath)) return

    const token = state.beginNativeCloseRequest(workspacePath)
    void enqueueNativePreviewOperation(async () => {
        try {
            await previewClose()
        } finally {
            const latest = usePreviewStore.getState()
            if (latest.nativeRequestIsCurrent(token)) {
                latest.closeNativeSession(owner)
                latest.settleNativeRequest(token)
            }
        }
    }).catch(() => undefined)
}

useWorkspaceStore.subscribe((state, previous) => {
    if (state.workspacePath !== previous.workspacePath) {
        closeNativePreviewForOwner(previous.workspacePath)
        return
    }
    if (previewTabIsOpen(previous.groups) && !previewTabIsOpen(state.groups)) {
        closeNativePreviewForOwner(state.workspacePath)
    }
})
