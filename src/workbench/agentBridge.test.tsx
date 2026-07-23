import { StrictMode } from "react"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { cleanup, render } from "@testing-library/react"
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks"
import { emit } from "@tauri-apps/api/event"

import { AGENT_SETTINGS_STORAGE_KEY } from "../app/workbench/SettingsDialog"
import { createFakeAcpAgentBridge } from "../agent/fakeAcpAgent"
import { agentInitialState, useAgentStore } from "../state/agentStore"
import { loadSessionIndex, upsertSessionIndexEntry } from "../state/sessionIndexStorage"
import { useWorkspaceStore } from "../state/workspaceStore"
import { AgentBridge } from "./AgentBridge"

type IpcCall = [string, unknown]
let idleCallbacks = new Map<number, IdleRequestCallback>()
let nextIdleId = 1

function installIdleQueue(): void {
    idleCallbacks = new Map()
    nextIdleId = 1
    Object.defineProperty(globalThis, "requestIdleCallback", {
        value: (callback: IdleRequestCallback) => {
            const id = nextIdleId++
            idleCallbacks.set(id, callback)
            return id
        },
        configurable: true,
        writable: true
    })
    Object.defineProperty(globalThis, "cancelIdleCallback", {
        value: (id: number) => void idleCallbacks.delete(id),
        configurable: true,
        writable: true
    })
}

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
    installIdleQueue()
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
        // diff content 不再另立 diff 卡（view/apply diff 已移除）；以 [diff: path] 併入 tool 內文。
        expect(session?.transcript.some((entry) => (
            "kind" in entry && entry.kind === "tool" && entry.text.includes("[diff: hello.txt]")
        ))).toBe(true)
    })
    expect(ipcCalls).toContainEqual([
        "agent_spawn",
        { command: "bunx pi-acp@latest", cwd: "/ws-a" },
    ])
})

it("P3: keeps the user prompt in the transcript after agent chunks overwrite it", async () => {
    const agentId = "agent-seed"
    const fake = createFakeAcpAgentBridge((line) => emit("agent://stdout", { id: agentId, line }))
    mockIPC((cmd, payload) => {
        if (cmd === "agent_list") return []
        if (cmd === "agent_spawn") return agentId
        if (cmd === "agent_write") return fake.write((payload as { chunk: string }).chunk)
        if (cmd === "agent_kill") return undefined
        return undefined
    }, { shouldMockEvents: true })
    render(<AgentBridge />)
    await vi.waitFor(() => expect(useAgentStore.getState().connection).not.toBeNull())
    const turn = useAgentStore.getState().sendPrompt("/ws-a", "edit hello")
    await vi.waitFor(() => expect(useAgentStore.getState().pendingPermissions.has("fake-session")).toBe(true))
    useAgentStore.getState().respondPermission("fake-session", "allow")
    await expect(turn).resolves.toBe("end_turn")
    const transcript = useAgentStore.getState().sessions.get("fake-session")!.transcript
    expect(transcript.some((e) => "who" in e && e.who === "you" && e.text === "edit hello")).toBe(true)
})

it("P9: newSession stores pi startup info as the session banner", async () => {
    const agentId = "agent-info"
    const fake = createFakeAcpAgentBridge((line) => emit("agent://stdout", { id: agentId, line }))
    mockIPC((cmd, payload) => {
        if (cmd === "agent_list") return []
        if (cmd === "agent_spawn") return agentId
        if (cmd === "agent_write") return fake.write((payload as { chunk: string }).chunk)
        if (cmd === "agent_kill") return undefined
        return undefined
    }, { shouldMockEvents: true })
    render(<AgentBridge />)
    await vi.waitFor(() => expect(useAgentStore.getState().connection).not.toBeNull())
    await useAgentStore.getState().newSession("/ws-a")
    expect(useAgentStore.getState().sessions.get("fake-session")?.infoBanner).toContain("pi 0.0.31")
})

