import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { cleanup, render } from "@testing-library/react"
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks"
import { emit } from "@tauri-apps/api/event"

import { AGENT_SETTINGS_STORAGE_KEY } from "../app/workbench/SettingsDialog"
import { createFakeAcpAgentBridge } from "../agent/fakeAcpAgent"
import { agentInitialState, useAgentStore } from "../state/agentStore"
import { useWorkspaceStore } from "../state/workspaceStore"
import { AgentBridge } from "./AgentBridge"

type IpcCall = [string, unknown]

function deferred<T = void>() {
    let resolve!: (value: T | PromiseLike<T>) => void
    const promise = new Promise<T>((r) => {
        resolve = r
    })
    return { promise, resolve }
}

function resetAgentStore() {
    useAgentStore.setState({
        ...agentInitialState,
        sessions: new Map(),
        pendingPermissions: new Map()
    })
}

function installLocalStorage(): void {
    const store = new Map<string, string>()
    const mock = {
        getItem: (k: string) => (store.has(k) ? store.get(k)! : null),
        setItem: (k: string, v: string) => void store.set(k, String(v)),
        removeItem: (k: string) => void store.delete(k),
        clear: () => store.clear(),
        key: (i: number) => [...store.keys()][i] ?? null,
        get length() {
            return store.size
        }
    }
    Object.defineProperty(globalThis, "localStorage", {
        value: mock,
        configurable: true,
        writable: true
    })
}

beforeEach(() => {
    clearMocks()
    installLocalStorage()
    localStorage.clear()
    resetAgentStore()
    useWorkspaceStore.setState({ workspacePath: "/ws-a" })
})

afterEach(() => {
    cleanup()
    resetAgentStore()
    useWorkspaceStore.setState({ workspacePath: null })
    clearMocks()
    vi.clearAllMocks()
})

it("wires agent stdout through the ACP connection and updates the agent store", async () => {
    const agentId = "agent-1"
    const fake = createFakeAcpAgentBridge((line) => emit("agent://stdout", { id: agentId, line }))
    const ipcCalls: IpcCall[] = []

    mockIPC((cmd, payload) => {
        ipcCalls.push([cmd, payload])
        if (cmd === "agent_list") return []
        if (cmd === "agent_spawn") return agentId
        if (cmd === "agent_write") {
            return fake.write((payload as { chunk: string }).chunk)
        }
        if (cmd === "agent_kill") return undefined
        return undefined
    }, { shouldMockEvents: true })

    render(<AgentBridge />)

    await vi.waitFor(() => expect(useAgentStore.getState().connection).not.toBeNull())

    const turn = useAgentStore.getState().sendPrompt("/ws-a", "edit hello")

    await vi.waitFor(() => {
        expect(useAgentStore.getState().pendingPermissions.has("fake-session")).toBe(true)
    })

    useAgentStore.getState().respondPermission("fake-session", "allow")

    await expect(turn).resolves.toBe("end_turn")
    await vi.waitFor(() => {
        const session = useAgentStore.getState().sessions.get("fake-session")
        expect(session?.tone).toBe("done")
        expect(session?.availableCommands).toEqual([{ name: "fix", description: "Run fake fix" }])
        expect(session?.transcript.some((entry) => (
            "who" in entry && entry.who === "agent" && entry.text.includes("Ready")
        ))).toBe(true)
        expect(session?.transcript.some((entry) => (
            "kind" in entry && entry.kind === "diff" && entry.text === "hello.txt"
        ))).toBe(true)
    })
    expect(ipcCalls).toContainEqual([
        "agent_spawn",
        { command: "bunx pi-acp@0.0.31", cwd: "/ws-a" },
    ])
})

it("kills stale reload processes and clears volatile session state when no session id is persisted", async () => {
    const ipcCalls: IpcCall[] = []
    mockIPC((cmd, payload) => {
        ipcCalls.push([cmd, payload])
        if (cmd === "agent_list") return ["stale-agent"]
        if (cmd === "agent_kill") return undefined
        if (cmd === "agent_spawn") throw new Error("recovery must not spawn without a persisted session id")
        return undefined
    })
    useAgentStore.getState().upsertSessionMeta({ id: "old-session", cwd: "/ws-a", name: "Old session" })
    useAgentStore.getState().selectSession("old-session")

    render(<AgentBridge />)

    await vi.waitFor(() => {
        expect(ipcCalls).toContainEqual(["agent_list", { cwd: "/ws-a" }])
        expect(ipcCalls).toContainEqual(["agent_kill", { id: "stale-agent", reason: "app_exit" }])
    })
    await vi.waitFor(() => {
        const state = useAgentStore.getState()
        expect(state.sessions.size).toBe(0)
        expect(state.activeSessionId).toBeNull()
        expect(state.connection).not.toBeNull()
        expect(state.connectionState).toBe("ready")
    })
    expect(ipcCalls.some(([cmd]) => cmd === "agent_spawn")).toBe(false)
})

