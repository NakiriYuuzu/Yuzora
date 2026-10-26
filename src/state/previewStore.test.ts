import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks"

import { enqueueNativePreviewOperation } from "../preview/nativePreviewQueue"
import {
    isLocalPreviewUrl,
    usePreviewStore
} from "./previewStore"
import { PREVIEW_TAB_PATH, useWorkspaceStore } from "./workspaceStore"

beforeEach(() => {
    clearMocks()
    usePreviewStore.getState().reset()
})

afterEach(() => {
    clearMocks()
})

describe("usePreviewStore", () => {

    it("navigates local URLs, pushes history, and clears forward history", () => {
        const s = usePreviewStore.getState()

        expect(s.navigate("/ws/a", "http://localhost:5173")).toBe(true)
        expect(s.navigate("/ws/a", "http://localhost:5173/about")).toBe(true)
        s.goBack("/ws/a")
        expect(s.navigate("/ws/a", "http://127.0.0.1:3000")).toBe(true)

        const nav = usePreviewStore.getState().navForWorkspace("/ws/a")
        expect(nav.url).toBe("http://127.0.0.1:3000")
        expect(nav.backStack).toEqual(["http://localhost:5173"])
        expect(nav.forwardStack).toEqual([])
    })

    it("clamps back and forward navigation at history boundaries", () => {
        const s = usePreviewStore.getState()
        s.navigate("/ws/a", "http://localhost:5173")
        s.navigate("/ws/a", "http://localhost:5173/about")

        s.goBack("/ws/a")
        s.goBack("/ws/a")
        expect(usePreviewStore.getState().navForWorkspace("/ws/a").url).toBe("http://localhost:5173")
        expect(usePreviewStore.getState().navForWorkspace("/ws/a").backStack).toEqual([])
        expect(usePreviewStore.getState().navForWorkspace("/ws/a").forwardStack).toEqual([
            "http://localhost:5173/about"
        ])

        s.goForward("/ws/a")
        s.goForward("/ws/a")
        expect(usePreviewStore.getState().navForWorkspace("/ws/a").url).toBe("http://localhost:5173/about")
        expect(usePreviewStore.getState().navForWorkspace("/ws/a").backStack).toEqual(["http://localhost:5173"])
        expect(usePreviewStore.getState().navForWorkspace("/ws/a").forwardStack).toEqual([])
    })

    it("keeps preview history scoped per workspace", () => {
        const s = usePreviewStore.getState()
        s.navigate("/ws/a", "http://localhost:5173")
        s.navigate("/ws/a", "http://localhost:5173/about")
        s.navigate("/ws/b", "http://127.0.0.1:3000")

        s.goBack("/ws/b")

        expect(usePreviewStore.getState().navForWorkspace("/ws/b").url).toBe("http://127.0.0.1:3000")
        expect(usePreviewStore.getState().navForWorkspace("/ws/b").backStack).toEqual([])
        expect(usePreviewStore.getState().navForWorkspace("/ws/a").url).toBe("http://localhost:5173/about")
    })

    it("syncs native history into nav state with a consumable one-shot marker", () => {
        const s = usePreviewStore.getState()
        s.navigate("/ws/a", "https://example.com/a")
        s.recordNativeOpen("/ws/a", "https://example.com/a")
        s.navigate("/ws/a", "https://example.com/b")
        s.recordNativeOpen("/ws/a", "https://example.com/b")

        expect(s.syncNativeBack("/ws/a")).toBe(true)
        let state = usePreviewStore.getState()
        expect(state.navForWorkspace("/ws/a")).toMatchObject({
            url: "https://example.com/a",
            backStack: [],
            forwardStack: ["https://example.com/b"]
        })
        const backMarker = state.nativeNavigationSyncs["/ws/a"]
        expect(backMarker).toEqual({ url: "https://example.com/a", token: 1 })
        state.consumeNativeNavigationSync("/ws/a", 999)
        expect(usePreviewStore.getState().nativeNavigationSyncs["/ws/a"]).toEqual(backMarker)
        state.consumeNativeNavigationSync("/ws/a", backMarker.token)
        expect(usePreviewStore.getState().nativeNavigationSyncs["/ws/a"]).toBeUndefined()

        expect(usePreviewStore.getState().syncNativeForward("/ws/a")).toBe(true)
        state = usePreviewStore.getState()
        expect(state.navForWorkspace("/ws/a")).toMatchObject({
            url: "https://example.com/b",
            backStack: ["https://example.com/a"],
            forwardStack: []
        })
        expect(state.nativeNavigationSyncs["/ws/a"]).toEqual({
            url: "https://example.com/b",
            token: 2
        })
        expect(state.nativeSession).toMatchObject({
            workspacePath: "/ws/a",
            currentUrl: "https://example.com/b",
            backStack: ["https://example.com/a"],
            forwardStack: []
        })
        state.navigate("/ws/a", "https://example.com/c")
        expect(usePreviewStore.getState().nativeNavigationSyncs["/ws/a"]).toBeUndefined()
        state.closeNativeSession("/ws/b")
        expect(usePreviewStore.getState().nativeSession).not.toBeNull()
        state.closeNativeSession("/ws/a")
        expect(usePreviewStore.getState().nativeSession).toBeNull()
    })

    it("close intent immediately revokes native identity while preserving outer navigation", () => {
        const s = usePreviewStore.getState()
        useWorkspaceStore.setState({ workspacePath: "/ws/a" })
        s.navigate("/ws/a", "https://example.com/a")
        s.recordNativeOpen("/ws/a", "https://example.com/a", "owned-old-session")
        s.receiveNativeNavigation({ sessionId: "owned-old-session", url: "https://example.com/b", canGoBack: true, canGoForward: false })
        const nav = usePreviewStore.getState().nav
        const markers = usePreviewStore.getState().nativeNavigationSyncs
        const close = s.beginNativeCloseRequest("/ws/a")
        expect(usePreviewStore.getState().nativeSession).toBeNull()
        s.beginNativeOpenRequest("/ws/a", "https://example.com/b")
        expect(s.nativeRequestIsCurrent(close)).toBe(false)
        expect(usePreviewStore.getState().nativeSession).toBeNull()
        expect(usePreviewStore.getState().nav).toBe(nav)
        expect(usePreviewStore.getState().nativeNavigationSyncs).toBe(markers)
    })

    it("assigns monotonic native open/close request tokens", () => {
        const s = usePreviewStore.getState()
        const first = s.beginNativeOpenRequest("/ws/a", "https://example.com/a")
        const second = s.beginNativeOpenRequest("/ws/a", "https://example.com/b")
        const close = s.beginNativeCloseRequest("/ws/a")

        expect([first, second, close]).toEqual([1, 2, 3])
        expect(s.nativeRequestIsCurrent(first)).toBe(false)
        expect(s.nativeRequestIsCurrent(close)).toBe(true)
        expect(usePreviewStore.getState().nativeRequest).toEqual({
            token: 3,
            kind: "close",
            workspacePath: "/ws/a"
        })
        expect(s.settleNativeRequest(second)).toBe(false)
        expect(s.settleNativeRequest(close)).toBe(true)
        expect(usePreviewStore.getState().nativeRequest).toBeNull()
    })

    it("reload bumps the nonce and responsive frame switches between full and mobile", () => {
        const s = usePreviewStore.getState()

        s.reload("/ws/a")
        s.reload("/ws/a")
        s.setFrame("/ws/a", "mobile")

        const nav = usePreviewStore.getState().navForWorkspace("/ws/a")
        expect(nav.reloadNonce).toBe(2)
        expect(nav.frame).toBe("mobile")
    })

    it("admits any http/https URL and rejects non-web schemes at the navigate choke point", () => {
        const s = usePreviewStore.getState()

        // P3: external https is now allowed (rendered in a child webview).
        expect(s.navigate("/ws/a", "https://example.com")).toBe(true)
        expect(s.navigate("/ws/a", "http://localhost:5173")).toBe(true)
        // Non-web schemes and garbage are still rejected.
        expect(s.navigate("/ws/a", "file:///etc/passwd")).toBe(false)
        expect(s.navigate("/ws/a", "javascript:alert(1)")).toBe(false)
        expect(s.navigate("/ws/a", "not a url")).toBe(false)

        const nav = usePreviewStore.getState().navForWorkspace("/ws/a")
        expect(nav.backStack).toEqual(["https://example.com"])
        expect(nav.url).toBe("http://localhost:5173")
    })

    it("does not create a no-op history entry for the current URL", () => {
        const s = usePreviewStore.getState()

        expect(s.navigate("/ws/a", "https://example.com")).toBe(true)
        expect(s.navigate("/ws/a", "https://example.com")).toBe(true)

        expect(usePreviewStore.getState().navForWorkspace("/ws/a")).toMatchObject({
            url: "https://example.com",
            backStack: [],
            forwardStack: []
        })
    })

    it("isLocalPreviewUrl separates local dev/static servers from external URLs", () => {
        expect(isLocalPreviewUrl("http://localhost:5173")).toBe(true)
        expect(isLocalPreviewUrl("http://127.0.0.1:4599/index.html")).toBe(true)
        // External https (and even https on localhost) goes to the child webview.
        expect(isLocalPreviewUrl("https://example.com")).toBe(false)
        expect(isLocalPreviewUrl("https://localhost:5173")).toBe(false)
        expect(isLocalPreviewUrl(null)).toBe(false)
        expect(isLocalPreviewUrl("garbage")).toBe(false)
    })

    it("physically closes a native preview when its workspace or Preview tab goes away", async () => {
        const calls: Array<{ cmd: string; args: Record<string, unknown> }> = []
        mockIPC((cmd, args) => {
            calls.push({ cmd, args: (args ?? {}) as Record<string, unknown> })
        })
        useWorkspaceStore.setState({
            workspacePath: "/ws/a",
            groups: [{
                tabs: [{
                    path: PREVIEW_TAB_PATH,
                    name: "Preview",
                    dirty: false,
                    externallyModified: false,
                    kind: "preview"
                }],
                activePath: PREVIEW_TAB_PATH
            }],
            activeGroupIndex: 0
        })
        usePreviewStore.getState().navigate("/ws/a", "https://example.com")
        usePreviewStore.getState().recordNativeOpen("/ws/a", "https://example.com")

        useWorkspaceStore.getState().setWorkspace("/ws/b")
        await new Promise((resolve) => setTimeout(resolve, 0))
        expect(calls.some((call) => call.cmd === "preview_close")).toBe(true)
        expect(usePreviewStore.getState().nativeSession).toBeNull()

        calls.length = 0
        useWorkspaceStore.setState({
            workspacePath: "/ws/a",
            groups: [{
                tabs: [{
                    path: PREVIEW_TAB_PATH,
                    name: "Preview",
                    dirty: false,
                    externallyModified: false,
                    kind: "preview"
                }],
                activePath: PREVIEW_TAB_PATH
            }],
            activeGroupIndex: 0
        })
        usePreviewStore.getState().recordNativeOpen("/ws/a", "https://example.com")
        useWorkspaceStore.getState().closePreviewTab()
        await new Promise((resolve) => setTimeout(resolve, 0))
        expect(calls.some((call) => call.cmd === "preview_close")).toBe(true)
        expect(usePreviewStore.getState().nativeSession).toBeNull()
    })

    it("still physically closes a departing native preview when reset invalidates its queued request", async () => {
        const calls: Array<{ cmd: string; args: Record<string, unknown> }> = []
        mockIPC((cmd, args) => {
            calls.push({ cmd, args: (args ?? {}) as Record<string, unknown> })
        })
        useWorkspaceStore.setState({ workspacePath: "/ws/a" })
        usePreviewStore.getState().recordNativeOpen("/ws/a", "https://example.com")

        let releaseQueue!: () => void
        const blockingOperation = enqueueNativePreviewOperation(
            () => new Promise<void>((resolve) => {
                releaseQueue = resolve
            })
        )
        await Promise.resolve()

        useWorkspaceStore.getState().setWorkspace("/ws/b")
        usePreviewStore.getState().reset()
        releaseQueue()
        await blockingOperation
        await new Promise((resolve) => setTimeout(resolve, 0))

        expect(calls.some((call) => call.cmd === "preview_close")).toBe(true)
    })

    describe("navigation failure detection", () => {
        const A = "https://example.com/a"
        const X = "https://example.com/x"
        const snap = (url: string, over: Record<string, unknown> = {}) => ({
            sessionId: "s1", url, canGoBack: true, canGoForward: false, loading: false, backCount: 0, ...over
        })
        const receive = (event: ReturnType<typeof snap>) => usePreviewStore.getState().receiveNativeNavigation(event)
        const failureOf = () => usePreviewStore.getState().navigationFailures["/ws/a"]

        beforeEach(() => {
            vi.useFakeTimers()
            useWorkspaceStore.setState({ workspacePath: "/ws/a" })
            const s = usePreviewStore.getState()
            s.navigate("/ws/a", A)
            s.recordNativeOpen("/ws/a", A, "s1")
            // The first idle snapshot teaches the store the native back-list length.
            receive(snap(A))
            s.navigate("/ws/a", X)
            s.recordNativeOpen("/ws/a", X, "s1")
        })
        afterEach(() => vi.useRealTimers())

        it("reports a failure when the idle view is back on the previous page with no new history", () => {
            receive(snap(A))
            const state = usePreviewStore.getState()
            expect(state.navigationFailures["/ws/a"]).toMatchObject({ url: X })
            expect(state.nav["/ws/a"].url).toBe(A)
            expect(state.nav["/ws/a"].backStack).toEqual([])
            expect(state.nativeSession?.currentUrl).toBe(A)
            expect(state.nativeSession?.backStack).toEqual([])
            expect(state.pendingNavigations["/ws/a"]).toBeUndefined()
        })

        it("detects a real failure even when the webview has forward history", () => {
            receive(snap(A, { canGoForward: true }))
            expect(failureOf()).toMatchObject({ url: X })
        })

        it("keeps the navigation pending while the view is loading, then reports the failure", () => {
            receive(snap(X, { loading: true }))
            vi.advanceTimersByTime(10_000)
            receive(snap(A, { loading: true }))
            expect(failureOf()).toBeUndefined()
            expect(usePreviewStore.getState().pendingNavigations["/ws/a"]).toBeDefined()
            receive(snap(A))
            expect(failureOf()).toMatchObject({ url: X })
            expect(usePreviewStore.getState().nav["/ws/a"].backStack).toEqual([])
        })

        it("treats a page that committed while still loading as success, so a Back is no failure", () => {
            receive(snap(X, { loading: true, backCount: 1 }))
            expect(usePreviewStore.getState().pendingNavigations["/ws/a"]).toBeUndefined()
            receive(snap(A, { canGoForward: true, backCount: 0 }))
            expect(failureOf()).toBeUndefined()
            expect(usePreviewStore.getState().nav["/ws/a"].url).toBe(A)
        })

        it("keeps history capabilities live while the requested page is still loading", () => {
            receive(snap(X, { loading: true, canGoBack: false, canGoForward: true }))
            const state = usePreviewStore.getState()
            expect(state.nativeSession).toMatchObject({ canGoBack: false, canGoForward: true, backCount: 0 })
            expect(state.nav["/ws/a"].url).toBe(X)
            expect(state.pendingNavigations["/ws/a"]).toBeDefined()
        })

        it("bases a navigation typed during a committed load on the fresh back-list length", () => {
            const C = "https://example.com/c"
            receive(snap(X, { loading: true, backCount: 1 }))
            usePreviewStore.getState().navigate("/ws/a", C)
            usePreviewStore.getState().recordNativeOpen("/ws/a", C, "s1")
            expect(usePreviewStore.getState().pendingNavigations["/ws/a"]).toMatchObject({ backCount: 1 })
            receive(snap(X, { backCount: 1 }))
            expect(failureOf()).toMatchObject({ url: C })
        })

        it("does not report a slow load that keeps reporting the requested URL", () => {
            for (let i = 0; i < 5; i++) { vi.advanceTimersByTime(5000); receive(snap(X, { loading: true })) }
            expect(failureOf()).toBeUndefined()
            expect(usePreviewStore.getState().nav["/ws/a"].url).toBe(X)
        })

        it("treats a committed load as success and ignores a later in-page return to the previous URL", () => {
            receive(snap(X, { backCount: 1 }))
            expect(usePreviewStore.getState().pendingNavigations["/ws/a"]).toBeUndefined()
            vi.advanceTimersByTime(5000)
            // An in-page link / JS router goes back to A: history grows, still no failure.
            receive(snap(A, { backCount: 2 }))
            const state = usePreviewStore.getState()
            expect(state.navigationFailures["/ws/a"]).toBeUndefined()
            expect(state.nav["/ws/a"].url).toBe(A)
            expect(state.nav["/ws/a"].backStack).toEqual([A])
        })

        it("does not treat a server redirect back to the previous URL as a failure", () => {
            receive(snap(A, { backCount: 1 }))
            const state = usePreviewStore.getState()
            expect(state.navigationFailures["/ws/a"]).toBeUndefined()
            expect(state.pendingNavigations["/ws/a"]).toBeUndefined()
            expect(state.nav["/ws/a"].backStack).toEqual([A])
        })

        it("does not track a re-entry that differs from the current page only by a trailing slash", () => {
            usePreviewStore.getState().reset()
            useWorkspaceStore.setState({ workspacePath: "/ws/a" })
            const s = usePreviewStore.getState()
            s.navigate("/ws/a", "http://localhost:3000/")
            s.recordNativeOpen("/ws/a", "http://localhost:3000/", "s1")
            receive(snap("http://localhost:3000/"))
            s.navigate("/ws/a", "http://localhost:3000")
            expect(usePreviewStore.getState().pendingNavigations["/ws/a"]).toBeUndefined()
            s.recordNativeOpen("/ws/a", "http://localhost:3000", "s1")
            receive(snap("http://localhost:3000/"))
            expect(failureOf()).toBeUndefined()
        })

        it("compares URLs normalized when judging a failure", () => {
            usePreviewStore.getState().reset()
            useWorkspaceStore.setState({ workspacePath: "/ws/a" })
            const s = usePreviewStore.getState()
            s.navigate("/ws/a", "http://localhost:3000")
            s.recordNativeOpen("/ws/a", "http://localhost:3000", "s1")
            receive(snap("http://localhost:3000/"))
            s.navigate("/ws/a", "http://localhost:3000/x")
            s.recordNativeOpen("/ws/a", "http://localhost:3000/x", "s1")
            receive(snap("http://localhost:3000/"))
            expect(failureOf()).toMatchObject({ url: "http://localhost:3000/x" })
        })

        it("never reports a failure for snapshots without loading/backCount (Windows shape)", () => {
            receive({ sessionId: "s1", url: A, canGoBack: true, canGoForward: false } as ReturnType<typeof snap>)
            const state = usePreviewStore.getState()
            expect(state.navigationFailures["/ws/a"]).toBeUndefined()
            expect(state.pendingNavigations["/ws/a"]).toBeUndefined()
        })

        it("does not start tracking before the native back count is known", () => {
            usePreviewStore.getState().reset()
            const s = usePreviewStore.getState()
            s.navigate("/ws/a", A)
            s.recordNativeOpen("/ws/a", A, "s1")
            s.navigate("/ws/a", X)
            expect(usePreviewStore.getState().pendingNavigations["/ws/a"]).toBeUndefined()
        })

        it("treats a back after commit (requested URL still ahead) as normal navigation", () => {
            receive(snap(X, { backCount: 1 }))
            receive(snap(A, { canGoBack: false, canGoForward: true, backCount: 0 }))
            const state = usePreviewStore.getState()
            expect(state.navigationFailures["/ws/a"]).toBeUndefined()
            expect(state.nav["/ws/a"].url).toBe(A)
        })

        it("stops tracking silently after the expiry window", () => {
            vi.advanceTimersByTime(31_000)
            receive(snap(A))
            const state = usePreviewStore.getState()
            expect(state.navigationFailures["/ws/a"]).toBeUndefined()
            expect(state.pendingNavigations["/ws/a"]).toBeUndefined()
        })

        it("treats a redirect to a third URL as success", () => {
            receive(snap("https://example.com/login", { backCount: 1 }))
            const state = usePreviewStore.getState()
            expect(state.navigationFailures["/ws/a"]).toBeUndefined()
            expect(state.nav["/ws/a"].url).toBe("https://example.com/login")
            expect(state.pendingNavigations["/ws/a"]).toBeUndefined()
        })

        it.each([
            ["the view loads again", () => snap(A, { loading: true })],
            ["the view moves to another page", () => snap("https://example.com/other")],
            ["the native history changes", () => snap(A, { backCount: 1 })]
        ])("retires a stale failure when %s", (_label, next) => {
            receive(snap(A))
            expect(failureOf()).toBeDefined()
            receive(snap(A)) // an identical idle snapshot keeps the alert
            expect(failureOf()).toBeDefined()
            receive(next())
            expect(failureOf()).toBeUndefined()
        })

        it("clears the failure on the next navigation, history move or explicit clear", () => {
            receive(snap(A))
            usePreviewStore.getState().clearNavigationFailure("/ws/a")
            expect(failureOf()).toBeUndefined()

            usePreviewStore.getState().navigate("/ws/a", "https://example.com/y")
            usePreviewStore.getState().recordNativeOpen("/ws/a", "https://example.com/y", "s1")
            receive(snap(A))
            expect(failureOf()).toMatchObject({ url: "https://example.com/y" })
            usePreviewStore.getState().navigate("/ws/a", "https://example.com/z")
            expect(failureOf()).toBeUndefined()
        })
    })
})
