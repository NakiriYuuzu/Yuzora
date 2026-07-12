import { beforeEach, describe, expect, it } from "vitest"

import type { DevServerInfo } from "../lib/types"
import { isLocalPreviewUrl, previewInitialState, usePreviewStore } from "./previewStore"

const runningServer: DevServerInfo = {
    workspace: "/ws/a",
    command: "bun run dev",
    port: 5173,
    status: { status: "running", port: 5173 }
}

const otherServer: DevServerInfo = {
    workspace: "/ws/b",
    command: "bun run dev",
    port: 3000,
    status: { status: "running", port: 3000 }
}

beforeEach(() => usePreviewStore.getState().reset())

describe("usePreviewStore", () => {
    it("sets dev server info and keeps it isolated by workspace", () => {
        const s = usePreviewStore.getState()

        s.setDevServer(runningServer)
        s.setDevServer(otherServer)

        const state = usePreviewStore.getState()
        expect(state.devServers["/ws/a"]).toEqual(runningServer)
        expect(state.devServers["/ws/b"]).toEqual(otherServer)
        expect(state.devServerForWorkspace("/ws/a")).toEqual(runningServer)
    })

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

    it("tracks async attempt identity per workspace and only restores the expected claim", () => {
        const s = usePreviewStore.getState()

        expect(s.attemptForWorkspace("/ws/a")).toBe(0)
        expect(s.beginAttempt("/ws/a")).toBe(1)
        expect(s.beginAttempt("/ws/a")).toBe(2)
        expect(s.beginAttempt("/ws/b")).toBe(1)
        expect(s.restoreAttempt("/ws/a", 1, 0)).toBe(false)
        expect(s.restoreAttempt("/ws/a", 2, 1)).toBe(true)
        expect(s.attemptForWorkspace("/ws/a")).toBe(1)
        expect(s.attemptForWorkspace("/ws/b")).toBe(1)
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

    it("reset restores the exported initial state", () => {
        const s = usePreviewStore.getState()
        s.setDevServer(runningServer)
        s.navigate("/ws/a", "http://localhost:5173")
        s.reload("/ws/a")
        s.setFrame("/ws/a", "mobile")

        s.reset()

        expect(usePreviewStore.getState()).toMatchObject(previewInitialState)
    })
})