it("threads usage, title, and authoritative config notifications through to the agent store", async () => {
    const agentId = "agent-usage"
    const fake = createFakeAcpAgentBridge((line) => emit("agent://stdout", { id: agentId, line }))
    mockIPC((cmd, payload) => {
        if (cmd === "agent_list") return []
        if (cmd === "agent_spawn") return agentId
        if (cmd === "agent_write") return fake.write((payload as { chunk: string }).chunk)
        if (cmd === "agent_kill") return undefined
        return undefined
    }, { shouldMockEvents: true })

    render(<AgentBridge />)
    await vi.waitFor(() => expect(useAgentStore.getState().connection).not.toBeNull())

    await useAgentStore.getState().newSession("/ws-a")
    expect(useAgentStore.getState().activeSessionId).toBe("fake-session")

    await fake.emitSessionUpdate({ sessionUpdate: "usage_update", used: 42, size: 500 })
    await fake.emitSessionUpdate({ sessionUpdate: "session_info_update", title: "Fix login bug" })
    await fake.emitSessionUpdate({
        sessionUpdate: "config_option_update",
        configOptions: [{
            id: "bridge-effort",
            name: "Effort",
            category: "thought_level",
            type: "select",
            currentValue: "high",
            options: [{ value: "high", name: "High" }]
        }]
    })

    await vi.waitFor(() => {
        const session = useAgentStore.getState().sessions.get("fake-session")
        expect(session?.usage).toEqual({ used: 42, size: 500 })
        expect(session?.title).toBe("Fix login bug")
        expect(session?.agentTitle).toBe("Fix login bug")
        expect(session?.configOptions).toEqual([{
            id: "bridge-effort",
            name: "Effort",
            category: "thought_level",
            type: "select",
            currentValue: "high",
            options: [{ value: "high", name: "High" }]
        }])
        expect(session?.configRevision).toBe(2)
    })
})

it("reaps true orphans once but preserves existing sessions (no reset)", async () => {
    const killed: string[] = []
    mockIPC((cmd, payload) => {
        if (cmd === "agent_list") return ["stale-agent"]
        if (cmd === "agent_kill") {
            killed.push((payload as { id: string }).id)
            return undefined
        }
        return undefined
    })
    useAgentStore.getState().upsertSessionMeta({ id: "old-session", cwd: "/ws-a", name: "Old" })
    render(<AgentBridge />)
    await vi.waitFor(() => expect(killed).toContain("stale-agent"))
    await vi.waitFor(() => expect(useAgentStore.getState().sessions.has("old-session")).toBe(true))
})

it("does not re-run recovery when switching A→B→A", async () => {
    const listCwds: string[] = []
    mockIPC((cmd, payload) => {
        if (cmd === "agent_list") {
            listCwds.push((payload as { cwd: string }).cwd)
            return []
        }
        if (cmd === "agent_kill") throw new Error("must not kill on workspace switch")
        return undefined
    })
    const { rerender } = render(<AgentBridge />)
    await vi.waitFor(() => expect(listCwds.length).toBe(1))
    useWorkspaceStore.setState({ workspacePath: "/ws-b" })
    rerender(<AgentBridge />)
    useWorkspaceStore.setState({ workspacePath: "/ws-a" })
    rerender(<AgentBridge />)
    expect(listCwds).toEqual(["/ws-a"])
})

