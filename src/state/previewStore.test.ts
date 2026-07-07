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