it("uses the persisted custom agent command when spawning ACP", async () => {
    localStorage.setItem(
        AGENT_SETTINGS_STORAGE_KEY,
        JSON.stringify({ preset: "custom", command: "uvx custom-acp", traceEnabled: false })
    )
    const agentId = "agent-custom"
    const fake = createFakeAcpAgentBridge((line) => emit("agent://stdout", { id: agentId, line }))
    const ipcCalls: IpcCall[] = []

    mockIPC((cmd, payload) => {
        ipcCalls.push([cmd, payload])
        if (cmd === "agent_list") return []
        if (cmd === "agent_spawn") return agentId
        if (cmd === "agent_write") {
            return fake.write((payload as { chunk: string }).chunk)
        }
        if (cmd === "agent_kill") return undefined
        if (cmd === "agent_set_trace") return undefined
        return undefined
    }, { shouldMockEvents: true })

    render(<AgentBridge />)

    await vi.waitFor(() => expect(useAgentStore.getState().connection).not.toBeNull())
    const turn = useAgentStore.getState().sendPrompt("/ws-a", "edit hello")

    await vi.waitFor(() => {
        expect(useAgentStore.getState().pendingPermissions.has("fake-session")).toBe(true)
    })
    useAgentStore.getState().respondPermission("fake-session", "allow")

    await expect(turn).resolves.toBe("end_turn")

    expect(ipcCalls).toContainEqual(["agent_spawn", { command: "uvx custom-acp", cwd: "/ws-a" }])
})

it("reads the latest persisted agent command at spawn time", async () => {
    const agentId = "agent-latest"
    const fake = createFakeAcpAgentBridge((line) => emit("agent://stdout", { id: agentId, line }))
    const ipcCalls: IpcCall[] = []

    mockIPC((cmd, payload) => {
        ipcCalls.push([cmd, payload])
        if (cmd === "agent_list") return []
        if (cmd === "agent_spawn") return agentId
        if (cmd === "agent_write") {
            return fake.write((payload as { chunk: string }).chunk)
        }
        if (cmd === "agent_kill") return undefined
        if (cmd === "agent_set_trace") return undefined
        return undefined
    }, { shouldMockEvents: true })

    render(<AgentBridge />)
    await vi.waitFor(() => expect(useAgentStore.getState().connection).not.toBeNull())

    localStorage.setItem(
        AGENT_SETTINGS_STORAGE_KEY,
        JSON.stringify({ preset: "custom", command: "bunx latest-acp", traceEnabled: false })
    )
    const turn = useAgentStore.getState().sendPrompt("/ws-a", "edit hello")
    await vi.waitFor(() => {
        expect(useAgentStore.getState().pendingPermissions.has("fake-session")).toBe(true)
    })
    useAgentStore.getState().respondPermission("fake-session", "allow")

    await expect(turn).resolves.toBe("end_turn")

    expect(ipcCalls).toContainEqual(["agent_spawn", { command: "bunx latest-acp", cwd: "/ws-a" }])
})

it("syncs persisted ACP trace on AgentBridge mount", async () => {
    localStorage.setItem(
        AGENT_SETTINGS_STORAGE_KEY,
        JSON.stringify({ preset: "pi", command: "bunx pi-acp", traceEnabled: true })
    )
    const ipcCalls: IpcCall[] = []
    mockIPC((cmd, payload) => {
        ipcCalls.push([cmd, payload])
        if (cmd === "agent_list") return []
        return undefined
    })

    render(<AgentBridge />)

    await vi.waitFor(() =>
        expect(ipcCalls).toContainEqual(["agent_set_trace", { enabled: true }])
    )
})

it("waits for persisted ACP trace sync before exposing the first spawn path", async () => {
    localStorage.setItem(
        AGENT_SETTINGS_STORAGE_KEY,
        JSON.stringify({ preset: "pi", command: "bunx pi-acp", traceEnabled: true })
    )
    const agentId = "agent-trace-first"
    const fake = createFakeAcpAgentBridge((line) => emit("agent://stdout", { id: agentId, line }))
    const trace = deferred()
    const order: string[] = []

    mockIPC((cmd, payload) => {
        if (cmd === "agent_list") return []
        if (cmd === "agent_set_trace") {
            order.push("agent_set_trace:start")
            return trace.promise.then(() => {
                order.push("agent_set_trace:done")
            })
        }
        if (cmd === "agent_spawn") {
            order.push("agent_spawn")
            return agentId
        }
        if (cmd === "agent_write") {
            return fake.write((payload as { chunk: string }).chunk)
        }
        if (cmd === "agent_kill") return undefined
        return undefined
    }, { shouldMockEvents: true })

    render(<AgentBridge />)

    await vi.waitFor(() => expect(order).toContain("agent_set_trace:start"))
    expect(useAgentStore.getState().connection).toBeNull()
    expect(order).not.toContain("agent_spawn")

    trace.resolve()
    await vi.waitFor(() => expect(useAgentStore.getState().connection).not.toBeNull())

    const turn = useAgentStore.getState().sendPrompt("/ws-a", "edit hello")
    await vi.waitFor(() => {
        expect(useAgentStore.getState().pendingPermissions.has("fake-session")).toBe(true)
    })
    useAgentStore.getState().respondPermission("fake-session", "allow")

    await expect(turn).resolves.toBe("end_turn")
    expect(order.indexOf("agent_set_trace:done")).toBeLessThan(order.indexOf("agent_spawn"))
})
