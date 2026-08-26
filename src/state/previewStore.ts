import { create } from "zustand"

import { previewClose, previewRevoke } from "../lib/ipc"
import type { DevServerInfo } from "../lib/types"
import { enqueueNativePreviewOperation } from "../preview/nativePreviewQueue"
import { PREVIEW_TAB_PATH, useWorkspaceStore } from "./workspaceStore"

type ResponsiveFrame = "full" | "mobile"

export type PreviewFrameMode = "static" | "dev-server"

export interface StaticPreviewSession {
    workspace: string
    token: string
    url: string
}

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

export interface PreviewNativeSession {
    workspacePath: string
    currentUrl: string
    backStack: string[]
    forwardStack: string[]
}

export type PreviewNativeRequest =
    | { token: number; kind: "open"; workspacePath: string; url: string }
    | { token: number; kind: "close"; workspacePath: string | null }

interface PreviewState {
    devServer: DevServerInfo | null
    devServers: Record<string, DevServerInfo>
    attempts: Record<string, number>
    nav: Record<string, PreviewNavState>
    nativeNavigationSyncs: Record<string, PreviewNativeNavigationSync>
    nativeNavigationSyncToken: number
    nativeSession: PreviewNativeSession | null
    nativeRequestToken: number
    nativeRequest: PreviewNativeRequest | null
    setDevServer: (info: DevServerInfo | null) => void
    devServerForWorkspace: (workspace: string) => DevServerInfo | null
    beginAttempt: (workspace: string) => number
    attemptForWorkspace: (workspace: string) => number
    restoreAttempt: (workspace: string, expected: number, previous: number) => boolean
    navForWorkspace: (workspace: string) => PreviewNavState
    navigate: (workspace: string, url: string) => boolean
    goBack: (workspace: string) => void
    goForward: (workspace: string) => void
    syncNativeBack: (workspace: string) => boolean
    syncNativeForward: (workspace: string) => boolean
    consumeNativeNavigationSync: (workspace: string, token: number) => void
    recordNativeOpen: (workspace: string, url: string) => void
    closeNativeSession: (workspace?: string) => void
    beginNativeOpenRequest: (workspace: string, url: string) => number
    beginNativeCloseRequest: (workspace: string | null) => number
    nativeRequestIsCurrent: (token: number) => boolean
    settleNativeRequest: (token: number) => boolean
    reload: (workspace: string) => void
    setFrame: (workspace: string, frame: ResponsiveFrame) => void
    staticPreview: StaticPreviewSession | null
    openStaticPreview: (workspace: string, session: { token: string; url: string }) => boolean
    revokeStaticPreview: () => void
    reset: () => void
}

export const previewInitialState = {
    devServer: null as DevServerInfo | null,
    devServers: {} as Record<string, DevServerInfo>,
    attempts: {} as Record<string, number>,
    nav: {} as Record<string, PreviewNavState>,
    nativeNavigationSyncs: {} as Record<string, PreviewNativeNavigationSync>,
    nativeNavigationSyncToken: 0,
    nativeSession: null as PreviewNativeSession | null,
    nativeRequestToken: 0,
    nativeRequest: null as PreviewNativeRequest | null,
    staticPreview: null as StaticPreviewSession | null
}

// P3: the navigate choke point now admits any http/https URL — external https is
// rendered in a child webview (an <iframe> can't host it), local dev servers keep
// the iframe path. Non-web schemes (file:, javascript:, …) are still rejected.
function isAllowedPreviewUrl(rawUrl: string): boolean {
    try {
        const url = new URL(rawUrl)
        return url.protocol === "http:" || url.protocol === "https:"
    } catch {
        return false
    }
}

// A local dev-server / static-server URL renders in the sandboxed <iframe>;
// everything else (external https) goes to the child webview. 127.0.0.1 is what
// the P3 static file server binds, so right-clicked HTML previews stay on the
// iframe path too.
export function previewFrameModeFor(
    url: string | null,
    session: StaticPreviewSession | null
): PreviewFrameMode {
    if (!url || !session) return "dev-server"
    try {
        const target = new URL(url)
        const sessionUrl = new URL(session.url)
        if (target.origin !== sessionUrl.origin) return "dev-server"
        const prefix = `/${session.token}/`
        return target.pathname === `/${session.token}` || target.pathname.startsWith(prefix)
            ? "static"
            : "dev-server"
    } catch {
        return "dev-server"
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

    setDevServer: (info) =>
        set((s) => {
            if (!info) return { devServer: null }
            return {
                devServer: info,
                devServers: { ...s.devServers, [info.workspace]: info }
            }
        }),

    devServerForWorkspace: (workspace) => get().devServers[workspace] ?? null,

    beginAttempt: (workspace) => {
        const attempt = (get().attempts[workspace] ?? 0) + 1
        set((s) => ({ attempts: { ...s.attempts, [workspace]: attempt } }))
        return attempt
    },

    attemptForWorkspace: (workspace) => get().attempts[workspace] ?? 0,

    restoreAttempt: (workspace, expected, previous) => {
        if ((get().attempts[workspace] ?? 0) !== expected) return false
        set((s) => ({ attempts: { ...s.attempts, [workspace]: previous } }))
        return true
    },

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

    recordNativeOpen: (workspace, url) =>
        set((s) => {
            const current = s.nativeSession
            if (current?.workspacePath !== workspace) {
                return {
                    nativeSession: {
                        workspacePath: workspace,
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
                    backStack: [...current.backStack, current.currentUrl],
                    forwardStack: []
                }
            }
        }),

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

    openStaticPreview: (workspace, session) => {
        const previous = get().staticPreview
        set({
            staticPreview: {
                workspace,
                token: session.token,
                url: session.url
            }
        })
        const opened = get().navigate(workspace, session.url)
        if (previous && previous.token !== session.token) {
            void previewRevoke(previous.token).catch(() => undefined)
        }
        return opened
    },

    revokeStaticPreview: () => {
        const session = get().staticPreview
        if (!session) return
        set({ staticPreview: null })
        void previewRevoke(session.token).catch(() => undefined)
    },

    reset: () => {
        const session = get().staticPreview
        set(previewInitialState)
        if (session) {
            void previewRevoke(session.token).catch(() => undefined)
        }
    }
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
        const current = usePreviewStore.getState()
        if (!current.nativeRequestIsCurrent(token)) return
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
        usePreviewStore.getState().revokeStaticPreview()
        closeNativePreviewForOwner(previous.workspacePath)
        return
    }
    if (previewTabIsOpen(previous.groups) && !previewTabIsOpen(state.groups)) {
        usePreviewStore.getState().revokeStaticPreview()
        closeNativePreviewForOwner(state.workspacePath)
    }
})
