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
    /** `WKWebView.isLoading` / webkit `is_loading`; absent where the platform cannot tell (Windows). */
    loading?: boolean | null
    /** Length of the native back list; a committed navigation adds one entry. */
    backCount?: number | null
}

export interface PreviewNativeSession {
    sessionId?: string
    backCount?: number
    canGoBack?: boolean
    canGoForward?: boolean
    outerBackStack?: string[]
    outerForwardStack?: string[]
    workspacePath: string
    currentUrl: string
    backStack: string[]
    forwardStack: string[]
}

/**
 * A user-typed navigation the native webview has not resolved yet. The native
 * layer exposes no load-failure event, so it is inferred from an idle snapshot
 * (`loading === false`): a failed load commits no history entry, leaving the
 * back-list length at `backCount` and the URL at `previous`. Any other idle
 * snapshot (longer back list, or a different URL) resolves it as a success.
 */
export interface PreviewPendingNavigation {
    requested: string
    previous: string
    /** Native back-list length when the navigation was requested. */
    backCount: number
    at: number
}

export interface PreviewNavigationFailure {
    url: string
    /** Normalized native URL and back-list length at failure time; a change means the failure is stale. */
    nativeUrl: string
    backCount: number
}

function normalizeUrl(rawUrl: string): string {
    try {
        return new URL(rawUrl).href
    } catch {
        return rawUrl
    }
}

// Safety net: give up tracking a navigation that never resolves after this.
const NAVIGATION_PENDING_EXPIRY_MS = 30_000

export type PreviewNativeRequest =
    | { token: number; kind: "open"; workspacePath: string; url: string }
    | { token: number; kind: "close"; workspacePath: string | null }

interface PreviewState {
    nav: Record<string, PreviewNavState>
    nativeNavigationSyncs: Record<string, PreviewNativeNavigationSync>
    nativeNavigationSyncToken: number
    pendingNavigations: Record<string, PreviewPendingNavigation>
    navigationFailures: Record<string, PreviewNavigationFailure>
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
    clearNavigationFailure: (workspace: string) => void
    reload: (workspace: string) => void
    setFrame: (workspace: string, frame: ResponsiveFrame) => void
    reset: () => void
}

export const previewInitialState = {
    nav: {} as Record<string, PreviewNavState>,
    nativeNavigationSyncs: {} as Record<string, PreviewNativeNavigationSync>,
    nativeNavigationSyncToken: 0,
    pendingNavigations: {} as Record<string, PreviewPendingNavigation>,
    navigationFailures: {} as Record<string, PreviewNavigationFailure>,
    nativeSession: null as PreviewNativeSession | null,
    nativeRequestToken: 0,
    nativeRequest: null as PreviewNativeRequest | null,
}

