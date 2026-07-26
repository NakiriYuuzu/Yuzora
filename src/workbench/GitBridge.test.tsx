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
    useWorkspaceStore.setState({
        workspacePath: "/w",
        groups: [{ tabs: [], activePath: null }],
        activeGroupIndex: 0
    })
})
afterEach(() => {
    cleanup()
    vi.clearAllMocks()
})

// 掛好 GitBridge 並以 stub 取代 store 動作（detect 一併 stub 掉，避免 effect A
// 走真 IPC）；回傳 stubs 供斷言。
async function mountBridge() {
    const refresh = vi.fn(async () => {})
    const loadBranches = vi.fn(async () => {})
    const detect = vi.fn(async () => {})
    useGitStore.setState({ refresh, loadBranches, detect })

    render(<GitBridge />)
    // The listeners are registered asynchronously (listen returns a promise).
    await vi.waitFor(() => expect(listeners.has("git:state-changed")).toBe(true))
    await vi.waitFor(() => expect(listeners.has("fs:external-change")).toBe(true))
    return { refresh, loadBranches }
}

it("git:state-changed for the live workspace refreshes status AND reloads branches", async () => {
    const { refresh, loadBranches } = await mountBridge()

    listeners.get("git:state-changed")!({ payload: { workspaceRoot: "/w" } })
    expect(refresh).toHaveBeenCalledTimes(1)
    expect(loadBranches).toHaveBeenCalledTimes(1)
})

// #57 T3 AC4：事件帶 workspace 標識，listener 過濾串場事件——切換 gap 內舊
// workspace 的 .git watcher 開火不得刷新新 workspace 的 git 面板。
it("git:state-changed from a stale workspace is dropped (#57 T3)", async () => {
    const { refresh, loadBranches } = await mountBridge()

    listeners.get("git:state-changed")!({ payload: { workspaceRoot: "/old" } })
    expect(refresh).not.toHaveBeenCalled()
    expect(loadBranches).not.toHaveBeenCalled()
})

it("fs:external-change refreshes only when the event matches the live workspace (#57 T3)", async () => {
    const { refresh } = await mountBridge()

    listeners.get("fs:external-change")!({
        payload: { workspaceRoot: "/old", paths: ["/old/a.ts"] }
    })
    expect(refresh).not.toHaveBeenCalled()

    listeners.get("fs:external-change")!({
        payload: { workspaceRoot: "/w", paths: ["/w/a.ts"] }
    })
    expect(refresh).toHaveBeenCalledTimes(1)
})