it("hydrates the newly active workspace's Session Index after switching workspaces — fixes F5", async () => {
    upsertSessionIndexEntry({
        sessionId: "restored-b",
        cwd: "/ws-b",
        createdAt: 1,
        lastActiveAt: 1
    })
    const listCwds: string[] = []
    mockIPC((cmd, payload) => {
        if (cmd === "agent_list") {
            listCwds.push((payload as { cwd: string }).cwd)
            return []
        }
        return undefined
    })

    const { rerender } = render(<AgentBridge />)
    await vi.waitFor(() => expect(useAgentStore.getState().connection).not.toBeNull())
    await vi.waitFor(() => expect(listCwds).toContain("/ws-a"))

    useWorkspaceStore.setState({ workspacePath: "/ws-b" })
    rerender(<AgentBridge />)

    await vi.waitFor(() => {
        expect(useAgentStore.getState().sessions.get("restored-b")?.restored).toBe(true)
    })
    // 孤兒回收只跑過一次（第一個 workspace），切到 /ws-b 不應再打一次 agent_list。
    expect(listCwds).toEqual(["/ws-a"])
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

it("F3: StrictMode 雙掛載下 startup recovery 仍會完成一次（stale agent 被 kill）", async () => {
    const killed: string[] = []
    mockIPC((cmd, payload) => {
        if (cmd === "agent_list") return ["stale-agent"]
        if (cmd === "agent_kill") {
            killed.push((payload as { id: string }).id)
            return undefined
        }
        return undefined
    })

    render(
        <StrictMode>
            <AgentBridge />
        </StrictMode>
    )

    await vi.waitFor(() => expect(killed).toContain("stale-agent"))
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

it("hydrates recovery state without spawning a background ACP process", async () => {
    upsertSessionIndexEntry({
        sessionId: "restored-a",
        cwd: "/ws-a",
        createdAt: 1,
        lastActiveAt: 1
    })
    const recovery = deferred<string[]>()
    const spawns: unknown[] = []
    mockIPC((cmd, payload) => {
        if (cmd === "agent_list") return recovery.promise
        if (cmd === "agent_spawn") {
            spawns.push(payload)
            return "unexpected-background-agent"
        }
        if (cmd === "agent_kill") return undefined
        return undefined
    }, { shouldMockEvents: true })

    render(<AgentBridge />)
    await vi.waitFor(() => expect(useAgentStore.getState().connection).not.toBeNull())
    expect(useAgentStore.getState().sessions.has("restored-a")).toBe(false)

    recovery.resolve([])
    await vi.waitFor(() => expect(useAgentStore.getState().sessions.get("restored-a")?.restored).toBe(true))
    await Promise.resolve()
    expect(spawns).toEqual([])
    expect(idleCallbacks.size).toBe(0)
})

it("Phase 4: Session Index survives a simulated restart and continueSession replays history, then resumes chatting", async () => {
    // 兩段各自的 fake agent bridge：一段代表「重啟前」的行程，一段代表「重啟後」
    // continueSession 重新 spawn 出來、宣告 loadSession 並 replay 歷史的行程。
    let spawnCounter = 0
    const bridgesById = new Map<string, ReturnType<typeof createFakeAcpAgentBridge>>()

    mockIPC((cmd, payload) => {
        if (cmd === "agent_list") return []
        if (cmd === "agent_spawn") {
            const id = `agent-${spawnCounter}`
            spawnCounter += 1
            const isSecondSpawn = spawnCounter === 2
            const bridge = createFakeAcpAgentBridge(
                (line) => emit("agent://stdout", { id, line }),
                isSecondSpawn
                    ? {
                        replayUpdates: [
                            { sessionUpdate: "user_message_chunk", content: { type: "text", text: "before restart" } },
                            { sessionUpdate: "agent_message_chunk", content: { type: "text", text: "reply before restart" } }
                        ]
                    }
                    : {}
            )
            bridgesById.set(id, bridge)
            return id
        }
        if (cmd === "agent_write") {
            const { id, chunk } = payload as { id: string; chunk: string }
            return bridgesById.get(id)!.write(chunk)
        }
        if (cmd === "agent_kill") return undefined
        return undefined
    }, { shouldMockEvents: true })

    // ── 重啟前：建立 session 並送出首個 prompt，promotion 後才落地 Index ──
    const first = render(<AgentBridge />)
    await vi.waitFor(() => expect(useAgentStore.getState().connection).not.toBeNull())
    await useAgentStore.getState().newSession("/ws-a")
    expect(loadSessionIndex()).toEqual([])
    const firstTurn = useAgentStore.getState().sendPrompt("/ws-a", "before restart")
    await vi.waitFor(() => expect(useAgentStore.getState().pendingPermissions.has("fake-session")).toBe(true))
    useAgentStore.getState().respondPermission("fake-session", "allow")
    await expect(firstTurn).resolves.toBe("end_turn")
    const sessionId = useAgentStore.getState().activeSessionId
    expect(sessionId).toBe("fake-session")
    expect(loadSessionIndex().some((entry) => entry.sessionId === sessionId && entry.cwd === "/ws-a")).toBe(true)

    first.unmount()

    // ── 模擬重啟：store 的 in-memory 狀態全部歸零，但不清 localStorage ──
    resetAgentStore()
    expect(useAgentStore.getState().sessions.size).toBe(0)

    render(<AgentBridge />)
    await vi.waitFor(() => expect(useAgentStore.getState().connection).not.toBeNull())
    await vi.waitFor(() => expect(useAgentStore.getState().sessions.get(sessionId!)?.restored).toBe(true))
    expect(useAgentStore.getState().activeSessionId).toBeNull() // hydrate 不動 activeSessionId

    // ── 點擊續聊：走 session/load replay ──
    await useAgentStore.getState().continueSession(sessionId!)

    await vi.waitFor(() => {
        const session = useAgentStore.getState().sessions.get(sessionId!)
        expect(session?.restored).toBe(false)
        expect(session?.transcript.some((e) => "who" in e && e.who === "you" && e.text === "before restart")).toBe(true)
        expect(session?.transcript.some((e) => "who" in e && e.who === "agent" && e.text.includes("reply before restart"))).toBe(true)
    })
    expect(useAgentStore.getState().activeSessionId).toBe(sessionId)

    // ── replay 完成後仍可送出新 prompt 續聊 ──
    const turn = useAgentStore.getState().sendPrompt("/ws-a", "continue please")
    await vi.waitFor(() => expect(useAgentStore.getState().pendingPermissions.has(sessionId!)).toBe(true))
    useAgentStore.getState().respondPermission(sessionId!, "allow")
    await expect(turn).resolves.toBe("end_turn")
})