// The navigation boundary admits only web URLs; renderer selection remains in
// PreviewPanel so remote frames keep their tunnel boundary.
function isAllowedPreviewUrl(rawUrl: string): boolean {
    try {
        const url = new URL(rawUrl)
        return url.protocol === "http:" || url.protocol === "https:" || url.protocol === "yuzora-preview:"
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

function clearedNavigationTracking(s: PreviewState, workspace: string) {
    const pendingNavigations = { ...s.pendingNavigations }
    const navigationFailures = { ...s.navigationFailures }
    delete pendingNavigations[workspace]
    delete navigationFailures[workspace]
    return { pendingNavigations, navigationFailures }
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
            const backCount = s.nativeSession?.workspacePath === workspace ? s.nativeSession.backCount : undefined
            const nativeNavigationSyncs = { ...s.nativeNavigationSyncs }
            delete nativeNavigationSyncs[workspace]
            const tracking = clearedNavigationTracking(s, workspace)
            if (nav.url && typeof backCount === "number" && normalizeUrl(url) !== normalizeUrl(nav.url)) {
                tracking.pendingNavigations[workspace] = { requested: url, previous: nav.url, backCount, at: Date.now() }
            }
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
                nativeNavigationSyncs,
                ...tracking
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
                nativeNavigationSyncs,
                ...clearedNavigationTracking(s, workspace)
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
                nativeNavigationSyncs,
                ...clearedNavigationTracking(s, workspace)
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
                ...clearedNavigationTracking(s, workspace),
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
                ...clearedNavigationTracking(s, workspace),
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

    receiveNativeNavigation: ({ sessionId, url, canGoBack, canGoForward, loading, backCount: rawBackCount }) => {
        if (!isAllowedPreviewUrl(url)) return
        set((s) => {
            const session = s.nativeSession
            if (!session || session.sessionId !== sessionId || s.nativeRequest !== null
                || useWorkspaceStore.getState().workspacePath !== session.workspacePath) return s
            const workspace = session.workspacePath
            const nav = s.nav[workspace]
            if (!nav || nav.url !== session.currentUrl) return s
            const backCount = typeof rawBackCount === "number" ? rawBackCount : undefined
            const nativeUrl = normalizeUrl(url)
            const pending = s.pendingNavigations[workspace]?.requested === nav.url
                ? s.pendingNavigations[workspace] : undefined
            const expired = !!pending && Date.now() - pending.at > NAVIGATION_PENDING_EXPIRY_MS
            // A longer back list means the requested page already committed, even if it is
            // still loading subresources: the navigation succeeded.
            const committed = !!pending && backCount !== undefined && backCount > pending.backCount
            // In flight: the view may still report `previous` or the requested URL, so the
            // optimistic nav stays put until it goes idle. History capabilities stay live
            // so Back/Forward and the next navigate() act on the real native history.
            if (pending && !expired && loading === true && !committed) {
                if (session.canGoBack === canGoBack && session.canGoForward === canGoForward
                    && session.backCount === backCount) return s
                return { nativeSession: { ...session, canGoBack, canGoForward, backCount,
                    outerForwardStack: canGoBack ? [] : session.outerForwardStack } }
            }
            const idle = loading === false && backCount !== undefined
            const previousUrl = pending ? normalizeUrl(pending.previous) : null
            // A failed provisional load never creates a history entry, so an idle view
            // that is back on `previous` with an unchanged back list means it failed.
            // A redirect or in-page link back to `previous` still grows the back list.
            if (pending && !expired && idle && nativeUrl === previousUrl && backCount === pending.backCount) {
                const token = s.nativeNavigationSyncToken + 1
                const tracking = clearedNavigationTracking(s, workspace)
                tracking.navigationFailures[workspace] = { url: pending.requested, nativeUrl, backCount }
                // navigate()/recordNativeOpen() pushed `previous` as history that never happened.
                return {
                    nav: { ...s.nav, [workspace]: { ...nav, url,
                        backStack: nav.backStack.at(-1) === url ? nav.backStack.slice(0, -1) : nav.backStack } },
                    nativeSession: { ...session, currentUrl: url, canGoBack, canGoForward, backCount,
                        backStack: session.backStack.at(-1) === url ? session.backStack.slice(0, -1) : session.backStack },
                    nativeNavigationSyncs: { ...s.nativeNavigationSyncs, [workspace]: { url, token } },
                    nativeNavigationSyncToken: token,
                    ...tracking
                }
            }
            const failure = s.navigationFailures[workspace]
            // Native link clicks and toolbar history/reload never touch the store, so the
            // alert is retired by the view moving on: loading again, another page, or history.
            const failureStale = !!failure && (loading === true || nativeUrl !== failure.nativeUrl
                || (backCount !== undefined && backCount !== failure.backCount))
            // Loading keeps the navigation in flight unless it already committed; anything
            // else that shows the view moved on (or cannot be judged, e.g. Windows) retires
            // it without a failure.
            const settled = !!pending && (expired || committed || (loading !== true
                && (!idle || backCount !== pending.backCount || nativeUrl !== previousUrl)))
            const changedUrl = nav.url !== url
            if (!changedUrl && !settled && !failureStale && session.canGoBack === canGoBack
                && session.canGoForward === canGoForward && session.backCount === backCount) return s
            const token = s.nativeNavigationSyncToken + 1
            const tracking = clearedNavigationTracking(s, workspace)
            return {
                // The native browser owns its actual history, including redirects,
                // replaceState and traversal. A snapshot never invents entries.
                nav: { ...s.nav, [workspace]: { ...nav, url } },
                nativeSession: { ...session, currentUrl: url, canGoBack, canGoForward, backCount,
                    outerForwardStack: canGoBack ? [] : session.outerForwardStack },
                ...(settled ? { pendingNavigations: tracking.pendingNavigations } : {}),
                ...(failureStale ? { navigationFailures: tracking.navigationFailures } : {}),
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
            // Revoke continuity before queued opens can supersede the close.
            // The physical close still follows the native queue/request token.
            nativeSession: null,
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

    clearNavigationFailure: (workspace) =>
        set((s) => {
            if (!s.navigationFailures[workspace]) return s
            const navigationFailures = { ...s.navigationFailures }
            delete navigationFailures[workspace]
            return { navigationFailures }
        }),

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
