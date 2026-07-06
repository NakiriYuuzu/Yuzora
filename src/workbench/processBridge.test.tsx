import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { cleanup, render } from "@testing-library/react"

const listeners = new Map<string, (e: unknown) => void>()
vi.mock("@tauri-apps/api/event", () => ({
    listen: vi.fn(async (event: string, cb: unknown) => {
        listeners.set(event, cb as (e: unknown) => void)
        return () => listeners.delete(event)
    })
}))

const devServerStopWorkspace = vi.fn()
vi.mock("../lib/ipc", () => ({
    devServerStopWorkspace: (...args: unknown[]) => devServerStopWorkspace(...args)
}))

import { ProcessBridge } from "./ProcessBridge"
import { usePreviewStore } from "../state/previewStore"
import { useWorkspaceStore } from "../state/workspaceStore"
import type { DevServerInfo } from "../lib/types"

function serverInfo(workspace: string, port: number): DevServerInfo {
    return {
        workspace,
        command: "bun run dev",
        port,
        status: { status: "running", port }
    }
}

beforeEach(() => {
    listeners.clear()
    devServerStopWorkspace.mockReset()
    usePreviewStore.getState().reset()
    useWorkspaceStore.setState({ workspacePath: "/ws-a" })
})

afterEach(() => {
    cleanup()
    vi.clearAllMocks()
})

it("feeds dev-server:status events for the current workspace into previewStore", async () => {
    render(<ProcessBridge />)
    await vi.waitFor(() => expect(listeners.has("dev-server:status")).toBe(true))

    const payload = serverInfo("/ws-a", 5173)
    listeners.get("dev-server:status")!({ payload })

    expect(usePreviewStore.getState().devServerForWorkspace("/ws-a")).toEqual(payload)
})

it("drops dev-server:status events from workspaces the UI has already left", async () => {
    render(<ProcessBridge />)
    await vi.waitFor(() => expect(listeners.has("dev-server:status")).toBe(true))

    listeners.get("dev-server:status")!({ payload: serverInfo("/ws-other", 3000) })
    expect(usePreviewStore.getState().devServerForWorkspace("/ws-other")).toBeNull()

    listeners.get("dev-server:status")!({ payload: serverInfo("/ws-a", 5173) })
    expect(usePreviewStore.getState().devServerForWorkspace("/ws-a")?.port).toBe(5173)
})

it("stops the previous workspace dev server and resets preview state when workspace changes", async () => {
    render(<ProcessBridge />)
    usePreviewStore.getState().setDevServer(serverInfo("/ws-a", 5173))
    usePreviewStore.getState().navigate("/ws-a", "http://localhost:5173")

    useWorkspaceStore.setState({ workspacePath: "/ws-b" })

    await vi.waitFor(() => expect(devServerStopWorkspace).toHaveBeenCalledWith("/ws-a"))
    expect(usePreviewStore.getState().devServerForWorkspace("/ws-a")).toBeNull()
    expect(usePreviewStore.getState().navForWorkspace("/ws-a").url).toBeNull()
})
