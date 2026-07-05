import { it, expect, vi, beforeEach, afterEach } from "vitest"
import { render, cleanup } from "@testing-library/react"

const listeners = new Map<string, (e: unknown) => void>()
vi.mock("@tauri-apps/api/event", () => ({
    listen: vi.fn(async (event: string, cb: unknown) => {
        listeners.set(event, cb as (e: unknown) => void)
        return () => listeners.delete(event)
    })
}))

const stopWorkspace = vi.fn()
vi.mock("../lsp/lspManager", () => ({
    stopWorkspace: (...a: unknown[]) => stopWorkspace(...a)
}))

import { LspBridge } from "./LspBridge"
import { useLspStore } from "../state/lspStore"
import { useWorkspaceStore } from "../state/workspaceStore"

beforeEach(() => {
    listeners.clear()
    stopWorkspace.mockReset()
    useLspStore.getState().reset()
    useWorkspaceStore.setState({ workspacePath: "/ws-a" })
})
afterEach(() => {
    cleanup()
    vi.clearAllMocks()
})

function info(workspace: string) {
    return {
        workspace,
        language: "typescript",
        serverId: "ts",
        command: "tsserver",
        path: null,
        status: { status: "starting" },
        lastStartupLog: null,
        lastError: null,
        restartCount: 0
    }
}

it("feeds lsp:server-status payloads for the current workspace into the store", async () => {
    render(<LspBridge />)
    await vi.waitFor(() => expect(listeners.has("lsp:server-status")).toBe(true))

    const payload = info("/ws-a")
    listeners.get("lsp:server-status")!({ payload })
    expect(useLspStore.getState().servers.typescript).toEqual(payload)
})

it("drops lsp:server-status events from a workspace the UI has left (S1)", async () => {
    render(<LspBridge />)
    await vi.waitFor(() => expect(listeners.has("lsp:server-status")).toBe(true))

    // stale event from a different workspace: ignored
    listeners.get("lsp:server-status")!({ payload: info("/ws-other") })
    expect(useLspStore.getState().servers.typescript).toBeUndefined()

    // event for the current workspace: accepted
    listeners.get("lsp:server-status")!({ payload: info("/ws-a") })
    expect(useLspStore.getState().servers.typescript).toBeDefined()
})

it("stops the previous workspace and resets state when the workspace changes", async () => {
    render(<LspBridge />)
    // feed some state so we can observe the reset
    useLspStore.getState().setInitialized("typescript", true)

    useWorkspaceStore.setState({ workspacePath: "/ws-b" })
    await vi.waitFor(() => expect(stopWorkspace).toHaveBeenCalledWith("/ws-a"))
    expect(useLspStore.getState().initialized).toEqual({})
})
