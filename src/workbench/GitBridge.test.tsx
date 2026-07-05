import { expect, it, vi, beforeEach, afterEach } from "vitest"
import { render, cleanup } from "@testing-library/react"
import { GitBridge } from "./GitBridge"
import { useGitStore, initialGitState } from "../state/gitStore"
import { useWorkspaceStore } from "../state/workspaceStore"

// Capture each event listener callback by event name so tests can fire them.
const listeners = new Map<string, (e: unknown) => void>()

vi.mock("@tauri-apps/api/event", () => ({
    listen: vi.fn(async (event: string, cb: unknown) => {
        listeners.set(event, cb as (e: unknown) => void)
        return () => listeners.delete(event)
    })
}))

beforeEach(() => {
    listeners.clear()
    useGitStore.setState(initialGitState)
    // No workspace → detect effect is a no-op; keeps the test focused on the
    // git:state-changed handler wiring.
    useWorkspaceStore.setState({
        workspacePath: null,
        groups: [{ tabs: [], activePath: null }],
        activeGroupIndex: 0
    })
})
afterEach(() => {
    cleanup()
    vi.clearAllMocks()
})

it("git:state-changed refreshes status AND reloads branches", async () => {
    const refresh = vi.fn(async () => {})
    const loadBranches = vi.fn(async () => {})
    useGitStore.setState({ refresh, loadBranches })

    render(<GitBridge />)
    // The listener is registered asynchronously (listen returns a promise).
    await vi.waitFor(() => expect(listeners.has("git:state-changed")).toBe(true))

    listeners.get("git:state-changed")!({})
    expect(refresh).toHaveBeenCalledTimes(1)
    expect(loadBranches).toHaveBeenCalledTimes(1)
})
