import { create } from "zustand"

import type { DevServerInfo } from "../lib/types"

export type ResponsiveFrame = "full" | "mobile"

export interface PreviewNavState {
    url: string | null
    backStack: string[]
    forwardStack: string[]
    reloadNonce: number
    frame: ResponsiveFrame
}

interface PreviewState {
    devServer: DevServerInfo | null
    devServers: Record<string, DevServerInfo>
    nav: Record<string, PreviewNavState>
    setDevServer: (info: DevServerInfo | null) => void
    devServerForWorkspace: (workspace: string) => DevServerInfo | null
    navForWorkspace: (workspace: string) => PreviewNavState
    navigate: (workspace: string, url: string) => boolean
    goBack: (workspace: string) => void
    goForward: (workspace: string) => void
    reload: (workspace: string) => void
    setFrame: (workspace: string, frame: ResponsiveFrame) => void
    reset: () => void
}

export const previewInitialState = {
    devServer: null as DevServerInfo | null,
    devServers: {} as Record<string, DevServerInfo>,
    nav: {} as Record<string, PreviewNavState>
}

function isAllowedPreviewUrl(rawUrl: string): boolean {
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

    navForWorkspace: (workspace) => get().nav[workspace] ?? defaultNav(),

    navigate: (workspace, url) => {
        if (!isAllowedPreviewUrl(url)) return false
        set((s) => {
            const nav = s.nav[workspace] ?? defaultNav()
            return {
                nav: {
                    ...s.nav,
                    [workspace]: {
                        ...nav,
                        url,
                        backStack: nav.url ? [...nav.backStack, nav.url] : nav.backStack,
                        forwardStack: []
                    }
                }
            }
        })
        return true
    },

    goBack: (workspace) =>
        set((s) => {
            const nav = s.nav[workspace] ?? defaultNav()
            if (!nav.url || nav.backStack.length === 0) return s
            const url = nav.backStack[nav.backStack.length - 1]
            return {
                nav: {
                    ...s.nav,
                    [workspace]: {
                        ...nav,
                        url,
                        backStack: nav.backStack.slice(0, -1),
                        forwardStack: [nav.url, ...nav.forwardStack]
                    }
                }
            }
        }),

    goForward: (workspace) =>
        set((s) => {
            const nav = s.nav[workspace] ?? defaultNav()
            if (nav.forwardStack.length === 0) return s
            const [url, ...forwardStack] = nav.forwardStack
            return {
                nav: {
                    ...s.nav,
                    [workspace]: {
                        ...nav,
                        url,
                        backStack: nav.url ? [...nav.backStack, nav.url] : nav.backStack,
                        forwardStack
                    }
                }
            }
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
